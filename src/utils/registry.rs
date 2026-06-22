// src-tauri/src/utils/registry.rs
use std::env;
use tracing::{debug, error, info, warn};
use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
use winreg::RegKey;
use winreg::enums::*;

// --- 常量定义，方便修改 ---
const PROG_ID: &str = "BMCBL.Asset";
const APP_NAME: &str = "Minecraft Bedrock Asset";
const EXTENSIONS: &[&str] = &[".mcpack", ".mcworld", ".mcaddon", ".mctemplate"];
// 注册表中的自定义标识键，用于检测是否需要更新关联
const REG_APP_PATH_KEY: &str = "AppPath";
const REG_VERSION_KEY: &str = "AssocVersion";
const ASSOC_VERSION: u32 = 1; // 关联版本号，格式变更时可递增强制刷新

/// 注册文件关联 (仅在 Windows 下有效)
/// 包含检查机制，仅在路径变更、版本变更或未注册时执行写入
pub fn register_file_associations() {
    #[cfg(target_os = "windows")]
    {
        if let Err(e) = register_associations_safe() {
            // 记录错误但不崩溃，文件关联失败不应影响主程序运行
            error!("Failed to register file associations: {:?}", e);
        }
    }
}

/// 规范化路径字符串用于比较（统一小写、去除首尾空格，Windows 路径不区分大小写）
#[cfg(target_os = "windows")]
fn normalize_path_for_compare(path: &str) -> String {
    path.trim().to_lowercase()
}

#[cfg(target_os = "windows")]
fn register_associations_safe() -> std::io::Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    // 目标路径：HKCU\Software\Classes
    let classes = hkcu.open_subkey("Software\\Classes")?;

    // 1. 获取当前 EXE 的绝对路径（规范化）
    let exe_path = env::current_exe()?;
    let exe_path_str = exe_path.to_str().unwrap_or("");

    if exe_path_str.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Invalid EXE path",
        ));
    }

    let normalized_exe_path = normalize_path_for_compare(exe_path_str);

    // --- 检查是否需要更新 ---
    // 尝试读取现有的 ProgID 配置
    let need_update = match classes.open_subkey(PROG_ID) {
        Ok(key) => {
            // 读取之前保存的 AppPath 和版本号
            let saved_path: String = key.get_value(REG_APP_PATH_KEY).unwrap_or_default();
            let saved_version: u32 = key.get_value(REG_VERSION_KEY).unwrap_or(0);

            // 如果 AppPath 为空，说明是旧版本，需要重新注册
            if saved_path.is_empty() {
                debug!("AppPath is empty (old format), need to register");
                true
            } else {
                // 规范化后比较路径
                let saved_path_normalized = normalize_path_for_compare(&saved_path);

                // 路径变更 或 版本变更 都需要重新注册
                let path_changed = saved_path_normalized != normalized_exe_path;
                let version_changed = saved_version != ASSOC_VERSION;

                if path_changed {
                    debug!("App path changed: '{}' -> '{}'", saved_path, exe_path_str);
                }
                if version_changed {
                    debug!(
                        "Assoc version changed: {} -> {}",
                        saved_version, ASSOC_VERSION
                    );
                }

                path_changed || version_changed
            }
        }
        Err(_) => {
            debug!("ProgID not found, need to register");
            true // ProgID 不存在，需要注册
        }
    };

    // 如果不需要更新，直接返回 Ok，不修改注册表
    if !need_update {
        debug!("File associations are up-to-date. Skipping registration.");
        return Ok(());
    }

    info!("Registering/Updating file associations for BMCBL...");

    // --- 执行注册 ---

    // 构造命令字符串
    let open_cmd = format!("\"{}\" --import-file \"%1\"", exe_path_str);
    let icon_str = format!("\"{}\",0", exe_path_str);

    // 2. 创建/更新 ProgID
    // HKCU\Software\Classes\BMCBL.Asset
    let (prog_key, _) = classes.create_subkey(PROG_ID)?;
    prog_key.set_value("", &APP_NAME)?; // 默认值：文件类型描述

    // [关键] 写入当前 EXE 路径作为标识，供下次检查使用
    prog_key.set_value(REG_APP_PATH_KEY, &exe_path_str)?;
    // 写入版本号
    prog_key.set_value(REG_VERSION_KEY, &ASSOC_VERSION)?;

    // 设置默认图标
    let (icon_key, _) = prog_key.create_subkey("DefaultIcon")?;
    icon_key.set_value("", &icon_str)?;

    // 设置打开命令
    let (shell_key, _) = prog_key.create_subkey("shell")?;
    let (open_key, _) = shell_key.create_subkey("open")?;
    let (cmd_key, _) = open_key.create_subkey("command")?;
    cmd_key.set_value("", &open_cmd)?;

    // 3. 关联后缀名
    for ext in EXTENSIONS {
        // 创建/打开后缀键: HKCU\Software\Classes\.mcpack
        // 注意：这里可能会覆盖其他程序的关联。
        // 如果想更安全，可以先检查该后缀是否已有 user choice，但这在 HKCU 下通常直接覆盖即可。
        let (ext_key, _) = classes.create_subkey(ext)?;

        // 将其默认值指向我们的 ProgID
        if let Err(e) = ext_key.set_value("", &PROG_ID) {
            warn!("Failed to set association for {}: {:?}", ext, e);
            continue;
        }
    }

    // 4. 通知系统刷新
    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }

    info!("File associations registered successfully.");
    Ok(())
}
