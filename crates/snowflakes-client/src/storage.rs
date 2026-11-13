//! Per-user files. The caller owns single-instance coordination; no credentials
//! are stored in the weather profile or the cache envelope.
use crate::{
    config::{ConfigError, FILE_LIMIT, LocalConfig, Warning, WarningKind, normalize_endpoint},
    session::{CACHE_LIFETIME, Session, SessionError},
};
use serde::{Deserialize, Serialize};
use snowflakes_protocol::Configuration;
use std::{
    fmt,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tempfile::NamedTempFile;

#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    Config(ConfigError),
    InvalidCache,
    CacheExpired,
    ClockRollback,
    WrongEndpoint,
    Session(SessionError),
}
impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // io::ErrorKind is useful without exposing a private path or file contents.
        match self {
            Self::Io(e) => write!(f, "storage I/O: {:?}", e.kind()),
            _ => write!(
                f,
                "{}",
                match self {
                    Self::Config(_) => "invalid local configuration",
                    Self::InvalidCache => "invalid cache",
                    Self::CacheExpired => "expired cache",
                    Self::ClockRollback => "untrusted clock",
                    Self::WrongEndpoint => "cache belongs to a different endpoint",
                    Self::Session(_) => "invalid session state",
                    Self::Io(_) => unreachable!(),
                }
            ),
        }
    }
}
impl std::error::Error for StoreError {}
impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<ConfigError> for StoreError {
    fn from(e: ConfigError) -> Self {
        Self::Config(e)
    }
}
impl From<SessionError> for StoreError {
    fn from(e: SessionError) -> Self {
        Self::Session(e)
    }
}

