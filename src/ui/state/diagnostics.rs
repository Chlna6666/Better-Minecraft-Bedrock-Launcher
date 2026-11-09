use crate::utils::diagnostics::{
    self, DiagnosticsReport,
};
use gpui::Global;
use std::collections::VecDeque;

/// Fatal crash reports and nonfatal operational errors have independent state:
/// closing a toast failure must not delete the pending crash report or cause
/// Sentry auto-submission.
#[derive(Default)]
pub struct DiagnosticsState {
    pub pending_report: Option<DiagnosticsReport>,
    pub submitting_sentry: bool,
    pub auto_report_attempted: bool,
    pub exporting_bundle: bool,
    recent_operation_reports: VecDeque<DiagnosticsReport>,
    active_operation_report_id: Option<String>,
}

impl Global for DiagnosticsState {}

impl DiagnosticsState {
    pub fn set_pending_report(&mut self, report: Option<DiagnosticsReport>) {
        self.pending_report = report;
        if self.pending_report.is_none() {
            self.submitting_sentry = false;
            self.auto_report_attempted = false;
        }
    }

    /// Record a recoverable failure without touching the crash report file.
    /// A bounded in-session history survives toast timeout/dismissal.
    pub fn record_operation_error(&mut self, stage: &str, error: &str) -> String {
        const MAX_HISTORY: usize = 48;
        const MAX_ERROR_CHARS: usize = 32_768;
        let mut message: String = error.chars().take(MAX_ERROR_CHARS + 1).collect();
        if message.chars().count() > MAX_ERROR_CHARS {
            message = message.chars().take(MAX_ERROR_CHARS).collect();
            message.push_str("\n[错误详情过长，已截断；请导出诊断包查看日志]");
        }
        let report = diagnostics::create_operation_error_report(stage, message);
        let id = report.id.clone();
        self.recent_operation_reports.push_back(report);
        while self.recent_operation_reports.len() > MAX_HISTORY {
            self.recent_operation_reports.pop_front();
        }
        id
    }

    pub fn show_operation_report(&mut self, id: &str) -> bool {
        if self.recent_operation_reports.iter().any(|report| report.id == id) {
            self.active_operation_report_id = Some(id.to_owned());
            true
        } else {
            false
        }
    }

    pub fn show_latest_operation_report(&mut self) -> bool {
        let Some(report) = self.recent_operation_reports.back() else {
            return false;
        };
        self.active_operation_report_id = Some(report.id.clone());
        true
    }

    pub fn active_operation_report(&self) -> Option<&DiagnosticsReport> {
        let id = self.active_operation_report_id.as_ref()?;
        self.recent_operation_reports.iter().find(|report| &report.id == id)
    }

    pub fn active_operation_report_id(&self) -> Option<&str> {
        self.active_operation_report_id.as_deref()
    }

    pub fn close_operation_report(&mut self) {
        self.active_operation_report_id = None;
    }

    pub fn has_visible_report(&self) -> bool {
        self.pending_report.is_some() || self.active_operation_report().is_some()
    }

    /// Clear ONLY the crash report after explicit dismissal or successful
    /// crash-only Sentry submission. Preserve ordinary error history.
    pub fn clear(&mut self) {
        self.pending_report = None;
        self.submitting_sentry = false;
        self.auto_report_attempted = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_errors_do_not_displace_crash_report() {
        let mut state = DiagnosticsState::default();
        let crash = diagnostics::create_startup_failure_report("bootstrap", "failed");
        let crash_id = crash.id.clone();
        state.set_pending_report(Some(crash));
        let operation = state.record_operation_error("download", "403 Forbidden");
        assert!(state.show_operation_report(&operation));
        assert_eq!(state.pending_report.as_ref().unwrap().id, crash_id);
        state.close_operation_report();
        assert!(state.pending_report.is_some());
        state.clear();
        assert!(state.show_latest_operation_report());
    }
}
