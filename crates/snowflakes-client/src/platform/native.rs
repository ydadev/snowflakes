//! Narrow Win32 ownership/IPC boundary. All pending I/O is cancelled and joined
//! before its OVERLAPPED, event or buffer may be dropped.
use crate::control::{Command, INTERNAL, INVALID, MAX_IPC_BYTES, Response, TIMEOUT};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
    time::Duration,
};
use windows::{
    Win32::{
        Foundation::*,
        Security::{Authorization::*, *},
        Storage::FileSystem::*,
        System::{IO::*, Pipes::*, RemoteDesktop::ProcessIdToSessionId, Threading::*},
    },
    core::{Error, HRESULT, PCWSTR, PWSTR, Result},
};

pub fn failure(code: WIN32_ERROR) -> Error {
    Error::from_hresult(HRESULT::from_win32(code.0))
}
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

pub struct Handle(pub HANDLE);
impl Handle {
    fn raw(&self) -> HANDLE {
        self.0
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}
// Kernel handles are thread-safe; ownership remains exclusive through this type.
unsafe impl Send for Handle {}

struct LocalMemory(HLOCAL);
impl Drop for LocalMemory {
    fn drop(&mut self) {
        unsafe {
            let _ = LocalFree(Some(self.0));
        }
    }
}

#[derive(Clone)]
pub struct Identity {
    pub sid: String,
    pub session_id: u32,
    pub logon_key: String,
    namespace: String,
}
impl Identity {
    pub fn current(namespace: &str) -> Result<Self> {
        if namespace.len() > 48
            || !namespace
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(failure(ERROR_INVALID_PARAMETER));
        }
        unsafe {
            let mut session_id = 0;
            ProcessIdToSessionId(GetCurrentProcessId(), &mut session_id)?;
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
            let token = Handle(token);
            let mut statistics = TOKEN_STATISTICS::default();
            let mut needed = 0;
            GetTokenInformation(
                token.0,
                TokenStatistics,
                Some((&mut statistics as *mut TOKEN_STATISTICS).cast()),
                size_of::<TOKEN_STATISTICS>() as u32,
                &mut needed,
            )?;
            let logon_key = format!(
                "{:08x}{:08x}",
                statistics.AuthenticationId.HighPart as u32, statistics.AuthenticationId.LowPart
            );
            Ok(Self {
                sid: process_sid(GetCurrentProcess())?,
                session_id,
                logon_key,
                namespace: namespace.into(),
            })
        }
    }
    pub fn pipe_name(&self) -> Vec<u16> {
        wide(&format!(
            r"\\.\pipe\Snowflakes-{}-{}-{}",
            self.session_id, self.sid, self.namespace
        ))
    }
    fn mutex_name(&self) -> Vec<u16> {
        wide(&format!(
            r"Local\Snowflakes-{}-{}-{}",
            self.session_id, self.sid, self.namespace
        ))
    }
}

fn process_sid(process: HANDLE) -> Result<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token)?;
        let token = Handle(token);
        let mut needed = 0;
        let _ = GetTokenInformation(token.0, TokenUser, None, 0, &mut needed);
        if needed == 0 || needed > 65536 {
            return Err(failure(ERROR_INVALID_DATA));
        }
        // usize allocation guarantees TOKEN_USER pointer alignment.
        let mut buffer = vec![0_usize; (needed as usize).div_ceil(size_of::<usize>())];
        GetTokenInformation(
            token.0,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )?;
        let user = &*(buffer.as_ptr().cast::<TOKEN_USER>());
        let mut text = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut text)?;
        let _memory = LocalMemory(HLOCAL(text.0.cast()));
        Ok(text.to_string()?)
    }
}

struct Security {
    descriptor: PSECURITY_DESCRIPTOR,
    _memory: LocalMemory,
}
impl Security {
    fn new(identity: &Identity) -> Result<Self> {
        let text = wide(&format!(
            "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{})",
            identity.sid
        ));
        unsafe {
            let mut descriptor = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(text.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )?;
            Ok(Self {
                descriptor,
                _memory: LocalMemory(HLOCAL(descriptor.0)),
            })
        }
    }
    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor.0,
            bInheritHandle: false.into(),
        }
    }
}

pub struct Instance {
    handle: Handle,
    owns: bool,
}
impl Instance {
    pub fn acquire(identity: &Identity) -> Result<Self> {
        let security = Security::new(identity)?;
        let name = identity.mutex_name();
        unsafe {
            let handle = Handle(CreateMutexW(
                Some(&security.attributes()),
                true,
                PCWSTR(name.as_ptr()),
            )?);
            let owns = if GetLastError() != ERROR_ALREADY_EXISTS {
                true
            } else {
                matches!(
                    WaitForSingleObject(handle.0, 0),
                    WAIT_OBJECT_0 | WAIT_ABANDONED
                )
            };
            Ok(Self { handle, owns })
        }
    }
    pub fn owns(&self) -> bool {
        self.owns
    }
}
impl Drop for Instance {
    fn drop(&mut self) {
        if self.owns {
            unsafe {
                let _ = ReleaseMutex(self.handle.0);
            }
        }
    }
}

