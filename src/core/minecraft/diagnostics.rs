//! Persistent, per-launch Minecraft process diagnostics.
//! This module deliberately does not use the launcher's own panic/Sentry reports.
use crate::utils::{file_ops, log_privacy};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tokio::sync::broadcast;
use uuid::Uuid;
use zip::write::SimpleFileOptions;

const MAX_TEXT_BYTES: u64 = 512 * 1024;
const MAX_DUMP_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct CrashExportOptions {
    pub include_launcher: bool,
    pub include_game: bool,
    pub include_extensions: bool,
    pub include_windows: bool,
    pub include_dumps: bool,
    pub redact_text: bool,
}

impl Default for CrashExportOptions {
    fn default() -> Self {
        Self {
            include_launcher: true,
            include_game: true,
            include_extensions: true,
            include_windows: true,
            include_dumps: false,
            redact_text: true,
        }
    }
}

impl CrashExportOptions {
    pub fn includes_any(self) -> bool {
        self.include_launcher || self.include_game || self.include_extensions
            || self.include_windows || self.include_dumps
    }

    pub fn needs_confirmation(self) -> bool {
        !self.redact_text || self.include_dumps
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EvidenceCategory {
    Launcher,
    Game,
    Extension,
}

fn evidence_category(name: &str) -> EvidenceCategory {
    if name == "logs/launch-task.log" {
        EvidenceCategory::Launcher
    } else if name == "logs/game.log" {
        EvidenceCategory::Game
    } else {
        EvidenceCategory::Extension
    }
}

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
    #[serde(default)]
    pub process_created_at: Option<u64>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub exit_code: Option<u32>,
    #[serde(default)]
    pub exit_signal: Option<i32>,
    pub kind: ExitKind,
    pub summary: String,
    #[serde(default)]
    pub acknowledged: bool,
    #[serde(default)]
    pub wer_fault: Option<WerFault>,
}

static REPORT_EVENTS: OnceLock<broadcast::Sender<GameSessionReport>> = OnceLock::new();
static ACTIVE_SESSIONS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static REPORT_WRITE_LOCK: Mutex<()> = Mutex::new(());

fn sessions() -> &'static Mutex<HashSet<String>> {
    ACTIVE_SESSIONS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Used by the hidden-window policy to avoid closing the monitor while any
/// launched Minecraft instance remains alive.
pub fn active_sessions() -> usize {
    sessions().lock().map_or(0, |active| active.len())
}

fn event_bus() -> &'static broadcast::Sender<GameSessionReport> {
    REPORT_EVENTS.get_or_init(|| broadcast::channel(64).0)
}

pub fn subscribe() -> broadcast::Receiver<GameSessionReport> {
    event_bus().subscribe()
}

#[cfg(target_os = "windows")]
fn process_creation_filetime(pid: u32) -> Option<u64> {
    use windows::Win32::Foundation::{CloseHandle, FILETIME};
    use windows::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: The handle belongs to the current process and is closed on all paths.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        let result = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user)
            .ok()
            .map(|_| ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64);
        let _ = CloseHandle(handle);
        result
    }
}

fn root() -> PathBuf {
    file_ops::state_subdir("diagnostics/game")
}

fn report_path(id: &str) -> Result<PathBuf, String> {
    Uuid::parse_str(id).map_err(|_| "无效的诊断报告 ID".to_string())?;
    Ok(root().join(format!("{id}.json")))
}

