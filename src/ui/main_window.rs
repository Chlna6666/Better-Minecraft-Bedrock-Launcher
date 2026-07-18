use crate::core::minecraft::remote_versions;
use crate::plugins::events::InjectionSlot;
use crate::ui::animation::request_animation_frame_if;
use crate::ui::components::color_picker::normalize_hex_color;
use crate::ui::components::icon::themed_icon;
use crate::ui::components::input::{InputEvent, InputState};
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::navigation::{AppRoute, RouteTarget};
use crate::ui::state::agreement::AgreementState;
use crate::ui::state::debug::DebugState;
use crate::ui::state::i18n::I18n;
use crate::ui::state::launcher::LauncherState;
use crate::ui::state::theme::ThemeState;
use crate::ui::state::update::UpdateState;
use crate::ui::theme::colors::{DarkColors, LightColors, lerp_theme_colors};
use crate::utils::updater::ReleaseSummary;
use gpui::*;
use std::any::type_name;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, trace, warn};

mod background;
mod background_support;
mod chrome;
mod chrome_view;
mod controls;
mod music_player;
mod page_loading;
mod page_registry;
mod route_effects;
mod support;
mod update_flow;

use background_support::*;
use support::*;

pub(crate) use background::startup_trace_elapsed_ms;
pub(crate) use chrome::AppChromeState;

pub(crate) const MAIN_WINDOW_INITIAL_SIZE: Size<Pixels> = size(px(972.), px(600.));

const STARTUP_ROUTE_BOOTSTRAP_DELAY: Duration = Duration::from_millis(120);
const STARTUP_UPDATE_CHECK_DELAY: Duration = Duration::from_millis(900);
const STARTUP_INTERACTION_WARMUP_DELAY: Duration = Duration::from_millis(1500);
const STARTUP_INTERACTION_WARMUP_STEP_DELAY: Duration = Duration::from_millis(80);

pub(crate) fn preload_startup_background_bytes_from_values(
    background_option: &str,
    local_image_path: &str,
    network_image_url: &str,
    cx: &mut App,
) -> usize {
    let source = resolve_background_source_from_values(
        background_option,
        local_image_path,
        network_image_url,
        0,
    );

    background_resource(&source)
        .map(|resource| cx.preload_compressed_image_resources([resource]).len())
        .unwrap_or(0)
}

pub(crate) fn preload_startup_background_target_from_values(
    background_option: &str,
    local_image_path: &str,
    network_image_url: &str,
    window: &Window,
    cx: &mut App,
) -> usize {
    let source = resolve_background_source_from_values(
        background_option,
        local_image_path,
        network_image_url,
        0,
    );

    background_resource(&source)
        .and_then(|resource| {
            cx.target_size_image_source(
                resource,
                window.viewport_size(),
                window.scale_factor(),
                ObjectFit::Cover,
            )
        })
        .map(|target| {
            let resource = target.resource().clone();
            let _task = cx.preload_target_size_image(target);
            cx.remove_compressed_image_resource(&resource);
            1
        })
        .unwrap_or(0)
}

fn optional_page_view_element<T>(route_key: &str, view: Option<Entity<T>>) -> AnyElement
where
    T: Render + 'static,
{
    view.map_or_else(
        || Empty {}.into_any_element(),
        |view| {
            AnyView::from(view)
                .cached_by(
                    StyleRefinement::default().size_full(),
                    &(route_key, type_name::<T>()),
                )
                .progressive()
                .into_any_element()
        },
    )
}

struct UpdateRenderState {
    suppress_background_animation_frames: bool,
}

struct AgreementRenderState {
    visible: bool,
    document: std::sync::Arc<crate::ui::components::markdown_renderer::MarkdownDocument>,
    scroll_handle: ScrollHandle,
    accept_unlocked: bool,
}

struct TopbarRenderState {
    theme_k: f32,
    theme_target_dark: bool,
    theme_animating: bool,
    theme_accent: Option<Hsla>,
    window_width: Pixels,
    music_snapshot: crate::ui::state::music::MusicSnapshot,
    music_expanded_factor: f32,
    music_progress_ratio: f32,
    music_volume_ratio: f32,
    music_drag_target: Option<crate::ui::state::music::MusicDragTarget>,
    music_popup_animating: bool,
    music_inline_factor: f32,
    music_inline_animating: bool,
    update_available: bool,
    visual_active_index: usize,
    pill_steps: f32,
    pill_direction: f32,
    pill_leading_progress: f32,
    pill_trailing_progress: f32,
    labels_layout_factor: f32,
    labels_opacity_factor: f32,
    nav_animating: bool,
    glass_effect_enabled: bool,
    plugin_navigation_pages: Vec<crate::plugins::runtime::PluginPage>,
}

struct MainWindowRenderModel {
    now: Instant,
    route: RouteTarget,
    builtin_route: AppRoute,
    theme_k: f32,
    theme_accent: Option<Hsla>,
    theme_colors: crate::ui::theme::colors::ThemeColors,
    debug_enabled: bool,
    window_width: Pixels,
    window_width_px: f32,
    window_height: Pixels,
    update_render_state: UpdateRenderState,
    agreement_render_state: AgreementRenderState,
    close_k: f32,
    quit_animating: bool,
    show_update_modal: bool,
    update_modal_visible: bool,
    update_modal_animating: bool,
    diagnostics_visible: bool,
    launcher_snapshot: crate::ui::hooks::use_launcher::LauncherSnapshot,
    launch_prereq_visible: bool,
    launch_prereq_busy_deadline: Option<Instant>,
    toast_visible: bool,
    toast_breadcrumb_visible: bool,
    dropdown_visible: bool,
}

#[derive(Clone, Copy)]
struct ThemeColorCache {
    factor: f32,
    accent: Option<Hsla>,
    colors: crate::ui::theme::colors::ThemeColors,
}

