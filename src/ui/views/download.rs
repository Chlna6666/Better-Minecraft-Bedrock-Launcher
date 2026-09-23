use crate::ui::animation::{tab_content_animation_key, tab_content_motion};
use crate::ui::components::modal;
use crate::ui::components::toast;
use crate::ui::state::i18n::I18n;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};
use crate::ui::views::download::state::{DownloadPageState, DownloadTab};
use gpui::*;
use std::rc::Rc;
use std::sync::Arc;
use tracing::warn;

mod common;
pub(crate) mod curseforge;
mod game;
mod loading;
mod mod_install;
mod mods;
mod native;
pub mod state;
mod toolbar;
mod version_import;

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
    loader_filter: state::DownloadLoaderFilter,
    page_index: usize,
    page_size: usize,
    local_file_count: usize,
    local_path_count: usize,
    operation_count: usize,
    task_snapshot_count: usize,
    task_snapshot_sequence: u64,
    levilamina_support_loaded: bool,
    levilamina_supported_game_count: usize,
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
        loader_filter: state.loader_filter,
        page_index: state.page_index,
        page_size: state.page_size,
        local_file_count: state.local_files.len(),
        local_path_count: state.local_path_by_package.len(),
        operation_count: state.operations_by_package.len(),
        task_snapshot_count: state.task_snapshots.len(),
        task_snapshot_sequence: state
            .task_snapshots
            .values()
            .map(|snapshot| snapshot.sequence)
            .max()
            .unwrap_or_default(),
        levilamina_support_loaded: state.levilamina_support_loaded,
        levilamina_supported_game_count: state.levilamina_support.versions.len(),
    }
}

type ModPanelObserveSignature = (
    bool,
    bool,
    bool,
    usize,
    SharedString,
    SharedString,
    SharedString,
    usize,
    bool,
    SharedString,
    (
        Option<String>,
        (SharedString, bool, bool, usize, SharedString),
        (
            bool,
            bool,
            bool,
            usize,
            bool,
            SharedString,
            SharedString,
            bool,
            SharedString,
        ),
    ),
);

fn build_mod_panel_observe_signature(state: &DownloadPageState) -> ModPanelObserveSignature {
    (
        state.levilauncher_loaded,
        state.levilauncher_loading,
        state.levilauncher_error.is_some(),
        state.levilauncher_all_mods.len(),
        state.search_query.clone(),
        state.levilauncher_selected_loader.clone(),
        state.levilauncher_selected_loader_version.clone(),
        state.levilauncher_page_index,
        state.levilauncher_modal_open,
        state.levilauncher_selected_version.clone(),
        (
            state
                .levilauncher_selected_mod
                .as_ref()
                .map(|m| m.package_id.clone()),
            (
                state
                    .levilauncher_install_target_path
                    .clone()
                    .unwrap_or_else(|| SharedString::from("")),
                state.levilauncher_install_busy,
                state.levilauncher_install_targets_loading,
                state.levilauncher_install_targets.len(),
                state
                    .levilauncher_install_error
                    .clone()
                    .unwrap_or_else(|| SharedString::from("")),
            ),
            (
                state.native_mods_loaded,
                state.native_mods_loading,
                state.native_mods_error.is_some(),
                state.native_mods.len(),
                state.native_mod_modal_open,
                state.native_mod_selected_file.clone(),
                state
                    .native_mod_target_path
                    .clone()
                    .unwrap_or_else(|| SharedString::from("")),
                state.native_mod_install_busy,
                state
                    .native_mod_install_error
                    .clone()
                    .unwrap_or_else(|| SharedString::from("")),
            ),
        ),
    )
}