#[derive(Debug)]
pub struct LoadedConfig {
    pub config: LocalConfig,
    pub warnings: Vec<Warning>,
    pub recovered_backup: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct UserStore {
    directory: PathBuf,
}

pub struct LoadedSession {
    pub session: Session,
    pub local: LoadedConfig,
    pub cache_error: Option<StoreError>,
}
impl UserStore {
    pub fn at(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }
    #[cfg(windows)]
    pub fn for_current_user() -> Result<Self, StoreError> {
        let base = std::env::var_os("LOCALAPPDATA")
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA is unavailable")
            })?;
        let base = PathBuf::from(base);
        if !base.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "LOCALAPPDATA must be absolute",
            )
            .into());
        }
        Ok(Self::at(base.join("Snowflakes")))
    }
    pub fn config_path(&self) -> PathBuf {
        self.directory.join("config.toml")
    }
    pub fn cache_path(&self) -> PathBuf {
        self.directory.join("server-cache.json")
    }

    pub fn open_session(
        &self,
        clock: CacheClock,
        now: Instant,
    ) -> Result<LoadedSession, StoreError> {
        let local = self.load_config()?;
        let mut session = Session::new(local.config.settings.clone())?;
        session.set_follow_server(local.config.network.follow_server, now)?;
        let mut cache_error = None;
        if !local.config.network.server_url.is_empty() {
            match self.load_cache(&local.config.network.server_url, clock) {
                Ok(Some(cache)) => {
                    if let Err(error) = cache.apply(&mut session, now) {
                        cache_error = Some(error);
                    }
                }
                Ok(None) => (),
                Err(error) => cache_error = Some(error),
            }
        }
        Ok(LoadedSession {
            session,
            local,
            cache_error,
        })
    }

    pub fn load_config(&self) -> Result<LoadedConfig, StoreError> {
        let bytes = match read_limited(&self.config_path(), FILE_LIMIT) {
            Ok(bytes) => bytes,
            Err(StoreError::Io(e)) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(LoadedConfig {
                    config: LocalConfig::default(),
                    warnings: vec![],
                    recovered_backup: None,
                });
            }
            Err(error) => return Err(error),
        };
        match LocalConfig::parse(&bytes) {
            Ok((config, warnings)) => Ok(LoadedConfig {
                config,
                warnings,
                recovered_backup: None,
            }),
            // Never silently replace a file written by a newer program.
            Err(ConfigError::UnsupportedVersion) => Err(ConfigError::UnsupportedVersion.into()),
            Err(_) => {
                let backup = self.backup("config-corrupt-", ".toml", &bytes)?;
                let config = LocalConfig::default();
                self.save_config(&config)?;
                Ok(LoadedConfig {
                    config,
                    warnings: vec![Warning {
                        field: String::new(),
                        kind: WarningKind::Recovered,
                    }],
                    recovered_backup: Some(backup),
                })
            }
        }
    }

    pub fn save_config(&self, config: &LocalConfig) -> Result<(), StoreError> {
        let bytes = config.encode()?;
        atomic_write(&self.config_path(), &bytes)?;
        Ok(())
    }

    pub fn save_cache(&self, cache: &CachedProfile) -> Result<(), StoreError> {
        cache.validate()?;
        let bytes = serde_json::to_vec(cache).map_err(|_| StoreError::InvalidCache)?;
        if bytes.len() > FILE_LIMIT * 2 {
            return Err(StoreError::InvalidCache);
        }
        atomic_write(&self.cache_path(), &bytes)?;
        Ok(())
    }

    pub fn load_cache(
        &self,
        endpoint: &str,
        clock: CacheClock,
    ) -> Result<Option<RestoredProfile>, StoreError> {
        let bytes = match read_limited(&self.cache_path(), FILE_LIMIT * 2) {
            Ok(bytes) => bytes,
            Err(StoreError::Io(e)) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut cache: CachedProfile = match serde_json::from_slice(&bytes) {
            Ok(cache) => cache,
            Err(_) => {
                self.backup("cache-corrupt-", ".json", &bytes)?;
                return Err(StoreError::InvalidCache);
            }
        };
        if let Err(error) = cache.validate() {
            self.backup("cache-corrupt-", ".json", &bytes)?;
            return Err(error);
        }
        if cache.endpoint != normalize_endpoint(endpoint)? {
            return Err(StoreError::WrongEndpoint);
        }
        let remaining = cache.remaining(clock)?;
        // A load is an observation, not a server validation. Keep the original
        // validation clocks and persist only a rollback high-water mark.
        cache.last_seen_unix_ms = clock.unix_ms;
        cache.last_seen_uptime_ms = clock.uptime_ms;
        self.save_cache(&cache)?;
        Ok(Some(RestoredProfile {
            configuration: cache.configuration,
            remaining,
        }))
    }

    fn backup(&self, prefix: &str, suffix: &str, bytes: &[u8]) -> Result<PathBuf, StoreError> {
        fs::create_dir_all(&self.directory)?;
        let mut file = tempfile::Builder::new()
            .prefix(prefix)
            .suffix(suffix)
            .tempfile_in(&self.directory)?;
        file.write_all(bytes)?;
        file.as_file().sync_all()?;
        let (_, path) = file.keep().map_err(|e| StoreError::Io(e.error))?;
        Ok(path)
    }
}

/// unix_ms is wall time, uptime_ms is GetTickCount64 on Windows (including sleep).
/// A platform adapter supplies both; deterministic tests use synthetic clocks.
#[derive(Debug, Clone, Copy)]
pub struct CacheClock {
    pub unix_ms: u64,
    pub uptime_ms: u64,
}

