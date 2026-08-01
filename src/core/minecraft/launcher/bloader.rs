use std::sync::OnceLock;

use pelite::pe64::{Pe, PeFile};

const COMPRESSED_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/BLoader.dll.zst"));
const EMBEDDED_VERSION: &str = env!("BMCBL_BLOADER_VERSION");

#[repr(C)]
#[allow(non_snake_case)]
struct VsFixedFileInfoWin32 {
    dwSignature: u32,
    dwStrucVersion: u32,
    dwFileVersionMS: u32,
    dwFileVersionLS: u32,
    dwProductVersionMS: u32,
    dwProductVersionLS: u32,
    dwFileFlagsMask: u32,
    dwFileFlags: u32,
    dwFileOS: u32,
    dwFileType: u32,
    dwFileSubtype: u32,
    dwFileDateMS: u32,
    dwFileDateLS: u32,
}

static DECOMPRESSED_BYTES: OnceLock<Result<Vec<u8>, String>> = OnceLock::new();

pub(super) fn embedded_version_string() -> &'static str {
    EMBEDDED_VERSION
}

pub(super) fn version_string(bytes: &[u8]) -> Option<String> {
    let file = PeFile::from_bytes(bytes).ok()?;
    let resources = file.resources().ok()?;
    let version_info = resources.version_info().ok()?;
    let fixed = version_info.fixed()?;
    // SAFETY: `fixed()` returns a valid VS_FIXEDFILEINFO-compatible blob owned by pelite.
    let info = unsafe { &*(fixed as *const _ as *const VsFixedFileInfoWin32) };
    Some(
        [
            ((info.dwFileVersionMS >> 16) & 0xFFFF) as u64,
            (info.dwFileVersionMS & 0xFFFF) as u64,
            ((info.dwFileVersionLS >> 16) & 0xFFFF) as u64,
            (info.dwFileVersionLS & 0xFFFF) as u64,
        ]
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join("."),
    )
}

pub(super) fn bytes() -> Result<&'static [u8], String> {
    match DECOMPRESSED_BYTES.get_or_init(|| {
        zstd::decode_all(COMPRESSED_BYTES)
            .map_err(|error| format!("解压内嵌 BLoader.dll 失败: {error}"))
    }) {
        Ok(bytes) => Ok(bytes.as_slice()),
        Err(error) => Err(error.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::{bytes, embedded_version_string, version_string};

    #[test]
    fn embedded_bloader_decompresses_to_pe_image() -> Result<(), String> {
        let bytes = bytes()?;
        assert_eq!(bytes.get(..2), Some(b"MZ".as_slice()));
        Ok(())
    }

    #[test]
    fn compile_time_version_matches_embedded_bloader() -> Result<(), String> {
        let bytes = bytes()?;
        assert_eq!(
            version_string(bytes).as_deref(),
            Some(embedded_version_string())
        );
        Ok(())
    }
}