fn operation(
    handle: HANDLE,
    timeout_ms: u32,
    action: impl FnOnce(*mut OVERLAPPED, *mut u32) -> Result<()>,
) -> Result<u32> {
    unsafe {
        let event = Handle(CreateEventW(None, true, false, None)?);
        let mut overlapped = OVERLAPPED {
            hEvent: event.0,
            ..Default::default()
        };
        let mut count = 0;
        match action(&mut overlapped, &mut count) {
            Ok(()) => Ok(count),
            Err(error) if error.code() == HRESULT::from_win32(ERROR_PIPE_CONNECTED.0) => Ok(0),
            Err(error) if error.code() == HRESULT::from_win32(ERROR_IO_PENDING.0) => {
                if WaitForSingleObject(event.0, timeout_ms) != WAIT_OBJECT_0 {
                    let _ = CancelIoEx(handle, Some(&overlapped));
                    // Kernel still owns the pointers until cancellation completes.
                    let _ = GetOverlappedResult(handle, &overlapped, &mut count, true);
                    return Err(failure(ERROR_TIMEOUT));
                }
                GetOverlappedResult(handle, &overlapped, &mut count, false)?;
                Ok(count)
            }
            Err(error) => Err(error),
        }
    }
}
fn read(handle: HANDLE, limit: usize, timeout_ms: u32) -> Result<Vec<u8>> {
    let mut bytes = vec![0; limit];
    let count = operation(handle, timeout_ms, |overlapped, count| unsafe {
        ReadFile(handle, Some(&mut bytes), Some(count), Some(overlapped))
    })?;
    bytes.truncate(count as usize);
    Ok(bytes)
}
fn write(handle: HANDLE, bytes: &[u8]) -> Result<()> {
    let count = operation(handle, 2000, |overlapped, count| unsafe {
        WriteFile(handle, Some(bytes), Some(count), Some(overlapped))
    })?;
    if count as usize != bytes.len() {
        return Err(failure(ERROR_WRITE_FAULT));
    }
    Ok(())
}

pub struct Envelope {
    pub command: Command,
    pub reply: SyncSender<Response>,
    pub deadline: std::time::Instant,
    pub trace: Option<crate::diagnostics::Trace>,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema_version: u32,
    command: Command,
}