/// Caller holds REPORT_WRITE_LOCK; merge immutable crash evidence so a late
/// acknowledgement cannot overwrite a more recent WER classification.
fn save_locked(report: &GameSessionReport) -> Result<(), String> {
    let path = report_path(&report.id)?;
    fs::create_dir_all(root()).map_err(|error| error.to_string())?;
    let mut latest = report.clone();
    if let Ok(existing) = read_report(&report.id) {
        latest.acknowledged |= existing.acknowledged;
        if latest.wer_fault.is_none() && existing.wer_fault.is_some() {
            latest.wer_fault = existing.wer_fault;
            latest.kind = existing.kind;
            latest.summary = existing.summary;
        }
    }
    let temp = root().join(format!("{}.{}.tmp", report.id, Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(&latest).map_err(|error| error.to_string())?;
    fs::write(&temp, bytes).map_err(|error| error.to_string())?;
    if let Err(error) = fs::rename(&temp, path) {
        let _ = fs::remove_file(temp);
        return Err(error.to_string());
    }
    Ok(())
}

fn save(report: &GameSessionReport) -> Result<(), String> {
    let _guard = REPORT_WRITE_LOCK.lock().map_err(|e| e.to_string())?;
    save_locked(report)
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
            process_created_at: {
                #[cfg(target_os = "windows")]
                { process_creation_filetime(pid) }
                #[cfg(not(target_os = "windows"))]
                { None }
            },
            started_at: Utc::now(),
            ended_at: None,
            exit_code: None,
            exit_signal: None,
            kind: ExitKind::Running,
            summary: "游戏运行中".to_string(),
            acknowledged: false,
            wer_fault: None,
        };
        if let Err(error) = save(&report) {
            tracing::warn!(%error, session_id = %report.id, "无法保存游戏会话");
        }
        if let Ok(mut active) = sessions().lock() { active.insert(report.id.clone()); }
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
        if let Some(signal) = self.exit_signal {
            self.kind = ExitKind::RunnerFailure;
            self.summary = format!(
                "Linux Proton/Wine 运行器收到信号 {signal} 后退出；无法单凭包装进程的信号确认 Minecraft 崩溃"
            );
        }
        if let Err(error) = save(&self) {
            tracing::warn!(%error, session_id = %self.id, "无法保存游戏退出记录");
        }
        // Preserve task-specific launch lines before the launch task is evicted
        // or latest.log is rotated. This is local-only, never sent automatically.
        let lines = crate::tasks::task_manager::task_logs(&self.task_id);
        if !lines.is_empty() {
            let contents = lines.iter().map(|line| line.as_ref()).collect::<Vec<_>>().join("\n");
            let path = root().join(format!("{}.task.log", self.id));
            if let Err(error) = fs::write(path, log_privacy::redact_log_text(&contents).as_bytes()) {
                tracing::warn!(%error, session_id = %self.id, "无法保存游戏启动任务日志");
            }
        }
        // Freeze log evidence at process exit. Reading live latest.log only
        // when the user later exports can silently capture another session.
        snapshot_text_evidence(&self);
        tracing::info!(
            session_id = %self.id,
            pid = self.pid,
            exit_code = ?self.exit_code,
            kind = ?self.kind,
            "游戏会话结束"
        );
        if let Ok(mut active) = sessions().lock() { active.remove(&self.id); }
        // Normal exits are events too, so a launcher hidden for crash monitoring
        // can terminate once its final game session exits.
        let _ = event_bus().send(self.clone());
        self
    }
}

/// Windows Error Reporting data accepted only after a PID + timestamp match.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WerFault {
    pub event_at: DateTime<Utc>,
    pub module: String,
    pub exception_code: String,
}

