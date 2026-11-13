//! Session-local WTS/power notifications. The hidden top-level HWND and its
//! registrations live on the graphics thread; consumers share only atomic facts.
use crate::session::{PauseReason, Session};
use snowflakes_protocol::Settings;
use std::{
    cell::Cell,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        System::{
            LibraryLoader::GetModuleHandleW,
            Power::*,
            RemoteDesktop::*,
            SystemServices::{
                GUID_ACDC_POWER_SOURCE, GUID_POWER_SAVING_STATUS, GUID_SESSION_DISPLAY_STATUS,
            },
        },
        UI::WindowsAndMessaging::*,
    },
    core::{PWSTR, Result, w},
};

const LOCKED: u32 = 1;
const SUSPENDED: u32 = 2;
const DISPLAYS_OFF: u32 = 4;
const DISCONNECTED: u32 = 8;
const REMOTE: u32 = 16;
const SESSION_BITS: u32 = LOCKED | DISCONNECTED | REMOTE;
const ON_BATTERY: u32 = 1;
const SAVING_POWER: u32 = 2;
const CLASS: windows::core::PCWSTR = w!("SnowflakesSystemEvents");

#[derive(Default)]
pub struct Conditions {
    bits: AtomicU32,
    ready: AtomicBool,
    revision: AtomicU64,
    power: AtomicU32,
}
impl Conditions {
    fn set(&self, bit: u32, active: bool) {
        let previous = if active {
            self.bits.fetch_or(bit, Ordering::AcqRel)
        } else {
            self.bits.fetch_and(!bit, Ordering::AcqRel)
        };
        if (previous & bit != 0) != active {
            self.revision.fetch_add(1, Ordering::Release);
        }
    }
    pub fn apply_to(&self, session: &mut Session) {
        let bits = self.bits.load(Ordering::Acquire);
        for (bit, reason) in [
            (LOCKED, PauseReason::Locked),
            (SUSPENDED, PauseReason::Suspended),
            (DISPLAYS_OFF, PauseReason::DisplaysOff),
            (DISCONNECTED, PauseReason::Disconnected),
            (REMOTE, PauseReason::RemoteSession),
        ] {
            let _ = session.set_condition(reason, bits & bit != 0);
        }
    }
    pub fn paused(&self, settings: Option<&Settings>) -> bool {
        let bits = self.bits.load(Ordering::Acquire);
        !self.ready.load(Ordering::Acquire)
            || bits & !REMOTE != 0
            || (bits & REMOTE != 0 && settings.is_none_or(|s| s.performance.pause_remote_session))
    }
    pub fn ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }
    pub fn low_power(&self) -> bool {
        self.power.load(Ordering::Acquire) != 0
    }
    fn set_power(&self, bit: u32, active: bool) {
        // Quality changes must not reset simulation time or clear a manual pause.
        if active {
            self.power.fetch_or(bit, Ordering::AcqRel);
        } else {
            self.power.fetch_and(!bit, Ordering::AcqRel);
        }
    }
    fn power_snapshot(&self, ac_line: u8, saver: u8) {
        // Unknown readings preserve the last known fact, initially normal quality.
        if ac_line <= 1 {
            self.set_power(ON_BATTERY, ac_line == 0);
        }
        if saver <= 1 {
            self.set_power(SAVING_POWER, saver == 1);
        }
    }
}

