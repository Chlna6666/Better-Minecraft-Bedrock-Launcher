use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::windows::prelude::OsStrExt;
use crate::commands::{close_launcher_window, minimize_launcher_window};
use crate::config::config::read_config;
use crate::core::inject::inject::{fast_inject, find_pid};
use crate::core::minecraft::appx::register::register_appx_package_async;
use crate::core::minecraft::appx::remove::remove_package;
use crate::core::minecraft::appx::utils::{get_manifest_identity, get_package_info};
use crate::core::minecraft::launcher::launch_uwp;
use crate::core::minecraft::mouse_lock::start_window_monitor;
use std::path::{Path, PathBuf};
use std::time::Duration;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::time::sleep;
use tracing::{debug, error, info};
use serde_json::json;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::sync::{Notify, Semaphore};
use std::sync::Arc;
use tokio::task::JoinHandle;
use tokio::fs;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use crate::core::minecraft::launcher::start::launch_win32;
use crate::core::minecraft::uwp_minimize_fix::enable_debugging_for_package;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DllConfig {
    pub enabled: bool,
    pub delay: u64,
}
/// 注入配置文件结构：映射文件名 -> 延迟（毫秒）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InjectConfig {
    pub files: HashMap<String, DllConfig>,
}

/// 简单解析：每段保留为整数（不要把长段再拆）
fn parse_version_to_vec_simple(v: &str) -> Vec<u64> {
    v.split(|c| c == '.' || c == '-' || c == '+')
        .map(|seg| {
            let digits: String = seg.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse::<u64>().unwrap_or(0)
        })
        .collect()
}

fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let va = parse_version_to_vec_simple(a);
    let vb = parse_version_to_vec_simple(b);
    let n = std::cmp::max(va.len(), vb.len());
    for i in 0..n {
        let ai = *va.get(i).unwrap_or(&0);
        let bi = *vb.get(i).unwrap_or(&0);
        match ai.cmp(&bi) {
            std::cmp::Ordering::Equal => continue,
            non_eq => return non_eq,
        }
    }
    std::cmp::Ordering::Equal
}

/// 新的判定函数：优先处理你观测到的特殊情况（1.21.12201.* 为 GDK），其余回退到阈值比较
fn is_win32_version(version: &str) -> bool {
    // 先把版本拆为数字段向量
    let v = parse_version_to_vec_simple(version);

    // 特殊规则：对于 1.21 系列，如果第三段 >= 12201，则视为 GDK/Win32（按你观测）
    if v.len() >= 3 {
        if v[0] == 1 && v[1] == 21 && v[2] >= 12201 {
            return true;
        }
    }

    // 否则按常规阈值比较（可以调整阈值）
    const THRESHOLD: &str = "1.21.12000.21";
    compare_versions(version, THRESHOLD) != std::cmp::Ordering::Less
}

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



