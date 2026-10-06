use super::mesh::{
    SkinLayerMode, SkinPreviewGeometrySource, SkinPreviewMeshes, skin_player_mesh,
    skin_preview_scene_view,
};
use super::selector::{
    render_current_preview, render_skin_selector, skin_selector_page_count,
    skin_selector_page_for_index,
};
use crate::ui::state::i18n::I18n;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};
use gpui::AnimationExt as _;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

const SKIN_PREVIEW_WINDOW_WIDTH: f32 = 640.0;
const SKIN_PREVIEW_WINDOW_HEIGHT: f32 = 600.0;
const SKIN_PREVIEW_STAGE_MAX_WIDTH: f32 = 608.0;
const SKIN_PREVIEW_STAGE_MAX_HEIGHT: f32 = 360.0;
const SKIN_PREVIEW_MIN_ZOOM: f32 = 0.72;
const SKIN_PREVIEW_MAX_ZOOM: f32 = 1.65;
const SKIN_PREVIEW_DEFAULT_ZOOM: f32 = 1.0;
const SKIN_PREVIEW_WHEEL_ZOOM_PER_LINE: f32 = 1.10;
const SKIN_PREVIEW_MAX_WHEEL_LINES_PER_EVENT: f32 = 4.0;

fn skin_preview_walk_time(walking: bool, started_at: Instant, now: Instant) -> Duration {
    if walking {
        now.saturating_duration_since(started_at)
    } else {
        Duration::ZERO
    }
}

fn skin_preview_wheel_zoom_factor(delta_y: f32, line_height: f32) -> f32 {
    if !delta_y.is_finite() || !line_height.is_finite() {
        return 1.0;
    }
    let line_height = line_height.abs().max(1.0);
    let lines = (delta_y / line_height).clamp(
        -SKIN_PREVIEW_MAX_WHEEL_LINES_PER_EVENT,
        SKIN_PREVIEW_MAX_WHEEL_LINES_PER_EVENT,
    );
    SKIN_PREVIEW_WHEEL_ZOOM_PER_LINE.powf(lines)
}

#[derive(Clone)]
pub struct SkinPreviewWindowSkin {
    pub display_name: SharedString,
    pub texture_path: SharedString,
    pub model_label: Option<SharedString>,
    pub preview_path: Option<SharedString>,
    pub geometry_path: Option<SharedString>,
    pub geometry_identifier: Option<SharedString>,
}

#[derive(Clone)]
pub struct SkinPreviewWindowInit {
    pub title: SharedString,
    pub skins: Arc<[SkinPreviewWindowSkin]>,
    pub selected_index: usize,
}

pub struct SkinPreviewWindowView {
    title: SharedString,
    skins: Arc<[SkinPreviewWindowSkin]>,
    selected_index: usize,
    mesh: Option<Result<Arc<SkinPreviewMeshes>, SharedString>>,
    mesh_request_id: u64,
    walking: bool,
    walk_started_at: Instant,
    view_yaw: f32,
    view_pitch: f32,
    view_zoom: f32,
    drag_position: Option<Point<Pixels>>,
    layer_mode: SkinLayerMode,
    selector_expanded: bool,
    selector_page: usize,
    _subscriptions: Vec<Subscription>,
}

