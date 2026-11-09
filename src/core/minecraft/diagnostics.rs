//! Persistent, per-launch Minecraft process diagnostics.
//! This module deliberately does not use the launcher's own panic/Sentry reports.
use crate::utils::{file_ops, log_privacy};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tokio::sync::broadcast;
use uuid::Uuid;
use zip::write::SimpleFileOptions;

pub mod analysis;
pub mod forensics;
pub use analysis::{CrashFinding, CrashReason, Confidence, EvidenceSource};
const GAME_DIAGNOSTICS_SCHEMA: u32 = 6;
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
            include_windows: cfg!(target_os = "windows"),
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
    pub schema_version: u32,
    pub id: String,
    pub task_id: String,
    pub version: String,
    pub game_directory: PathBuf,
    pub platform: String,
    pub pid: u32,
    pub process_created_at: Option<u64>,
    pub compatibility_prefix: Option<PathBuf>,
    pub compatibility_logs: Option<PathBuf>,
    pub compatibility_fault: Option<CompatibilityFault>,
    pub early_exit: bool,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub exit_code: Option<u32>,
    pub exit_signal: Option<i32>,
    pub kind: ExitKind,
    pub summary: String,
    pub acknowledged: bool,
    pub wer_fault: Option<WerFault>,
    pub findings: Vec<CrashFinding>,
    pub forensics: forensics::ForensicSummary,
    pub log_offsets: HashMap<String, u64>,
}

/// Wine and Proton diagnostics do not establish which Windows guest process
/// emitted the error; the observed process is still the native runner.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompatibilityFault {
    WineUnhandledException,
    WineDependencyFailure,
    RuntimeFailure,
}

pub fn classify_compatibility_output(text: &str) -> Option<CompatibilityFault> {
    let text = text.to_ascii_lowercase();
    if ["unhandled page fault", "unhandled exception", "wine: unhandled",
        "wine: assertion failed"].iter().any(|term| text.contains(term)) {
        Some(CompatibilityFault::WineUnhandledException)
    } else if ["err:module:import_dll", "unimplemented function",
        "library not found", "module not found"].iter().any(|term| text.contains(term)) {
        Some(CompatibilityFault::WineDependencyFailure)
    } else if ["wineserver: failed", "failed to initialize wine",
        "proton: fatal", "pressure-vessel-wrap: error"]
        .iter().any(|term| text.contains(term)) {
        Some(CompatibilityFault::RuntimeFailure)
    } else {
        None
    }
}

#[cfg(target_os = "linux")]
fn parse_linux_proc_starttime(stat: &str) -> Option<u64> {
    // Linux /proc/PID/stat field 2 may contain whitespace and parentheses.
    // Field 22 is the starttime tick counter (index 19 after comm).
    stat.rsplit_once(')')?.1.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(target_os = "linux")]
