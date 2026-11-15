//! Coded local faults. Producers never accept messages, paths or setting values.
use crate::storage::StoreError;
use serde::Serialize;
use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Read, Seek, SeekFrom, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const FILES: usize = 5;
pub const MAX_BYTES: u64 = 2 * 1024 * 1024;
const QUEUE: usize = 128;
const MAX_RECORD: usize = 512;
const LOCK_WAIT: Duration = Duration::from_millis(100);
const CLOSE_WAIT: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Fault {
    ConfigurationRead,
    ConfigurationWrite,
    ConfigurationRecovered,
    ConfigurationUnknown,
    ConfigurationWrongType,
    ConfigurationAdjusted,
    ConfigurationInvalidValue,
    CacheRejected,
    ExitMarker,
    PolicyRead,
    InstanceGuard,
    IpcStart,
    TrayCreate,
    TrayAdd,
    TrayVersion,
    TrayUpdate,
    TrayDelete,
    TrayNotice,
    TrayMenu,
    PanelCreate,
    Graphics,
    SystemEvents,
    MonitorDiscovery,
    DiagnosticsStart,
    StateTransition,
    ClockRead,
    CursorPosition,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageCode {
    Io,
    Configuration,
    InvalidCache,
    CacheExpired,
    ClockRollback,
    WrongEndpoint,
    Session,
}
#[derive(Clone, Copy, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Code {
    Hresult(i32),
    Os(i32),
    Storage(StorageCode),
    Count(u32),
    Unavailable,
}
impl Code {
    pub fn io(error: &io::Error) -> Self {
        error
            .raw_os_error()
            .map_or(Self::Storage(StorageCode::Io), Self::Os)
    }
    pub fn storage(error: &StoreError) -> Self {
        let code = match error {
            StoreError::Io(error) => return Self::io(error),
            StoreError::Config(_) => StorageCode::Configuration,
            StoreError::InvalidCache => StorageCode::InvalidCache,
            StoreError::CacheExpired => StorageCode::CacheExpired,
            StoreError::ClockRollback => StorageCode::ClockRollback,
            StoreError::WrongEndpoint => StorageCode::WrongEndpoint,
            StoreError::Session(_) => StorageCode::Session,
        };
        Self::Storage(code)
    }
}
#[derive(Serialize)]
struct Record {
    schema_version: u8,
    version: &'static str,
    timestamp_unix_ms: u64,
    fault: Fault,
    code: Code,
}
impl Record {
    fn new(fault: Fault, code: Code) -> Self {
        Self {
            schema_version: 1,
            version: env!("CARGO_PKG_VERSION"),
            timestamp_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
            fault,
            code,
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub worker_running: bool,
    pub enqueued: u64,
    pub written: u64,
    pub dropped: u64,
    pub write_errors: u64,
}
#[derive(Default)]
struct Counters {
    running: AtomicBool,
    enqueued: AtomicU64,
    written: AtomicU64,
    dropped: AtomicU64,
    write_errors: AtomicU64,
}
#[derive(Default)]
struct Stop {
    closing: AtomicBool,
    abort: AtomicBool,
}
pub struct Recorder {
    sender: Option<SyncSender<Record>>,
    worker: Option<JoinHandle<()>>,
    counters: Arc<Counters>,
    stop: Arc<Stop>,
}
impl Recorder {
    /// File creation and writes take place only on the owned worker, on fault.
    pub fn start(directory: PathBuf) -> Arc<Self> {
        let (sender, receiver) = mpsc::sync_channel::<Record>(QUEUE);
        let counters = Arc::new(Counters::default());
        let stats = counters.clone();
        let stop = Arc::new(Stop::default());
        let worker_stop = stop.clone();
        counters.running.store(true, Ordering::Release);
        let worker = thread::Builder::new()
            .name("snowflakes-errors".into())
            .spawn(move || {
                struct Running(Arc<Counters>);
                impl Drop for Running {
                    fn drop(&mut self) {
                        self.0.running.store(false, Ordering::Release);
                    }
                }
                let _running = Running(stats.clone());
                let sink = Sink {
                    directory,
                    limit: MAX_BYTES,
                };
                for record in &receiver {
                    if worker_stop.abort.load(Ordering::Acquire) {
                        stats
                            .dropped
                            .fetch_add(1 + receiver.try_iter().count() as u64, Ordering::Relaxed);
                        break;
                    }
                    match sink.append(&record, &worker_stop) {
                        Ok(()) => {
                            stats.written.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(_) => {
                            stats.write_errors.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            });
        let worker = match worker {
            Ok(worker) => Some(worker),
            Err(_) => {
                counters.running.store(false, Ordering::Release);
                counters.write_errors.fetch_add(1, Ordering::Relaxed);
                None
            }
        };
        Arc::new(Self {
            sender: Some(sender),
            worker,
            counters,
            stop,
        })
    }
    pub fn record(&self, fault: Fault, code: Code) {
        if self
            .sender
            .as_ref()
            .is_some_and(|sender| sender.try_send(Record::new(fault, code)).is_ok())
        {
            self.counters.enqueued.fetch_add(1, Ordering::Relaxed);
        } else {
            self.counters.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn storage(&self, fault: Fault, error: &StoreError) {
        self.record(fault, Code::storage(error));
    }
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            worker_running: self.counters.running.load(Ordering::Acquire),
            enqueued: self.counters.enqueued.load(Ordering::Relaxed),
            written: self.counters.written.load(Ordering::Relaxed),
            dropped: self.counters.dropped.load(Ordering::Relaxed),
            write_errors: self.counters.write_errors.load(Ordering::Relaxed),
        }
    }
}
impl Drop for Recorder {
    fn drop(&mut self) {
        self.stop.closing.store(true, Ordering::Release);
        drop(self.sender.take());
        if let Some(worker) = self.worker.take() {
            let deadline = Instant::now() + CLOSE_WAIT;
            while !worker.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
            if worker.is_finished() {
                let _ = worker.join();
            } else {
                // No forced thread termination or indefinite wait for a stalled
                // filesystem. The worker stops at its next safe boundary.
                self.stop.abort.store(true, Ordering::Release);
            }
        }
    }
}

struct Sink {
    directory: PathBuf,
    limit: u64,
}
impl Sink {
    fn path(&self, index: usize) -> PathBuf {
        self.directory.join(if index == 0 {
            "errors.jsonl".into()
        } else {
            format!("errors.{index}.jsonl")
        })
    }
    fn regular(&self, path: &std::path::Path, limited: bool) -> io::Result<()> {
        match fs::symlink_metadata(path) {
            Ok(meta) if !meta.file_type().is_file() || (limited && meta.len() > self.limit) => Err(
                io::Error::new(io::ErrorKind::InvalidData, "invalid log slot"),
            ),
            Ok(_) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
    fn append(&self, record: &Record, stop: &Stop) -> io::Result<()> {
        let mut line = serde_json::to_vec(record).map_err(io::Error::other)?;
        line.push(b'\n');
        if line.len() > MAX_RECORD || line.len() as u64 > self.limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "oversized log record",
            ));
        }
        fs::create_dir_all(&self.directory)?;
        let lock_path = self.directory.join(".errors.lock");
        self.regular(&lock_path, false)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        let deadline = Instant::now() + LOCK_WAIT;
        loop {
            if stop.abort.load(Ordering::Acquire) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            match lock.try_lock() {
                Ok(()) => break,
                Err(TryLockError::Error(error)) => return Err(error),
                Err(TryLockError::WouldBlock) => {
                    if stop.closing.load(Ordering::Acquire) || Instant::now() >= deadline {
                        return Err(io::ErrorKind::WouldBlock.into());
                    }
                    thread::sleep(Duration::from_millis(5));
                }
            }
        }
        // All cooperating instances use this one stable lock file. Never hold
        // a rotated log handle across the move, and never move directories.
        for index in 0..FILES {
            self.regular(&self.path(index), true)?;
        }
        let open = || {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(self.path(0))
        };
        let mut file = open()?;
        trim_partial(&mut file)?;
        if file.metadata()?.len() + line.len() as u64 > self.limit {
            drop(file);
            match fs::remove_file(self.path(FILES - 1)) {
                Ok(()) => (),
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error),
            }
            for index in (1..FILES).rev() {
                match fs::rename(self.path(index - 1), self.path(index)) {
                    Ok(()) => (),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error),
                }
            }
            file = open()?;
        }
        file.seek(SeekFrom::End(0))?;
        file.write_all(&line)?;
        Ok(())
    }
}
fn trim_partial(file: &mut File) -> io::Result<()> {
    let size = file.metadata()?.len();
    if size == 0 {
        return Ok(());
    }
    let count = size.min(MAX_RECORD as u64);
    file.seek(SeekFrom::Start(size - count))?;
    let mut tail = vec![0; count as usize];
    file.read_exact(&mut tail)?;
    if tail.last() == Some(&b'\n') {
        return Ok(());
    }
    let length = match tail.iter().rposition(|&byte| byte == b'\n') {
        Some(index) => size - count + index as u64 + 1,
        None if size <= MAX_RECORD as u64 => 0,
        None => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid log tail",
            ));
        }
    };
    file.set_len(length)
}

#[cfg(test)]
mod tests;
