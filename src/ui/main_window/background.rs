use super::*;
use tracing::{info, instrument};

pub(crate) const CUSTOM_BACKGROUND_PIPELINE_ENABLED: bool = true;
const BACKGROUND_ANIMATION_MAX_FPS: f32 = 12.0;
const BACKGROUND_GPU_FOREGROUND_BLUR_ENABLED: bool = true;
// The configured background blur belongs to the background image itself. Using a fullscreen
// backdrop-filter here makes every later scene primitive depend on a permanent full-window capture
// and turns otherwise unrelated retained updates into backdrop-composition dependencies. Keep true
// backdrop blur for glass/modal surfaces; blur the image layer directly instead.
const BACKGROUND_BLUR_OVERLAY_REFERENCE_PX: f32 = 24.0;
const BACKGROUND_BLUR_OVERLAY_MAX_ALPHA: f32 = 0.22;

pub(crate) fn startup_trace_origin() -> Instant {
    static STARTUP_TRACE_ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *STARTUP_TRACE_ORIGIN.get_or_init(Instant::now)
}

pub(crate) fn startup_trace_elapsed_ms() -> f64 {
    startup_trace_origin().elapsed().as_secs_f64() * 1000.0
}

pub(super) struct AppBackgroundView {
    bootstrap_background_option: SharedString,
    bootstrap_local_image_path: SharedString,
    bootstrap_network_image_url: SharedString,
    last_background_error_signature: Option<String>,
    _subscriptions: Vec<Subscription>,
    last_background_settings: Option<BackgroundSettingsSnapshot>,
    cached_display_background: Option<BackgroundSource>,
    animation_suppressed: bool,
    startup_first_paint_logged: bool,
    preloaded_background_resource: Option<AssetLocation>,
    preloaded_background_task: Option<CompressedImageTask>,
}

impl AppBackgroundView {
    pub(super) fn new(
        bootstrap_background_option: SharedString,
        bootstrap_local_image_path: SharedString,
        bootstrap_network_image_url: SharedString,
        cx: &mut Context<Self>,
    ) -> Self {
        let _ = startup_trace_origin();
        let _subscriptions = vec![
            cx.observe_global::<crate::ui::views::settings::state::SettingsPageState>(
                |this, cx| {
                    let settings = this.read_background_settings_snapshot(cx);
                    if this.last_background_settings.as_ref() != Some(&settings) {
                        cx.notify();
                    }
                },
            ),
        ];
        Self {
            bootstrap_background_option,
            bootstrap_local_image_path,
            bootstrap_network_image_url,
            last_background_error_signature: None,
            _subscriptions,
            last_background_settings: None,
            cached_display_background: None,
            animation_suppressed: false,
            startup_first_paint_logged: false,
            preloaded_background_resource: None,
            preloaded_background_task: None,
        }
    }

    pub(super) fn set_animation_suppressed(&mut self, suppressed: bool) -> bool {
        let changed = animation_suppression_changed(self.animation_suppressed, suppressed);
        if !changed {
            return false;
        }

        self.animation_suppressed = suppressed;
        changed
    }

    pub(super) fn reset_to_default_background(&mut self) {
        self.last_background_error_signature = None;
    }

    fn read_background_settings_snapshot(&self, cx: &App) -> BackgroundSettingsSnapshot {
        let settings: &crate::ui::views::settings::state::SettingsPageState =
            cx.global::<crate::ui::views::settings::state::SettingsPageState>();

        BackgroundSettingsSnapshot {
            loaded: settings.loaded,
            background_option: settings.background_option.to_string(),
            local_image_path: settings.local_image_path.to_string(),
            network_image_url: settings.network_image_url.to_string(),
            background_blur: normalize_background_blur_for_rendering(
                settings.background_blur_preview,
            ),
            network_image_refresh_nonce: settings.network_image_refresh_nonce,
        }
    }

    fn animation_policy(&self, window: &Window) -> ImageAnimationPolicy {
        background_animation_policy(self.animation_suppressed, window.is_window_active())
    }

