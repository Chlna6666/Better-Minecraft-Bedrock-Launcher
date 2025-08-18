use once_cell::sync::Lazy;
use std::fs;
use std::fs::File;
use std::io::{BufWriter, Read, Seek, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;
use tauri::AppHandle;
use tokio::sync::mpsc;
use tokio::task;
use tracing::{debug, info};

use zip::ZipArchive;

pub static CANCEL_EXTRACT: Lazy<AtomicBool> = Lazy::new(Default::default);

/// 从阻塞线程汇报的进度消息
struct ProgressMsg {
    extracted: u64,
    total: Option<u64>,
    speed: Option<String>,
    eta: Option<String>,
    stage_meta: serde_json::Value,
}

/// 主接口（保持 async），内部 spawn_blocking 做实际解压
pub async fn extract_zip<R: Read + Seek + Send + 'static>(
    mut archive: ZipArchive<R>,
    destination: &str,
    force_replace: bool,
    app: AppHandle,
) -> Result<crate::core::result::CoreResult<()>, crate::core::result::CoreError> {
    CANCEL_EXTRACT.store(false, Ordering::SeqCst);

    // channel 用于从阻塞线程发送进度
    let (tx, mut rx) = mpsc::channel::<ProgressMsg>(16);
    // 把必要参数克隆到阻塞线程
    let dest = destination.to_string();

    // spawn_blocking 执行阻塞解压（在独立线程）
    let handle = task::spawn_blocking(move || {
        // 1) 先收集索引元数据（单次访问）
        let mut total: u64 = 0;
        let mut entries_meta = Vec::with_capacity(archive.len());
        for i in 0..archive.len() {
            let e = archive.by_index(i).map_err(|z| z)?;
            let size = e.size();
            let name = e.mangled_name();
            let is_dir = e.is_dir();
            entries_meta.push((i, name, size, is_dir));
            total = total.saturating_add(size);
            // 注意：不能把 `e` 保留（它借用 archive），只取元数据即可
        }

        let start = Instant::now();
        let mut extracted: u64 = 0;

        // 2) 逐项解压（在阻塞线程中）
        for (idx, name, size, is_dir) in entries_meta {
            if CANCEL_EXTRACT.load(Ordering::SeqCst) {
                debug!("用户已取消解压（阻塞线程检测）");
                return Ok((extracted, total)); // 返回已提取量以便上层处理
            }

            // 重新打开 entry（这里安全）
            let mut entry = archive.by_index(idx)?;

            let out_path = Path::new(&dest).join(&name);
            if is_dir {
                // make dir and continue
                if let Some(p) = out_path.parent() {
                    fs::create_dir_all(p).ok();
                }
                fs::create_dir_all(&out_path).ok();
                continue;
            }

            // 已存在处理
            if out_path.exists() {
                if force_replace {
                    if out_path.is_dir() {
                        let _ = fs::remove_dir_all(&out_path);
                    } else {
                        let _ = fs::remove_file(&out_path);
                    }
                } else {
                    extracted = extracted.saturating_add(size);
                    // send a progress update (一次性)
                    let elapsed = start.elapsed().as_secs_f64();
                    let speed = crate::core::minecraft::utils::format_speed(extracted, elapsed);
                    let eta = crate::core::minecraft::utils::format_eta(Some(total), extracted, elapsed);
                    let _ = tx.blocking_send(ProgressMsg {
                        extracted,
                        total: Some(total),
                        speed: Some(speed),
                        eta: Some(eta),
                        stage_meta: json!({ "stage": "extracting" }),
                    });
                    continue;
                }
            }

            if let Some(p) = out_path.parent() {
                fs::create_dir_all(p).ok();
            }

            // 使用 BufWriter 减少系统调用
            let f = File::create(&out_path)?;
            let mut writer = BufWriter::new(f);

            // 分片拷贝：手动 loop，便于打断与上报
            let mut buf = [0u8; 64 * 1024]; // 64KiB
            loop {
                if CANCEL_EXTRACT.load(Ordering::SeqCst) {
                    debug!("用户已取消解压（拷贝中检测）");
                    return Ok((extracted, total));
                }
                let bytes_read = match entry.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => n,
                    Err(e) => return Err(e),
                };
                writer.write_all(&buf[..bytes_read])?;
                extracted = extracted.saturating_add(bytes_read as u64);

                // 控制上报频率：这里我们简单按 bytes 阈值或也可按时间阈值
                // 例如：每写 256KiB 通知一次；这里直接每 64KiB 发一次（channel 带缓冲）
                let elapsed = start.elapsed().as_secs_f64();
                let speed = crate::core::minecraft::utils::format_speed(extracted, elapsed);
                let eta = crate::core::minecraft::utils::format_eta(Some(total), extracted, elapsed);
                let _ = tx.blocking_send(ProgressMsg {
                    extracted,
                    total: Some(total),
                    speed: Some(speed),
                    eta: Some(eta),
                    stage_meta: json!({ "stage": "extracting" }),
                });
            }

            // flush writer ensure data written
            writer.flush()?;
        }

        // 解压完成，发送 final progress
        let elapsed = start.elapsed().as_secs_f64();
        let speed = crate::core::minecraft::utils::format_speed(total, elapsed);
        let _ = tx.blocking_send(ProgressMsg {
            extracted: total,
            total: Some(total),
            speed: Some(speed),
            eta: Some("00:00:00".to_string()),
            stage_meta: json!({ "stage": "extracting", "status": "completed" }),
        });

        info!("解压完成，总计 {} bytes, 总耗时 {:.2} 秒", total, start.elapsed().as_secs_f64());
        Ok((total, total))
    });

    // async 端：接收进度并调用 emit_progress
    // 注意：如果需要尽快响应取消，上层可以 set CANCEL_EXTRACT = true
    while let Some(msg) = rx.recv().await {
        let _ = crate::core::minecraft::utils::emit_progress(
            &app,
            msg.extracted,
            msg.total,
            msg.speed.as_deref(),
            msg.eta.as_deref(),
            Some(msg.stage_meta),
        ).await;
    }

    // 等待阻塞任务完成并检查结果
    match handle.await {
        Ok(Ok((extracted, _total))) => {
            if CANCEL_EXTRACT.load(Ordering::SeqCst) {
                return Ok(crate::core::result::CoreResult::Cancelled);
            }
            Ok(crate::core::result::CoreResult::Success(()))
        }
        Ok(Err(e)) => {
            // map zip/io errors to CoreError
            Err(crate::core::result::CoreError::from(e))
        }
        Err(join_err) => {
            Err(crate::core::result::CoreError::Other(format!("join error: {}", join_err)))
        }
    }
}
