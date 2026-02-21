use crate::utils::file_ops;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::runtime_assets::{WINDIVERT64_SYS, WINTUN_DLL};

static EASYTIER_RUNTIME_INIT: OnceLock<Result<(), String>> = OnceLock::new();

fn runtime_dir() -> PathBuf {
    file_ops::bmcbl_subdir("runtime").join("easytier")
}

fn write_if_missing_or_size_mismatch(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let need_write = match fs::metadata(path) {
        Ok(m) => m.len() != bytes.len() as u64,
        Err(_) => true,
    };
    if !need_write {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Atomic-ish replace: write temp then rename.
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    let _ = fs::remove_file(path);
    fs::rename(tmp, path)?;
    Ok(())
}

#[cfg(windows)]
fn load_library_from(path: &Path) -> Result<(), String> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::System::LibraryLoader::LoadLibraryW;

    let wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let handle = unsafe { LoadLibraryW(PCWSTR(wide.as_ptr())) }
        .map_err(|e| format!("LoadLibraryW failed for {} ({e})", path.display()))?;
    if handle.is_invalid() {
        return Err(format!("LoadLibraryW returned invalid handle for {}", path.display()));
    }
    Ok(())
}

/// Ensures EasyTier runtime dependencies (wintun, windivert) are embedded and available on disk,
/// and preloads wintun.dll so embedded EasyTier can create a virtual NIC reliably.
pub fn ensure_easytier_runtime_ready() -> Result<(), String> {
    EASYTIER_RUNTIME_INIT
        .get_or_init(|| {
            let dir = runtime_dir();
            fs::create_dir_all(&dir).map_err(|e| format!("create runtime dir failed: {e}"))?;

            if let Some(bytes) = WINTUN_DLL {
                let dst = dir.join("wintun.dll");
                write_if_missing_or_size_mismatch(&dst, bytes)
                    .map_err(|e| format!("write wintun.dll failed: {e}"))?;

                #[cfg(windows)]
                load_library_from(&dst)?;
            } else {
                return Err("wintun.dll is not bundled (missing EasyTier third_party assets during build)".to_string());
            }

            // Best-effort: EasyTier can run without WinDivert in some modes, so don't fail hard.
            if let Some(bytes) = WINDIVERT64_SYS {
                let dst = dir.join("WinDivert64.sys");
                let _ = write_if_missing_or_size_mismatch(&dst, bytes);
            }

            Ok(())
        })
        .clone()
}