pub struct MainWindowView {
    background_view: Entity<background::AppBackgroundView>,
    chrome_view: Option<Entity<chrome_view::AppChromeView>>,
    music_library_load_started: bool,
    // 页面视图懒加载：首次进入路由时才创建，离开时可按需释放
    home_page_view: Option<Entity<crate::ui::views::home::HomePageView>>,
    download_page_view: Option<Entity<crate::ui::views::download::DownloadPageView>>,
    manage_page_view: Option<Entity<crate::ui::views::manage::ManagePageView>>,
    tools_page_view: Option<Entity<crate::ui::views::tools::ToolsPageView>>,
    settings_page_view: Option<Entity<crate::ui::views::settings::SettingsPageView>>,
    tasks_page_view: Option<Entity<crate::ui::views::tasks::TasksPageView>>,
    plugin_page_view: Option<Entity<crate::ui::views::plugin::PluginPageView>>,
    plugin_page_key: Option<(String, String)>,
    download_controls_initialized: bool,
    download_controls_subscriptions: Vec<Subscription>,
    download_overlay_active: bool,
    download_overlay_task_updates_task: Option<Task<()>>,
    download_prefs_last_save: Option<Instant>,
    download_curseforge_invalidate_seq_seen: u64,
    download_curseforge_invalidate_pending_seen: bool,
    manage_controls_initialized: bool,
    manage_controls_subscriptions: Vec<Subscription>,
    tools_controls_initialized: bool,
    tools_controls_subscriptions: Vec<Subscription>,
    settings_controls_initialized: bool,
    settings_controls_subscriptions: Vec<Subscription>,
    _route_subscriptions: Vec<Subscription>,
    _reactor_subscriptions: Vec<Subscription>,
    _window_subscriptions: Vec<Subscription>,
    update_download_listener_running: bool,
    update_markdown_view: Option<Entity<crate::ui::overlays::update::UpdateMarkdownView>>,
    settings_load_started: bool,
    background_animation_suppressed: bool,
    last_route_for_side_effects: Option<RouteTarget>,
    runtime_font_logged: bool,
    startup_route_bootstrapped: bool,
    startup_deferred_ready: bool,
    was_window_minimized: bool,
    theme_color_cache: Option<ThemeColorCache>,
}

impl MainWindowView {
    fn ensure_global_reactor_subscriptions(&mut self, cx: &mut Context<Self>) {
        if !self._reactor_subscriptions.is_empty() {
            return;
        }
    }

    fn ensure_music_library_load_started(&mut self, cx: &mut Context<Self>) {
        if self.music_library_load_started {
            return;
        }

        self.music_library_load_started = true;
        crate::ui::state::music::spawn_library_load(cx);
    }

    fn ensure_chrome_view_loaded(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chrome_view.is_some() {
            return;
        }

        self.chrome_view = Some(cx.new(|cx| chrome_view::AppChromeView::new(window, cx)));
    }

    fn ensure_startup_route_bootstrapped(&mut self, cx: &mut Context<Self>) {
        if self.startup_route_bootstrapped {
            return;
        }

        let initial_route = crate::ui::navigation::current_route_target(cx);
        self.handle_route_change_without_window(initial_route, cx);
        self.startup_route_bootstrapped = true;
    }

    fn is_window_minimized(window: &Window) -> bool {
        window.is_minimized()
    }

    fn maybe_trim_working_set_on_minimize(&mut self, window: &Window) {
        let is_minimized = Self::is_window_minimized(window);
        if is_minimized == self.was_window_minimized {
            return;
        }

        self.was_window_minimized = is_minimized;
        if !is_minimized {
            return;
        }

        info!("window_minimized: scheduling working set trim");
        crate::utils::memory::spawn_working_set_trim_task("window_minimized");
    }

    fn theme_colors_for_render(
        &mut self,
        now: Instant,
        cx: &App,
    ) -> (f32, Option<Hsla>, crate::ui::theme::colors::ThemeColors) {
        let (theme_k, theme_accent, theme_animating) = {
            let theme = cx.global::<ThemeState>();
            (theme.factor(now), theme.accent, theme.is_animating(now))
        };

        if !theme_animating
            && let Some(cache) = self.theme_color_cache
            && same_theme_factor(cache.factor, theme_k)
            && same_optional_hsla(cache.accent, theme_accent)
        {
            return (theme_k, theme_accent, cache.colors);
        }

        let theme_colors = lerp_theme_colors(
            &LightColors::colors(),
            &DarkColors::colors(),
            theme_k,
            theme_accent,
        );
        if !theme_animating {
            self.theme_color_cache = Some(ThemeColorCache {
                factor: theme_k,
                accent: theme_accent,
                colors: theme_colors,
            });
        }

        (theme_k, theme_accent, theme_colors)
    }

    fn build_render_model(
        &mut self,
        now: Instant,
        window: &Window,
        cx: &App,
    ) -> MainWindowRenderModel {
        let route = crate::ui::navigation::current_route_target(cx);
        let builtin_route = match &route {
            RouteTarget::Builtin(route) => *route,
            RouteTarget::Plugin { .. } => AppRoute::Home,
        };
        let debug_enabled = cx.global::<DebugState>().enabled;
        let update_render_state = self.read_update_render_state(now, debug_enabled, cx);
        let (theme_k, theme_accent, theme_colors) = self.theme_colors_for_render(now, cx);
        let window_bounds = window.bounds();
        let window_width = window_bounds.size.width;
        let window_height = window_bounds.size.height;
        let agreement_render_state = self.read_agreement_render_state(cx);
        let close_k = cx.global::<crate::ui::state::quit::QuitState>().factor(now);
        let quit_animating = cx
            .global::<crate::ui::state::quit::QuitState>()
            .is_animating(now);
        let (show_update_modal, update_modal_visible, update_modal_animating) = {
            let update_state: &UpdateState = cx.global::<UpdateState>();
            (
                update_state.should_render_modal(now),
                update_state.modal_visible,
                update_state.is_modal_render_animating(now),
            )
        };
        let diagnostics_visible = cx
            .global::<crate::ui::state::diagnostics::DiagnosticsState>()
            .pending_report
            .is_some();
        let launcher_snapshot = crate::ui::hooks::use_launcher::read_launcher_snapshot(now, cx);
        let (launch_prereq_visible, launch_prereq_busy_deadline) = {
            let launch_prereq_state =
                cx.global::<crate::ui::state::launch_prereq::LaunchPrereqState>();
            (
                launch_prereq_state.visible,
                launch_prereq_state.next_busy_animation_deadline(now),
            )
        };
        let (toast_visible, toast_breadcrumb_visible) = {
            let toast_state = cx.global::<crate::ui::components::toast::ToastState>();
            let window_id = window.window_handle().window_id();
            (
                crate::ui::components::toast::has_visible_toasts(window_id, now, toast_state),
                crate::ui::components::toast::has_visible_breadcrumb(window_id, now, toast_state),
            )
        };
        let dropdown_visible = {
            let dropdown_state =
                cx.global::<crate::ui::components::dropdown::DropdownOverlayState>();
            crate::ui::components::dropdown::has_visible_overlay(now, dropdown_state)
        };

        MainWindowRenderModel {
            now,
            route,
            builtin_route,
            theme_k,
            theme_accent,
            theme_colors,
            debug_enabled,
            window_width,
            window_width_px: window_width / px(1.0),
            window_height,
            update_render_state,
            agreement_render_state,
            close_k,
            quit_animating,
            show_update_modal,
            update_modal_visible,
            update_modal_animating,
            diagnostics_visible,
            launcher_snapshot,
            launch_prereq_visible,
            launch_prereq_busy_deadline,
            toast_visible,
            toast_breadcrumb_visible,
            dropdown_visible,
        }
    }

