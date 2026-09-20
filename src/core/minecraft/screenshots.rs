use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::core::minecraft::paths::{
    GamePathOptions, GameTargetDir, game_target_dirs, resolve_game_target_parent,
};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct McScreenshotInfo {
    pub key: String,
    pub image_path: String,
    pub folder_path: String,
    pub file_name: String,
    pub capture_time: Option<String>,
    pub modified: Option<String>,
    pub size_bytes: Option<u64>,
    pub json_path: Option<String>,
    pub mc_path: Option<String>,
    pub source_root: Option<String>,
    pub gdk_user: Option<String>,
}

pub fn resolve_screenshots_dir(options: &GamePathOptions) -> Option<PathBuf> {
    resolve_game_target_parent(options, GameTargetDir::Screenshots, false)
}

pub fn list_screenshots_standard(options: &GamePathOptions) -> Result<Vec<McScreenshotInfo>> {
    let roots = game_target_dirs(options, GameTargetDir::Screenshots);
    let mut screenshots = Vec::new();

    for root in roots {
        collect_screenshots_in_dir(&root, &root, &mut screenshots)?;
        if let Ok(entries) = fs::read_dir(&root) {
            for entry in entries.flatten() {
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_dir() {
                    collect_screenshots_in_dir(&entry.path(), &root, &mut screenshots)?;
                }
            }
        }
    }

    screenshots.sort_by(|left, right| {
        right
            .capture_time
            .as_deref()
            .or(right.modified.as_deref())
            .unwrap_or("")
            .cmp(
                left.capture_time
                    .as_deref()
                    .or(left.modified.as_deref())
                    .unwrap_or(""),
            )
            .then_with(|| right.file_name.cmp(&left.file_name))
    });

    Ok(screenshots)
}

fn screenshot_task_token(task_id: &str) -> String {
    let mut token = String::with_capacity(task_id.len().min(64));
    for ch in task_id.chars().take(64) {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
            token.push(ch);
        } else {
            token.push('_');
        }
    }
    if token.is_empty() {
        "task".to_string()
    } else {
        token
    }
}

fn rollback_screenshot_files(staged: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    let mut errors = Vec::new();
    for (original, tombstone) in staged.iter().rev() {
        if !tombstone.exists() {
            continue;
        }
        if original.exists() {
            errors.push(format!("无法回滚 {}：原文件已重新出现", original.display()));
            continue;
        }
        if let Err(error) = fs::rename(tombstone, original) {
            errors.push(format!("恢复 {} 失败: {error}", original.display()));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("；"))
    }
}

enum DeleteScreenshotOutcome {
    Completed(String),
    Cancelled(String),
}

fn delete_screenshot_transactionally(
    info: &McScreenshotInfo,
    task_id: &str,
) -> Result<DeleteScreenshotOutcome, String> {
    if crate::tasks::task_manager::is_cancelled(task_id) {
        return Ok(DeleteScreenshotOutcome::Cancelled(
            "截图删除已取消".to_string(),
        ));
    }
    let mut targets = vec![PathBuf::from(&info.image_path)];
    for path in [&info.json_path, &info.mc_path].into_iter().flatten() {
        let path = PathBuf::from(path);
        if !targets.contains(&path) {
            targets.push(path);
        }
    }
    targets.retain(|path| path.exists());
    if targets.is_empty() {
        return Ok(DeleteScreenshotOutcome::Completed(
            "截图已不存在".to_string(),
        ));
    }

    let token = screenshot_task_token(task_id);
    let mut planned = Vec::with_capacity(targets.len());
    for (index, target) in targets.iter().enumerate() {
        let parent = target
            .parent()
            .ok_or_else(|| format!("截图文件没有父目录: {}", target.display()))?;
        let name = target
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("screenshot");
        let tombstone = parent.join(format!(".{name}.bmcb-delete-{token}-{index}"));
        if tombstone.exists() {
            return Err(format!("截图删除暂存文件已存在: {}", tombstone.display()));
        }
        planned.push((target.clone(), tombstone));
    }

    let mut staged: Vec<(PathBuf, PathBuf)> = Vec::with_capacity(planned.len());
    for (target, tombstone) in planned {
        if crate::tasks::task_manager::is_cancelled(task_id) {
            let cancelled = "截图删除已取消".to_string();
            if let Err(rollback_error) = rollback_screenshot_files(&staged) {
                return Err(format!("{cancelled}；{rollback_error}"));
            }
            return Ok(DeleteScreenshotOutcome::Cancelled(cancelled));
        }

        if let Err(error) = fs::rename(&target, &tombstone) {
            let rollback_error = rollback_screenshot_files(&staged).err();
            return Err(match rollback_error {
                Some(rollback_error) => format!(
                    "暂存截图删除失败 {}: {error}；{rollback_error}",
                    target.display()
                ),
                None => format!("暂存截图删除失败 {}: {error}", target.display()),
            });
        }
        staged.push((target, tombstone));
    }

    // Image and sidecars are now atomically absent as one logical screenshot. Cleanup after this
    // commit boundary is intentionally non-cancellable.
    for (_, tombstone) in &staged {
        fs::remove_file(tombstone).map_err(|error| {
            format!("清理截图删除暂存文件失败 {}: {error}", tombstone.display())
        })?;
    }

    Ok(DeleteScreenshotOutcome::Completed("截图已删除".to_string()))
}