#[cfg(target_os = "windows")]
fn match_application_error(xml: &str, report: &GameSessionReport) -> Option<WerFault> {
    use regex::Regex;
    let events = Regex::new(r"(?s)<Event(?:\s|>).*?</Event>").ok()?;
    let data = |block: &str, key: &str| -> Option<String> {
        let regex = Regex::new(&format!(r#"<Data Name="{}">([^<]*)</Data>"#, regex::escape(key))).ok()?;
        regex.captures(block).map(|c| c[1].to_string())
    };
    let event_time = Regex::new(r#"TimeCreated[^>]*SystemTime="([^"]+)""#).ok()?;
    for found in events.find_iter(xml) {
        let block = found.as_str();
        if !block.contains("<EventID>1000</EventID>") { continue; }
        let Some(pid_text) = data(block, "ProcessId") else { continue };
        let actual_pid = if pid_text.starts_with("0x") || pid_text.starts_with("0X") {
            u32::from_str_radix(&pid_text[2..], 16).ok()
        } else {
            pid_text.parse::<u32>().ok()
        };
        let Some(actual_pid) = actual_pid else { continue };
        if actual_pid != report.pid { continue; }
        // An abandoned session can be confirmed only by exact process
        // generation, not by PID/time proximity. Windows may reuse PIDs.
        match (report.process_created_at, data(block, "AppStartTime")) {
            (Some(expected), Some(app_start)) => {
                let parsed = if app_start.starts_with("0x") || app_start.starts_with("0X") {
                    u64::from_str_radix(&app_start[2..], 16).ok()
                } else {
                    app_start.parse::<u64>().ok()
                };
                if parsed != Some(expected) { continue; }
            }
            (None, _) | (Some(_), None) if report.kind == ExitKind::Unknown => {
                continue;
            }
            _ => {}
        }
        let app_name = data(block, "AppName").unwrap_or_default().to_lowercase();
        if !app_name.contains("minecraft") { continue; }
        let Some(timestamp) = event_time.captures(block)
            .and_then(|c| DateTime::parse_from_rfc3339(&c[1]).ok())
            .map(|value| value.with_timezone(&Utc)) else {
                continue;
            };
        if timestamp < report.started_at - chrono::Duration::seconds(30) { continue; }
        if report.ended_at.is_some_and(|end| timestamp > end + chrono::Duration::seconds(120)) {
            continue;
        }
        return Some(WerFault {
            event_at: timestamp,
            module: data(block, "ModuleName").unwrap_or_else(|| "unknown".to_string()),
            exception_code: data(block, "ExceptionCode").unwrap_or_else(|| "unknown".to_string()),
        });
    }
    None
}

/// Query a bounded number of Application Error records without changing Windows
/// registry settings. A missing/inaccessible event log is not a fatal error.
#[cfg(target_os = "windows")]
pub async fn enrich_windows_wer(mut report: GameSessionReport) -> GameSessionReport {
    if report.kind != ExitKind::SuspectedCrash && report.kind != ExitKind::Unknown {
        return report;
    }
    // WER often writes Event 1000 after the crashed process has exited.
    // Bounded retries avoid permanently missing a late event without polling at
    // frame rate. Every probe is outside the GPUI thread.
    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
        let mut command = tokio::process::Command::new("wevtutil.exe");
        command.args([
            "qe", "Application",
            "/q:*[System[(EventID=1000)]]",
            "/c:64", "/rd:true", "/f:xml",
        ]).kill_on_drop(true);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5), command.output()
        ).await;
        if let Ok(Ok(output)) = result
            && output.status.success()
            && let Some(wer) = match_application_error(
                &String::from_utf8_lossy(&output.stdout), &report
            )
        {
            report.kind = ExitKind::ConfirmedCrash;
            report.summary = format!(
                "Windows 确认 Minecraft 发生崩溃：{}（故障模块 {}）",
                wer.exception_code, wer.module
            );
            report.wer_fault = Some(wer);
            if let Err(error) = save(&report) {
                tracing::warn!(%error, session_id = %report.id, "无法保存 Windows WER 证据");
            }
            // Respect an acknowledgement that may have happened while WER
            // was being written. Never reopen dismissed reports.
            if let Ok(saved) = read_report(&report.id) {
                report.acknowledged = saved.acknowledged;
            }
            if !report.acknowledged {
                let _ = event_bus().send(report.clone());
            }
            break;
        }
    }
    report
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

/// Reconcile sessions left as Running when a previous launcher instance ended
/// unexpectedly. This never calls a missing process a "game crash": only WER
/// evidence can later promote it from Unknown to ConfirmedCrash.
pub fn recover_interrupted_sessions() -> Vec<GameSessionReport> {
    let Ok(entries) = fs::read_dir(root()) else { return Vec::new() };
    let mut pending = Vec::new();
    for entry in entries.flatten().take(256) {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        if !fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file()) {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else { continue };
        let Ok(mut report) = serde_json::from_slice::<GameSessionReport>(&bytes) else { continue };
        if report.kind != ExitKind::Running { continue }
        if sessions().lock().is_ok_and(|active| active.contains(&report.id)) {
            continue;
        }
        #[cfg(target_os = "windows")]
        {
            let running = match (report.process_created_at, process_creation_filetime(report.pid)) {
                (Some(expected), Some(actual)) => expected == actual,
                // Without original creation time, do not claim a live process
                // is another one just because its PID has been recycled.
                (None, Some(_)) => true,
                _ => false,
            };
            if running { continue; }
        }
        #[cfg(target_os = "linux")]
        if Path::new("/proc").join(report.pid.to_string()).exists() {
            continue;
        }
        report.ended_at = Some(Utc::now()); // Reconciliation time, NOT observed exit time.
        report.kind = ExitKind::Unknown;
        report.summary =
            "上次启动器在游戏会话结束前关闭，无法确认游戏是正常退出还是崩溃；需要系统事件证据".to_string();
        if let Err(error) = save(&report) {
            tracing::warn!(%error, session_id=%report.id, "无法恢复中断的游戏会话");
        }
        pending.push(report);
    }
    pending
}

pub fn recent_reports(limit: usize, unacknowledged_only: bool) -> Vec<GameSessionReport> {
    let Ok(entries) = fs::read_dir(root()) else { return Vec::new() };
    let mut reports = entries.flatten().filter_map(|entry| {
        if entry.path().extension().is_none_or(|ext| ext != "json") { return None; }
        let bytes = fs::read(entry.path()).ok()?;
        let report: GameSessionReport = serde_json::from_slice(&bytes).ok()?;
        // Unknown sessions remain inspectable in history but must not
        // trigger an automatic game-crash popup without supporting evidence.
        if !(report.kind.is_abnormal() || (!unacknowledged_only && report.kind == ExitKind::Unknown))
            || (unacknowledged_only && report.acknowledged)
        {
            return None;
        }
        Some(report)
    }).collect::<Vec<_>>();
    reports.sort_unstable_by(|a, b| b.started_at.cmp(&a.started_at));
    reports.truncate(limit);
    reports
}

pub fn acknowledge(id: &str) -> Result<(), String> {
    let _guard = REPORT_WRITE_LOCK.lock().map_err(|e| e.to_string())?;
    let mut report = read_report(id)?;
    report.acknowledged = true;
    save_locked(&report)
}

fn tail_bytes(path: &Path, max: u64) -> Option<String> {
    // Never follow symlinks from a mod-controlled log directory into private files.
    if !fs::symlink_metadata(path).ok()?.file_type().is_file() {
        return None;
    }
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

/// Bounded evidence provider discovery; never traverse world saves or resource packs.
fn text_files(report: &GameSessionReport) -> Vec<(String, PathBuf)> {
    let mut files = vec![
        // Global latest.log may already belong to another Minecraft session.
        // Export only the launch task snapshot tied to this report ID.
        ("logs/launch-task.log".to_string(), root().join(format!("{}.task.log", report.id))),
        ("logs/game.log".to_string(), report.game_directory.join("logs/latest.log")),
        ("logs/levilamina.log".to_string(), report.game_directory.join("logs/LeviLamina.log")),
        ("logs/bloader.log".to_string(), report.game_directory.join("logs/BLoader.log")),
    ];
    let directories = [
        report.game_directory.join("crash-logs"),
        report.game_directory.join("logs/crash"),
        report.game_directory.join("LeviLamina/logs"),
        report.game_directory.join("Minecraft Bedrock/logs"),
        report.game_directory.join("Minecraft Bedrock/CrashLogs"),
        report.game_directory.join("BLoader/logs"),
        file_ops::logs_dir().join("proton"),
    ];
    let mut number = 0usize;
    for dir in directories {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.take(128).flatten() {
            if number >= 48 { return files; }
            let path = entry.path();
            let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else { continue };
            if !matches!(ext.to_ascii_lowercase().as_str(), "log" | "txt" | "json") { continue }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else { continue };
            let name = name.to_ascii_lowercase();
            if !name.contains("crash") && !name.contains("levilamina") && !name.contains("bloader") && !name.contains("proton") {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(&path) else { continue };
            if !meta.file_type().is_file() { continue }
            let Some(modified) = meta.modified().ok().map(DateTime::<Utc>::from) else { continue };
            if modified < report.started_at - chrono::Duration::minutes(5) { continue; }
            if report.ended_at.is_some_and(|end| modified > end + chrono::Duration::minutes(10)) {
                continue;
            }
            files.push((format!("logs/provider-{number:02}.{ext}"), path));
            number += 1;
        }
    }
    files
}

fn evidence_dir(report: &GameSessionReport) -> PathBuf {
    root().join(format!("{}-evidence", report.id))
}

/// Snapshot only bounded, redacted text during the session's exit processing.
/// Never store absolute source paths in the exported evidence manifest.
fn snapshot_text_evidence(report: &GameSessionReport) {
    let directory = evidence_dir(report);
    // Retain the original bounded evidence locally. Redaction is performed
    // when exporting; an already-redacted snapshot could never provide a
    // truthful "raw text" option later. No network upload is performed.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::create_dir_all(&directory).is_ok() {
            let _ = fs::set_permissions(&directory, fs::Permissions::from_mode(0o700));
        }
    }
    let mut included = Vec::<String>::new();
    for (name, path) in text_files(report) {
        if name != "logs/launch-task.log" {
            let Some(modified) = fs::symlink_metadata(&path).ok()
                .and_then(|meta| meta.modified().ok())
                .map(DateTime::<Utc>::from) else { continue };
            if modified < report.started_at - chrono::Duration::minutes(5)
                || report.ended_at.is_some_and(|end| {
                    modified > end + chrono::Duration::minutes(5)
                })
            {
                continue;
            }
        }
        let Some(contents) = tail_bytes(&path, MAX_TEXT_BYTES) else { continue };
        let target = directory.join(&name);
        if let Some(parent) = target.parent()
            && fs::create_dir_all(parent).is_err()
        {
            continue;
        }
        // The source is already bounded by MAX_TEXT_BYTES. The raw snapshot
        // stays in the application's local diagnostics store.
        if fs::write(&target, contents.as_bytes()).is_ok() {
            included.push(name);
        }
    }
    // The manifest is written last: a partial capture never falsely claims
    // that missing log entries were preserved.
    if let Ok(data) = serde_json::to_vec(&included) {
        let _ = fs::create_dir_all(&directory);
        if let Err(error) = fs::write(directory.join("manifest.json"), data) {
            tracing::warn!(%error, session_id=%report.id, "无法保存游戏诊断证据清单");
        }
    }
}

fn frozen_text_files(report: &GameSessionReport) -> Vec<(String, PathBuf)> {
    let directory = evidence_dir(report);
    if let Ok(data) = fs::read(directory.join("manifest.json"))
        && let Ok(names) = serde_json::from_slice::<Vec<String>>(&data)
    {
        return names.into_iter().take(64).filter_map(|name| {
            let valid = name.starts_with("logs/")
                && !name.contains('\\')
                && name.split('/').all(|segment| {
                    !segment.is_empty() && segment != "." && segment != ".."
                });
            valid.then(|| {
                let path = directory.join(&name);
                (name, path)
            })
        }).collect();
    }
    // Pre-upgrade reports did not freeze all providers. Only the per-task
    // snapshot remains safe to export; never fallback to live game logs.
    let task = root().join(format!("{}.task.log", report.id));
    if task.is_file() {
        vec![("logs/launch-task.log".to_string(), task)]
    } else {
        Vec::new()
    }
}

fn dump_candidates(report: &GameSessionReport) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let mut dirs = vec![
        report.game_directory.clone(),
        report.game_directory.join("crash-logs"),
        report.game_directory.join("logs"),
        report.game_directory.join("LeviLamina/logs"),
    ];
    #[cfg(target_os = "windows")]
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local_app_data).join("CrashDumps"));
    }
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else { continue };
        for entry in entries.take(128).flatten() {
            let path = entry.path();
            if !path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("dmp")) { continue }
            let Ok(meta) = fs::symlink_metadata(&path) else { continue };
            if !meta.file_type().is_file() || meta.len() > MAX_DUMP_BYTES { continue }
            let Some(modified) = meta.modified().ok().map(DateTime::<Utc>::from) else { continue };
            if modified < report.started_at - chrono::Duration::minutes(5) { continue }
            if report.ended_at.is_some_and(|end| modified > end + chrono::Duration::minutes(10)) {
                continue;
            }
            // Global CrashDumps can contain other Minecraft instances.
            if path.parent().is_some_and(|parent| parent.ends_with("CrashDumps")) {
                let Some(file_name) = path.file_name() else { continue };
                let name = file_name.to_string_lossy().to_ascii_lowercase();
                if !name.contains("minecraft") || !name.contains(&report.pid.to_string()) {
                    continue;
                }
            }
            candidates.push(path);
            if candidates.len() >= 4 { return candidates; }
        }
    }
    candidates
}