impl SkinPreviewWindowView {
    fn new(init: SkinPreviewWindowInit, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe_global::<ThemeState>(|_, cx| {
                cx.notify();
            }),
            cx.observe_global_in::<I18n>(window, |this, window, cx| {
                let title = t!("SkinPreview.window_title", name = &this.title);
                window.set_title(title.as_ref());
                cx.notify();
            }),
        ];
        let selected_index = init.selected_index.min(init.skins.len().saturating_sub(1));
        let selector_page = skin_selector_page_for_index(selected_index);
        let mut this = Self {
            title: init.title,
            skins: init.skins,
            selected_index,
            mesh: None,
            mesh_request_id: 0,
            walking: false,
            walk_started_at: Instant::now(),
            view_yaw: 0.0,
            view_pitch: 0.0,
            view_zoom: SKIN_PREVIEW_DEFAULT_ZOOM,
            drag_position: None,
            layer_mode: SkinLayerMode::Extruded,
            selector_expanded: false,
            selector_page,
            _subscriptions: subscriptions,
        };
        this.load_mesh(cx);
        this
    }

    fn load_mesh(&mut self, cx: &mut Context<Self>) {
        let Some(skin) = self.current_skin().cloned() else {
            self.mesh = Some(Err(t!("SkinPreview.no_texture")));
            cx.notify();
            return;
        };
        self.mesh = None;
        self.mesh_request_id = self.mesh_request_id.saturating_add(1);
        let request_id = self.mesh_request_id;
        let texture_path = skin.texture_path.to_string();
        let slim_arms = skin
            .model_label
            .as_ref()
            .is_some_and(|label| label.as_ref().eq_ignore_ascii_case("Alex"));
        let layer_mode = self.layer_mode;
        let geometry_source = skin
            .geometry_path
            .as_ref()
            .zip(skin.geometry_identifier.as_ref())
            .map(|(path, identifier)| SkinPreviewGeometrySource {
                path: path.to_string(),
                identifier: identifier.to_string(),
            });
        cx.notify();
        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    skin_player_mesh(
                        Path::new(&texture_path),
                        slim_arms,
                        layer_mode,
                        geometry_source,
                    )
                })
                .await
                .map_err(SharedString::from);

            if let Err(error) = handle.update(cx, |this, cx| {
                if this.mesh_request_id == request_id {
                    this.mesh = Some(result);
                    cx.notify();
                }
            }) {
                eprintln!("Failed to update skin preview mesh: {error:?}");
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn theme_colors(&self, now: Instant, cx: &App) -> ThemeColors {
        let theme = cx.global::<ThemeState>();
        lerp_theme_colors(
            &LightColors::colors(),
            &DarkColors::colors(),
            theme.factor(now),
            theme.accent,
        )
    }

    fn current_skin(&self) -> Option<&SkinPreviewWindowSkin> {
        self.skins.get(self.selected_index)
    }

    fn current_model_label(&self) -> SharedString {
        self.current_skin()
            .and_then(|skin| skin.model_label.clone())
            .filter(|label| !label.as_ref().trim().is_empty())
            .unwrap_or_else(|| SharedString::from("Steve"))
    }

    fn current_skin_label(&self) -> SharedString {
        self.current_skin()
            .map(|skin| skin.display_name.clone())
            .filter(|label| !label.as_ref().trim().is_empty())
            .unwrap_or_else(|| self.title.clone())
    }

    pub(super) fn select_skin(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.skins.len() || index == self.selected_index {
            return;
        }

        self.selected_index = index;
        self.selector_page = skin_selector_page_for_index(index);
        self.walk_started_at = Instant::now();
        self.load_mesh(cx);
    }

    fn select_previous_skin(&mut self, cx: &mut Context<Self>) {
        if self.skins.len() < 2 {
            return;
        }
        let index = if self.selected_index == 0 {
            self.skins.len() - 1
        } else {
            self.selected_index - 1
        };
        self.select_skin(index, cx);
    }

    fn select_next_skin(&mut self, cx: &mut Context<Self>) {
        if self.skins.len() < 2 {
            return;
        }
        self.select_skin((self.selected_index + 1) % self.skins.len(), cx);
    }

    fn toggle_walking(&mut self, now: Instant, cx: &mut Context<Self>) {
        self.walking = !self.walking;
        self.walk_started_at = now;
        cx.notify();
    }

    fn toggle_layer_mode(&mut self, cx: &mut Context<Self>) {
        self.layer_mode = if self.layer_mode.is_extruded() {
            SkinLayerMode::Flat
        } else {
            SkinLayerMode::Extruded
        };
        self.load_mesh(cx);
    }

    fn zoom_preview_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        let next_zoom =
            (self.view_zoom * factor).clamp(SKIN_PREVIEW_MIN_ZOOM, SKIN_PREVIEW_MAX_ZOOM);
        if (next_zoom - self.view_zoom).abs() <= f32::EPSILON {
            return;
        }
        self.view_zoom = next_zoom;
        cx.notify();
    }

    pub(super) fn toggle_selector_expanded(&mut self, cx: &mut Context<Self>) {
        self.selector_page = skin_selector_page_for_index(self.selected_index);
        self.selector_expanded = !self.selector_expanded;
        cx.notify();
    }

    pub(super) fn select_previous_selector_page(&mut self, cx: &mut Context<Self>) {
        let page_count = skin_selector_page_count(self.skins.len());
        if page_count < 2 {
            return;
        }
        self.selector_page = if self.selector_page == 0 {
            page_count - 1
        } else {
            self.selector_page - 1
        };
        cx.notify();
    }

    pub(super) fn select_next_selector_page(&mut self, cx: &mut Context<Self>) {
        let page_count = skin_selector_page_count(self.skins.len());
        if page_count < 2 {
            return;
        }
        self.selector_page = (self.selector_page + 1) % page_count;
        cx.notify();
    }

    fn begin_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.drag_position = Some(position);
        cx.notify();
    }

    fn update_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !event.dragging() {
            self.drag_position = None;
            return;
        }
        let Some(previous) = self.drag_position.replace(event.position) else {
            cx.notify();
            return;
        };
        let delta_x = (event.position.x - previous.x) / px(1.0);
        let delta_y = (event.position.y - previous.y) / px(1.0);
        self.view_yaw += delta_x * 0.012;
        self.view_pitch = (self.view_pitch + delta_y * 0.010).clamp(-0.75, 0.45);
        cx.notify();
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        self.drag_position = None;
        cx.notify();
    }

    fn render_button(
        &self,
        colors: &ThemeColors,
        id: &'static str,
        icon: &'static str,
        active: bool,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .w(px(34.))
            .h(px(34.))
            .rounded(px(crate::ui::theme::tokens::radius::MD))
            .border_1()
            .border_color(if active { colors.accent } else { colors.border })
            .bg(if active {
                Hsla {
                    a: 0.12,
                    ..colors.accent
                }
            } else {
                colors.surface
            })
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .child(
                svg()
                    .path(icon)
                    .w(px(15.))
                    .h(px(15.))
                    .text_color(if active {
                        colors.accent
                    } else {
                        colors.text_secondary
                    }),
            )
    }

    fn render_canvas(
        &self,
        colors: &ThemeColors,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let _i18n = cx.global::<I18n>().clone();
        match &self.mesh {
            Some(Ok(mesh)) => {
                let mesh = mesh.clone();
                let view_yaw = self.view_yaw;
                let view_pitch = self.view_pitch;
                let view_zoom = self.view_zoom;
                let walk_time = skin_preview_walk_time(self.walking, self.walk_started_at, now);
                let scene_view = match skin_preview_scene_view(
                    &mesh, view_yaw, view_pitch, view_zoom, walk_time,
                ) {
                    Ok(scene_view) => scene_view,
                    Err(error) => {
                        return centered_status(
                            colors,
                            t!("SkinPreview.mesh_error", detail = error),
                        );
                    }
                };
                div()
                    .relative()
                    .size_full()
                    .overflow_hidden()
                    .bg(colors.surface)
                    .cursor_pointer()
                    .child(gpui_3d::scene_view(scene_view))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            this.begin_drag(event.position, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                        this.update_drag(event, cx);
                        cx.stop_propagation();
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                            this.end_drag(cx);
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                            this.end_drag(cx);
                            cx.stop_propagation();
                        }),
                    )
                    .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                        let line_height = window.line_height();
                        let delta_y = event.delta.pixel_delta(line_height).y / px(1.0);
                        let factor = skin_preview_wheel_zoom_factor(
                            delta_y,
                            line_height / px(1.0),
                        );
                        if (factor - 1.0).abs() > f32::EPSILON {
                            this.zoom_preview_by(factor, cx);
                        }
                        cx.stop_propagation();
                    }))
                    .into_any_element()
            }
            Some(Err(error)) => {
                centered_status(colors, t!("SkinPreview.mesh_error", detail = error))
            }
            None => centered_status(colors, t!("SkinPreview.generating")),
        }
    }
}

