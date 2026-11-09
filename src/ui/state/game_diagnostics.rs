use crate::core::minecraft::diagnostics::GameSessionReport;
use gpui::Global;

#[derive(Default)]
pub struct GameDiagnosticsState {
    pub pending: Option<GameSessionReport>,
    pub export_status: Option<String>,
}

impl Global for GameDiagnosticsState {}
