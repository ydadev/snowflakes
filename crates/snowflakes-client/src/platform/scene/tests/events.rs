//! Real cross-process WinEvents, on a desktop that is never made the input desktop.
use super::super::*;
use std::{
    io::{BufRead, BufReader, Write},
    os::windows::process::CommandExt,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
};
use windows::{
    Win32::System::{StationsAndDesktops::*, Threading::*},
    core::w,
};

const CHILD: &str = "platform::scene::tests::events::hidden_window_event_child";
const DESKTOP_ENV: &str = "SNOWFLAKES_WINEVENT_TEST_DESKTOP";
const ACCESS: u32 = DESKTOP_CREATEWINDOW.0
    | DESKTOP_ENUMERATE.0
    | DESKTOP_HOOKCONTROL.0
    | DESKTOP_READOBJECTS.0
    | DESKTOP_WRITEOBJECTS.0;

struct TestDesktop {
    original: HDESK,
    owned: HDESK,
}
impl TestDesktop {
    fn attach(name: &str, create: bool) -> Self {
        let name: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
        unsafe {
            let original = GetThreadDesktop(GetCurrentThreadId()).unwrap();
            let owned = if create {
                CreateDesktopW(
                    PCWSTR(name.as_ptr()),
                    PCWSTR::null(),
                    None,
                    Default::default(),
                    ACCESS,
                    None,
                )
            } else {
                OpenDesktopW(PCWSTR(name.as_ptr()), Default::default(), false, ACCESS)
            }
            .unwrap();
            if let Err(error) = SetThreadDesktop(owned) {
                let _ = CloseDesktop(owned);
                panic!("cannot attach isolated test desktop: {:?}", error.code());
            }
            Self { original, owned }
        }
    }
}
impl Drop for TestDesktop {
    fn drop(&mut self) {
        unsafe {
            let restored = SetThreadDesktop(self.original);
            let closed = CloseDesktop(self.owned);
            if !std::thread::panicking() {
                restored.unwrap();
                closed.unwrap();
            }
        }
    }
}

struct Window(HWND);
impl Window {
    fn new(x: i32) -> Self {
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                w!("STATIC"),
                w!(""),
                WS_POPUP,
                x,
                100,
                160,
                80,
                None,
                None,
                None,
                None,
            )
            .unwrap()
        };
        let window = Self(hwnd);
        window.check();
        window
    }
    fn check(&self) {
        let mut pid = 0;
        unsafe {
            assert_eq!(
                GetWindowThreadProcessId(self.0, Some(&mut pid)),
                GetCurrentThreadId()
            );
            assert_eq!(pid, GetCurrentProcessId());
            assert!(!IsWindowVisible(self.0).as_bool());
        }
    }
    fn move_to(&self, x: i32, y: i32) {
        self.check();
        unsafe {
            SetWindowPos(
                self.0,
                None,
                x,
                y,
                0,
                0,
                SWP_NOZORDER | SWP_NOSIZE | SWP_NOACTIVATE,
            )
            .unwrap();
        }
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        self.check();
        unsafe {
            DestroyWindow(self.0).unwrap();
        }
    }
}

fn reply(value: &str) {
    println!("SF_EVENT {value}");
    std::io::stdout().flush().unwrap();
}

#[test]
#[ignore = "subprocess entry point; invoked only by the native WinEvent test"]
fn hidden_window_event_child() {
    let Some(name) = std::env::var_os(DESKTOP_ENV) else {
        return;
    };
    let _desktop = TestDesktop::attach(&name.to_string_lossy(), false);
    let mut windows = Vec::new();
    reply("ready");
    for command in std::io::stdin().lock().lines() {
        match command.unwrap().as_str() {
            "create" => {
                windows.push(Window::new(100));
                windows.push(Window::new(320));
                reply(&format!(
                    "created {} {}",
                    windows[0].0.0 as usize, windows[1].0.0 as usize
                ));
            }
            "move" => {
                for index in 0..32 {
                    windows[0].move_to(100 + index * 4, 120 + index * 2);
                }
                reply("moved");
            }
            "reorder" => {
                windows[0].check();
                unsafe {
                    SetWindowPos(
                        windows[0].0,
                        Some(HWND_TOP),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    )
                    .unwrap();
                }
                reply("reordered");
            }
            "close" => {
                windows.remove(0);
                reply("closed");
            }
            "quit" => {
                windows.clear();
                reply("bye");
                return;
            }
            _ => panic!("unexpected test command"),
        }
    }
}

