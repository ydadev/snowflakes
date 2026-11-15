use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Only this generated top-level HKCU subtree is ever written/deleted by tests.
pub(in crate::platform) struct Fixture {
    base: String,
}
impl Fixture {
    pub fn absent() -> Self {
        static IDS: AtomicUsize = AtomicUsize::new(0);
        Self {
            base: format!(
                "SnowflakesPolicyTests-{}-{}",
                std::process::id(),
                IDS.fetch_add(1, Ordering::Relaxed)
            ),
        }
    }
    pub fn new() -> Self {
        let fixture = Self::absent();
        drop(fixture.create(&fixture.base));
        fixture
    }
    pub fn source(&self) -> Source {
        Source {
            path: format!("{}\\Parent\\Snowflakes", self.base),
            user: true,
        }
    }
    fn create(&self, path: &str) -> Key {
        assert!(path == self.base || path.starts_with(&format!("{}\\", self.base)));
        let name = wide(path);
        let mut key = HKEY::default();
        let result = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(name.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_VOLATILE,
                KEY_ALL_ACCESS | KEY_WOW64_64KEY,
                None,
                &mut key,
                None,
            )
        };
        assert_eq!(result, ERROR_SUCCESS);
        Key(key)
    }
    pub fn set(&self, name: &str, value: u32) {
        self.raw(name, REG_DWORD, &value.to_le_bytes());
    }
    pub fn raw(&self, name: &str, kind: REG_VALUE_TYPE, value: &[u8]) {
        let key = self.create(&self.source().path);
        let name = wide(name);
        assert_eq!(
            unsafe { RegSetValueExW(key.0, PCWSTR(name.as_ptr()), None, kind, Some(value)) },
            ERROR_SUCCESS
        );
    }
    pub fn remove_value(&self, name: &str) {
        let key = self.create(&self.source().path);
        let name = wide(name);
        assert_eq!(
            unsafe { RegDeleteValueW(key.0, PCWSTR(name.as_ptr())) },
            ERROR_SUCCESS
        );
    }
    pub fn remove_parent(&self) {
        let parent = wide(&format!("{}\\Parent", self.base));
        assert_eq!(
            unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(parent.as_ptr())) },
            ERROR_SUCCESS
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(self.base.starts_with("SnowflakesPolicyTests-") && !self.base.contains('\\'));
        let name = wide(&self.base);
        let result = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(name.as_ptr())) };
        assert!([ERROR_SUCCESS, ERROR_FILE_NOT_FOUND].contains(&result));
    }
}