impl Render for SkinPreviewWindowView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = window.animation_time();
        let has_walk_animation = matches!(
            &self.mesh,
            Some(Ok(mesh)) if mesh.has_walk_animation()
        );
        if self.walking && has_walk_animation {
            window.request_animation_frame();
        }
        let colors = self.theme_colors(now, cx);
        let model_label = self.current_model_label();
        let skin_label = self.current_skin_label();
        let layer_label = if self.layer_mode.is_extruded() {
            t!("SkinPreview.layer_3d")
        } else {
            t!("SkinPreview.layer_flat")
        };
        let skin_counter = if self.skins.is_empty() {
            "0/0".to_string()
        } else {
            format!("{}/{}", self.selected_index + 1, self.skins.len())
        };

        div()
            .size_full()
            .bg(colors.settings_panel_bg)
            .text_color(colors.text_primary)
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(58.))
                    .px(px(18.))
                    .border_b_1()
                    .border_color(colors.border)
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .min_w(px(0.))
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(render_current_preview(self.current_skin(), &colors))
                            .child(
                                div()
                                    .min_w(px(0.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(3.))
                                    .child(
                                        div()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .text_size(px(14.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(self.title.clone()),
                                    )
                                    .child(
                                        div()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .text_size(px(11.))
                                            .text_color(colors.text_secondary)
                                            .child(t!(
                                                "SkinPreview.subtitle",
                                                layer = &layer_label,
                                                skin = &skin_label,
                                                counter = &skin_counter,
                                                model = &model_label
                                            )),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .when(self.skins.len() > 1, |this| {
                                this.child(
                                    self.render_button(
                                        &colors,
                                        "skin-preview-previous",
                                        lucide_gpui::icon!(chevron_left),
                                        false,
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.select_previous_skin(cx);
                                        }),
                                    ),
                                )
                                .child(
                                    self.render_button(
                                        &colors,
                                        "skin-preview-next",
                                        lucide_gpui::icon!(chevron_right),
                                        false,
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.select_next_skin(cx);
                                        }),
                                    ),
                                )
                            })
                            .child(
                                self.render_button(
                                    &colors,
                                    "skin-preview-toggle-layer-mode",
                                    lucide_gpui::icon!(layers_2),
                                    self.layer_mode.is_extruded(),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.toggle_layer_mode(cx);
                                    }),
                                ),
                            )
                            .child(
                                self.render_button(
                                    &colors,
                                    "skin-preview-toggle-motion",
                                    if self.walking {
                                        lucide_gpui::icon!(pause)
                                    } else {
                                        lucide_gpui::icon!(play)
                                    },
                                    self.walking,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, window, cx| {
                                        this.toggle_walking(window.animation_time(), cx);
                                    }),
                                ),
                            )
                            .child(
                                self.render_button(
                                    &colors,
                                    "skin-preview-close",
                                    lucide_gpui::icon!(x),
                                    false,
                                )
                                .on_click(|_event, window, _cx| {
                                    window.remove_window();
                                }),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .p(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .w_full()
                            .h_full()
                            .max_w(px(SKIN_PREVIEW_STAGE_MAX_WIDTH))
                            .max_h(px(SKIN_PREVIEW_STAGE_MAX_HEIGHT))
                            .rounded(px(crate::ui::theme::tokens::radius::SM))
                            .border_1()
                            .border_color(colors.border)
                            .overflow_hidden()
                            .child(self.render_canvas(&colors, now, cx)),
                    ),
            )
            .when(self.skins.len() > 1, |this| {
                this.child(render_skin_selector(
                    &self.skins,
                    self.selected_index,
                    self.selector_expanded,
                    self.selector_page,
                    &colors,
                    cx,
                ))
            })
    }
}

