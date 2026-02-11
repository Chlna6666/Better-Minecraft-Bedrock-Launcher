// src-tauri/src/utils/npcap.rs
//
// EasyTier (via pnet on Windows) requires Npcap/WinPcap runtime DLLs.
// If Npcap is not installed, EasyTier may fail to start with "Packet.dll not found".

#[cfg(target_os = "windows")]
use std::path::PathBuf;

#[cfg(target_os = "windows")]
use windows::Win32::System::LibraryLoader::{FreeLibrary, GetModuleHandleW, LoadLibraryW};

#[cfg(target_os = "windows")]
use windows::core::w;

#[cfg(target_os = "windows")]
fn dll_search_candidates(dll: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();

    if let Ok(windir) = std::env::var("WINDIR") {
        // Npcap typically installs into %WINDIR%\System32\Npcap (and SysWOW64\Npcap on 64-bit).
        out.push(PathBuf::from(&windir).join("System32").join("Npcap").join(dll));
        out.push(PathBuf::from(&windir).join("SysWOW64").join("Npcap").join(dll));
        out.push(PathBuf::from(&windir).join("System32").join(dll));
        out.push(PathBuf::from(&windir).join("SysWOW64").join(dll));
    }

    // Also try current directory (next to the exe).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.push(dir.join(dll));
        }
    }

    out
}

#[cfg(target_os = "windows")]
fn can_load_library(dll: &str) -> bool {
    unsafe {
        if GetModuleHandleW(w!(dll)).is_ok() {
            return true;
        }
        if let Ok(h) = LoadLibraryW(w!(dll)) {
            let _ = FreeLibrary(h);
            return true;
        }
    }
    false
}

/// Ensure Npcap runtime DLLs exist for EasyTier on Windows.
///
/// Returns a user-facing error message if missing.
#[cfg(target_os = "windows")]
pub fn ensure_npcap_runtime() -> Result<(), String> {
    // Packet.dll + wpcap.dll are the typical runtime pair.
    // We check both because missing either can break pcap-based operations.
    for dll in ["Packet.dll", "wpcap.dll"] {
        if can_load_library(dll) {
            continue;
        }

        // If not loadable, see if it exists in well-known locations. This helps with better error text.
        let candidates = dll_search_candidates(dll);
        if candidates.iter().any(|p| p.exists()) {
            // Exists but not loadable: PATH/DLL search order issue.
            return Err(format!(
                "{dll} 已存在但无法加载，可能是环境变量 PATH 未包含 Npcap 目录或 DLL 损坏。请重新安装 Npcap 并启用 WinPcap 兼容模式后重试。"
            ));
        }

        return Err(format!(
            "缺少依赖 {dll}，EasyTier 无法启动。请安装 Npcap（建议勾选 WinPcap Compatible Mode）后重试。"
        ));
    }

    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn ensure_npcap_runtime() -> Result<(), String> {
    Ok(())
}

