use super::mesh::{SkinPreviewMeshes, skin_player_mesh, skin_preview_paint_meshes};
use super::selector::{
    render_current_preview, render_skin_selector, skin_selector_page_count,
    skin_selector_page_for_index,
};
use crate::ui::animation::request_animation_frame_if;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

const SKIN_PREVIEW_WINDOW_WIDTH: f32 = 640.0;
const SKIN_PREVIEW_WINDOW_HEIGHT: f32 = 600.0;
const SKIN_PREVIEW_STAGE_MAX_WIDTH: f32 = 608.0;
const SKIN_PREVIEW_STAGE_MAX_HEIGHT: f32 = 360.0;

#[derive(Clone)]
pub struct SkinPreviewWindowSkin {
    pub display_name: SharedString,
    pub texture_path: SharedString,
    pub model_label: Option<SharedString>,
    pub preview_path: Option<SharedString>,
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
    drag_position: Option<Point<Pixels>>,
    selector_expanded: bool,
    selector_page: usize,
    _subscriptions: Vec<Subscription>,
}

impl SkinPreviewWindowView {
    fn new(init: SkinPreviewWindowInit, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe_global::<ThemeState>(|_, cx| {
            cx.notify();
        })];
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
            view_yaw: 0.42,
            view_pitch: -0.18,
            drag_position: None,
            selector_expanded: false,
            selector_page,
            _subscriptions: subscriptions,
        };
        this.load_mesh(cx);
        this
    }

    fn load_mesh(&mut self, cx: &mut Context<Self>) {
        let Some(skin) = self.current_skin().cloned() else {
            self.mesh = Some(Err(SharedString::from("这个皮肤包没有可预览的皮肤贴图")));
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
        cx.notify();
        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(
                    async move { skin_player_mesh(Path::new(&texture_path), slim_arms) },
                )
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

    fn theme_colors(&self, cx: &App) -> ThemeColors {
        let theme = cx.global::<ThemeState>();
        lerp_theme_colors(
            &LightColors::colors(),
            &DarkColors::colors(),
            theme.factor(Instant::now()),
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

    fn walk_phase(&self, now: Instant) -> f32 {
        if self.walking {
            now.saturating_duration_since(self.walk_started_at)
                .as_secs_f32()
                * 5.2
        } else {
            0.0
        }
    }

    fn toggle_walking(&mut self, cx: &mut Context<Self>) {
        self.walking = !self.walking;
        self.walk_started_at = Instant::now();
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
    ) -> Stateful<Div> {
        div()
            .id(id)
            .w(px(34.))
            .h(px(34.))
            .rounded(px(8.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .child(
                svg()
                    .path(icon)
                    .w(px(15.))
                    .h(px(15.))
                    .text_color(colors.text_secondary),
            )
    }

    fn render_canvas(
        &self,
        colors: &ThemeColors,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match &self.mesh {
            Some(Ok(mesh)) => {
                let mesh = mesh.clone();
                let view_yaw = self.view_yaw;
                let view_pitch = self.view_pitch;
                let walk_phase = self.walk_phase(now);
                let walking = self.walking;
                div()
                    .relative()
                    .size_full()
                    .overflow_hidden()
                    .bg(colors.surface)
                    .cursor_pointer()
                    .child(
                        canvas(
                            move |bounds, _window, _cx| bounds,
                            move |bounds, _prepaint, window, _cx| {
                                let height = f32::from(bounds.size.height).max(1.0);
                                let aspect = f32::from(bounds.size.width).max(1.0) / height;
                                for paint_mesh in skin_preview_paint_meshes(
                                    &mesh, aspect, view_yaw, view_pitch, walk_phase, walking,
                                ) {
                                    window.paint_gpu_mesh_3d(
                                        bounds,
                                        paint_mesh.mesh,
                                        paint_mesh.parameters,
                                    );
                                }
                            },
                        )
                        .absolute()
                        .inset_0(),
                    )
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
                    .into_any_element()
            }
            Some(Err(error)) => centered_status(colors, error.clone()),
            None => centered_status(colors, SharedString::from("正在生成 3D 预览...")),
        }
    }
}

impl Render for SkinPreviewWindowView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        request_animation_frame_if(
            window,
            self.walking && self.mesh.as_ref().is_some_and(Result::is_ok),
        );
        let colors = self.theme_colors(cx);
        let model_label = self.current_model_label();
        let skin_label = self.current_skin_label();
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
                                            .child(format!(
                                                "3D 皮肤预览 · {} · {skin_counter} · {}",
                                                skin_label.as_ref(),
                                                model_label.as_ref()
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
                                        lucide_gpui::icons::icon_chevron_left(),
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
                                        lucide_gpui::icons::icon_chevron_right(),
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
                                    "skin-preview-toggle-motion",
                                    if self.walking {
                                        lucide_gpui::icons::icon_pause()
                                    } else {
                                        lucide_gpui::icons::icon_play()
                                    },
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.toggle_walking(cx);
                                    }),
                                ),
                            )
                            .child(
                                self.render_button(
                                    &colors,
                                    "skin-preview-close",
                                    lucide_gpui::icons::icon_x(),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    |_event, window, _cx| {
                                        window.remove_window();
                                    },
                                ),
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
                            .rounded(px(10.))
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
    let title = format!("皮肤 3D 预览 - {}", init.title);
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
            title: Some(SharedString::from("皮肤 3D 预览")),
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