pub fn open_skin_preview_window(init: SkinPreviewWindowInit, cx: &mut App) {
    let title = t!("SkinPreview.window_title", name = &init.title).to_string();
    let options = skin_preview_window_options(cx);
    let window = cx.open_window(options, move |window, cx| {
        window.set_title(&title);
        window.on_window_should_close(cx, |window, _cx| {
            window.remove_window();
            true
        });
        window.activate_window();

        let view = cx.new(|cx| SkinPreviewWindowView::new(init, window, cx));
        cx.new(|cx| crate::ui::runtime::root_view::RootView::new(view, window, cx))
    });

    if let Err(error) = window {
        eprintln!("Failed to open skin preview window: {error:?}");
    }
}

fn skin_preview_window_options(cx: &mut App) -> WindowOptions {
    let mut options = WindowOptions::default();
    let fixed_size = size(
        px(SKIN_PREVIEW_WINDOW_WIDTH),
        px(SKIN_PREVIEW_WINDOW_HEIGHT),
    );
    options.window_bounds = Some(WindowBounds::centered(fixed_size, cx));
    options.window_min_size = Some(fixed_size);
    options.is_resizable = false;
    options.is_minimizable = true;
    options.is_movable = true;

    #[cfg(windows)]
    {
        options.titlebar = Some(TitlebarOptions {
            title: Some(t!("SkinPreview.title")),
            appears_transparent: false,
            ..Default::default()
        });
        options.window_background = WindowBackgroundAppearance::Opaque;
    }

    options
}