pub fn start_delete_screenshot_task(info: McScreenshotInfo) -> Result<String, String> {
    let detail = info.file_name.clone();
    let task_id = crate::tasks::task_manager::create_task_with_details(
        None,
        "删除截图",
        Some(detail),
        "deleting_screenshot",
        None,
        false,
    );
    crate::tasks::task_manager::register_task_cooperative_cancel(task_id.clone());

    let worker_task_id = task_id.clone();
    let blocking_task_id = task_id.clone();
    let workflow = crate::tasks::runtime::spawn_io(async move {
        let result = crate::tasks::runtime::run_io_blocking(move || {
            delete_screenshot_transactionally(&info, &blocking_task_id)
        })
        .await;

        match result {
            Ok(Ok(DeleteScreenshotOutcome::Completed(message))) => {
                crate::tasks::task_manager::finish_task(
                    &worker_task_id,
                    "completed",
                    Some(message),
                );
            }
            Ok(Ok(DeleteScreenshotOutcome::Cancelled(message))) => {
                crate::tasks::task_manager::finish_task(
                    &worker_task_id,
                    "cancelled",
                    Some(message),
                );
            }
            Ok(Err(error)) => crate::tasks::task_manager::finish_task(
                &worker_task_id,
                "error",
                Some(error),
            ),
            Err(error) => crate::tasks::task_manager::finish_task(
                &worker_task_id,
                "error",
                Some(error),
            ),
        }
    })
    .map_err(|error| {
        crate::tasks::task_manager::finish_task(&task_id, "error", Some(error.clone()));
        error
    })?;

    let monitor_task_id = task_id.clone();
    if let Err(error) = crate::tasks::runtime::spawn_io(async move {
        if let Err(error) = workflow.await {
            crate::tasks::task_manager::finish_task(
                &monitor_task_id,
                "error",
                Some(format!("截图删除任务异常结束: {error}")),
            );
        }
    }) {
        crate::tasks::task_manager::finish_task(&task_id, "error", Some(error.clone()));
        return Err(error);
    }

    Ok(task_id)
}

fn collect_screenshots_in_dir(
    dir: &Path,
    root: &Path,
    screenshots: &mut Vec<McScreenshotInfo>,
) -> Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("读取截图目录失败: {}", dir.display()));
        }
    };

    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }

        let path = entry.path();
        if !is_screenshot_image(&path) {
            continue;
        }

        let metadata = fs::metadata(&path).ok();
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        let parent = path.parent().unwrap_or(dir);
        let json_path = parent.join(format!("{stem}.json"));
        let mc_path = parent.join(format!("{stem}.mc"));
        let capture_time = parse_capture_time(&json_path).or_else(|| {
            metadata
                .as_ref()
                .and_then(|metadata| metadata.modified().ok())
                .map(system_time_to_iso)
        });

        let image_path = path.to_string_lossy().to_string();
        screenshots.push(McScreenshotInfo {
            key: format!("screenshot:{image_path}"),
            image_path,
            folder_path: parent.to_string_lossy().to_string(),
            file_name: path
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_default(),
            capture_time,
            modified: metadata
                .as_ref()
                .and_then(|metadata| metadata.modified().ok())
                .map(system_time_to_iso),
            size_bytes: metadata.map(|metadata| metadata.len()),
            json_path: json_path
                .exists()
                .then(|| json_path.to_string_lossy().to_string()),
            mc_path: mc_path
                .exists()
                .then(|| mc_path.to_string_lossy().to_string()),
            source_root: Some(root.to_string_lossy().to_string()),
            gdk_user: infer_gdk_user(&path),
        });
    }

    Ok(())
}

