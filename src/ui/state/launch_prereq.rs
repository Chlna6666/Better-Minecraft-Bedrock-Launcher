use std::collections::VecDeque;
use std::time::{Duration, Instant};

use gpui::{Global, SharedString};

use crate::core::minecraft::launcher::preflight::LaunchPrerequisiteCheck;

const MAX_LAUNCH_PREREQ_LOGS: usize = 18;
const BUSY_ANIMATION_FRAME_INTERVAL: Duration = Duration::from_millis(120);

#[derive(Clone, Debug)]
pub struct PendingLaunchVersion {
    pub folder: SharedString,
    pub name: SharedString,
    pub version: SharedString,
    pub kind: SharedString,
    pub path: SharedString,
    pub launch_args: Option<SharedString>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchPrereqOperation {
    Checking,
    OpeningDeveloperSettings,
    EnablingDeveloperMode,
    InstallingUwpDependencies,
    InstallingGameInput,
    InstallingWindowsAppSdk,
}

pub struct LaunchPrereqState {
    pub visible: bool,
    pub request_id: u64,
    pub version: Option<PendingLaunchVersion>,
    pub check: Option<LaunchPrerequisiteCheck>,
    pub operation: Option<LaunchPrereqOperation>,
    pub progress_percent: Option<u32>,
    pub progress_stage: SharedString,
    pub progress_target: Option<SharedString>,
    pub error_message: Option<SharedString>,
    pub admin_notice: Option<SharedString>,
    logs: VecDeque<SharedString>,
    busy_animation_started_at: Option<Instant>,
}

impl Global for LaunchPrereqState {}

impl Default for LaunchPrereqState {
    fn default() -> Self {
        Self {
            visible: false,
            request_id: 0,
            version: None,
            check: None,
            operation: None,
            progress_percent: None,
            progress_stage: SharedString::default(),
            progress_target: None,
            error_message: None,
            admin_notice: None,
            logs: VecDeque::new(),
            busy_animation_started_at: None,
        }
    }
}

impl LaunchPrereqState {
    pub fn begin(&mut self, version: PendingLaunchVersion) -> u64 {
        let now = Instant::now();
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.visible = true;
        self.version = Some(version);
        self.check = None;
        self.operation = Some(LaunchPrereqOperation::Checking);
        self.progress_percent = None;
        self.progress_stage = SharedString::default();
        self.progress_target = None;
        self.error_message = None;
        self.admin_notice = None;
        self.logs.clear();
        self.start_busy_animation_at(now);
        self.request_id
    }

    pub fn request_matches(&self, request_id: u64) -> bool {
        self.visible && self.request_id == request_id
    }

    pub fn cancel_active_request(&mut self) -> Option<u64> {
        if !self.visible {
            return None;
        }

        let cancelled_request_id = self.request_id;
        self.request_id = self.request_id.wrapping_add(1).max(1);
        self.dismiss();
        Some(cancelled_request_id)
    }

    pub fn set_check_if_matches(
        &mut self,
        request_id: u64,
        check: LaunchPrerequisiteCheck,
    ) -> bool {
        if !self.request_matches(request_id) {
            return false;
        }

        self.check = Some(check);
        self.operation = None;
        self.progress_percent = None;
        self.progress_stage = SharedString::default();
        self.progress_target = None;
        self.error_message = None;
        self.admin_notice = None;
        self.stop_busy_animation();
        true
    }

    pub fn set_operation_if_matches(
        &mut self,
        request_id: u64,
        operation: LaunchPrereqOperation,
    ) -> bool {
        if !self.request_matches(request_id) {
            return false;
        }

        self.operation = Some(operation);
        self.progress_percent = None;
        self.progress_stage = SharedString::default();
        self.progress_target = None;
        self.error_message = None;
        self.admin_notice = None;
        self.start_busy_animation();
        true
    }

    pub fn update_progress_if_matches(
        &mut self,
        request_id: u64,
        percent: u32,
        stage: impl Into<SharedString>,
        target: Option<SharedString>,
    ) -> bool {
        if !self.request_matches(request_id) {
            return false;
        }

        self.progress_percent = Some(percent.min(100));
        self.progress_stage = stage.into();
        self.progress_target = target;
        true
    }

    pub fn push_log_if_matches(&mut self, request_id: u64, line: impl Into<SharedString>) -> bool {
        if !self.request_matches(request_id) {
            return false;
        }

        if self.logs.len() >= MAX_LAUNCH_PREREQ_LOGS {
            self.logs.pop_front();
        }
        self.logs.push_back(line.into());
        true
    }

    pub fn set_error_if_matches(
        &mut self,
        request_id: u64,
        error_message: impl Into<SharedString>,
    ) -> bool {
        if !self.request_matches(request_id) {
            return false;
        }

        self.error_message = Some(error_message.into());
        self.operation = None;
        self.progress_percent = None;
        self.progress_stage = SharedString::default();
        self.progress_target = None;
        self.stop_busy_animation();
        true
    }