    fn render_active_page(&self, route: &RouteTarget) -> AnyElement {
        match route {
            RouteTarget::Builtin(AppRoute::Home) => {
                optional_page_view_element(AppRoute::Home.pathname(), self.home_page_view.clone())
            }
            RouteTarget::Builtin(AppRoute::Download) => optional_page_view_element(
                AppRoute::Download.pathname(),
                self.download_page_view.clone(),
            ),
            RouteTarget::Builtin(AppRoute::Manage) => optional_page_view_element(
                AppRoute::Manage.pathname(),
                self.manage_page_view.clone(),
            ),
            RouteTarget::Builtin(AppRoute::Tools) => {
                optional_page_view_element(AppRoute::Tools.pathname(), self.tools_page_view.clone())
            }
            RouteTarget::Builtin(AppRoute::Tasks) => {
                optional_page_view_element(AppRoute::Tasks.pathname(), self.tasks_page_view.clone())
            }
            RouteTarget::Builtin(AppRoute::Settings) => optional_page_view_element(
                AppRoute::Settings.pathname(),
                self.settings_page_view.clone(),
            ),
            RouteTarget::Plugin { .. } => {
                let route_key = route.pathname();
                optional_page_view_element(route_key.as_str(), self.plugin_page_view.clone())
            }
        }
    }