/// 读取或自动创建 mods 下的注入配置文件（inject_config.json）
/// 返回一个映射：dll 文件名 -> DllConfig （只保留 enabled = true 的）
pub async fn load_or_create_inject_config(
    mods_dir: &Path
) -> anyhow::Result<Vec<(PathBuf, u64, Vec<u16>)>> {
    let mut config = InjectConfig::default();

    if !mods_dir.exists() {
        fs::create_dir_all(mods_dir)
            .await
            .with_context(|| format!("无法创建 mods 目录：{}", mods_dir.display()))?;
        debug!("已创建 mods 目录：{}", mods_dir.display());
    }

    // 收集 DLL 文件名
    let mut dll_names: Vec<String> = Vec::new();
    let mut rd = fs::read_dir(mods_dir)
        .await
        .with_context(|| format!("读取 mods 目录失败：{}", mods_dir.display()))?;
    while let Some(entry) = rd.next_entry().await.with_context(|| "遍历 mods 目录时出错")? {
        let p = entry.path();
        if p.extension()
            .and_then(|ext| ext.to_str())
            .map_or(false, |ext| ext.eq_ignore_ascii_case("dll"))
        {
            if let Some(name) = p.file_name().and_then(|n| n.to_str().map(|s| s.to_string())) {
                dll_names.push(name);
            }
        }
    }

    let cfg_path = mods_dir.join("inject_config.json");

    if fs::metadata(&cfg_path).await.is_ok() {
        let raw = fs::read_to_string(&cfg_path).await?;
        match serde_json::from_str::<InjectConfig>(&raw) {
            Ok(mut c) => {
                let mut changed = false;
                for dll in dll_names.iter() {
                    if !c.files.contains_key(dll) {
                        c.files.insert(
                            dll.clone(),
                            DllConfig { enabled: true, delay: 0 },
                        );
                        changed = true;
                    }
                }
                if changed {
                    let pretty = serde_json::to_string_pretty(&c)?;
                    let mut f = File::create(&cfg_path).await?;
                    f.write_all(pretty.as_bytes()).await?;
                }
                config = c;
            }
            Err(e) => {
                let bak = mods_dir.join("inject_config.json.bak");
                let _ = fs::rename(&cfg_path, &bak).await;
                debug!("注入配置解析失败，已备份到 {}，错误: {:?}", bak.display(), e);
            }
        }
    }

    if config.files.is_empty() {
        for dll in dll_names.iter() {
            config.files.insert(dll.clone(), DllConfig { enabled: true, delay: 0 });
        }
        let pretty = serde_json::to_string_pretty(&config)?;
        let mut f = File::create(&cfg_path).await?;
        f.write_all(pretty.as_bytes()).await?;
        debug!("已创建默认注入配置: {}", cfg_path.display());
    }

    // 预先规范化路径并构建 wide-string，返回只包含 enabled 且文件存在的条目
    let mut result: Vec<(PathBuf, u64, Vec<u16>)> = Vec::new();
    for (name, cfg) in config.files.into_iter() {
        if !cfg.enabled {
            continue;
        }
        let path = mods_dir.join(&name);
        if !path.exists() {
            debug!("注入配置中 DLL 不存在，跳过: {}", path.display());
            continue;
        }

        // 使用 tokio 的 canonicalize（异步）
        match tokio::fs::canonicalize(&path).await {
            Ok(abs) => {
                // 预先做 UTF-16 转换（LoadLibraryW 期待的是 wide null-terminated string）
                let wide: Vec<u16> = OsStr::new(abs.to_str().unwrap_or_default())
                    .encode_wide()
                    .chain(Some(0))
                    .collect();
                result.push((abs, cfg.delay, wide));
            }
            Err(e) => {
                debug!("canonicalize 失败，跳过 {}: {:?}", path.display(), e);
                continue;
            }
        }
    }

    Ok(result)
}

// ---------- prepare_injection_tasks：接收预加载好的 wide-string，任务里直接调用 fast_inject ----------
// schedule: Vec<(PathBuf absolute, delay_ms, wide_string)>
// 调试丰富的 prepare_injection_tasks（直接替换）
fn prepare_injection_tasks(
    schedule: Vec<(PathBuf, u64, Vec<u16>)>,
    max_concurrency: usize,
) -> (Arc<Notify>, Arc<AtomicU32>, Vec<JoinHandle<()>>) {
    let notify = Arc::new(Notify::new());
    let pid_atomic = Arc::new(AtomicU32::new(0));
    let sem = Arc::new(Semaphore::new(max_concurrency));
    debug!("prepare_injection_tasks: schedule.len={} max_concurrency={}", schedule.len(), max_concurrency);
    let mut handles = Vec::new();

    for (dll_path, delay_ms, wide) in schedule.into_iter() {
        debug!("spawn task for {} (delay {}ms)", dll_path.display(), delay_ms);
        let notify_cloned = notify.clone();
        let pid_cloned = pid_atomic.clone();
        let sem_cloned = sem.clone();
        let wide_clone = wide.clone();
        let dll_display = dll_path.clone();

        let handle = tokio::spawn(async move {
            debug!("[{}] task created, will wait for pid...", dll_display.display());

            // ---- robust wait: 如果 pid 已经被写入就直接继续；否则循环等待 notify ----
            loop {
                let pid_now = pid_cloned.load(Ordering::SeqCst);
                if pid_now != 0 {
                    debug!("[{}] pid already set = {}, skip waiting", dll_display.display(), pid_now);
                    break;
                }
                debug!("[{}] pid == 0, awaiting notify...", dll_display.display());
                notify_cloned.notified().await;
                // loop will re-check pid; protects against missed notify/race
            }

            let pid = pid_cloned.load(Ordering::SeqCst);
            debug!("[{}] proceeding with pid = {}", dll_display.display(), pid);
            if pid == 0 {
                debug!("[{}] nothing to do (pid still 0), return", dll_display.display());
                return;
            }

            // delay (不占用 semaphore)
            if delay_ms > 0 {
                debug!("[{}] sleeping {} ms before acquire", dll_display.display(), delay_ms);
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                debug!("[{}] sleep done", dll_display.display());
            }

            // acquire semaphore
            debug!("[{}] acquiring semaphore...", dll_display.display());
            let permit = sem_cloned.acquire().await;
            if permit.is_err() {
                error!("[{}] semaphore acquire failed: {:?}", dll_display.display(), permit);
                return;
            }
            let permit = permit.unwrap();
            debug!("[{}] acquired semaphore, starting injection (pid={})", dll_display.display(), pid);

            // 调用 fast_inject 并记录返回
            match fast_inject(pid, dll_display.clone(), wide_clone.clone()).await {
                Ok(_) => debug!("[{}] 注入成功", dll_display.display()),
                Err(e) => error!("[{}] 注入失败: {:?}", dll_display.display(), e),
            }

            drop(permit);
            debug!("[{}] 注入任务结束", dll_display.display());
        });

        handles.push(handle);
    }

    (notify, pid_atomic, handles)
}