fn process_creation_ticks(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_linux_proc_starttime(&stat)
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
            latest.findings = existing.findings;
            latest.forensics = existing.forensics;
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
    pub fn begin(
        task_id: &str, version: &str, game_directory: &Path,
        platform: &str, pid: u32, compatibility_prefix: Option<PathBuf>,
        compatibility_logs: Option<PathBuf>,
    ) -> Self {
        let report = Self {
            schema_version: GAME_DIAGNOSTICS_SCHEMA,
            id: Uuid::new_v4().to_string(),
            task_id: task_id.to_string(),
            version: version.to_string(),
            game_directory: game_directory.to_path_buf(),
            platform: platform.to_string(),
            pid,
            process_created_at: {
                #[cfg(target_os = "windows")]
                { process_creation_filetime(pid) }
                #[cfg(target_os = "linux")]
                { process_creation_ticks(pid) }
            },
            compatibility_prefix,
            compatibility_logs,
            compatibility_fault: None,
            early_exit: false,
            started_at: Utc::now(),
            ended_at: None,
            exit_code: None,
            exit_signal: None,
            kind: ExitKind::Running,
            summary: "游戏运行中".to_string(),
            acknowledged: false,
            wer_fault: None,
            findings: Vec::new(),
            forensics: forensics::ForensicSummary::default(),
            log_offsets: initial_log_offsets(game_directory),
        };
        if let Err(error) = save(&report) {
            tracing::warn!(%error, session_id = %report.id, "无法保存游戏会话");
        }
        if let Ok(mut active) = sessions().lock() { active.insert(report.id.clone()); }
        tracing::info!(session_id = %report.id, pid, platform, "游戏会话已开始");
        report
    }

    pub fn finish_compatibility(
        mut self, result: Result<u32, String>, output: &str, early_exit: bool,
    ) -> Self {
        self.early_exit = early_exit;
        let mut start = output.len().saturating_sub(MAX_TEXT_BYTES as usize);
        while !output.is_char_boundary(start) {
            start += 1;
        }
        let tail = &output[start..];
        self.compatibility_fault = self.compatibility_fault.or_else(|| classify_compatibility_output(tail));
        if !tail.is_empty() {
            if let Err(error) = fs::write(
                root().join(format!("{}.runner.log", self.id)), tail.as_bytes()
            ) {
                tracing::warn!(%error, session_id=%self.id, "无法保存兼容运行器输出");
            }
        }
        self.finish(result)
    }

    pub fn finish(mut self, result: Result<u32, String>) -> Self {
        self.ended_at = Some(Utc::now());
        match result {
            Ok(code) => {
                self.exit_code = Some(code);
                self.kind = classify_exit(self.platform.as_str(), code);
                self.summary = exit_description(self.kind, code);
                // The native Wine/Proton/UMU command can exit zero without
                // proving that the Windows Minecraft guest exited cleanly.
                if self.compatibility_prefix.is_some() && code == 0 {
                    self.summary =
                        "Linux 兼容运行器已结束（状态码 0）；未独立确认 Minecraft 客户端的退出状态"
                            .to_string();
                }
            }
            Err(error) => {
                self.kind = ExitKind::Unknown;
                let role = if self.compatibility_prefix.is_some() { "兼容运行器" } else { "游戏进程" };
                self.summary = format!("无法读取{role}的退出状态：{error}");
            }
        }
        if let Some(signal) = self.exit_signal {
            self.kind = ExitKind::RunnerFailure;
            self.summary = format!(
                "Linux 兼容运行器收到信号 {signal} 后退出，不能据此确认 Minecraft 本体崩溃"
            );
        }
        if let Some(fault) = self.compatibility_fault {
            match fault {
                CompatibilityFault::WineUnhandledException => {
                    self.kind = ExitKind::SuspectedCrash;
                    self.summary = "Wine 记录了未处理异常，疑似游戏或兼容层子进程崩溃，尚未确认具体进程".to_string();
                }
                CompatibilityFault::WineDependencyFailure => {
                    // Wine can log optional DLL failures even during a healthy
                    // session. Do not elevate a zero-exit runner to a crash.
                    if self.exit_code != Some(0) || self.early_exit {
                        self.kind = ExitKind::RunnerFailure;
                        self.summary = "Wine/Proton 报告依赖库或未实现函数错误".to_string();
                    }
                }
                CompatibilityFault::RuntimeFailure => {
                    if self.exit_code != Some(0) || self.early_exit {
                        self.kind = ExitKind::RunnerFailure;
                        self.summary = "Proton/UMU/Wine 运行环境发生错误".to_string();
                    }
                }
            }
        }
        if self.early_exit && self.kind == ExitKind::Normal {
            self.kind = ExitKind::Unknown;
            self.summary = "兼容运行器在启动检测期退出（状态码 0），无法确认游戏已启动".to_string();
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
        // Freeze evidence only for abnormal sessions. Normal play should not
        // create unnecessary raw log copies or increase persistent disk usage.
        if self.kind.is_abnormal() || self.early_exit {
            snapshot_text_evidence(&self);
            let (findings, evidence) = analyze_snapshot(&self);
            self.findings = findings;
            self.forensics = evidence;
            if let Err(error) = save(&self) {
                tracing::warn!(%error, session_id=%self.id, "无法保存游戏崩溃自动分析");
            }
        }
        // Proton can write large direct logs. Only bounded evidence snapshots
        // are retained. Never remove arbitrary directories supplied by a
        // manually edited report JSON.
        if let Some(logs) = self.compatibility_logs.as_ref()
            && logs.parent() == Some(root().join("compat-logs").as_path())
            && logs.file_name().and_then(|name| name.to_str())
                .is_some_and(|name| Uuid::parse_str(name).is_ok())
        {
            // Proton crash-report directories may contain minidumps, which
            // cannot be safely text-redacted. Keep bounded dumps available
            // for explicit opt-in export; delete ordinary verbose logs once
            // frozen so they do not accumulate after normal play.
            let contains_dump = fs::read_dir(logs.join("crash-reports"))
                .ok().is_some_and(|entries| {
                    entries.flatten().take(128).any(|entry| {
                        let path = entry.path();
                        path.extension().and_then(|ext| ext.to_str()).is_some_and(
                            |ext| ext.eq_ignore_ascii_case("dmp")
                                || ext.eq_ignore_ascii_case("mdmp")
                        ) && fs::symlink_metadata(&path).is_ok_and(
                            |meta| meta.file_type().is_file() && meta.len() <= MAX_DUMP_BYTES
                        )
                    })
                });
            if !contains_dump {
                if let Err(error) = fs::remove_dir_all(logs)
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    tracing::warn!(%error, session_id=%self.id, "清理临时 Proton 日志失败");
                }
            }
        }
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
            let (findings, evidence) = analyze_snapshot(&report);
            report.findings = findings;
            report.forensics = evidence;
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
    decode_report(&bytes)
}

fn decode_report(bytes: &[u8]) -> Result<GameSessionReport, String> {
    let report: GameSessionReport =
        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    if report.schema_version != GAME_DIAGNOSTICS_SCHEMA {
        return Err(format!(
            "不支持的开发诊断格式 {}（当前为 {}）",
            report.schema_version, GAME_DIAGNOSTICS_SCHEMA
        ));
    }
    Ok(report)
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
        let Ok(mut report) = decode_report(&bytes) else { continue };
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
        {
            let observed = process_creation_ticks(report.pid);
            if observed.is_some()
                && (report.process_created_at.is_none()
                    || observed == report.process_created_at)
            {
                continue;
            }
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
        let report = decode_report(&bytes).ok()?;
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

/// Capture only known hot log file lengths at launch. These are cheap metadata
/// checks, not a recursive prefix scan. The baseline prevents stale entries
/// from earlier sessions being classified as a fresh crash.
fn initial_log_offsets(game_dir: &Path) -> HashMap<String, u64> {
    let mut offsets = HashMap::new();
    for (name, path) in [
        ("logs/game.log", game_dir.join("logs/latest.log")),
        ("logs/levilamina.log", game_dir.join("logs/LeviLamina.log")),
        ("logs/bloader.log", game_dir.join("logs/BLoader.log")),
    ] {
        if let Ok(meta) = fs::symlink_metadata(path)
            && meta.file_type().is_file()
        {
            offsets.insert(name.to_string(), meta.len());
        }
    }
    offsets
}

/// Reads only appended text for a preexisting log. If the file was rotated
/// and became shorter, treat it as a new file; a same-length rewrite cannot
/// be attributed safely and is conservatively ignored.
fn tail_after_baseline(path: &Path, baseline: u64, max: u64) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    if !fs::symlink_metadata(path).ok()?.file_type().is_file() { return None; }
    let mut file = fs::File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    if size == baseline { return None; }
    let start = if size < baseline { size.saturating_sub(max) }
        else { baseline.max(size.saturating_sub(max)) };
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(max).read_to_end(&mut bytes).ok()?;
    if bytes.is_empty() { return None; }
    Some(String::from_utf8_lossy(&bytes).into_owned())
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
        ("logs/compatibility-runner.log".to_string(),
            root().join(format!("{}.runner.log", report.id))),
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
        report.compatibility_logs.clone().unwrap_or_default(),
        report.compatibility_logs.as_ref()
            .map(|logs| logs.join("crash-reports")).unwrap_or_default(),
    ];
    let mut number = 0usize;
    let mut directories = directories.to_vec();
    #[cfg(target_os = "linux")]
    if let Some(prefix) = &report.compatibility_prefix {
        if let Ok(users) = fs::read_dir(prefix.join("drive_c/users")) {
            for user in users.take(16).flatten() {
                let profile = user.path();
                if !fs::symlink_metadata(&profile)
                    .is_ok_and(|meta| meta.file_type().is_dir())
                {
                    continue;
                }
                directories.push(profile.join("AppData/Local/CrashDumps"));
                directories.push(profile.join("AppData/Local/Microsoft/Windows/WER"));
                directories.push(profile.join("Temp"));
            }
        }
    }
    for dir in directories {
        if !fs::symlink_metadata(&dir)
            .is_ok_and(|meta| meta.file_type().is_dir())
        {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.take(128).flatten() {
            if number >= 48 { return files; }
            let path = entry.path();
            let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else { continue };
            if !matches!(ext.to_ascii_lowercase().as_str(), "log" | "txt" | "json") { continue }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else { continue };
            let name = name.to_ascii_lowercase();
            if report.compatibility_logs.as_ref() != Some(&dir)
                && report.compatibility_logs.as_ref().is_none_or(|logs| dir != logs.join("crash-reports"))
                && !name.contains("crash")
                && !name.contains("levilamina")
                && !name.contains("bloader")
            {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(&path) else { continue };
            if !meta.file_type().is_file() { continue }
            let Some(modified) = meta.modified().ok().map(DateTime::<Utc>::from) else { continue };
            if modified < report.started_at - chrono::Duration::minutes(5) { continue; }
            if report.ended_at.is_some_and(|end| modified > end + chrono::Duration::minutes(10)) {
                continue;
            }
            let compatibility_source = report.compatibility_logs.as_ref()
                .is_some_and(|root| dir.starts_with(root))
                || report.compatibility_prefix.as_ref()
                    .is_some_and(|prefix| dir.starts_with(prefix));
            let category = if compatibility_source { "runner" } else { "mod" };
            files.push((format!("logs/{category}-provider-{number:02}.{ext}"), path));
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
        let contents = match report.log_offsets.get(&name) {
            Some(baseline) => tail_after_baseline(&path, *baseline, MAX_TEXT_BYTES),
            None => tail_bytes(&path, MAX_TEXT_BYTES),
        };
        let Some(contents) = contents else { continue };
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

/// Evaluate only immutable, bounded per-session evidence on the I/O worker.
/// Stored findings contain stable reason identifiers and evidence origin only.
fn analyze_snapshot(
    report: &GameSessionReport,
) -> (Vec<CrashFinding>, forensics::ForensicSummary) {
    let mut sources = Vec::new();
    let mut total = 0usize;
    for (name, path) in frozen_text_files(report).into_iter().take(32) {
        if total >= 2 * 1024 * 1024 { break; }
        let remaining = 2 * 1024 * 1024 - total;
        if let Some(text) = tail_bytes(&path, (MAX_TEXT_BYTES as usize).min(remaining) as u64) {
            total += text.len();
            sources.push((name, text));
        }
    }
    let mut evidence = forensics::inspect_text(&sources);
    evidence.minidumps = forensics::inspect_dumps(&dump_candidates(report));
    let findings = analysis::analyze_with_forensics(report, &sources, &evidence);
    (findings, evidence)
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
    // No migration/fallback for prior development snapshot formats.
    Vec::new()
}

fn dump_candidates(report: &GameSessionReport) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let mut dirs = vec![
        report.game_directory.clone(),
        report.game_directory.join("crash-logs"),
        report.game_directory.join("logs"),
        report.game_directory.join("LeviLamina/logs"),
    ];
    if let Some(logs) = &report.compatibility_logs {
        dirs.push(logs.join("crash-reports"));
    }
    #[cfg(target_os = "linux")]
    if let Some(prefix) = &report.compatibility_prefix {
        if let Ok(users) = fs::read_dir(prefix.join("drive_c/users")) {
            for user in users.take(16).flatten() {
                let profile = user.path();
                if !fs::symlink_metadata(&profile)
                    .is_ok_and(|meta| meta.file_type().is_dir())
                {
                    continue;
                }
                dirs.push(profile.join("AppData/Local/CrashDumps"));
                dirs.push(profile.join("Local Settings/Application Data/CrashDumps"));
            }
        }
    }
    #[cfg(target_os = "windows")]
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local_app_data).join("CrashDumps"));
    }
    for dir in dirs {
        // A Wine prefix or mod-controlled directory may contain symlinks.
        // Never follow a linked directory into unrelated private files.
        if !fs::symlink_metadata(&dir)
            .is_ok_and(|meta| meta.file_type().is_dir())
        {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.take(128).flatten() {
            let path = entry.path();
            if !path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext|
                ext.eq_ignore_ascii_case("dmp") || ext.eq_ignore_ascii_case("mdmp")) { continue }
            let Ok(meta) = fs::symlink_metadata(&path) else { continue };
            if !meta.file_type().is_file() || meta.len() > MAX_DUMP_BYTES { continue }
            let Some(modified) = meta.modified().ok().map(DateTime::<Utc>::from) else { continue };
            if modified < report.started_at - chrono::Duration::minutes(5) { continue }
            if report.ended_at.is_some_and(|end| modified > end + chrono::Duration::minutes(10)) {
                continue;
            }
            // A native Wine/Proton PID is not the Minecraft Windows guest PID.
            // Prefix dumps are identified by EXE name and session time only.
            if path.parent().is_some_and(|parent| parent.ends_with("CrashDumps")) {
                let Some(file_name) = path.file_name() else { continue };
                let name = file_name.to_string_lossy().to_ascii_lowercase();
                if !name.contains("minecraft") {
                    continue;
                }
                if report.compatibility_prefix.is_none()
                    && !name.contains(&report.pid.to_string())
                {
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
            "schema_version": report.schema_version,
            "session_id": report.id, "task_id": report.task_id, "version": report.version,
            "platform": report.platform, "pid": report.pid,
            "process_created_at": report.process_created_at,
            "started_at": report.started_at, "ended_at": report.ended_at,
            "exit_code": report.exit_code, "exit_signal": report.exit_signal,
            "kind": report.kind,
            "findings": report.findings,
            "forensics": report.forensics,
            "compatibility_fault": report.compatibility_fault,
            "observed_process": if report.compatibility_prefix.is_some() { "runner" } else { "game" },
            "early_exit": report.early_exit,
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
            schema_version: GAME_DIAGNOSTICS_SCHEMA,
            id: Uuid::new_v4().to_string(),
            task_id: "task-1".to_string(),
            version: "1.26.51".to_string(),
            game_directory: PathBuf::from("C:/game"),
            platform: "gdk".to_string(),
            pid: 0x112A,
            process_created_at: Some(0xABCD),
            compatibility_prefix: None,
            compatibility_logs: None,
            compatibility_fault: None,
            early_exit: false,
            started_at: now - chrono::Duration::seconds(8),
            ended_at: Some(now),
            exit_code: None,
            exit_signal: None,
            kind: ExitKind::Unknown,
            summary: "未确认".to_string(),
            acknowledged: false,
            wer_fault: None,
            findings: Vec::new(),
            forensics: forensics::ForensicSummary::default(),
            log_offsets: HashMap::new(),
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
    fn wine_faults_are_not_inferred_from_runner_exit_code() {
        assert_eq!(
            classify_compatibility_output("wine: Unhandled page fault on read access"),
            Some(CompatibilityFault::WineUnhandledException)
        );
        assert_eq!(
            classify_compatibility_output("0024:err:module:import_dll Library foo.dll not found"),
            Some(CompatibilityFault::WineDependencyFailure)
        );
        assert_eq!(classify_compatibility_output("exited with code 1"), None);
    }

    #[test]
    fn previous_dev_schema_is_not_migrated() {
        let json = br#"{"schema_version":1,"id":"x"}"#;
        assert!(decode_report(json).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_process_generation_ignores_parenthesized_comm() {
        let mut fields = vec!["S".to_string()];
        fields.extend((4..=52).map(|value| value.to_string()));
        fields[19] = "987654321".to_string();
        let stat = format!("123 (Minecraft (Wine) Windows.exe) {}", fields.join(" "));
        assert_eq!(parse_linux_proc_starttime(&stat), Some(987654321));
    }

    #[test]
    fn strict_format_requires_current_schema() {
        let report = GameSessionReport {
            schema_version: GAME_DIAGNOSTICS_SCHEMA,
            id: Uuid::new_v4().to_string(),
            task_id: "dev-test".to_string(),
            version: "test".to_string(),
            game_directory: PathBuf::new(),
            platform: "wine".to_string(),
            pid: 123,
            process_created_at: Some(42),
            compatibility_prefix: Some(PathBuf::from("/tmp/wine-test")),
            compatibility_logs: None,
            compatibility_fault: None,
            early_exit: false,
            started_at: Utc::now(),
            ended_at: None,
            exit_code: None,
            exit_signal: None,
            kind: ExitKind::Running,
            summary: String::new(),
            acknowledged: false,
            wer_fault: None,
            findings: Vec::new(),
            forensics: forensics::ForensicSummary::default(),
            log_offsets: HashMap::new(),
        };
        let bytes = serde_json::to_vec(&report).unwrap();
        assert!(decode_report(&bytes).is_ok());
        let mut obsolete = serde_json::to_value(&report).unwrap();
        obsolete["schema_version"] = serde_json::json!(GAME_DIAGNOSTICS_SCHEMA - 1);
        let obsolete = serde_json::to_vec(&obsolete).unwrap();
        assert!(decode_report(&obsolete).is_err());
    }

    #[test]
    fn old_log_failure_is_not_attributed_to_a_new_session() {
        use std::io::Write as _;
        let file = std::env::temp_dir().join(format!("bmcbl-game-log-{}.log", Uuid::new_v4()));
        fs::write(&file, b"Failed to load plugin: OldAddon.dll\n").unwrap();
        let offset = fs::metadata(&file).unwrap().len();
        assert!(tail_after_baseline(&file, offset, MAX_TEXT_BYTES).is_none());
        let mut output = fs::OpenOptions::new().append(true).open(&file).unwrap();
        output.write_all(b"Failed to load plugin: NewAddon.dll\n").unwrap();
        let tail = tail_after_baseline(&file, offset, MAX_TEXT_BYTES).unwrap();
        assert!(tail.contains("NewAddon.dll"));
        assert!(!tail.contains("OldAddon.dll"));
        let _ = fs::remove_file(file);
    }

    #[test]
    fn report_identifiers_cannot_traverse_directories() {
        assert!(report_path("../../settings.toml").is_err());
        assert!(report_path(&Uuid::new_v4().to_string()).is_ok());
    }
}
