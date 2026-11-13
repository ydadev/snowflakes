//! The owning event loop serializes mutations. Network workers only return a ticket
//! and bytes; they cannot change local settings, policy, pause state or credentials.
use serde::{Deserialize, Serialize};
use snowflakes_protocol::{Configuration, Settings, ValidationError, WeatherMode};
use std::{
    collections::BTreeSet,
    fmt,
    time::{Duration, Instant},
};

pub const CACHE_LIFETIME: Duration = Duration::from_secs(86_400);
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    Manual,
    Fullscreen,
    Locked,
    Suspended,
    RemoteSession,
    GraphicsUnavailable,
    DisplaysOff,
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileSource {
    Local,
    Server,
    WaitingForServer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    User,
    Policy,
}

/// Capability token: callers can copy a returned ticket, but cannot construct one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestTicket(u64, u64);

static SESSION_IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    InvalidProfile(ValidationError),
    LocalEditingUnavailable,
    StaleResponse,
    RequestExpired,
    OlderRevision,
    ConflictingRevision,
    InvalidLimits,
    Closed,
    TicketExhausted,
    MonitorScopedCondition,
    RestoreUnavailable,
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for SessionError {}
impl From<ValidationError> for SessionError {
    fn from(error: ValidationError) -> Self {
        Self::InvalidProfile(error)
    }
}

/// A validated projection of machine policy. Registry I/O belongs to the platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    pub enabled: bool,
    pub allow_snow: bool,
    pub allow_rain: bool,
    pub max_intensity_percent: u8,
    pub max_drift_size_percent: u8,
    pub max_puddle_size_percent: u8,
    pub max_fps: u8,
    pub pause_on_fullscreen: bool,
    pub allow_remote_session: bool,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_snow: true,
            allow_rain: true,
            max_intensity_percent: 100,
            max_drift_size_percent: 100,
            max_puddle_size_percent: 100,
            max_fps: 60,
            pause_on_fullscreen: false,
            allow_remote_session: true,
        }
    }
}

impl Limits {
    fn validate(&self) -> Result<(), SessionError> {
        if self.max_intensity_percent > 100
            || self.max_drift_size_percent > 100
            || self.max_puddle_size_percent > 100
            || ![30, 60].contains(&self.max_fps)
        {
            return Err(SessionError::InvalidLimits);
        }
        Ok(())
    }

    fn constrain(&self, settings: &mut Settings) -> bool {
        let original = settings.clone();
        settings.snow.intensity_percent = settings
            .snow
            .intensity_percent
            .min(self.max_intensity_percent);
        settings.rain.intensity_percent = settings
            .rain
            .intensity_percent
            .min(self.max_intensity_percent);
        settings.snow.drift_size_percent = settings
            .snow
            .drift_size_percent
            .min(self.max_drift_size_percent);
        settings.rain.puddle_size_percent = settings
            .rain
            .puddle_size_percent
            .min(self.max_puddle_size_percent);
        settings.performance.target_fps = settings.performance.target_fps.min(self.max_fps);
        settings.performance.pause_fullscreen |= self.pause_on_fullscreen;
        settings.performance.pause_remote_session |= !self.allow_remote_session;
        settings.effects_enabled &= match settings.weather.mode {
            WeatherMode::Snow => self.allow_snow,
            WeatherMode::Rain => self.allow_rain,
        };
        *settings != original
    }
}

struct Cache {
    configuration: Configuration,
    loaded_at: Instant,
    expires_at: Instant,
    expiry_handled: bool,
}

impl Cache {
    fn valid(&self, now: Instant) -> bool {
        !self.expiry_handled && now >= self.loaded_at && now < self.expires_at
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub source: ProfileSource,
    pub settings: Option<Settings>,
    pub should_render: bool,
    pub effects_enabled: bool,
    pub pauses: BTreeSet<PauseReason>,
    pub limited: bool,
    pub exit: Option<ExitReason>,
    /// Change token consumed by the renderer to discard old particles/accumulation.
    pub clear_epoch: u64,
}

/// Acknowledges acceptance, even while paused; rendering is a separate fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedProfile {
    pub generation_id: String,
    pub revision: u64,
    pub limited: bool,
}

pub struct Session {
    local: Settings,
    follow_server: bool,
    cache: Option<Cache>,
    limits: Limits,
    conditions: BTreeSet<PauseReason>,
    inflight: Option<(RequestTicket, Instant)>,
    next_ticket: u64,
    session_id: u64,
    clear_epoch: u64,
    exit: Option<ExitReason>,
}

