//! Production Native queries on a private desktop, never the user's input desktop.
use super::*;
use crate::{geometry::Point, snow::World};
use snowflakes_protocol::Settings;

mod cleanup_probe;
mod monitors;
mod resize;
mod visibility;

fn object_name(handle: HANDLE) -> String {
    let mut value = [0u16; 256];
    unsafe {
        GetUserObjectInformationW(
            handle,
            UOI_NAME,
            Some(value.as_mut_ptr().cast()),
            size_of_val(&value) as u32,
            None,
        )
        .unwrap();
    }
    let length = value
        .iter()
        .position(|c| *c == 0)
        .expect("object name was not terminated");
    String::from_utf16(&value[..length]).unwrap()
}

fn receiving_input(desktop: HDESK) -> bool {
    let mut input = BOOL(1);
    unsafe {
        GetUserObjectInformationW(
            HANDLE(desktop.0),
            UOI_IO,
            Some((&mut input as *mut BOOL).cast()),
            size_of::<BOOL>() as u32,
            None,
        )
        .unwrap();
    }
    input.as_bool()
}

fn private(name: &str) {
    assert!(name.starts_with("SnowflakesNative-"));
    let desktop = unsafe { GetThreadDesktop(GetCurrentThreadId()).unwrap() };
    assert!(object_name(HANDLE(desktop.0)).eq_ignore_ascii_case(name));
    assert!(
        !receiving_input(desktop),
        "fixture desktop must never receive input"
    );
    assert_eq!(ACCESS & DESKTOP_SWITCHDESKTOP.0, 0);
    assert!(
        unsafe {
            AreDpiAwarenessContextsEqual(
                GetThreadDpiAwarenessContext(),
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            )
        }
        .as_bool()
    );
}

pub(super) struct NativeWindow(HWND);
impl NativeWindow {
    fn new(name: &str, x: i32) -> Self {
        private(name);
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE,
                w!("STATIC"),
                w!(""),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
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
        assert!(unsafe { IsWindowVisible(hwnd) }.as_bool());
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
            assert!(
                AreDpiAwarenessContextsEqual(
                    GetWindowDpiAwarenessContext(self.0),
                    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2
                )
                .as_bool()
            );
        }
    }
}
impl Drop for NativeWindow {
    fn drop(&mut self) {
        self.check();
        unsafe {
            DestroyWindow(self.0).unwrap();
        }
    }
}