    pub fn set_admin_notice_if_matches(
        &mut self,
        request_id: u64,
        message: impl Into<SharedString>,
    ) -> bool {
        if !self.request_matches(request_id) {
            return false;
        }

        self.admin_notice = Some(message.into());
        true
    }

    pub fn clear_if_matches(&mut self, request_id: u64) -> bool {
        if self.request_id != request_id {
            return false;
        }

        self.dismiss();
        true
    }

    pub fn dismiss(&mut self) {
        self.visible = false;
        self.version = None;
        self.check = None;
        self.operation = None;
        self.progress_percent = None;
        self.progress_stage = SharedString::default();
        self.progress_target = None;
        self.error_message = None;
        self.admin_notice = None;
        self.logs.clear();
        self.stop_busy_animation();
    }

    pub fn log_lines(&self) -> Vec<SharedString> {
        self.logs.iter().cloned().collect()
    }

    pub fn is_busy(&self) -> bool {
        self.operation.is_some()
    }

    pub fn has_active_request(&self) -> bool {
        self.visible
    }

    fn start_busy_animation(&mut self) {
        self.start_busy_animation_at(Instant::now());
    }

    fn start_busy_animation_at(&mut self, now: Instant) {
        if self.busy_animation_started_at.is_none() {
            self.busy_animation_started_at = Some(now);
        }
    }

    fn stop_busy_animation(&mut self) {
        self.busy_animation_started_at = None;
    }

    pub fn busy_animation_rotation(&self, now: Instant) -> f32 {
        let Some(started_at) = self.busy_animation_started_at else {
            return 0.0;
        };

        let frame = now.saturating_duration_since(started_at).as_millis()
            / BUSY_ANIMATION_FRAME_INTERVAL.as_millis().max(1);
        let cycle_frames = (900 / BUSY_ANIMATION_FRAME_INTERVAL.as_millis().max(1)).max(1);
        ((frame % cycle_frames) as f32 / cycle_frames as f32) * std::f32::consts::TAU
    }

    pub fn next_busy_animation_deadline(&self, now: Instant) -> Option<Instant> {
        if !self.is_busy() {
            return None;
        }

        let started_at = self.busy_animation_started_at?;
        let elapsed = now.saturating_duration_since(started_at);
        let frame_count = elapsed.as_nanos() / BUSY_ANIMATION_FRAME_INTERVAL.as_nanos().max(1);
        Some(started_at + BUSY_ANIMATION_FRAME_INTERVAL * (frame_count as u32 + 1))
    }
}

#[cfg(test)]
mod tests {
    use super::{BUSY_ANIMATION_FRAME_INTERVAL, LaunchPrereqState, PendingLaunchVersion};
    use gpui::SharedString;
    use std::time::Instant;

    fn pending_version() -> PendingLaunchVersion {
        PendingLaunchVersion {
            folder: SharedString::from("folder"),
            name: SharedString::from("Version"),
            version: SharedString::from("1.20.0"),
            kind: SharedString::from("release"),
            path: SharedString::from("C:/Minecraft"),
            launch_args: None,
        }
    }

    #[test]
    fn busy_animation_deadline_advances_by_low_fps_interval() {
        let mut state = LaunchPrereqState::default();
        let now = Instant::now();

        state.begin(pending_version());
        let started_at = state
            .busy_animation_started_at
            .expect("busy animation should start");
        let first = state.next_busy_animation_deadline(now);
        let second = state.next_busy_animation_deadline(first.expect("first deadline"));

        assert_eq!(first, Some(started_at + BUSY_ANIMATION_FRAME_INTERVAL));
        assert_eq!(second, Some(started_at + BUSY_ANIMATION_FRAME_INTERVAL * 2));
    }

    #[test]
    fn busy_animation_stops_when_operation_finishes() {
        let mut state = LaunchPrereqState::default();
        let now = Instant::now();

        let request_id = state.begin(pending_version());
        assert!(state.next_busy_animation_deadline(now).is_some());

        let check = crate::core::minecraft::launcher::preflight::LaunchPrerequisiteCheck {
            platform: crate::core::minecraft::launcher::preflight::LaunchPlatform::Uwp,
            developer_mode_required: false,
            missing_uwp_dependencies: Vec::new(),
            game_input_plan: None,
            windows_app_sdk_plan: None,
        };
        state.set_check_if_matches(request_id, check);

        assert!(state.next_busy_animation_deadline(now).is_none());
    }

    #[test]
    fn active_request_remains_active_after_issues_are_shown() {
        let mut state = LaunchPrereqState::default();
        let request_id = state.begin(pending_version());
        let check = crate::core::minecraft::launcher::preflight::LaunchPrerequisiteCheck {
            platform: crate::core::minecraft::launcher::preflight::LaunchPlatform::Uwp,
            developer_mode_required: false,
            missing_uwp_dependencies: Vec::new(),
            game_input_plan: None,
            windows_app_sdk_plan: None,
        };

        state.set_check_if_matches(request_id, check);

        assert!(state.has_active_request());
        assert!(!state.is_busy());
    }
}
