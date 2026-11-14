//! Finite, unpaced model benchmark. Never creates a native window or client session.
use serde::Serialize;
use serde_json::{Value, json};
use snowflakes_client::{
    geometry::{Rect, Scene, Screen, Window},
    snow::{BudgetSnapshot, DT, MAX_FLAKES, World},
};
use snowflakes_protocol::Settings;
use std::{collections::BTreeSet, time::Instant};

const EPISODE_STEPS: usize = 120;
const SAMPLE_EVERY: usize = 30;
const BASELINE_WINDOWS: u64 = 10;
const MATERIAL_ROOFS_PER_SCREEN: u64 = 126;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Scenario {
    Baseline,
    MaterialStress,
}

struct Options {
    scenario: Scenario,
    steps: usize,
    warmup: usize,
    seed: u64,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            scenario: Scenario::Baseline,
            steps: 3600,
            warmup: 120,
            seed: 0x5eed,
        }
    }
}
fn parse(args: impl IntoIterator<Item = String>) -> Result<Option<Options>, &'static str> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    let mut seen = BTreeSet::new();
    while let Some(key) = args.next() {
        if key == "--help" {
            return Ok(None);
        }
        if !seen.insert(key.clone()) {
            return Err("duplicate_option");
        }
        let value = args.next().ok_or("missing_value")?;
        match key.as_str() {
            "--scenario" => {
                options.scenario = match value.as_str() {
                    "baseline" => Scenario::Baseline,
                    "material-stress" => Scenario::MaterialStress,
                    _ => return Err("invalid_scenario"),
                }
            }
            "--steps" => {
                options.steps = value
                    .parse()
                    .ok()
                    .filter(|n| (60..=36_000).contains(n))
                    .ok_or("invalid_steps")?
            }
            "--warmup" => {
                options.warmup = value
                    .parse()
                    .ok()
                    .filter(|n| (0..=3600).contains(n))
                    .ok_or("invalid_warmup")?
            }
            "--seed" => {
                options.seed = value
                    .parse()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or("invalid_seed")?
            }
            _ => return Err("unknown_option"),
        }
    }
    Ok(Some(options))
}

struct Fixture {
    world: World,
    settings: Settings,
}
fn window(key: u64, left: f32, top: f32, width: f32) -> Window {
    Window {
        key,
        rect: Rect {
            left,
            top,
            right: left + width,
            bottom: top + 120.0,
        },
        visible: true,
        collect: true,
    }
}
fn prepare(scenario: Scenario, seed: u64) -> Result<Fixture, &'static str> {
    let mut scene = Scene::default();
    let stress = scenario == Scenario::MaterialStress;
    for i in 0..if stress { 2 } else { 1 } {
        let (width, height, scale) = if stress {
            (3840.0, 2160.0, 2.0)
        } else {
            (1920.0, 1080.0, 1.0)
        };
        scene.screens.push(Screen {
            key: i + 1,
            persistent_id: true,
            rect: Rect {
                left: i as f32 * width,
                top: 0.0,
                right: (i + 1) as f32 * width,
                bottom: height,
            },
            floor: height,
            scale,
        });
    }
    let mut settings = Settings::default();
    settings.snow.intensity_percent = if stress { 0 } else { 50 };
    settings.snow.drift_size_percent = if stress { 100 } else { 35 };
    settings.snow.melting = !stress;
    if !stress {
        for i in 0..BASELINE_WINDOWS {
            scene.windows.push(window(
                100 + i,
                50.0 + (i % 5) as f32 * 370.0,
                150.0 + (i / 5) as f32 * 300.0,
                280.0,
            ));
        }
        let mut world = World::new(scene, seed);
        for drift in world.drifts.values_mut() {
            drift
                .mass
                .fill(drift.cell_width * if drift.support.ground { 2.0 } else { 1.0 });
        }
        world.introduced_mass = world.total_mass();
        world.seed_air(&settings);
        return Ok(Fixture { world, settings });
    }
    for screen in &scene.screens {
        scene.windows.push(window(
            6 + screen.key,
            screen.rect.left + 100.0,
            200.0,
            400.0,
        ));
    }
    let mut world = World::new(scene.clone(), seed);
    for drift in world.drifts.values_mut().filter(|d| !d.support.ground) {
        drift.mass.fill(drift.cell_width);
    }
    world.introduced_mass = world.total_mass();
    scene.windows.clear();
    world.set_scene(scene.clone());
    world.step(&settings, None);
    for screen in &scene.screens {
        let body = world
            .flakes
            .iter()
            .find(|b| {
                world
                    .scene
                    .nearest_screen(b.position)
                    .is_some_and(|s| s.key == screen.key)
            })
            .ok_or("material_fixture_missing_source")?
            .clone();
        while world.release(body.clone()) {}
    }
    if world.flakes.len() != MAX_FLAKES * 2 {
        return Err("material_fixture_not_saturated");
    }
    for screen in &scene.screens {
        for i in 0..MATERIAL_ROOFS_PER_SCREEN {
            scene.windows.push(window(
                1000 + screen.key * 1000 + i,
                screen.rect.left + 300.0 + (i % 4) as f32 * 700.0,
                320.0 + (i / 4) as f32 * 32.0,
                500.0,
            ));
        }
    }
    world.set_scene(scene.clone());
    for drift in world.drifts.values_mut().filter(|d| !d.support.ground) {
        drift.mass.fill(drift.cell_width);
        world.introduced_mass += drift.total_mass();
    }
    scene.windows.clear();
    world.set_scene(scene);
    if world.pending_collapse_count() != 128 {
        return Err("material_fixture_missing_queue");
    }
    Ok(Fixture { world, settings })
}

