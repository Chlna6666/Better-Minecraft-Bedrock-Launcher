use crate::commands::{close_launcher_window, minimize_launcher_window};
use crate::config::config::read_config;
use crate::core::inject::inject::{find_pid, inject};
use crate::core::minecraft::appx::register::register_appx_package_async;
use crate::core::minecraft::appx::remove::remove_package;
use crate::core::minecraft::appx::utils::{get_manifest_identity, get_package_info};
use crate::core::minecraft::launcher::launch_uwp;
use crate::core::minecraft::mouse_lock::start_window_monitor;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::time::sleep;
use tracing::{debug, error, info};
use windows::core::{w, BOOL, PCWSTR, PWSTR};
use serde_json::json;

/// 将 manifest 中的 Identity name 映射到常见的 AUMID（示例）
pub fn identity_to_aumid(identity: &str) -> Option<String> {
    Some(match identity {
        "Microsoft.MinecraftUWP" => {
            "Microsoft.MinecraftUWP_8wekyb3d8bbwe!App".to_string()
        }
        "Microsoft.MinecraftWindowsBeta" => {
            "Microsoft.MinecraftWindowsBeta_8wekyb3d8bbwe!App".to_string()
        }
        "Microsoft.MinecraftEducationEdition" => {
            "Microsoft.MinecraftEducationEdition_8wekyb3d8bbwe!Microsoft.MinecraftEducationEdition".to_string()
        }
        "Microsoft.MinecraftEducationPreview" => {
            "Microsoft.MinecraftEducationPreview_8wekyb3d8bbwe!Microsoft.MinecraftEducationEdition".to_string()
        }
        _ => return None,
    })
}

/// 向前端发送启动进度事件（事件名：`launch-progress`）
/// payload: { stage, status, message?, code? }
fn emit_launch(app: &AppHandle, stage: &str, status: &str, message: Option<String>, code: Option<String>) {
    let payload = json!({
        "stage": stage,
        "status": status,
        "message": message,
        "code": code,
    });
    // 忽略发送错误（例如窗口已关闭）
    let _ = app.emit("launch-progress", payload);
}

/// 主流程：比较版本 ->（必要时）卸载旧包 -> 注册新包 -> 启动
///
/// `app` 用于向前端发送事件（emit_launch）。
pub async fn register_and_start(
    package_folder: &str,
    auto_start: bool,
    app: &AppHandle,
) -> Result<Option<u32>, String> {
    emit_launch(app, "start", "info", Some(format!("准备注册与启动：{}", package_folder)), None);

    let (identity_name, identity_version) = match get_manifest_identity(package_folder).await {
        Ok(iv) => {
            emit_launch(app, "manifest", "ok", Some(format!("解析清单: {} v{}", iv.0, iv.1)), None);
            iv
        }
        Err(e) => {
            let msg = format!("读取清单 Identity 失败: {}", e);
            error!("{}", msg);
            emit_launch(app, "manifest", "error", Some(msg.clone()), None);
            return Err(msg);
        }
    };

    let aumid = match identity_to_aumid(&identity_name) {
        Some(id) => id,
        None => {
            let msg = format!("未识别的 Identity Name: {}", identity_name);
            error!("{}", msg);
            emit_launch(app, "manifest", "error", Some(msg.clone()), None);
            return Err(msg);
        }
    };
    info!("目标包 AUMID = {}", aumid);
    emit_launch(app, "lookup", "info", Some(format!("目标包 AUMID = {}", aumid)), None);

    match get_package_info(&aumid) {
        Ok(Some((installed_version, _, package_full_name))) => {
            debug!("已安装包版本: {}", installed_version);
            emit_launch(app, "lookup", "ok", Some(format!("已安装版本: {}", installed_version)), None);

            if installed_version == identity_version {
                info!("版本一致，直接启动");
                emit_launch(app, "lookup", "info", Some("版本一致，准备启动".into()), None);
                if auto_start {
                    emit_launch(app, "launch", "info", Some("开始启动已安装包".into()), None);
                    return match launch_uwp(&identity_name) {
                        Ok(pid) => {
                            emit_launch(app, "launch", "ok", Some(format!("启动成功, PID: {:?}", pid)), None);
                            Ok(pid)
                        },
                        Err(e) => {
                            let msg = format!("启动失败: {}", e);
                            error!("{}", msg);
                            emit_launch(app, "launch", "error", Some(msg.clone()), None);
                            Err(msg)
                        }
                    };
                }
                return Ok(None);
            } else {
                info!(
                    "版本不一致：已安装 v{}，清单 v{}，将重新注册",
                    installed_version, identity_version
                );
                emit_launch(app, "lookup", "info", Some(format!("版本不一致，卸载旧包 {}", package_full_name)), None);
                // 尝试卸载旧包（异步），并通知前端
                remove_package(&package_full_name).await;
                emit_launch(app, "remove", "info", Some(format!("已请求卸载 {}", package_full_name)), None);
            }
        }
        Ok(None) => {
            info!("系统中未找到 AUMID={} 的包，准备注册", aumid);
            emit_launch(app, "lookup", "ok", Some(format!("系统中未找到 AUMID {}, 将注册", aumid)), None);
        }
        Err(e) => {
            let msg = format!("查询已安装包信息失败: {:?}", e);
            error!("{}", msg);
            // emit_launch(app, "lookup", "error", Some(msg.clone()), None);
            // 继续尝试注册
        }
    }

    // 调用注册
    emit_launch(app, "register", "info", Some(format!("开始注册 APPX：{}", package_folder)), None);
    match register_appx_package_async(package_folder).await {
        Ok(_) => {
            info!("Appx 包注册成功");
            emit_launch(app, "register", "ok", Some("Appx 注册成功".into()), None);
        }
        Err(e) => {
            // windows 错误通常包含 HRESULT 与 message
            let err_msg = format!("{:?}", e);
            error!("注册 Appx 包失败: {:?}", e);
            // 尝试提取 HRESULT
            let code = err_msg
                .split_whitespace()
                .find(|s| s.starts_with("HRESULT("))
                .map(|s| s.to_string());
            emit_launch(app, "register", "error", Some(err_msg.clone()), code);
            let msg = format!("注册 Appx 包失败: {:?}", e);
            return Err(msg);
        }
    }

    if auto_start {
        info!("注册完成，自动启动: {}", identity_name);
        emit_launch(app, "launch", "info", Some(format!("注册完成，自动启动: {}", identity_name)), None);
        return match launch_uwp(&identity_name) {
            Ok(pid) => {
                emit_launch(app, "launch", "ok", Some(format!("启动成功, PID: {:?}", pid)), None);
                Ok(pid)
            }
            Err(e) => {
                let msg = format!("启动失败: {}", e);
                error!("{}", msg);
                emit_launch(app, "launch", "error", Some(msg.clone()), None);
                Err(msg)
            }
        };
    }

    Ok(None)
}