pub(super) fn command(command: &str, windows: &mut Vec<NativeWindow>) -> bool {
    if monitors::command(command, windows) {
        return true;
    }
    match command {
        "native-create" => {
            assert!(windows.is_empty());
            let name = std::env::var(DESKTOP_ENV).unwrap();
            windows.push(NativeWindow::new(&name, 100));
            windows.push(NativeWindow::new(&name, 320));
            reply(&format!(
                "native-created {} {}",
                windows[0].0.0 as usize, windows[1].0.0 as usize
            ));
        }
        "native-move" => {
            private(&std::env::var(DESKTOP_ENV).unwrap());
            windows[0].check();
            unsafe {
                SetWindowPos(
                    windows[0].0,
                    None,
                    180,
                    140,
                    0,
                    0,
                    SWP_NOZORDER | SWP_NOSIZE | SWP_NOACTIVATE,
                )
                .unwrap();
            }
            reply("native-moved");
        }
        "native-nudge" | "native-slide" => {
            private(&std::env::var(DESKTOP_ENV).unwrap());
            windows[0].check();
            let mut bounds = RECT::default();
            unsafe {
                GetWindowRect(windows[0].0, &mut bounds).unwrap();
                SetWindowPos(
                    windows[0].0,
                    None,
                    bounds.left + i32::from(command == "native-nudge"),
                    bounds.top + if command == "native-slide" { 20 } else { 0 },
                    0,
                    0,
                    SWP_NOZORDER | SWP_NOSIZE | SWP_NOACTIVATE,
                )
                .unwrap();
            }
            reply(if command == "native-nudge" {
                "native-nudged"
            } else {
                "native-slid"
            });
        }
        "native-cover" | "native-uncover" => {
            private(&std::env::var(DESKTOP_ENV).unwrap());
            windows[0].check();
            windows[1].check();
            let mut roof = RECT::default();
            unsafe {
                GetWindowRect(windows[0].0, &mut roof).unwrap();
                let (left, top) = if command == "native-cover" {
                    (roof.left + 80, roof.top - 40)
                } else {
                    (320, 100)
                };
                SetWindowPos(
                    windows[1].0,
                    Some(HWND_TOP),
                    left,
                    top,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOACTIVATE,
                )
                .unwrap();
            }
            reply("native-cover-changed");
        }
        "native-close" => {
            windows.remove(0);
            reply("native-closed");
        }
        "native-wide" | "native-grow-right" | "native-shrink-right" | "native-cut-left" => {
            private(&std::env::var(DESKTOP_ENV).unwrap());
            windows[0].check();
            let mut bounds = RECT::default();
            unsafe {
                GetWindowRect(windows[0].0, &mut bounds).unwrap();
            }
            let (left, width) = match command {
                "native-wide" => (bounds.left, 520),
                "native-grow-right" => (bounds.left, 640),
                "native-shrink-right" => (bounds.left, 360),
                _ => (bounds.right - 360, 360),
            };
            unsafe {
                SetWindowPos(
                    windows[0].0,
                    None,
                    left,
                    bounds.top,
                    width,
                    bounds.bottom - bounds.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
                .unwrap();
            }
            reply("native-resized");
        }
        "native-hide" | "native-show" | "native-minimize" | "native-restore" => {
            private(&std::env::var(DESKTOP_ENV).unwrap());
            windows[0].check();
            let hwnd = windows[0].0;
            unsafe {
                match command {
                    "native-hide" | "native-show" => {
                        SetWindowPos(
                            hwnd,
                            None,
                            0,
                            0,
                            0,
                            0,
                            SWP_NOZORDER
                                | SWP_NOMOVE
                                | SWP_NOSIZE
                                | SWP_NOACTIVATE
                                | if command == "native-hide" {
                                    SWP_HIDEWINDOW
                                } else {
                                    SWP_SHOWWINDOW
                                },
                        )
                        .unwrap();
                        assert_eq!(IsWindowVisible(hwnd).as_bool(), command == "native-show");
                    }
                    "native-minimize" => {
                        let _ = ShowWindow(hwnd, SW_SHOWMINNOACTIVE);
                        assert!(IsIconic(hwnd).as_bool());
                    }
                    _ => {
                        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                        assert!(!IsIconic(hwnd).as_bool() && IsWindowVisible(hwnd).as_bool());
                    }
                }
            }
            reply("native-state-changed");
        }
        "native-cloak" | "native-uncloak" => {
            private(&std::env::var(DESKTOP_ENV).unwrap());
            windows[0].check();
            let value = BOOL::from(command == "native-cloak");
            unsafe {
                DwmSetWindowAttribute(
                    windows[0].0,
                    DWMWA_CLOAK,
                    (&value as *const BOOL).cast(),
                    size_of::<BOOL>() as u32,
                )
                .unwrap();
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let mut state = 0u32;
                unsafe {
                    DwmGetWindowAttribute(
                        windows[0].0,
                        DWMWA_CLOAKED,
                        (&mut state as *mut u32).cast(),
                        4,
                    )
                    .unwrap();
                }
                if (state & 1 != 0) == value.as_bool() {
                    break;
                }
                assert!(Instant::now() < deadline, "DWM cloak state did not settle");
                std::thread::sleep(Duration::from_millis(2));
            }
            reply("native-state-changed");
        }
        _ => return false,
    }
    true
}

fn poll_native(tracker: &mut Tracker, millis: u64) -> Scene {
    assert!(
        millis < 1000,
        "periodic reconciliation must not replace native events"
    );
    let scene = tracker
        .poll(tracker.started + Duration::from_millis(millis))
        .unwrap()
        .expect("production Native snapshot missing");
    assert!(tracker.ready());
    assert!(!scene.screens.is_empty());
    for screen in &scene.screens {
        assert!(screen.rect.width() > 0.0 && screen.rect.height() > 0.0);
        assert!(screen.floor > screen.rect.top && screen.floor <= screen.rect.bottom);
    }
    scene
}

fn info(handle: usize, pid: u32) -> WindowInfo {
    let value = Native::default()
        .read(handle)
        .expect("captioned fixture was excluded by Native::read");
    assert_eq!(value.process, pid);
    assert!(
        value.thread != 0 && value.visible && value.collect && !value.taskbar && !value.maximized
    );
    let hwnd = HWND(handle as *mut _);
    let mut cloaked = 0u32;
    let mut bounds = RECT::default();
    unsafe {
        DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&mut cloaked as *mut u32).cast(), 4).unwrap();
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut bounds as *mut RECT).cast(),
            size_of::<RECT>() as u32,
        )
        .is_err()
        {
            GetWindowRect(hwnd, &mut bounds).unwrap();
        }
    }
    assert_eq!(cloaked, 0);
    assert_eq!(value.rect, rect(bounds));
    value
}

