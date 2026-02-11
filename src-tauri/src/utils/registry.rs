// src-tauri/src/utils/registry.rs
use std::env;
use std::path::Path;
use winreg::enums::*;
use winreg::RegKey;
use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
use tracing::{info, error, debug, warn};

// --- 常量定义，方便修改 ---
const PROG_ID: &str = "BMCBL.Asset";
const APP_NAME: &str = "Minecraft Bedrock Asset";
const EXTENSIONS: &[&str] = &[".mcpack", ".mcworld", ".mcaddon", ".mctemplate"];
// 注册表中的自定义标识键，用于检测是否需要更新关联
const REG_APP_PATH_KEY: &str = "AppPath";

/// 注册文件关联 (仅在 Windows 下有效)
/// 包含检查机制，仅在路径变更或未注册时执行写入
pub fn register_file_associations() {
    #[cfg(target_os = "windows")]
    {
        if let Err(e) = register_associations_safe() {
            // 记录错误但不崩溃，文件关联失败不应影响主程序运行
            error!("Failed to register file associations: {:?}", e);
        }
    }
}

#[cfg(target_os = "windows")]
fn register_associations_safe() -> std::io::Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    // 目标路径：HKCU\Software\Classes
    let classes = hkcu.open_subkey("Software\\Classes")?;

    // 1. 获取当前 EXE 的绝对路径
    let exe_path = env::current_exe()?;
    let exe_path_str = exe_path.to_str().unwrap_or("");

    if exe_path_str.is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid EXE path"));
    }

    // --- 检查是否需要更新 ---
    // 尝试读取现有的 ProgID 配置
    let need_update = match classes.open_subkey(PROG_ID) {
        Ok(key) => {
            // 读取之前保存的 AppPath
            let saved_path: String = key.get_value(REG_APP_PATH_KEY).unwrap_or_default();
            // 如果路径不一致，说明程序位置变了（或者更新了），需要重新注册
            saved_path != exe_path_str
        }
        Err(_) => true, // ProgID 不存在，需要注册
    };

    if !need_update {
        debug!("File associations are up-to-date. Skipping registration.");
        return Ok(());
    }

    info!("Registering/Updating file associations for BMCBL...");

    // --- 执行注册 ---

    // 构造命令字符串
    let open_cmd = format!("\"{}\" \"%1\"", exe_path_str);
    let icon_str = format!("\"{}\",0", exe_path_str);

    // 2. 创建/更新 ProgID
    // HKCU\Software\Classes\BMCBL.Asset
    let (prog_key, _) = classes.create_subkey(PROG_ID)?;
    prog_key.set_value("", &APP_NAME)?; // 默认值：文件类型描述

    // [关键] 写入当前 EXE 路径作为标识，供下次检查使用
    prog_key.set_value(REG_APP_PATH_KEY, &exe_path_str)?;

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