    fn compose_root(
        &mut self,
        model: &MainWindowRenderModel,
        page: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut root = div()
            .relative()
            .size_full()
            .bg(gpui::transparent_black())
            .child(
                AnyView::from(self.background_view.clone())
                    .cached_absolute_by(&"main-window-background")
                    .reuse_on_window_refresh()
                    .critical()
                    .into_any_element(),
            );

        let close_k = model.close_k.clamp(0.0, 1.0);
        if close_k > 0.0 {
            root = root
                .opacity(1.0 - close_k)
                .top(px(close_k.powf(1.25) * 10.0));
        }

        root = root.child(page);
        if let Some(chrome_view) = &self.chrome_view {
            root = root.child(
                AnyView::from(chrome_view.clone())
                    .cached_absolute_by(&"main-window-chrome")
                    .critical()
                    .into_any_element(),
            );
        }

        if model.builtin_route == AppRoute::Download
            && let Some(download_overlay) =
                crate::ui::views::download::render_download_overlay(&model.theme_colors, cx)
        {
            root = root.child(download_overlay);
        }

        if model.builtin_route == AppRoute::Settings
            && let Some(settings_overlay) = crate::ui::views::settings::render_settings_overlay(
                &model.theme_colors,
                model.window_width,
                model.window_height,
                model.theme_k,
                model.theme_accent,
                cx.global::<I18n>(),
                cx.global::<crate::ui::views::settings::state::SettingsPageState>(),
                model.agreement_render_state.document.clone(),
            )
        {
            root = root.child(settings_overlay);
        }

        if model.builtin_route == AppRoute::Tools
            && let Some(tools_overlay) = crate::ui::views::tools::render_tools_overlay(
                &model.theme_colors,
                model.window_width,
                model.window_height,
                cx.global::<crate::ui::views::tools::state::ToolsPageState>(),
            )
        {
            root = root.child(tools_overlay);
        }

        if model.builtin_route == AppRoute::Manage
            && let Some(manage_page_view) = &self.manage_page_view
            && let Some(manage_overlay) = crate::ui::views::manage::render_manage_overlay(
                &model.theme_colors,
                cx.global::<I18n>(),
                manage_page_view,
                cx,
            )
        {
            root = root.child(manage_overlay);
        }

        if model.builtin_route == AppRoute::Tasks
            && let Some(tasks_page_view) = &self.tasks_page_view
            && let Some(tasks_overlay) = crate::ui::views::tasks::render_tasks_overlay(
                &model.theme_colors,
                tasks_page_view,
                cx,
            )
        {
            root = root.child(tasks_overlay);
        }

        if model.agreement_render_state.visible {
            let agreement_title = cx.global::<I18n>().t("UserAgreement.title");
            let agreement_accept_label = cx.global::<I18n>().t("UserAgreement.accept_button");
            root = root.child(
                crate::ui::overlays::user_agreement::render_user_agreement_modal(
                    model.agreement_render_state.document.clone(),
                    model.window_width,
                    model.window_height,
                    model.theme_k,
                    model.theme_accent,
                    agreement_title,
                    agreement_accept_label,
                    model.agreement_render_state.scroll_handle.clone(),
                    model.agreement_render_state.accept_unlocked,
                    crate::ui::overlays::user_agreement::UserAgreementModalOptions::required_acceptance(),
                ),
            );
        }

        if !model.agreement_render_state.visible && model.diagnostics_visible {
            crate::ui::overlays::diagnostics::trigger_auto_sentry_submit_if_needed(cx);
        }

        if !model.agreement_render_state.visible
            && model.diagnostics_visible
            && let Some(diagnostics_overlay) =
                crate::ui::overlays::diagnostics::render_diagnostics_overlay(
                    &model.theme_colors,
                    model.window_width,
                    model.window_height,
                    cx.global::<I18n>(),
                    cx.global::<crate::ui::state::diagnostics::DiagnosticsState>(),
                )
        {
            root = root.child(diagnostics_overlay);
        }

        let should_render_update_modal = !model.agreement_render_state.visible
            && !model.diagnostics_visible
            && model.show_update_modal;
        if should_render_update_modal {
            let (
                release,
                downloading,
                task_id,
                snap,
                download_error,
                markdown_document,
                changelog_scroll_handle,
            ) = {
                let update_state: &UpdateState = cx.global::<UpdateState>();
                (
                    update_state.available.clone(),
                    update_state.downloading,
                    update_state.task_id.clone(),
                    update_state.last_task_snapshot.clone(),
                    update_state.download_error.clone(),
                    update_state.cached_md_document(),
                    update_state.changelog_scroll_handle.clone(),
                )
            };

            if let Some(release) = release {
                let update_modal_factor =
                    cx.global::<UpdateState>().modal_animation_factor(model.now);
                let markdown_view = if downloading {
                    None
                } else {
                    Some(self.sync_update_markdown_view(
                        &release,
                        markdown_document,
                        model.theme_colors,
                        model.theme_k > 0.5,
                        cx,
                    ))
                };
                if markdown_view.is_none() {
                    self.pause_update_markdown_view(cx);
                }

                root = root.child(crate::ui::overlays::update::render_update_modal(
                    release,
                    markdown_view,
                    changelog_scroll_handle,
                    model.window_width,
                    model.window_height,
                    model.update_modal_visible,
                    downloading,
                    task_id,
                    snap,
                    download_error,
                    model.theme_k,
                    update_modal_factor,
                    model.theme_accent,
                    cx.global::<I18n>(),
                ));
            } else {
                self.pause_update_markdown_view(cx);
            }
        } else {
            self.pause_update_markdown_view(cx);
        }

        if !model.agreement_render_state.visible {
            if model.launcher_snapshot.show_modal {
                root = root.child(crate::ui::overlays::launcher::render_launcher_overlay(
                    &model.launcher_snapshot,
                    window,
                    cx,
                ));
            }

            if model.launch_prereq_visible {
                if let Some(deadline) = model.launch_prereq_busy_deadline {
                    window.request_invalidation_at(deadline, cx);
                }

                let launch_prereq_state =
                    cx.global::<crate::ui::state::launch_prereq::LaunchPrereqState>();
                root = root.child(
                    crate::ui::overlays::launch_prereq::render_launch_prereq_overlay(
                        launch_prereq_state,
                        window,
                        cx,
                    ),
                );
            }
        }

        if !model.agreement_render_state.visible
            && let Some(modal) = crate::plugins::runtime::active_modal(cx)
        {
            root = root.child(self.render_plugin_modal(modal, model, window, cx));
        }

        if model.toast_visible {
            let toast_state = cx.global::<crate::ui::components::toast::ToastState>();
            root = root.child(crate::ui::components::toast::render_overlay(
                window,
                cx,
                &model.theme_colors,
                model.now,
                toast_state,
            ));
        }

        if model.dropdown_visible {
            let dropdown_state =
                cx.global::<crate::ui::components::dropdown::DropdownOverlayState>();
            root = root.child(crate::ui::components::dropdown::render_overlay(
                window,
                model.now,
                dropdown_state,
            ));
        }

        if model.toast_breadcrumb_visible {
            let toast_state = cx.global::<crate::ui::components::toast::ToastState>();
            root = root.child(crate::ui::components::toast::render_breadcrumb_overlay(
                window,
                cx,
                &model.theme_colors,
                model.now,
                toast_state,
            ));
        }

        let active_page_id = match &model.route {
            RouteTarget::Builtin(route) => route.pathname(),
            RouteTarget::Plugin { page_id, .. } => page_id.as_str(),
        };
        for injection in crate::plugins::runtime::render_injections(
            cx,
            InjectionSlot::MainRootOverlay,
            Some(active_page_id),
        ) {
            root = root.child(crate::plugins::ui_dsl::render_validated_view_tree(
                &injection.tree,
                &injection.plugin_id,
                None,
                window,
                cx,
            ));
        }
        root
    }

    fn render_plugin_modal(
        &mut self,
        modal: crate::plugins::runtime::PluginModalState,
        model: &MainWindowRenderModel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let width = px((modal.width as f32)
            .min(model.window_width_px - 32.0)
            .max(280.0));
        let height = px((modal.height as f32)
            .min(model.window_height / px(1.0) - 40.0)
            .max(240.0));
        let close = Rc::new(|cx: &mut App| {
            crate::plugins::runtime::close_modal(cx);
        });
        let page = match crate::plugins::runtime::render_page(cx, &modal.plugin_id, &modal.page_id)
        {
            Ok(tree) => crate::plugins::ui_dsl::render_validated_view_tree(
                &tree,
                &modal.plugin_id,
                Some(&modal.page_id),
                window,
                cx,
            ),
            Err(error) => {
                crate::plugins::ui_dsl::fallback_panel(error.to_string()).into_any_element()
            }
        };
        let title = modal.title.clone();
        let content = div()
            .flex()
            .flex_col()
            .w(width)
            .max_w(px(model.window_width_px - 32.0))
            .h(height)
            .max_h(px(model.window_height / px(1.0) - 40.0))
            .overflow_hidden()
            .rounded(px(10.0))
            .border_1()
            .border_color(Hsla {
                a: 0.56,
                ..model.theme_colors.border
            })
            .bg(Hsla {
                a: if model.theme_k > 0.5 { 0.90 } else { 0.84 },
                ..model.theme_colors.settings_card_bg
            })
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .px(px(16.0))
                    .py(px(10.0))
                    .border_b_1()
                    .border_color(Hsla {
                        a: 0.48,
                        ..model.theme_colors.border
                    })
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(model.theme_colors.text_primary)
                            .child(title),
                    )
                    .child(
                        div()
                            .w(px(30.0))
                            .h(px(30.0))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover({
                                let hover_color = model.theme_colors.surface_hover;
                                move |style| style.bg(hover_color)
                            })
                            .child(themed_icon(
                                lucide_gpui::icons::icon_x(),
                                15.0,
                                model.theme_colors.text_secondary,
                            ))
                            .on_mouse_up(MouseButton::Left, |_event, _window, cx| {
                                crate::plugins::runtime::close_modal(cx);
                            }),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_y_scrollbar()
                    .scrollbar_width(px(0.0))
                    .px(px(16.0))
                    .py(px(14.0))
                    .child(page),
            );

