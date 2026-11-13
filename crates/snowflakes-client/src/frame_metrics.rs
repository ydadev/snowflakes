//! Optional counters of S_OK Present calls. These do not measure displayed pixels.
use crate::quality::RenderQuality;
use serde::{Deserialize, Serialize};
use snowflakes_protocol::Settings;

pub const MAX_DISPLAYS: usize = 32;
pub const UPPER_US: [u64; 31] = [
    1_000, 2_000, 4_000, 8_000, 10_000, 12_000, 14_000, 16_000, 17_000, 18_000, 19_000, 20_000,
    21_000, 22_000, 23_000, 24_000, 25_000, 26_000, 28_000, 30_000, 33_000, 34_000, 40_000, 50_000,
    66_000, 100_000, 200_000, 500_000, 1_000_000, 5_000_000, 10_000_000,
];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counters {
    pub presents: u64,
    pub intervals: u64,
    pub interval_sum_ns: u64,
    pub histogram: [u64; 32],
    pub breaks: u64,
    pub continuous: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Display {
    pub id: String,
    pub epoch: u64,
    pub counters: Counters,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub schema_version: u8,
    #[serde(default)]
    pub configuration_epoch: u64,
    pub elapsed_ns: u64,
    pub sample_age_us: u64,
    pub display_latency_measured: bool,
    pub upper_us: [u64; 31],
    pub omitted_displays: usize,
    pub displays: Vec<Display>,
}
impl Snapshot {
    pub fn new(
        elapsed_ns: u64,
        configuration_epoch: u64,
        displays: impl Iterator<Item = Display>,
    ) -> Self {
        let mut snapshot = Self {
            schema_version: 2,
            configuration_epoch,
            elapsed_ns,
            sample_age_us: 0,
            display_latency_measured: false,
            upper_us: UPPER_US,
            omitted_displays: 0,
            displays: Vec::new(),
        };
        for display in displays {
            if snapshot.displays.len() < MAX_DISPLAYS {
                snapshot.displays.push(display);
            } else {
                snapshot.omitted_displays += 1;
            }
        }
        snapshot
    }
}
#[derive(Default)]
pub struct Capture {
    last: Option<Configuration>,
    pub epoch: u64,
}
struct Configuration {
    settings: Option<Settings>,
    quality: Option<RenderQuality>,
    clear_epoch: u64,
    enabled: bool,
}
impl Capture {
    /// Track parameters actually observed by the renderer, even if A -> B -> A
    /// completes before the external collector's next status poll.
    pub fn observe(
        &mut self,
        settings: Option<&Settings>,
        quality: Option<RenderQuality>,
        clear_epoch: u64,
        enabled: bool,
    ) -> bool {
        if self.last.as_ref().is_some_and(|last| {
            last.settings.as_ref() == settings
                && last.quality == quality
                && last.clear_epoch == clear_epoch
                && last.enabled == enabled
        }) {
            return false;
        }
        self.epoch += 1;
        self.last = Some(Configuration {
            settings: settings.cloned(),
            quality,
            clear_epoch,
            enabled,
        });
        true
    }
}
#[derive(Default)]
pub struct Meter {
    last_ns: Option<u64>,
    pub counters: Counters,
}
impl Meter {
    /// Call only after the corresponding swap chain returned S_OK.
    pub fn submitted(&mut self, elapsed_ns: u64) {
        if let Some(last) = self.last_ns {
            if elapsed_ns > last {
                let delta = elapsed_ns - last;
                let upper_us = delta.div_ceil(1_000);
                let bucket = UPPER_US.partition_point(|&upper| upper < upper_us);
                self.counters.histogram[bucket] += 1;
                self.counters.intervals += 1;
                self.counters.interval_sum_ns += delta;
            } else {
                self.counters.breaks += 1;
            }
        }
        self.counters.presents += 1;
        self.counters.continuous = true;
        self.last_ns = Some(elapsed_ns);
    }
    /// Deliberate inactivity breaks continuity, not the accumulated counters.
    pub fn suspend(&mut self) {
        if self.last_ns.take().is_some() {
            self.counters.breaks += 1;
        }
        self.counters.continuous = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_b_a_parameter_changes_remain_visible_to_a_later_collector() {
        let original = Settings::default();
        let mut changed = original.clone();
        changed.snow.intensity_percent = 70;
        let quality = Some(RenderQuality::for_settings(&original, false));
        let mut capture = Capture::default();
        assert!(capture.observe(Some(&original), quality, 0, true));
        assert!(!capture.observe(Some(&original), quality, 0, true));
        assert!(capture.observe(Some(&changed), quality, 0, true));
        assert!(capture.observe(Some(&original), quality, 0, true));
        assert_eq!(capture.epoch, 3);
    }
    #[test]
    fn quality_clear_and_global_gate_changes_do_not_rewrite_settings() {
        let settings = Settings::default();
        let original = settings.clone();
        let normal = Some(RenderQuality::for_settings(&settings, false));
        let reduced = Some(RenderQuality::for_settings(&settings, true));
        let mut capture = Capture::default();
        capture.observe(Some(&settings), normal, 0, true);
        assert!(capture.observe(Some(&settings), reduced, 0, true));
        assert!(capture.observe(Some(&settings), reduced, 1, true));
        assert!(capture.observe(Some(&settings), reduced, 1, false));
        assert!(capture.observe(None, None, 1, false));
        assert!(!capture.observe(None, None, 1, false));
        assert_eq!(capture.epoch, 5);
        assert_eq!(settings, original);
    }
    #[test]
    fn schema_one_can_be_read_without_claiming_a_configuration_epoch() {
        let mut legacy = serde_json::to_value(Snapshot::new(1, 5, std::iter::empty())).unwrap();
        legacy["schema_version"] = 1.into();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("configuration_epoch");
        let snapshot: Snapshot = serde_json::from_value(legacy).unwrap();
        assert_eq!(snapshot.schema_version, 1);
        assert_eq!(snapshot.configuration_epoch, 0);
    }
    #[test]
    fn sixty_and_thirty_hz_have_distinct_cumulative_distributions() {
        for (step, count, upper) in [(16_666_667, 600, 17_000), (33_333_334, 300, 34_000)] {
            let mut meter = Meter::default();
            for i in 0..=count {
                meter.submitted(i * step);
            }
            assert_eq!(meter.counters.presents, count + 1);
            assert_eq!(meter.counters.intervals, count);
            assert_eq!(meter.counters.interval_sum_ns, count * step);
            let index = UPPER_US.iter().position(|&v| v == upper).unwrap();
            assert_eq!(meter.counters.histogram[index], count);
            assert_eq!(meter.counters.histogram.iter().sum::<u64>(), count);
        }
    }
    #[test]
    fn pause_excludes_the_gap_and_retains_pre_pause_evidence() {
        let mut meter = Meter::default();
        meter.submitted(0);
        meter.submitted(20_000_000);
        meter.suspend();
        meter.suspend();
        assert!(!meter.counters.continuous);
        meter.submitted(10_000_000_000);
        meter.submitted(10_020_000_000);
        assert_eq!(meter.counters.presents, 4);
        assert_eq!(meter.counters.intervals, 2);
        assert_eq!(meter.counters.interval_sum_ns, 40_000_000);
        assert_eq!(meter.counters.breaks, 1);
    }
    #[test]
    fn exact_threshold_is_inclusive_but_one_nanosecond_over_is_not() {
        let mut meter = Meter::default();
        meter.submitted(0);
        meter.submitted(25_000_000);
        meter.submitted(50_000_001);
        let index = UPPER_US.iter().position(|&v| v == 25_000).unwrap();
        assert_eq!(meter.counters.histogram[index], 1);
        assert_eq!(meter.counters.histogram[index + 1], 1);
        meter.submitted(50_000_001);
        assert_eq!(meter.counters.breaks, 1);
        assert_eq!(meter.counters.intervals, 2);
    }
    #[test]
    fn stall_is_measured_and_overflow_is_kept_instead_of_capped() {
        let mut meter = Meter::default();
        meter.submitted(0);
        meter.submitted(30_000_000_000);
        assert_eq!(meter.counters.histogram[31], 1);
        assert_eq!(meter.counters.interval_sum_ns, 30_000_000_000);
    }
    #[test]
    fn snapshot_bound_is_explicit_and_fits_the_ipc_budget() {
        let snapshot = Snapshot::new(
            1,
            1,
            (0..65).map(|index| Display {
                id: format!("display-{index}"),
                epoch: index,
                counters: Counters {
                    histogram: [u64::MAX; 32],
                    ..Default::default()
                },
            }),
        );
        assert_eq!(snapshot.displays.len(), MAX_DISPLAYS);
        assert_eq!(snapshot.omitted_displays, 33);
        assert!(!snapshot.display_latency_measured);
        assert!(serde_json::to_vec(&snapshot).unwrap().len() < 40_000);
    }
}
