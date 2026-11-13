//! Presentation quality is derived without editing the physical weather profile.
use snowflakes_protocol::Settings;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderQuality {
    pub target_fps: u8,
    pub secondary_details: bool,
}
impl RenderQuality {
    pub fn for_settings(settings: &Settings, low_power: bool) -> Self {
        let reduced = settings.performance.battery_saver && low_power;
        Self {
            target_fps: settings
                .performance
                .target_fps
                .min(if reduced { 30 } else { 60 }),
            secondary_details: !reduced,
        }
    }
    pub fn interval(self) -> Duration {
        Duration::from_secs_f64(1.0 / f64::from(self.target_fps))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_quality_preserves_the_profile_and_respects_the_requested_limit() {
        let mut settings = Settings::default();
        let original = settings.clone();
        let battery = RenderQuality::for_settings(&settings, true);
        assert_eq!(battery.target_fps, 30);
        assert!(!battery.secondary_details);
        assert_eq!(settings, original);
        let mains = RenderQuality::for_settings(&settings, false);
        assert_eq!(mains.target_fps, 60);
        assert!(mains.secondary_details);
        assert!(battery.interval() > mains.interval());
        settings.performance.target_fps = 30;
        assert_eq!(RenderQuality::for_settings(&settings, false).target_fps, 30);
        settings.performance.battery_saver = false;
        assert!(RenderQuality::for_settings(&settings, true).secondary_details);
        settings.performance.target_fps = 60;
        assert_eq!(RenderQuality::for_settings(&settings, true), mains);
    }
}
