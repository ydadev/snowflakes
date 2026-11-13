//! Typed local commands. Persistence succeeds before a preference is applied.
use crate::{
    config::LocalConfig,
    session::{ExitReason, PauseReason, ProfileSource, Session},
    storage::{CacheClock, UserStore},
};
use serde::{Deserialize, Serialize};
use snowflakes_protocol::{Display, MonitorSelection, Settings, WeatherMode};
use std::time::Instant;

pub const MAX_IPC_BYTES: usize = 65_536;
pub const OK: i32 = 0;
pub const INTERNAL: i32 = 1;
pub const INVALID: i32 = 2;
pub const ABSENT: i32 = 3;
pub const POLICY: i32 = 4;
pub const NO_SESSION: i32 = 5;
pub const TIMEOUT: i32 = 6;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "setting",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Setting {
    Mode(WeatherMode),
    SnowIntensity(u8),
    FlakeSize(u8),
    FlakeVisibility(u8),
    RainIntensity(u8),
    DriftSize(u8),
    PuddleSize(u8),
    Opacity(u8),
    SnowWind(i16),
    RainWind(i16),
    EffectsEnabled(bool),
    TargetFps(u8),
    BatterySaver(bool),
    SnowOnWindows(bool),
    SnowOnGround(bool),
    SnowMelting(bool),
    CursorEnabled(bool),
    Display(Display),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Status {},
    Pause {},
    Resume {},
    Clear {},
    Set { setting: Setting },
    FollowServer { enabled: bool },
    SyncNow {},
    Quit {},
    Settings {},
    ServerUrl { value: String },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub version: String,
    pub pid: u32,
    pub session_id: u32,
    pub follow_server: bool,
    pub source: ProfileSource,
    pub pauses: Vec<PauseReason>,
    pub desired_effects_enabled: bool,
    pub limited: bool,
    pub rendering: bool,
    pub renderer: String,
    pub particle_count: u32,
    pub monitor_count: u32,
    #[serde(default)]
    pub effective_target_fps: u8,
    #[serde(default)]
    pub reduced_detail: bool,
    pub clear_epoch: u64,
    pub settings: Option<Settings>,
    pub configuration_warnings: usize,
    pub cache_state: String,
    pub network_state: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub code: i32,
    pub message: String,
    pub status: Option<Status>,
}
impl Response {
    pub fn error(code: i32, message: &str) -> Self {
        Self {
            code,
            message: message.into(),
            status: None,
        }
    }
}

