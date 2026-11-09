use crate::utils::diagnostics::{self, DiagnosticsReport};
use gpui::Global;
use std::collections::VecDeque;

/// A persisted crash report and transient operation failures have independent
/// lifetimes. Sentry upload does not acknowledge or dismiss either report.
#[derive(Default)]
pub struct DiagnosticsState {
    pub pending_report: Option<DiagnosticsReport>,
    pub submitting_sentry: bool,
    pub auto_report_attempted: bool,
    pub sentry_submission_failed: bool,
    pub exporting_bundle: bool,
    recent_operation_reports: VecDeque<DiagnosticsReport>,
    active_operation_report_id: Option<String>,
}

impl Global for DiagnosticsState {}

impl DiagnosticsState {
    pub fn set_pending_report(&mut self, report: Option<DiagnosticsReport>) {
        let old_id = self.pending_report.as_ref().map(|report| report.id.as_str());
        let new_id = report.as_ref().map(|report| report.id.as_str());
        if old_id != new_id {
            self.submitting_sentry = false;
            self.auto_report_attempted = false;
            self.sentry_submission_failed = false;
        }
        self.pending_report = report;
    }

    /// Atomically guard both automatic and manual Sentry submissions. A
    /// successful upload remains viewable until the user explicitly dismisses
    /// the report. Failed auto uploads can be retried manually.
    pub fn begin_sentry_submission(&mut self, report_id: &str, automatic: bool) -> bool {
        let Some(report) = self.pending_report.as_ref() else {
            return false;
        };
        if report.id != report_id
            || !report.is_fatal_incident()
            || report.sentry_submitted_at.is_some()
            || self.submitting_sentry
            || (automatic && self.auto_report_attempted)
        {
            return false;
        }
        self.submitting_sentry = true;
        self.auto_report_attempted = true;
        self.sentry_submission_failed = false;
        true
    }

    /// Ignore results for dismissed or replaced reports. No upload callback
    /// may close a modal, delete the persisted report, or clear crash markers.
    pub fn finish_sentry_submission(
        &mut self,
        report_id: &str,
        submitted_at: Option<String>,
    ) -> bool {
        let Some(report) = self.pending_report.as_mut() else {
            return false;
        };
        if report.id != report_id {
            return false;
        }
        self.submitting_sentry = false;
        self.sentry_submission_failed = submitted_at.is_none();
        if let Some(sent_at) = submitted_at {
            report.sentry_submitted_at = Some(sent_at);
        }
        true
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

    /// Called only on explicit user dismissal. Operation error history stays.
    pub fn clear(&mut self) {
        self.pending_report = None;
        self.submitting_sentry = false;
        self.auto_report_attempted = false;
        self.sentry_submission_failed = false;
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

    #[test]
    fn successful_auto_submit_keeps_report_visible_and_prevents_duplicate() {
        let mut state = DiagnosticsState::default();
        let report = diagnostics::create_startup_failure_report("boot", "panic");
        let id = report.id.clone();
        state.set_pending_report(Some(report));
        assert!(state.begin_sentry_submission(&id, true));
        assert!(!state.begin_sentry_submission(&id, true));
        assert!(!state.begin_sentry_submission(&id, false));
        assert!(state.finish_sentry_submission(&id, Some("2026-11-09T08:00:00Z".into())));
        assert!(state.has_visible_report());
        assert!(!state.submitting_sentry);
        assert_eq!(
            state.pending_report.as_ref().unwrap().sentry_submitted_at.as_deref(),
            Some("2026-11-09T08:00:00Z")
        );
        assert!(!state.begin_sentry_submission(&id, true));
        assert!(!state.begin_sentry_submission(&id, false));
    }

    #[test]
    fn failed_auto_submit_keeps_report_visible_and_allows_manual_retry() {
        let mut state = DiagnosticsState::default();
        let report = diagnostics::create_startup_failure_report("boot", "panic");
        let id = report.id.clone();
        state.set_pending_report(Some(report));
        assert!(state.begin_sentry_submission(&id, true));
        assert!(state.finish_sentry_submission(&id, None));
        assert!(state.sentry_submission_failed);
        assert!(state.has_visible_report());
        assert!(!state.begin_sentry_submission(&id, true));
        assert!(state.begin_sentry_submission(&id, false));
        assert!(!state.sentry_submission_failed);
    }

    #[test]
    fn stale_sentry_completion_never_changes_new_report() {
        let mut state = DiagnosticsState::default();
        let old = diagnostics::create_startup_failure_report("old", "failed");
        assert!(state.begin_sentry_submission(&old.id, false) == false);
        state.set_pending_report(Some(old.clone()));
        assert!(state.begin_sentry_submission(&old.id, true));
        let new = diagnostics::create_startup_failure_report("new", "failed");
        state.set_pending_report(Some(new.clone()));
        assert!(!state.finish_sentry_submission(&old.id, Some("now".into())));
        assert_eq!(state.pending_report.unwrap().id, new.id);
        assert!(!state.submitting_sentry);
    }

    #[test]
    fn nonfatal_errors_cannot_enter_sentry_submission() {
        let mut state = DiagnosticsState::default();
        let report = diagnostics::create_operation_error_report("download", "403");
        let id = report.id.clone();
        state.set_pending_report(Some(report));
        assert!(!state.begin_sentry_submission(&id, true));
        assert!(!state.begin_sentry_submission(&id, false));
    }
}
