use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
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

pub fn write_external_servers(
    options: &GamePathOptions,
    entries: &[ExternalServerEntry],
) -> Result<()> {
    let file_path =
        resolve_external_servers_file(options).context("无法解析 external_servers.txt 路径")?;
    write_entries_to_file(&file_path, entries)
}

pub fn add_external_server(
    options: &GamePathOptions,
    name: &str,
    address: &str,
    port: u16,
) -> Result<ExternalServerEntry> {
    let (name, address) = validate_server_input(name, address, port)?;

    let file_path =
        resolve_external_servers_file(options).context("无法解析 external_servers.txt 路径")?;
    let mut lines = read_external_server_lines(&file_path)?;
    let next_index = lines
        .iter()
        .filter_map(|line| match line {
            ExternalServerLine::Parsed { entry, .. } => Some(entry.index),
            ExternalServerLine::Raw(_) => None,
        })
        .max()
        .map_or(0, |index| index + 1);
    let line_number = lines.len() + 1;
    let entry = ExternalServerEntry {
        key: server_key(next_index, address, port),
        index: next_index,
        name: name.to_string(),
        address: address.to_string(),
        port,
        file_path: file_path.to_string_lossy().to_string(),
        line_number,
    };
    lines.push(ExternalServerLine::Parsed {
        entry: entry.clone(),
        metadata: current_unix_seconds().to_string(),
    });
    write_lines_to_file(&file_path, &lines)?;

    Ok(entry)
}

pub fn update_external_server(
    options: &GamePathOptions,
    key: &str,
    name: &str,
    address: &str,
    port: u16,
) -> Result<ExternalServerEntry> {
    let (name, address) = validate_server_input(name, address, port)?;

    let file_path =
        resolve_external_servers_file(options).context("无法解析 external_servers.txt 路径")?;
    let mut lines = read_external_server_lines(&file_path)?;
    let mut updated = None;

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
        updated = Some(entry.clone());
        break;
    }

    let Some(entry) = updated else {
        bail!("未找到服务器: {key}");
    };

    write_lines_to_file(&file_path, &lines)?;
    Ok(entry)
}

pub fn delete_external_server(options: &GamePathOptions, key: &str) -> Result<()> {
    let file_path =
        resolve_external_servers_file(options).context("无法解析 external_servers.txt 路径")?;
    let mut lines = read_external_server_lines(&file_path)?;
    let before = lines.len();
    lines.retain(|line| match line {
        ExternalServerLine::Parsed { entry, .. } => entry.key != key,
        ExternalServerLine::Raw(_) => true,
    });
    if lines.len() == before {
        bail!("未找到服务器: {key}");
    }
    write_lines_to_file(&file_path, &lines)
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
