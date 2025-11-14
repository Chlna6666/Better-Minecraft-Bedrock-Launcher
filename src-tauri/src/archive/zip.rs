// src/downloads/extract_zip.rs  （文件名按你的工程调整）
use std::fs;
use std::fs::File;
use std::io::{BufWriter, Read, Seek, Write};
use std::path::Path;
use std::time::Instant as StdInstant;

use zip::result::ZipError;
use zip::ZipArchive;

use tokio::task;
use tracing::{debug, info};

use crate::result::{CoreError, CoreResult};
use crate::tasks::task_manager::{finish_task, is_cancelled, set_total, update_progress};

/// 将 archive 解压到 destination
/// 注意：新增参数 `task_id`（拥有所有权的 String），用于取消/进度上报
pub async fn extract_zip<R: Read + Seek + Send + 'static>(
    mut archive: ZipArchive<R>,
    destination: &str,
    force_replace: bool,
    task_id: String,
) -> Result<CoreResult<()>, CoreError> {
    // spawn_blocking 内执行实际解压（IO 密集）
    let dest_string = destination.to_string();
    let task_id_clone_for_block = task_id.clone();

    let handle = task::spawn_blocking(move || -> Result<(), ZipError> {
        // 1) 收集条目并计算总大小
        let mut total: u64 = 0;
        let mut entries = Vec::with_capacity(archive.len());
        for i in 0..archive.len() {
            let e = archive.by_index(i)?;
            let size = e.size();
            let name = e.mangled_name();
            let is_dir = e.is_dir();
            entries.push((i, name, size, is_dir));
            total = total.saturating_add(size);
        }

        // 设置 task_manager 的 total（线程安全）
        set_total(&task_id_clone_for_block, Some(total));

        let start = StdInstant::now();

        // 逐项解压
        for (idx, name, size, is_dir) in entries {
            // 取消检查（使用 task_manager）
            if is_cancelled(&task_id_clone_for_block) {
                debug!("解压已被取消（检测到 task cancelled）");
                // 写最终状态并返回（注意：finish_task 也可以放到 async 侧，但这里放在 blocking 侧也可以）
                finish_task(&task_id_clone_for_block, "cancelled", Some("user cancelled".into()));
                return Ok(());
            }

            let mut entry = archive.by_index(idx)?;
            let out_path = Path::new(&dest_string).join(&name);

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
                    // 已存在：视为已完成此 entry 的大小
                    update_progress(&task_id_clone_for_block, size, Some(total), Some("extracting"));
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
                // 取消检查
                if is_cancelled(&task_id_clone_for_block) {
                    debug!("解压在写入过程中被取消");
                    finish_task(&task_id_clone_for_block, "cancelled", Some("user cancelled".into()));
                    return Ok(());
                }

                let bytes_read = match entry.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) => return Err(ZipError::from(e)),
                };

                writer.write_all(&buf[..bytes_read])?;
                // 上报增量到 task_manager（由 task_manager 计算速度/ETA）
                update_progress(&task_id_clone_for_block, bytes_read as u64, Some(total), Some("extracting"));
            }

            writer.flush()?;
        }

        // 强制把 done 置为 total（避免短期微小误差）
        update_progress(&task_id_clone_for_block, 0, Some(total), Some("extracting"));
        finish_task(&task_id_clone_for_block, "completed", None);

        info!("解压完成，总计 {} bytes, 总耗时 {:.2} 秒", total, start.elapsed().as_secs_f64());
        Ok(())
    });

    // 等待 blocking 任务完成
    match handle.await {
        Ok(Ok(())) => {
            // 如果调用方在 async 侧想再次检查取消可以用 is_cancelled(task_id)
            if is_cancelled(&task_id) {
                return Ok(CoreResult::Cancelled);
            }
            Ok(CoreResult::Success(()))
        }
        Ok(Err(zip_err)) => Err(CoreError::from(zip_err)),
        Err(join_err) => Err(CoreError::Other(format!("join error: {}", join_err))),
    }
}