struct WindowState {
    conditions: Arc<Conditions>,
    session_id: u32,
    refresh: Cell<bool>,
    ending: Cell<bool>,
}
impl WindowState {
    fn session_event(&self, event: u32, session_id: u32) {
        if session_id != self.session_id {
            return;
        }
        match event {
            WTS_SESSION_LOCK => self.conditions.set(LOCKED, true),
            WTS_SESSION_UNLOCK => self.conditions.set(LOCKED, false),
            WTS_SESSION_LOGOFF => {
                self.ending.set(true);
                self.conditions.set(DISCONNECTED, true);
            }
            WTS_CONSOLE_DISCONNECT | WTS_REMOTE_DISCONNECT => {
                self.conditions.set(DISCONNECTED, true)
            }
            WTS_CONSOLE_CONNECT | WTS_REMOTE_CONNECT | WTS_SESSION_LOGON => {
                self.conditions.set(DISCONNECTED, false);
                if event != WTS_SESSION_LOGON {
                    self.conditions.set(REMOTE, event == WTS_REMOTE_CONNECT);
                }
                self.refresh.set(true);
            }
            _ => {}
        }
    }
    fn power_event(&self, event: u32) {
        match event {
            PBT_APMSUSPEND => self.conditions.set(SUSPENDED, true),
            PBT_APMPOWERSTATUSCHANGE => self.refresh.set(true),
            PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND | PBT_APMRESUMECRITICAL => {
                self.conditions.set(SUSPENDED, false);
                self.refresh.set(true);
            }
            _ => {}
        }
    }
    fn display_event(&self, value: u32) {
        // Dimming still permits rendering. Unknown future values keep the last fact.
        if value <= 2 {
            self.conditions.set(DISPLAYS_OFF, value == 0);
        }
    }
    fn setting_event(&self, guid: windows::core::GUID, value: u32) {
        if guid == GUID_SESSION_DISPLAY_STATUS {
            self.display_event(value);
        } else if guid == GUID_ACDC_POWER_SOURCE && value <= 2 {
            // Notification values differ from SYSTEM_POWER_STATUS.ACLineStatus.
            // Both onboard batteries and short-term UPS power request economy.
            self.conditions.set_power(ON_BATTERY, value != 0);
        } else if guid == GUID_POWER_SAVING_STATUS && value <= 1 {
            self.conditions.set_power(SAVING_POWER, value == 1);
        }
    }
}

