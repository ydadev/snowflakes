//! CLI addressing metadata, separate from the typed IPC command and launch path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub session_id: u32,
    pub user_sid: Option<String>,
}

/// Extract global addressing options without choosing an arbitrary logged-on user.
pub fn parse(args: &[String]) -> Result<(Option<Selection>, Vec<String>), &'static str> {
    let mut session = None;
    let mut sid = None;
    let mut remaining = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--session" => {
                if session.is_some() {
                    return Err("duplicate_session_selector");
                }
                index += 1;
                let value = args.get(index).ok_or("missing_session_id")?;
                if value.is_empty()
                    || value.len() > 10
                    || !value.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err("invalid_session_id");
                }
                let value = value.parse::<u32>().map_err(|_| "invalid_session_id")?;
                if value == u32::MAX {
                    return Err("invalid_session_id");
                }
                session = Some(value);
            }
            "--user-sid" => {
                if sid.is_some() {
                    return Err("duplicate_user_sid");
                }
                index += 1;
                let value = args.get(index).ok_or("missing_user_sid")?;
                // Native conversion subsequently validates and canonicalizes the SID.
                // Aliases, paths, embedded NULs and unbounded endpoint components are rejected here.
                if !(6..=192).contains(&value.len())
                    || !value.starts_with("S-1-")
                    || !value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                {
                    return Err("invalid_user_sid");
                }
                sid = Some(value.clone());
            }
            _ => remaining.push(args[index].clone()),
        }
        index += 1;
    }
    match session {
        Some(session_id) => Ok((
            Some(Selection {
                session_id,
                user_sid: sid,
            }),
            remaining,
        )),
        None if sid.is_some() => Err("user_sid_requires_session"),
        None => Ok((None, remaining)),
    }
}

/// The caller's process token is the native adapter's source of this role.
pub fn allowed(caller_sid: &str, target_sid: &str, enabled_administrator: bool) -> bool {
    caller_sid == target_sid || caller_sid == "S-1-5-18" || enabled_administrator
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_address_preserves_command_and_optional_owner_in_any_position() {
        for args in [
            vec![
                "--session",
                "12",
                "--status",
                "--json",
                "--user-sid",
                "S-1-5-21-100-200-300-1001",
            ],
            vec![
                "--user-sid",
                "S-1-5-21-100-200-300-1001",
                "--status",
                "--session",
                "12",
                "--json",
            ],
        ] {
            let (target, args) =
                parse(&args.into_iter().map(str::to_owned).collect::<Vec<_>>()).unwrap();
            assert_eq!(
                target,
                Some(Selection {
                    session_id: 12,
                    user_sid: Some("S-1-5-21-100-200-300-1001".into())
                })
            );
            assert_eq!(args, ["--status", "--json"]);
            assert!(crate::control::parse_command(&args).is_ok());
        }
        let (target, args) = parse(&["--pause".into()]).unwrap();
        assert!(target.is_none());
        assert_eq!(args, ["--pause"]);
        let (target, _) = parse(&["--session".into(), "0".into(), "--status".into()]).unwrap();
        assert_eq!(target.unwrap().session_id, 0);
    }
    #[test]
    fn ambiguous_reserved_and_endpoint_injection_arguments_are_rejected() {
        for args in [
            vec!["--session"],
            vec!["--session", "-1"],
            vec!["--session", "+1"],
            vec!["--session", " 1"],
            vec!["--session", "4294967295"],
            vec!["--session", "4294967296"],
            vec!["--session", "1", "--session", "2"],
            vec!["--user-sid", "S-1-5-18"],
            vec!["--session", "1", "--user-sid"],
            vec!["--session", "1", "--user-sid", "BA"],
            vec!["--session", "1", "--user-sid", "S-1-5-18\0suffix"],
            vec!["--session", "1", "--user-sid", "S-1-5-18\\pipe"],
            vec![
                "--session",
                "1",
                "--user-sid",
                "S-1-5-18",
                "--user-sid",
                "S-1-5-18",
            ],
        ] {
            assert!(parse(&args.into_iter().map(str::to_owned).collect::<Vec<_>>()).is_err());
        }
        assert!(
            parse(&[
                "--session".into(),
                "1".into(),
                "--user-sid".into(),
                format!("S-1-{}", "1".repeat(192))
            ])
            .is_err()
        );
    }
    #[test]
    fn foreign_owner_requires_an_enabled_admin_role_or_system() {
        let owner = "S-1-5-21-100-200-300-1001";
        let other = "S-1-5-21-100-200-300-1002";
        assert!(allowed(owner, owner, false));
        assert!(!allowed(owner, other, false));
        assert!(allowed(owner, other, true));
        assert!(allowed("S-1-5-18", other, false));
    }
}
