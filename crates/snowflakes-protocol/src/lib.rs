//! Versioned, complete weather profiles shared with the PHP application.
//! Connection credentials and local preferences are deliberately separate.

use serde::{Deserialize, Serialize};
use std::fmt;

pub const SCHEMA_VERSION: u32 = 2;
pub const MAX_DOCUMENT_BYTES: usize = 65_536;
pub const MAX_REVISION: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError(String);

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for ValidationError {}

fn require(condition: bool, field: &str) -> Result<(), ValidationError> {
    if condition {
        Ok(())
    } else {
        Err(ValidationError(format!("invalid {field}")))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub schema_version: u32,
    pub generation_id: String,
    pub revision: u64,
    pub profile_id: String,
    pub global_enabled: bool,
    pub settings: Settings,
}

impl Configuration {
    pub fn from_json(bytes: &[u8]) -> Result<Self, ValidationError> {
        require(bytes.len() <= MAX_DOCUMENT_BYTES, "document size")?;
        let configuration: Self = serde_json::from_slice(bytes)
            .map_err(|error| ValidationError(format!("invalid JSON profile: {error}")))?;
        configuration.validate()?;
        Ok(configuration)
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        require(self.schema_version == SCHEMA_VERSION, "schema_version")?;
        require(is_uuid(&self.generation_id), "generation_id")?;
        require((1..=MAX_REVISION).contains(&self.revision), "revision")?;
        let id = self.profile_id.as_bytes();
        require(
            !id.is_empty()
                && id.len() <= 64
                && id[0].is_ascii_lowercase()
                && id
                    .iter()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-'),
            "profile_id",
        )?;
        self.settings.validate()
    }

    pub fn effects_enabled(&self) -> bool {
        self.global_enabled && self.settings.effects_enabled
    }
}

pub fn is_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
            }
        })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub effects_enabled: bool,
    pub weather: Weather,
    pub snow: Snow,
    pub rain: Rain,
    pub interaction: Interaction,
    pub performance: Performance,
    pub display: Display,
}

impl Settings {
    pub fn validate(&self) -> Result<(), ValidationError> {
        require(
            (10..=100).contains(&self.weather.opacity_percent),
            "weather.opacity_percent",
        )?;
        require(self.snow.intensity_percent <= 100, "snow.intensity_percent")?;
        require(
            (25..=200).contains(&self.snow.flake_size_percent),
            "snow.flake_size_percent",
        )?;
        require(
            self.snow.flake_visibility_percent <= 200,
            "snow.flake_visibility_percent",
        )?;
        require(
            self.snow.drift_size_percent <= 100,
            "snow.drift_size_percent",
        )?;
        require(
            (-100..=100).contains(&self.snow.wind_percent),
            "snow.wind_percent",
        )?;
        require(self.rain.intensity_percent <= 100, "rain.intensity_percent")?;
        require(
            self.rain.puddle_size_percent <= 100,
            "rain.puddle_size_percent",
        )?;
        require(
            (-100..=100).contains(&self.rain.wind_percent),
            "rain.wind_percent",
        )?;
        require(
            [30, 60].contains(&self.performance.target_fps),
            "performance.target_fps",
        )?;
        require(
            self.display.monitor_ids.len() <= 16,
            "display.monitor_ids length",
        )?;
        require(
            (self.display.selection == MonitorSelection::Selected)
                != self.display.monitor_ids.is_empty(),
            "display selection",
        )?;
        for (index, id) in self.display.monitor_ids.iter().enumerate() {
            require(
                !id.is_empty()
                    && id.chars().count() <= 256
                    && !id.chars().any(|c| c <= '\u{1f}')
                    && !self.display.monitor_ids[..index].contains(id),
                "display.monitor_ids entry",
            )?;
        }
        Ok(())
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            effects_enabled: true,
            weather: Weather {
                mode: WeatherMode::Snow,
                opacity_percent: 75,
            },
            snow: Snow {
                intensity_percent: 35,
                flake_size_percent: 100,
                flake_visibility_percent: 100,
                drift_size_percent: 35,
                wind_percent: 0,
                on_windows: true,
                on_ground: true,
                melting: true,
            },
            rain: Rain {
                intensity_percent: 40,
                puddle_size_percent: 35,
                wind_percent: 0,
                on_windows: true,
                on_ground: true,
                evaporation: true,
            },
            interaction: Interaction {
                cursor_enabled: true,
            },
            performance: Performance {
                target_fps: 60,
                battery_saver: true,
                pause_fullscreen: true,
                pause_remote_session: true,
            },
            display: Display {
                selection: MonitorSelection::All,
                monitor_ids: vec![],
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeatherMode {
    Snow,
    Rain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Weather {
    pub mode: WeatherMode,
    pub opacity_percent: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snow {
    pub intensity_percent: u8,
    pub flake_size_percent: u8,
    pub flake_visibility_percent: u8,
    pub drift_size_percent: u8,
    pub wind_percent: i16,
    pub on_windows: bool,
    pub on_ground: bool,
    pub melting: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rain {
    pub intensity_percent: u8,
    pub puddle_size_percent: u8,
    pub wind_percent: i16,
    pub on_windows: bool,
    pub on_ground: bool,
    pub evaporation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Interaction {
    pub cursor_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Performance {
    pub target_fps: u8,
    pub battery_saver: bool,
    pub pause_fullscreen: bool,
    pub pause_remote_session: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorSelection {
    All,
    Primary,
    Selected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Display {
    pub selection: MonitorSelection,
    pub monitor_ids: Vec<String>,
}
