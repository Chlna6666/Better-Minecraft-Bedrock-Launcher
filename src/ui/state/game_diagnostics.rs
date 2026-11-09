use crate::core::minecraft::diagnostics::{CrashExportOptions, GameSessionReport};
use gpui::Global;

#[derive(Default)]
pub struct GameDiagnosticsState {
    pub pending: Option<GameSessionReport>,
    pub history: Vec<GameSessionReport>,
    pub options: CrashExportOptions,
    pub awaiting_sensitive_confirmation: bool,
    pub show_technical: bool,
    pub exporting: bool,
    pub export_status: Option<String>,
    pub export_location: Option<String>,
    pub export_generation: u64,
}

impl GameDiagnosticsState {
    /// Refresh the evidence of the currently open incident without resetting
    /// expanded details, ZIP selection, or in-progress export state.
    pub fn receive_report(&mut self, report: GameSessionReport) {
        if self.pending.as_ref().is_some_and(|current| current.id == report.id) {
            self.pending = Some(report);
        } else {
            self.select_report(report);
        }
    }

    /// Each incident starts from privacy-safe defaults, regardless of earlier choices.
    pub fn select_report(&mut self, report: GameSessionReport) {
        self.pending = Some(report);
        self.options = CrashExportOptions::default();
        self.awaiting_sensitive_confirmation = false;
        self.show_technical = false;
        self.export_status = None;
        self.export_location = None;
    }
}

impl Global for GameDiagnosticsState {}
