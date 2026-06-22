use crate::ui::animation::request_animation_frame_if;
use crate::ui::components::modal;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};
use crate::ui::views::download::state::{DownloadPageState, DownloadTab};
use gpui::*;
use std::rc::Rc;
use std::time::Instant;

mod common;
pub(crate) mod curseforge;
mod game;
mod mods;
pub mod state;
mod toolbar;

pub(crate) fn is_entity_released_error(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string().contains("entity released"))
}

actions!(download_page, [PasteShare, CloseOverlay]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-v", PasteShare, Some("Download")),
        KeyBinding::new("escape", CloseOverlay, Some("Download")),
    ]);
}

#[derive(Clone, PartialEq, Eq)]
struct GamePanelObserveSignature {
    loading: bool,
    force_refresh_next: bool,
    error: SharedString,
    version_count: usize,
    first_package_id: SharedString,
    last_package_id: SharedString,
    search_query: SharedString,
    channel_filter: state::DownloadChannelFilter,
    page_index: usize,
    page_size: usize,
    local_file_count: usize,
    local_path_count: usize,
    operation_count: usize,
}

fn build_game_panel_observe_signature(state: &DownloadPageState) -> GamePanelObserveSignature {
    GamePanelObserveSignature {
        loading: state.loading,
        force_refresh_next: state.force_refresh_next,
        error: state
            .error
            .clone()
            .unwrap_or_else(|| SharedString::from("")),
        version_count: state.versions.len(),
        first_package_id: state
            .versions
            .first()
            .map(|version| version.package_id.clone())
            .unwrap_or_else(|| SharedString::from("")),
        last_package_id: state
            .versions
            .last()
            .map(|version| version.package_id.clone())
            .unwrap_or_else(|| SharedString::from("")),
        search_query: state.search_query.clone(),
        channel_filter: state.channel_filter,
        page_index: state.page_index,
        page_size: state.page_size,
        local_file_count: state.local_files.len(),
        local_path_count: state.local_path_by_package.len(),
        operation_count: state.operations_by_package.len(),
    }
}

pub struct DownloadPageView {
    _subscriptions: Vec<Subscription>,
    curseforge_resource_panel: Entity<curseforge::CurseForgeResourcePanelView>,
    game_panel_view: Option<Entity<game::DownloadGamePanelView>>,
    last_observed_tab: DownloadTab,
    active: bool,
    last_observed_curseforge_toolbar_signature: (
        bool,
        bool,
        usize,
        SharedString,
        i32,
        bool,
        bool,
        bool,
        bool,
        bool,
    ),
}

impl DownloadPageView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (last_observed_tab, last_observed_curseforge_toolbar_signature) =
            cx.read_global(|state: &DownloadPageState, _cx| {
                (
                    state.tab,
                    (
                        state.curseforge_loaded,
                        state.curseforge_loading,
                        state.curseforge_versions.len(),
                        state.curseforge_selected_game_version.clone(),
                        state.curseforge_sort_field,
                        state.curseforge_invalidate_task.is_some(),
                        state.curseforge_search_commit_task.is_some(),
                        state.curseforge_results_loading,
                        state.curseforge_page_commit_task.is_some(),
                        state.curseforge_pending_page_index.is_some(),
                    ),
                )
            });

        let mut subscriptions = Vec::new();
        subscriptions.push(cx.observe_global::<DownloadPageState>(|this, cx| {
            let (tab, curseforge_toolbar_signature) =
                cx.read_global(|state: &DownloadPageState, _cx| {
                    (
                        state.tab,
                        (
                            state.curseforge_loaded,
                            state.curseforge_loading,
                            state.curseforge_versions.len(),
                            state.curseforge_selected_game_version.clone(),
                            state.curseforge_sort_field,
                            state.curseforge_invalidate_task.is_some(),
                            state.curseforge_search_commit_task.is_some(),
                            state.curseforge_results_loading,
                            state.curseforge_page_commit_task.is_some(),
                            state.curseforge_pending_page_index.is_some(),
                        ),
                    )
                });

            let tab_changed = this.last_observed_tab != tab;
            let curseforge_toolbar_changed =
                this.last_observed_curseforge_toolbar_signature != curseforge_toolbar_signature;

            this.last_observed_tab = tab;
            this.last_observed_curseforge_toolbar_signature = curseforge_toolbar_signature;

            if tab_changed {
                cx.notify();
                return;
            }

            if tab == DownloadTab::Mod {
                cx.notify();
                return;
            }

            if tab == DownloadTab::ResourcePack && curseforge_toolbar_changed {
                cx.notify();
            }
        }));
        subscriptions.push(cx.observe_global::<ThemeState>(|_, cx| {
            cx.notify();
        }));

        let game_panel_view = if last_observed_tab == DownloadTab::Game {
            Some(cx.new(game::DownloadGamePanelView::new))
        } else {
            None
        };

        Self {
            _subscriptions: subscriptions,
            curseforge_resource_panel: cx.new(curseforge::CurseForgeResourcePanelView::new),
            game_panel_view,
            last_observed_tab,
            active: true,
            last_observed_curseforge_toolbar_signature,
        }
    }

    fn ensure_game_panel_view(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Entity<game::DownloadGamePanelView> {
        if let Some(view) = &self.game_panel_view {
            return view.clone();
        }

        let view = cx.new(game::DownloadGamePanelView::new);
        self.game_panel_view = Some(view.clone());
        view
    }

    pub(crate) fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }

        self.active = active;
        let curseforge_resource_panel = self.curseforge_resource_panel.clone();
        let _ = curseforge_resource_panel.update(cx, |view, cx| {
            view.set_active(active, cx);
            Ok::<(), anyhow::Error>(())
        });
    }
}

