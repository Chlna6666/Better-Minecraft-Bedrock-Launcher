use crate::utils::diagnostics::DiagnosticsReport;
use gpui::Global;

#[derive(Default)]
pub struct DiagnosticsState {
    pub pending_report: Option<DiagnosticsReport>,
    pub submitting_sentry: bool,
    pub auto_report_attempted: bool,
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

    pub fn clear(&mut self) {
        self.pending_report = None;
        self.submitting_sentry = false;
        self.auto_report_attempted = false;
    }
}