pub(crate) fn start_task_event_bridge(cx: &mut App) {
    cx.spawn_stream(
        crate::tasks::task_manager::task_event_stream(),
        |delivery, cx| match delivery {
            crate::tasks::task_manager::TaskEventDelivery::Event(event) => {
                let relevant = cx.read_global(|state: &DownloadPageState, _cx| {
                    task_event_relevant_to_download_state(state, &event)
                });
                if !relevant {
                    return;
                }
                let failure = cx.update_global(|state: &mut DownloadPageState, _cx| {
                    apply_task_event_to_download_state(state, event)
                });
                if let Some((file_name, error)) = failure {
                    let file_name_string = file_name.to_string();
                    let error_string = error.to_string();
                    toast::error(
                        cx,
                        t!(
                            "DownloadPage.operation_failed",
                            file = &file_name_string,
                            error = &error_string
                        ),
                    );
                }
            }
            crate::tasks::task_manager::TaskEventDelivery::Batch(events) => {
                let any_relevant = cx.read_global(|state: &DownloadPageState, _cx| {
                    events
                        .iter()
                        .any(|event| task_event_relevant_to_download_state(state, event))
                });
                if !any_relevant {
                    return;
                }
                let failures = cx.update_global(|state: &mut DownloadPageState, _cx| {
                    events
                        .into_iter()
                        .filter_map(|event| apply_task_event_to_download_state(state, event))
                        .collect::<Vec<_>>()
                });
                for (file_name, error) in failures {
                    let file_name_string = file_name.to_string();
                    let error_string = error.to_string();
                    toast::error(
                        cx,
                        t!(
                            "DownloadPage.operation_failed",
                            file = &file_name_string,
                            error = &error_string
                        ),
                    );
                }
            }
            crate::tasks::task_manager::TaskEventDelivery::ResyncRequired => {
                let (task_ids, has_stale_snapshots) =
                    cx.read_global(|state: &DownloadPageState, _cx| {
                        let task_ids = state
                            .operations_by_package
                            .values()
                            .map(|operation| operation.task_id.clone())
                            .collect::<Vec<_>>();
                        (task_ids, !state.task_snapshots.is_empty())
                    });
                if task_ids.is_empty() && !has_stale_snapshots {
                    return;
                }
                let snapshots = task_ids
                    .into_iter()
                    .filter_map(|task_id| {
                        crate::tasks::task_manager::get_snapshot_arc(task_id.as_ref())
                            .filter(|snapshot| {
                                snapshot.visibility
                                    != crate::tasks::task_manager::TaskVisibility::Hidden
                            })
                            .map(|snapshot| (Arc::from(task_id.as_ref()), snapshot))
                    })
                    .collect::<Vec<_>>();
                cx.update_global(|state: &mut DownloadPageState, _cx| {
                    state.task_snapshots.clear();
                    state.task_snapshots.extend(snapshots);
                });
            }
        },
    )
    .detach();
}

fn download_state_tracks_task(state: &DownloadPageState, task_id: &str) -> bool {
    state
        .operations_by_package
        .values()
        .any(|operation| operation.task_id.as_ref() == task_id)
}

fn task_event_relevant_to_download_state(
    state: &DownloadPageState,
    event: &crate::tasks::task_manager::TaskEvent,
) -> bool {
    match event {
        crate::tasks::task_manager::TaskEvent::Updated(snapshot) => {
            download_state_tracks_task(state, snapshot.id.as_ref())
        }
        crate::tasks::task_manager::TaskEvent::Removed(task_id) => {
            state.task_snapshots.contains_key(task_id.as_ref())
        }
    }
}