    fn render_background_layer(
        &self,
        source: &BackgroundSource,
        animation_policy: ImageAnimationPolicy,
    ) -> AnyElement {
        match source {
            BackgroundSource::None => div().absolute().inset_0().into_any_element(),
            BackgroundSource::FetchedImage(image) => img(image.clone())
                .animation_policy(animation_policy)
                .id("main-window-background-image")
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            BackgroundSource::Embedded(path) => img(path.clone())
                .animation_policy(animation_policy)
                .id("main-window-background-image")
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            BackgroundSource::LocalPath(path) => img(path.clone())
                .animation_policy(animation_policy)
                .id("main-window-background-image")
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            BackgroundSource::NetworkUrl(url) => img(url.clone())
                .animation_policy(animation_policy)
                .id("main-window-background-image")
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
        }
    }

    fn render_background_container(
        &self,
        source: Option<&BackgroundSource>,
        blur: f32,
        animation_policy: ImageAnimationPolicy,
    ) -> Div {
        let container = div().absolute().inset_0().bg(gpui::transparent_black());
        let blur = normalize_background_blur_for_rendering(blur);
        let container = match source {
            Some(source) => {
                let layer = self.render_background_layer(source, animation_policy);
                if background_uses_gpu_blur(blur) {
                    container.child(
                        div()
                            .absolute()
                            .inset_0()
                            .blur(background_foreground_blur_radius(blur))
                            .child(layer),
                    )
                } else {
                    container.child(layer)
                }
            }
            None => container,
        };

        if blur == 0.0 {
            return container;
        }

        // Keep the inexpensive tint as an independent foreground quad. It must not participate in
        // the image blur, otherwise its alpha would be blurred into the window edges.
        container.child(
            div()
                .absolute()
                .inset_0()
                .bg(background_blur_overlay_color(blur)),
        )
    }

    fn update_cached_background_source(&mut self, settings: &BackgroundSettingsSnapshot) {
        let source_changed = self
            .last_background_settings
            .as_ref()
            .is_none_or(|previous| !previous.has_same_source(settings));
        if !source_changed && self.cached_display_background.is_some() {
            return;
        }

        let source = if settings.loaded {
            resolve_background_source_from_values(
                &settings.background_option,
                &settings.local_image_path,
                &settings.network_image_url,
                settings.network_image_refresh_nonce,
            )
        } else {
            resolve_background_source_from_values(
                self.bootstrap_background_option.as_ref(),
                self.bootstrap_local_image_path.as_ref(),
                self.bootstrap_network_image_url.as_ref(),
                0,
            )
        };
        if matches!(source, BackgroundSource::None) {
            self.reset_to_default_background();
        }
        self.cached_display_background = Some(source);
    }

    fn sync_preloaded_background_resource(
        &mut self,
        source: Option<&BackgroundSource>,
        cx: &mut App,
    ) {
        let next_resource = source.and_then(background_resource);
        if self.preloaded_background_resource == next_resource {
            return;
        }

        if let Some(previous_resource) = self.preloaded_background_resource.take() {
            self.preloaded_background_task.take();
            if next_resource.as_ref() != Some(&previous_resource) {
                cx.remove_compressed_image_resource(&previous_resource);
            }
        }

        if let Some(resource) = next_resource {
            let task = cx
                .preload_compressed_image_resources([resource.clone()])
                .into_iter()
                .next();
            self.preloaded_background_resource = Some(resource);
            self.preloaded_background_task = task;
        }
    }
}

fn normalize_background_blur_for_rendering(blur: f32) -> f32 {
    let blur = crate::config::config::clamp_background_blur(blur);
    if blur.is_finite() && blur > 0.0 {
        blur
    } else {
        0.0
    }
}

fn background_uses_gpu_blur(blur: f32) -> bool {
    BACKGROUND_GPU_FOREGROUND_BLUR_ENABLED && blur.is_finite() && blur > 0.0
}

fn background_foreground_blur_radius(blur: f32) -> Pixels {
    // Preserve the user-visible radius mapping used by the previous backdrop implementation while
    // changing only which pixels are filtered.
    px(blur / 3.0)
}

