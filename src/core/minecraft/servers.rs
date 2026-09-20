use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::minecraft::paths::{GamePathOptions, GameTargetDir, resolve_game_target_parent};

const SERVER_FILE_NAME: &str = "external_servers.txt";

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ExternalServerEntry {
    pub key: String,
    pub index: usize,
    pub name: String,
    pub address: String,
    pub port: u16,
    pub file_path: String,
    pub line_number: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ExternalServerLine {
    Parsed {
        entry: ExternalServerEntry,
        metadata: String,
    },
    Raw(String),
}

pub fn resolve_external_servers_file(options: &GamePathOptions) -> Option<PathBuf> {
    resolve_game_target_parent(options, GameTargetDir::MinecraftPe, false)
        .map(|path| path.join(SERVER_FILE_NAME))
}

pub fn read_external_servers(options: &GamePathOptions) -> Result<Vec<ExternalServerEntry>> {
    let file_path = match resolve_external_servers_file(options) {
        Some(path) => path,
        None => return Ok(Vec::new()),
    };
    Ok(read_external_server_lines(&file_path)?
        .into_iter()
        .filter_map(|line| match line {
            ExternalServerLine::Parsed { entry, .. } => Some(entry),
            ExternalServerLine::Raw(_) => None,
        })
        .collect())
}

enum ServerMutation {
    Add {
        name: String,
        address: String,
        port: u16,
    },
    Update {
        key: String,
        name: String,
        address: String,
        port: u16,
    },
    Delete {
        key: String,
    },
}

enum ServerMutationOutcome {
    Completed(String),
    Cancelled(String),
}

fn server_task_token(task_id: &str) -> String {
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

fn serialize_external_server_lines(lines: &[ExternalServerLine]) -> String {
    let mut content = String::new();
    for line in lines {
        match line {
            ExternalServerLine::Parsed { entry, metadata } => {
                let metadata = if metadata.trim().is_empty() {
                    "0"
                } else {
                    metadata.trim()
                };
                content.push_str(&format!(
                    "{}:{}:{}:{}:{}",
                    entry.index, entry.name, entry.address, entry.port, metadata
                ));
            }
            ExternalServerLine::Raw(line) => content.push_str(line),
        }
        content.push_str("\r\n");
    }
    content
}

fn remove_file_if_exists(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("清理服务器事务文件失败 {}: {error}", path.display())),
    }
}

fn write_lines_to_file_transactionally(
    file_path: &Path,
    lines: &[ExternalServerLine],
    task_id: &str,
) -> Result<bool, String> {
    let parent = file_path
        .parent()
        .ok_or_else(|| format!("服务器文件没有父目录: {}", file_path.display()))?;
    let parent_existed = parent.exists();
    fs::create_dir_all(parent)
        .map_err(|error| format!("创建服务器目录失败 {}: {error}", parent.display()))?;

    let token = server_task_token(task_id);
    let staging = parent.join(format!(".{SERVER_FILE_NAME}.bmcb-staging-{token}"));
    let backup = parent.join(format!(".{SERVER_FILE_NAME}.bmcb-backup-{token}"));
    remove_file_if_exists(&staging)?;
    remove_file_if_exists(&backup)?;

    let content = serialize_external_server_lines(lines);
    let mut staging_file = fs::File::create(&staging)
        .map_err(|error| format!("创建服务器 staging 失败: {error}"))?;
    if let Err(error) = staging_file.write_all(content.as_bytes()) {
        let _ = remove_file_if_exists(&staging);
        return Err(format!("写入服务器 staging 失败: {error}"));
    }
    if let Err(error) = staging_file.sync_all() {
        let _ = remove_file_if_exists(&staging);
        return Err(format!("刷新服务器 staging 失败: {error}"));
    }
    drop(staging_file);

    if crate::tasks::task_manager::is_cancelled(task_id) {
        let _ = remove_file_if_exists(&staging);
        if !parent_existed {
            let _ = fs::remove_dir(parent);
        }
        return Ok(false);
    }

    let had_previous = file_path.exists();
    if had_previous {
        fs::rename(file_path, &backup).map_err(|error| {
            let _ = remove_file_if_exists(&staging);
            format!("备份旧服务器列表失败: {error}")
        })?;
    }

    if crate::tasks::task_manager::is_cancelled(task_id) {
        if had_previous {
            fs::rename(&backup, file_path)
                .map_err(|error| format!("取消服务器写入时恢复旧文件失败: {error}"))?;
        }
        let _ = remove_file_if_exists(&staging);
        if !parent_existed {
            let _ = fs::remove_dir(parent);
        }
        return Ok(false);
    }

    if let Err(error) = fs::rename(&staging, file_path) {
        let rollback = if had_previous {
            fs::rename(&backup, file_path)
                .map_err(|restore_error| format!("提交失败且恢复旧文件失败: {restore_error}"))
        } else {
            Ok(())
        };
        let _ = remove_file_if_exists(&staging);
        rollback?;
        return Err(format!("提交服务器列表失败: {error}"));
    }

    // Atomic rename above is the commit boundary. Late cancellation must not undo the committed
    // server list; only cleanup remains.
    if had_previous {
        remove_file_if_exists(&backup)?;
    }
    Ok(true)
}