/// Export the user-selected evidence categories to a local ZIP.
///
/// Every text source comes from the immutable per-session snapshot, not from
/// a rotating global latest.log. The default redact_text=true scrubs on export.
/// No automatic upload is performed. DMPs are never text-redacted.
pub fn export_zip_with_options(
    id: &str,
    destination: &Path,
    options: CrashExportOptions,
) -> Result<(), String> {
    if !options.includes_any() {
        return Err("请选择至少一类诊断材料".to_string());
    }
    let report = read_report(id)?;
    let parent = destination.parent().ok_or("无效的导出目录")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    if destination.exists() {
        return Err("目标 ZIP 已存在，请另选文件名以免覆盖现有诊断材料".to_string());
    }
    let temp = parent.join(format!(".bmcbl-crash-{}.partial", Uuid::new_v4()));
    let result = (|| -> Result<(), String> {
        let file = fs::File::create(&temp).map_err(|error| error.to_string())?;
        let mut writer = zip::ZipWriter::new(file);
        let compression = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let text = |value: &str| -> String {
            if options.redact_text {
                log_privacy::redact_log_text(value).into_owned()
            } else {
                value.to_string()
            }
        };
        let summary = format!(
            "BMCBL 游戏诊断\n会话：{}\n版本：{}\n平台：{}\nPID：{}\n退出状态：{}\n{}\n文本脱敏：{}\n",
            report.id, report.version, report.platform, report.pid,
            report.exit_code.map_or_else(|| "未知".to_string(), |code| format!("0x{code:08X}")),
            report.summary,
            if options.redact_text { "开启" } else { "关闭（用户主动选择）" },
        );
        writer.start_file("summary.txt", compression).map_err(|error| error.to_string())?;
        writer.write_all(text(&summary).as_bytes()).map_err(|error| error.to_string())?;

        // Source directory is intentionally never serialized into the ZIP.
        let metadata = serde_json::json!({
            "session_id": report.id, "task_id": report.task_id, "version": report.version,
            "platform": report.platform, "pid": report.pid,
            "process_created_at": report.process_created_at,
            "started_at": report.started_at, "ended_at": report.ended_at,
            "exit_code": report.exit_code, "exit_signal": report.exit_signal,
            "kind": report.kind,
        });
        writer.start_file("session.json", compression).map_err(|error| error.to_string())?;
        let json = serde_json::to_string_pretty(&metadata).map_err(|error| error.to_string())?;
        writer.write_all(text(&json).as_bytes()).map_err(|error| error.to_string())?;

        if options.include_windows {
            if let Some(wer) = &report.wer_fault {
                writer.start_file("windows/application-error.json", compression)
                    .map_err(|error| error.to_string())?;
                let raw = serde_json::to_string_pretty(wer).map_err(|error| error.to_string())?;
                writer.write_all(text(&raw).as_bytes()).map_err(|error| error.to_string())?;
            }
        }

        let sources = frozen_text_files(&report)
            .into_iter()
            .filter(|(name, _)| match evidence_category(name) {
                EvidenceCategory::Launcher => options.include_launcher,
                EvidenceCategory::Game => options.include_game,
                EvidenceCategory::Extension => options.include_extensions,
            })
            .collect::<Vec<_>>();
        let mut manifest = Vec::with_capacity(sources.len());
        for (entry_name, path) in sources {
            let Some(contents) = tail_bytes(&path, MAX_TEXT_BYTES) else {
                manifest.push(serde_json::json!({
                    "archive_path": entry_name, "included": false,
                    "reason": "会话快照不可读取"
                }));
                continue;
            };
            writer.start_file(entry_name.as_str(), compression)
                .map_err(|error| error.to_string())?;
            writer.write_all(text(&contents).as_bytes()).map_err(|error| error.to_string())?;
            manifest.push(serde_json::json!({
                "archive_path": entry_name, "included": true,
                "text_redacted": options.redact_text
            }));
        }
        writer.start_file("evidence.json", compression).map_err(|error| error.to_string())?;
        let manifest_text = serde_json::to_string_pretty(&manifest)
            .map_err(|error| error.to_string())?;
        writer.write_all(text(&manifest_text).as_bytes())
            .map_err(|error| error.to_string())?;

        if options.include_dumps {
            // Dumps may contain secrets and cannot be safely redacted.
            // UI requires an explicit opt-in and a separate warning.
            for (index, dump) in dump_candidates(&report).iter().enumerate() {
                writer.start_file(format!("dumps/game-{index}.dmp"), compression)
                    .map_err(|error| error.to_string())?;
                let mut source = fs::File::open(dump).map_err(|error| error.to_string())?;
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
    if destination.exists() {
        let _ = fs::remove_file(&temp);
        return Err("目标 ZIP 已存在，请另选文件名以免覆盖现有诊断材料".to_string());
    }
    if let Err(error) = fs::rename(&temp, destination) {
        let _ = fs::remove_file(&temp);
        return Err(error.to_string());
    }
    Ok(())
}

/// Keep the original API working for existing call sites.
pub fn export_zip(id: &str, destination: &Path, include_dump: bool) -> Result<(), String> {
    export_zip_with_options(
        id,
        destination,
        CrashExportOptions {
            include_dumps: include_dump,
            ..CrashExportOptions::default()
        },
    )
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

    #[cfg(target_os = "windows")]
    #[test]
    fn wer_match_requires_pid_and_creation_time_for_recovered_session() {
        let now = Utc::now();
        let mut report = GameSessionReport {
            id: Uuid::new_v4().to_string(),
            task_id: "task-1".to_string(),
            version: "1.26.51".to_string(),
            game_directory: PathBuf::from("C:/game"),
            platform: "gdk".to_string(),
            pid: 0x112A,
            process_created_at: Some(0xABCD),
            started_at: now - chrono::Duration::seconds(8),
            ended_at: Some(now),
            exit_code: None,
            exit_signal: None,
            kind: ExitKind::Unknown,
            summary: "未确认".to_string(),
            acknowledged: false,
            wer_fault: None,
        };
        let event = |process_id: &str, app_start: &str| format!(
            r#"<Event><System><EventID>1000</EventID><TimeCreated SystemTime="{}"/></System><EventData><Data Name="ProcessId">{process_id}</Data><Data Name="AppStartTime">{app_start}</Data><Data Name="AppName">Minecraft.Windows.exe</Data><Data Name="ModuleName">example.dll</Data><Data Name="ExceptionCode">c0000005</Data></EventData></Event>"#,
            now.to_rfc3339()
        );
        assert!(match_application_error(&event("0x112a", "0xabcd"), &report).is_some());
        assert!(match_application_error(&event("0x112b", "0xabcd"), &report).is_none());
        assert!(match_application_error(&event("0x112a", "0xff00"), &report).is_none());
        assert!(match_application_error(&event("0x112a", "invalid"), &report).is_none());
        report.process_created_at = None;
        assert!(match_application_error(&event("0x112a", "0xabcd"), &report).is_none());
    }

    #[test]
    fn export_options_are_private_by_default() {
        let options = CrashExportOptions::default();
        assert!(options.redact_text);
        assert!(!options.include_dumps);
        assert!(!options.needs_confirmation());
        assert_eq!(evidence_category("logs/launch-task.log"), EvidenceCategory::Launcher);
        assert_eq!(evidence_category("logs/game.log"), EvidenceCategory::Game);
        assert_eq!(evidence_category("logs/provider-00.log"), EvidenceCategory::Extension);
    }

    #[test]
    fn risky_export_requires_confirmation() {
        let options = CrashExportOptions { redact_text: false, ..Default::default() };
        assert!(options.needs_confirmation());
        let options = CrashExportOptions { include_dumps: true, ..Default::default() };
        assert!(options.needs_confirmation());
    }

    #[test]
    fn report_identifiers_cannot_traverse_directories() {
        assert!(report_path("../../settings.toml").is_err());
        assert!(report_path(&Uuid::new_v4().to_string()).is_ok());
    }
}