struct Host {
    child: Child,
    input: ChildStdin,
    output: Receiver<String>,
    reader: Option<std::thread::JoinHandle<()>>,
}
impl Host {
    fn new(desktop: &str) -> Self {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                CHILD,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(DESKTOP_ENV, desktop)
            .creation_flags(CREATE_NO_WINDOW.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, output) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout)
                .lines()
                .map_while(std::result::Result::ok)
            {
                if let Some((_, value)) = line.split_once("SF_EVENT ")
                    && send.send(value.to_owned()).is_err()
                {
                    break;
                }
            }
        });
        let mut host = Self {
            child,
            input,
            output,
            reader: Some(reader),
        };
        assert_eq!(host.receive(), "ready");
        host
    }
    fn receive(&mut self) -> String {
        self.output
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|error| {
                panic!(
                    "test child response failed: {error:?}; exit={:?}",
                    self.child.try_wait().unwrap()
                );
            })
    }
    fn command(&mut self, value: &str) -> String {
        writeln!(self.input, "{value}").unwrap();
        self.input.flush().unwrap();
        self.receive()
    }
    fn finish(&mut self) {
        assert_eq!(self.command("quit"), "bye");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "test child did not cleanly exit");
                break;
            }
            assert!(Instant::now() < deadline, "test child cleanup timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn pump() {
    // Bounded wall time is only for asynchronous delivery. Registry time below
    // stays under one second so periodic reconciliation cannot hide a lost event.
    let deadline = Instant::now() + Duration::from_millis(150);
    loop {
        unsafe {
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

struct FixtureSource {
    handles: Vec<usize>,
    pid: u32,
    reads: Vec<usize>,
    orders: usize,
}
impl Source for FixtureSource {
    type Error = windows::core::Error;
    fn order(&mut self) -> Result<Vec<usize>> {
        self.orders += 1;
        Ok(Native::default()
            .order()?
            .into_iter()
            .filter(|h| self.handles.contains(h))
            .collect())
    }
    fn read(&mut self, handle: usize) -> Option<WindowInfo> {
        assert!(self.handles.contains(&handle));
        self.reads.push(handle);
        let hwnd = HWND(handle as *mut _);
        let mut pid = 0;
        let thread = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if thread == 0 {
            return None;
        }
        assert_eq!(pid, self.pid);
        assert!(!unsafe { IsWindowVisible(hwnd) }.as_bool());
        let mut bounds = RECT::default();
        unsafe {
            GetWindowRect(hwnd, &mut bounds).unwrap();
        }
        // Native::read deliberately rejects hidden windows. Only this fixture
        // Source admits their numeric geometry; no production filter is weakened.
        Some(WindowInfo {
            handle,
            process: pid,
            thread,
            rect: rect(bounds),
            visible: true,
            collect: true,
            maximized: false,
            taskbar: false,
        })
    }
    fn screens(&mut self) -> Result<Vec<Screen>> {
        Ok(vec![Screen {
            key: 1,
            persistent_id: true,
            rect: Rect {
                left: 0.0,
                top: 0.0,
                right: 800.0,
                bottom: 600.0,
            },
            floor: 600.0,
            scale: 1.0,
        }])
    }
    fn foreground(&mut self) -> usize {
        0
    }
}

fn poll(tracker: &mut Tracker, source: &mut FixtureSource, millis: u64) -> Option<Scene> {
    tracker
        .registry
        .changes
        .merge(CHANGES.with_borrow_mut(std::mem::take));
    tracker
        .poll_source(tracker.started + Duration::from_millis(millis), source)
        .unwrap()
}

#[test]
fn native_order_accepts_empty_desktop_before_and_after_hidden_window_lifetime() {
    let name = format!("SnowflakesEmpty-{}-{}", std::process::id(), unsafe {
        GetCurrentThreadId()
    });
    let _desktop = TestDesktop::attach(&name, true);
    for _ in 0..2 {
        unsafe {
            SetLastError(ERROR_ACCESS_DENIED);
        }
        assert!(
            Native::default()
                .order()
                .expect("validated empty desktop was rejected")
                .is_empty()
        );
        let mut host = Host::new(&name);
        let created = host.command("create");
        let handles: Vec<usize> = created
            .strip_prefix("created ")
            .unwrap()
            .split_whitespace()
            .map(|handle| handle.parse().unwrap())
            .collect();
        assert_eq!(handles.len(), 2);
        let mut native = Native::default();
        let order = native.order().unwrap();
        for handle in handles {
            assert!(order.contains(&handle));
            assert_eq!(native.owner(handle).unwrap().0, host.child.id());
        }
        host.finish();
    }
    unsafe {
        SetLastError(ERROR_ACCESS_DENIED);
    }
    assert!(Native::default().order().unwrap().is_empty());
}

#[test]
fn native_winevents_update_only_changed_hidden_child_windows_before_reconciliation() {
    let name = format!("SnowflakesEvents-{}-{}", std::process::id(), unsafe {
        GetCurrentThreadId()
    });
    let _desktop = TestDesktop::attach(&name, true);
    let mut tracker = Tracker::new().unwrap();
    let mut host = Host::new(&name);
    let mut source = FixtureSource {
        handles: vec![],
        pid: host.child.id(),
        reads: vec![],
        orders: 0,
    };
    assert!(
        poll(&mut tracker, &mut source, 0)
            .unwrap()
            .windows
            .is_empty()
    );
    let created = host.command("create");
    source.handles = created
        .strip_prefix("created ")
        .unwrap()
        .split_whitespace()
        .map(|h| h.parse().unwrap())
        .collect();
    assert_eq!(source.handles.len(), 2);
    pump();
    assert!(poll(&mut tracker, &mut source, 29).is_none());
    let before =
        poll(&mut tracker, &mut source, 30).expect("native create events were not delivered");
    assert_eq!(before.windows.len(), 2);
    assert_eq!(source.reads.len(), 2);
    let first = source.handles[0];
    let key = before
        .windows
        .iter()
        .find(|w| w.rect.left == 100.0)
        .unwrap()
        .key;
    let other_key = before
        .windows
        .iter()
        .find(|w| w.rect.left == 320.0)
        .unwrap()
        .key;
    source.reads.clear();
    assert_eq!(host.command("move"), "moved");
    pump();
    assert!(poll(&mut tracker, &mut source, 59).is_none());
    assert!(source.reads.is_empty());
    let moved = poll(&mut tracker, &mut source, 60).expect("native move events were not delivered");
    let window = moved.windows.iter().find(|w| w.key == key).unwrap();
    assert_eq!((window.rect.left, window.rect.top), (224.0, 182.0));
    assert_eq!(
        source.reads,
        [first],
        "the move burst must coalesce without reading the other window"
    );
    source.reads.clear();
    let orders = source.orders;
    assert_eq!(host.command("reorder"), "reordered");
    pump();
    let reordered =
        poll(&mut tracker, &mut source, 90).expect("native reorder events were not delivered");
    assert_eq!(reordered.windows[0].key, key);
    assert_eq!(source.orders, orders + 1);
    assert!(source.reads.is_empty());
    let orders = source.orders;
    {
        let own = Window::new(20);
        own.move_to(40, 30);
    }
    pump();
    assert!(
        poll(&mut tracker, &mut source, 120).is_none(),
        "own process events must be skipped"
    );
    assert_eq!(source.orders, orders);
    assert_eq!(host.command("close"), "closed");
    pump();
    let closed =
        poll(&mut tracker, &mut source, 150).expect("native close events were not delivered");
    assert_eq!(closed.windows.len(), 1);
    assert_eq!(closed.windows[0].key, other_key);
    let mut registry = std::mem::take(&mut tracker.registry);
    drop(tracker);
    assert_eq!(host.command("move"), "moved");
    let moved_after_unhook = source.read(source.handles[1]).unwrap();
    assert_eq!(
        (moved_after_unhook.rect.left, moved_after_unhook.rect.top),
        (224.0, 182.0)
    );
    pump();
    registry
        .changes
        .merge(CHANGES.with_borrow_mut(std::mem::take));
    assert!(
        registry
            .poll(Duration::from_millis(180), &mut source)
            .unwrap()
            .is_none(),
        "dropping Tracker must unregister the native callbacks"
    );
    host.finish();
}
