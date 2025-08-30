// extract_zip.rs
use once_cell::sync::Lazy;
use std::fs;
use std::fs::File;
use std::io::{BufWriter, Read, Seek, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant as StdInstant;

use serde_json::json;
use tokio::sync::mpsc;
use tokio::task;
use tokio::time::{sleep, Duration};
use tracing::{debug, info};
use zip::result::ZipError;
use zip::ZipArchive;

use crate::progress::extract_progress::{report_extract_progress, ExtractProgress};

pub static CANCEL_EXTRACT: Lazy<AtomicBool> = Lazy::new(Default::default);

struct ProgressMsg {
    // 这里 channel 仅用于发送 init / final 等控制消息
    total: Option<u64>,
    stage_meta: serde_json::Value,
}

pub async fn extract_zip<R: Read + Seek + Send + 'static>(
    mut archive: ZipArchive<R>,
    destination: &str,
    force_replace: bool,
) -> Result<crate::core::result::CoreResult<()>, crate::core::result::CoreError> {
    CANCEL_EXTRACT.store(false, Ordering::SeqCst);

    // 1) 共享原子计数器（阻塞线程直接更新）
    let shared_extracted = Arc::new(AtomicU64::new(0));
    let shared_extracted_for_thread = shared_extracted.clone();

    // channel 用于 init / final 控制消息
    let (tx, mut rx) = mpsc::channel::<ProgressMsg>(8);

    let dest = destination.to_string();
    let handle = task::spawn_blocking(move || -> Result<(u64, u64), zip::result::ZipError> {
        // 1) 先收集索引元数据（单次访问），并计算 total
        let mut total: u64 = 0;
        let mut entries_meta = Vec::with_capacity(archive.len());
        for i in 0..archive.len() {
            let e = archive.by_index(i)?;
            let size = e.size();
            let name = e.mangled_name();
            let is_dir = e.is_dir();
            entries_meta.push((i, name, size, is_dir));
            total = total.saturating_add(size);
        }

        // 发送初始化消息（告诉 async 端 total）
        let _ = tx.blocking_send(ProgressMsg {
            total: Some(total),
            stage_meta: json!({ "stage": "init" }),
        });

        let start = StdInstant::now();
        // 逐项解压：**不再在这里计算 speed/eta**，而是直接更新 shared_extracted
        for (idx, name, size, is_dir) in entries_meta {
            if CANCEL_EXTRACT.load(Ordering::SeqCst) {
                debug!("用户已取消解压（阻塞线程检测）");
                // 发送取消状态（可选）
                let _ = tx.blocking_send(ProgressMsg {
                    total: Some(total),
                    stage_meta: json!({ "stage": "extracting", "status": "cancelled" }),
                });
                return Ok((shared_extracted_for_thread.load(Ordering::Relaxed), total));
            }

            let mut entry = archive.by_index(idx)?;

            let out_path = Path::new(&dest).join(&name);
            if is_dir {
                if let Some(p) = out_path.parent() {
                    fs::create_dir_all(p).ok();
                }
                fs::create_dir_all(&out_path).ok();
                continue;
            }

            if out_path.exists() {
                if force_replace {
                    if out_path.is_dir() {
                        let _ = fs::remove_dir_all(&out_path);
                    } else {
                        let _ = fs::remove_file(&out_path);
                    }
                } else {
                    // 已存在：直接视为已提取 size
                    shared_extracted_for_thread.fetch_add(size, Ordering::Relaxed);
                    continue;
                }
            }

            if let Some(p) = out_path.parent() {
                fs::create_dir_all(p).ok();
            }

            let f = File::create(&out_path)?;
            let mut writer = BufWriter::new(f);

            let mut buf = [0u8; 64 * 1024];
            loop {
                if CANCEL_EXTRACT.load(Ordering::SeqCst) {
                    debug!("用户已取消解压（拷贝中检测）");
                    let _ = tx.blocking_send(ProgressMsg {
                        total: Some(total),
                        stage_meta: json!({ "stage": "extracting", "status": "cancelled" }),
                    });
                    return Ok((shared_extracted_for_thread.load(Ordering::Relaxed), total));
                }
                let bytes_read = match entry.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) => return Err(ZipError::from(e)),
                };
                writer.write_all(&buf[..bytes_read])?;
                // 直接更新共享原子计数器
                shared_extracted_for_thread.fetch_add(bytes_read as u64, Ordering::Relaxed);
            }

            writer.flush()?;
        }

        // 完成：确保原子计数等于 total（有时可能略小，强制设置）
        shared_extracted_for_thread.store(total, Ordering::Relaxed);

        // 发送 final 完成消息
        let _ = tx.blocking_send(ProgressMsg {
            total: Some(total),
            stage_meta: json!({ "stage": "extracting", "status": "completed" }),
        });

        info!("解压完成，总计 {} bytes, 总耗时 {:.2} 秒", total, start.elapsed().as_secs_f64());
        Ok((total, total))
    });

    // async 端：等待 init 消息来创建 ExtractProgress，并启动 monitor（periodic reporter）
    let mut progress_opt: Option<ExtractProgress> = None;
    let mut monitor_handle_opt = None;

    while let Some(msg) = rx.recv().await {
        if msg.stage_meta.get("stage").and_then(|v| v.as_str()) == Some("init") {
            let total = msg.total.unwrap_or(0);
            // 用共享原子创建 ExtractProgress
            let mut progress = ExtractProgress::with_extracted(total, shared_extracted.clone());

            // spawn monitor task（每 500ms 检查并上报）
            let mut mon_progress = progress; // 按你的示例，monitor 持有 progress (mutable)
            let monitor = tokio::spawn(async move {
                loop {
                    // 退出条件：当 total>0 且已完成，或外部取消
                    if mon_progress.total > 0 && mon_progress.extracted.load(Ordering::Relaxed) >= mon_progress.total {
                        // final 上报
                        report_extract_progress(&mut mon_progress, json!({"stage": "extracting", "status":"completed"})).await;
                        break;
                    }
                    if CANCEL_EXTRACT.load(Ordering::SeqCst) {
                        report_extract_progress(&mut mon_progress, json!({"stage": "extracting", "status":"cancelled"})).await;
                        break;
                    }

                    if mon_progress.should_emit() {
                        report_extract_progress(&mut mon_progress, json!({"stage": "extracting"})).await;
                        mon_progress.update_prev();
                        mon_progress.mark_emitted();
                    }

                    sleep(Duration::from_millis(500)).await;
                }
            });
            monitor_handle_opt = Some(monitor);
            // 把 progress 放入 progress_opt 以便后续（如果需要）
            // NOTE: monitor 已拥有 mon_progress（moved），不用再保留 progress_opt if you don't need it.
            continue;
        }

        // 处理 completed/cancelled 控制消息 —— 等待 monitor 结束
        let status = msg.stage_meta.get("status").and_then(|v| v.as_str()).unwrap_or("");
        if status == "completed" || status == "cancelled" {
            // 等待阻塞任务返回并等待 monitor 结束
            if let Some(mh) = monitor_handle_opt.take() {
                let _ = mh.await;
            }
            break;
        }
    }

    // 等待阻塞任务完成并检查结果
    match handle.await {
        Ok(Ok((_extracted, _total))) => {
            if CANCEL_EXTRACT.load(Ordering::SeqCst) {
                return Ok(crate::core::result::CoreResult::Cancelled);
            }
            Ok(crate::core::result::CoreResult::Success(()))
        }
        Ok(Err(e)) => Err(crate::core::result::CoreError::from(e)),
        Err(join_err) => Err(crate::core::result::CoreError::Other(format!("join error: {}", join_err))),
    }
}
