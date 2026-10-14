//! ZIP directory-name filtering before opening payload readers.

use std::io::{Read, Seek};
use std::path::Path;

use zip::ZipArchive;

pub(super) fn scan_candidate<R: Read + Seek>(archive: &ZipArchive<R>, index: usize) -> bool {
    let Some(name) = archive.name_for_index(index) else {
        return false;
    };
    // scan_archive propagates name-decoding failures only after opening the
    // entry successfully. Keep that path, including unsupported/encrypted files.
    let Ok(name) = name else {
        return true;
    };
    if name.contains("__MACOSX") {
        return false;
    }
    let path = Path::new(name.as_ref());
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("level.dat" | "manifest.json")
    ) || path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["mcpack", "mcaddon", "mcworld", "mctemplate", "zip"]
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

pub(super) fn name_matches<R: Read + Seek>(
    archive: &ZipArchive<R>,
    index: usize,
    predicate: impl FnOnce(&str) -> bool,
) -> bool {
    archive
        .name_for_index(index)
        .is_some_and(|name| name.is_ok_and(|name| predicate(&name)))
}

pub(super) fn path_eq(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left.bytes().zip(right.bytes()).all(|(left, right)| {
            let left = if left == b'\\' { b'/' } else { left };
            let right = if right == b'\\' { b'/' } else { right };
            left.eq_ignore_ascii_case(&right)
        })
}