#[derive(Serialize)]
struct Observed {
    samples: usize,
    min_active_bodies: usize,
    peak: BudgetSnapshot,
    max_mass_relative_error: f64,
}
impl Observed {
    fn new() -> Self {
        Self {
            samples: 0,
            min_active_bodies: usize::MAX,
            peak: BudgetSnapshot::default(),
            max_mass_relative_error: 0.0,
        }
    }
    fn sample(&mut self, world: &World) -> Result<(), &'static str> {
        let b = world.budget_snapshot();
        if b.active_bodies > b.max_active_bodies
            || b.pending_packets > b.max_pending_packets
            || b.pending_bodies > b.max_pending_bodies
            || b.drift_cells > 65_536
            || b.supports > 256
        {
            return Err("model_count_bound_failed");
        }
        let error = (world.introduced_mass - world.removed_mass - world.total_mass()).abs()
            / world.introduced_mass.max(1.0);
        if !error.is_finite() || error > 0.001 {
            return Err("model_mass_balance_failed");
        }
        self.max_mass_relative_error = self.max_mass_relative_error.max(error);
        self.min_active_bodies = self.min_active_bodies.min(b.active_bodies);
        self.samples += 1;
        macro_rules! maxima { ($($field:ident),*) => { $(self.peak.$field = self.peak.$field.max(b.$field);)* }; }
        maxima!(
            active_bodies,
            active_slabs,
            active_profile_cells,
            pending_packets,
            pending_bodies,
            pending_profile_cells,
            spill_bodies,
            supports,
            drift_cells,
            falling_masks,
            max_active_bodies,
            max_pending_packets,
            max_pending_bodies
        );
        Ok(())
    }
}
fn distribution(mut nanos: Vec<u64>) -> Option<Value> {
    if nanos.is_empty() {
        return None;
    }
    nanos.sort_unstable();
    let at = |percent: usize| nanos[(nanos.len() * percent).div_ceil(100) - 1] as f64 / 1000.0;
    Some(
        json!({ "samples": nanos.len(), "mean_us": nanos.iter().map(|&n| u128::from(n)).sum::<u128>() as f64 / nanos.len() as f64 / 1000.0,
        "p50_us": at(50), "p95_us": at(95), "p99_us": at(99), "max_us": *nanos.last().unwrap() as f64 / 1000.0,
        "over_physics_step_budget": nanos.iter().filter(|&&n| n as f64 > f64::from(DT) * 1e9).count() }),
    )
}

