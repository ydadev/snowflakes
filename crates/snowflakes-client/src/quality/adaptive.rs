//! Bounded presentation policy. Work duration is wall cost, not GPU utilization.
use super::RenderQuality;
use crate::geometry::{Rect, Scene};
use snowflakes_protocol::{Settings, WeatherMode};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

const LARGE_ENTER: u64 = 2 * 3840 * 2160;
const LARGE_EXIT: u64 = LARGE_ENTER * 3 / 4;
const SLOW_TIME: Duration = Duration::from_secs(1);
const FAST_TIME: Duration = Duration::from_secs(5);
const MAX_CONTRIBUTION: Duration = Duration::from_millis(100);
const MAX_IDLE: Duration = Duration::from_millis(250);

/// Sum owned render-target areas, not the bounding desktop or logical DIP area.
fn output_pixels(rectangles: impl IntoIterator<Item = Rect>) -> u64 {
    rectangles.into_iter().fold(0_u64, |sum, rect| {
        let width = f64::from(rect.right) - f64::from(rect.left);
        let height = f64::from(rect.bottom) - f64::from(rect.top);
        if !width.is_finite() || !height.is_finite() {
            return u64::MAX;
        }
        // Float-to-integer conversion saturates; no wrap on malformed geometry.
        sum.saturating_add((width.max(0.0) * height.max(0.0)).ceil() as u64)
    })
}

/// Scene already contains only the user's selected displays.
pub fn active_output_pixels(
    scene: &Scene,
    fullscreen: &BTreeSet<u64>,
    pause_fullscreen: bool,
) -> u64 {
    output_pixels(
        scene
            .screens
            .iter()
            .filter(|screen| !pause_fullscreen || !fullscreen.contains(&screen.key))
            .map(|screen| screen.rect),
    )
}

#[derive(Default)]
pub struct Governor {
    base: Option<RenderQuality>,
    pixels: u64,
    large: bool,
    cost_reduced: bool,
    last: Option<Instant>,
    slow_time: Duration,
    fast_time: Duration,
}

pub struct FrameWork {
    pub completed: Instant,
    pub work: Duration,
    pub accepted: bool,
    pub reconfigured: bool,
    pub active: bool,
}
impl Governor {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    fn clear_evidence(&mut self) {
        self.last = None;
        self.slow_time = Duration::ZERO;
        self.fast_time = Duration::ZERO;
    }
    pub fn select(
        &mut self,
        settings: &Settings,
        low_power: bool,
        pixels: u64,
        active: bool,
    ) -> RenderQuality {
        let base = RenderQuality::for_settings(settings, low_power);
        if !active || pixels == 0 || settings.weather.mode != WeatherMode::Snow {
            self.reset();
            return base;
        }
        if self.base != Some(base) {
            self.reset();
            self.base = Some(base);
        }
        if self.pixels != pixels {
            self.pixels = pixels;
            self.cost_reduced = false;
            self.clear_evidence();
        }
        self.large = if self.large {
            pixels > LARGE_EXIT
        } else {
            pixels >= LARGE_ENTER
        };
        self.current().unwrap_or(base)
    }
    pub fn current(&self) -> Option<RenderQuality> {
        self.base.map(|mut quality| {
            if self.large || self.cost_reduced {
                quality.target_fps = quality.target_fps.min(30);
                quality.secondary_details = false;
            }
            quality
        })
    }
    /// Return the NEXT frame's quality. The rendered quality remains a separate
    /// Copy value, so its report and metric epoch cannot be changed retroactively.
    pub fn complete(
        &mut self,
        settings: &Settings,
        low_power: bool,
        pixels: u64,
        frame: FrameWork,
    ) -> RenderQuality {
        if frame.reconfigured {
            self.reset();
        }
        let next = self.select(settings, low_power, pixels, frame.active);
        self.observe(
            frame.completed,
            frame.work,
            frame.accepted && frame.active && !frame.reconfigured,
        );
        self.current().unwrap_or(next)
    }
    /// Call once per completed update. Skipped/occluded/error/init frames do not
    /// supply work evidence. The caller separately resets on known discontinuity.
    pub fn observe(&mut self, completed: Instant, work: Duration, accepted: bool) {
        let Some(base) = self.base else {
            return;
        };
        if !accepted || self.large || !base.secondary_details {
            self.clear_evidence();
            return;
        }
        let previous = self.last.replace(completed);
        let Some(elapsed) = previous.and_then(|last| completed.checked_duration_since(last)) else {
            self.clear_evidence();
            self.last = Some(completed);
            return;
        };
        if elapsed.is_zero() || work > elapsed || elapsed > work.saturating_add(MAX_IDLE) {
            self.clear_evidence();
            self.last = Some(completed);
            return;
        }
        let contribution = elapsed.min(MAX_CONTRIBUTION);
        if !self.cost_reduced && work >= base.interval() {
            self.fast_time = Duration::ZERO;
            self.slow_time = self.slow_time.saturating_add(contribution).min(SLOW_TIME);
            if self.slow_time >= SLOW_TIME {
                self.cost_reduced = true;
                self.clear_evidence();
                self.last = Some(completed);
            }
        } else if self.cost_reduced && work <= base.interval() / 2 {
            self.slow_time = Duration::ZERO;
            self.fast_time = self.fast_time.saturating_add(contribution).min(FAST_TIME);
            if self.fast_time >= FAST_TIME {
                self.cost_reduced = false;
                self.clear_evidence();
                self.last = Some(completed);
            }
        } else {
            self.clear_evidence();
            self.last = Some(completed);
        }
    }
}

#[cfg(test)]
mod tests;
