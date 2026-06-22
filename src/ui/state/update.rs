use crate::tasks::task_manager::TaskSnapshot;
use crate::ui::animation::{ease_in_cubic, ease_out_back, is_running};
use crate::ui::components::markdown_renderer::MarkdownDocument;
use crate::utils::updater::ReleaseSummary;
use gpui::{Global, ScrollHandle};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

#[derive(Default)]
pub struct UpdateState {
    pub check_started: bool,
    pub checking: bool,
    pub available: Option<ReleaseSummary>,
    pub show_modal: bool,
    pub modal_shown_once: bool,
    pub last_error: Option<String>,

    // Download/apply flow (WebView2's useUpdaterWithModal equivalent).
    pub downloading: bool,
    pub task_id: Option<String>,
    pub task_updates: Option<broadcast::Receiver<Arc<TaskSnapshot>>>,
    pub last_task_snapshot: Option<Arc<TaskSnapshot>>,
    pub download_error: Option<String>,

    // Modal animation state
    /// Set when the UI has requested opening the modal, but we are waiting for a background
    /// snapshot to be captured before actually showing it.
    pub modal_pending_open: bool,
    /// Token that changes for each open request. Used by the view layer to invalidate snapshots.
    pub modal_open_requested_at: Option<Instant>,
    pub modal_animation_started_at: Option<Instant>,
    pub modal_animation_duration: Duration,
    pub modal_visible: bool,

    // Cached parsed markdown for the current release to avoid re-parsing on every render
    pub(crate) cached_release_tag: Option<String>,
    pub(crate) cached_md_document: Arc<MarkdownDocument>,
    pub markdown_cache_refresh_inflight: bool,
    pub changelog_scroll_handle: ScrollHandle,
}

impl Global for UpdateState {}

impl UpdateState {
    pub fn begin_download(
        &mut self,
        task_id: String,
        task_updates: broadcast::Receiver<Arc<TaskSnapshot>>,
    ) {
        self.downloading = true;
        self.task_id = Some(task_id);
        self.task_updates = Some(task_updates);
        self.last_task_snapshot = None;
        self.download_error = None;
    }

    pub fn cancel_download(&mut self) {
        self.downloading = false;
        self.task_id = None;
        self.task_updates = None;
        self.last_task_snapshot = None;
        self.download_error = None;
    }

    pub fn finish_download(&mut self) {
        self.downloading = false;
        self.task_updates = None;
    }

    pub fn fail_download(&mut self, error: impl Into<String>) {
        let error = error.into();
        if is_cancelled_download_error(&error) {
            self.cancel_download();
            return;
        }

        self.downloading = false;
        self.task_id = None;
        self.task_updates = None;
        self.last_task_snapshot = None;
        self.download_error = Some(error);
    }

    pub fn request_open_modal(&mut self, now: Instant) {
        if self.show_modal || self.modal_pending_open {
            return;
        }

        self.changelog_scroll_handle = ScrollHandle::new();
        self.modal_shown_once = true;
        self.modal_open_requested_at = Some(now);
        self.modal_pending_open = true;
        if self.needs_md_cache_refresh() {
            self.modal_animation_started_at = None;
            self.modal_animation_duration = Duration::from_millis(190);
            self.modal_visible = false;
        }
    }

    pub fn set_show_modal(&mut self, show: bool, now: Instant) {
        if show && self.show_modal && self.modal_visible && !self.modal_pending_open {
            return;
        }

        if !show {
            self.modal_pending_open = false;

            if !self.show_modal && !self.modal_visible {
                self.modal_animation_started_at = None;
                self.modal_animation_duration = Duration::ZERO;
                return;
            }

            if !self.modal_visible {
                if !self.is_modal_animating(now) {
                    self.show_modal = false;
                    self.modal_animation_started_at = None;
                    self.modal_animation_duration = Duration::ZERO;
                }
                return;
            }
        }

        self.modal_animation_started_at = Some(now);
        self.modal_animation_duration = if show {
            Duration::from_millis(140)
        } else {
            Duration::from_millis(250)
        };
        self.modal_visible = show;
        if show {
            self.changelog_scroll_handle = ScrollHandle::new();
            self.modal_pending_open = false;
            self.modal_open_requested_at.get_or_insert(now);
        }
        // Keep the modal in the render tree while it is open or animating closed.
        // The view layer clears `show_modal` once the close animation finishes.
        self.show_modal = self.show_modal || show;
    }

    pub fn modal_animation_factor(&self, now: Instant) -> f32 {
        if self.modal_visible && self.modal_animation_duration == Duration::ZERO {
            return 1.0;
        }

        let Some(t0) = self.modal_animation_started_at else {
            return if self.modal_visible { 1.0 } else { 0.0 };
        };
        let t = (now.saturating_duration_since(t0).as_secs_f32()
            / self
                .modal_animation_duration
                .max(Duration::from_millis(1))
                .as_secs_f32())
        .clamp(0.0, 1.0);

        if self.modal_visible {
            ease_out_back(t, 0.22).clamp(0.0, 1.04)
        } else {
            1.0 - ease_in_cubic(t)
        }
    }

