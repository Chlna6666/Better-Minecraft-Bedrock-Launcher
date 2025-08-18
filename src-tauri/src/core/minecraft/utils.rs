use std::sync::Arc;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

/// 计算速度字符串（单位自适应：B/s、KB/s、MB/s）
/// processed: 已处理的字节数（下载、解压等）
/// elapsed: 已用时间（秒）
pub fn format_speed(processed: u64, elapsed: f64) -> String {
    let speed = if elapsed > 0.0 { processed as f64 / elapsed } else { 0.0 };
    match speed {
        s if s >= 1e6 => format!("{:.2} MB/s", s / 1e6),
        s if s >= 1e3 => format!("{:.2} KB/s", s / 1e3),
        s             => format!("{:.2} B/s", s),
    }
}

/// 计算 ETA（剩余时间）字符串，格式为 HH:MM:SS，无法计算时返回 "unknown"
/// total: 总字节数
/// processed: 已处理的字节数
/// elapsed: 已耗时（秒）
pub fn format_eta(total: Option<u64>, processed: u64, elapsed: f64) -> String {
    if let (Some(total), true) = (total, elapsed > 0.0) {
        if processed > 0 && processed < total {
            let speed = processed as f64 / elapsed;
            if speed > 0.0 {
                let remaining = total - processed;
                let eta_secs = (remaining as f64 / speed).max(0.0);
                let hours = (eta_secs / 3600.0).floor() as u64;
                let minutes = ((eta_secs % 3600.0) / 60.0).floor() as u64;
                let seconds = (eta_secs % 60.0).floor() as u64;
                return format!("{:02}:{:02}:{:02}", hours, minutes, seconds);
            }
        }
    }
    "unknown".to_string()
}


/// 通用的进度推送器：下载或解压都可以用
pub async fn emit_progress(
    app: &AppHandle,
    processed: u64,
    total_opt: Option<u64>,
    speed: Option<&str>,
    eta: Option<&str>,
    extra: Option<serde_json::Value>, // 可附加任意额外字段，例如 { "threads": 4 }
    
) {
    // 1. 计算 total / percent / status
    let total = total_opt.unwrap_or(processed);
    let percent = if total > 0 {
        (processed as f64 / total as f64 * 100.0).min(100.0)
    } else {
        0.0
    };

    // 2. 构造基础 JSON
    let mut payload = json!({
        "processed": processed,
        "total": total,
        "percent": percent,
    });

    // 3. speed / eta 字段（下载时提供，解压可传 None）
    if let Some(s) = speed {
        payload["speed"] = json!(s);
    }
    if let Some(e) = eta {
        payload["eta"] = json!(e);
    }

    // 4. 附加 extra
    if let Some(extra_obj) = extra {
        if let Some(map) = extra_obj.as_object() {
            for (k, v) in map {
                payload[k] = v.clone();
            }
        }
    }

    // 5. 如果下载或解压刚好完成，标记 status
    if let Some(t) = total_opt {
        if processed >= t {
            payload["status"] = json!("completed");
        }
    }

    // 6. 用 Arc + 单次 tokio::spawn 推送到所有窗口
    let arc_payload = Arc::new(payload);
    let windows = app.windows();
    tokio::spawn(async move {
        for window in windows.values() {
            let p = Arc::clone(&arc_payload);
            let _ = window.emit("install-progress", p.clone());
        }
    });
}