fn apply_server_mutation(
    options: &GamePathOptions,
    mutation: ServerMutation,
    task_id: &str,
) -> Result<ServerMutationOutcome, String> {
    if crate::tasks::task_manager::is_cancelled(task_id) {
        return Ok(ServerMutationOutcome::Cancelled(
            "服务器操作已取消".to_string(),
        ));
    }

    let file_path = resolve_external_servers_file(options)
        .ok_or_else(|| "无法解析 external_servers.txt 路径".to_string())?;
    let mut lines = read_external_server_lines(&file_path).map_err(|error| error.to_string())?;

    let message = match mutation {
        ServerMutation::Add {
            name,
            address,
            port,
        } => {
            let (name, address) =
                validate_server_input(&name, &address, port).map_err(|error| error.to_string())?;
            let next_index = lines
                .iter()
                .filter_map(|line| match line {
                    ExternalServerLine::Parsed { entry, .. } => Some(entry.index),
                    ExternalServerLine::Raw(_) => None,
                })
                .max()
                .map_or(0, |index| index + 1);
            let entry = ExternalServerEntry {
                key: server_key(next_index, address, port),
                index: next_index,
                name: name.to_string(),
                address: address.to_string(),
                port,
                file_path: file_path.to_string_lossy().to_string(),
                line_number: lines.len() + 1,
            };
            lines.push(ExternalServerLine::Parsed {
                entry,
                metadata: current_unix_seconds().to_string(),
            });
            "服务器已添加".to_string()
        }
        ServerMutation::Update {
            key,
            name,
            address,
            port,
        } => {
            let (name, address) =
                validate_server_input(&name, &address, port).map_err(|error| error.to_string())?;
            let mut found = false;
            for line in &mut lines {
                let ExternalServerLine::Parsed { entry, .. } = line else {
                    continue;
                };
                if entry.key != key {
                    continue;
                }
                entry.name = name.to_string();
                entry.address = address.to_string();
                entry.port = port;
                entry.key = server_key(entry.index, address, port);
                found = true;
                break;
            }
            if !found {
                return Err(format!("未找到服务器: {key}"));
            }
            "服务器已更新".to_string()
        }
        ServerMutation::Delete { key } => {
            let before = lines.len();
            lines.retain(|line| match line {
                ExternalServerLine::Parsed { entry, .. } => entry.key != key,
                ExternalServerLine::Raw(_) => true,
            });
            if lines.len() == before {
                return Err(format!("未找到服务器: {key}"));
            }
            "服务器已删除".to_string()
        }
    };

    if write_lines_to_file_transactionally(&file_path, &lines, task_id)? {
        Ok(ServerMutationOutcome::Completed(message))
    } else {
        Ok(ServerMutationOutcome::Cancelled(
            "服务器操作已取消".to_string(),
        ))
    }
}

fn start_server_mutation_task(
    options: GamePathOptions,
    mutation: ServerMutation,
    title: &'static str,
    detail: String,
) -> Result<String, String> {
    let task_id = crate::tasks::task_manager::create_task_with_details(
        None,
        title,
        Some(detail),
        "updating_servers",
        None,
        false,
    );
    crate::tasks::task_manager::register_task_cooperative_cancel(task_id.clone());

    let worker_task_id = task_id.clone();
    let blocking_task_id = task_id.clone();
    let workflow = crate::tasks::runtime::spawn_io(async move {
        let result = crate::tasks::runtime::run_io_blocking(move || {
            apply_server_mutation(&options, mutation, &blocking_task_id)
        })
        .await;

        match result {
            Ok(Ok(ServerMutationOutcome::Completed(message))) => {
                crate::tasks::task_manager::finish_task(
                    &worker_task_id,
                    "completed",
                    Some(message),
                );
            }
            Ok(Ok(ServerMutationOutcome::Cancelled(message))) => {
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
                Some(format!("服务器任务异常结束: {error}")),
            );
        }
    }) {
        crate::tasks::task_manager::finish_task(&task_id, "error", Some(error.clone()));
        return Err(error);
    }

    Ok(task_id)
}

