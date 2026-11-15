//! Resolve only local IPC destinations. No token is impersonated and no process is launched.
use super::*;
use crate::control::target::{Selection, allowed};
use windows::Win32::System::RemoteDesktop::*;

struct Sid {
    raw: PSID,
    _memory: LocalMemory,
}
impl Sid {
    fn parse(value: &str) -> Result<Self> {
        let value = wide(value);
        let mut raw = PSID::default();
        // The input remains live; the returned LocalAlloc buffer is owned here.
        unsafe {
            ConvertStringSidToSidW(PCWSTR(value.as_ptr()), &mut raw)?;
        }
        Ok(Self {
            raw,
            _memory: LocalMemory(HLOCAL(raw.0)),
        })
    }
}
fn sid_text(sid: PSID) -> Result<String> {
    let mut text = PWSTR::null();
    // All callers retain the validated SID allocation through conversion.
    unsafe {
        ConvertSidToStringSidW(sid, &mut text)?;
        let _memory = LocalMemory(HLOCAL(text.0.cast()));
        Ok(text.to_string()?)
    }
}

pub(in crate::platform) fn resolve(caller: &Identity, selection: &Selection) -> Result<Endpoint> {
    resolve_with(
        caller,
        selection,
        session_owner_sid,
        process_is_administrator,
    )
}
fn resolve_with(
    caller: &Identity,
    selection: &Selection,
    owner: impl FnOnce(u32) -> Result<String>,
    administrator: impl FnOnce() -> Result<bool>,
) -> Result<Endpoint> {
    let sid = match &selection.user_sid {
        Some(text) => {
            let parsed = Sid::parse(text)?;
            sid_text(parsed.raw)?
        }
        None => owner(selection.session_id)?,
    };
    let privileged = if allowed(&caller.sid, &sid, false) {
        false
    } else {
        administrator()?
    };
    if !allowed(&caller.sid, &sid, privileged) {
        return Err(failure(ERROR_ACCESS_DENIED));
    }
    let endpoint = Endpoint {
        sid,
        session_id: selection.session_id,
        namespace: caller.namespace.clone(),
    };
    // The Win32 limit is 256 characters, excluding the terminating NUL.
    if endpoint.pipe_name().len() > 257 {
        return Err(failure(ERROR_INVALID_PARAMETER));
    }
    Ok(endpoint)
}

fn administrator_in_token(primary: HANDLE) -> Result<bool> {
    let admin = Sid::parse("S-1-5-32-544")?;
    let mut duplicate = HANDLE::default();
    let mut member = windows::core::BOOL::default();
    // Use the same process identity as Identity::current, never an ambient thread token.
    // The duplicate is queried only; it is never assigned to a thread or process.
    unsafe {
        DuplicateToken(primary, SecurityIdentification, &mut duplicate)?;
        let duplicate = Handle(duplicate);
        CheckTokenMembership(Some(duplicate.0), admin.raw, &mut member)?;
    }
    Ok(member.as_bool())
}
fn process_is_administrator() -> Result<bool> {
    let mut token = HANDLE::default();
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_DUPLICATE,
            &mut token,
        )?;
    }
    let token = Handle(token);
    administrator_in_token(token.0)
}

