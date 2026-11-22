//! Unified Minecraft Bedrock platform/version rules.
//!
//! Self-contained on purpose: BMCBL does not depend on the MCProX crate. The
//! thresholds and comparison semantics are kept in sync with
//! `ZCodeProject/MCProX/crates/mcprox-pe` by the boundary tests below.
//!
//! Before this module, BMCBL had two copies of `is_win32_version` with
//! different thresholds (`1.21.12000.20` in `installed.rs`, `1.21.12000.21` in
//! `task/windows.rs`); both now route through here.

use std::cmp::Ordering;

/// First full-trust Win32 (GDK) build.
pub(crate) const GDK_VERSION_THRESHOLD: &str = "1.21.12000.20";

/// Below this version BLoader is not deployed (legacy UWP path).
pub(crate) const LEGACY_UWP_CUTOFF: &str = "1.21.0.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BloaderSupport {
    /// GDK: BMCBL can deliver the DLL through the virtual-mod launch plan.
    VirtualMods,
    /// Modern UWP: BLoader runs, but the virtual-mod plan is not wired, so the
    /// DLL must be installed under `BLoader/mods`.
    PhysicalModOnly,
    /// Legacy UWP / unknown: BLoader is not available.
    Unsupported,
}

impl BloaderSupport {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::VirtualMods => "virtual-mods",
            Self::PhysicalModOnly => "physical-mod-only",
            Self::Unsupported => "unsupported",
        }
    }
}

fn next_number(version: &str, cursor: &mut usize) -> Option<u64> {
    let bytes = version.as_bytes();
    let len = bytes.len();

    while *cursor < len {
        if bytes[*cursor].is_ascii_digit() {
            break;
        }
        *cursor += 1;
    }

    if *cursor >= len {
        return None;
    }

    let start = *cursor;
    while *cursor < len && bytes[*cursor].is_ascii_digit() {
        *cursor += 1;
    }

    version[start..*cursor].parse::<u64>().ok()
}

/// Compares two version strings by their ordered numeric groups.
pub(crate) fn compare_versions(left: &str, right: &str) -> Ordering {
    let mut left_cursor = 0;
    let mut right_cursor = 0;

    loop {
        let left_number = next_number(left, &mut left_cursor);
        let right_number = next_number(right, &mut right_cursor);

        match (left_number, right_number) {
            (Some(left_number), Some(right_number)) => match left_number.cmp(&right_number) {
                Ordering::Equal => continue,
                non_equal => return non_equal,
            },
            (Some(left_number), None) => {
                return if left_number == 0 {
                    Ordering::Equal
                } else {
                    Ordering::Greater
                };
            }
            (None, Some(right_number)) => {
                return if right_number == 0 {
                    Ordering::Equal
                } else {
                    Ordering::Less
                };
            }
            (None, None) => return Ordering::Equal,
        }
    }
}

fn version_components(version: &str) -> (u64, u64, u64) {
    let mut cursor = 0;
    (
        next_number(version, &mut cursor).unwrap_or(0),
        next_number(version, &mut cursor).unwrap_or(0),
        next_number(version, &mut cursor).unwrap_or(0),
    )
}

/// True for full-trust Win32 (GDK) builds.
pub(crate) fn is_win32_version(version: &str) -> bool {
    let (major, minor, build) = version_components(version);

    if major == 1 && minor == 21 && build >= 12201 {
        return true;
    }

    compare_versions(version, GDK_VERSION_THRESHOLD) != Ordering::Less
}

/// True for UWP builds older than 1.21, where BLoader is not deployed.
pub(crate) fn is_legacy_uwp(version: &str) -> bool {
    compare_versions(version, LEGACY_UWP_CUTOFF) == Ordering::Less
}

pub(crate) fn bloader_support(version: &str) -> BloaderSupport {
    if is_win32_version(version) {
        BloaderSupport::VirtualMods
    } else if is_legacy_uwp(version) {
        BloaderSupport::Unsupported
    } else {
        BloaderSupport::PhysicalModOnly
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gdk_threshold_is_unified() {
        assert!(!is_win32_version("1.21.12000.19"));
        assert!(is_win32_version("1.21.12000.20"));
        assert!(is_win32_version("1.21.12000.21"));
        assert!(is_win32_version("1.26.60.5"));
    }

    #[test]
    fn legacy_uwp_is_below_1_21() {
        assert!(is_legacy_uwp("1.20.81.01"));
        assert!(!is_legacy_uwp("1.21.0.0"));
    }

    #[test]
    fn bloader_support_matches_platform() {
        assert_eq!(
            bloader_support("1.20.81.01"),
            BloaderSupport::Unsupported
        );
        assert_eq!(
            bloader_support("1.21.51.02"),
            BloaderSupport::PhysicalModOnly
        );
        assert_eq!(
            bloader_support("1.26.60.5"),
            BloaderSupport::VirtualMods
        );
    }
}
