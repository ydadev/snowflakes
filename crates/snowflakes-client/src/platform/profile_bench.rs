//! Explicit, finite native experiment. Never presents or controls a running client.
use super::{
    tests::{
        WEATHER_TEST, distance_to_profile, profile_edges, profile_pixels,
        render_profile_reference_components, render_profile_reference_with_factory,
    },
    *,
};
use crate::{geometry::Window, profile_cache::Stats, snow::SnowSlab};
use serde::Serialize;
use windows::{
    Win32::System::Threading::{GetCurrentThread, GetThreadTimes},
    core::{Error, HRESULT},
};

const WARMUP: usize = 16;
const FRAMES: usize = 80;
const WIDTH: usize = 1280;
const HEIGHT: usize = 720;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Backend {
    Reference,
    Paths,
    Realizations,
    RealizedFill,
    RealizedStroke,
}
impl Backend {
    fn name(self) -> &'static str {
        match self {
            Self::Reference => "reference",
            Self::Paths => "paths",
            Self::Realizations => "realizations",
            Self::RealizedFill => "realized_fill",
            Self::RealizedStroke => "realized_stroke",
        }
    }
}

fn query_ready(status: HRESULT) -> Result<bool> {
    match status.0 {
        0 => Ok(true),
        1 => Ok(false), // S_FALSE must not be erased by HRESULT::ok().
        _ => {
            status.ok()?;
            Err(Error::new(E_UNEXPECTED, "unexpected query status"))
        }
    }
}
fn interval_us(first: u64, last: u64, frequency: u64, disjoint: bool) -> Option<f64> {
    if disjoint || frequency == 0 {
        return None;
    }
    Some(last.checked_sub(first)? as f64 * 1_000_000.0 / frequency as f64)
}

