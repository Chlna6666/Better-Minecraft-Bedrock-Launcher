use crate::core::minecraft::diagnostics::{CrashExportOptions, GameSessionReport};
use gpui::Global;
use std::collections::HashSet;

#[derive(Default)]
pub struct GameDiagnosticsState {
    pub pending: Option<GameSessionReport>,
    pub history: Vec<GameSessionReport>,
    /// Distinct incidents are queued instead of replacing an open diagnosis.
    pub queued: Vec<GameSessionReport>,
    /// Prevent an asynchronous late-WER update from reopening a dismissed popup.
    dismissed_ids: HashSet<String>,
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
        if report.acknowledged || self.dismissed_ids.contains(&report.id) {
            return;
        }
        if self.pending.as_ref().is_some_and(|current| current.id == report.id) {
            self.pending = Some(report);
        } else if let Some(previous) = self.queued.iter_mut()
            .find(|previous| previous.id == report.id)
        {
            *previous = report;
        } else if self.pending.is_some() {
            self.queued.push(report);
        } else {
            self.select_report(report);
        }
    }

    /// Each incident starts from privacy-safe defaults, regardless of earlier choices.
    pub fn select_report(&mut self, report: GameSessionReport) {
        self.pending = Some(report);
        // Invalidate any previous report's completed-export callback.
        self.export_generation = self.export_generation.wrapping_add(1);
        self.exporting = false;
        self.options = CrashExportOptions::default();
        self.awaiting_sensitive_confirmation = false;
        self.show_technical = false;
        self.export_status = None;
        self.export_location = None;
    }

    /// Acknowledge only the report the user actually dismissed.
    /// The next independent failure is shown without losing its diagnostic data.
    pub fn dismiss_pending_if_matches(&mut self, id: &str) -> bool {
        if self.pending.as_ref().is_none_or(|report| report.id != id) {
            return false;
        }
        self.dismissed_ids.insert(id.to_string());
        if self.queued.is_empty() {
            self.pending = None;
            self.export_generation = self.export_generation.wrapping_add(1);
            self.exporting = false;
            self.awaiting_sensitive_confirmation = false;
            self.export_status = None;
            self.export_location = None;
        } else {
            let next = self.queued.remove(0);
            self.select_report(next);
        }
        true
    }
}

impl Global for GameDiagnosticsState {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::minecraft::diagnostics::{ExitKind, forensics::ForensicSummary};
    use chrono::Utc;
    use std::{collections::HashMap, path::PathBuf};

    fn report(id: &str) -> GameSessionReport {
        GameSessionReport {
            schema_version: 7,
            id: id.into(),
            task_id: format!("task-{id}"),
            version: "dev".into(),
            game_directory: PathBuf::new(),
            platform: "gdk".into(),
            pid: 42,
            process_created_at: None,
            compatibility_prefix: None,
            compatibility_logs: None,
            compatibility_fault: None,
            early_exit: false,
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            exit_code: Some(0xC0000005),
            exit_signal: None,
            kind: ExitKind::SuspectedCrash,
            summary: "test".into(),
            acknowledged: false,
            wer_fault: None,
            findings: Vec::new(),
            forensics: ForensicSummary::default(),
            log_offsets: HashMap::new(),
        }
    }

    #[test]
    fn concurrent_crashes_do_not_replace_the_open_dialog() {
        let mut state = GameDiagnosticsState::default();
        state.receive_report(report("first"));
        state.show_technical = true;
        state.receive_report(report("second"));
        state.receive_report(report("second"));
        assert_eq!(state.pending.as_ref().unwrap().id, "first");
        assert!(state.show_technical);
        assert_eq!(state.queued.len(), 1);
        assert!(state.dismiss_pending_if_matches("first"));
        assert_eq!(state.pending.as_ref().unwrap().id, "second");
        assert!(!state.show_technical);
        assert!(state.queued.is_empty());
        assert!(!state.dismiss_pending_if_matches("first"));
        state.receive_report(report("first"));
        assert_eq!(state.pending.as_ref().unwrap().id, "second");
        assert!(state.dismiss_pending_if_matches("second"));
        assert!(state.pending.is_none());
    }

    #[test]
    fn acknowledged_late_wer_does_not_open_another_dialog() {
        let mut state = GameDiagnosticsState::default();
        let mut incident = report("acknowledged");
        incident.acknowledged = true;
        state.receive_report(incident);
        assert!(state.pending.is_none());
    }
}