fn centered_status(colors: &ThemeColors, label: SharedString) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(colors.surface)
        .text_size(px(12.))
        .text_color(colors.text_secondary)
        .child(label)
        .into_any_element()
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paused_walk_time_stays_neutral_during_pointer_rerenders() {
        let started = Instant::now();
        let later = started + Duration::from_secs(20);
        assert_eq!(
            skin_preview_walk_time(false, started, later),
            Duration::ZERO,
        );
    }

    #[test]
    fn running_walk_time_uses_the_frame_clock() {
        let started = Instant::now();
        let now = started + Duration::from_millis(250);
        assert_eq!(
            skin_preview_walk_time(true, started, now),
            Duration::from_millis(250),
        );
    }

    #[test]
    fn wheel_zoom_tracks_fractional_scroll_delta() {
        let one_line = skin_preview_wheel_zoom_factor(16.0, 16.0);
        let quarter_line = skin_preview_wheel_zoom_factor(4.0, 16.0);
        let reverse_line = skin_preview_wheel_zoom_factor(-16.0, 16.0);

        assert!((one_line - SKIN_PREVIEW_WHEEL_ZOOM_PER_LINE).abs() < 1.0e-6);
        assert!(quarter_line > 1.0 && quarter_line < one_line);
        assert!((reverse_line - one_line.recip()).abs() < 1.0e-6);
    }

    #[test]
    fn wheel_zoom_clamps_single_event_spikes() {
        let clamped = skin_preview_wheel_zoom_factor(10_000.0, 16.0);
        let expected = SKIN_PREVIEW_WHEEL_ZOOM_PER_LINE.powf(SKIN_PREVIEW_MAX_WHEEL_LINES_PER_EVENT);
        assert!((clamped - expected).abs() < 1.0e-6);
    }
}