fn next(monitor: &mut Monitor, clock: &mut Instant) -> Limits {
    *clock += Duration::from_millis(200);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(result) = monitor.refresh(*clock) {
            return result.unwrap();
        }
        assert!(
            Instant::now() < deadline,
            "registry event did not arrive before the backup deadline"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn policy_projection_keeps_types_clamps_and_removal_semantics() {
    let fixture = Fixture::new();
    let source = fixture.source();
    assert_eq!(source.read().unwrap(), Limits::default());
    for (name, value) in [
        ("Enabled", 0),
        ("AllowedModes", 2),
        ("MaxIntensityPercent", 900),
        ("MaxDriftSizePercent", 17),
        ("MaxPuddleSizePercent", 39),
        ("MaxFps", 59),
        ("PauseOnFullscreen", 1),
        ("AllowRemoteSession", 0),
    ] {
        fixture.set(name, value);
    }
    assert_eq!(
        source.read().unwrap(),
        Limits {
            enabled: false,
            allow_snow: false,
            allow_rain: true,
            max_intensity_percent: 100,
            max_drift_size_percent: 17,
            max_puddle_size_percent: 39,
            max_fps: 30,
            pause_on_fullscreen: true,
            allow_remote_session: false,
        }
    );
    fixture.raw("Enabled", REG_SZ, &[0, 0]);
    assert!(source.read().is_err());
    fixture.remove_value("Enabled");
    assert!(source.read().unwrap().enabled);
}

#[test]
fn absent_parent_creation_deletion_and_replacement_rebind_notifications() {
    let fixture = Fixture::new();
    let mut clock = Instant::now();
    let mut monitor = Monitor::new(fixture.source(), clock);
    assert_eq!(monitor.refresh(clock).unwrap().unwrap(), Limits::default());
    assert_eq!(monitor.watch.as_ref().unwrap().keys.len(), 1);
    fixture.set("MaxIntensityPercent", 21);
    assert_eq!(next(&mut monitor, &mut clock).max_intensity_percent, 21);
    assert_eq!(monitor.watch.as_ref().unwrap().keys.len(), 3);
    fixture.remove_parent();
    assert_eq!(next(&mut monitor, &mut clock), Limits::default());
    assert_eq!(monitor.watch.as_ref().unwrap().keys.len(), 1);
    fixture.set("MaxDriftSizePercent", 11);
    assert_eq!(next(&mut monitor, &mut clock).max_drift_size_percent, 11);
}

#[test]
fn one_shot_changes_coalesce_and_replaced_waits_cannot_signal_the_new_event() {
    let fixture = Fixture::new();
    fixture.set("MaxIntensityPercent", 0);
    let mut clock = Instant::now();
    let mut monitor = Monitor::new(fixture.source(), clock);
    monitor.refresh(clock).unwrap().unwrap();
    for value in 1..=100 {
        fixture.set("MaxIntensityPercent", value);
        fixture.set("MaxDriftSizePercent", value / 2);
        assert!(monitor.refresh(clock).is_none()); // busy command traffic cannot bypass coalescing
        let limits = next(&mut monitor, &mut clock);
        assert_eq!(limits.max_intensity_percent, value as u8);
        assert_eq!(limits.max_drift_size_percent, (value / 2) as u8);
        assert_eq!(monitor.watch.as_ref().unwrap().keys.len(), 3);
        assert!(monitor.refresh(clock + COALESCE).is_none());
    }
}

#[test]
fn backup_deadline_works_without_a_watch_and_restores_notifications() {
    let fixture = Fixture::absent();
    let start = Instant::now();
    let mut monitor = Monitor::new(fixture.source(), start);
    assert_eq!(monitor.refresh(start).unwrap().unwrap(), Limits::default());
    assert!(monitor.watch.is_none());
    fixture.set("Enabled", 0);
    assert!(
        monitor
            .refresh(start + FALLBACK - Duration::from_millis(1))
            .is_none()
    );
    let mut clock = start + FALLBACK;
    assert!(!monitor.refresh(clock).unwrap().unwrap().enabled);
    assert_eq!(monitor.watch.as_ref().unwrap().keys.len(), 3);
    fixture.set("Enabled", 1);
    assert!(next(&mut monitor, &mut clock).enabled);
}

#[test]
fn denied_notify_right_keeps_queryable_policy_and_uses_the_backup() {
    use windows::Win32::Security::{Authorization::*, *};
    struct Descriptor(PSECURITY_DESCRIPTOR);
    impl Descriptor {
        fn from(text: &str) -> Self {
            let text = wide(text);
            let mut raw = PSECURITY_DESCRIPTOR::default();
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    PCWSTR(text.as_ptr()),
                    SDDL_REVISION_1,
                    &mut raw,
                    None,
                )
                .unwrap();
            }
            Self(raw)
        }
    }
    impl Drop for Descriptor {
        fn drop(&mut self) {
            unsafe {
                let _ = LocalFree(Some(HLOCAL(self.0.0)));
            }
        }
    }
    struct Restore {
        key: Key,
        descriptor: Descriptor,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            assert_eq!(
                unsafe {
                    RegSetKeySecurity(
                        self.key.0,
                        DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                        self.descriptor.0,
                    )
                },
                ERROR_SUCCESS
            );
        }
    }
    let fixture = Fixture::new();
    fixture.set("MaxIntensityPercent", 23);
    let source = fixture.source();
    let sid = super::super::native::Identity::current("policy-acl-test")
        .unwrap()
        .sid;
    let restore = Restore {
        key: fixture.create(&source.path),
        descriptor: Descriptor::from(&format!("D:P(A;;KA;;;{sid})")),
    };
    // This temporary leaf permits querying and restoring its DACL, but not KEY_NOTIFY.
    // The pre-opened writer remains valid and cleanup restores access even on panic.
    let query_only = Descriptor::from(&format!("D:P(A;;0x00060001;;;{sid})"));
    assert_eq!(
        unsafe {
            RegSetKeySecurity(
                restore.key.0,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                query_only.0,
            )
        },
        ERROR_SUCCESS
    );
    assert!(
        matches!(Watch::new(&source), Err(e) if e.code() == windows::core::HRESULT::from_win32(ERROR_ACCESS_DENIED.0))
    );
    let start = Instant::now();
    let mut monitor = Monitor::new(source, start);
    assert_eq!(
        monitor
            .refresh(start)
            .unwrap()
            .unwrap()
            .max_intensity_percent,
        23
    );
    assert!(monitor.watch.is_none());
    let name = wide("MaxIntensityPercent");
    assert_eq!(
        unsafe {
            RegSetValueExW(
                restore.key.0,
                PCWSTR(name.as_ptr()),
                None,
                REG_DWORD,
                Some(&31u32.to_le_bytes()),
            )
        },
        ERROR_SUCCESS
    );
    assert!(
        monitor
            .refresh(start + FALLBACK - Duration::from_millis(1))
            .is_none()
    );
    assert_eq!(
        monitor
            .refresh(start + FALLBACK)
            .unwrap()
            .unwrap()
            .max_intensity_percent,
        31
    );
}