pub fn start_add_external_server_task(
    options: GamePathOptions,
    name: String,
    address: String,
    port: u16,
) -> Result<String, String> {
    validate_server_input(&name, &address, port).map_err(|error| error.to_string())?;
    let detail = format!("{} · {}:{port}", name.trim(), address.trim());
    start_server_mutation_task(
        options,
        ServerMutation::Add {
            name,
            address,
            port,
        },
        "添加服务器",
        detail,
    )
}

pub fn start_update_external_server_task(
    options: GamePathOptions,
    key: String,
    name: String,
    address: String,
    port: u16,
) -> Result<String, String> {
    validate_server_input(&name, &address, port).map_err(|error| error.to_string())?;
    let detail = format!("{} · {}:{port}", name.trim(), address.trim());
    start_server_mutation_task(
        options,
        ServerMutation::Update {
            key,
            name,
            address,
            port,
        },
        "更新服务器",
        detail,
    )
}

pub fn start_delete_external_server_task(
    options: GamePathOptions,
    key: String,
    detail: String,
) -> Result<String, String> {
    start_server_mutation_task(
        options,
        ServerMutation::Delete { key },
        "删除服务器",
        detail,
    )
}

fn read_external_server_lines(file_path: &Path) -> Result<Vec<ExternalServerLine>> {
    let content = match fs::read_to_string(file_path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("读取 external_servers.txt 失败: {}", file_path.display())
            });
        }
    };

    Ok(content
        .lines()
        .enumerate()
        .filter_map(|(line_index, line)| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            Some(parse_external_server_line(trimmed, file_path, line_index))
        })
        .collect())
}

fn parse_external_server_line(
    line: &str,
    file_path: &Path,
    line_index: usize,
) -> ExternalServerLine {
    let fields: Vec<&str> = line.splitn(5, ':').collect();
    if fields.len() < 4 {
        return ExternalServerLine::Raw(line.to_string());
    }

    let Some(index) = parse_server_index(fields[0]) else {
        return ExternalServerLine::Raw(line.to_string());
    };
    let Ok(port) = fields[3].trim().parse::<u16>() else {
        return ExternalServerLine::Raw(line.to_string());
    };

    let name = fields[1].trim();
    let address = fields[2].trim();
    if name.is_empty() || address.is_empty() {
        return ExternalServerLine::Raw(line.to_string());
    }

    ExternalServerLine::Parsed {
        entry: ExternalServerEntry {
            key: server_key(index, address, port),
            index,
            name: name.to_string(),
            address: address.to_string(),
            port,
            file_path: file_path.to_string_lossy().to_string(),
            line_number: line_index + 1,
        },
        metadata: fields
            .get(4)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "0".to_string()),
    }
}

fn parse_server_index(value: &str) -> Option<usize> {
    let trimmed = value.trim();
    if trimmed.chars().all(|ch| ch.is_ascii_digit()) {
        return trimmed.parse().ok();
    }

    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("server") {
        return None;
    }

    let digits: String = trimmed.chars().filter(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[cfg(test)]
fn write_entries_to_file(file_path: &Path, entries: &[ExternalServerEntry]) -> Result<()> {
    let lines = entries
        .iter()
        .cloned()
        .map(|entry| ExternalServerLine::Parsed {
            entry,
            metadata: current_unix_seconds().to_string(),
        })
        .collect::<Vec<_>>();
    write_lines_to_file(file_path, &lines)
}

fn validate_server_input<'a>(
    name: &'a str,
    address: &'a str,
    port: u16,
) -> Result<(&'a str, &'a str)> {
    let name = name.trim();
    let address = address.trim();
    if name.is_empty() {
        bail!("服务器名称不能为空");
    }
    if address.is_empty() {
        bail!("服务器地址不能为空");
    }
    if port == 0 {
        bail!("端口必须大于 0");
    }

    Ok((name, address))
}

#[cfg(test)]
fn write_lines_to_file(file_path: &Path, lines: &[ExternalServerLine]) -> Result<()> {
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建服务器目录失败: {}", parent.display()))?;
    }

    let mut content = String::new();
    for line in lines {
        match line {
            ExternalServerLine::Parsed { entry, metadata } => {
                let metadata = if metadata.trim().is_empty() {
                    "0"
                } else {
                    metadata.trim()
                };
                content.push_str(&format!(
                    "{}:{}:{}:{}:{}",
                    entry.index, entry.name, entry.address, entry.port, metadata
                ));
            }
            ExternalServerLine::Raw(line) => content.push_str(line),
        }
        content.push_str("\r\n");
    }

    fs::write(file_path, content)
        .with_context(|| format!("写入 external_servers.txt 失败: {}", file_path.display()))
}