/// 主流程：比较版本 ->（必要时）卸载旧包 -> 注册新包 -> 启动
///
/// `app` 用于向前端发送事件（emit_launch）。
pub async fn register_and_start(
    package_folder: &str,
    auto_start: bool,
    app: &AppHandle,
) -> Result<Option<u32>, String> {
    let config = read_config().map_err(|e| e.to_string())?;
    let game_cfg = &config.game;
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

    // 判断是否为 Win32/GDK 版本（若为 true 则跳过 Appx 注册流程）
    let is_win32 = is_win32_version(&identity_version);
    if is_win32 {
        emit_launch(app, "manifest", "info", Some(format!("检测到 Win32/GDK 版本: {}，将跳过 Appx 注册", identity_version)), None);
    }

    // 只有在非 Win32 时，才需要使用 aumid / get_package_info / register_appx
    let mut need_register = true;
    let mut package_full_name_opt: Option<String> = None;
    let mut aumid_opt: Option<String> = None;

    if !is_win32 {
        // 计算 AUMID（仅用于 UWP/Appx 流程）
        let aumid = match identity_to_aumid(&identity_name) {
            Some(id) => id,
            None => {
                let msg = format!("未识别的 Identity Name: {}", identity_name);
                error!("{}", msg);
                emit_launch(app, "manifest", "error", Some(msg.clone()), None);
                return Err(msg);
            }
        };
        aumid_opt = Some(aumid.clone());
        info!("目标包 AUMID = {}", aumid);
        emit_launch(app, "lookup", "info", Some(format!("目标包 AUMID = {}", aumid)), None);

        match get_package_info(&aumid) {
            Ok(Some((installed_version, _family_name, package_full_name))) => {
                debug!("已安装包版本: {}", installed_version);
                emit_launch(app, "lookup", "ok", Some(format!("已安装版本: {}", installed_version)), None);

                package_full_name_opt = Some(package_full_name.clone());

                if installed_version == identity_version {
                    info!("版本一致，无需重新注册");
                    emit_launch(app, "lookup", "info", Some("版本一致，准备启动".into()), None);
                    need_register = false;
                } else {
                    info!("版本不一致，卸载旧包 {}", package_full_name);
                    emit_launch(app, "lookup", "info", Some(format!("版本不一致，卸载旧包 {}", package_full_name)), None);
                    let _ = remove_package(&package_full_name).await;
                    emit_launch(app, "remove", "info", Some(format!("已请求卸载 {}", package_full_name)), None);
                }
            }
            Ok(None) => {
                info!("系统中未找到包，准备注册");
                emit_launch(app, "lookup", "ok", Some("未找到安装包，将注册".into()), None);
            }
            Err(e) => {
                error!("查询已安装包信息失败: {:?}", e);
                // 继续尝试注册
            }
        }
    } else {
        // Win32: 不需要注册 Appx
        need_register = false;
    }

    // 执行注册（仅在 need_register 且 非 Win32 时）
    if need_register {
        // aumid_opt 应该存在
        let aumid_for_log = aumid_opt.clone().unwrap_or_else(|| "<unknown aumid>".to_string());
        emit_launch(app, "register", "info", Some(format!("开始注册 APPX：{} (aumid={})", package_folder, aumid_for_log)), None);
        match register_appx_package_async(package_folder).await {
            Ok(_) => {
                info!("Appx 包注册成功");
                emit_launch(app, "register", "ok", Some("Appx 注册成功".into()), None);
            }
            Err(e) => {
                let err_msg = format!("{:?}", e);
                error!("注册 Appx 包失败: {:?}", e);
                let code = err_msg
                    .split_whitespace()
                    .find(|s| s.starts_with("HRESULT("))
                    .map(|s| s.to_string());
                emit_launch(app, "register", "error", Some(err_msg.clone()), code);
                return Err(format!("注册 Appx 包失败: {:?}", e));
            }
        }
    }

    // 剩余逻辑（mods_dir、注入准备等）保持不变...
    let mods_dir = Path::new(package_folder).join("mods");

    // ---------------------------------------------
    // 注入准备：只有在 auto_start && game_cfg.inject_on_launch 为 true 时才读取配置并准备注入任务
    // ---------------------------------------------
    let mut inject_handles: Vec<JoinHandle<()>> = Vec::new();
    let mut inject_notify_opt: Option<Arc<Notify>> = None;
    let mut inject_pid_atomic_opt: Option<Arc<AtomicU32>> = None;

    if auto_start && game_cfg.inject_on_launch {
        // 读取/创建注入配置
        match load_or_create_inject_config(&mods_dir).await {
            Ok(schedule) => {
                // 仅当 schedule 非空才准备注入任务
                if !schedule.is_empty() {
                    let (notify, pid_atomic, handles) = prepare_injection_tasks(schedule, 4);
                    inject_notify_opt = Some(notify);
                    inject_pid_atomic_opt = Some(pid_atomic);
                    inject_handles = handles;
                    emit_launch(app, "inject", "info", Some("已准备注入任务".into()), None);
                } else {
                    emit_launch(app, "inject", "info", Some("注入列表为空，跳过注入准备".into()), None);
                }
            }
            Err(e) => {
                let msg = format!("读取或创建注入配置失败: {:?}", e);
                error!("{}", msg);
                emit_launch(&app, "inject", "error", Some(msg.clone()), None);
                // 失败时不阻塞启动，继续启动流程（但不会注入）
            }
        }
    } else {
        // 未开启注入或非自动启动：不读取注入配置
        debug!("inject_on_launch 未启用或非 auto_start，跳过注入准备");
    }

    // ==== 统一启动流程 ====
    if auto_start {
        // 如果是 Win32 跳过获取 package_full_name（调试相关启用也与 Appx 无关）
        let package_full_name = if is_win32 {
            // 对 Win32，我们不需要 package_full_name 用于启用调试模式（uwp_minimize_fix 仅用于 UWP）
            String::new()
        } else if let Some(full_name) = package_full_name_opt {
            full_name
        } else {
            // 获取最新注册包的 FullName（仅在非 Win32 时有效）
            let aumid = aumid_opt.as_ref().map(|s| s.as_str()).unwrap_or("");
            match get_package_info(aumid) {
                Ok(Some((_ver, _fam, full_name))) => full_name,
                _ => {
                    let msg = "无法获取安装包 FullName，无法启用调试".to_string();
                    error!("{}", msg);
                    emit_launch(app, "launch", "error", Some(msg.clone()), None);
                    return Err(msg);
                }
            }
        };

        if !is_win32 && game_cfg.uwp_minimize_fix {
            // 启用调试(修复UWP最小化停滞)
            match enable_debugging_for_package(&package_full_name) {
                Ok(_) => info!("已启用调试模式: {}", package_full_name),
                Err(e) => error!("启用调试模式失败: {:?}", e),
            }
        }

        emit_launch(app, "launch", "info", Some(format!("开始启动: {}", identity_name)), None);

        // 如果是 Win32：使用 launch_win32（跳过 launch_uwp）
        if is_win32 {
            match launch_win32(package_folder) {
                Ok(pid_opt) => {
                    emit_launch(app, "launch", "ok", Some(format!("Win32 启动成功, PID: {:?}", pid_opt)), None);

                    if let Some(pid) = pid_opt {
                        if let Some(pid_atomic) = &inject_pid_atomic_opt {
                            pid_atomic.store(pid, Ordering::SeqCst);
                            debug!("已设置 PID: {}", pid);
                        }
                        if let Some(notify) = &inject_notify_opt {
                            notify.notify_waiters();
                        }
                    } else {
                        // 如果无法直接拿到 pid，尝试通过 find_pid 查找常见进程名
                        debug!("Win32 无直接 PID，尝试通过 find_pid 查找");
                        match find_pid("Minecraft.Windows.exe") {
                            Ok(pid) => {
                                if let Some(pid_atomic) = &inject_pid_atomic_opt {
                                    pid_atomic.store(pid, Ordering::SeqCst);
                                }
                                if let Some(notify) = &inject_notify_opt {
                                    notify.notify_waiters();
                                }
                            }
                            Err(e) => {
                                error!("通过 find_pid 查找 PID 失败: {:?}", e);
                                return Err("无法获取进程 PID，启动失败".to_string());
                            }
                        }
                    }

                    // 等待注入任务（若存在）
                    if !inject_handles.is_empty() {
                        let join_all = futures::future::join_all(inject_handles);
                        match tokio::time::timeout(Duration::from_secs(30), join_all).await {
                            Ok(results) => {
                                for (idx, r) in results.into_iter().enumerate() {
                                    match r {
                                        Ok(_) => debug!("inject handle[{}] finished ok", idx),
                                        Err(join_err) => {
                                            error!("inject handle[{}] join error: {:?}", idx, join_err);
                                            if join_err.is_panic() {
                                                error!(" -> inject handle[{}] panicked", idx);
                                            }
                                        }
                                    }
                                }
                                debug!("所有注入任务已返回（在超时内）");
                            }
                            Err(_) => {
                                debug!("等待注入任务超时（30s），继续后续流程");
                            }
                        }
                    }

                    return Ok(pid_opt)
                }
                Err(e) => {
                    let msg = format!("启动 Win32 可执行失败: {:?}", e);
                    error!("{}", msg);
                    emit_launch(app, "launch", "error", Some(msg.clone()), None);
                    return  Err(msg)
                }
            }
        } else {
            // 原有 UWP 启动路径（保持不变）
            return match launch_uwp(&identity_name) {
                Ok(pid_opt) => {
                    emit_launch(app, "launch", "ok", Some(format!("启动成功, PID: {:?}", pid_opt)), None);

                    // **立刻把 PID 写入 atomic 并通知所有等待任务开始注入（如果注入已准备好）**
                    if let Some(pid) = pid_opt {
                        // 如果成功获取到 PID，则直接进行注入操作
                        if let Some(pid_atomic) = &inject_pid_atomic_opt {
                            pid_atomic.store(pid, Ordering::SeqCst);
                            debug!("已设置 PID: {}", pid);
                        }
                        if let Some(notify) = &inject_notify_opt {
                            // 立即通知所有等待的注入任务开始注入
                            notify.notify_waiters();
                        }
                    } else {
                        // 如果没有成功获取 PID，尝试通过 find_pid 查找进程 ID
                        debug!("未能直接获取 PID，尝试通过 find_pid 查找");
                        match find_pid("Minecraft.Windows.exe") {
                            Ok(pid) => {
                                debug!("通过 find_pid 获取到 PID: {}", pid);
                                if let Some(pid_atomic) = &inject_pid_atomic_opt {
                                    pid_atomic.store(pid, Ordering::SeqCst);
                                    debug!("已设置 PID: {}", pid);
                                }
                                if let Some(notify) = &inject_notify_opt {
                                    // 立即通知所有等待的注入任务开始注入
                                    notify.notify_waiters();
                                }
                            }
                            Err(e) => {
                                error!("通过 find_pid 查找 PID 失败: {:?}", e);
                                return Err("无法获取进程 PID，启动失败".to_string());
                            }
                        }
                    }

                    // 如果需要等待注入全部完成且有注入任务，可以等待所有 handles（可设置超时）
                    if !inject_handles.is_empty() {
                        let join_all = futures::future::join_all(inject_handles);
                        match tokio::time::timeout(Duration::from_secs(30), join_all).await {
                            Ok(results) => {
                                for (idx, r) in results.into_iter().enumerate() {
                                    match r {
                                        Ok(_) => debug!("inject handle[{}] finished ok", idx),
                                        Err(join_err) => {
                                            error!("inject handle[{}] join error: {:?}", idx, join_err);
                                            if join_err.is_panic() {
                                                error!(" -> inject handle[{}] panicked", idx);
                                            }
                                        }
                                    }
                                }
                                debug!("所有注入任务已返回（在超时内）");
                            }
                            Err(_) => {
                                debug!("等待注入任务超时（30s），继续后续流程");
                            }
                        }
                    }

                    // 其余行为保持不变（启动监控、窗口隐藏等）
                    Ok(pid_opt)
                }
                Err(e) => {
                    let msg = format!("启动失败: {}", e);
                    error!("{}", msg);
                    emit_launch(app, "launch", "error", Some(msg.clone()), None);
                    Err(msg)
                }
            };
        }
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


    // 注入 DLL：注入逻辑不再从磁盘遍历 mods（注入列表由上面的 inject_schedule 提供）
    match maybe_pid {
        Some(pid) => {
            if game_cfg.lock_mouse_on_launch {
                emit_launch(&app, "input", "info", Some("启用鼠标锁定监控".into()), None);
                start_window_monitor("Minecraft", &game_cfg.unlock_mouse_hotkey, game_cfg.reduce_pixels);
            }
            // 等待一段时间确保监控线程启动
            sleep(Duration::from_secs(2)).await;
        }
        None => {

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