fn apply_task_event_to_download_state(
    state: &mut DownloadPageState,
    event: crate::tasks::task_manager::TaskEvent,
) -> Option<(SharedString, SharedString)> {
    match event {
        crate::tasks::task_manager::TaskEvent::Updated(snapshot) => {
            let operation_entry = state
                .operations_by_package
                .iter()
                .find(|(_, operation)| operation.task_id.as_ref() == snapshot.id.as_ref())
                .map(|(package_id, operation)| {
                    (package_id.clone(), operation.file_name.clone())
                });
            let Some((package_id, file_name)) = operation_entry else {
                return None;
            };

            if snapshot.is_terminal() {
                state.task_snapshots.remove(snapshot.id.as_ref());
                state.operations_by_package.remove(&package_id);
                if snapshot.status.as_ref() == "completed" {
                    if let Some(local_path) = snapshot.message.as_ref() {
                        state.local_path_by_package.insert(
                            package_id,
                            SharedString::from(local_path.to_string()),
                        );
                    }
                    state.local_files.insert(file_name);
                    return None;
                }

                let message = snapshot
                    .message
                    .as_ref()
                    .map(|message| SharedString::from(message.to_string()))
                    .unwrap_or_else(|| SharedString::from(snapshot.status.to_string()));
                return Some((file_name, message));
            }

            if snapshot.visibility == crate::tasks::task_manager::TaskVisibility::Hidden {
                state.task_snapshots.remove(snapshot.id.as_ref());
                return None;
            }
            let is_newer = state
                .task_snapshots
                .get(snapshot.id.as_ref())
                .is_none_or(|current| current.sequence <= snapshot.sequence);
            if is_newer {
                state.task_snapshots.insert(snapshot.id.clone(), snapshot);
            }
            None
        }
        crate::tasks::task_manager::TaskEvent::Removed(task_id) => {
            state.task_snapshots.remove(task_id.as_ref());
            None
        }
    }
}

pub struct DownloadPageView {
    _subscriptions: Vec<Subscription>,
    curseforge_resource_panel: Entity<curseforge::CurseForgeResourcePanelView>,
    game_panel_view: Option<Entity<game::DownloadGamePanelView>>,
    mod_panel_cache: mods::ModPanelRenderCache,
    native_mod_panel_cache: native::NativeModPanelRenderCache,
    mod_image_cache: Entity<BoundedImageCache>,
    secondary_prefetch_scheduled: bool,
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
    last_observed_mod_panel_signature: ModPanelObserveSignature,
}