impl Session {
    pub fn new(local: Settings) -> Result<Self, SessionError> {
        local.validate()?;
        let session_id = SESSION_IDS
            .try_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |id| id.checked_add(1),
            )
            .map_err(|_| SessionError::TicketExhausted)?;
        Ok(Self {
            local,
            follow_server: true,
            cache: None,
            limits: Limits::default(),
            conditions: BTreeSet::new(),
            inflight: None,
            next_ticket: 0,
            session_id,
            clear_epoch: 0,
            exit: None,
        })
    }

    pub fn follows_server(&self) -> bool {
        self.follow_server
    }
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    pub fn reset_server(&mut self, now: Instant) -> Result<Option<RequestTicket>, SessionError> {
        self.ensure_open()?;
        let before = self.snapshot(now);
        let cancelled = self.inflight.take().map(|(ticket, _)| ticket);
        self.cache = None;
        self.invalidate_for_change(&before, now);
        Ok(cancelled)
    }

    /// Startup hydration preserves the remaining lifetime; it never behaves like
    /// a successful network check and cannot replace a running session's cache.
    pub fn restore_cache(
        &mut self,
        configuration: Configuration,
        remaining: Duration,
        now: Instant,
    ) -> Result<(), SessionError> {
        self.ensure_open()?;
        if self.cache.is_some()
            || self.inflight.is_some()
            || self.next_ticket != 0
            || remaining.is_zero()
            || remaining > CACHE_LIFETIME
        {
            return Err(SessionError::RestoreUnavailable);
        }
        configuration.validate()?;
        let before = self.snapshot(now);
        let expires_at = now
            .checked_add(remaining)
            .ok_or(SessionError::RestoreUnavailable)?;
        self.cache = Some(Cache {
            configuration,
            loaded_at: now,
            expires_at,
            expiry_handled: false,
        });
        self.invalidate_for_change(&before, now);
        Ok(())
    }
    pub fn local_settings(&self) -> &Settings {
        &self.local
    }
    pub fn server_configuration(&self) -> Option<&Configuration> {
        self.cache.as_ref().map(|cache| &cache.configuration)
    }

    pub fn set_follow_server(
        &mut self,
        follow: bool,
        now: Instant,
    ) -> Result<Option<RequestTicket>, SessionError> {
        self.ensure_open()?;
        if self.follow_server == follow {
            return Ok(None);
        }
        let before = self.snapshot(now);
        self.follow_server = follow;
        let cancelled = self.inflight.take().map(|(ticket, _)| ticket);
        self.invalidate_for_change(&before, now);
        Ok(cancelled)
    }

    pub fn edit_local(&mut self, settings: Settings, now: Instant) -> Result<(), SessionError> {
        self.ensure_open()?;
        if self.follow_server {
            return Err(SessionError::LocalEditingUnavailable);
        }
        settings.validate()?;
        let before = self.snapshot(now);
        self.local = settings;
        self.invalidate_for_change(&before, now);
        Ok(())
    }

    pub fn set_limits(
        &mut self,
        limits: Limits,
        now: Instant,
    ) -> Result<Option<RequestTicket>, SessionError> {
        self.ensure_open()?;
        limits.validate()?;
        let before = self.snapshot(now);
        let forbidden = !limits.enabled;
        self.limits = limits;
        if forbidden {
            return Ok(self.close(ExitReason::Policy));
        }
        self.invalidate_for_change(&before, now);
        Ok(None)
    }

    pub fn set_condition(&mut self, reason: PauseReason, active: bool) -> Result<(), SessionError> {
        // Fullscreen is a per-monitor fact, never a session-wide pause.
        if reason == PauseReason::Fullscreen {
            return Err(SessionError::MonitorScopedCondition);
        }
        if active {
            self.conditions.insert(reason);
        } else {
            self.conditions.remove(&reason);
        }
        Ok(())
    }

    pub fn clear(&mut self) {
        // This is an opaque change token, not an ordered persistent revision.
        self.clear_epoch = self.clear_epoch.wrapping_add(1);
    }

    pub fn close(&mut self, reason: ExitReason) -> Option<RequestTicket> {
        if self.exit.is_none() {
            self.exit = Some(reason);
            self.clear();
        }
        self.inflight.take().map(|(ticket, _)| ticket)
    }

    /// Polling eligibility does not depend on effects being enabled or paused.
    pub fn begin_sync(&mut self, now: Instant) -> Result<Option<RequestTicket>, SessionError> {
        if self.exit.is_some() || !self.follow_server || self.inflight.is_some() {
            return Ok(None);
        }
        self.next_ticket = self
            .next_ticket
            .checked_add(1)
            .ok_or(SessionError::TicketExhausted)?;
        let ticket = RequestTicket(self.session_id, self.next_ticket);
        self.inflight = Some((ticket, now));
        Ok(Some(ticket))
    }

    /// Called by the event loop even when rendering is stopped. The returned
    /// ticket tells the network worker to cancel its timed-out operation.
    pub fn tick(&mut self, now: Instant) -> Option<RequestTicket> {
        if let Some(cache) = &mut self.cache
            && !cache.expiry_handled
            && !cache.valid(now)
        {
            cache.expiry_handled = true;
            if self.follow_server {
                self.clear();
            }
        }
        if self.inflight.is_some_and(|(_, started)| {
            now.checked_duration_since(started)
                .is_none_or(|age| age >= REQUEST_TIMEOUT)
        }) {
            return self.inflight.take().map(|(ticket, _)| ticket);
        }
        None
    }

    /// On errors the valid cache is retained without renewing its lifetime.
    pub fn sync_failed(&mut self, ticket: RequestTicket) -> bool {
        if self.inflight.is_some_and(|(current, _)| current == ticket) {
            self.inflight = None;
            true
        } else {
            false
        }
    }

    pub fn receive(
        &mut self,
        ticket: RequestTicket,
        bytes: &[u8],
        now: Instant,
    ) -> Result<AcceptedProfile, SessionError> {
        if !self.follow_server || self.exit.is_some() {
            return Err(SessionError::StaleResponse);
        }
        let Some((current, started)) = self.inflight else {
            return Err(SessionError::StaleResponse);
        };
        if current != ticket {
            return Err(SessionError::StaleResponse);
        }
        self.inflight = None;
        if now
            .checked_duration_since(started)
            .is_none_or(|age| age >= REQUEST_TIMEOUT)
        {
            return Err(SessionError::RequestExpired);
        }
        let configuration = Configuration::from_json(bytes)?;
        if let Some(cache) = &self.cache
            && cache.configuration.generation_id == configuration.generation_id
        {
            if configuration.revision < cache.configuration.revision {
                return Err(SessionError::OlderRevision);
            }
            if configuration.revision == cache.configuration.revision
                && configuration != cache.configuration
            {
                return Err(SessionError::ConflictingRevision);
            }
        }
        let before = self.snapshot(now);
        let generation_id = configuration.generation_id.clone();
        let revision = configuration.revision;
        // Assignment only happens after complete validation and revision checks.
        self.cache = Some(Cache {
            configuration,
            loaded_at: now,
            expires_at: now + CACHE_LIFETIME,
            expiry_handled: false,
        });
        self.invalidate_for_change(&before, now);
        Ok(AcceptedProfile {
            generation_id,
            revision,
            limited: self.snapshot(now).limited,
        })
    }

    pub fn snapshot(&self, now: Instant) -> Snapshot {
        let (source, selected, global_enabled) = if !self.follow_server {
            (ProfileSource::Local, Some(&self.local), true)
        } else if let Some(cache) = self.cache.as_ref().filter(|cache| cache.valid(now)) {
            (
                ProfileSource::Server,
                Some(&cache.configuration.settings),
                cache.configuration.global_enabled,
            )
        } else {
            (ProfileSource::WaitingForServer, None, false)
        };
        let mut settings = selected.cloned();
        let limited = settings
            .as_mut()
            .is_some_and(|settings| self.limits.constrain(settings));
        let mut pauses = self.conditions.clone();
        if let Some(settings) = &settings {
            if !settings.performance.pause_fullscreen {
                pauses.remove(&PauseReason::Fullscreen);
            }
            if !settings.performance.pause_remote_session {
                pauses.remove(&PauseReason::RemoteSession);
            }
        }
        let effects_enabled = self.exit.is_none()
            && self.limits.enabled
            && global_enabled
            && settings
                .as_ref()
                .is_some_and(|settings| settings.effects_enabled);
        let should_render = effects_enabled && pauses.is_empty();
        Snapshot {
            source,
            settings,
            should_render,
            effects_enabled,
            pauses,
            limited,
            exit: self.exit,
            clear_epoch: self.clear_epoch,
        }
    }

    /// The window tracker supplies the fullscreen fact for this monitor only.
    pub fn snapshot_for_monitor(&self, now: Instant, fullscreen: bool) -> Snapshot {
        let mut snapshot = self.snapshot(now);
        if fullscreen
            && snapshot
                .settings
                .as_ref()
                .is_some_and(|s| s.performance.pause_fullscreen)
        {
            snapshot.pauses.insert(PauseReason::Fullscreen);
            snapshot.should_render = false;
        }
        snapshot
    }

    fn ensure_open(&self) -> Result<(), SessionError> {
        if self.exit.is_some() {
            Err(SessionError::Closed)
        } else {
            Ok(())
        }
    }

    fn invalidate_for_change(&mut self, before: &Snapshot, now: Instant) {
        let after = self.snapshot(now);
        let weather_changed = before.settings.as_ref().map(|s| s.weather.mode)
            != after.settings.as_ref().map(|s| s.weather.mode);
        // Pauses freeze accumulation; disabling a profile must clear even while paused.
        if weather_changed || (before.effects_enabled && !after.effects_enabled) {
            self.clear();
        }
    }
}
