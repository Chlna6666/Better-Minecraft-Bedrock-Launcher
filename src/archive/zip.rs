// src/downloads/extract_zip.rs  （文件名按你的工程调整）
use std::fs;
use std::fs::File;
use std::io::{BufWriter, Read, Seek, Write};
use std::path::Path;
use std::time::Duration as StdDuration;
use std::time::Instant as StdInstant;

use zip::ZipArchive;

use tokio::task;
use tracing::{debug, info};

use crate::result::{CoreError, CoreResult};
use crate::tasks::task_manager::{
    TaskVisualization, finish_task, is_cancelled, set_task_visualization, set_total,
    task_visualization_enabled, update_progress,
};

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

    let handle = task::spawn_blocking(move || -> Result<(), String> {
        // 1) 收集条目并计算总大小
        let mut total: u64 = 0;
        let mut entries = Vec::with_capacity(archive.len());
        for i in 0..archive.len() {
            let e = archive
                .by_index(i)
                .map_err(|error| format!("读取 zip 条目 #{i} 失败: {error}"))?;
            let size = e.size();
            let name = e
                .mangled_name()
                .map_err(|error| format!("解析 zip 条目路径失败 #{i}: {error}"))?;
            let is_dir = e.is_dir();
            entries.push((i, name, size, is_dir));
            total = total.saturating_add(size);
        }

        // 设置 task_manager 的 total（线程安全）
        set_total(&task_id_clone_for_block, Some(total));
        let entry_total = entries.len() as u64;
        if task_visualization_enabled() {
            set_task_visualization(
                &task_id_clone_for_block,
                Some(TaskVisualization {
                    worker_total: Some(1),
                    worker_active: Some(1),
                    unit_label: Some("文件".to_string()),
                    unit_total: Some(entry_total),
                    unit_done: Some(0),
                    current_item: Some("等待解压文件".to_string()),
                    threads: None,
                }),
            );
        }

        let start = StdInstant::now();
        let mut pending_progress = 0u64;
        let mut finished_entries = 0u64;
        let mut last_progress_emit = StdInstant::now();

        // 逐项解压
        for (idx, name, size, is_dir) in entries {
            let display_name = name.to_string_lossy().to_string();
            if task_visualization_enabled() {
                set_task_visualization(
                    &task_id_clone_for_block,
                    Some(TaskVisualization {
                        worker_total: Some(1),
                        worker_active: Some(1),
                        unit_label: Some("文件".to_string()),
                        unit_total: Some(entry_total),
                        unit_done: Some(finished_entries),
                        current_item: Some(display_name.clone()),
                        threads: None,
                    }),
                );
            }

            // 取消检查（使用 task_manager）
            if is_cancelled(&task_id_clone_for_block) {
                debug!("解压已被取消（检测到 task cancelled）");
                // 写最终状态并返回（注意：finish_task 也可以放到 async 侧，但这里放在 blocking 侧也可以）
                finish_task(
                    &task_id_clone_for_block,
                    "cancelled",
                    Some("user cancelled".into()),
                );
                return Ok(());
            }

            let mut entry = archive
                .by_index(idx)
                .map_err(|error| format!("读取 zip 条目失败: {display_name} ({error})"))?;
            let out_path = Path::new(&dest_string).join(&name);

            if is_dir {
                if let Some(p) = out_path.parent() {
                    fs::create_dir_all(p)
                        .map_err(|error| format!("创建父目录失败: {} ({error})", p.display()))?;
                }
                fs::create_dir_all(&out_path)
                    .map_err(|error| format!("创建目录失败: {} ({error})", out_path.display()))?;
                finished_entries = finished_entries.saturating_add(1);
                continue;
            }

            if out_path.exists() {
                if force_replace {
                    if out_path.is_dir() {
                        fs::remove_dir_all(&out_path).map_err(|error| {
                            format!("删除已有目录失败: {} ({error})", out_path.display())
                        })?;
                    } else {
                        fs::remove_file(&out_path).map_err(|error| {
                            format!("删除已有文件失败: {} ({error})", out_path.display())
                        })?;
                    }
                } else {
                    // 已存在：视为已完成此 entry 的大小
                    pending_progress = pending_progress.saturating_add(size);
                    if pending_progress >= 1024 * 1024
                        || last_progress_emit.elapsed() >= StdDuration::from_millis(200)
                    {
                        update_progress(
                            &task_id_clone_for_block,
                            pending_progress,
                            Some(total),
                            Some("extracting"),
                        );
                        pending_progress = 0;
                        last_progress_emit = StdInstant::now();
                    }
                    finished_entries = finished_entries.saturating_add(1);
                    continue;
                }
            }

            if let Some(p) = out_path.parent() {
                fs::create_dir_all(p)
                    .map_err(|error| format!("创建父目录失败: {} ({error})", p.display()))?;
            }

            let f = File::create(&out_path)
                .map_err(|error| format!("创建文件失败: {} ({error})", out_path.display()))?;
            let mut writer = BufWriter::new(f);

            let mut buf = [0u8; 64 * 1024];
            loop {
                // 取消检查
                if is_cancelled(&task_id_clone_for_block) {
                    debug!("解压在写入过程中被取消");
                    finish_task(
                        &task_id_clone_for_block,
                        "cancelled",
                        Some("user cancelled".into()),
                    );
                    return Ok(());
                }

                let bytes_read = entry
                    .read(&mut buf)
                    .map_err(|error| format!("读取压缩条目失败: {display_name} ({error})"))?;
                if bytes_read == 0 {
                    break;
                }

                writer
                    .write_all(&buf[..bytes_read])
                    .map_err(|error| format!("写入文件失败: {} ({error})", out_path.display()))?;
                pending_progress = pending_progress.saturating_add(bytes_read as u64);
                if pending_progress >= 1024 * 1024
                    || last_progress_emit.elapsed() >= StdDuration::from_millis(200)
                {
                    update_progress(
                        &task_id_clone_for_block,
                        pending_progress,
                        Some(total),
                        Some("extracting"),
                    );
                    pending_progress = 0;
                    last_progress_emit = StdInstant::now();
                }
            }

            writer
                .flush()
                .map_err(|error| format!("刷新文件失败: {} ({error})", out_path.display()))?;
            finished_entries = finished_entries.saturating_add(1);
            if task_visualization_enabled() {
                set_task_visualization(
                    &task_id_clone_for_block,
                    Some(TaskVisualization {
                        worker_total: Some(1),
                        worker_active: Some(1),
                        unit_label: Some("文件".to_string()),
                        unit_total: Some(entry_total),
                        unit_done: Some(finished_entries),
                        current_item: Some(display_name),
                        threads: None,
                    }),
                );
            }
        }

        if pending_progress > 0 {
            update_progress(
                &task_id_clone_for_block,
                pending_progress,
                Some(total),
                Some("extracting"),
            );
        }

        update_progress(&task_id_clone_for_block, 0, Some(total), Some("extracting"));

        info!(
            "解压完成，总计 {} bytes, 总耗时 {:.2} 秒",
            total,
            start.elapsed().as_secs_f64()
        );
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
        Ok(Err(error)) => Err(CoreError::Other(error)),
        Err(join_err) => Err(CoreError::Other(format!("join error: {}", join_err))),
    }
}