impl DownloadPageView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (
            last_observed_tab,
            last_observed_curseforge_toolbar_signature,
            last_observed_mod_panel_signature,
        ) = cx.read_global(|state: &DownloadPageState, _cx| {
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
                build_mod_panel_observe_signature(state),
            )
        });

        let mut subscriptions = Vec::new();
        subscriptions.push(cx.observe_global::<DownloadPageState>(|this, cx| {
            let (tab, curseforge_toolbar_signature, mod_panel_signature) =
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
                        build_mod_panel_observe_signature(state),
                    )
                });

            let tab_changed = this.last_observed_tab != tab;
            let curseforge_toolbar_changed =
                this.last_observed_curseforge_toolbar_signature != curseforge_toolbar_signature;
            let mod_panel_changed = this.last_observed_mod_panel_signature != mod_panel_signature;

            this.last_observed_tab = tab;
            this.last_observed_curseforge_toolbar_signature = curseforge_toolbar_signature;
            this.last_observed_mod_panel_signature = mod_panel_signature;

            if tab_changed {
                // Keep signatures current while resident but hidden, without waking the window.
                if this.active {
                    cx.notify();
                }
                return;
            }

            if tab == DownloadTab::Mod {
                if this.active && mod_panel_changed {
                    cx.notify();
                }
                return;
            }

            if this.active
                && tab == DownloadTab::ResourcePack
                && curseforge_toolbar_changed
            {
                cx.notify();
            }
        }));
        subscriptions.push(cx.observe_global::<ThemeState>(|this, cx| {
            if this.active {
                cx.notify();
            }
        }));

        let page_jump_input =
            cx.read_global(|state: &DownloadPageState, _cx| state.page_jump_input.clone());
        if let Some(input) = page_jump_input {
            let sub = cx.subscribe(
                &input,
                |_this, input, ev: &crate::ui::components::input::InputEvent, cx| {
                    if matches!(
                        ev,
                        crate::ui::components::input::InputEvent::PressEnter { .. }
                    ) {
                        let raw = input.read(cx).value().to_string();
                        cx.update_global(|s: &mut DownloadPageState, cx| match s.tab {
                            DownloadTab::Game => {
                                let total_pages =
                                    (s.versions.len() + s.page_size - 1) / s.page_size;
                                if total_pages > 0 {
                                    let parsed = raw.trim().parse::<usize>().ok();
                                    if let Some(n) = parsed {
                                        let target = n.clamp(1, total_pages);
                                        s.page_index = target.saturating_sub(1);
                                        s.game_rows_scroll.set_offset(point(px(0.), px(0.)));
                                    }
                                }
                            }
                            DownloadTab::ResourcePack => {
                                let page_size = s.curseforge_page_size.max(1) as usize;
                                let total_count = s.curseforge_total_count.unwrap_or(0) as usize;
                                let total_pages = (total_count + page_size - 1) / page_size;
                                if total_pages > 0 {
                                    let parsed = raw.trim().parse::<usize>().ok();
                                    if let Some(n) = parsed {
                                        let target = n.clamp(1, total_pages);
                                        let target_page = target.saturating_sub(1);
                                        s.curseforge_page_index = target_page;
                                        s.curseforge_page_commit_task.take();
                                        s.curseforge_pending_page_index = None;
                                        curseforge::begin_page_results_transition_in_state(s, cx);
                                        curseforge::ensure_results_loaded_after_page_transition(
                                            false,
                                            target_page,
                                            cx,
                                        );
                                    }
                                }
                            }
                            DownloadTab::Mod => {}
                        });
                        let _ = input.update(cx, |st, cx| {
                            st.set_text(SharedString::from(""), cx);
                        });
                    }
                },
            );
            subscriptions.push(sub);
        }

        let game_panel_view = if last_observed_tab == DownloadTab::Game {
            Some(cx.new(game::DownloadGamePanelView::new))
        } else {
            None
        };

        Self {
            _subscriptions: subscriptions,
            curseforge_resource_panel: cx.new(curseforge::CurseForgeResourcePanelView::new),
            game_panel_view,
            mod_panel_cache: mods::ModPanelRenderCache::default(),
            native_mod_panel_cache: native::NativeModPanelRenderCache::default(),
            mod_image_cache: BoundedImageCache::new(
                BoundedImageCacheConfig {
                    max_items: 96,
                    max_bytes: 48 * 1024 * 1024,
                },
                cx,
            ),
            secondary_prefetch_scheduled: false,
            last_observed_tab,
            active: false,
            last_observed_curseforge_toolbar_signature,
            last_observed_mod_panel_signature,
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
        let active = self.active;
        let _ = view.update(cx, |view, cx| {
            view.set_active(active, cx);
            Ok::<(), anyhow::Error>(())
        });
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
        if let Some(game_panel_view) = &self.game_panel_view {
            let _ = game_panel_view.update(cx, |view, cx| {
                view.set_active(active, cx);
                Ok::<(), anyhow::Error>(())
            });
        }
        if active {
            cx.notify();
        }
    }
}

impl Render for DownloadPageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = window.animation_time();
        let theme = cx.global::<ThemeState>();
        let colors = lerp_theme_colors(
            &LightColors::colors(),
            &DarkColors::colors(),
            theme.factor(now),
            theme.accent,
        );
        let window_size = window.bounds().size;
        let active_tab = cx.read_global(|state: &DownloadPageState, _cx| state.tab);

        if self.active && !self.secondary_prefetch_scheduled {
            self.secondary_prefetch_scheduled = true;
            window.on_next_frame(|_window, cx| {
                // Preload the common Mod data while the user is still looking at the current tab.
                // Hidden-state observers absorb the resulting global updates without repainting the
                // Download page unless Mod is actually active.
                ensure_levilamina_support_loaded(cx);
                let native_source = cx.read_global(|state: &DownloadPageState, _cx| {
                    state.levilauncher_selected_loader == "native"
                });
                if native_source {
                    ensure_native_mods_loaded(cx);
                } else {
                    ensure_levilauncher_loaded(cx);
                }
            });
        }
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
                let (mod_modal_open, game_dialog_open) =
                    cx.read_global(|s: &DownloadPageState, _cx| {
                        (s.levilauncher_modal_open, s.game_dialog.is_some())
                    });
                if mod_modal_open {
                    cx.update_global(|s: &mut DownloadPageState, _cx| {
                        s.levilauncher_modal_open = false;
                        s.levilauncher_selected_mod = None;
                    });
                } else if game_dialog_open {
                    dismiss_game_dialog(cx);
                } else if cx.global::<DownloadPageState>().tab == DownloadTab::ResourcePack {
                    curseforge::handle_close_overlay(cx);
                }
            })
            .child(render_download_page(
                window,
                cx,
                colors,
                window_size.width,
                window_size.height,
                &self.curseforge_resource_panel,
                game_panel_view.as_ref(),
                &mut self.mod_panel_cache,
                &mut self.native_mod_panel_cache,
                &self.mod_image_cache,
            ))
    }
}