fn is_screenshot_image(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "jpeg" | "jpg" | "png"
            )
        })
}

fn parse_capture_time(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<Value>(&content).ok()?;
    let timestamp = value.get("captureTime")?.as_i64()?;
    DateTime::<Utc>::from_timestamp(timestamp, 0).map(|time| time.to_rfc3339())
}

fn infer_gdk_user(path: &Path) -> Option<String> {
    let parts: Vec<_> = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect();
    parts
        .windows(2)
        .find(|window| window[0].eq_ignore_ascii_case("Users"))
        .map(|window| window[1].to_string())
}

fn system_time_to_iso(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be valid")
            .as_nanos();
        std::env::temp_dir().join(format!("bmcb_screenshot_test_{name}_{nanos}"))
    }

    #[test]
    fn scans_root_and_one_nested_screenshot_directory() {
        let root = temp_dir("scan");
        let nested = root.join("2535413569375435");
        fs::create_dir_all(&nested).expect("create temp dir");
        fs::write(root.join("root.jpeg"), [0_u8; 3]).expect("write image");
        fs::write(nested.join("nested.jpeg"), [1_u8; 3]).expect("write image");
        fs::write(nested.join("nested.json"), r#"{"captureTime":1773495614}"#).expect("write json");

        let mut screenshots = Vec::new();
        collect_screenshots_in_dir(&root, &root, &mut screenshots).expect("scan root");
        collect_screenshots_in_dir(&nested, &root, &mut screenshots).expect("scan nested");

        assert_eq!(screenshots.len(), 2);
        assert!(screenshots.iter().any(|item| item.file_name == "root.jpeg"));
        assert!(screenshots.iter().any(|item| item.capture_time.is_some()));

        fs::remove_dir_all(root).expect("remove temp dir");
    }

    #[test]
    fn deletes_image_and_sidecar_files() {
        let root = temp_dir("delete");
        fs::create_dir_all(&root).expect("create temp dir");
        let image = root.join("shot.jpeg");
        let json = root.join("shot.json");
        let mc = root.join("shot.mc");
        fs::write(&image, [0_u8; 3]).expect("write image");
        fs::write(&json, "{}").expect("write json");
        fs::write(&mc, [1_u8; 3]).expect("write mc");

        let info = McScreenshotInfo {
            key: "screenshot:test".to_string(),
            image_path: image.to_string_lossy().to_string(),
            folder_path: root.to_string_lossy().to_string(),
            file_name: "shot.jpeg".to_string(),
            capture_time: None,
            modified: None,
            size_bytes: None,
            json_path: Some(json.to_string_lossy().to_string()),
            mc_path: Some(mc.to_string_lossy().to_string()),
            source_root: None,
            gdk_user: None,
        };

        let task_id = format!(
            "screenshot-delete-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time should be valid")
                .as_nanos()
        );
        crate::tasks::task_manager::create_task_with_details(
            Some(task_id.clone()),
            "截图删除测试",
            None,
            "running",
            None,
            false,
        );
        let outcome =
            delete_screenshot_transactionally(&info, &task_id).expect("delete screenshot");
        assert!(matches!(outcome, DeleteScreenshotOutcome::Completed(_)));
        let _ = crate::tasks::task_manager::remove_task(&task_id);

        assert!(!image.exists());
        assert!(!json.exists());
        assert!(!mc.exists());

        fs::remove_dir_all(root).expect("remove temp dir");
    }
}