        crate::ui::components::modal::modal_layer_dismissible(
            content,
            Hsla {
                a: 0.28,
                ..gpui::black()
            },
            close,
        )
    }

    fn read_background_animation_suppressed(
        &self,
        actual_route: &RouteTarget,
        suppress_background_animation_frames: bool,
    ) -> bool {
        background_animation_suppressed(actual_route, suppress_background_animation_frames)
    }

    fn sync_background_animation_suppressed(
        &mut self,
        suppressed: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.background_animation_suppressed == suppressed {
            return false;
        }

        self.background_animation_suppressed = suppressed;
        let _ = self.background_view.update(cx, |view, cx| {
            if view.set_animation_suppressed(suppressed) {
                cx.notify();
            }
        });
        true
    }

    fn sync_background_animation_policy_for_route(
        &mut self,
        route: &RouteTarget,
        update_render_state: &UpdateRenderState,
        cx: &mut Context<Self>,
    ) -> bool {
        let should_suppress_background_animation = self.read_background_animation_suppressed(
            route,
            update_render_state.suppress_background_animation_frames,
        );
        self.sync_background_animation_suppressed(should_suppress_background_animation, cx)
    }

    fn sync_current_background_animation_policy(
        &mut self,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> bool {
        let route = crate::ui::navigation::current_route_target(cx);
        let debug_enabled = cx.global::<DebugState>().enabled;
        let update_render_state = self.read_update_render_state(now, debug_enabled, cx);
        self.sync_background_animation_policy_for_route(&route, &update_render_state, cx)
    }

    fn sync_update_markdown_view(
        &mut self,
        release: &ReleaseSummary,
        document: Arc<crate::ui::components::markdown_renderer::MarkdownDocument>,
        colors: crate::ui::theme::colors::ThemeColors,
        dark: bool,
        cx: &mut Context<Self>,
    ) -> Entity<crate::ui::overlays::update::UpdateMarkdownView> {
        if let Some(view) = &self.update_markdown_view {
            let matches = view
                .read(cx)
                .matches(&release.tag, &document, &colors, dark);
            if matches {
                view.update(cx, |view, cx| {
                    if view.set_active(true) {
                        cx.notify();
                    }
                });
                return view.clone();
            }

            view.update(cx, |view, cx| {
                view.update(release.tag.clone(), document.clone(), colors, dark);
                cx.notify();
            });
            return view.clone();
        }

        let view = cx.new(|_cx| {
            crate::ui::overlays::update::UpdateMarkdownView::new(
                release.tag.clone(),
                document,
                colors,
                dark,
            )
        });
        self.update_markdown_view = Some(view.clone());
        view
    }

    fn pause_update_markdown_view(&mut self, cx: &mut Context<Self>) {
        if let Some(view) = &self.update_markdown_view {
            view.update(cx, |view, _cx| {
                view.set_active(false);
            });
        }
    }

    fn read_agreement_render_state(&self, cx: &App) -> AgreementRenderState {
        let agreement: &AgreementState = cx.global::<AgreementState>();
        AgreementRenderState {
            visible: agreement.is_visible(),
            document: agreement.cached_document(),
            scroll_handle: agreement.agreement_scroll_handle.clone(),
            accept_unlocked: agreement.accept_unlocked,
        }
    }

    fn ensure_download_page_loaded(&mut self, force_refresh: bool, cx: &mut Context<Self>) {
        self.ensure_downloads_index(cx);

        let (loaded, loading) = {
            let s: &crate::ui::views::download::state::DownloadPageState =
                cx.global::<crate::ui::views::download::state::DownloadPageState>();
            (s.loaded, s.loading)
        };
        if loading || (loaded && !force_refresh) {
            return;
        }

        cx.update_global(
            |s: &mut crate::ui::views::download::state::DownloadPageState, _cx| {
                s.loading = true;
                s.error = None;
                if force_refresh {
                    s.loaded = false;
                    s.versions.clear();
                }
                s.force_refresh_next = false;
            },
        );
        self.notify_download_page(cx);

        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let result = remote_versions::load_or_fetch_versions(force_refresh).await;
            let _ = tx.send(result);
        });

        let download_page_view = self.download_page_view.as_ref().map(Entity::downgrade);
        cx.spawn(async move |_this, cx| {
            let result = match rx.await {
                Ok(v) => v,
                Err(_) => Err(anyhow::anyhow!("remote version loader task dropped")),
            };

            match result {
                Ok(remote) => {
                    let mut versions = Vec::with_capacity(remote.len());

                    for v in remote {
                        let package_id = SharedString::from(v.package_id.clone());
                        let version = SharedString::from(v.version.clone());
                        let build_type = SharedString::from(v.build_type.clone());
                        let md5 = v.md5.clone().map(SharedString::from);

                        versions.push(crate::ui::views::download::state::DownloadRemoteVersion {
                            version,
                            package_id,
                            version_type: v.version_type,
                            build_type,
                            archival_status: v.archival_status,
                            meta_present: v.meta_present,
                            md5,
                            is_gdk: v.is_gdk,
                        });
                    }

                    if let Err(err) = cx.update_global(
                        |s: &mut crate::ui::views::download::state::DownloadPageState, _cx| {
                            s.versions = versions;
                            s.local_path_by_package.clear();
                            s.loaded = true;
                            s.loading = false;
                            s.error = None;
                        },
                    ) {
                        tracing::warn!("update_global failed: {err:?}");
                    } else if let Some(view) = &download_page_view {
                        notify_weak_view_async(view, cx)?;
                    }
                }
                Err(e) => {
                    if let Err(err) = cx.update_global(
                        |s: &mut crate::ui::views::download::state::DownloadPageState, _cx| {
                            s.loading = false;
                            s.error = Some(SharedString::from(e.to_string()));
                        },
                    ) {
                        tracing::warn!("update_global failed: {err:?}");
                    } else if let Some(view) = &download_page_view {
                        notify_weak_view_async(view, cx)?;
                    }
                }
            }

            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn ensure_curseforge_loaded(&mut self, cx: &mut Context<Self>) {
        let (loaded, loading, has_error) = cx.read_global(
            |s: &crate::ui::views::download::state::DownloadPageState, _cx| {
                (
                    s.curseforge_loaded,
                    s.curseforge_loading,
                    s.curseforge_error.is_some(),
                )
            },
        );
        if loaded || loading {
            return;
        }
        // Avoid refetch loops when the API is down; user can press refresh to clear the error.
        if has_error {
            return;
        }

        cx.update_global(
            |s: &mut crate::ui::views::download::state::DownloadPageState, _cx| {
                s.curseforge_loading = true;
                s.curseforge_error = None;
            },
        );
        self.notify_download_page(cx);

        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let result = async {
                let client = crate::core::curseforge::CurseForgeClient::new()?;
                let (categories, versions) =
                    tokio::join!(client.get_categories(), client.get_minecraft_versions());
                Ok::<_, String>((categories?, versions?))
            }
            .await;
            let _ = tx.send(result);
        });

        let download_page_view = self.download_page_view.as_ref().map(Entity::downgrade);
        let curseforge_view_epoch = cx
            .global::<crate::ui::views::download::state::DownloadPageState>()
            .curseforge_view_epoch;
        cx.spawn(async move |_this, cx| {
            let result = rx
                .await
                .map_err(|_| "curseforge load task dropped".to_string());
            match result {
                Ok(Ok((categories, versions))) => {
                    let mut entries = categories
                        .into_iter()
                        .map(
                            |c| crate::ui::views::download::state::CurseForgeCategoryEntry {
                                id: c.id,
                                name: SharedString::from(c.name),
                                slug: SharedString::from(c.slug),
                                icon_url: c.icon_url.map(SharedString::from),
                                is_class: c.is_class.unwrap_or(false),
                                class_id: c.class_id,
                                parent_category_id: c.parent_category_id,
                            },
                        )
                        .collect::<Vec<_>>();
                    entries.sort_by_key(|entry| entry.id);

                    let version_entries = versions
                        .into_iter()
                        .map(SharedString::from)
                        .collect::<Vec<_>>();

                    match cx.update_global(
                        |s: &mut crate::ui::views::download::state::DownloadPageState, cx| {
                            if s.curseforge_view_epoch != curseforge_view_epoch {
                                return false;
                            }
                            s.curseforge_categories = entries;
                            s.curseforge_versions = version_entries;
                            s.curseforge_loaded = true;
                            s.curseforge_loading = false;
                            s.curseforge_error = None;
                            true
                        },
                    ) {
                        Ok(true) => {
                            if let Some(view) = &download_page_view {
                                notify_weak_view_async(view, cx)?;
                            }
                        }
                        Ok(false) => {}
                        Err(err) => tracing::warn!("update_global failed: {err:?}"),
                    }
                }
                Ok(Err(e)) | Err(e) => {
                    match cx.update_global(
                        |s: &mut crate::ui::views::download::state::DownloadPageState, _cx| {
                            if s.curseforge_view_epoch != curseforge_view_epoch {
                                return false;
                            }
                            s.curseforge_loading = false;
                            s.curseforge_error = Some(SharedString::from(e.to_string()));
                            true
                        },
                    ) {
                        Ok(true) => {
                            if let Some(view) = &download_page_view {
                                notify_weak_view_async(view, cx)?;
                            }
                        }
                        Ok(false) => {}
                        Err(err) => tracing::warn!("update_global failed: {err:?}"),
                    }
                }
            }

            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn ensure_curseforge_controls(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    fn ensure_curseforge_results_loaded(&mut self, force_refresh: bool, cx: &mut Context<Self>) {
        crate::ui::views::download::curseforge::ensure_results_loaded(force_refresh, cx);
    }

    fn ensure_downloads_index(&mut self, cx: &mut Context<Self>) {
        let (loaded, loading) = {
            let s: &crate::ui::views::download::state::DownloadPageState =
                cx.global::<crate::ui::views::download::state::DownloadPageState>();
            (s.downloads_index_loaded, s.downloads_index_loading)
        };
        if loaded || loading {
            return;
        }

        cx.update_global(
            |s: &mut crate::ui::views::download::state::DownloadPageState, _cx| {
                s.downloads_index_loading = true;
            },
        );
        self.notify_download_page(cx);

        let download_page_view = self.download_page_view.as_ref().map(Entity::downgrade);
        cx.spawn(async move |_this, cx| {
            let dir = crate::utils::file_ops::bmcbl_subdir("downloads");
            let set = tokio::task::spawn_blocking(move || {
                let mut out = std::collections::HashSet::new();
                if let Ok(rd) = std::fs::read_dir(dir) {
                    for entry in rd.flatten() {
                        if let Ok(ft) = entry.file_type() {
                            if !ft.is_file() {
                                continue;
                            }
                        }
                        if let Some(name) = entry.file_name().to_str() {
                            out.insert(SharedString::from(name.to_string()));
                        }
                    }
                }
                out
            })
            .await
            .unwrap_or_default();

            if let Err(err) = cx.update_global(
                |s: &mut crate::ui::views::download::state::DownloadPageState, _cx| {
                    s.local_files = set;
                    s.downloads_index_loaded = true;
                    s.downloads_index_loading = false;
                },
            ) {
                tracing::warn!("update_global failed: {err:?}");
            } else if let Some(view) = &download_page_view {
                notify_weak_view_async(view, cx)?;
            }

            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn ensure_download_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.download_controls_initialized {
            return;
        }

        let prefs = crate::core::ui_prefs::load_download_ui_prefs();
        let initial_page_size = prefs
            .as_ref()
            .map(|p| p.page_size)
            .unwrap_or(10)
            // Keep per-page rendering cheap while still allowing small user tweaks.
            .clamp(8, 12);
        let initial_filter = prefs
            .as_ref()
            .map(|p| p.channel_filter.as_str())
            .unwrap_or("all");
        let initial_filter = match initial_filter {
            "release" => crate::ui::views::download::state::DownloadChannelFilter::Release,
            "beta" => crate::ui::views::download::state::DownloadChannelFilter::Beta,
            "preview" => crate::ui::views::download::state::DownloadChannelFilter::Preview,
            _ => crate::ui::views::download::state::DownloadChannelFilter::All,
        };

        // Create search input and channel select once and keep subscriptions alive in MainWindowView.
        let search_input = cx.update_global(
            |s: &mut crate::ui::views::download::state::DownloadPageState, cx| {
                s.page_size = initial_page_size;
                s.channel_filter = initial_filter;
                s.search_query = SharedString::from("");
                s.page_index = 0;

                if s.search_input.is_none() {
                    let input = cx.new(|cx| {
                        let mut st = InputState::new(window, cx);
                        st.set_placeholder(SharedString::from("搜索游戏版本..."), window, cx);
                        st
                    });
                    s.search_input = Some(input);
                }

                if s.page_jump_input.is_none() {
                    let input = cx.new(|cx| {
                        let mut st = InputState::new(window, cx);
                        st.set_placeholder(SharedString::from("1/1"), window, cx);
                        st
                    });
                    s.page_jump_input = Some(input);
                }

                s.search_input.clone()
            },
        );
        let (initial_invalidate_seq, initial_invalidate_pending) = cx.read_global(
            |s: &crate::ui::views::download::state::DownloadPageState, _cx| {
                (
                    s.curseforge_invalidate_seq,
                    s.curseforge_invalidate_task.is_some(),
                )
            },
        );
        self.download_curseforge_invalidate_seq_seen = initial_invalidate_seq;
        self.download_curseforge_invalidate_pending_seen = initial_invalidate_pending;

        let sub = cx.observe_global::<crate::ui::views::download::state::DownloadPageState>(
            |this, cx| {
                let route = crate::ui::navigation::current_route(cx);
                let (
                    tab,
                    invalidate_seq,
                    invalidate_pending,
                    curseforge_loaded,
                    curseforge_loading,
                ) = cx.read_global(
                    |s: &crate::ui::views::download::state::DownloadPageState, _cx| {
                        (
                            s.tab,
                            s.curseforge_invalidate_seq,
                            s.curseforge_invalidate_task.is_some(),
                            s.curseforge_loaded,
                            s.curseforge_loading,
                        )
                    },
                );

                if route != AppRoute::Download
                    || tab != crate::ui::views::download::state::DownloadTab::ResourcePack
                {
                    return;
                }

                let invalidate_changed =
                    this.download_curseforge_invalidate_seq_seen != invalidate_seq;
                let pending_changed =
                    this.download_curseforge_invalidate_pending_seen != invalidate_pending;
                if !invalidate_changed && !pending_changed {
                    return;
                }

                this.download_curseforge_invalidate_seq_seen = invalidate_seq;
                this.download_curseforge_invalidate_pending_seen = invalidate_pending;

                if invalidate_pending {
                    return;
                }

                if !curseforge_loaded && !curseforge_loading {
                    this.ensure_curseforge_loaded(cx);
                }
                this.ensure_curseforge_results_loaded(false, cx);
                this.notify_download_page(cx);
            },
        );
        self.download_controls_subscriptions.push(sub);

        let refresh_sub = cx
            .observe_global::<crate::ui::views::download::state::DownloadPageState>(|this, cx| {
                let route = crate::ui::navigation::current_route(cx);
                if route != AppRoute::Download {
                    return;
                }

                let (tab, force_refresh_next, loading) = cx.read_global(
                    |s: &crate::ui::views::download::state::DownloadPageState, _cx| {
                        (s.tab, s.force_refresh_next, s.loading)
                    },
                );

                if tab != crate::ui::views::download::state::DownloadTab::Game
                    || !force_refresh_next
                    || loading
                {
                    return;
                }

                this.ensure_download_page_loaded(true, cx);
                this.notify_download_page(cx);
            });
        self.download_controls_subscriptions.push(refresh_sub);

        let overlay_sub = cx
            .observe_global::<crate::ui::views::download::state::DownloadPageState>(|this, cx| {
                let route = crate::ui::navigation::current_route(cx);
                if route != AppRoute::Download {
                    this.download_overlay_active = false;
                    return;
                }

                let active = cx.read_global(
                    |s: &crate::ui::views::download::state::DownloadPageState, _cx| {
                        s.game_dialog.is_some()
                            || (matches!(
                                s.tab,
                                crate::ui::views::download::state::DownloadTab::ResourcePack
                            ) && s.curseforge_install_open)
                    },
                );
                let should_notify = this.download_overlay_active || active;
                this.download_overlay_active = active;

                if should_notify {
                    cx.notify();
                }
            });
        self.download_controls_subscriptions.push(overlay_sub);

        let mut task_updates = crate::tasks::task_manager::subscribe_task_updates();
        self.download_overlay_task_updates_task = Some(cx.spawn(async move |handle, cx| {
            loop {
                match task_updates.recv().await {
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }

                if handle
                    .update(cx, |this, cx| {
                        if this.download_overlay_active {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
        }));

        if let Some(input) = search_input {
            let sub = cx.subscribe(&input, |this, input, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    let query = input.read(cx).value();
                    cx.update_global(|s: &mut crate::ui::views::download::state::DownloadPageState, cx| {
                        if matches!(s.tab, crate::ui::views::download::state::DownloadTab::ResourcePack) {
                            s.curseforge_search_commit_task.take();
                            s.curseforge_search_commit_seq =
                                s.curseforge_search_commit_seq.wrapping_add(1);
                            let seq = s.curseforge_search_commit_seq;
                            let query = query.clone();
                            let task = cx.spawn(async move |_this, cx| {
                                Timer::after(Duration::from_millis(180)).await;
                                let _ = cx.update_global(
                                    |s: &mut crate::ui::views::download::state::DownloadPageState, cx| {
                                        if s.curseforge_search_commit_seq != seq
                                            || !matches!(
                                                s.tab,
                                                crate::ui::views::download::state::DownloadTab::ResourcePack
                                            )
                                        {
                                            return;
                                        }
                                        s.curseforge_search_commit_task = None;
                                        if s.search_query.as_ref() == query.as_ref() {
                                            return;
                                        }
                                        s.search_query = query;
                                        s.page_index = 0;
                                        s.curseforge_page_index = 0;
                                        crate::ui::views::download::curseforge::
                                            schedule_invalidate_results_in_state(s, cx);
                                    },
                                );
                            });
                            s.curseforge_search_commit_task = Some(task);
                            return;
                        }
                        if s.search_query.as_ref() == query.as_ref() {
                            return;
                        }
                        s.search_query = query;
                        s.page_index = 0;
                        s.curseforge_page_index = 0;
                    });
                    this.notify_download_page(cx);
                    this.save_download_ui_prefs_throttled(cx);
                }
            });
            self.download_controls_subscriptions.push(sub);
        }

        self.download_controls_initialized = true;
    }

    fn save_download_ui_prefs_throttled(&mut self, cx: &mut Context<Self>) {
        const MIN_INTERVAL_MS: u64 = 900;
        let now = Instant::now();
        if let Some(last) = self.download_prefs_last_save {
            if now.duration_since(last).as_millis() as u64 <= MIN_INTERVAL_MS {
                return;
            }
        }

        let (search_query, channel_filter, page_size) = cx.read_global(
            |s: &crate::ui::views::download::state::DownloadPageState, _cx| {
                (s.search_query.to_string(), s.channel_filter, s.page_size)
            },
        );

        let filter_str = match channel_filter {
            crate::ui::views::download::state::DownloadChannelFilter::All => "all",
            crate::ui::views::download::state::DownloadChannelFilter::Release => "release",
            crate::ui::views::download::state::DownloadChannelFilter::Beta => "beta",
            crate::ui::views::download::state::DownloadChannelFilter::Preview => "preview",
        }
        .to_string();

        let prefs = crate::core::ui_prefs::DownloadUiPrefs {
            search_query,
            channel_filter: filter_str,
            page_size,
        };

        self.download_prefs_last_save = Some(now);
        cx.spawn(async move |_this, cx| {
            let result = cx
                .background_spawn(
                    async move { crate::core::ui_prefs::save_download_ui_prefs(&prefs) },
                )
                .await;

            match result {
                Ok(()) => {}
                Err(error) => {
                    tracing::warn!("save download ui prefs failed: {error}");
                }
            }
        })
        .detach();
    }
}

fn background_animation_suppressed(
    _actual_route: &RouteTarget,
    suppress_background_animation_frames: bool,
) -> bool {
    suppress_background_animation_frames
}

fn same_theme_factor(left: f32, right: f32) -> bool {
    (left - right).abs() <= f32::EPSILON
}

fn same_optional_hsla(left: Option<Hsla>, right: Option<Hsla>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => same_hsla(left, right),
        (None, None) => true,
        _ => false,
    }
}

fn same_hsla(left: Hsla, right: Hsla) -> bool {
    same_theme_factor(left.h, right.h)
        && same_theme_factor(left.s, right.s)
        && same_theme_factor(left.l, right.l)
        && same_theme_factor(left.a, right.a)
}

impl Render for MainWindowView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let render_started = Instant::now();
        self.ensure_global_reactor_subscriptions(cx);
        if !self.runtime_font_logged {
            self.runtime_font_logged = true;
            info!(
                "runtime_text_style: window_font_family={}",
                window.text_style().font_family
            );
        }
        let update_state_changed = self.sync_update_state(render_started, cx);
        let model = self.build_render_model(render_started, window, cx);
        let _ = self.sync_background_animation_policy_for_route(
            &model.route,
            &model.update_render_state,
            cx,
        );
        crate::ui::hooks::use_launcher::sync_launcher_state(render_started, cx);
        self.ensure_chrome_view_loaded(window, cx);
        if self.startup_deferred_ready {
            self.ensure_startup_route_bootstrapped(cx);
            self.ensure_route_controls(model.builtin_route, window, cx);
            self.ensure_music_library_load_started(cx);
        }
        request_animation_frame_if(window, model.quit_animating);
        request_animation_frame_if(window, model.update_modal_animating);
        {
            let launcher_state = cx.global::<LauncherState>();
            request_animation_frame_if(window, launcher_state.is_modal_animating(render_started));
        }
        if update_state_changed {
            cx.notify();
        }

        if model.debug_enabled {
            crate::ui::window::debug::state::record_main_window_frame(
                model.now,
                model.window_width_px,
                model.window_height / px(1.),
            );
        }

        let page = self.render_active_page(&model.route);
        let root = self.compose_root(&model, page, window, cx);

        let render_elapsed = render_started.elapsed();
        if render_elapsed >= Duration::from_millis(16) {
            warn!(
                route = ?model.route,
                elapsed_ms = render_elapsed.as_millis(),
                width = model.window_width_px,
                height = model.window_height / px(1.),
                quit_animating = model.quit_animating,
                update_modal_animating = model.update_modal_animating,
                update_state_changed,
                "main window render slow"
            );
        } else {
            trace!(
                route = ?model.route,
                elapsed_ms = render_elapsed.as_millis(),
                width = model.window_width_px,
                height = model.window_height / px(1.),
                quit_animating = model.quit_animating,
                update_modal_animating = model.update_modal_animating,
                update_state_changed,
                "main window render"
            );
        }

        if model.debug_enabled {
            crate::ui::window::debug::state::record_main_window_render_finished(
                render_started.elapsed(),
            );
        }

        root
    }
}

#[cfg(test)]
mod tests {
    use super::background_animation_suppressed;
    use crate::ui::navigation::{AppRoute, RouteTarget};

    #[test]
    fn non_home_route_does_not_suppress_background_animation() {
        let route = RouteTarget::Builtin(AppRoute::Download);

        assert!(!background_animation_suppressed(&route, false));
    }

    #[test]
    fn modal_suppression_still_pauses_background_animation() {
        let route = RouteTarget::Builtin(AppRoute::Home);

        assert!(background_animation_suppressed(&route, true));
    }
}
