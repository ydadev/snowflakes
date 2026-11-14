//! Existing bounded graphics retry policy and persistent, sanitized failure reason.
use std::time::{Duration, Instant};

pub(super) struct Recovery {
    failures: u8,
    retry: Instant,
    pub error: Option<i32>,
}
impl Recovery {
    pub fn new(now: Instant) -> Self {
        Self {
            failures: 0,
            retry: now,
            error: None,
        }
    }
    pub fn can_create(&self, now: Instant) -> bool {
        self.failures < 3 && now >= self.retry
    }
    pub fn failed(&mut self, now: Instant, code: i32) {
        self.failures = self.failures.saturating_add(1);
        self.error = Some(code);
        self.retry = now + Duration::from_secs(1 << self.failures.min(3));
    }
    pub fn submitted(&mut self, accepted: bool) {
        if accepted {
            self.error = None;
        }
    }
    pub fn renderer(&self, normal: &'static str) -> &'static str {
        if self.failures >= 3 {
            "graphics_unavailable"
        } else if self.error.is_some() {
            "graphics_recovering"
        } else {
            normal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphics_retry_retains_reason_between_attempts_and_stops_after_three_failures() {
        let now = Instant::now();
        let mut state = Recovery::new(now);
        assert!(state.can_create(now));
        for (elapsed, delay, code) in [(0, 2, -1), (2, 4, -2), (6, 8, -3)] {
            let failed_at = now + Duration::from_secs(elapsed);
            state.failed(failed_at, code);
            assert_eq!(state.error, Some(code));
            assert!(
                !state.can_create(failed_at + Duration::from_secs(delay) - Duration::from_nanos(1))
            );
            state.submitted(false); // no layers, skipped tick, or DXGI_STATUS_OCCLUDED
            assert_eq!(state.error, Some(code));
            if code != -3 {
                assert_eq!(state.renderer("d3d11_direct2d"), "graphics_recovering");
                assert!(state.can_create(failed_at + Duration::from_secs(delay)));
            }
        }
        assert_eq!(
            state.renderer("d3d11_direct2d_idle"),
            "graphics_unavailable"
        );
        assert!(!state.can_create(now + Duration::from_secs(86_400)));
        let reset = Recovery::new(now + Duration::from_secs(86_400));
        assert!(reset.can_create(now + Duration::from_secs(86_400)));
        assert_eq!(reset.error, None);
    }

    #[test]
    fn accepted_submission_clears_the_reason_without_resetting_the_failure_budget() {
        let now = Instant::now();
        let mut state = Recovery::new(now);
        for code in [-1, -2] {
            state.failed(now, code);
            state.submitted(true);
            assert_eq!(state.error, None);
            assert_eq!(state.renderer("d3d11_direct2d"), "d3d11_direct2d");
        }
        state.failed(now, -3);
        assert_eq!(state.renderer("d3d11_direct2d"), "graphics_unavailable");
        assert!(!state.can_create(now + Duration::from_secs(100)));
    }
}
