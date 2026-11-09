use crate::core::minecraft::diagnostics::GameSessionReport;
use gpui::Global;

#[derive(Default)]
pub struct GameDiagnosticsState {
    pub pending: Option<GameSessionReport>,
    pub export_status: Option<String>,
    pub history: Vec<GameSessionReport>,
    pub confirm_dump_export: bool,
}

impl Global for GameDiagnosticsState {}