pub struct Controller {
    pub session: Session,
    config: LocalConfig,
    store: UserStore,
    session_id: u32,
    configuration_warnings: usize,
    cache_state: String,
    settings_requested: bool,
}
impl Controller {
    pub fn open(
        store: UserStore,
        session_id: u32,
        clock: CacheClock,
        now: Instant,
    ) -> Result<Self, crate::storage::StoreError> {
        let loaded = store.open_session(clock, now)?;
        let configuration_warnings = loaded.local.warnings.len();
        let cache_state = if loaded.cache_error.is_some() {
            "rejected"
        } else if loaded.session.server_configuration().is_some() {
            "loaded"
        } else {
            "empty"
        }
        .into();
        Ok(Self {
            session: loaded.session,
            config: loaded.local.config,
            store,
            session_id,
            configuration_warnings,
            cache_state,
            settings_requested: false,
        })
    }
    pub fn config(&self) -> &LocalConfig {
        &self.config
    }
    pub fn ui_view(&self, now: Instant) -> crate::ui::View {
        crate::ui::View {
            status: self.status(now),
            server_url: self.config.network.server_url.clone(),
            limits: self.session.limits().clone(),
            monitors: None,
        }
    }
    pub fn take_settings_request(&mut self) -> bool {
        std::mem::take(&mut self.settings_requested)
    }
    pub fn status(&self, now: Instant) -> Status {
        let snapshot = self.session.snapshot(now);
        Status {
            version: env!("CARGO_PKG_VERSION").into(),
            pid: std::process::id(),
            session_id: self.session_id,
            follow_server: self.session.follows_server(),
            source: snapshot.source,
            pauses: snapshot.pauses.into_iter().collect(),
            desired_effects_enabled: snapshot.effects_enabled,
            limited: snapshot.limited,
            rendering: false,
            renderer: "not_connected".into(),
            particle_count: 0,
            monitor_count: 0,
            effective_target_fps: 0,
            reduced_detail: false,
            clear_epoch: snapshot.clear_epoch,
            settings: snapshot.settings,
            configuration_warnings: self.configuration_warnings,
            cache_state: self.cache_state.clone(),
            network_state: "not_connected".into(),
        }
    }
    pub fn handle(&mut self, command: Command, now: Instant) -> Response {
        if self.session.snapshot(now).exit.is_some() {
            return Response::error(ABSENT, "instance_closing");
        }
        match command {
            Command::Status {} => (),
            Command::Settings {} => {
                self.settings_requested = true;
                return Response {
                    code: OK,
                    message: "settings_requested".into(),
                    status: Some(self.status(now)),
                };
            }
            Command::ServerUrl { value } => {
                let value = value.trim();
                let endpoint = if value.is_empty() {
                    String::new()
                } else {
                    match crate::config::normalize_endpoint(value) {
                        Ok(value) => value,
                        Err(_) => return Response::error(INVALID, "invalid_server_url"),
                    }
                };
                if endpoint != self.config.network.server_url {
                    let mut config = self.config.clone();
                    config.network.server_url = endpoint;
                    if self.store.save_config(&config).is_err() {
                        return Response::error(INTERNAL, "preferences_not_saved");
                    }
                    if self.session.reset_server(now).is_err() {
                        return Response::error(INTERNAL, "source_change_failed");
                    }
                    self.config = config;
                    self.cache_state = "empty".into();
                }
            }
            Command::Pause {} => {
                let _ = self.session.set_condition(PauseReason::Manual, true);
            }
            Command::Resume {} => {
                let _ = self.session.set_condition(PauseReason::Manual, false);
            }
            Command::Clear {} => self.session.clear(),
            Command::Quit {} => {
                self.session.close(ExitReason::User);
            }
            Command::SyncNow {} => return Response::error(INTERNAL, "network_not_connected"),
            Command::FollowServer { enabled } => {
                let mut config = self.config.clone();
                config.network.follow_server = enabled;
                if self.store.save_config(&config).is_err() {
                    return Response::error(INTERNAL, "preferences_not_saved");
                }
                if self.session.set_follow_server(enabled, now).is_err() {
                    return Response::error(INTERNAL, "source_change_failed");
                }
                self.config = config;
            }
            Command::Set { setting } => {
                if self.session.follows_server() {
                    return Response::error(INVALID, "local_profile_read_only");
                }
                let mut config = self.config.clone();
                match setting {
                    Setting::Mode(v) => config.settings.weather.mode = v,
                    Setting::SnowIntensity(v) => config.settings.snow.intensity_percent = v,
                    Setting::FlakeSize(v) => config.settings.snow.flake_size_percent = v,
                    Setting::FlakeVisibility(v) => {
                        config.settings.snow.flake_visibility_percent = v
                    }
                    Setting::RainIntensity(v) => config.settings.rain.intensity_percent = v,
                    Setting::DriftSize(v) => config.settings.snow.drift_size_percent = v,
                    Setting::PuddleSize(v) => config.settings.rain.puddle_size_percent = v,
                    Setting::Opacity(v) => config.settings.weather.opacity_percent = v,
                    Setting::SnowWind(v) => config.settings.snow.wind_percent = v,
                    Setting::RainWind(v) => config.settings.rain.wind_percent = v,
                    Setting::EffectsEnabled(v) => config.settings.effects_enabled = v,
                    Setting::TargetFps(v) => config.settings.performance.target_fps = v,
                    Setting::BatterySaver(v) => config.settings.performance.battery_saver = v,
                    Setting::SnowOnWindows(v) => config.settings.snow.on_windows = v,
                    Setting::SnowOnGround(v) => config.settings.snow.on_ground = v,
                    Setting::SnowMelting(v) => config.settings.snow.melting = v,
                    Setting::CursorEnabled(v) => config.settings.interaction.cursor_enabled = v,
                    Setting::Display(v) => config.settings.display = v,
                }
                if config.settings.validate().is_err() {
                    return Response::error(INVALID, "invalid_setting");
                }
                if self.store.save_config(&config).is_err() {
                    return Response::error(INTERNAL, "preferences_not_saved");
                }
                if self
                    .session
                    .edit_local(config.settings.clone(), now)
                    .is_err()
                {
                    return Response::error(INTERNAL, "local_change_failed");
                }
                self.config = config;
            }
        }
        Response {
            code: OK,
            message: "ok".into(),
            status: Some(self.status(now)),
        }
    }
}