struct Capture {
    scene: Scene,
    info: Vec<WindowInfo>,
}
impl Capture {
    fn key(&self, index: usize) -> u64 {
        self.scene
            .windows
            .iter()
            .find(|w| w.rect == self.info[index].rect && w.visible && w.collect)
            .expect("native captioned support missing from scene")
            .key
    }
}
fn create(tracker: &mut Tracker, host: &mut Host) -> Capture {
    assert!(poll_native(tracker, 0).windows.is_empty());
    let created = host.command("native-create");
    let handles: Vec<usize> = created
        .strip_prefix("native-created ")
        .unwrap()
        .split_whitespace()
        .map(|handle| handle.parse().unwrap())
        .collect();
    assert_eq!(handles.len(), 2);
    pump();
    let scene = poll_native(tracker, 30);
    let mut native = Native::default();
    let order = native.order().unwrap();
    let values: Vec<_> = handles
        .into_iter()
        .map(|handle| {
            assert!(order.contains(&handle));
            assert_eq!(native.owner(handle).unwrap().0, host.child.id());
            info(handle, host.child.id())
        })
        .collect();
    let capture = Capture {
        scene,
        info: values,
    };
    assert_ne!(capture.key(0), capture.key(1));
    capture
}

#[test]
fn production_native_tracker_reads_dwm_geometry_and_updates_move_close_without_fixture_source() {
    let name = format!("SnowflakesNative-{}-{}", std::process::id(), unsafe {
        GetCurrentThreadId()
    });
    let _desktop = TestDesktop::attach(&name, true);
    private(&name);
    let mut tracker = Tracker::new().unwrap();
    let mut host = Host::new(&name);
    let captured = create(&mut tracker, &mut host);
    let key = captured.key(0);
    let control = captured.key(1);
    crate::platform::scene::tests::cleanup::phase("before_owned_window");
    {
        let own = NativeWindow::new(&name, 550);
        crate::platform::scene::tests::cleanup::phase("owned_window_alive");
        assert!(
            Native::default().read(own.0.0 as usize).is_none(),
            "own visible windows must be excluded"
        );
    }
    crate::platform::scene::tests::cleanup::phase("owned_window_destroyed");
    let hidden = host.command("create");
    for handle in hidden
        .strip_prefix("created ")
        .unwrap()
        .split_whitespace()
        .map(|h| h.parse().unwrap())
    {
        assert!(
            Native::default().read(handle).is_none(),
            "hidden windows must be excluded"
        );
    }
    assert_eq!(host.command("native-move"), "native-moved");
    pump();
    let moved = poll_native(&mut tracker, 60);
    let after = info(captured.info[0].handle, host.child.id());
    assert_eq!(after.rect.left, captured.info[0].rect.left + 80.0);
    assert_eq!(after.rect.top, captured.info[0].rect.top + 40.0);
    assert_eq!(
        moved.windows.iter().find(|w| w.key == key).unwrap().rect,
        after.rect
    );
    assert_eq!(
        moved
            .windows
            .iter()
            .find(|w| w.key == control)
            .unwrap()
            .rect,
        captured.info[1].rect
    );
    assert_eq!(host.command("native-close"), "native-closed");
    pump();
    let closed = poll_native(&mut tracker, 90);
    assert!(closed.windows.iter().all(|w| w.key != key));
    assert_eq!(
        closed
            .windows
            .iter()
            .find(|w| w.key == control)
            .unwrap()
            .rect,
        captured.info[1].rect
    );
    assert!(
        Native::default().read(captured.info[0].handle).is_none(),
        "destroyed window must be excluded"
    );
    host.finish();
    private(&name);
}