impl Render for DownloadPageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let tab_animating =
            cx.read_global(|state: &DownloadPageState, _cx| state.tab_anim_factor(now).1);
        request_animation_frame_if(window, tab_animating);
        let theme = cx.global::<ThemeState>();
        let colors = lerp_theme_colors(
            &LightColors::colors(),
            &DarkColors::colors(),
            theme.factor(now),
            theme.accent,
        );
        let window_size = window.window_bounds().get_bounds().size;
        let active_tab = cx.read_global(|state: &DownloadPageState, _cx| state.tab);
        let game_panel_view = if active_tab == DownloadTab::Game {
            Some(self.ensure_game_panel_view(cx))
        } else {
            self.game_panel_view.clone()
        };

        div()
            .size_full()
            .key_context("Download")
            .on_action(|_: &PasteShare, _window, cx| {
                if cx.global::<DownloadPageState>().tab == DownloadTab::ResourcePack {
                    curseforge::handle_clipboard_share_paste(cx);
                }
            })
            .on_action(|_: &CloseOverlay, _window, cx| {
                if cx.global::<DownloadPageState>().tab == DownloadTab::ResourcePack {
                    curseforge::handle_close_overlay(cx);
                }
            })
            .child(render_download_page(
                window,
                cx,
                colors,
                window_size.width,
                window_size.height,
                now,
                &self.curseforge_resource_panel,
                game_panel_view.as_ref(),
            ))
    }
}

pub fn render_download_page(
    window: &mut Window,
    cx: &mut Context<DownloadPageView>,
    colors: ThemeColors,
    _window_width: Pixels,
    _window_height: Pixels,
    now: Instant,
    curseforge_resource_panel: &Entity<curseforge::CurseForgeResourcePanelView>,
    game_panel_view: Option<&Entity<game::DownloadGamePanelView>>,
) -> impl IntoElement {
    let (active_tab, tab_t, tab_animating) = cx.read_global(|state: &DownloadPageState, _cx| {
        let (tab_t, tab_animating) = state.tab_anim_factor(now);
        (state.tab, tab_t, tab_animating)
    });
    let tab_t = (1.0 - (1.0 - tab_t).powi(3)).clamp(0.0, 1.0);
    let content_opacity = if tab_animating {
        0.92 + 0.08 * tab_t
    } else {
        1.0
    };

    // Mirror `.upstream_bmbl_1/src/components/UnifiedPageLayout/*`:
    // one glass panel with a fixed header, a scrollable content area, and a footer.
    let header = div()
        .flex()
        .flex_col()
        .child(
            toolbar::render_toolbar(&colors, cx.global::<DownloadPageState>(), now)
                .rounded(px(0.))
                .border_0(),
        )
        .child(div().h(px(1.)).bg(Hsla {
            a: 0.06,
            ..colors.border
        }));

    let body: AnyElement = match active_tab {
        DownloadTab::Game => game_panel_view
            .cloned()
            .map(IntoElement::into_any_element)
            .unwrap_or_else(|| div().size_full().into_any_element()),
        DownloadTab::ResourcePack => curseforge_resource_panel.clone().into_any_element(),
        DownloadTab::Mod => {
            mods::render_mod_panel(&colors, cx.global::<DownloadPageState>()).into_any_element()
        }
    };

    let unified_panel = div()
        .size_full()
        .rounded(px(20.))
        .border_1()
        .border_color(Hsla {
            a: 0.40,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.65,
            ..colors.surface
        })
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(header)
        .child(
            div()
                .flex_1()
                .min_h(px(0.))
                .min_w(px(0.))
                .opacity(content_opacity)
                .flex()
                .flex_col()
                .child(body),
        );

    let page = common::page_shell(unified_panel, &colors);
    div().size_full().child(page)
}

pub fn dismiss_game_dialog(cx: &mut App) {
    cx.update_global(|state: &mut DownloadPageState, _cx| {
        state.game_dialog = None;
        state.game_dialog_input = None;
        state.game_dialog_folder_input = None;
        state.game_dialog_cdn_loading = false;
        state.game_dialog_cdn_error = None;
        state.game_dialog_cdn_results.clear();
        state.game_dialog_selected_cdn_base = None;
    });
}

pub fn render_download_overlay(colors: &ThemeColors, cx: &App) -> Option<AnyElement> {
    let (dialog, dialog_folder_input, cdn_loading, cdn_error, cdn_results, selected_cdn_base) = cx
        .read_global(|state: &DownloadPageState, _cx| {
            (
                state.game_dialog.clone(),
                state.game_dialog_folder_input.clone(),
                state.game_dialog_cdn_loading,
                state.game_dialog_cdn_error.clone(),
                state.game_dialog_cdn_results.clone(),
                state.game_dialog_selected_cdn_base.clone(),
            )
        });

    dialog.map(|dialog| {
        modal::modal_layer_dismissible(
            game::render_game_dialog(
                colors,
                dialog,
                dialog_folder_input.as_ref(),
                cdn_loading,
                cdn_error,
                cdn_results,
                selected_cdn_base,
            ),
            hsla(0.0, 0.0, 0.0, 0.28),
            Rc::new(dismiss_game_dialog),
        )
        .into_any_element()
    })
}