fn session_text(session: u32, class: WTS_INFO_CLASS) -> Result<String> {
    struct Buffer(PWSTR);
    impl Drop for Buffer {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    WTSFreeMemory(self.0.0.cast());
                }
            }
        }
    }
    let mut text = PWSTR::null();
    let mut bytes = 0u32;
    let result =
        unsafe { WTSQuerySessionInformationW(None, session, class, &mut text, &mut bytes) };
    let _buffer = Buffer(text);
    result.map_err(|error| {
        if error.code() == HRESULT::from_win32(ERROR_INVALID_PARAMETER.0)
            || error.code() == HRESULT::from_win32(ERROR_CTX_WINSTATION_NOT_FOUND.0)
        {
            failure(ERROR_NO_SUCH_LOGON_SESSION)
        } else {
            error
        }
    })?;
    if text.is_null() || bytes == 0 || bytes > 8192 || !bytes.is_multiple_of(2) {
        return Err(failure(ERROR_INVALID_DATA));
    }
    // WTS owns an aligned UTF-16 allocation with the returned byte length.
    let chars = unsafe { std::slice::from_raw_parts(text.0, (bytes / 2) as usize) };
    let end = chars
        .iter()
        .position(|&v| v == 0)
        .ok_or_else(|| failure(ERROR_INVALID_DATA))?;
    String::from_utf16(&chars[..end]).map_err(|_| failure(ERROR_INVALID_DATA))
}
fn session_account(session: u32) -> Result<(String, String)> {
    let user = session_text(session, WTSUserName)?;
    let domain = session_text(session, WTSDomainName)?;
    if user.is_empty() || domain.is_empty() {
        return Err(failure(ERROR_NO_SUCH_LOGON_SESSION));
    }
    if user.contains('\\') || domain.contains('\\') {
        return Err(failure(ERROR_INVALID_DATA));
    }
    Ok((domain, user))
}
fn session_owner_sid(session: u32) -> Result<String> {
    let first = session_account(session)?;
    // Reject a change observed during resolution instead of combining two logons.
    if session_account(session)? != first {
        return Err(failure(ERROR_RETRY));
    }
    lookup_user_sid(&format!("{}\\{}", first.0, first.1))
}
fn lookup_user_sid(account: &str) -> Result<String> {
    let name = wide(account);
    let mut sid_bytes = 0u32;
    let mut domain_chars = 0u32;
    let mut kind = SID_NAME_USE::default();
    let probe = unsafe {
        LookupAccountNameW(
            PCWSTR::null(),
            PCWSTR(name.as_ptr()),
            None,
            &mut sid_bytes,
            None,
            &mut domain_chars,
            &mut kind,
        )
    };
    match probe {
        Err(e) if e.code() == HRESULT::from_win32(ERROR_INSUFFICIENT_BUFFER.0) => {}
        Err(e) => return Err(e),
        Ok(()) => return Err(failure(ERROR_INVALID_DATA)),
    }
    if sid_bytes == 0 || sid_bytes > SECURITY_MAX_SID_SIZE || domain_chars > 32768 {
        return Err(failure(ERROR_INVALID_DATA));
    }
    let mut sid = vec![0usize; (sid_bytes as usize).div_ceil(size_of::<usize>())];
    let mut domain = vec![0u16; domain_chars as usize];
    let raw = PSID(sid.as_mut_ptr().cast());
    // Aligned bounded buffers remain live through the second lookup and conversion.
    unsafe {
        LookupAccountNameW(
            PCWSTR::null(),
            PCWSTR(name.as_ptr()),
            Some(raw),
            &mut sid_bytes,
            Some(PWSTR(domain.as_mut_ptr())),
            &mut domain_chars,
            &mut kind,
        )?;
    }
    if kind != SidTypeUser {
        return Err(failure(ERROR_NO_SUCH_LOGON_SESSION));
    }
    sid_text(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection(session_id: u32, sid: Option<&str>) -> Selection {
        Selection {
            session_id,
            user_sid: sid.map(str::to_owned),
        }
    }
    fn forbidden_owner(_: u32) -> Result<String> {
        panic!("an explicit SID must bypass account lookup")
    }
    fn forbidden_role() -> Result<bool> {
        panic!("the owner and SYSTEM must not require an administrator lookup")
    }
    fn denied(result: Result<Endpoint>, code: WIN32_ERROR) {
        assert!(matches!(result, Err(e) if e.code() == HRESULT::from_win32(code.0)));
    }

    #[test]
    fn explicit_owner_and_system_resolve_without_account_or_role_lookup() {
        let mut caller = Identity::current("target-owner").unwrap();
        let expected = caller.pipe_name();
        let target = resolve_with(
            &caller,
            &selection(caller.session_id, Some(&caller.sid)),
            forbidden_owner,
            forbidden_role,
        )
        .unwrap();
        assert!(target.pipe_name() == expected);
        caller.sid = "S-1-5-18".into();
        let target = resolve_with(
            &caller,
            &selection(12, Some("S-1-5-21-100-200-300-1001")),
            forbidden_owner,
            forbidden_role,
        )
        .unwrap();
        assert_eq!(target.session_id, 12);
        assert_eq!(target.namespace, "target-owner");
    }

    #[test]
    fn foreign_owner_needs_enabled_role_and_resolution_failure_never_falls_back() {
        let mut caller = Identity::current("target-denied").unwrap();
        caller.sid = "S-1-5-21-100-200-300-1001".into();
        let selected = selection(12, Some("S-1-5-21-100-200-300-1002"));
        denied(
            resolve_with(&caller, &selected, forbidden_owner, || Ok(false)),
            ERROR_ACCESS_DENIED,
        );
        assert!(resolve_with(&caller, &selected, forbidden_owner, || Ok(true)).is_ok());
        denied(
            resolve_with(
                &caller,
                &selection(12, None),
                |_| Err(failure(ERROR_NO_SUCH_LOGON_SESSION)),
                forbidden_role,
            ),
            ERROR_NO_SUCH_LOGON_SESSION,
        );
        denied(
            resolve_with(
                &caller,
                &selection(12, Some("S-1-not-a-sid")),
                forbidden_owner,
                forbidden_role,
            ),
            ERROR_INVALID_SID,
        );
    }

    #[test]
    fn canonical_endpoint_cannot_exceed_win32_pipe_name_limit() {
        let mut caller = Identity::current(&"x".repeat(48)).unwrap();
        caller.sid = "S-1-5-18".into();
        let long = format!("S-1-0xFFFFFFFFFFFF{}", "-4294967295".repeat(15));
        denied(
            resolve_with(
                &caller,
                &selection(u32::MAX - 1, Some(&long)),
                forbidden_owner,
                forbidden_role,
            ),
            ERROR_INVALID_PARAMETER,
        );
    }

    #[test]
    fn current_wts_owner_matches_process_or_has_no_interactive_owner() {
        let caller = Identity::current("target-wts").unwrap();
        match session_owner_sid(caller.session_id) {
            Ok(sid) => assert!(sid == caller.sid),
            Err(error) => {
                assert_eq!(caller.session_id, 0);
                assert_eq!(
                    error.code(),
                    HRESULT::from_win32(ERROR_NO_SUCH_LOGON_SESSION.0)
                );
            }
        }
    }

    #[test]
    fn process_role_ignores_ambient_anonymous_token_and_disabled_admin_group() {
        fn admin_attributes(token: HANDLE) -> Option<u32> {
            let mut needed = 0;
            unsafe {
                let _ = GetTokenInformation(token, TokenGroups, None, 0, &mut needed);
            }
            assert!(needed > 0 && needed <= 65536);
            let mut bytes = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
            unsafe {
                GetTokenInformation(
                    token,
                    TokenGroups,
                    Some(bytes.as_mut_ptr().cast()),
                    needed,
                    &mut needed,
                )
                .unwrap();
                let groups = &*bytes.as_ptr().cast::<TOKEN_GROUPS>();
                assert!(
                    std::mem::offset_of!(TOKEN_GROUPS, Groups)
                        + groups.GroupCount as usize * size_of::<SID_AND_ATTRIBUTES>()
                        <= needed as usize
                );
                let groups =
                    std::slice::from_raw_parts(groups.Groups.as_ptr(), groups.GroupCount as usize);
                groups.iter().find_map(|group| {
                    (sid_text(group.Sid).unwrap() == "S-1-5-32-544").then_some(group.Attributes)
                })
            }
        }
        let mut raw = HANDLE::default();
        unsafe {
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY | TOKEN_DUPLICATE, &mut raw).unwrap();
        }
        let primary = Handle(raw);
        let expected = administrator_in_token(primary.0).unwrap();
        struct Revert;
        impl Drop for Revert {
            fn drop(&mut self) {
                unsafe { RevertToSelf().unwrap() };
            }
        }
        unsafe { ImpersonateAnonymousToken(GetCurrentThread()).unwrap() };
        let revert = Revert;
        // An explicit handle remains authoritative even while the thread is anonymous.
        assert_eq!(administrator_in_token(primary.0).unwrap(), expected);
        drop(revert);
        let admin = Sid::parse("S-1-5-32-544").unwrap();
        let disabled = [SID_AND_ATTRIBUTES {
            Sid: admin.raw,
            Attributes: 0,
        }];
        let mut restricted = HANDLE::default();
        unsafe {
            CreateRestrictedToken(
                primary.0,
                CREATE_RESTRICTED_TOKEN_FLAGS(0),
                Some(&disabled),
                None,
                None,
                &mut restricted,
            )
            .unwrap();
        }
        let restricted = Handle(restricted);
        assert!(!administrator_in_token(restricted.0).unwrap());
        if admin_attributes(primary.0).is_some() {
            use windows::Win32::System::SystemServices::{
                SE_GROUP_ENABLED, SE_GROUP_USE_FOR_DENY_ONLY,
            };
            let attributes = admin_attributes(restricted.0).unwrap();
            assert_ne!(attributes & SE_GROUP_USE_FOR_DENY_ONLY as u32, 0);
            assert_eq!(attributes & SE_GROUP_ENABLED as u32, 0);
            println!("restricted administrator group verified as deny-only");
        }
    }
}