#[cfg(windows)]
impl CacheClock {
    pub fn current() -> Result<Self, StoreError> {
        let wall = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| StoreError::ClockRollback)?;
        let unix_ms = u64::try_from(wall.as_millis()).map_err(|_| StoreError::ClockRollback)?;
        // SAFETY: read-only API with no pointers; includes sleep and hibernation.
        let uptime_ms = unsafe { windows::Win32::System::SystemInformation::GetTickCount64() };
        Ok(Self { unix_ms, uptime_ms })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CachedProfile {
    cache_version: u32,
    endpoint: String,
    configuration: Configuration,
    validated_unix_ms: u64,
    validated_uptime_ms: u64,
    last_seen_unix_ms: u64,
    last_seen_uptime_ms: u64,
}

impl CachedProfile {
    pub fn verified(
        endpoint: &str,
        configuration: Configuration,
        clock: CacheClock,
    ) -> Result<Self, StoreError> {
        configuration
            .validate()
            .map_err(|_| StoreError::InvalidCache)?;
        let cache = Self {
            cache_version: 1,
            endpoint: normalize_endpoint(endpoint)?,
            configuration,
            validated_unix_ms: clock.unix_ms,
            validated_uptime_ms: clock.uptime_ms,
            last_seen_unix_ms: clock.unix_ms,
            last_seen_uptime_ms: clock.uptime_ms,
        };
        cache.validate()?;
        Ok(cache)
    }

    fn validate(&self) -> Result<(), StoreError> {
        if self.cache_version != 1
            || self.validated_unix_ms == 0
            || self.last_seen_unix_ms < self.validated_unix_ms
            || normalize_endpoint(&self.endpoint)? != self.endpoint
        {
            return Err(StoreError::InvalidCache);
        }
        self.configuration
            .validate()
            .map_err(|_| StoreError::InvalidCache)
    }

    pub fn remaining(&self, clock: CacheClock) -> Result<Duration, StoreError> {
        self.validate()?;
        if clock.unix_ms < self.last_seen_unix_ms {
            return Err(StoreError::ClockRollback);
        }
        if clock.unix_ms == self.last_seen_unix_ms && clock.uptime_ms <= self.last_seen_uptime_ms {
            return Err(StoreError::ClockRollback);
        }
        if clock.uptime_ms < self.last_seen_uptime_ms
            && clock.unix_ms - self.last_seen_unix_ms < clock.uptime_ms
        {
            return Err(StoreError::ClockRollback);
        }
        let wall_age = clock.unix_ms - self.validated_unix_ms;
        let uptime_age = clock.uptime_ms.checked_sub(self.validated_uptime_ms);
        // Wall time cannot plausibly advance substantially less than uptime
        // within a boot. On reboot, wall age includes the previous boot too.
        if uptime_age.is_some_and(|age| age > wall_age.saturating_add(60_000)) {
            return Err(StoreError::ClockRollback);
        }
        // If uptime restarted, age is supplied by the immutable wall deadline.
        let age = Duration::from_millis(wall_age.max(uptime_age.unwrap_or(0)));
        CACHE_LIFETIME
            .checked_sub(age)
            .filter(|left| !left.is_zero())
            .ok_or(StoreError::CacheExpired)
    }
}

#[derive(Debug)]
pub struct RestoredProfile {
    pub configuration: Configuration,
    pub remaining: Duration,
}
impl RestoredProfile {
    pub fn apply(self, session: &mut Session, now: Instant) -> Result<(), StoreError> {
        session.restore_cache(self.configuration, self.remaining, now)?;
        Ok(())
    }
}

fn read_limited(path: &Path, limit: usize) -> Result<Vec<u8>, StoreError> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(ConfigError::TooLarge.into());
    }
    Ok(bytes)
}

/// Same-directory temporary + flushed contents + atomic replacement. The old
/// file is never removed first. Failure before persist keeps it intact.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = NamedTempFile::with_prefix_in(".sf-write-", parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    // Windows can temporarily deny replacement while an otherwise compatible
    // reader or scanner closes its handle. Retry the same complete temp file;
    // never remove the destination or fall back to a non-atomic copy.
    for attempt in 0..=20 {
        match temporary.persist(path) {
            Ok(_) => break,
            Err(error)
                if cfg!(windows)
                    && attempt < 20
                    && error.error.kind() == io::ErrorKind::PermissionDenied =>
            {
                temporary = error.file;
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error.error),
        }
    }
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}