fn background_blur_overlay_color(blur: f32) -> gpui::Hsla {
    let alpha = (blur / BACKGROUND_BLUR_OVERLAY_REFERENCE_PX).clamp(0.0, 1.0)
        * BACKGROUND_BLUR_OVERLAY_MAX_ALPHA;
    gpui::Hsla {
        a: alpha,
        ..gpui::transparent_black()
    }
}

fn background_animation_policy(
    animation_suppressed: bool,
    window_active: bool,
) -> ImageAnimationPolicy {
    if animation_suppressed || !window_active {
        ImageAnimationPolicy::paused()
    } else {
        ImageAnimationPolicy {
            play: true,
            max_fps: Some(BACKGROUND_ANIMATION_MAX_FPS),
            inactive_max_fps: None,
        }
    }
}

fn animation_suppression_changed(current: bool, next: bool) -> bool {
    current != next
}

impl Render for AppBackgroundView {
    #[instrument(name = "AppBackgroundView::render", skip_all)]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let animation_policy = self.animation_policy(window);
        if !CUSTOM_BACKGROUND_PIPELINE_ENABLED {
            let default_background = default_background_source();
            return self.render_background_container(
                Some(&default_background),
                crate::config::config::default_background_blur(),
                animation_policy,
            );
        }

        let settings = self.read_background_settings_snapshot(cx);
        self.update_cached_background_source(&settings);
        if !self.startup_first_paint_logged {
            self.startup_first_paint_logged = true;
            info!(
                "startup_trace: background_first_paint t={:.3}ms",
                startup_trace_elapsed_ms()
            );
        }

        let display_background = self.cached_display_background.clone();
        self.sync_preloaded_background_resource(display_background.as_ref(), cx);
        self.last_background_settings = Some(settings.clone());

        self.render_background_container(
            display_background.as_ref(),
            settings.background_blur,
            animation_policy,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BACKGROUND_ANIMATION_MAX_FPS, BACKGROUND_BLUR_OVERLAY_MAX_ALPHA,
        BACKGROUND_GPU_FOREGROUND_BLUR_ENABLED, animation_suppression_changed,
        background_animation_policy, background_blur_overlay_color, background_uses_gpu_blur,
        normalize_background_blur_for_rendering,
    };

    #[test]
    fn background_animation_policy_pauses_when_suppressed() {
        let policy = background_animation_policy(true, true);
        assert!(!policy.play);
    }

    #[test]
    fn background_animation_policy_pauses_when_window_inactive() {
        let policy = background_animation_policy(false, false);
        assert!(!policy.play);
    }

    #[test]
    fn background_animation_policy_caps_active_playback() {
        let policy = background_animation_policy(false, true);
        assert!(policy.play);
        assert_eq!(policy.max_fps, Some(BACKGROUND_ANIMATION_MAX_FPS));
    }

    #[test]
    fn background_animation_suppression_change_reports_dirty() {
        assert!(animation_suppression_changed(true, false));
        assert!(!animation_suppression_changed(false, false));
    }

    #[test]
    fn only_zero_or_non_finite_background_blur_is_an_identity_effect() {
        assert!(BACKGROUND_GPU_FOREGROUND_BLUR_ENABLED);
        assert_eq!(normalize_background_blur_for_rendering(0.0), 0.0);
        assert_eq!(normalize_background_blur_for_rendering(0.1), 0.1);
        assert_eq!(normalize_background_blur_for_rendering(0.5), 0.5);
        assert_eq!(normalize_background_blur_for_rendering(f32::NAN), 0.0);
        assert!(!background_uses_gpu_blur(0.0));
        assert!(!background_uses_gpu_blur(f32::NAN));
        assert!(background_uses_gpu_blur(0.1));
        assert!(background_uses_gpu_blur(0.5));
        assert!(background_uses_gpu_blur(1.0));
        assert_eq!(background_blur_overlay_color(0.0).a, 0.0);
        assert_eq!(
            background_blur_overlay_color(f32::MAX).a,
            BACKGROUND_BLUR_OVERLAY_MAX_ALPHA
        );
    }
}