#[derive(Serialize)]
struct ProcessSample {
    cpu_100ns: u64,
    private_bytes: usize,
    working_set_bytes: usize,
    peak_private_bytes_process_lifetime: usize,
    peak_working_set_bytes_process_lifetime: usize,
}
#[cfg(windows)]
fn process_sample() -> Result<Option<ProcessSample>, &'static str> {
    use windows::Win32::{
        Foundation::FILETIME,
        System::{
            ProcessStatus::{
                GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
            },
            Threading::{GetCurrentProcess, GetProcessTimes},
        },
    };
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    let bytes = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    let mut memory = PROCESS_MEMORY_COUNTERS_EX {
        cb: bytes,
        ..Default::default()
    };
    // SAFETY: the current-process pseudo-handle is valid and is not closed.
    // All output structs live through the calls. EX has the documented counters
    // prefix, and cb describes the entire allocated EX, not only that prefix.
    unsafe {
        let process = GetCurrentProcess();
        GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user)
            .map_err(|_| "process_times_failed")?;
        GetProcessMemoryInfo(
            process,
            (&raw mut memory).cast::<PROCESS_MEMORY_COUNTERS>(),
            bytes,
        )
        .map_err(|_| "process_memory_failed")?;
    }
    let ticks = |v: FILETIME| (u64::from(v.dwHighDateTime) << 32) | u64::from(v.dwLowDateTime);
    Ok(Some(ProcessSample {
        cpu_100ns: ticks(kernel)
            .checked_add(ticks(user))
            .ok_or("process_time_overflow")?,
        private_bytes: memory.PrivateUsage,
        working_set_bytes: memory.WorkingSetSize,
        peak_private_bytes_process_lifetime: memory.PeakPagefileUsage,
        peak_working_set_bytes_process_lifetime: memory.PeakWorkingSetSize,
    }))
}
#[cfg(not(windows))]
fn process_sample() -> Result<Option<ProcessSample>, &'static str> {
    Ok(None)
}