pub(super) fn render_download_page(
    _window: &mut Window,
    cx: &mut Context<DownloadPageView>,
    colors: ThemeColors,
    _window_width: Pixels,
    window_height: Pixels,
    curseforge_resource_panel: &Entity<curseforge::CurseForgeResourcePanelView>,
    game_panel_view: Option<&Entity<game::DownloadGamePanelView>>,
    mod_panel_cache: &mut mods::ModPanelRenderCache,
    native_mod_panel_cache: &mut native::NativeModPanelRenderCache,
    mod_image_cache: &Entity<BoundedImageCache>,
) -> impl IntoElement {
    let (active_tab, tab_anim_from, tab_anim_seq) =
        cx.read_global(|state: &DownloadPageState, _cx| {
            (state.tab, state.tab_anim_from, state.tab_anim_seq)
        });

    // Mirror `.upstream_bmbl_1/src/components/UnifiedPageLayout/*`:
    // one glass panel with a fixed header, a scrollable content area, and a footer.
    let header = div()
        .flex()
        .flex_col()
        .child(
            toolbar::render_toolbar(
                &colors,
                cx.global::<DownloadPageState>(),
                cx.global::<I18n>(),
            )
            .rounded(px(0.))
            .border_0(),
        )
        .child(div().h(px(1.)).bg(Hsla {
            a: 0.06,
            ..colors.border
        }));

    // ResourcePack 自己维护真实的左侧分类栏、右侧内容壳和结果列表加载态。
    // 外层统一骨架只负责游戏和模组，避免把 ResourcePack 整个页面替换掉。
    let show_loading = active_tab != DownloadTab::ResourcePack
        && cx.read_global(|state: &DownloadPageState, _cx| {
            loading::should_render_loading(state, active_tab)
        });

    let body: AnyElement = if show_loading {
        loading::render_loading_placeholder(&colors, window_height).into_any_element()
    } else {
        match active_tab {
            DownloadTab::Game => game_panel_view
                .cloned()
                .map(IntoElement::into_any_element)
                .unwrap_or_else(|| div().size_full().into_any_element()),
            DownloadTab::ResourcePack => curseforge_resource_panel.clone().into_any_element(),
            DownloadTab::Mod => {
                mods::render_mod_panel(
                    cx,
                    &colors,
                    mod_panel_cache,
                    native_mod_panel_cache,
                    mod_image_cache,
                )
                .into_any_element()
            }
        }
    };

    let body = if tab_anim_seq != 0
        && tab_anim_from != active_tab
        && !crate::core::ui_prefs::reduced_motion()
    {
        div()
            .size_full()
            .min_w(px(0.))
            .min_h(px(0.))
            .child(body)
            .composite_layer()
            .with_animation(
                tab_content_animation_key("download-tab-content", tab_anim_seq),
                tab_content_motion(tab_anim_from.index(), active_tab.index()),
                |content, _progress| content,
            )
            .into_any_element()
    } else {
        body
    };

    let unified_panel = crate::ui::components::page_shell::page_panel(&colors)
        .size_full()
        .flex()
        .flex_col()
        .child(header)
        .child(
            // Keep the page-sized/virtualized body at final geometry. The toolbar's lightweight
            // indicator owns the tab animation cadence, so list rows and images are never relaid
            // out simply to animate a tab transition.
            div()
                .flex_1()
                .min_h(px(0.))
                .min_w(px(0.))
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
        state.game_dialog_install_levilamina = false;
        state.game_dialog_selected_levilamina_version = SharedString::from("");
    });
}