unsafe extern "system" fn proc(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(l.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const WindowState;
        if !ptr.is_null() {
            // Observer keeps the Box alive until after DestroyWindow returns.
            let state = &*ptr;
            match message {
                WM_WTSSESSION_CHANGE => {
                    state.session_event(w.0 as u32, l.0 as u32);
                    return LRESULT(0);
                }
                WM_POWERBROADCAST => {
                    if w.0 as u32 == PBT_POWERSETTINGCHANGE && l.0 != 0 {
                        // Windows owns this synchronous notification buffer. Data is
                        // a variable-length byte tail; validate its DWORD length.
                        let setting = &*(l.0 as *const POWERBROADCAST_SETTING);
                        if setting.DataLength == 4 {
                            state.setting_event(
                                setting.PowerSetting,
                                std::ptr::read_unaligned(setting.Data.as_ptr().cast::<u32>()),
                            );
                        }
                    } else {
                        state.power_event(w.0 as u32);
                    }
                    return LRESULT(1);
                }
                WM_QUERYENDSESSION => return LRESULT(1),
                WM_ENDSESSION if w.0 != 0 => {
                    state.ending.set(true);
                    state.conditions.set(DISCONNECTED, true);
                    return LRESULT(0);
                }
                WM_NCDESTROY => {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
                _ => {}
            }
        }
        DefWindowProcW(hwnd, message, w, l)
    }
}

struct WtsBuffer(PWSTR);
impl Drop for WtsBuffer {
    fn drop(&mut self) {
        unsafe {
            WTSFreeMemory(self.0.0.cast());
        }
    }
}
fn query<T: Copy>(session_id: u32, class: WTS_INFO_CLASS) -> Result<T> {
    let mut pointer = PWSTR::null();
    let mut bytes = 0;
    unsafe {
        WTSQuerySessionInformationW(None, session_id, class, &mut pointer, &mut bytes)?;
    }
    let buffer = WtsBuffer(pointer);
    if buffer.0.is_null() || (bytes as usize) < size_of::<T>() {
        return Err(windows::core::Error::from_hresult(
            ERROR_INVALID_DATA.to_hresult(),
        ));
    }
    // Native buffers have no Rust lifetime/alignment guarantee. Do not inspect
    // or retain account names contained in the extended WTS structure.
    Ok(unsafe { std::ptr::read_unaligned(buffer.0.0.cast::<T>()) })
}

fn session_bits(state: WTS_CONNECTSTATE_CLASS, flags: u32, protocol: u16) -> Result<u32> {
    if flags != WTS_SESSIONSTATE_LOCK && flags != WTS_SESSIONSTATE_UNLOCK {
        return Err(windows::core::Error::from_hresult(
            ERROR_INVALID_DATA.to_hresult(),
        ));
    }
    Ok(if flags == WTS_SESSIONSTATE_LOCK {
        LOCKED
    } else {
        0
    } | if state != WTSActive { DISCONNECTED } else { 0 }
        | if protocol != 0 { REMOTE } else { 0 })
}

pub struct Observer {
    hwnd: HWND,
    state: Box<WindowState>,
    power: Vec<HPOWERNOTIFY>,
    wts: bool,
    next_query: Instant,
}
impl Observer {
    pub fn new(session_id: u32, conditions: Arc<Conditions>) -> Result<Self> {
        let instance = unsafe { HINSTANCE(GetModuleHandleW(None)?.0) };
        let class = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance,
            lpszClassName: CLASS,
            ..Default::default()
        };
        if unsafe { RegisterClassW(&class) } == 0
            && unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS
        {
            return Err(windows::core::Error::from_thread());
        }
        let state = Box::new(WindowState {
            conditions,
            session_id,
            refresh: Cell::new(true),
            ending: Cell::new(false),
        });
        // A message-only HWND would miss broadcast power/end-session messages.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                CLASS,
                w!("Snowflakes"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance),
                Some((&*state as *const WindowState).cast()),
            )?
        };
        let mut observer = Self {
            hwnd,
            state,
            power: Vec::new(),
            wts: false,
            next_query: Instant::now(),
        };
        unsafe {
            WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION)?;
            observer.wts = true;
            observer.power.push(RegisterPowerSettingNotification(
                HANDLE(hwnd.0),
                &GUID_SESSION_DISPLAY_STATUS,
                DEVICE_NOTIFY_WINDOW_HANDLE,
            )?);
            for guid in [GUID_ACDC_POWER_SOURCE, GUID_POWER_SAVING_STATUS] {
                // A missing quality notification does not disable rendering;
                // the periodic numeric snapshot remains available as a fallback.
                if let Ok(handle) = RegisterPowerSettingNotification(
                    HANDLE(hwnd.0),
                    &guid,
                    DEVICE_NOTIFY_WINDOW_HANDLE,
                ) {
                    observer.power.push(handle);
                }
            }
        }
        Ok(observer)
    }
    pub fn poll(&mut self, now: Instant) {
        if now < self.next_query && !self.state.refresh.replace(false) {
            return;
        }
        self.state.refresh.set(false);
        self.next_query = now + Duration::from_secs(5);
        let mut power = SYSTEM_POWER_STATUS::default();
        if unsafe { GetSystemPowerStatus(&mut power) }.is_ok() {
            self.state
                .conditions
                .power_snapshot(power.ACLineStatus, power.SystemStatusFlag);
        }
        let result = (|| -> Result<u32> {
            let info: WTSINFOEXW = query(self.state.session_id, WTSSessionInfoEx)?;
            if info.Level != 1 {
                return Err(windows::core::Error::from_hresult(
                    ERROR_INVALID_DATA.to_hresult(),
                ));
            }
            let info = unsafe { info.Data.WTSInfoExLevel1 };
            if info.SessionId != self.state.session_id
                || info.SessionFlags as u32 == WTS_SESSIONSTATE_UNKNOWN
            {
                return Err(windows::core::Error::from_hresult(
                    ERROR_INVALID_DATA.to_hresult(),
                ));
            }
            let protocol: u16 = query(self.state.session_id, WTSClientProtocolType)?;
            session_bits(info.SessionState, info.SessionFlags as u32, protocol)
        })();
        match result {
            Ok(bits) => {
                let bits = bits
                    | if self.state.ending.get() {
                        DISCONNECTED
                    } else {
                        0
                    };
                let mut old = self.state.conditions.bits.load(Ordering::Acquire);
                loop {
                    match self.state.conditions.bits.compare_exchange_weak(
                        old,
                        (old & !SESSION_BITS) | bits,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    ) {
                        Ok(_) => {
                            if old != (old & !SESSION_BITS) | bits {
                                self.state
                                    .conditions
                                    .revision
                                    .fetch_add(1, Ordering::Release);
                            }
                            break;
                        }
                        Err(current) => old = current,
                    }
                }
                self.state.conditions.ready.store(true, Ordering::Release);
            }
            Err(_) => self.state.conditions.ready.store(false, Ordering::Release),
        }
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        self.state.conditions.ready.store(false, Ordering::Release);
        unsafe {
            for power in self.power.drain(..) {
                let _ = UnregisterPowerSettingNotification(power);
            }
            if self.wts {
                let _ = WTSUnRegisterSessionNotification(self.hwnd);
            }
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> WindowState {
        let conditions = Arc::new(Conditions::default());
        conditions.ready.store(true, Ordering::Release);
        WindowState {
            conditions,
            session_id: 42,
            refresh: Cell::new(false),
            ending: Cell::new(false),
        }
    }
    fn session() -> Session {
        let mut session = Session::new(Settings::default()).unwrap();
        session.set_follow_server(false, Instant::now()).unwrap();
        session
    }
    #[test]
    fn power_changes_keep_simulation_revision_and_manual_pause_unchanged() {
        let state = state();
        let mut session = session();
        session.set_condition(PauseReason::Manual, true).unwrap();
        let settings = session.local_settings().clone();
        let revision = state.conditions.revision();
        state.conditions.power_snapshot(0, 0);
        assert!(state.conditions.low_power());
        state.conditions.power_snapshot(255, 255);
        assert!(state.conditions.low_power());
        state.conditions.power_snapshot(1, 0);
        assert!(!state.conditions.low_power());
        state.setting_event(GUID_ACDC_POWER_SOURCE, 1);
        assert!(state.conditions.low_power());
        state.setting_event(GUID_POWER_SAVING_STATUS, 1);
        state.setting_event(GUID_ACDC_POWER_SOURCE, 0);
        assert!(
            state.conditions.low_power(),
            "saver is independent of AC power"
        );
        state.setting_event(GUID_POWER_SAVING_STATUS, 99);
        assert!(state.conditions.low_power());
        state.setting_event(GUID_POWER_SAVING_STATUS, 0);
        assert!(!state.conditions.low_power());
        state.setting_event(GUID_ACDC_POWER_SOURCE, 2);
        state.setting_event(GUID_ACDC_POWER_SOURCE, 99);
        assert!(state.conditions.low_power(), "UPS requests economy too");
        state.power_event(PBT_APMPOWERSTATUSCHANGE);
        assert!(state.refresh.get());
        assert_eq!(state.conditions.revision(), revision);
        assert!(!state.conditions.paused(Some(&settings)));
        state.conditions.apply_to(&mut session);
        assert_eq!(session.local_settings(), &settings);
        assert_eq!(
            session.snapshot(Instant::now()).pauses,
            [PauseReason::Manual].into()
        );
    }
    #[test]
    fn native_power_snapshot_has_documented_numeric_values() {
        let mut power = SYSTEM_POWER_STATUS::default();
        unsafe {
            GetSystemPowerStatus(&mut power).unwrap();
        }
        assert!([0, 1, 255].contains(&power.ACLineStatus));
        assert!([0, 1].contains(&power.SystemStatusFlag));
    }
    #[test]
    fn native_reasons_remain_independent_of_manual_pause_and_each_other() {
        let state = state();
        let mut session = session();
        session.set_condition(PauseReason::Manual, true).unwrap();
        state.session_event(WTS_SESSION_LOCK, 42);
        state.power_event(PBT_APMSUSPEND);
        state.display_event(0);
        state.conditions.apply_to(&mut session);
        assert_eq!(session.snapshot(Instant::now()).pauses.len(), 4);
        state.power_event(PBT_APMRESUMEAUTOMATIC);
        state.conditions.apply_to(&mut session);
        let snapshot = session.snapshot(Instant::now());
        assert!(!snapshot.pauses.contains(&PauseReason::Suspended));
        assert!(snapshot.pauses.contains(&PauseReason::DisplaysOff));
        assert!(snapshot.pauses.contains(&PauseReason::Locked));
        state.session_event(WTS_SESSION_UNLOCK, 42);
        state.display_event(1);
        state.conditions.apply_to(&mut session);
        assert_eq!(
            session.snapshot(Instant::now()).pauses,
            [PauseReason::Manual].into()
        );
        assert!(!session.snapshot(Instant::now()).should_render);
    }
    #[test]
    fn other_sessions_are_ignored_and_remote_setting_is_honored() {
        let state = state();
        let mut settings = Settings::default();
        state.session_event(WTS_SESSION_LOCK, 43);
        state.session_event(WTS_REMOTE_DISCONNECT, 43);
        assert!(!state.conditions.paused(Some(&settings)));
        state.session_event(WTS_REMOTE_CONNECT, 42);
        assert!(state.conditions.paused(Some(&settings)));
        settings.performance.pause_remote_session = false;
        assert!(!state.conditions.paused(Some(&settings)));
        state.session_event(WTS_REMOTE_DISCONNECT, 42);
        assert!(state.conditions.paused(Some(&settings)));
        state.session_event(WTS_CONSOLE_CONNECT, 42);
        assert!(!state.conditions.paused(Some(&Settings::default())));
    }
    #[test]
    fn dimming_unknown_notifications_and_initialization_do_not_cancel_a_pause() {
        let state = state();
        let before = state.conditions.revision();
        state.display_event(0);
        state.display_event(99);
        state.power_event(999);
        assert_eq!(state.conditions.revision(), before + 1);
        assert!(state.conditions.paused(Some(&Settings::default())));
        state.display_event(2);
        // Even a complete pause/resume burst between frames must reset the clock.
        assert_eq!(state.conditions.revision(), before + 2);
        assert!(!state.conditions.paused(Some(&Settings::default())));
        state.conditions.ready.store(false, Ordering::Release);
        assert!(state.conditions.paused(Some(&Settings::default())));
    }
    #[test]
    fn startup_snapshot_detects_lock_disconnection_and_remote_protocol() {
        assert_eq!(
            session_bits(WTSActive, WTS_SESSIONSTATE_LOCK, 0).unwrap(),
            LOCKED
        );
        assert_eq!(
            session_bits(WTSDisconnected, WTS_SESSIONSTATE_UNLOCK, 2).unwrap(),
            DISCONNECTED | REMOTE
        );
        assert_eq!(
            session_bits(WTSActive, WTS_SESSIONSTATE_UNLOCK, 0).unwrap(),
            0
        );
        assert!(session_bits(WTSActive, WTS_SESSIONSTATE_UNKNOWN, 0).is_err());
    }
    #[test]
    fn current_session_query_reads_only_numeric_state() {
        use windows::Win32::System::{
            RemoteDesktop::ProcessIdToSessionId, Threading::GetCurrentProcessId,
        };
        let mut session_id = 0;
        unsafe {
            ProcessIdToSessionId(GetCurrentProcessId(), &mut session_id).unwrap();
        }
        let info: WTSINFOEXW = query(session_id, WTSSessionInfoEx).unwrap();
        assert_eq!(info.Level, 1);
        let numeric = unsafe {
            (
                info.Data.WTSInfoExLevel1.SessionId,
                info.Data.WTSInfoExLevel1.SessionFlags,
            )
        };
        assert_eq!(numeric.0, session_id);
        assert!(
            [
                WTS_SESSIONSTATE_LOCK,
                WTS_SESSIONSTATE_UNLOCK,
                WTS_SESSIONSTATE_UNKNOWN
            ]
            .contains(&(numeric.1 as u32))
        );
        let protocol: u16 = query(session_id, WTSClientProtocolType).unwrap();
        assert!(protocol <= 2);
    }
    #[test]
    fn hidden_native_window_dispatches_notifications_and_releases_registrations() {
        let conditions = Arc::new(Conditions::default());
        let observer = Observer::new(42, conditions.clone()).unwrap();
        assert_eq!(observer.power.len(), 3);
        let hwnd = observer.hwnd;
        assert!(!unsafe { IsWindowVisible(hwnd) }.as_bool());
        unsafe {
            let _ = SendMessageW(
                hwnd,
                WM_WTSSESSION_CHANGE,
                Some(WPARAM(WTS_SESSION_LOCK as usize)),
                Some(LPARAM(42)),
            );
            let _ = SendMessageW(
                hwnd,
                WM_POWERBROADCAST,
                Some(WPARAM(PBT_APMSUSPEND as usize)),
                Some(LPARAM(0)),
            );
        }
        assert_eq!(
            conditions.bits.load(Ordering::Acquire) & (LOCKED | SUSPENDED),
            LOCKED | SUSPENDED
        );
        #[repr(C)]
        struct PowerPacket {
            guid: windows::core::GUID,
            length: u32,
            value: u32,
        }
        let mut packet = PowerPacket {
            guid: GUID_SESSION_DISPLAY_STATUS,
            length: 1,
            value: 0,
        };
        unsafe {
            let _ = SendMessageW(
                hwnd,
                WM_POWERBROADCAST,
                Some(WPARAM(PBT_POWERSETTINGCHANGE as usize)),
                Some(LPARAM((&packet as *const PowerPacket) as isize)),
            );
        }
        assert_eq!(conditions.bits.load(Ordering::Acquire) & DISPLAYS_OFF, 0);
        packet.length = 4;
        unsafe {
            let _ = SendMessageW(
                hwnd,
                WM_POWERBROADCAST,
                Some(WPARAM(PBT_POWERSETTINGCHANGE as usize)),
                Some(LPARAM((&packet as *const PowerPacket) as isize)),
            );
            let _ = SendMessageW(hwnd, WM_ENDSESSION, Some(WPARAM(1)), Some(LPARAM(0)));
        }
        assert_ne!(conditions.bits.load(Ordering::Acquire) & DISPLAYS_OFF, 0);
        packet.guid = GUID_ACDC_POWER_SOURCE;
        packet.value = 1;
        packet.length = 1;
        let notify = |packet: &PowerPacket| unsafe {
            SendMessageW(
                hwnd,
                WM_POWERBROADCAST,
                Some(WPARAM(PBT_POWERSETTINGCHANGE as usize)),
                Some(LPARAM((packet as *const PowerPacket) as isize)),
            )
        };
        notify(&packet);
        assert!(!conditions.low_power());
        packet.length = 4;
        notify(&packet);
        assert!(conditions.low_power());
        packet.value = 0;
        notify(&packet);
        assert!(!conditions.low_power());
        packet.guid = GUID_POWER_SAVING_STATUS;
        packet.value = 1;
        notify(&packet);
        assert!(conditions.low_power());
        assert!(observer.state.ending.get());
        assert_ne!(conditions.bits.load(Ordering::Acquire) & DISCONNECTED, 0);
        drop(observer);
        assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
    }
}