#[tauri::command]
pub async fn launch_appx(
    app: AppHandle,
    file_name: String,
    auto_start: bool,
) -> Result<(), String> {
    let versions_root = Path::new("./BMCBL/versions");
    let package_folder: PathBuf = versions_root.join(&file_name);

    if !package_folder.exists() {
        let msg = format!("版本路径不存在: {}", package_folder.display());
        emit_launch(&app, "start", "error", Some(msg.clone()), None);
        return Err(msg);
    }
    if !package_folder.is_dir() {
        let msg = format!("{} 不是一个目录，请检查", package_folder.display());
        emit_launch(&app, "start", "error", Some(msg.clone()), None);
        return Err(msg);
    }

    let package_folder_str = package_folder
        .to_str()
        .ok_or_else(|| format!("路径转换为字符串失败: {}", package_folder.display()))?
        .to_string();

    let config = read_config().map_err(|e| e.to_string())?;
    let game_cfg = &config.game;

    emit_launch(&app, "start", "info", Some(format!("准备启动版本: {}", file_name)), None);

    // 启动并获取 PID（注意：现在 register_and_start 接受 app 引用用于推送事件）
    let maybe_pid = register_and_start(&package_folder_str, auto_start, &app).await?;


    // 注入 DLL
    match maybe_pid {
        Some(pid) => {
            if game_cfg.inject_on_launch {
                // 等待注入延迟（毫秒）
                if game_cfg.inject_delay > 0 {
                    emit_launch(&app, "inject", "info", Some(format!("注入延迟 {} ms", game_cfg.inject_delay)), None);
                    sleep(Duration::from_millis(game_cfg.inject_delay as u64)).await;
                }
                emit_launch(&app, "inject", "info", Some("注入已启用，开始注入 DLL".into()), None);
                if let Err(e) = inject(&package_folder, None, Some(pid)) {
                    let msg = format!("注入 DLL 失败: {}", e);
                    emit_launch(&app, "inject", "error", Some(msg.clone()), None);
                    return Err(msg);
                } else {
                    emit_launch(&app, "inject", "ok", Some("注入成功".into()), None);
                }
            }

            if game_cfg.lock_mouse_on_launch {
                emit_launch(&app, "input", "info", Some("启用鼠标锁定监控".into()), None);
                start_window_monitor("Minecraft", &game_cfg.unlock_mouse_hotkey, game_cfg.reduce_pixels);
            }
            // 等待一段时间确保监控线程启动
            sleep(Duration::from_secs(2)).await;
        }
        None => {
            emit_launch(&app, "inject", "info", Some("未获得 PID，尝试对可执行名注入（如启用）".into()), None);
            if game_cfg.inject_on_launch {
                if let Err(e) = inject(&package_folder, Some("Minecraft.Windows.exe"), None) {
                    let msg = format!("注入 DLL 失败: {}", e);
                    emit_launch(&app, "inject", "error", Some(msg.clone()), None);
                    return Err(msg);
                } else {
                    emit_launch(&app, "inject", "ok", Some("注入成功（按进程名）".into()), None);
                }
            }
        }
    }

    // 启动器窗口可见性控制（执行前后都发送事件）
    match game_cfg.launcher_visibility.as_str() {
        "minimize" => {
            emit_launch(&app, "launcher_visibility", "info", Some("最小化启动器窗口".into()), None);
            minimize_launcher_window(&app);
            emit_launch(&app, "launcher_visibility", "ok", Some("已最小化启动器窗口".into()), None);
        }
        "close" => {
            emit_launch(&app, "launcher_visibility", "info", Some("关闭启动器窗口".into()), None);
            close_launcher_window(&app);
            emit_launch(&app, "launcher_visibility", "ok", Some("已关闭启动器窗口".into()), None);
        }
        _ => {} // keep 默认不处理
    }

    emit_launch(&app, "done", "ok", Some("启动流程完成".into()), None);

    Ok(())
}
