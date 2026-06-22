use crate::utils::diagnostics::{self, DiagnosticsReport, DiagnosticsSeverity};
use gpui::BorrowAppContext as _;
use std::fmt::Display;
use thiserror::Error;
use tokio::task::JoinError;
use tracing::error;
use zip::result::ZipError;

const DEFAULT_CORE_ERROR_STAGE: &str = "core";
const DEFAULT_APPLICATION_ERROR_STAGE: &str = "application";

/// 核心错误类型
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("Request error: {0}")]
    Request(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("XML parsing error: {0}")]
    Xml(#[from] xmltree::ParseError),

    #[error("Zip error: {0}")]
    Zip(#[from] ZipError),

    #[error("Bad update identity")]
    BadUpdateIdentity,

    #[error("Unknown content length")]
    UnknownContentLength,

    #[error("Task join error: {0}")]
    Join(#[from] JoinError),

    #[error("Config error: {0}")]
    Config(String),

    #[error("{0}")]
    Other(String),

    #[error("Operation timed out")]
    Timeout,

    /// 校验和不匹配（例如 MD5 校验失败）
    #[error("Checksum mismatch: {0}")]
    ChecksumMismatch(String),
}

impl From<tokio::time::error::Elapsed> for CoreError {
    fn from(_: tokio::time::error::Elapsed) -> Self {
        CoreError::Timeout
    }
}

/// 核心结果类型
#[derive(Debug)]
pub enum CoreResult<T = ()> {
    Success(T),
    Cancelled,
    Error(CoreError),
}

impl<T> CoreResult<T> {
    pub fn success(value: T) -> Self {
        CoreResult::Success(value)
    }

    pub fn cancelled() -> Self {
        CoreResult::Cancelled
    }

    pub fn error(err: CoreError) -> Self {
        CoreResult::Error(err)
    }
}

impl<T> From<Result<T, CoreError>> for CoreResult<T> {
    fn from(r: Result<T, CoreError>) -> Self {
        match r {
            Ok(v) => CoreResult::Success(v),
            Err(e) => {
                show_core_error("程序错误", &e);
                CoreResult::Error(e)
            }
        }
    }
}

pub fn show_core_error(title: &str, error: &CoreError) {
    show_core_error_at(DEFAULT_CORE_ERROR_STAGE, title, error);
}

pub fn show_core_error_at(stage: impl Into<String>, title: &str, error: &CoreError) {
    show_application_error(title, stage, error);
}

pub fn show_other_error(title: &str, message: impl Into<String>) {
    show_application_error(title, DEFAULT_APPLICATION_ERROR_STAGE, message.into());
}

pub fn show_application_error(title: &str, stage: impl Into<String>, error: impl Display) {
    let message = error.to_string();
    let report = report_application_error(stage, &message, DiagnosticsSeverity::Error);
    show_reported_error_dialog(title, &message, report.as_ref());
}

pub fn show_application_error_in_app(
    cx: &mut gpui::App,
    title: &str,
    stage: impl Into<String>,
    error: impl Display,
) {
    let message = error.to_string();
    let report = report_application_error(stage, &message, DiagnosticsSeverity::Error);
    if let Some(report) = report.clone() {
        publish_report_to_ui(cx, report);
    }
    show_reported_error_dialog(title, &message, report.as_ref());
}

pub fn show_startup_failure(title: &str, stage: impl Into<String>, error: impl Display) {
    let message = error.to_string();
    let report = report_startup_failure(stage, &message);
    show_reported_error_dialog(title, &message, report.as_ref());
}

pub fn report_application_error(
    stage: impl Into<String>,
    error: impl Into<String>,
    severity: DiagnosticsSeverity,
) -> Option<DiagnosticsReport> {
    let stage = stage.into();
    let error_message = error.into();
    let report = diagnostics::create_application_error_report(stage, error_message, severity);
    persist_diagnostics_report(report)
}

pub fn report_startup_failure(
    stage: impl Into<String>,
    error: impl Into<String>,
) -> Option<DiagnosticsReport> {
    let report = diagnostics::create_startup_failure_report(stage, error);
    persist_diagnostics_report(report)
}

fn persist_diagnostics_report(report: DiagnosticsReport) -> Option<DiagnosticsReport> {
    match diagnostics::persist_report(&report) {
        Ok(()) => Some(report),
        Err(error) => {
            error!(?error, "failed to persist diagnostics report");
            eprintln!("Failed to persist diagnostics report: {error:#}");
            None
        }
    }
}

fn publish_report_to_ui(cx: &mut gpui::App, report: DiagnosticsReport) {
    cx.update_global(
        |diagnostics_state: &mut crate::ui::state::diagnostics::DiagnosticsState, _cx| {
            diagnostics_state.set_pending_report(Some(report));
        },
    );
}

fn show_reported_error_dialog(title: &str, message: &str, report: Option<&DiagnosticsReport>) {
    let body = match report {
        Some(report) => format!(
            "{message}\n\n错误报告 ID: {}\n可在诊断窗口中查看、复制或提交该报告。",
            report.id
        ),
        None => format!("{message}\n\n诊断报告保存失败，请查看 logs/latest.log 获取更多信息。"),
    };
    show_error_dialog(title, &body);
}

pub fn show_error_dialog(title: &str, message: &str) {
    error!(title, message, "showing application error dialog");
    eprintln!("{title}: {message}");

    let shown = rfd::MessageDialog::new()
        .set_title(title)
        .set_description(message)
        .set_level(rfd::MessageLevel::Error)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();

    if matches!(shown, rfd::MessageDialogResult::Custom(_)) {
        eprintln!("{title}: {message}");
    }
}
