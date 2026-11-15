//! Hidden Win32 controls -> real control owner/disk/view -> a separate GPU thread.
use super::*;
use crate::platform::runtime::control_probe::{Probe, Summary};
use crate::{control::Setting, session::PauseReason, ui::View};
use std::sync::{Arc, Mutex, mpsc};

const CHILD: &str =
    "platform::weather::worker_probe::snow_pixels::controls::private_controls_child";

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Sample {
    trace_id: Option<u64>,
    intensity: u8,
    size: u8,
    visibility: u8,
    particles: usize,
    min_radius: f32,
    max_radius: f32,
    pixels: usize,
    alpha_sum: u64,
    #[serde(skip)]
    physics: String,
    #[serde(skip)]
    image: Vec<u8>,
}
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Evidence {
    stage: String,
    completed: bool,
    samples: Vec<Sample>,
    diagnostics: Summary,
    applied_keys: Vec<u8>,
    renderer_wakes: usize,
    visibility_physics_equal: bool,
    restored_pixels_equal: bool,
    failed_save_profile_and_pixels_equal: bool,
    dpi: u32,
    gpu_thread_joined: bool,
    layers_destroyed: bool,
    max_mass_relative_error: f64,
    present_calls: usize,
    accepted_presents: usize,
    occluded_presents: usize,
}
enum Draw {
    Frame {
        steps: usize,
        reply: mpsc::SyncSender<Sample>,
    },
    Stop,
}
struct GraphicsEnd {
    trace: super::Evidence,
    dpi: u32,
    wakes: usize,
}
struct Graphics {
    send: mpsc::Sender<Draw>,
    join: Option<std::thread::JoinHandle<GraphicsEnd>>,
}
fn view_settings(view: &Arc<Mutex<View>>) -> (Settings, Option<u64>) {
    let v = view.lock().unwrap();
    assert!(v.status.pauses.contains(&PauseReason::Manual));
    (v.status.settings.clone().unwrap(), v.trace.map(|t| t.id))
}
fn render_thread(
    view: Arc<Mutex<View>>,
    receive: mpsc::Receiver<Draw>,
    ready: mpsc::SyncSender<u32>,
) -> GraphicsEnd {
    guard_if_requested();
    let _dpi = Dpi::pmv2();
    guard_control_probe();
    let mut weather = Weather::new(true).unwrap();
    let (initial, _) = view_settings(&view);
    assert_eq!(initial.snow.intensity_percent, 0);
    let mut now = Instant::now();
    let mut trace = super::Evidence::default();
    frame(&mut weather, &initial, now, &mut trace);
    assert!(weather.world.flakes.is_empty());
    assert_eq!(weather.world.introduced_mass, 0.0);
    assert_eq!(weather.layers.len(), 1);
    let hwnd = weather.layers.values().next().unwrap().hwnd;
    let epoch = weather.layers.values().next().unwrap().metrics_epoch;
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let mut message = MSG::default();
    unsafe {
        let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
    }
    ready.send(unsafe { GetCurrentThreadId() }).unwrap();
    let mut wakes = 0;
    while let Ok(command) = receive.recv() {
        let Draw::Frame { steps, reply } = command else {
            break;
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if unsafe {
                PeekMessageW(
                    &mut message,
                    Some(HWND(-1_isize as *mut _)),
                    WM_NULL,
                    WM_NULL,
                    PM_REMOVE,
                )
            }
            .as_bool()
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "real control owner did not wake the graphics queue"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        wakes += 1;
        let (settings, trace_id) = view_settings(&view);
        pump();
        for _ in 0..steps {
            now += crate::timing::STEP;
            frame(&mut weather, &settings, now, &mut trace);
        }
        snapshot_frame(&mut weather, &settings, now, &mut trace);
        let layer = weather.layers.values().next().unwrap();
        assert_eq!(layer.hwnd, hwnd);
        assert_eq!(layer.metrics_epoch, epoch);
        let image = Image::read(layer);
        let sample = Sample {
            trace_id,
            intensity: settings.snow.intensity_percent,
            size: settings.snow.flake_size_percent,
            visibility: settings.snow.flake_visibility_percent,
            particles: weather.world.flakes.len(),
            min_radius: weather
                .world
                .flakes
                .iter()
                .map(|b| b.radius)
                .fold(f32::MAX, f32::min),
            max_radius: weather
                .world
                .flakes
                .iter()
                .map(|b| b.radius)
                .fold(0.0, f32::max),
            pixels: image.count(image.screen.rect),
            alpha_sum: image
                .bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| u64::from(p[3]))
                .sum(),
            physics: format!("{:?}", weather.world),
            image: image.bytes,
        };
        assert!(weather.world.flakes.iter().all(|b| b.slab.is_none()));
        assert!(weather.world.drifts.values().all(|d| d.total_mass() == 0.0));
        reply.send(sample).unwrap();
    }
    assert_eq!(weather.layers.values().next().unwrap().hwnd, hwnd);
    drop(weather);
    assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
    guard_control_probe();
    GraphicsEnd { trace, dpi, wakes }
}
impl Graphics {
    fn start(view: Arc<Mutex<View>>) -> (Self, u32) {
        let (send, receive) = mpsc::channel();
        let (ready, initialized) = mpsc::sync_channel(1);
        let join = std::thread::spawn(move || render_thread(view, receive, ready));
        let graphics = Self {
            send,
            join: Some(join),
        };
        let thread = initialized.recv_timeout(Duration::from_secs(5)).unwrap();
        (graphics, thread)
    }
    fn draw(&self, steps: usize) -> Sample {
        let (reply, response) = mpsc::sync_channel(1);
        self.send.send(Draw::Frame { steps, reply }).unwrap();
        response.recv_timeout(Duration::from_secs(5)).unwrap()
    }
    fn finish(mut self) -> GraphicsEnd {
        self.send.send(Draw::Stop).unwrap();
        self.join
            .take()
            .unwrap()
            .join()
            .expect("lower GPU thread failed")
    }
}
impl Drop for Graphics {
    fn drop(&mut self) {
        let _ = self.send.send(Draw::Stop);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
fn apply(probe: &Probe, graphics: &Graphics, steps: usize, evidence: &mut Evidence) {
    let applied = probe.apply_next().expect("native panel command missing");
    let sample = graphics.draw(steps);
    assert_eq!(sample.trace_id, Some(applied.trace_id));
    let published = probe.settings();
    assert_eq!(
        (sample.intensity, sample.size, sample.visibility),
        (
            published.snow.intensity_percent,
            published.snow.flake_size_percent,
            published.snow.flake_visibility_percent
        )
    );
    evidence.applied_keys.push(applied.key);
    evidence.samples.push(sample);
}
fn run(evidence: &mut Evidence) {
    evidence.stage = "panel_and_owner".into();
    let _dpi = Dpi::pmv2();
    guard_control_probe();
    let probe = Probe::new();
    evidence.stage = "gpu_start".into();
    let (graphics, thread) = Graphics::start(probe.render_view());
    probe.attach_render_thread(thread);
    evidence.stage = "initial_intensity".into();
    probe.slide(Setting::SnowIntensity(100));
    apply(&probe, &graphics, 120, evidence);
    assert!(evidence.samples[0].particles > 20 && evidence.samples[0].pixels > 20);
    probe.slide(Setting::SnowIntensity(0));
    apply(&probe, &graphics, 0, evidence);
    // Coalescing must happen in the actual Panel queue, before Owner dispatch.
    probe.slide(Setting::FlakeSize(125));
    probe.slide(Setting::FlakeSize(150));
    probe.slide(Setting::FlakeVisibility(0));
    apply(&probe, &graphics, 0, evidence);
    assert_eq!(evidence.samples[2].size, 150);
    let base = &evidence.samples[1];
    let large = &evidence.samples[2];
    assert_eq!(base.particles, large.particles);
    assert!((large.min_radius / base.min_radius - 1.5).abs() < 1e-5);
    assert!((large.max_radius / base.max_radius - 1.5).abs() < 1e-5);
    assert!(large.max_radius > large.min_radius * 1.2 && large.pixels > base.pixels);
    apply(&probe, &graphics, 0, evidence);
    assert_eq!(evidence.samples[3].pixels, 0);
    assert_eq!(evidence.samples[3].physics, evidence.samples[2].physics);
    assert!(probe.apply_next().is_none());
    probe.slide(Setting::FlakeVisibility(200));
    apply(&probe, &graphics, 0, evidence);
    assert_eq!(evidence.samples[4].physics, evidence.samples[2].physics);
    assert!(evidence.samples[4].alpha_sum > evidence.samples[2].alpha_sum);
    probe.slide(Setting::FlakeVisibility(100));
    apply(&probe, &graphics, 0, evidence);
    assert_eq!(evidence.samples[5].physics, evidence.samples[2].physics);
    assert_eq!(evidence.samples[5].image, evidence.samples[2].image);
    evidence.visibility_physics_equal = true;
    evidence.restored_pixels_equal = true;
    probe.slide(Setting::FlakeSize(100));
    apply(&probe, &graphics, 0, evidence);
    let restored = &evidence.samples[6];
    let original = &evidence.samples[1];
    assert_eq!(restored.particles, original.particles);
    assert!((restored.min_radius / original.min_radius - 1.0).abs() < 1e-5);
    assert!((restored.max_radius / original.max_radius - 1.0).abs() < 1e-5);
    assert!(restored.pixels > 20);
    assert!(probe.apply_next().is_none());
    assert_eq!(evidence.applied_keys, [1, 1, 18, 19, 19, 19, 18]);
    evidence.stage = "failed_save".into();
    probe.slide(Setting::FlakeVisibility(50));
    let rejected = probe.reject_next_locked();
    let unchanged = graphics.draw(0);
    assert_eq!(unchanged.trace_id, Some(rejected.trace_id));
    assert_eq!(unchanged.trace_id, evidence.samples[6].trace_id);
    assert_eq!(unchanged.visibility, 100);
    assert_eq!(unchanged.physics, evidence.samples[6].physics);
    assert_eq!(unchanged.image, evidence.samples[6].image);
    evidence.samples.push(unchanged);
    evidence.failed_save_profile_and_pixels_equal = true;
    evidence.diagnostics = probe.finish();
    assert_eq!(evidence.diagnostics.input, 9);
    assert_eq!(evidence.diagnostics.dispatch, 8);
    assert_eq!(evidence.diagnostics.received, 8);
    assert_eq!(evidence.diagnostics.applied, 7);
    assert_eq!(evidence.diagnostics.superseded, 7);
    assert_eq!(evidence.diagnostics.rejected, 1);
    assert_eq!(evidence.diagnostics.dropped, 0);
    assert_eq!(evidence.diagnostics.unfinished, 1);
    let ended = graphics.finish();
    evidence.renderer_wakes = ended.wakes;
    assert_eq!(evidence.renderer_wakes, 8);
    evidence.dpi = ended.dpi;
    evidence.max_mass_relative_error = ended.trace.max_mass_relative_error;
    evidence.present_calls = ended.trace.present_calls;
    evidence.accepted_presents = ended.trace.accepted_presents;
    evidence.occluded_presents = ended.trace.occluded_presents;
    evidence.gpu_thread_joined = true;
    evidence.layers_destroyed = true;
    guard_control_probe();
    evidence.stage = "complete".into();
    evidence.completed = true;
}
#[test]
#[ignore = "explicit hidden Panel/Controller/disk/view/GPU bridge on a private desktop"]
fn hidden_native_controls_reach_saved_view_and_separate_gpu_thread() {
    let evidence: Evidence = isolated(CHILD);
    assert!(evidence.completed && evidence.gpu_thread_joined && evidence.layers_destroyed);
    println!(
        "native_controls_pixels {}",
        serde_json::to_string(&evidence).unwrap()
    );
}
#[test]
#[ignore = "helper launched only on the assigned private startup desktop"]
fn private_controls_child() {
    let Ok(_) = std::env::var(DESKTOP_ENV) else {
        return;
    };
    let path = std::env::var_os(REPORT_ENV).expect("probe report path missing");
    let mut evidence = Evidence::default();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&mut evidence)));
    std::fs::write(path, serde_json::to_vec(&evidence).unwrap()).unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}