struct Queries {
    context: ID3D11DeviceContext,
    disjoint: ID3D11Query,
    first: ID3D11Query,
    last: ID3D11Query,
    pipeline: ID3D11Query,
}
impl Queries {
    fn new(gpu: &Gpu) -> Result<Self> {
        unsafe {
            let create = |kind| {
                let mut value = None;
                gpu._d3d.CreateQuery(
                    &D3D11_QUERY_DESC {
                        Query: kind,
                        MiscFlags: 0,
                    },
                    Some(&mut value),
                )?;
                value.ok_or_else(|| Error::new(E_UNEXPECTED, "query unavailable"))
            };
            Ok(Self {
                context: gpu._d3d.GetImmediateContext()?,
                disjoint: create(D3D11_QUERY_TIMESTAMP_DISJOINT)?,
                first: create(D3D11_QUERY_TIMESTAMP)?,
                last: create(D3D11_QUERY_TIMESTAMP)?,
                pipeline: create(D3D11_QUERY_PIPELINE_STATISTICS)?,
            })
        }
    }
    // Safety: T must match the native result type of this query, and contain only POD fields.
    unsafe fn read<T: Default>(&self, query: &ID3D11Query, deadline: Instant) -> Result<T> {
        let mut value = T::default();
        let mut polls = 0;
        loop {
            let status = unsafe {
                (self.context.vtable().GetData)(
                    self.context.as_raw(),
                    query.as_raw(),
                    (&mut value as *mut T).cast(),
                    size_of::<T>() as u32,
                    D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32,
                )
            };
            if query_ready(status)? {
                return Ok(value);
            }
            if Instant::now() >= deadline {
                return Err(Error::new(E_ABORT, "bounded GPU query timed out"));
            }
            polls += 1;
            if polls < 16 {
                std::thread::yield_now();
            } else {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
    fn measure(&self, deadline: Instant, draw: impl FnOnce() -> Result<()>) -> Result<Sample> {
        unsafe {
            self.context.Begin(&self.disjoint);
            self.context.Begin(&self.pipeline);
            self.context.End(&self.first);
            let start = Instant::now();
            let result = draw();
            let submission_us = start.elapsed().as_secs_f64() * 1_000_000.0;
            self.context.End(&self.last);
            self.context.End(&self.pipeline);
            self.context.End(&self.disjoint);
            // Test-only flush/fence, not an optimization added to the production renderer.
            self.context.Flush();
            result?;
            let deadline = deadline.min(Instant::now() + Duration::from_secs(2));
            let data: D3D11_QUERY_DATA_TIMESTAMP_DISJOINT = self.read(&self.disjoint, deadline)?;
            let first: u64 = self.read(&self.first, deadline)?;
            let last: u64 = self.read(&self.last, deadline)?;
            let pipeline: D3D11_QUERY_DATA_PIPELINE_STATISTICS =
                self.read(&self.pipeline, deadline)?;
            assert!(
                pipeline.PSInvocations > 0,
                "query interval contained no pixel shading"
            );
            Ok(Sample {
                submission_us,
                completed_us: start.elapsed().as_secs_f64() * 1_000_000.0,
                // Elapsed GPU timeline includes gaps while CPU feeds D2D. It is NOT GPU busy time.
                gpu_interval_us: interval_us(first, last, data.Frequency, data.Disjoint.as_bool()),
                pixel_shader_invocations: pipeline.PSInvocations,
                input_primitives: pipeline.IAPrimitives,
                clipped_primitives: pipeline.CPrimitives,
            })
        }
    }
}
struct Sample {
    submission_us: f64,
    completed_us: f64,
    gpu_interval_us: Option<f64>,
    pixel_shader_invocations: u64,
    input_primitives: u64,
    clipped_primitives: u64,
}

fn component_plan(backend: Backend, fill: bool, stroke: bool) -> TestPlan {
    TestPlan {
        fill,
        stroke,
        realize_fill: matches!(backend, Backend::Realizations | Backend::RealizedFill),
        realize_stroke: matches!(backend, Backend::Realizations | Backend::RealizedStroke),
    }
}
fn configure_components(gpu: &mut Gpu, factory: &ID2D1Factory1, backend: Backend, plan: TestPlan) {
    gpu.profiles = Profiles::new(factory, &gpu.context);
    if matches!(backend, Backend::Paths | Backend::Reference) {
        gpu.profiles.disable_realizations();
    }
    gpu.profiles.set_test_plan(plan);
}

#[test]
#[ignore = "explicit release component experiment; hidden D3D11 target, no Present; finite 60s bound"]
fn native_profile_components_cost_without_presentation() {
    assert_eq!(
        TestPlan::DEFAULT,
        component_plan(Backend::RealizedStroke, true, true)
    );
    let build = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    assert_eq!(
        build, "release",
        "run the explicit experiment with --release"
    );
    let _serial = WEATHER_TEST.lock().unwrap();
    let _weather = Weather::new(false).unwrap();
    let settings = Settings::default();
    let quality = RenderQuality::for_settings(&settings, false);
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut layer = Layer::new(Fixture::new(16, false).screen, 1, false).unwrap();
    let hwnd = layer.hwnd;
    let gpu = layer.gpu.as_mut().unwrap();
    assert!(!unsafe { IsWindowVisible(hwnd) }.as_bool());
    let queries = Queries::new(gpu).unwrap();
    let factory: ID2D1Factory1 = unsafe { gpu.context.GetFactory().unwrap().cast().unwrap() };
    let adapter = unsafe {
        let dxgi: IDXGIDevice = gpu._d3d.cast().unwrap();
        let native = dxgi.GetAdapter().unwrap();
        let desc = native.cast::<IDXGIAdapter1>().unwrap().GetDesc1().unwrap();
        let driver = native
            .CheckInterfaceSupport(&IDXGIDevice::IID)
            .ok()
            .map(|v| {
                let v = v as u64;
                format!(
                    "{}.{}.{}.{}",
                    v >> 48,
                    (v >> 32) & 0xffff,
                    (v >> 16) & 0xffff,
                    v & 0xffff
                )
            });
        serde_json::json!({ "vendor_id": desc.VendorId, "device_id": desc.DeviceId,
            "software": desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0,
            "feature_level": format!("0x{:04x}", gpu._d3d.GetFeatureLevel().0), "driver_version": driver })
    };
    const COUNT: usize = 48;
    const WARM: usize = 12;
    for slabs in [16, 128] {
        for (component, fill, stroke) in [
            ("both", true, true),
            ("fill", true, false),
            ("stroke", false, true),
        ] {
            let forward = if fill && stroke {
                vec![
                    Backend::Reference,
                    Backend::Paths,
                    Backend::RealizedFill,
                    Backend::RealizedStroke,
                    Backend::Realizations,
                ]
            } else {
                vec![Backend::Reference, Backend::Paths, Backend::Realizations]
            };
            // Validate image equivalence and prewarm pipelines before the timing sequence.
            for &backend in &forward[1..] {
                let plan = component_plan(backend, fill, stroke);
                configure_components(gpu, &factory, backend, plan);
                let mut fixture = Fixture::new(slabs, false);
                fixture.update(WARM + COUNT - 1);
                for _ in 0..2 {
                    queries
                        .measure(deadline, || {
                            gpu.render_snow(
                                &fixture.screen,
                                &fixture.world,
                                &settings,
                                quality,
                                1.0,
                            )
                        })
                        .unwrap();
                }
                compare_component_pixels(gpu, &fixture, &settings, plan);
                assert!(
                    Instant::now() < deadline,
                    "component experiment exceeded bound"
                );
            }
            let mut expected_fingerprint = None;
            for (run, &backend) in forward.iter().chain(forward.iter().rev()).enumerate() {
                let plan = component_plan(backend, fill, stroke);
                configure_components(gpu, &factory, backend, plan);
                let mut fixture = Fixture::new(slabs, false);
                let draw = |f: &Fixture| {
                    if backend == Backend::Reference {
                        unsafe {
                            render_profile_reference_components(
                                gpu, &factory, &f.screen, &f.world, &settings, 1.0, plan,
                            )
                            .map(|_| ())
                        }
                    } else {
                        gpu.render_snow(&f.screen, &f.world, &settings, quality, 1.0)
                    }
                };
                for frame in 0..WARM {
                    assert!(
                        Instant::now() < deadline,
                        "component experiment exceeded bound"
                    );
                    fixture.update(frame);
                    queries.measure(deadline, || draw(&fixture)).unwrap();
                }
                let before = gpu.profiles.stats().0;
                let mut samples = Vec::with_capacity(COUNT);
                for frame in WARM..WARM + COUNT {
                    assert!(
                        Instant::now() < deadline,
                        "component experiment exceeded bound"
                    );
                    fixture.update(frame);
                    samples.push(queries.measure(deadline, || draw(&fixture)).unwrap());
                }
                let hash = fixture.fingerprint();
                assert_eq!(*expected_fingerprint.get_or_insert(hash), hash);
                let (stats, fills, strokes, usage) = gpu.profiles.stats();
                if !plan.realize_fill || !plan.fill {
                    assert_eq!(fills, 0);
                }
                if !plan.realize_stroke || !plan.stroke {
                    assert_eq!(strokes, 0);
                }
                let (builds, hits, _) = delta(stats, before);
                if backend != Backend::Reference {
                    assert_eq!(builds, 0);
                    assert_eq!(hits as usize, (slabs + fixture.world.drifts.len()) * COUNT);
                }
                let mean_counter = |extract: fn(&Sample) -> u64| {
                    samples.iter().map(|s| extract(s) as f64).sum::<f64>() / COUNT as f64
                };
                eprintln!(
                    "profile_components {}",
                    serde_json::json!({
                    "schema_version": 2, "scope": "serialized_offscreen_components_only", "adapter": adapter,
                    "production_default": plan == TestPlan::DEFAULT,
                    "allocation_policy": "requested_components_only",
                    "target": [WIDTH, HEIGHT], "slabs": slabs, "drifts": fixture.world.drifts.len(),
                    "components": component, "backend": backend.name(), "run": run + 1, "warmup": WARM, "frames": COUNT,
                    "fixture_hash": format!("{hash:016x}"),
                    "submission_wall": distribution(samples.iter().map(|s| s.submission_us).collect()),
                    "gpu_timeline_not_busy": distribution(samples.iter().filter_map(|s| s.gpu_interval_us).collect()),
                    "invalid_gpu_samples": samples.iter().filter(|s| s.gpu_interval_us.is_none()).count(),
                    "pixel_shader_invocations_mean": mean_counter(|s| s.pixel_shader_invocations),
                    "input_primitives_mean": mean_counter(|s| s.input_primitives),
                    "clipped_primitives_mean": mean_counter(|s| s.clipped_primitives),
                    "cache_builds": builds, "cache_hits": hits, "cache_usage": [usage.0, usage.1],
                    "fill_attempts_including_warmup": fills, "stroke_attempts_including_warmup": strokes,
                    "visible_fps_acceptance": "not_run" })
                );
                assert!(!unsafe { IsWindowVisible(hwnd) }.as_bool());
            }
        }
    }
    gpu.profiles.clear();
    assert!(
        Instant::now() < deadline,
        "component experiment exceeded bound"
    );
}
#[derive(Debug, Serialize)]
struct Distribution {
    samples: usize,
    mean_us: f64,
    p50_us: f64,
    p95_us: f64,
    max_us: f64,
}
fn distribution(mut values: Vec<f64>) -> Option<Distribution> {
    if values.is_empty() || values.iter().any(|v| !v.is_finite() || *v < 0.0) {
        return None;
    }
    values.sort_unstable_by(f64::total_cmp);
    let quantile =
        |percent: usize| values[(values.len() * percent).div_ceil(100).saturating_sub(1)];
    Some(Distribution {
        samples: values.len(),
        mean_us: values.iter().sum::<f64>() / values.len() as f64,
        p50_us: quantile(50),
        p95_us: quantile(95),
        max_us: *values.last().unwrap(),
    })
}
fn thread_cpu_us() -> Result<u64> {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe {
        GetThreadTimes(
            GetCurrentThread(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )?;
    }
    let ticks =
        |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    Ok((ticks(kernel) + ticks(user)) / 10)
}

struct Fixture {
    screen: Screen,
    world: World,
    changing: bool,
}
impl Fixture {
    fn new(slabs: usize, changing: bool) -> Self {
        let screen = Screen {
            key: 1,
            persistent_id: true,
            rect: Rect {
                left: -1600.0,
                top: -900.0,
                right: -320.0,
                bottom: -180.0,
            },
            floor: -200.0,
            scale: 1.0,
        };
        let windows = (0..8)
            .map(|i| {
                let left = screen.rect.left + 24.0 + (i % 4) as f32 * 310.0;
                let top = screen.rect.top + 280.0 + (i / 4) as f32 * 280.0;
                Window {
                    key: 100 + i,
                    rect: Rect {
                        left,
                        top,
                        right: left + 240.0,
                        bottom: top + 100.0,
                    },
                    visible: true,
                    collect: true,
                }
            })
            .collect();
        let mut world = World::new(
            Scene {
                screens: vec![screen.clone()],
                windows,
            },
            703,
        );
        for i in 0..slabs {
            let mut body = Flake::new(Point::default(), 3.0);
            body.age = 1.0;
            body.mass = 150.0;
            body.slab = Some(SnowSlab {
                heights: vec![0.0; 24],
                cell_width: 4.0,
                center: Point::new(48.0, -3.0),
            });
            assert!(world.release(body));
            assert_eq!(world.flakes.len(), i + 1);
        }
        let mut fixture = Self {
            screen,
            world,
            changing,
        };
        fixture.update(0);
        fixture
    }
    fn update(&mut self, frame: usize) {
        let changed = if self.changing {
            frame as f32 * 0.002
        } else {
            0.0
        };
        for (i, body) in self.world.flakes.iter_mut().enumerate() {
            body.position = Point::new(
                self.screen.rect.left
                    + 48.0
                    + (i % 16) as f32 * 78.0
                    + (frame as f32 * 0.03 + i as f32).sin() * 5.0,
                self.screen.rect.top + 70.0 + (i / 16) as f32 * 65.0,
            );
            body.angle = i as f32 * 0.037 + frame as f32 * 0.003;
            for (cell, height) in body.slab.as_mut().unwrap().heights.iter_mut().enumerate() {
                *height = if (10..13).contains(&cell) {
                    0.0
                } else {
                    3.0 + ((cell * 7 + i * 3) % 11) as f32 * 0.7 + i as f32 * 0.013 + changed
                };
            }
        }
        for (i, drift) in self.world.drifts.values_mut().enumerate() {
            for (cell, mass) in drift.mass.iter_mut().enumerate() {
                *mass = (4.0 + (cell % 9) as f32 + i as f32 * 0.07 + changed) * drift.cell_width;
            }
        }
        self.world.reset_interpolation();
    }
    fn keys(&self) -> Vec<Vec<u32>> {
        self.world
            .flakes
            .iter()
            .map(|f| Shape::Slab(f.slab.as_ref().unwrap()))
            .chain(
                self.world
                    .drifts
                    .values()
                    .map(|d| Shape::Drift(d, &self.world.scene)),
            )
            .map(|shape| {
                let mut key = Vec::new();
                assert!(shape.key(&mut key).is_some());
                key
            })
            .collect()
    }
    fn fingerprint(&self) -> u64 {
        let mut hash = 0xcbf29ce484222325_u64;
        for word in self
            .keys()
            .iter()
            .flatten()
            .copied()
            .chain(self.world.flakes.iter().flat_map(|f| {
                [
                    f.position.x.to_bits(),
                    f.position.y.to_bits(),
                    f.angle.to_bits(),
                ]
            }))
        {
            for byte in word.to_le_bytes() {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            }
        }
        hash
    }
}

fn delta(after: Stats, before: Stats) -> (u64, u64, u64) {
    (
        after.builds - before.builds,
        after.hits - before.hits,
        after.evictions - before.evictions,
    )
}
fn compare_pixels(gpu: &Gpu, fixture: &Fixture, settings: &Settings) {
    compare_component_pixels(gpu, fixture, settings, TestPlan::ALL);
}
fn compare_component_pixels(gpu: &Gpu, fixture: &Fixture, settings: &Settings, plan: TestPlan) {
    unsafe {
        let factory = gpu.context.GetFactory().unwrap().cast().unwrap();
        render_profile_reference_components(
            gpu,
            &factory,
            &fixture.screen,
            &fixture.world,
            settings,
            1.0,
            plan,
        )
        .unwrap();
        let reference = profile_pixels(gpu).unwrap();
        gpu.render_snow(
            &fixture.screen,
            &fixture.world,
            settings,
            RenderQuality::for_settings(settings, false),
            1.0,
        )
        .unwrap();
        let actual = profile_pixels(gpu).unwrap();
        let edges = profile_edges(&fixture.screen, &fixture.world, 1.0);
        let mut covered = 0;
        for (i, (a, b)) in actual
            .as_chunks::<4>()
            .0
            .iter()
            .zip(reference.as_chunks::<4>().0)
            .enumerate()
        {
            covered += usize::from(a[3] > 0 || b[3] > 0);
            if a.iter().zip(b).any(|(&a, &b)| a.abs_diff(b) > 3) {
                let p = Point::new((i % WIDTH) as f32 + 0.5, (i / WIDTH) as f32 + 0.5);
                assert!(
                    distance_to_profile(p, &edges) <= 1.5,
                    "benchmark changed coverage/gradient at {p:?}"
                );
            }
        }
        assert!(covered > 1000);
    }
}

#[test]
fn pending_errors_and_disjoint_timestamps_cannot_be_reported_as_valid_gpu_measurements() {
    assert!(!query_ready(S_FALSE).unwrap());
    assert!(query_ready(S_OK).unwrap());
    assert!(query_ready(E_FAIL).is_err());
    assert!(query_ready(HRESULT(2)).is_err());
    assert_eq!(interval_us(10, 110, 1_000_000, false), Some(100.0));
    for (a, b, f, d) in [
        (10, 110, 0, false),
        (110, 10, 1000, false),
        (10, 110, 1000, true),
    ] {
        assert!(interval_us(a, b, f, d).is_none());
    }
}
#[test]
fn reported_percentiles_use_nearest_rank_and_do_not_accept_invalid_or_empty_samples() {
    let summary = distribution((1..=20).rev().map(f64::from).collect()).unwrap();
    assert_eq!(
        (
            summary.samples,
            summary.mean_us,
            summary.p50_us,
            summary.p95_us,
            summary.max_us
        ),
        (20, 10.5, 10.0, 19.0, 20.0)
    );
    for values in [vec![], vec![f64::NAN], vec![f64::INFINITY], vec![-1.0]] {
        assert!(distribution(values).is_none());
    }
}
#[test]
fn benchmark_fixtures_change_motion_independently_from_exact_local_profiles() {
    let mut stable = Fixture::new(128, false);
    let mut changing = Fixture::new(128, true);
    let before = stable.keys();
    assert_eq!(before, changing.keys());
    let first = stable.world.flakes[0].position;
    stable.update(17);
    changing.update(17);
    assert_ne!(first, stable.world.flakes[0].position);
    assert_eq!(before, stable.keys());
    for (a, b) in before.iter().zip(changing.keys()) {
        assert_ne!(*a, b);
    }
    assert_eq!(stable.world.flakes.len(), 128);
    assert_eq!(stable.world.drifts.len(), 9);
    let mut repeat = Fixture::new(128, true);
    repeat.update(17);
    assert_eq!(repeat.fingerprint(), changing.fingerprint());
}

#[test]
#[ignore = "explicit release experiment; hardware D3D11/DWM; hidden, no Present; finite 60s bound"]
fn native_profile_cache_cost_without_presentation() {
    let build_profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    assert_eq!(
        build_profile, "release",
        "run the explicit experiment with --release"
    );
    let _serial = WEATHER_TEST.lock().unwrap();
    let _weather = Weather::new(false).unwrap();
    let settings = Settings::default();
    let quality = RenderQuality::for_settings(&settings, false);
    let deadline = Instant::now() + Duration::from_secs(60);
    let screen = Fixture::new(16, false).screen;
    let mut layer = Layer::new(screen, 1, false).unwrap();
    let hwnd = layer.hwnd;
    let gpu = layer.gpu.as_mut().unwrap();
    assert!(!unsafe { IsWindowVisible(hwnd) }.as_bool());
    assert_eq!(unsafe { gpu._bitmap.GetPixelSize() }.width as usize, WIDTH);
    assert_eq!(
        unsafe { gpu._bitmap.GetPixelSize() }.height as usize,
        HEIGHT
    );
    let queries = Queries::new(gpu).unwrap();
    let factory: ID2D1Factory1 = unsafe { gpu.context.GetFactory().unwrap().cast().unwrap() };
    for (name, slabs, changing) in [
        ("moving16", 16, false),
        ("moving128", 128, false),
        ("changing128", 128, true),
    ] {
        // Pixel readbacks and their CPU oracle run before timing, never between compared blocks.
        for backend in [Backend::Paths, Backend::Realizations] {
            gpu.profiles = Profiles::new(&factory, &gpu.context);
            gpu.profiles.set_test_plan(TestPlan::ALL);
            if backend == Backend::Paths {
                gpu.profiles.disable_realizations();
            }
            let mut fixture = Fixture::new(slabs, changing);
            fixture.update(WARMUP + FRAMES - 1);
            for _ in 0..2 {
                queries
                    .measure(deadline, || {
                        gpu.render_snow(&fixture.screen, &fixture.world, &settings, quality, 1.0)
                    })
                    .unwrap();
            }
            compare_pixels(gpu, &fixture, &settings);
            assert!(Instant::now() < deadline, "experiment duration exceeded");
        }
        let mut fingerprint = None;
        for (run, backend) in [
            Backend::Reference,
            Backend::Paths,
            Backend::Realizations,
            Backend::Realizations,
            Backend::Paths,
            Backend::Reference,
        ]
        .into_iter()
        .enumerate()
        {
            gpu.profiles = Profiles::new(&factory, &gpu.context);
            gpu.profiles.set_test_plan(TestPlan::ALL);
            if backend == Backend::Paths {
                gpu.profiles.disable_realizations();
            }
            let cached = backend != Backend::Reference;
            let mut fixture = Fixture::new(slabs, changing);
            let draw = |fixture: &Fixture| {
                if cached {
                    gpu.render_snow(&fixture.screen, &fixture.world, &settings, quality, 1.0)
                } else {
                    unsafe {
                        render_profile_reference_with_factory(
                            gpu,
                            &factory,
                            &fixture.screen,
                            &fixture.world,
                            &settings,
                            1.0,
                        )
                        .map(|_| ())
                    }
                }
            };
            for frame in 0..WARMUP {
                assert!(Instant::now() < deadline, "experiment duration exceeded");
                fixture.update(frame);
                queries.measure(deadline, || draw(&fixture)).unwrap();
            }
            let before = gpu.profiles.stats().0;
            let cpu_before = thread_cpu_us().unwrap();
            let mut samples = Vec::with_capacity(FRAMES);
            for frame in WARMUP..WARMUP + FRAMES {
                assert!(Instant::now() < deadline, "experiment duration exceeded");
                fixture.update(frame);
                samples.push(queries.measure(deadline, || draw(&fixture)).unwrap());
            }
            let thread_cpu = thread_cpu_us().unwrap() - cpu_before;
            let (stats, fills, strokes, usage) = gpu.profiles.stats();
            let (builds, hits, evictions) = delta(stats, before);
            let current = fixture.fingerprint();
            assert_eq!(*fingerprint.get_or_insert(current), current);
            let expected = (slabs + fixture.world.drifts.len()) * FRAMES;
            if cached {
                assert_eq!(builds as usize, if changing { expected } else { 0 });
                assert_eq!(hits as usize, if changing { 0 } else { expected });
                assert!(
                    usage.0 <= crate::profile_cache::MAX_ENTRIES
                        && usage.1 <= crate::profile_cache::MAX_POINTS
                );
                if !changing && backend == Backend::Realizations {
                    assert!(gpu.profiles.ready().0 > 0 && gpu.profiles.ready().1 > 0);
                }
            }
            let valid_gpu = samples
                .iter()
                .filter_map(|s| s.gpu_interval_us)
                .collect::<Vec<_>>();
            eprintln!(
                "profile_cost {}",
                serde_json::json!({
                    "schema_version": 2, "scope": "serialized_offscreen_profiles_only", "workload": name,
                    "build_profile": build_profile,
                    "run": run + 1, "backend": backend.name(),
                    "target": [WIDTH, HEIGHT], "slabs": slabs, "drifts": fixture.world.drifts.len(),
                    "warmup": WARMUP, "frames": FRAMES, "fixture_hash": format!("{current:016x}"),
                    "submission_wall": distribution(samples.iter().map(|s| s.submission_us).collect()),
                    "completed_wall": distribution(samples.iter().map(|s| s.completed_us).collect()),
                    "gpu_timeline_not_busy": distribution(valid_gpu),
                    "invalid_gpu_samples": samples.iter().filter(|s| s.gpu_interval_us.is_none()).count(),
                    "pixel_shader_invocations_min": samples.iter().map(|s| s.pixel_shader_invocations).min(),
                    "batch_thread_cpu_us_per_frame": thread_cpu as f64 / FRAMES as f64,
                    "reference_paths": if cached { 0 } else { expected }, "cache_builds": builds,
                    "cache_hits": hits, "cache_evictions": evictions, "cache_usage": [usage.0, usage.1],
                    "cumulative_fill_attempts": fills, "cumulative_stroke_attempts": strokes,
                    "visible_fps_acceptance": "not_run"
                })
            );
            assert!(!unsafe { IsWindowVisible(hwnd) }.as_bool());
        }
    }
    gpu.profiles.clear();
    assert_eq!(gpu.profiles.stats().3, (0, 0));
    assert!(Instant::now() < deadline, "experiment duration exceeded");
}