pub fn render_download_overlay(colors: &ThemeColors, cx: &App) -> Option<AnyElement> {
    let (has_game_dialog, levilauncher_modal_open, native_mod_modal_open) =
        cx.read_global(|state: &DownloadPageState, _cx| {
            (
                state.game_dialog.is_some(),
                state.levilauncher_modal_open,
                state.native_mod_modal_open,
            )
        });

    if has_game_dialog {
        let (
            dialog,
            dialog_folder_input,
            cdn_loading,
            cdn_error,
            cdn_results,
            selected_cdn_base,
            cdn_expanded,
            loader_versions,
            install_levilamina,
            selected_levilamina_version,
        ) = cx.read_global(|state: &DownloadPageState, _cx| {
            (
                state.game_dialog.clone(),
                state.game_dialog_folder_input.clone(),
                state.game_dialog_cdn_loading,
                state.game_dialog_cdn_error.clone(),
                state.game_dialog_cdn_results.clone(),
                state.game_dialog_selected_cdn_base.clone(),
                state.game_dialog_cdn_expanded,
                state
                    .game_dialog
                    .as_ref()
                    .map(|dialog| {
                        state
                            .levilamina_support
                            .loader_versions(dialog.version.as_ref())
                    })
                    .unwrap_or_default(),
                state.game_dialog_install_levilamina,
                state.game_dialog_selected_levilamina_version.clone(),
            )
        });

        if let Some(dialog) = dialog {
            return Some(
                modal::modal_layer_dismissible(
                    game::render_game_dialog(
                        colors,
                        cx.global::<crate::ui::state::i18n::I18n>(),
                        dialog,
                        dialog_folder_input.as_ref(),
                        cdn_loading,
                        cdn_error,
                        cdn_results,
                        selected_cdn_base,
                        cdn_expanded,
                        loader_versions,
                        install_levilamina,
                        selected_levilamina_version,
                    ),
                    hsla(0.0, 0.0, 0.0, 0.28),
                    Rc::new(dismiss_game_dialog),
                )
                .into_any_element(),
            );
        }
    }

    let levilauncher_selected_mod = levilauncher_modal_open
        .then(|| {
            cx.read_global(|state: &DownloadPageState, _cx| state.levilauncher_selected_mod.clone())
        })
        .flatten();

    if levilauncher_modal_open && let Some(mod_entry) = levilauncher_selected_mod {
        let dismiss_fn = Rc::new(|cx: &mut App| {
            cx.update_global(|s: &mut DownloadPageState, _cx| {
                s.levilauncher_modal_open = false;
                s.levilauncher_selected_mod = None;
            });
        });

        return Some(
            modal::modal_layer_dismissible(
                mods::render_detail_modal_content(colors, cx, &mod_entry),
                hsla(0.0, 0.0, 0.0, 0.45),
                dismiss_fn,
            )
            .into_any_element(),
        );
    }

    let native_mod_selected = native_mod_modal_open
        .then(|| cx.read_global(|state: &DownloadPageState, _cx| state.native_mod_selected.clone()))
        .flatten();
    if native_mod_modal_open && let Some(mod_entry) = native_mod_selected {
        let dismiss_fn = Rc::new(|cx: &mut App| {
            native::dismiss_modal(cx);
        });
        return Some(
            modal::modal_layer_dismissible(
                native::render_detail_modal_content(colors, cx, &mod_entry),
                hsla(0.0, 0.0, 0.0, 0.45),
                dismiss_fn,
            )
            .into_any_element(),
        );
    }

    curseforge::render_curseforge_install_overlay(colors, cx)
}