fn server_key(index: usize, address: &str, port: u16) -> String {
    format!("server:{index}:{address}:{port}")
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_file(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be valid")
            .as_nanos();
        std::env::temp_dir().join(format!("bmcb_server_test_{name}_{nanos}.txt"))
    }

    #[test]
    fn parses_external_server_line() {
        let path = temp_file("parse");
        let line =
            parse_external_server_line("7:CubeCraft:play.cubecraft.net:19132:1777204231", &path, 0);

        let ExternalServerLine::Parsed { entry, metadata } = line else {
            panic!("server line should parse");
        };
        assert_eq!(entry.index, 7);
        assert_eq!(entry.name, "CubeCraft");
        assert_eq!(entry.address, "play.cubecraft.net");
        assert_eq!(entry.port, 19132);
        assert_eq!(metadata, "1777204231");
    }

    #[test]
    fn parses_legacy_external_server_line() {
        let path = temp_file("legacy_parse");
        let line =
            parse_external_server_line("server 7:CubeCraft:play.cubecraft.net:19132:0", &path, 0);

        let ExternalServerLine::Parsed { entry, metadata } = line else {
            panic!("server line should parse");
        };
        assert_eq!(entry.index, 7);
        assert_eq!(entry.name, "CubeCraft");
        assert_eq!(entry.address, "play.cubecraft.net");
        assert_eq!(entry.port, 19132);
        assert_eq!(metadata, "0");
    }

    #[test]
    fn preserves_raw_lines_when_deleting() {
        let path = temp_file("delete");
        fs::write(
            &path,
            "raw line\r\n0:One:one.example.com:19132:123\r\n1:Two:two.example.com:19132:456\r\n",
        )
        .expect("write temp file");

        let mut lines = read_external_server_lines(&path).expect("read lines");
        let key = match &lines[1] {
            ExternalServerLine::Parsed { entry, .. } => entry.key.clone(),
            ExternalServerLine::Raw(_) => panic!("expected parsed line"),
        };
        lines.retain(|line| match line {
            ExternalServerLine::Parsed { entry, .. } => entry.key != key,
            ExternalServerLine::Raw(_) => true,
        });
        write_lines_to_file(&path, &lines).expect("write lines");

        let content = fs::read_to_string(&path).expect("read temp file");
        assert!(content.contains("raw line"));
        assert!(!content.contains("One"));
        assert!(content.contains("1:Two:two.example.com:19132:456"));

        fs::remove_file(path).expect("remove temp file");
    }

    #[test]
    fn write_entries_uses_crlf() {
        let path = temp_file("write");
        let entry = ExternalServerEntry {
            key: "server:0:test.example.com:19132".to_string(),
            index: 0,
            name: "Test".to_string(),
            address: "test.example.com".to_string(),
            port: 19132,
            file_path: path.to_string_lossy().to_string(),
            line_number: 1,
        };
        write_lines_to_file(
            &path,
            &[ExternalServerLine::Parsed {
                entry,
                metadata: "789".to_string(),
            }],
        )
        .expect("write entries");

        let content = fs::read_to_string(&path).expect("read temp file");
        assert_eq!(content, "0:Test:test.example.com:19132:789\r\n");

        fs::remove_file(path).expect("remove temp file");
    }

    #[test]
    fn updates_server_and_preserves_metadata() {
        let path = temp_file("update");
        fs::write(&path, "1:Old:old.example.com:19132:1777204231\r\n").expect("write temp file");

        let mut lines = read_external_server_lines(&path).expect("read lines");
        let key = match &lines[0] {
            ExternalServerLine::Parsed { entry, .. } => entry.key.clone(),
            ExternalServerLine::Raw(_) => panic!("expected parsed line"),
        };
        for line in &mut lines {
            let ExternalServerLine::Parsed { entry, .. } = line else {
                continue;
            };
            if entry.key == key {
                entry.name = "New".to_string();
                entry.address = "new.example.com".to_string();
                entry.port = 19133;
                entry.key = server_key(entry.index, &entry.address, entry.port);
            }
        }
        write_lines_to_file(&path, &lines).expect("write lines");

        let content = fs::read_to_string(&path).expect("read temp file");
        assert_eq!(content, "1:New:new.example.com:19133:1777204231\r\n");

        fs::remove_file(path).expect("remove temp file");
    }
}