fn run(options: Options) -> Result<Value, &'static str> {
    if cfg!(debug_assertions) {
        return Err("release_build_required");
    }
    let initial = Instant::now();
    let mut fixture = Some(prepare(options.scenario, options.seed)?);
    let initial_setup_us = initial.elapsed().as_secs_f64() * 1e6;
    for i in 0..options.warmup {
        if options.scenario == Scenario::MaterialStress && i > 0 && i % EPISODE_STEPS == 0 {
            drop(fixture.take());
            fixture = Some(prepare(options.scenario, options.seed)?);
        }
        let f = fixture.as_mut().unwrap();
        f.world.step(&f.settings, None);
    }
    if options.scenario == Scenario::MaterialStress {
        drop(fixture.take());
        fixture = Some(prepare(options.scenario, options.seed)?);
    }
    let mut observed = Observed::new();
    observed.sample(&fixture.as_ref().unwrap().world)?;
    let mut steps = Vec::with_capacity(options.steps);
    let mut resets = Vec::new();
    let processors = std::thread::available_parallelism()
        .map_err(|_| "parallelism_unavailable")?
        .get();
    let started = Instant::now();
    let before = process_sample()?;
    for i in 0..options.steps {
        if started.elapsed().as_secs() >= 300 {
            return Err("benchmark_time_limit");
        }
        if options.scenario == Scenario::MaterialStress && i > 0 && i % EPISODE_STEPS == 0 {
            observed.sample(&fixture.as_ref().unwrap().world)?;
            let reset = Instant::now();
            drop(fixture.take());
            fixture = Some(prepare(options.scenario, options.seed)?);
            resets.push(reset.elapsed().as_nanos() as u64);
            observed.sample(&fixture.as_ref().unwrap().world)?;
        }
        let f = fixture.as_mut().unwrap();
        let step = Instant::now();
        f.world.step(&f.settings, None);
        steps.push(step.elapsed().as_nanos() as u64);
        if i % SAMPLE_EVERY == 0 || i + 1 == options.steps {
            observed.sample(&f.world)?;
        }
    }
    let after = process_sample()?;
    let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
    let cpu_ms = match (&before, &after) {
        (Some(a), Some(b)) => Some(
            b.cpu_100ns
                .checked_sub(a.cpu_100ns)
                .ok_or("process_time_regressed")? as f64
                / 10_000.0,
        ),
        _ => None,
    };
    let f = fixture.as_ref().unwrap();
    Ok(
        json!({ "schema_version": 1, "tool": "snow-model-bench", "version": env!("CARGO_PKG_VERSION"),
        "build_profile": "release", "scope": "unpaced_model_only", "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
        "scenario": options.scenario, "seed": options.seed, "warmup_steps": options.warmup, "measured_steps": options.steps,
        "physics_dt_seconds": DT, "episode_steps": if options.scenario == Scenario::MaterialStress { Some(EPISODE_STEPS) } else { None },
        "screens": f.world.scene.screens.iter().map(|s| json!({ "width_px": s.rect.width(), "height_px": s.rect.height(), "scale": s.scale })).collect::<Vec<_>>(),
        "snow_settings": f.settings.snow, "initial_setup_us": initial_setup_us,
        "fixture": { "open_windows_during_steps": f.world.scene.windows.len(),
            "closed_stocked_roofs_per_episode": if options.scenario == Scenario::MaterialStress { 2 * (MATERIAL_ROOFS_PER_SCREEN + 1) } else { 0 },
            "active_pool_seed": if options.scenario == Scenario::MaterialStress { "cohesive_source_clones" } else { "snowfall" } },
        "measured_wall_ms_including_resets_and_observer": wall_ms, "step_time": distribution(steps), "reset_time": distribution(resets),
        "process": { "before": before, "after": after, "cpu_ms": cpu_ms, "available_logical_processors": processors,
            "cpu_percent_normalized_available": cpu_ms.map(|cpu| cpu / wall_ms / processors as f64 * 100.0),
            "native_metrics_unavailable_reason": if cfg!(windows) { None } else { Some("unsupported_platform") } },
        "observed": observed, "validation": { "mass_and_count_bounds": "passed", "product_performance_acceptance": "not_run" } }),
    )
}
fn main() {
    let result = parse(std::env::args().skip(1)).and_then(|options| match options {
        None => { println!("snow_model_bench --scenario baseline|material-stress --steps 60..36000 --warmup 0..3600 --seed 1..u64_max"); Ok(()) }
        Some(options) => run(options).and_then(|report| { println!("{}", serde_json::to_string(&report).map_err(|_| "report_encoding_failed")?); Ok(()) }),
    });
    if let Err(code) = result {
        eprintln!("{code}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arguments_bound_work_and_reject_ambiguous_runs() {
        for args in [
            vec!["--steps", "0"],
            vec!["--steps", "36001"],
            vec!["--warmup", "3601"],
            vec!["--seed", "0"],
            vec!["--scenario", "rain"],
            vec!["--steps"],
            vec!["--steps", "60", "--steps", "60"],
        ] {
            assert!(parse(args.into_iter().map(str::to_owned)).is_err());
        }
        assert!(parse(["--help".to_owned()]).unwrap().is_none());
        let options = parse(["--steps", "60", "--warmup", "0"].map(str::to_owned))
            .unwrap()
            .unwrap();
        assert_eq!((options.steps, options.warmup), (60, 0));
    }
    #[test]
    fn duration_quantiles_use_nearest_rank_and_keep_the_upper_tail() {
        let result = distribution((1..=20).map(|n| n * 1000).collect()).unwrap();
        assert_eq!(result["p50_us"], 10.0);
        assert_eq!(result["p95_us"], 19.0);
        assert_eq!(result["p99_us"], 20.0);
        assert_eq!(result["mean_us"], 10.5);
        assert!(distribution(Vec::new()).is_none());
    }
    #[test]
    fn scenarios_prove_snowfall_and_material_load_separately() {
        for scenario in [Scenario::Baseline, Scenario::MaterialStress] {
            let fixture = prepare(scenario, 0x5eed).unwrap();
            let b = fixture.world.budget_snapshot();
            if scenario == Scenario::Baseline {
                assert_eq!(b.active_bodies, 350);
                assert_eq!(b.active_slabs, 0);
                assert_eq!(b.supports, BASELINE_WINDOWS as usize + 1);
            } else {
                assert_eq!(b.active_bodies, 4800);
                assert_eq!(b.active_slabs, 4800);
                assert_eq!(b.pending_packets, 128);
                assert!(b.pending_bodies >= 128 && b.pending_bodies <= 1024);
            }
            Observed::new().sample(&fixture.world).unwrap();
        }
    }
    #[cfg(windows)]
    #[test]
    fn native_metrics_read_only_the_current_process() {
        let sample = process_sample().unwrap().unwrap();
        assert!(sample.private_bytes > 0 && sample.working_set_bytes > 0);
        assert!(sample.peak_private_bytes_process_lifetime >= sample.private_bytes);
        assert!(sample.peak_working_set_bytes_process_lifetime >= sample.working_set_bytes);
    }
}
