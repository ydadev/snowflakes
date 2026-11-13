//! Deterministic local monitor identifiers. Raw Windows interface paths never
//! leave the native discovery boundary or enter status/configuration files.
pub fn key(interface_paths: &[String], fallback_device: &str) -> (u64, bool) {
    let mut paths: Vec<String> = interface_paths
        .iter()
        .filter(|p| !p.is_empty())
        .map(|p| p.to_ascii_lowercase())
        .collect();
    paths.sort_unstable();
    paths.dedup();
    let persistent = !paths.is_empty();
    if !persistent {
        paths.push(fallback_device.to_ascii_lowercase());
    }
    let mut hash = 0xcbf29ce484222325_u64;
    let domain: &[u8] = if persistent {
        b"monitor-interface-v1"
    } else {
        b"monitor-gdi-v1"
    };
    for part in std::iter::once(domain).chain(paths.iter().map(|s| s.as_bytes())) {
        for byte in part.iter().copied().chain(std::iter::once(0)) {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    // The upper bit belongs to ground support identities, not display devices.
    ((hash & 0x7fff_ffff_ffff_ffff).max(1), persistent)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interface_identity_ignores_adapter_renumbering_and_order() {
        let first = key(
            &["SYNTHETIC-MONITOR-B".into(), "synthetic-monitor-a".into()],
            "adapter-1",
        );
        let second = key(
            &[
                "SYNTHETIC-MONITOR-A".into(),
                "synthetic-monitor-b".into(),
                "synthetic-monitor-a".into(),
            ],
            "adapter-8",
        );
        assert_eq!(first, second);
        // Persisted configurations depend on the versioned hash staying stable.
        assert_eq!(first.0, 0x467c_d208_6876_0ffa);
        assert!(first.1 && first.0 != 0 && first.0 & (1 << 63) == 0);
        assert_ne!(first, key(&["synthetic-monitor-c".into()], "adapter-1"));
    }
    #[test]
    fn fallback_is_explicit_and_separate_from_a_physical_interface() {
        assert!(!key(&[], "virtual-device").1);
        assert_eq!(
            key(&[], "VIRTUAL-DEVICE"),
            key(&[String::new()], "virtual-device")
        );
        assert_ne!(
            key(&[], "virtual-device").0,
            key(&["virtual-device".into()], "ignored").0
        );
        assert_ne!(key(&[], "virtual-device-a"), key(&[], "virtual-device-b"));
    }
}
