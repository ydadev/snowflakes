//! Real Native -> Weather::update -> Layer lifecycle under monitor selection.
//! This exercises the lower update path, not the Observer-gated Worker loop.
use super::*;
use windows::Win32::Graphics::Gdi::*;

const CHILD: &str = "platform::weather::worker_probe::monitors::private_monitor_selection_child";

#[derive(Clone)]
struct Monitor {
    rect: Rect,
    primary: bool,
}
#[derive(Default)]
struct Inventory {
    monitors: Vec<Monitor>,
    failed: bool,
}
unsafe extern "system" fn monitor(handle: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    // The synchronous enumeration owns this stack inventory. No unwind crosses FFI.
    let inventory = unsafe { &mut *(data.0 as *mut Inventory) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(handle, &mut info) }.as_bool() {
        inventory.failed = true;
        return BOOL(0);
    }
    inventory.monitors.push(Monitor {
        rect: Rect {
            left: info.rcMonitor.left as f32,
            top: info.rcMonitor.top as f32,
            right: info.rcMonitor.right as f32,
            bottom: info.rcMonitor.bottom as f32,
        },
        primary: info.dwFlags & MONITORINFOF_PRIMARY != 0,
    });
    BOOL(1)
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Phase {
    name: String,
    layers: usize,
    present_codes: Vec<i32>,
    accepted: u32,
    pending_attempts: usize,
}
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Evidence {
    stage: String,
    completed: bool,
    error: Option<i32>,
    monitors: usize,
    distinct_secondary: bool,
    dpi: Vec<u32>,
    phases: Vec<Phase>,
    primary_retained: bool,
    selected_retained: bool,
    exclusions_destroyed: bool,
    empty_selection_released_all: bool,
    restored_new_epochs: bool,
    shutdown_destroyed_all: bool,
}

#[derive(Clone, Copy)]
struct Identity {
    hwnd: HWND,
    swap: usize,
    epoch: u64,
    presents: u64,
}
fn identities(weather: &Weather) -> BTreeMap<u64, Identity> {
    weather
        .layers
        .iter()
        .map(|(&key, layer)| {
            (
                key,
                Identity {
                    hwnd: layer.hwnd,
                    swap: layer.gpu.as_ref().unwrap().swap.as_raw() as usize,
                    epoch: layer.metrics_epoch,
                    presents: layer.meter.as_ref().unwrap().counters.presents,
                },
            )
        })
        .collect()
}
fn retained(before: Identity, after: Identity) {
    assert_eq!(before.hwnd, after.hwnd);
    assert_eq!(before.swap, after.swap);
    assert_eq!(before.epoch, after.epoch);
}
fn destroyed(identities: impl Iterator<Item = Identity>) {
    for identity in identities {
        assert!(!unsafe { IsWindow(Some(identity.hwnd)) }.as_bool());
    }
}

fn verify(weather: &Weather, expected: &[Monitor], desktop: &str) {
    guard_if_requested();
    assert!(weather.tracker.ready());
    assert_eq!(weather.layers.len(), expected.len());
    assert_eq!(weather.world.scene.screens.len(), expected.len());
    assert!(weather.graphics.error.is_none());
    if !expected.is_empty() {
        assert_eq!(
            layers(unsafe { GetCurrentThreadId() }, desktop).len(),
            expected.len()
        );
    }
    for layer in weather.layers.values() {
        assert_eq!(
            expected
                .iter()
                .filter(|m| m.rect == layer.screen.rect)
                .count(),
            1
        );
        let mut bounds = RECT::default();
        let mut client = RECT::default();
        unsafe {
            GetWindowRect(layer.hwnd, &mut bounds).unwrap();
            GetClientRect(layer.hwnd, &mut client).unwrap();
        }
        assert_eq!(
            (bounds.left, bounds.top, bounds.right, bounds.bottom),
            (
                layer.screen.rect.left as i32,
                layer.screen.rect.top as i32,
                layer.screen.rect.right as i32,
                layer.screen.rect.bottom as i32,
            )
        );
        let width = (bounds.right - bounds.left) as u32;
        let height = (bounds.bottom - bounds.top) as u32;
        assert_eq!(
            (client.left, client.top, client.right, client.bottom),
            (0, 0, width as i32, height as i32)
        );
        let gpu = layer.gpu.as_ref().unwrap();
        let size = unsafe { gpu._bitmap.GetPixelSize() };
        let desc = unsafe { gpu.swap.GetDesc1() }.unwrap();
        assert_eq!((size.width, size.height), (width, height));
        assert_eq!((desc.Width, desc.Height), (width, height));
        let scale = unsafe { GetDpiForWindow(layer.hwnd) } as f32 / 96.0;
        assert!(scale > 0.0);
        assert_eq!(layer.screen.scale, scale);
        let screen = weather
            .world
            .scene
            .screens
            .iter()
            .find(|s| s.key == layer.screen.key)
            .unwrap();
        assert_eq!(screen.rect, layer.screen.rect);
        assert_eq!(screen.scale, scale);
        assert!(layer.visible && unsafe { IsWindowVisible(layer.hwnd) }.as_bool());
    }
}

fn update(weather: &mut Weather, settings: &Settings, name: &str, evidence: &mut Evidence) {
    evidence.stage = name.into();
    guard_if_requested();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut pending_attempts = 0;
    loop {
        // Match the native message pump preceding update on the real Worker.
        unsafe {
            let mut message = MSG::default();
            for _ in 0..512 {
                if !PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    break;
                }
                assert_ne!(message.message, WM_QUIT);
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        let before = identities(weather);
        PRESENT_TRACE.with_borrow_mut(|trace| assert!(trace.replace(Vec::new()).is_none()));
        let result = weather.update(
            Instant::now(),
            settings,
            RenderQuality::for_settings(settings, false),
        );
        let codes = PRESENT_TRACE.with_borrow_mut(Option::take).unwrap();
        if let Err(error) = &result {
            evidence.error = Some(error.code().0);
        }
        let accepted = result.unwrap();
        let after = identities(weather);
        if !weather.tracker.ready() {
            assert_eq!(accepted, 0);
            assert!(codes.is_empty() && weather.graphics.error.is_none());
            assert_eq!(before.len(), after.len());
            for (key, identity) in &before {
                retained(*identity, after[key]);
                assert_eq!(identity.presents, after[key].presents);
            }
            assert!(
                weather.layers.values().all(
                    |layer| !layer.visible && !unsafe { IsWindowVisible(layer.hwnd) }.as_bool()
                )
            );
            pending_attempts += 1;
            assert!(
                Instant::now() < deadline,
                "Native scene did not become ready"
            );
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        assert_eq!(codes.len(), weather.layers.len());
        assert_eq!(
            accepted as usize,
            codes.iter().filter(|&&code| code == 0).count()
        );
        assert!(codes.iter().all(|&code| code >= 0));
        let increments: u64 = after
            .iter()
            .map(|(key, identity)| {
                let previous = before
                    .get(key)
                    .filter(|old| old.epoch == identity.epoch)
                    .map_or(0, |old| old.presents);
                identity.presents.checked_sub(previous).unwrap()
            })
            .sum();
        assert_eq!(increments, u64::from(accepted));
        evidence.phases.push(Phase {
            name: name.into(),
            layers: after.len(),
            present_codes: codes,
            accepted,
            pending_attempts,
        });
        return;
    }
}

fn run(evidence: &mut Evidence, desktop: &str) {
    guard_if_requested();
    let original_dpi = unsafe { GetThreadDpiAwarenessContext() };
    let dpi = Dpi::pmv2();
    let mut inventory = Inventory::default();
    assert!(
        unsafe {
            EnumDisplayMonitors(
                None,
                None,
                Some(monitor),
                LPARAM((&mut inventory as *mut Inventory) as isize),
            )
        }
        .as_bool()
    );
    assert!(!inventory.failed && !inventory.monitors.is_empty());
    assert!(inventory.monitors.len() <= frame_metrics::MAX_DISPLAYS);
    let monitors = inventory.monitors;
    evidence.monitors = monitors.len();
    let primary: Vec<_> = monitors.iter().filter(|m| m.primary).cloned().collect();
    assert_eq!(primary.len(), 1);
    let selected = monitors
        .iter()
        .find(|m| !m.primary)
        .unwrap_or(&primary[0])
        .clone();
    evidence.distinct_secondary = !selected.primary;
    let mut settings = Settings::default();
    settings.performance.pause_fullscreen = false;
    settings.interaction.cursor_enabled = false;
    let mut weather = Weather::new(true).unwrap();
    update(&mut weather, &settings, "all", evidence);
    verify(&weather, &monitors, desktop);
    assert!(!weather.world.flakes.is_empty());
    let initial = identities(&weather);
    let screens = weather.world.scene.screens.clone();
    let primary_key = screens
        .iter()
        .find(|s| s.rect == primary[0].rect)
        .unwrap()
        .key;
    let selected_screen = screens
        .iter()
        .find(|s| s.rect == selected.rect)
        .unwrap()
        .clone();
    evidence.dpi = weather
        .layers
        .values()
        .map(|layer| unsafe { GetDpiForWindow(layer.hwnd) })
        .collect();

    settings.display.selection = MonitorSelection::Primary;
    update(&mut weather, &settings, "primary", evidence);
    verify(&weather, &primary, desktop);
    retained(initial[&primary_key], identities(&weather)[&primary_key]);
    destroyed(
        initial
            .iter()
            .filter(|(key, _)| **key != primary_key)
            .map(|(_, value)| *value),
    );
    evidence.primary_retained = true;

    let before_selected = identities(&weather);
    settings.display = Display {
        selection: MonitorSelection::Selected,
        monitor_ids: vec![selected_screen.id()],
    };
    update(&mut weather, &settings, "selected", evidence);
    verify(&weather, std::slice::from_ref(&selected), desktop);
    let selection = identities(&weather);
    if selected_screen.key != primary_key {
        destroyed(before_selected.values().copied());
        assert!(
            selection[&selected_screen.key].epoch
                > initial.values().map(|id| id.epoch).max().unwrap()
        );
        evidence.exclusions_destroyed = true;
    }

    update(&mut weather, &settings, "selected_unchanged", evidence);
    verify(&weather, std::slice::from_ref(&selected), desktop);
    retained(
        selection[&selected_screen.key],
        identities(&weather)[&selected_screen.key],
    );
    evidence.selected_retained = true;

    let last_epoch = weather.next_metrics_epoch;
    let missing = (0..=screens.len() as u64)
        .find(|key| !screens.iter().any(|screen| screen.key == *key))
        .unwrap();
    settings.display.monitor_ids = vec![format!("display-{missing:x}")];
    update(&mut weather, &settings, "unavailable", evidence);
    verify(&weather, &[], desktop);
    destroyed(selection.values().copied());
    assert_eq!(weather.clock.fraction(), 0.0);
    evidence.empty_selection_released_all = true;

    settings.display = Display {
        selection: MonitorSelection::All,
        monitor_ids: vec![],
    };
    update(&mut weather, &settings, "all_restored", evidence);
    verify(&weather, &monitors, desktop);
    let restored = identities(&weather);
    assert_eq!(
        restored.keys().collect::<Vec<_>>(),
        initial.keys().collect::<Vec<_>>()
    );
    assert!(restored.values().all(|id| id.epoch > last_epoch));
    evidence.restored_new_epochs = true;
    evidence.stage = "shutdown".into();
    drop(weather);
    destroyed(restored.values().copied());
    evidence.shutdown_destroyed_all = true;
    desktop_guard(
        unsafe { GetThreadDesktop(GetCurrentThreadId()) }.unwrap(),
        desktop,
    );
    drop(dpi);
    assert!(
        unsafe { AreDpiAwarenessContextsEqual(GetThreadDpiAwarenessContext(), original_dpi) }
            .as_bool()
    );
    evidence.stage = "complete".into();
    evidence.completed = true;
}

#[test]
#[ignore = "explicit Native/Layer hardware test on a private startup desktop"]
fn native_monitor_selection_preserves_retained_layers_and_releases_excluded_layers() {
    let evidence: Evidence = isolated(CHILD);
    assert!(evidence.completed && evidence.error.is_none());
    assert!(evidence.primary_retained && evidence.selected_retained);
    assert!(!evidence.distinct_secondary || evidence.exclusions_destroyed);
    assert!(
        evidence.empty_selection_released_all
            && evidence.restored_new_epochs
            && evidence.shutdown_destroyed_all
    );
    println!(
        "native_monitor_layers {}",
        serde_json::to_string(&evidence).unwrap()
    );
}

#[test]
#[ignore = "helper launched only on the explicitly assigned private startup desktop"]
fn private_monitor_selection_child() {
    let Ok(expected) = std::env::var(DESKTOP_ENV) else {
        return;
    };
    let path = std::env::var_os(REPORT_ENV).expect("probe report path missing");
    let mut evidence = Evidence::default();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(&mut evidence, &expected)
    }));
    std::fs::write(path, serde_json::to_vec(&evidence).unwrap()).unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}
