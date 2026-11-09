use crate::core::minecraft::diagnostics::{CrashExportOptions, GameSessionReport};
use gpui::Global;

#[derive(Default)]
pub struct GameDiagnosticsState {
    pub pending: Option<GameSessionReport>,
    pub history: Vec<GameSessionReport>,
    pub options: CrashExportOptions,
    pub awaiting_sensitive_confirmation: bool,
    pub exporting: bool,
    pub export_status: Option<String>,
    pub export_location: Option<String>,
    pub export_generation: u64,
}

impl GameDiagnosticsState {
    /// Switching incidents never resets the user's deliberate export choices.
    pub fn select_report(&mut self, report: GameSessionReport) {
        self.pending = Some(report);
        self.awaiting_sensitive_confirmation = false;
        self.export_status = None;
        self.export_location = None;
    }
}

impl Global for GameDiagnosticsState {}
