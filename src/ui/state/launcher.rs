use crate::tasks::task_manager::TaskSnapshot;
use crate::ui::animation::{ease_in_cubic, ease_out_back, is_running};
use gpui::{Global, ScrollHandle, SharedString};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct LauncherState {
    pub show_modal: bool,
    pub modal_visible: bool,
    pub modal_animation_started_at: Option<Instant>,
    pub modal_animation_duration: Duration,
    pub task_id: Option<Arc<str>>,
    pub version_folder: SharedString,
    pub version_name: SharedString,
    pub version: SharedString,
    pub kind: SharedString,
    pub package_path: SharedString,
    pub launch_args: SharedString,
    pub loader_version: SharedString,
    pub last_snapshot: Option<Arc<TaskSnapshot>>,
    pub log_scroll_handle: ScrollHandle,
}

impl Global for LauncherState {}

impl Default for LauncherState {
    fn default() -> Self {
        Self {
            show_modal: false,
            modal_visible: false,
            modal_animation_started_at: None,
            modal_animation_duration: Duration::default(),
            task_id: None,
            version_folder: SharedString::default(),
            version_name: SharedString::default(),
            version: SharedString::default(),
            kind: SharedString::default(),
            package_path: SharedString::default(),
            launch_args: SharedString::default(),
            loader_version: SharedString::default(),
            last_snapshot: None,
            log_scroll_handle: ScrollHandle::new(),
        }
    }
}

impl LauncherState {
    pub fn begin(
        &mut self,
        task_id: impl Into<Arc<str>>,
        version_folder: SharedString,
        version_name: SharedString,
        version: SharedString,
        kind: SharedString,
        package_path: SharedString,
        launch_args: Option<SharedString>,
        loader_version: SharedString,
        now: Instant,
    ) {
        self.show_modal = true;
        self.modal_visible = true;
        self.modal_animation_started_at = Some(now);
        self.modal_animation_duration = Duration::from_millis(320);
        self.task_id = Some(task_id.into());
        self.version_folder = version_folder;
        self.version_name = version_name;
        self.version = version;
        self.kind = kind;
        self.package_path = package_path;
        self.launch_args = launch_args.unwrap_or_default();
        self.loader_version = loader_version;
        self.last_snapshot = None;
        self.log_scroll_handle = ScrollHandle::new();
    }

    pub fn apply_snapshot(&mut self, snapshot: Arc<TaskSnapshot>) {
        self.last_snapshot = Some(snapshot);
    }

    pub fn request_close(&mut self, now: Instant) {
        if !self.show_modal {
            return;
        }
        self.modal_visible = false;
        self.modal_animation_started_at = Some(now);
        self.modal_animation_duration = Duration::from_millis(240);
    }

    pub fn launch_in_progress(&self) -> bool {
        if let Some(snapshot) = self.last_snapshot.as_ref() {
            return matches!(
                snapshot.status.as_ref(),
                "running" | "paused" | "cancelling"
            );
        }

        self.task_id.is_some()
    }

    pub fn dismiss_modal(&mut self) {
        self.show_modal = false;
        self.modal_visible = false;
        self.modal_animation_started_at = None;
    }

    pub fn finish_close(&mut self) {
        self.show_modal = false;
        self.modal_visible = false;
        self.modal_animation_started_at = None;
        self.modal_animation_duration = Duration::ZERO;
        self.task_id = None;
        self.version_folder = SharedString::default();
        self.version_name = SharedString::default();
        self.version = SharedString::default();
        self.kind = SharedString::default();
        self.package_path = SharedString::default();
        self.launch_args = SharedString::default();
        self.loader_version = SharedString::default();
        self.last_snapshot = None;
        self.log_scroll_handle = ScrollHandle::new();
    }

    pub fn finish_close_if_elapsed(&mut self, now: Instant) -> bool {
        if !self.show_modal || self.modal_visible || self.is_modal_animating(now) {
            return false;
        }

        self.finish_close();
        true
    }

    pub fn modal_animation_factor(&self, now: Instant) -> f32 {
        let Some(started_at) = self.modal_animation_started_at else {
            return if self.modal_visible { 1.0 } else { 0.0 };
        };
        let progress = (now.saturating_duration_since(started_at).as_secs_f32()
            / self
                .modal_animation_duration
                .max(Duration::from_millis(1))
                .as_secs_f32())
        .clamp(0.0, 1.0);
        if self.modal_visible {
            ease_out_back(progress, 0.22).clamp(0.0, 1.04)
        } else {
            1.0 - ease_in_cubic(progress)
        }
    }

    pub fn is_modal_animating(&self, now: Instant) -> bool {
        is_running(
            now,
            self.modal_animation_started_at,
            self.modal_animation_duration,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::LauncherState;
    use crate::tasks::task_manager::TaskSnapshot;
    use gpui::SharedString;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn closing_modal_finishes_after_animation_elapsed() {
        let mut state = LauncherState::default();
        let now = Instant::now();

        state.begin(
            "task-1",
            SharedString::from("folder"),
            SharedString::from("version"),
            SharedString::from("1.20.0"),
            SharedString::from("release"),
            SharedString::from("C:/Minecraft"),
            None,
            SharedString::from(""),
            now,
        );
        state.request_close(now + Duration::from_millis(20));

        assert!(state.show_modal);
        assert!(!state.modal_visible);

        let finished = state.finish_close_if_elapsed(now + Duration::from_secs(1));

        assert!(finished);
        assert!(!state.show_modal);
        assert!(!state.modal_visible);
        assert!(!state.is_modal_animating(now + Duration::from_secs(1)));
        assert!(state.task_id.is_none());
    }

    #[test]
    fn launch_in_progress_tracks_running_snapshot() {
        let mut state = launcher_with_status("running");

        assert!(state.launch_in_progress());

        state.last_snapshot = Some(task_snapshot("error"));

        assert!(!state.launch_in_progress());
    }

    fn launcher_with_status(status: &'static str) -> LauncherState {
        let mut state = LauncherState::default();
        state.task_id = Some(Arc::from("task-1"));
        state.last_snapshot = Some(task_snapshot(status));
        state
    }

    fn task_snapshot(status: &'static str) -> Arc<TaskSnapshot> {
        Arc::new(TaskSnapshot {
            id: Arc::from("task-1"),
            title: Arc::from("title"),
            detail: None,
            stage: Arc::from("stage"),
            total: None,
            done: 0,
            speed_bytes_per_sec: 0.0,
            eta: Arc::from("unknown"),
            percent: None,
            status: Arc::from(status),
            cancel_requested: false,
            message: None,
            supports_pause: false,
            visualization: None,
            started_at_unix: 0,
            last_update_unix: 0,
            sequence: 0,
            visibility: crate::tasks::task_manager::TaskVisibility::Visible,
        })
    }
}
