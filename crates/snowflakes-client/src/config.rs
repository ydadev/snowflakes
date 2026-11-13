//! Local TOML is forgiving; the network JSON protocol remains strict.
use serde::{Deserialize, Serialize};
use snowflakes_protocol::Settings;
use std::fmt;
use toml::Value;
use url::Url;

pub const CONFIG_VERSION: i64 = 2;
pub const FILE_LIMIT: usize = 65_536;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    TooLarge,
    InvalidSyntax,
    UnsupportedVersion,
    InvalidValue,
    Encoding,
}
impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ConfigError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WarningKind {
    Unknown,
    WrongType,
    Adjusted,
    InvalidValue,
    Recovered,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub field: String,
    pub kind: WarningKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Network {
    pub follow_server: bool,
    pub server_url: String,
    pub poll_interval_seconds: u32,
    pub max_cache_age_seconds: u32,
}
impl Default for Network {
    fn default() -> Self {
        Self {
            follow_server: true,
            server_url: String::new(),
            poll_interval_seconds: 600,
            max_cache_age_seconds: 86_400,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Startup {
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalConfig {
    pub schema_version: i64,
    pub network: Network,
    #[serde(flatten)]
    pub settings: Settings,
    pub startup: Startup,
}
impl Default for LocalConfig {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_VERSION,
            network: Network::default(),
            settings: Settings::default(),
            startup: Startup::default(),
        }
    }
}

/// Credentials, query strings and fragments have no place in a stored base URL.
pub fn normalize_endpoint(value: &str) -> Result<String, ConfigError> {
    if value.len() > 2048 || value.chars().any(char::is_control) {
        return Err(ConfigError::InvalidValue);
    }
    let mut url = Url::parse(value).map_err(|_| ConfigError::InvalidValue)?;
    if !["http", "https"].contains(&url.scheme())
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ConfigError::InvalidValue);
    }
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    Ok(url.into())
}

impl LocalConfig {
    pub fn parse(bytes: &[u8]) -> Result<(Self, Vec<Warning>), ConfigError> {
        if bytes.len() > FILE_LIMIT {
            return Err(ConfigError::TooLarge);
        }
        let text = std::str::from_utf8(bytes).map_err(|_| ConfigError::Encoding)?;
        let supplied: Value = toml::from_str(text).map_err(|_| ConfigError::InvalidSyntax)?;
        let version = supplied.get("schema_version");
        if let Some(version) = version {
            if version.as_integer().is_none() {
                return Err(ConfigError::InvalidValue);
            }
            if version.as_integer() != Some(CONFIG_VERSION) {
                return Err(ConfigError::UnsupportedVersion);
            }
        }
        let mut warnings = Vec::new();
        let mut merged = Value::try_from(Self::default()).map_err(|_| ConfigError::InvalidValue)?;
        merge(&mut merged, &supplied, "", &mut warnings);
        let mut local = merged.as_table().ok_or(ConfigError::InvalidValue)?.clone();
        let schema_version = local
            .remove("schema_version")
            .and_then(|v| v.as_integer())
            .ok_or(ConfigError::InvalidValue)?;
        let network: Value = local.remove("network").ok_or(ConfigError::InvalidValue)?;
        let startup: Startup = local
            .remove("startup")
            .ok_or(ConfigError::InvalidValue)?
            .try_into()
            .map_err(|_| ConfigError::InvalidValue)?;
        let mut network: Network = network.try_into().map_err(|_| ConfigError::InvalidValue)?;
        if !network.server_url.is_empty() {
            match normalize_endpoint(&network.server_url) {
                Ok(url) => network.server_url = url,
                Err(_) => {
                    network.server_url.clear();
                    warnings.push(Warning {
                        field: "network.server_url".into(),
                        kind: WarningKind::InvalidValue,
                    });
                }
            }
        }
        let settings: Settings = Value::Table(local)
            .try_into()
            .map_err(|_| ConfigError::InvalidValue)?;
        settings.validate().map_err(|_| ConfigError::InvalidValue)?;
        Ok((
            Self {
                schema_version,
                network,
                settings,
                startup,
            },
            warnings,
        ))
    }

    pub fn encode(&self) -> Result<Vec<u8>, ConfigError> {
        self.settings
            .validate()
            .map_err(|_| ConfigError::InvalidValue)?;
        if self.schema_version != CONFIG_VERSION
            || self.network.poll_interval_seconds != 600
            || self.network.max_cache_age_seconds != 86_400
        {
            return Err(ConfigError::InvalidValue);
        }
        if !self.network.server_url.is_empty() {
            normalize_endpoint(&self.network.server_url)?;
        }
        let mut canonical = self.clone();
        if !canonical.network.server_url.is_empty() {
            canonical.network.server_url = normalize_endpoint(&canonical.network.server_url)?;
        }
        let bytes = toml::to_string_pretty(&canonical)
            .map_err(|_| ConfigError::InvalidValue)?
            .into_bytes();
        if bytes.len() > FILE_LIMIT {
            return Err(ConfigError::TooLarge);
        }
        Ok(bytes)
    }
}

// Deserialize only after known fields have been normalized; protocol structs
// can keep deny_unknown_fields without compromising local forward compatibility.
fn merge(default: &mut Value, supplied: &Value, path: &str, warnings: &mut Vec<Warning>) {
    let warn = |kind| Warning {
        field: path.to_owned(),
        kind,
    };
    if let (Value::Table(expected), Value::Table(actual)) = (&mut *default, supplied) {
        for (key, value) in actual {
            let child = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            if let Some(target) = expected.get_mut(key) {
                merge(target, value, &child, warnings);
            } else {
                warnings.push(Warning {
                    field: child,
                    kind: WarningKind::Unknown,
                });
            }
        }
        return;
    }
    if std::mem::discriminant(default) != std::mem::discriminant(supplied) {
        warnings.push(warn(WarningKind::WrongType));
        return;
    }
    if let Value::Integer(value) = supplied {
        let range = match path {
            "snow.intensity_percent"
            | "rain.intensity_percent"
            | "snow.drift_size_percent"
            | "rain.puddle_size_percent" => Some((0, 100)),
            "snow.wind_percent" | "rain.wind_percent" => Some((-100, 100)),
            "weather.opacity_percent" => Some((10, 100)),
            "network.poll_interval_seconds" => Some((600, 600)),
            "network.max_cache_age_seconds" => Some((86_400, 86_400)),
            _ => None,
        };
        let adjusted = if path == "performance.target_fps" {
            if *value < 60 { 30 } else { 60 }
        } else if let Some((min, max)) = range {
            (*value).clamp(min, max)
        } else {
            *value
        };
        if adjusted != *value {
            warnings.push(warn(WarningKind::Adjusted));
        }
        *default = Value::Integer(adjusted);
        return;
    }
    if let Value::String(value) = supplied {
        let valid = match path {
            "weather.mode" => ["snow", "rain"].contains(&value.as_str()),
            "display.selection" => ["all", "primary", "selected"].contains(&value.as_str()),
            _ => true,
        };
        if !valid {
            warnings.push(warn(WarningKind::InvalidValue));
            return;
        }
    }
    *default = supplied.clone();
}