pub struct Server {
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Server {
    pub fn start(identity: &Identity, outgoing: SyncSender<Envelope>) -> Result<Self> {
        let security = Security::new(identity)?;
        let name = identity.pipe_name();
        let pipe = unsafe {
            CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                MAX_IPC_BYTES as u32,
                MAX_IPC_BYTES as u32,
                1000,
                Some(&security.attributes()),
            )
        };
        if pipe.is_invalid() {
            return Err(Error::from_thread());
        }
        let pipe = Handle(pipe);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::spawn(move || {
            while !worker_stop.load(Ordering::Acquire) {
                let connected = operation(pipe.raw(), 250, |overlapped, _| unsafe {
                    ConnectNamedPipe(pipe.raw(), Some(overlapped))
                });
                if connected.is_err() {
                    unsafe {
                        let _ = DisconnectNamedPipe(pipe.raw());
                    }
                    continue;
                }
                if let Ok(bytes) = read(pipe.raw(), MAX_IPC_BYTES, 2000) {
                    let response =
                        match serde_json::from_slice::<Request>(&bytes) {
                            Ok(Request {
                                schema_version: 1,
                                command,
                            }) => {
                                let (reply, result) = mpsc::sync_channel(1);
                                if outgoing
                                    .try_send(Envelope {
                                        command,
                                        reply,
                                        deadline: std::time::Instant::now()
                                            + Duration::from_millis(1500),
                                        trace: None,
                                    })
                                    .is_ok()
                                {
                                    result.recv_timeout(Duration::from_secs(2)).unwrap_or_else(
                                        |_| Response::error(TIMEOUT, "dispatch_timeout"),
                                    )
                                } else {
                                    Response::error(INTERNAL, "instance_busy")
                                }
                            }
                            _ => Response::error(INVALID, "invalid_message"),
                        };
                    if let Ok(bytes) = serde_json::to_vec(&response)
                        && bytes.len() <= MAX_IPC_BYTES
                        && write(pipe.raw(), &bytes).is_ok()
                    {
                        // Bounded acknowledgement replaces blocking FlushFileBuffers.
                        let _ = read(pipe.raw(), 1, 2000);
                    }
                }
                unsafe {
                    let _ = DisconnectNamedPipe(pipe.raw());
                }
            }
        });
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub fn send(identity: &Identity, command: &Command) -> Result<Response> {
    let name = identity.pipe_name();
    unsafe {
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        let handle = loop {
            match CreateFileW(
                PCWSTR(name.as_ptr()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                None,
            ) {
                Ok(handle) => break Handle(handle),
                Err(error) if error.code() == HRESULT::from_win32(ERROR_PIPE_BUSY.0) => {
                    if std::time::Instant::now() >= deadline {
                        return Err(failure(ERROR_TIMEOUT));
                    }
                    let _ = WaitNamedPipeW(PCWSTR(name.as_ptr()), 250);
                }
                Err(error) => return Err(error),
            }
        };
        let mut server_pid = 0;
        GetNamedPipeServerProcessId(handle.0, &mut server_pid)?;
        let mut session = 0;
        ProcessIdToSessionId(server_pid, &mut session)?;
        let server = Handle(OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            server_pid,
        )?);
        if session != identity.session_id || process_sid(server.0)? != identity.sid {
            return Err(failure(ERROR_ACCESS_DENIED));
        }
        SetNamedPipeHandleState(handle.0, Some(&PIPE_READMODE_MESSAGE), None, None)?;
        let bytes = serde_json::to_vec(&Request {
            schema_version: 1,
            command: command.clone(),
        })
        .map_err(|_| failure(ERROR_INVALID_DATA))?;
        if bytes.len() > MAX_IPC_BYTES {
            return Err(failure(ERROR_INVALID_DATA));
        }
        write(handle.0, &bytes)?;
        let bytes = read(handle.0, MAX_IPC_BYTES, 3000)?;
        let response = serde_json::from_slice(&bytes).map_err(|_| failure(ERROR_INVALID_DATA))?;
        write(handle.0, &[6])?;
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity(label: &str) -> Identity {
        static IDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        Identity::current(&format!(
            "{}-{}-{}",
            label,
            std::process::id(),
            IDS.fetch_add(1, Ordering::Relaxed)
        ))
        .unwrap()
    }
    #[test]
    fn mutex_denies_a_second_thread_and_recovers_an_abandoned_owner() {
        let identity = identity("mutex");
        let owner = Instance::acquire(&identity).unwrap();
        assert!(owner.owns());
        let other = identity.clone();
        assert!(
            !thread::spawn(move || Instance::acquire(&other).unwrap().owns())
                .join()
                .unwrap()
        );
        drop(owner);
        let abandoned = identity.clone();
        let leaked = thread::spawn(move || {
            let guard = Instance::acquire(&abandoned).unwrap();
            assert!(guard.owns());
            let raw = guard.handle.0.0 as usize;
            std::mem::forget(guard); // deliberately model a crashed owning thread
            raw
        })
        .join()
        .unwrap();
        let leaked_handle = Handle(HANDLE(leaked as *mut core::ffi::c_void));
        let recovered = Instance::acquire(&identity).unwrap();
        assert!(recovered.owns());
        drop(recovered);
        drop(leaked_handle);
    }

    #[test]
    fn explicit_pipe_acl_denies_even_anonymous_read_access() {
        let identity = identity("acl");
        let (sender, _receiver) = mpsc::sync_channel(1);
        let server = Server::start(&identity, sender).unwrap();
        struct Revert;
        impl Drop for Revert {
            fn drop(&mut self) {
                unsafe {
                    RevertToSelf().unwrap();
                }
            }
        }
        unsafe {
            ImpersonateAnonymousToken(GetCurrentThread()).unwrap();
        }
        let revert = Revert;
        let name = identity.pipe_name();
        let attempt = unsafe {
            CreateFileW(
                PCWSTR(name.as_ptr()),
                GENERIC_READ.0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                None,
            )
        };
        drop(revert);
        let denied = match attempt {
            Ok(handle) => {
                drop(Handle(handle));
                false
            }
            Err(error) => error.code() == HRESULT::from_win32(ERROR_ACCESS_DENIED.0),
        };
        drop(server);
        assert!(denied);
    }

    #[test]
    fn unresponsive_client_cannot_hold_shutdown_and_invalid_wire_never_dispatches() {
        let identity = identity("timeout");
        let (sender, receiver) = mpsc::sync_channel(1);
        let server = Server::start(&identity, sender).unwrap();
        let name = identity.pipe_name();
        let client = unsafe {
            Handle(
                CreateFileW(
                    PCWSTR(name.as_ptr()),
                    GENERIC_READ.0 | GENERIC_WRITE.0,
                    FILE_SHARE_MODE(0),
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED,
                    None,
                )
                .unwrap(),
            )
        };
        unsafe {
            SetNamedPipeHandleState(client.0, Some(&PIPE_READMODE_MESSAGE), None, None).unwrap();
        }
        write(
            client.0,
            br#"{"schema_version":2,"command":{"command":"quit"}}"#,
        )
        .unwrap();
        let response: Response =
            serde_json::from_slice(&read(client.0, MAX_IPC_BYTES, 3000).unwrap()).unwrap();
        assert_eq!(response.code, INVALID);
        assert!(receiver.try_recv().is_err());
        // Do not acknowledge: the server must time out instead of blocking forever.
        let started = std::time::Instant::now();
        drop(server);
        assert!(started.elapsed() < Duration::from_secs(4));
    }
}
