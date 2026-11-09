//! Persistent, per-launch Minecraft process diagnostics.
//! This module deliberately does not use the launcher's own panic/Sentry reports.
use crate::utils::{file_ops, log_privacy};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tokio::sync::broadcast;
use uuid::Uuid;
use zip::write::SimpleFileOptions;

const MAX_TEXT_BYTES: u64 = 512 * 1024;
const MAX_DUMP_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitKind {
    Running,
    Normal,
    SuspectedCrash,
    ConfirmedCrash,
    RunnerFailure,
    Unknown,
}

impl ExitKind {
    pub fn is_abnormal(self) -> bool {
        matches!(self, Self::SuspectedCrash | Self::ConfirmedCrash | Self::RunnerFailure)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GameSessionReport {
    pub id: String,
    pub task_id: String,
    pub version: String,
    pub game_directory: PathBuf,
    pub platform: String,
    pub pid: u32,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub exit_code: Option<u32>,
    pub kind: ExitKind,
    pub summary: String,
    #[serde(default)]
    pub acknowledged: bool,
}

static REPORT_EVENTS: OnceLock<broadcast::Sender<GameSessionReport>> = OnceLock::new();

fn event_bus() -> &'static broadcast::Sender<GameSessionReport> {
    REPORT_EVENTS.get_or_init(|| broadcast::channel(64).0)
}

pub fn subscribe() -> broadcast::Receiver<GameSessionReport> {
    event_bus().subscribe()
}

fn root() -> PathBuf {
    file_ops::state_subdir("diagnostics/game")
}

fn report_path(id: &str) -> Result<PathBuf, String> {
    Uuid::parse_str(id).map_err(|_| "无效的诊断报告 ID".to_string())?;
    Ok(root().join(format!("{id}.json")))
}

fn save(report: &GameSessionReport) -> Result<(), String> {
    let path = report_path(&report.id)?;
    fs::create_dir_all(root()).map_err(|error| error.to_string())?;
    let temp = root().join(format!("{}.{}.tmp", report.id, Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(report).map_err(|error| error.to_string())?;
    fs::write(&temp, bytes).map_err(|error| error.to_string())?;
    if let Err(error) = fs::rename(&temp, path) {
        let _ = fs::remove_file(temp);
        return Err(error.to_string());
    }
    Ok(())
}

impl GameSessionReport {
    pub fn begin(task_id: &str, version: &str, game_directory: &Path, platform: &str, pid: u32) -> Self {
        let report = Self {
            id: Uuid::new_v4().to_string(),
            task_id: task_id.to_string(),
            version: version.to_string(),
            game_directory: game_directory.to_path_buf(),
            platform: platform.to_string(),
            pid,
            started_at: Utc::now(),
            ended_at: None,
            exit_code: None,
            kind: ExitKind::Running,
            summary: "游戏运行中".to_string(),
            acknowledged: false,
        };
        if let Err(error) = save(&report) {
            tracing::warn!(%error, session_id = %report.id, "无法保存游戏会话");
        }
        tracing::info!(session_id = %report.id, pid, platform, "游戏会话已开始");
        report
    }

    pub fn finish(mut self, result: Result<u32, String>) -> Self {
        self.ended_at = Some(Utc::now());
        match result {
            Ok(code) => {
                self.exit_code = Some(code);
                self.kind = classify_exit(self.platform.as_str(), code);
                self.summary = exit_description(self.kind, code);
            }
            Err(error) => {
                self.kind = ExitKind::Unknown;
                self.summary = format!("无法读取游戏进程的退出状态：{error}");
            }
        }
        if let Err(error) = save(&self) {
            tracing::warn!(%error, session_id = %self.id, "无法保存游戏退出记录");
        }
        tracing::info!(
            session_id = %self.id,
            pid = self.pid,
            exit_code = ?self.exit_code,
            kind = ?self.kind,
            "游戏会话结束"
        );
        if self.kind.is_abnormal() {
            let _ = event_bus().send(self.clone());
        }
        self
    }
}

pub fn classify_exit(platform: &str, code: u32) -> ExitKind {
    if code == 0 {
        ExitKind::Normal
    } else if platform == "proton" || platform == "wine" || platform == "umu" {
        // Child.wait() observes the runner, not necessarily the Minecraft child.
        ExitKind::RunnerFailure
    } else {
        ExitKind::SuspectedCrash
    }
}

pub fn exit_description(kind: ExitKind, code: u32) -> String {
    match kind {
        ExitKind::Normal => "游戏已正常退出".to_string(),
        ExitKind::RunnerFailure => format!("兼容运行器异常退出（状态码 0x{code:08X}）；尚不能确认游戏崩溃"),
        _ => {
            let detail = match code {
                0xC0000005 => "访问冲突",
                0xC000001D => "非法指令",
                0xC0000094 => "整数除零",
                0xC00000FD => "栈溢出",
                0xC0000409 => "快速失败或栈缓冲区检查失败",
                0xC0000135 => "缺少运行库或依赖组件",
                _ => "未识别的异常退出状态",
            };
            format!("游戏意外退出（0x{code:08X}，{detail}）；需要结合崩溃日志确认原因")
        }
    }
}

pub fn read_report(id: &str) -> Result<GameSessionReport, String> {
    let bytes = fs::read(report_path(id)?).map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

pub fn recent_reports(limit: usize, unacknowledged_only: bool) -> Vec<GameSessionReport> {
    let Ok(entries) = fs::read_dir(root()) else { return Vec::new() };
    let mut reports = entries.flatten().filter_map(|entry| {
        if entry.path().extension().is_none_or(|ext| ext != "json") { return None; }
        let bytes = fs::read(entry.path()).ok()?;
        let report: GameSessionReport = serde_json::from_slice(&bytes).ok()?;
        if !report.kind.is_abnormal() || (unacknowledged_only && report.acknowledged) {
            return None;
        }
        Some(report)
    }).collect::<Vec<_>>();
    reports.sort_unstable_by(|a, b| b.started_at.cmp(&a.started_at));
    reports.truncate(limit);
    reports
}

pub fn acknowledge(id: &str) -> Result<(), String> {
    let mut report = read_report(id)?;
    report.acknowledged = true;
    save(&report)
}

fn tail_bytes(path: &Path, max: u64) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    if size > max {
        use std::io::{Seek, SeekFrom};
        file.seek(SeekFrom::Start(size - max)).ok()?;
    }
    let mut bytes = Vec::new();
    file.take(max).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn text_files(report: &GameSessionReport) -> Vec<(&'static str, PathBuf)> {
    // Paths are intentionally narrow and tied to the launched version.
    vec![
        ("logs/bmcbl.log", file_ops::logs_dir().join("latest.log")),
        ("logs/bmcbl-previous.log", file_ops::logs_dir().join("previous.log")),
        ("logs/game.log", report.game_directory.join("logs/latest.log")),
        ("logs/levilamina.log", report.game_directory.join("logs/LeviLamina.log")),
        ("logs/bloader.log", report.game_directory.join("logs/BLoader.log")),
    ]
}

/// Export a bounded, redacted ZIP. No upload is ever performed here.
/// Dumps are excluded by default because they can contain credentials and world data.
pub fn export_zip(id: &str, destination: &Path, include_dump: bool) -> Result<(), String> {
    let report = read_report(id)?;
    let parent = destination.parent().ok_or("无效的导出目录")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temp = parent.join(format!(".bmcbl-crash-{}.partial", Uuid::new_v4()));
    let result = (|| -> Result<(), String> {
        let file = fs::File::create(&temp).map_err(|error| error.to_string())?;
        let mut writer = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let summary = format!(
            "BMCBL 游戏诊断\n会话：{}\n版本：{}\n平台：{}\nPID：{}\n退出状态：{}\n{}\n",
            report.id, report.version, report.platform, report.pid,
            report.exit_code.map_or_else(|| "未知".to_string(), |code| format!("0x{code:08X}")),
            report.summary
        );
        writer.start_file("summary.txt", options).map_err(|error| error.to_string())?;
        writer.write_all(log_privacy::redact_log_text(&summary).as_bytes()).map_err(|error| error.to_string())?;
        // Never export the raw private game_directory from the on-disk session JSON.
        let metadata = serde_json::json!({
            "session_id": report.id, "task_id": report.task_id, "version": report.version,
            "platform": report.platform, "pid": report.pid, "started_at": report.started_at,
            "ended_at": report.ended_at, "exit_code": report.exit_code, "kind": report.kind,
        });
        writer.start_file("session.json", options).map_err(|error| error.to_string())?;
        writer.write_all(serde_json::to_string_pretty(&metadata).map_err(|error| error.to_string())?.as_bytes())
            .map_err(|error| error.to_string())?;
        for (entry_name, path) in text_files(&report) {
            if let Some(contents) = tail_bytes(&path, MAX_TEXT_BYTES) {
                writer.start_file(entry_name, options).map_err(|error| error.to_string())?;
                writer.write_all(log_privacy::redact_log_text(&contents).as_bytes()).map_err(|error| error.to_string())?;
            }
        }
        if include_dump {
            // Only include an explicitly chosen, bounded dump from the launched version.
            // The UI must warn that minidumps cannot be reliably redacted.
            let dump = report.game_directory.join("crash.dmp");
            if dump.is_file() && dump.metadata().is_ok_and(|m| m.len() <= MAX_DUMP_BYTES) {
                writer.start_file("dumps/game.dmp", options).map_err(|error| error.to_string())?;
                let mut source = fs::File::open(&dump).map_err(|error| error.to_string())?;
                std::io::copy(&mut source, &mut writer).map_err(|error| error.to_string())?;
            }
        }
        writer.finish().map_err(|error| error.to_string())?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temp, destination) {
        let _ = fs::remove_file(&temp);
        return Err(error.to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_runner_failures_from_native_game_crashes() {
        assert_eq!(classify_exit("gdk", 0), ExitKind::Normal);
        assert_eq!(classify_exit("gdk", 0xC0000005), ExitKind::SuspectedCrash);
        assert_eq!(classify_exit("proton", 1), ExitKind::RunnerFailure);
    }

    #[test]
    fn report_identifiers_cannot_traverse_directories() {
        assert!(report_path("../../settings.toml").is_err());
        assert!(report_path(&Uuid::new_v4().to_string()).is_ok());
    }
}
