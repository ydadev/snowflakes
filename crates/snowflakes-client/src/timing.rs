//! Bounded monotonic simulation stepping. Sleeping or pausing never creates
//! precipitation for the elapsed inactive interval.
use std::time::{Duration, Instant};

pub const STEP: Duration = Duration::from_nanos(16_666_667);
pub const MAX_STEPS: u32 = 5;
pub const DISCONTINUITY: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Steps {
    pub count: u32,
    pub discarded: Duration,
}

#[derive(Default)]
pub struct StepClock {
    last: Option<Instant>,
    remainder: Duration,
}

impl StepClock {
    /// Position between the previous and current fixed-step poses. No extrapolation.
    pub fn fraction(&self) -> f32 {
        (self.remainder.as_secs_f64() / STEP.as_secs_f64()) as f32
    }
    pub fn reset(&mut self) {
        self.last = None;
        self.remainder = Duration::ZERO;
    }

    pub fn advance(&mut self, now: Instant, running: bool) -> Steps {
        if !running {
            self.reset();
            return Steps {
                count: 0,
                discarded: Duration::ZERO,
            };
        }
        let Some(previous) = self.last.replace(now) else {
            return Steps {
                count: 0,
                discarded: Duration::ZERO,
            };
        };
        let Some(delta) = now.checked_duration_since(previous) else {
            self.reset();
            return Steps {
                count: 0,
                discarded: Duration::ZERO,
            };
        };
        if delta >= DISCONTINUITY {
            self.remainder = Duration::ZERO;
            return Steps {
                count: 0,
                discarded: delta,
            };
        }
        self.remainder += delta;
        let available = (self.remainder.as_nanos() / STEP.as_nanos()) as u32;
        let count = available.min(MAX_STEPS);
        let discarded = STEP * available.saturating_sub(count);
        self.remainder -= STEP * available;
        Steps { count, discarded }
    }
}
