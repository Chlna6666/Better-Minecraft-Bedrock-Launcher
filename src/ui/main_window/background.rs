use super::*;
use tracing::{info, instrument};

pub(crate) const CUSTOM_BACKGROUND_PIPELINE_ENABLED: bool = true;
const BACKGROUND_ANIMATION_MAX_FPS: f32 = 12.0;
const BACKGROUND_GPU_BACKDROP_BLUR_ENABLED: bool = true;
// Background blur is a full-background effect. Nova now isolates backdrop sources by draw order,
// so the application no longer needs to carve a 60px hole around the titlebar to avoid sharing
// filter results with the titlebar glass.
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
    preloaded_background_target: Option<ImageRenderRequest>,
    preloaded_background_task: Option<SizedImageTask>,
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
            preloaded_background_target: None,
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
            background_blur: crate::config::config::clamp_background_blur(
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
        let blur = crate::config::config::clamp_background_blur(blur);
        let container = match source {
            Some(source) => container.child(self.render_background_layer(source, animation_policy)),
            None => container,
        };

        if blur <= 0.0 {
            return container;
        }

        let overlay = div()
            .absolute()
            .inset_0()
            .bg(background_blur_overlay_color(blur));

        if background_uses_gpu_blur(blur) {
            container.child(overlay.backdrop_blur(background_backdrop_blur_style(blur)))
        } else {
            container.child(overlay)
        }
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
        window: &mut Window,
        cx: &mut App,
    ) {
        let next_resource = source.and_then(background_resource);
        let next_target = next_resource.clone().and_then(|resource| {
            cx.image_render_request(
                resource,
                window.viewport_size(),
                window.scale_factor(),
                ObjectFit::Cover,
            )
        });
        if self.preloaded_background_resource == next_resource
            && self.preloaded_background_target == next_target
        {
            return;
        }

        if let Some(previous_resource) = self.preloaded_background_resource.take() {
            self.preloaded_background_task.take();
            if let Some(previous_target) = self.preloaded_background_target.take() {
                cx.remove_image_render_request_in(&previous_target, Some(window));
                if next_resource.as_ref() != Some(&previous_resource) {
                    cx.remove_compressed_image_resource(&previous_resource);
                }
            }
        }

        if let (Some(resource), Some(target)) = (next_resource, next_target) {
            let task = cx.preload_sized_image(target.clone());
            self.preloaded_background_resource = Some(resource);
            self.preloaded_background_target = Some(target);
            self.preloaded_background_task = Some(task);
        }
    }
}

fn background_uses_gpu_blur(blur: f32) -> bool {
    BACKGROUND_GPU_BACKDROP_BLUR_ENABLED && blur.is_finite() && blur > 0.0
}

fn background_backdrop_blur_style(blur: f32) -> BackdropBlurStyle {
    // Background blur is capped at 10px by configuration, so keep it full-resolution. The new
    // separable Gaussian uses exactly two passes and no longer needs Dual-Kawase levels.
    BackdropBlurStyle::new(px(blur)).downsample(1).levels(2)
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
    if animation_suppressed {
        ImageAnimationPolicy::paused()
    } else {
        ImageAnimationPolicy {
            play: true,
            max_fps: Some(BACKGROUND_ANIMATION_MAX_FPS),
            inactive_max_fps: Some(if window_active {
                BACKGROUND_ANIMATION_MAX_FPS
            } else {
                1.0
            }),
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
        self.sync_preloaded_background_resource(display_background.as_ref(), window, cx);
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
        BACKGROUND_GPU_BACKDROP_BLUR_ENABLED, animation_suppression_changed,
        background_animation_policy, background_blur_overlay_color, background_uses_gpu_blur,
    };

    #[test]
    fn background_animation_policy_pauses_when_suppressed() {
        let policy = background_animation_policy(true, true);
        assert!(!policy.play);
    }

    #[test]
    fn background_animation_policy_throttles_when_window_inactive() {
        let policy = background_animation_policy(false, false);
        assert!(policy.play);
        assert_eq!(policy.inactive_max_fps, Some(1.0));
    }

    #[test]
    fn background_animation_policy_caps_active_playback() {
        let policy = background_animation_policy(false, true);
        assert_eq!(policy.max_fps, Some(BACKGROUND_ANIMATION_MAX_FPS));
    }

    #[test]
    fn background_animation_suppression_change_reports_dirty() {
        assert!(animation_suppression_changed(true, false));
        assert!(!animation_suppression_changed(false, false));
    }

    #[test]
    fn subpixel_background_blur_enters_gaussian_gpu_filter() {
        assert!(BACKGROUND_GPU_BACKDROP_BLUR_ENABLED);
        assert!(!background_uses_gpu_blur(0.0));
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