    pub fn is_modal_animating(&self, now: Instant) -> bool {
        is_running(
            now,
            self.modal_animation_started_at,
            self.modal_animation_duration,
        )
    }

    pub fn is_modal_render_animating(&self, now: Instant) -> bool {
        self.show_modal && self.is_modal_animating(now)
    }

    pub fn should_render_modal(&self, now: Instant) -> bool {
        self.show_modal && (self.modal_visible || self.is_modal_animating(now))
    }

    pub fn finish_close_if_elapsed(&mut self, now: Instant) -> bool {
        if !self.show_modal || self.modal_visible || self.is_modal_animating(now) {
            return false;
        }

        self.show_modal = false;
        self.modal_animation_started_at = None;
        self.modal_animation_duration = Duration::ZERO;
        true
    }

    pub fn needs_md_cache_refresh(&self) -> bool {
        self.available
            .as_ref()
            .is_some_and(|release| self.cached_release_tag.as_ref() != Some(&release.tag))
    }

    pub fn cached_md_document(&self) -> Arc<MarkdownDocument> {
        self.cached_md_document.clone()
    }
}

pub(crate) fn is_cancelled_download_error(error: &str) -> bool {
    error.trim().eq_ignore_ascii_case("download cancelled")
}

#[cfg(test)]
mod tests {
    use super::UpdateState;
    use crate::utils::updater::ReleaseSummary;
    use std::time::{Duration, Instant};

    fn release(tag: &str) -> ReleaseSummary {
        ReleaseSummary {
            tag: tag.to_string(),
            name: None,
            prerelease: false,
            published_at: None,
            asset_name: None,
            asset_url: None,
            asset_size: None,
            body: None,
        }
    }

    #[test]
    fn open_modal_request_waits_for_markdown_cache_when_needed() {
        let mut state = UpdateState {
            available: Some(release("v9.9.9")),
            ..UpdateState::default()
        };

        state.request_open_modal(Instant::now());

        assert!(state.modal_pending_open);
        assert!(!state.show_modal);
        assert!(!state.modal_visible);
    }

    #[test]
    fn open_modal_request_is_applied_by_window_sync_even_when_cache_is_ready() {
        let mut state = UpdateState {
            available: Some(release("v9.9.9")),
            cached_release_tag: Some("v9.9.9".to_string()),
            ..UpdateState::default()
        };

        state.request_open_modal(Instant::now());

        assert!(state.modal_pending_open);
        assert!(!state.modal_visible);
    }

    #[test]
    fn hiding_already_hidden_modal_does_not_start_close_animation() {
        let mut state = UpdateState::default();
        let now = Instant::now();

        state.set_show_modal(false, now);

        assert!(!state.show_modal);
        assert!(!state.modal_visible);
        assert!(!state.is_modal_animating(now));
        assert!(state.modal_animation_started_at.is_none());
    }

    #[test]
    fn cancelled_download_error_is_not_persisted() {
        let mut state = UpdateState {
            downloading: true,
            task_id: Some("download-task".to_string()),
            download_error: Some("previous error".to_string()),
            ..UpdateState::default()
        };

        state.fail_download("Download cancelled");

        assert!(!state.downloading);
        assert!(state.task_id.is_none());
        assert!(state.download_error.is_none());
    }

    #[test]
    fn real_download_error_is_persisted() {
        let mut state = UpdateState {
            downloading: true,
            task_id: Some("download-task".to_string()),
            ..UpdateState::default()
        };

        state.fail_download("network unavailable");

        assert!(!state.downloading);
        assert!(state.task_id.is_none());
        assert_eq!(state.download_error.as_deref(), Some("network unavailable"));
    }

    #[test]
    fn hiding_closing_modal_does_not_restart_close_animation() {
        let mut state = UpdateState::default();
        let open_at = Instant::now();
        state.set_show_modal(true, open_at);

        let close_at = open_at + Duration::from_millis(20);
        state.set_show_modal(false, close_at);
        let started_at = state.modal_animation_started_at;

        state.set_show_modal(false, close_at + Duration::from_millis(20));

        assert_eq!(state.modal_animation_started_at, started_at);
        assert!(state.show_modal);
        assert!(!state.modal_visible);
    }

    #[test]
    fn stale_animation_without_show_modal_does_not_render_modal() {
        let mut state = UpdateState::default();
        let now = Instant::now();

        state.modal_animation_started_at = Some(now);
        state.modal_animation_duration = Duration::from_millis(250);

        assert!(state.is_modal_animating(now));
        assert!(!state.is_modal_render_animating(now));
        assert!(!state.should_render_modal(now));
    }

    #[test]
    fn closing_modal_finishes_after_animation_elapsed() {
        let mut state = UpdateState::default();
        let now = Instant::now();

        state.set_show_modal(true, now);
        state.set_show_modal(false, now + Duration::from_millis(20));

        assert!(state.show_modal);
        assert!(!state.modal_visible);

        let finished = state.finish_close_if_elapsed(now + Duration::from_secs(1));

        assert!(finished);
        assert!(!state.show_modal);
        assert!(!state.should_render_modal(now + Duration::from_secs(1)));
        assert!(state.modal_animation_started_at.is_none());
    }
}