fn conserved(world: &World) {
    assert!(
        (world.introduced_mass - world.removed_mass - world.total_mass()).abs()
            < world.introduced_mass.max(1.0) * 1e-5
    );
}

#[test]
fn production_native_scene_closure_releases_snow_slabs_and_preserves_mass_through_pause() {
    for paused in [false, true] {
        let name = format!(
            "SnowflakesNative-{}-{}-{paused}",
            std::process::id(),
            unsafe { GetCurrentThreadId() }
        );
        let _desktop = TestDesktop::attach(&name, true);
        private(&name);
        let mut tracker = Tracker::new().unwrap();
        let mut host = Host::new(&name);
        let captured = create(&mut tracker, &mut host);
        let key = captured.key(0);
        let control = captured.key(1);
        let center = Point::new(
            (captured.info[0].rect.left + captured.info[0].rect.right) * 0.5,
            captured.info[0].rect.top,
        );
        let screen = captured
            .scene
            .screens
            .iter()
            .find(|s| s.rect.contains(center))
            .unwrap()
            .clone();
        let mut world = World::new(captured.scene, 101);
        for key in [key, control] {
            let drift = world.drifts.get_mut(&key).unwrap();
            drift.mass.fill(5.0 * drift.cell_width);
        }
        world.introduced_mass = world.total_mass();
        let mass = world.drifts[&key].total_mass();
        let control_mass = world.drifts[&control].total_mass();
        let mut settings = Settings::default();
        settings.snow.intensity_percent = 0;
        settings.snow.wind_percent = 0;
        settings.snow.melting = false;
        settings.snow.drift_size_percent = 100;
        world.step(&settings, None);
        world.step(&settings, None);
        if paused {
            world.set_paused_monitors([screen.key].into());
        }
        assert_eq!(host.command("native-close"), "native-closed");
        pump();
        world.set_scene(poll_native(&mut tracker, 60));
        assert!(!world.drifts.contains_key(&key));
        conserved(&world);
        if paused {
            for _ in 0..120 {
                world.step(&settings, None);
                conserved(&world);
            }
            assert!(world.flakes.is_empty());
            assert!(world.pending_collapse_count() > 0);
            world.set_paused_monitors(Default::default());
        }
        world.step(&settings, None);
        assert!(!world.flakes.is_empty());
        assert!(
            world
                .flakes
                .iter()
                .all(|f| f.slab.is_some() && f.skip_support == Some(key))
        );
        for _ in 0..1800 {
            world.step(&settings, None);
            conserved(&world);
            if world.flakes.is_empty() && world.pending_collapse_count() == 0 {
                break;
            }
        }
        assert!(world.flakes.is_empty() && world.pending_collapse_count() == 0);
        let floor = &world.drifts[&(screen.key | (1 << 63))];
        assert!((floor.total_mass() - mass).abs() < mass * 1e-5);
        assert!((world.drifts[&control].total_mass() - control_mass).abs() < 0.01);
        assert!(
            (0..floor.mass.len())
                .map(|i| floor.height(i, &world.scene))
                .fold(0.0, f32::max)
                > 1.0
        );
        let occupied = floor.mass.iter().filter(|mass| **mass > 0.001).count();
        assert!(occupied >= 3 && occupied as f32 * floor.cell_width < screen.rect.width() * 0.5);
        assert!(world.removed_mass < world.introduced_mass * 1e-6);
        host.finish();
        private(&name);
    }
}