fn merge_support_loader_versions(state: &mut DownloadPageState) {
    for version in state.levilamina_support.all_loader_versions() {
        let version = SharedString::from(version);
        if !state.levilauncher_loader_versions.contains(&version) {
            state.levilauncher_loader_versions.push(version);
        }
    }
    state.levilauncher_loader_versions.sort_by(|left, right| {
        crate::core::levilamina::compare_version_desc(left.as_ref(), right.as_ref())
    });
}

pub fn ensure_levilauncher_loaded(cx: &mut App) {
    let (loaded, loading) = cx
        .read_global(|s: &DownloadPageState, _cx| (s.levilauncher_loaded, s.levilauncher_loading));
    if loaded || loading {
        return;
    }

    cx.update_global(|s: &mut DownloadPageState, _cx| {
        s.levilauncher_loading = true;
        s.levilauncher_error = None;
    });

    let load_task = gpui_tokio::Tokio::spawn_result(cx, async {
        crate::core::levilamina::package_index()
            .await
            .map_err(anyhow::Error::msg)
    });
    cx.spawn(async move |cx| {
        let result = load_task.await;
        let _ = cx.update_global(|s: &mut DownloadPageState, _cx| {
            s.levilauncher_loading = false;
            match result {
                Ok(data) => {
                    s.levilauncher_loaded = true;
                    s.levilauncher_loader_versions = data
                        .loader_versions
                        .into_iter()
                        .map(SharedString::from)
                        .collect();
                    s.levilauncher_all_mods = data.client_mods;
                    merge_support_loader_versions(s);
                }
                Err(err) => {
                    s.levilauncher_loaded = false;
                    s.levilauncher_error = Some(SharedString::from(err.to_string()));
                }
            }
        });
    })
    .detach();
}

pub fn ensure_native_mods_loaded(cx: &mut App) {
    let (loaded, loading) = cx.read_global(|state: &DownloadPageState, _cx| {
        (state.native_mods_loaded, state.native_mods_loading)
    });
    if loaded || loading {
        return;
    }

    cx.update_global(|state: &mut DownloadPageState, _cx| {
        state.native_mods_loading = true;
        state.native_mods_error = None;
    });
    let load_task = gpui_tokio::Tokio::spawn_result(cx, async {
        crate::core::native_mods::package_index()
            .await
            .map_err(anyhow::Error::msg)
    });
    cx.spawn(async move |cx| {
        let result = load_task.await;
        let _ = cx.update_global(|state: &mut DownloadPageState, _cx| {
            state.native_mods_loading = false;
            match result {
                Ok(entries) => {
                    state.native_mods_loaded = true;
                    state.native_mods = entries;
                }
                Err(error) => {
                    state.native_mods_loaded = false;
                    state.native_mods_error = Some(SharedString::from(error.to_string()));
                }
            }
        });
    })
    .detach();
}

pub fn ensure_levilamina_support_loaded(cx: &mut App) {
    let (loaded, loading) = cx.read_global(|state: &DownloadPageState, _cx| {
        (
            state.levilamina_support_loaded,
            state.levilamina_support_loading,
        )
    });
    if loaded || loading {
        return;
    }

    cx.update_global(|state: &mut DownloadPageState, _cx| {
        state.levilamina_support_loading = true;
        state.levilamina_support_error = None;
    });
    let load_task = gpui_tokio::Tokio::spawn_result(cx, async {
        crate::core::levilamina::support_database()
            .await
            .map_err(anyhow::Error::msg)
    });
    cx.spawn(async move |cx| {
        let result = load_task.await;
        if let Err(error) = cx.update_global(|state: &mut DownloadPageState, _cx| {
            state.levilamina_support_loading = false;
            match result {
                Ok(database) => {
                    state.levilamina_support_loaded = true;
                    state.levilamina_support = database;
                    if state.levilauncher_loaded {
                        merge_support_loader_versions(state);
                    }
                }
                Err(error) => {
                    state.levilamina_support_loaded = false;
                    state.levilamina_support_error = Some(SharedString::from(error.to_string()));
                }
            }
        }) {
            warn!(%error, "LeviLamina 支持数据库加载完成后 UI 已释放");
        }
    })
    .detach();
}
