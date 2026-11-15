//! Read-only machine policy and one-shot registry notifications, owned by the control worker.
use super::native::{Handle, failure, wide};
use crate::session::Limits;
use std::time::{Duration, Instant};
use windows::{
    Win32::{
        Foundation::*,
        System::{Registry::*, Threading::*},
    },
    core::{Error, PCWSTR, Result},
};

const FALLBACK: Duration = Duration::from_secs(60);
const COALESCE: Duration = Duration::from_millis(100);

#[derive(Clone)]
pub(super) struct Source {
    path: String,
    // Test selection is compiled out of release; no CLI or environment override exists.
    #[cfg(test)]
    user: bool,
}
impl Source {
    pub fn machine() -> Self {
        Self {
            path: "Software\\Policies\\Snowflakes".into(),
            #[cfg(test)]
            user: false,
        }
    }
    fn root(&self) -> HKEY {
        #[cfg(test)]
        if self.user {
            return HKEY_CURRENT_USER;
        }
        HKEY_LOCAL_MACHINE
    }
    pub fn value(&self, name: &str) -> Result<Option<u32>> {
        let path = wide(&self.path);
        let name = wide(name);
        let mut value = 0u32;
        let mut size = 4;
        let result = unsafe {
            RegGetValueW(
                self.root(),
                PCWSTR(path.as_ptr()),
                PCWSTR(name.as_ptr()),
                RRF_RT_REG_DWORD | RRF_SUBKEY_WOW6464KEY,
                None,
                Some((&mut value as *mut u32).cast()),
                Some(&mut size),
            )
        };
        match result {
            ERROR_SUCCESS => Ok(Some(value)),
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Ok(None),
            other => Err(failure(other)),
        }
    }
    pub fn read(&self) -> Result<Limits> {
        let mut limits = Limits::default();
        if let Some(v) = self.value("Enabled")? {
            limits.enabled = v != 0;
        }
        if let Some(v) = self.value("AllowedModes")? {
            limits.allow_snow = v & 1 != 0;
            limits.allow_rain = v & 2 != 0;
        }
        if let Some(v) = self.value("MaxIntensityPercent")? {
            limits.max_intensity_percent = v.min(100) as u8;
        }
        if let Some(v) = self.value("MaxDriftSizePercent")? {
            limits.max_drift_size_percent = v.min(100) as u8;
        }
        if let Some(v) = self.value("MaxPuddleSizePercent")? {
            limits.max_puddle_size_percent = v.min(100) as u8;
        }
        if let Some(v) = self.value("MaxFps")? {
            limits.max_fps = if v < 60 { 30 } else { 60 };
        }
        if let Some(v) = self.value("PauseOnFullscreen")? {
            limits.pause_on_fullscreen = v != 0;
        }
        if let Some(v) = self.value("AllowRemoteSession")? {
            limits.allow_remote_session = v != 0;
        }
        Ok(limits)
    }
}

struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

struct Watch {
    // Closing registered keys cancels their waits and can signal the event. Keys must
    // be dropped before the event, including partial construction and replacement.
    keys: Vec<Key>,
    event: Handle,
}
impl Watch {
    fn new(source: &Source) -> Result<Self> {
        let mut watch = Self {
            keys: Vec::new(),
            event: Handle(unsafe { CreateEventW(None, true, false, PCWSTR::null())? }),
        };
        let mut path = String::new();
        let mut parts = source.path.split('\\').peekable();
        while let Some(part) = parts.next() {
            if !path.is_empty() {
                path.push('\\');
            }
            path.push_str(part);
            let name = wide(&path);
            let mut key = HKEY::default();
            let opened = unsafe {
                RegOpenKeyExW(
                    source.root(),
                    PCWSTR(name.as_ptr()),
                    None,
                    KEY_NOTIFY | KEY_WOW64_64KEY,
                    &mut key,
                )
            };
            match opened {
                ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => break,
                ERROR_SUCCESS => (),
                other => return Err(failure(other)),
            }
            let key = Key(key);
            // Ancestors see direct child replacement/security, never unrelated values.
            // The leaf sees only policy values/security. No recursive SOFTWARE watch.
            let changes = if parts.peek().is_some() {
                REG_NOTIFY_CHANGE_NAME
            } else {
                REG_NOTIFY_CHANGE_LAST_SET
            };
            let armed = unsafe {
                RegNotifyChangeKeyValue(
                    key.0,
                    false,
                    changes | REG_NOTIFY_CHANGE_SECURITY | REG_NOTIFY_THREAD_AGNOSTIC,
                    Some(watch.event.0),
                    true,
                )
            };
            if armed != ERROR_SUCCESS {
                return Err(failure(armed));
            }
            watch.keys.push(key);
        }
        if watch.keys.is_empty() {
            return Err(failure(ERROR_PATH_NOT_FOUND));
        }
        Ok(watch)
    }
    fn signalled(&self) -> Result<bool> {
        match unsafe { WaitForSingleObject(self.event.0, 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(Error::from_thread()),
        }
    }
}

pub(super) struct Monitor {
    source: Source,
    watch: Option<Watch>,
    next: Instant,
    earliest: Instant,
}
impl Monitor {
    pub fn new(source: Source, now: Instant) -> Self {
        // First refresh rebuilds/arms the watch before the first authoritative read.
        Self {
            source,
            watch: None,
            next: now,
            earliest: now,
        }
    }
    pub fn refresh(&mut self, now: Instant) -> Option<Result<Limits>> {
        if now < self.earliest {
            return None;
        }
        let signalled = self
            .watch
            .as_ref()
            .is_some_and(|w| w.signalled().unwrap_or(true));
        if now < self.next && !signalled {
            return None;
        }
        // New event and registrations are independent of the old pending waits.
        // Dropping the old keys cannot spuriously signal the new event. Never re-arm
        // an unfinished wait on the same key, which would leak registrations.
        self.watch = Watch::new(&self.source).ok();
        self.next = now + FALLBACK;
        self.earliest = now + COALESCE;
        Some(self.source.read())
    }
}

#[cfg(test)]
pub(super) mod tests;