pub fn parse_command(args: &[String]) -> Result<Command, &'static str> {
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--status", "--json"] | ["--status"] => Ok(Command::Status {}),
        ["--pause"] => Ok(Command::Pause {}),
        ["--resume"] => Ok(Command::Resume {}),
        ["--clear"] => Ok(Command::Clear {}),
        ["--sync-now"] => Ok(Command::SyncNow {}),
        ["--quit"] => Ok(Command::Quit {}),
        ["--settings"] => Ok(Command::Settings {}),
        ["--server-url", value] => Ok(Command::ServerUrl {
            value: (*value).to_owned(),
        }),
        ["--follow-server", "on"] => Ok(Command::FollowServer { enabled: true }),
        ["--follow-server", "off"] => Ok(Command::FollowServer { enabled: false }),
        ["--monitors", mode @ ("all" | "primary")] => Ok(Command::Set {
            setting: Setting::Display(Display {
                selection: if *mode == "all" {
                    MonitorSelection::All
                } else {
                    MonitorSelection::Primary
                },
                monitor_ids: Vec::new(),
            }),
        }),
        ["--monitors", "selected", ids @ ..] if !ids.is_empty() && ids.len() <= 16 => {
            Ok(Command::Set {
                setting: Setting::Display(Display {
                    selection: MonitorSelection::Selected,
                    monitor_ids: ids.iter().map(|id| (*id).to_owned()).collect(),
                }),
            })
        }
        ["--set", entry] => {
            let (key, value) = entry.split_once('=').ok_or("expected_key_value")?;
            let setting = match key {
                "mode" => Setting::Mode(match value {
                    "snow" => WeatherMode::Snow,
                    "rain" => WeatherMode::Rain,
                    _ => return Err("invalid_mode"),
                }),
                "snow_intensity" => {
                    Setting::SnowIntensity(value.parse().map_err(|_| "invalid_percentage")?)
                }
                "rain_intensity" => {
                    Setting::RainIntensity(value.parse().map_err(|_| "invalid_percentage")?)
                }
                "flake_size" => {
                    Setting::FlakeSize(value.parse().map_err(|_| "invalid_percentage")?)
                }
                "flake_visibility" => {
                    Setting::FlakeVisibility(value.parse().map_err(|_| "invalid_percentage")?)
                }
                "drift_size" => {
                    Setting::DriftSize(value.parse().map_err(|_| "invalid_percentage")?)
                }
                "puddle_size" => {
                    Setting::PuddleSize(value.parse().map_err(|_| "invalid_percentage")?)
                }
                "opacity" => Setting::Opacity(value.parse().map_err(|_| "invalid_percentage")?),
                "snow_wind" => Setting::SnowWind(value.parse().map_err(|_| "invalid_wind")?),
                "rain_wind" => Setting::RainWind(value.parse().map_err(|_| "invalid_wind")?),
                "snow_on_windows" | "snow_on_ground" | "snow_melting" | "cursor_enabled" => {
                    let enabled = match value {
                        "true" => true,
                        "false" => false,
                        _ => return Err("invalid_boolean"),
                    };
                    match key {
                        "snow_on_windows" => Setting::SnowOnWindows(enabled),
                        "snow_on_ground" => Setting::SnowOnGround(enabled),
                        "snow_melting" => Setting::SnowMelting(enabled),
                        _ => Setting::CursorEnabled(enabled),
                    }
                }
                "target_fps" => Setting::TargetFps(match value {
                    "30" => 30,
                    "60" => 60,
                    _ => return Err("invalid_fps"),
                }),
                "battery_saver" => Setting::BatterySaver(match value {
                    "true" => true,
                    "false" => false,
                    _ => return Err("invalid_boolean"),
                }),
                "effects_enabled" => Setting::EffectsEnabled(match value {
                    "true" => true,
                    "false" => false,
                    _ => return Err("invalid_boolean"),
                }),
                _ => return Err("unknown_setting"),
            };
            Ok(Command::Set { setting })
        }
        _ => Err("invalid_command"),
    }
}
