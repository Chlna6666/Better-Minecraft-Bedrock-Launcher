use crate::config::config::read_config;
use crate::utils::app_info;
use crate::utils::file_ops;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::warn;

const REPORTS_DIR: &str = "diagnostics";
const SESSION_FILE: &str = "session.json";
const PENDING_REPORT_FILE: &str = "pending-report.json";
const CRASH_SIGNAL_FILE: &str = "crash-signal.json";
const REPORT_ARCHIVE_DIR: &str = "archive";
const LATEST_LOG_FILE: &str = "logs/latest.log";
const PREVIOUS_LOG_FILE: &str = "logs/previous.log";
const MAX_LOG_TAIL_BYTES: usize = 64 * 1024;
const MAX_SUMMARY_LEN: usize = 240;
const GITHUB_ISSUE_BASE_URL: &str =
    "https://github.com/Chlna6666/Better-Minecraft-Bedrock-Launcher/issues/new";
const SENSITIVE_KEYS: &[&str] = &[
    "authorization",
    "access_token",
    "refresh_token",
    "token",
    "api_key",
    "apikey",
    "key",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticsSeverity {
    Info,
    Warning,
    Error,
    Fatal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticsKind {
    Panic,
    UnhandledException,
    UnexpectedExit,
    StartupFailure,
    ApplicationError,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DiagnosticsDetail {
    Panic {
        location: Option<String>,
        payload: String,
        backtrace: Option<String>,
    },
    UnhandledException {
        code: String,
        address: String,
    },
    UnexpectedExit {
        reason: String,
    },
    StartupFailure {
        stage: String,
        error: String,
    },
    ApplicationError {
        stage: String,
        error: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticsReport {
    pub id: String,
    pub kind: DiagnosticsKind,
    pub severity: DiagnosticsSeverity,
    pub created_at: String,
    pub app_version: String,
    pub build_info: String,
    pub os: String,
    pub process_id: u32,
    pub summary: String,
    pub detail: DiagnosticsDetail,
    pub log_tail: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
struct SessionMarker {
    process_id: u32,
    started_at: String,
    clean_shutdown: bool,
    app_version: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct CrashSignal {
    kind: DiagnosticsKind,
    created_at: String,
    detail: DiagnosticsDetail,
}

#[derive(Clone, Debug)]
pub struct DiagnosticsSharePayload {
    pub title: String,
    pub body_markdown: String,
    pub github_issue_url: String,
    pub sentry_dsn: Option<String>,
}

pub fn prepare_previous_run_reports() -> Result<()> {
    ensure_diagnostics_dirs()?;

    if pending_report_path().exists() {
        remove_file_if_exists(&crash_signal_path())?;
        clear_session_marker()?;
        return Ok(());
    }

    if let Some(report) = report_from_crash_signal()? {
        write_pending_report(&report)?;
        archive_report(&report)?;
        remove_file_if_exists(&crash_signal_path())?;
        clear_session_marker()?;
        return Ok(());
    }

    clear_stale_session_marker()?;

    Ok(())
}

pub fn mark_session_started() -> Result<()> {
    ensure_diagnostics_dirs()?;
    let marker = SessionMarker {
        process_id: std::process::id(),
        started_at: Utc::now().to_rfc3339(),
        clean_shutdown: false,
        app_version: app_info::get_version().to_string(),
    };
    write_json(&session_marker_path(), &marker)
}

pub fn mark_clean_shutdown() -> Result<()> {
    if !session_marker_path().exists() {
        return Ok(());
    }

    let mut marker: SessionMarker = read_json(&session_marker_path())?;
    marker.clean_shutdown = true;
    write_json(&session_marker_path(), &marker)
}

pub fn clear_session_marker() -> Result<()> {
    let path = session_marker_path();
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn clear_crash_signal() -> Result<()> {
    let path = crash_signal_path();
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn record_crash_signal(signal: CrashSignal) -> Result<()> {
    ensure_diagnostics_dirs()?;
    write_json(&crash_signal_path(), &signal)
}

pub fn record_panic_signal(
    location: Option<String>,
    payload: String,
    backtrace: Option<String>,
) -> Result<()> {
    record_crash_signal(CrashSignal {
        kind: DiagnosticsKind::Panic,
        created_at: Utc::now().to_rfc3339(),
        detail: DiagnosticsDetail::Panic {
            location,
            payload,
            backtrace,
        },
    })
}

pub fn record_unhandled_exception_signal(code: u32, address: usize) -> Result<()> {
    record_crash_signal(CrashSignal {
        kind: DiagnosticsKind::UnhandledException,
        created_at: Utc::now().to_rfc3339(),
        detail: DiagnosticsDetail::UnhandledException {
            code: format!("0x{code:08X}"),
            address: format!("0x{address:X}"),
        },
    })
}

pub fn create_startup_failure_report(
    stage: impl Into<String>,
    error: impl Into<String>,
) -> DiagnosticsReport {
    let detail = DiagnosticsDetail::StartupFailure {
        stage: stage.into(),
        error: error.into(),
    };
    build_report(
        DiagnosticsKind::StartupFailure,
        DiagnosticsSeverity::Fatal,
        detail,
    )
}

pub fn create_application_error_report(
    stage: impl Into<String>,
    error: impl Into<String>,
    severity: DiagnosticsSeverity,
) -> DiagnosticsReport {
    let detail = DiagnosticsDetail::ApplicationError {
        stage: stage.into(),
        error: error.into(),
    };
    build_report(DiagnosticsKind::ApplicationError, severity, detail)
}

pub fn persist_report(report: &DiagnosticsReport) -> Result<()> {
    ensure_diagnostics_dirs()?;
    write_pending_report(report)?;
    archive_report(report)
}

pub fn load_pending_report() -> Result<Option<DiagnosticsReport>> {
    let path = pending_report_path();
    if !path.exists() {
        return Ok(None);
    }

    read_json(&path).map(Some)
}

pub fn acknowledge_pending_report() -> Result<()> {
    let path = pending_report_path();
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn diagnostics_share_payload(report: &DiagnosticsReport) -> DiagnosticsSharePayload {
    let body_markdown = report_markdown(report);
    let title = format!("[{}] {}", report.kind.label(), report.summary);
    let issue_url = format!(
        "{base}?title={title}&body={body}",
        base = GITHUB_ISSUE_BASE_URL,
        title = url::form_urlencoded::byte_serialize(title.as_bytes()).collect::<String>(),
        body = url::form_urlencoded::byte_serialize(body_markdown.as_bytes()).collect::<String>(),
    );
    let sentry_dsn = read_config().ok().and_then(|config| {
        crate::config::config::resolved_error_report_sentry_dsn(&config.launcher)
    });

    DiagnosticsSharePayload {
        title,
        body_markdown,
        github_issue_url: issue_url,
        sentry_dsn,
    }
}

pub fn submit_report_to_sentry(report: &DiagnosticsReport, dsn: &str) -> Result<()> {
    let guard = init_sentry_client(dsn)?;
    if !guard.is_enabled() {
        anyhow::bail!("sentry client is disabled");
    }

    sentry::with_scope(
        |scope| {
            scope.set_level(Some(match report.severity {
                DiagnosticsSeverity::Info => sentry::Level::Info,
                DiagnosticsSeverity::Warning => sentry::Level::Warning,
                DiagnosticsSeverity::Error => sentry::Level::Error,
                DiagnosticsSeverity::Fatal => sentry::Level::Fatal,
            }));
            scope.set_tag("report_kind", report.kind.label());
            scope.set_tag("app_version", &report.app_version);
            scope.set_tag("report_id", &report.id);
            scope.set_extra("process_id", serde_json::Value::from(report.process_id));
            scope.set_extra("os", serde_json::Value::from(report.os.clone()));
            scope.set_extra(
                "created_at",
                serde_json::Value::from(report.created_at.clone()),
            );
            scope.set_extra(
                "detail",
                serde_json::to_value(&report.detail).unwrap_or_default(),
            );
            scope.set_extra("log_tail", serde_json::Value::from(report.log_tail.clone()));
        },
        || {
            sentry::capture_message(&report.summary, sentry::Level::Error);
        },
    );

    if !guard.close(Some(Duration::from_secs(5))) {
        anyhow::bail!("timed out waiting for sentry transport");
    }

    Ok(())
}

pub fn send_sentry_test_log(dsn: &str) -> Result<()> {
    let guard = init_sentry_client(dsn)?;
    if !guard.is_enabled() {
        anyhow::bail!("sentry client is disabled");
    }

    sentry::logger_info!(
        log_type = "test",
        log.source = "sentry_rust_sdk",
        "Log sent for testing"
    );

    if !guard.close(Some(Duration::from_secs(5))) {
        anyhow::bail!("timed out waiting for sentry transport");
    }

    Ok(())
}

fn init_sentry_client(dsn: &str) -> Result<sentry::ClientInitGuard> {
    let options = sentry::ClientOptions {
        dsn: Some(dsn.parse().context("invalid sentry dsn")?),
        release: sentry::release_name!(),
        attach_stacktrace: true,
        default_integrations: true,
        enable_logs: true,
        send_default_pii: false,
        ..sentry::ClientOptions::default()
    };

    Ok(sentry::init(options))
}

pub fn report_markdown(report: &DiagnosticsReport) -> String {
    let detail_json =
        serde_json::to_string_pretty(&report.detail).unwrap_or_else(|_| "{}".to_string());
    format!(
        "# BMCBL Error Report\n\n\
        - Report ID: `{}`\n\
        - Kind: `{}`\n\
        - Severity: `{}`\n\
        - Created At: `{}`\n\
        - App Version: `{}`\n\
        - OS: `{}`\n\
        - PID: `{}`\n\n\
        ## Summary\n\n\
        {}\n\n\
        ## Detail\n\n\
        ```json\n{}\n```\n\n\
        ## Log Tail\n\n\
        ```text\n{}\n```",
        report.id,
        report.kind.label(),
        report.severity.label(),
        report.created_at,
        report.app_version,
        report.os,
        report.process_id,
        report.summary,
        detail_json,
        report.log_tail
    )
}

fn report_from_crash_signal() -> Result<Option<DiagnosticsReport>> {
    let path = crash_signal_path();
    if !path.exists() {
        return Ok(None);
    }

    let signal: CrashSignal = read_json(&path)?;

    Ok(Some(build_report_with_log_tail(
        signal.kind,
        DiagnosticsSeverity::Fatal,
        signal.detail,
        read_previous_run_log_tail(),
    )))
}

fn clear_stale_session_marker() -> Result<()> {
    let path = session_marker_path();
    if !path.exists() {
        return Ok(());
    }

    let marker = match read_json::<SessionMarker>(&path) {
        Ok(marker) => marker,
        Err(error) => {
            warn!(
                ?error,
                path = %path.display(),
                "discarding unreadable diagnostics session marker"
            );
            remove_file_if_exists(&path)?;
            return Ok(());
        }
    };

    if marker.clean_shutdown {
        remove_file_if_exists(&path)?;
        return Ok(());
    }

    warn!(
        process_id = marker.process_id,
        started_at = %marker.started_at,
        app_version = %marker.app_version,
        "previous diagnostics session was not marked clean; no crash signal was present, so no user-facing report was created"
    );
    remove_file_if_exists(&path)?;
    Ok(())
}

fn read_previous_run_log_tail() -> String {
    let previous_log = read_sanitized_log_tail(&previous_log_path()).unwrap_or_default();
    if previous_log.is_empty() {
        read_sanitized_log_tail(&latest_log_path()).unwrap_or_default()
    } else {
        previous_log
    }
}

fn build_report(
    kind: DiagnosticsKind,
    severity: DiagnosticsSeverity,
    detail: DiagnosticsDetail,
) -> DiagnosticsReport {
    build_report_with_log_tail(
        kind,
        severity,
        detail,
        read_sanitized_log_tail(&latest_log_path()).unwrap_or_default(),
    )
}

fn build_report_with_log_tail(
    kind: DiagnosticsKind,
    severity: DiagnosticsSeverity,
    detail: DiagnosticsDetail,
    log_tail: String,
) -> DiagnosticsReport {
    let summary = build_summary(kind, &detail);
    DiagnosticsReport {
        id: uuid::Uuid::new_v4().to_string(),
        kind,
        severity,
        created_at: Utc::now().to_rfc3339(),
        app_version: app_info::get_version().to_string(),
        build_info: app_info::get_build_info(),
        os: detect_os_summary(),
        process_id: std::process::id(),
        summary,
        detail,
        log_tail,
    }
}

fn build_summary(kind: DiagnosticsKind, detail: &DiagnosticsDetail) -> String {
    let summary = match detail {
        DiagnosticsDetail::Panic { payload, .. } => format!("{}: {}", kind.label(), payload),
        DiagnosticsDetail::UnhandledException { code, address } => {
            format!("{}: {} {}", kind.label(), code, address)
        }
        DiagnosticsDetail::UnexpectedExit { reason } => format!("{}: {}", kind.label(), reason),
        DiagnosticsDetail::StartupFailure { stage, error } => {
            format!("{} at {}: {}", kind.label(), stage, error)
        }
        DiagnosticsDetail::ApplicationError { stage, error } => {
            format!("{} at {}: {}", kind.label(), stage, error)
        }
    };

    truncate_string(sanitize_string(summary), MAX_SUMMARY_LEN)
}

fn archive_report(report: &DiagnosticsReport) -> Result<()> {
    let archive_dir = diagnostics_dir().join(REPORT_ARCHIVE_DIR);
    fs::create_dir_all(&archive_dir)?;
    write_json(&archive_dir.join(format!("{}.json", report.id)), report)
}

fn write_pending_report(report: &DiagnosticsReport) -> Result<()> {
    write_json(&pending_report_path(), report)
}

fn ensure_diagnostics_dirs() -> Result<()> {
    fs::create_dir_all(diagnostics_dir())?;
    fs::create_dir_all(diagnostics_dir().join(REPORT_ARCHIVE_DIR))?;
    Ok(())
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn diagnostics_dir() -> PathBuf {
    file_ops::bmcbl_subdir(REPORTS_DIR)
}

fn session_marker_path() -> PathBuf {
    diagnostics_dir().join(SESSION_FILE)
}

fn pending_report_path() -> PathBuf {
    diagnostics_dir().join(PENDING_REPORT_FILE)
}

fn crash_signal_path() -> PathBuf {
    diagnostics_dir().join(CRASH_SIGNAL_FILE)
}

fn latest_log_path() -> PathBuf {
    file_ops::bmcbl_subdir(LATEST_LOG_FILE)
}

fn previous_log_path() -> PathBuf {
    file_ops::bmcbl_subdir(PREVIOUS_LOG_FILE)
}

fn read_sanitized_log_tail(path: &Path) -> Result<String> {
    if !path.exists() {
        return Ok(String::new());
    }

    let bytes = fs::read(path)?;
    let tail = if bytes.len() > MAX_LOG_TAIL_BYTES {
        &bytes[bytes.len() - MAX_LOG_TAIL_BYTES..]
    } else {
        bytes.as_slice()
    };
    let text = String::from_utf8_lossy(tail).into_owned();
    Ok(sanitize_string(text).trim().to_string())
}

fn sanitize_string(input: impl Into<String>) -> String {
    let mut output = input.into();
    for key in SENSITIVE_KEYS {
        output = mask_key_value(&output, key);
    }
    for candidate in sensitive_path_candidates() {
        if candidate.is_empty() {
            continue;
        }
        output = output.replace(&candidate, "<user-path>");
        output = output.replace(&candidate.replace('\\', "/"), "<user-path>");
    }
    output
}

fn sensitive_path_candidates() -> Vec<String> {
    let mut paths = Vec::new();
    for name in ["USERPROFILE", "APPDATA", "LOCALAPPDATA"] {
        if let Some(value) = std::env::var_os(name) {
            let value = value.to_string_lossy().to_string();
            if !value.is_empty() {
                paths.push(value);
            }
        }
    }
    paths
}

fn mask_key_value(input: &str, key: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for line in input.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(position) = lower.find(key) {
            if let Some(separator_offset) = line[position..].find(['=', ':']) {
                let separator_index = position + separator_offset;
                output.push_str(&line[..=separator_index]);
                output.push_str(" [redacted]");
                output.push('\n');
                continue;
            }
        }
        output.push_str(line);
        output.push('\n');
    }
    output.trim_end_matches('\n').to_string()
}

fn detect_os_summary() -> String {
    let os = sysinfo::System::long_os_version()
        .or_else(sysinfo::System::os_version)
        .unwrap_or_else(|| std::env::consts::OS.to_string());
    let kernel = sysinfo::System::kernel_version().unwrap_or_default();
    if kernel.is_empty() {
        os
    } else {
        format!("{os} ({kernel})")
    }
}

fn truncate_string(input: String, max_len: usize) -> String {
    if input.chars().count() <= max_len {
        return input;
    }

    input.chars().take(max_len).collect::<String>() + "..."
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let payload = serde_json::to_vec_pretty(value)?;
    fs::write(path, payload)?;
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let payload = fs::read(path)?;
    serde_json::from_slice(&payload).context("failed to parse diagnostics json")
}

impl DiagnosticsKind {
    pub fn label(self) -> &'static str {
        match self {
            DiagnosticsKind::Panic => "panic",
            DiagnosticsKind::UnhandledException => "unhandled_exception",
            DiagnosticsKind::UnexpectedExit => "unexpected_exit",
            DiagnosticsKind::StartupFailure => "startup_failure",
            DiagnosticsKind::ApplicationError => "application_error",
        }
    }
}

impl DiagnosticsSeverity {
    pub fn label(self) -> &'static str {
        match self {
            DiagnosticsSeverity::Info => "info",
            DiagnosticsSeverity::Warning => "warning",
            DiagnosticsSeverity::Error => "error",
            DiagnosticsSeverity::Fatal => "fatal",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(kind: DiagnosticsKind, detail: DiagnosticsDetail) -> DiagnosticsReport {
        build_report(kind, DiagnosticsSeverity::Error, detail)
    }

    #[test]
    fn markdown_contains_summary_and_log_tail() {
        let report = report(
            DiagnosticsKind::StartupFailure,
            DiagnosticsDetail::StartupFailure {
                stage: "bootstrap".to_string(),
                error: "boom".to_string(),
            },
        );

        let markdown = report_markdown(&report);
        assert!(markdown.contains("BMCBL Error Report"));
        assert!(markdown.contains("Summary"));
        assert!(markdown.contains("Log Tail"));
    }

    #[test]
    fn github_url_encodes_title_and_body() {
        let report = report(
            DiagnosticsKind::UnexpectedExit,
            DiagnosticsDetail::UnexpectedExit {
                reason: "contains spaces".to_string(),
            },
        );

        let payload = diagnostics_share_payload(&report);
        assert!(payload.github_issue_url.starts_with(GITHUB_ISSUE_BASE_URL));
        assert!(payload.github_issue_url.contains("title="));
        assert!(payload.github_issue_url.contains("body="));
    }

    #[test]
    fn sanitize_masks_sensitive_lines() {
        let input = "authorization: bearer abc\npath=C:\\Users\\alice\\file";
        let output = sanitize_string(input.to_string());
        assert!(output.contains("[redacted]"));
        assert!(!output.contains("bearer abc"));
    }

    #[test]
    fn truncate_shortens_long_strings() {
        let input = "a".repeat(300);
        let output = truncate_string(input, 20);
        assert!(output.len() <= 23);
        assert!(output.ends_with("..."));
    }

    #[test]
    fn application_error_summary_includes_stage() {
        let report = report(
            DiagnosticsKind::ApplicationError,
            DiagnosticsDetail::ApplicationError {
                stage: "open_main_window".to_string(),
                error: "failed".to_string(),
            },
        );

        assert!(
            report
                .summary
                .contains("application_error at open_main_window")
        );
    }

    #[test]
    fn build_report_with_log_tail_keeps_previous_run_log_tail() {
        let report = build_report_with_log_tail(
            DiagnosticsKind::Panic,
            DiagnosticsSeverity::Fatal,
            DiagnosticsDetail::Panic {
                location: Some("src/main.rs:1".to_string()),
                payload: "boom".to_string(),
                backtrace: None,
            },
            "previous run log".to_string(),
        );

        assert_eq!(report.log_tail, "previous run log");
    }
}
