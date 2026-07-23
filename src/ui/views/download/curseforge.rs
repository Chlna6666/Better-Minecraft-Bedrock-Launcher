use crate::tasks::task_manager::TaskSnapshot;
use crate::ui::animation::repeating_linear_motion;
use crate::ui::components::dropdown::{Dropdown, DropdownOption};
use crate::ui::components::html_renderer::render_html_document;
use crate::ui::components::icon::themed_icon;
use crate::ui::components::input::{Input, InputState, Paste};
use crate::ui::components::modal;
use crate::ui::hooks::use_local_versions::{
    LocalVersionsSnapshot, read_local_versions_snapshot, use_local_versions,
};
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::download::state::{DownloadPageState, DownloadTab};
use anyhow::Result;
use gpui::AnimationExt;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_hooks::{hook_element, hook_render};
use lucide_gpui::icons as lucide_icons;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::env;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::common::{
    format_bytes, format_count, format_date_ymd, sanitize_single_line, status_card,
    truncate_with_ellipsis, wait_task_finished,
};
use super::is_entity_released_error;

mod content;
mod modals;
mod results;
mod results_state;
mod share_actions;

pub(crate) use results::{
    CurseForgeResultCardProps, CurseForgeResultsListView,
    render_curseforge_results_list_placeholder_aligned, render_result_logo_placeholder,
};

pub(crate) use modals::handle_close_overlay;
pub(crate) use results_state::{
    apply_results_query_change_in_state, begin_page_results_transition_in_state,
    ensure_results_loaded, ensure_results_loaded_after_page_transition,
    invalidate_results_now_in_state, schedule_invalidate_results_in_state,
};
pub(crate) use share_actions::handle_clipboard_share_paste;

const CURSEFORGE_RESULT_CARD_PITCH_PX: f32 = 84.0;
const CURSEFORGE_PAGE_COMMIT_DELAY_MS: u64 = 180;
const CURSEFORGE_RESULTS_TRANSITION_MS: u64 = 180;
const CURSEFORGE_RESULTS_REVEAL_WARMUP_MS: u64 = 16;
const CURSEFORGE_RESULT_CARD_STAGGER_MS: u64 = 18;
const CURSEFORGE_RESULT_CARD_ANIMATION_MS: u64 = 120;
const CURSEFORGE_RESULT_LOGO_RENDER_BUDGET: usize = 12;
const CURSEFORGE_RESULT_CARD_OVERSCAN: usize = 2;
const CURSEFORGE_SIDEBAR_IMAGE_CACHE_ITEMS: usize = 64;
const CURSEFORGE_SIDEBAR_IMAGE_BYTES_PER_ITEM: usize = 96 * 1024;
const CURSEFORGE_DETAIL_IMAGE_CACHE_ITEMS: usize = 24;
const CURSEFORGE_DETAIL_IMAGE_BYTES_PER_ITEM: usize = 512 * 1024;
const CURSEFORGE_DETAIL_SCROLL_ACCELERATION: f32 = 1.65;

pub(crate) fn should_render_curseforge_result_images() -> bool {
    env::var_os("BMCBL_DISABLE_CURSEFORGE_RESULT_IMAGES").is_none()
}

pub(crate) fn should_use_gpui_direct_result_images() -> bool {
    true
}

pub(crate) fn should_mount_curseforge_result_images() -> bool {
    true
}

pub(crate) fn should_render_curseforge_sidebar() -> bool {
    env::var_os("BMCBL_CURSEFORGE_NO_SIDEBAR").is_none()
}

pub(crate) fn should_mount_curseforge_sidebar_images() -> bool {
    env::var_os("BMCBL_CURSEFORGE_SIDEBAR_PLACEHOLDER_ONLY").is_none()
}

pub(crate) fn should_use_gpui_direct_sidebar_images() -> bool {
    true
}

pub(crate) fn should_animate_curseforge_result_cards() -> bool {
    env::var_os("BMCBL_CURSEFORGE_RESULT_STATIC").is_none()
}

fn curseforge_sidebar_image_cache_config() -> BoundedImageCacheConfig {
    BoundedImageCacheConfig {
        max_items: CURSEFORGE_SIDEBAR_IMAGE_CACHE_ITEMS,
        max_bytes: CURSEFORGE_SIDEBAR_IMAGE_CACHE_ITEMS
            .saturating_mul(CURSEFORGE_SIDEBAR_IMAGE_BYTES_PER_ITEM),
    }
}
pub(crate) struct GlobalCurseForgeDetailImageCache(pub Entity<BoundedImageCache>);
impl gpui::Global for GlobalCurseForgeDetailImageCache {}

fn curseforge_detail_image_cache_config() -> BoundedImageCacheConfig {
    BoundedImageCacheConfig {
        max_items: CURSEFORGE_DETAIL_IMAGE_CACHE_ITEMS,
        max_bytes: CURSEFORGE_DETAIL_IMAGE_CACHE_ITEMS
            .saturating_mul(CURSEFORGE_DETAIL_IMAGE_BYTES_PER_ITEM),
    }
}

fn clamped_curseforge_results_scroll_offset_y(state: &DownloadPageState) -> Pixels {
    let scroll_handle = &state.curseforge_results_scroll;
    let max_offset_y = scroll_handle.max_offset().height;
    scroll_handle.offset().y.clamp(-max_offset_y, px(0.))
}

fn clamp_curseforge_results_scroll_in_state(state: &mut DownloadPageState) -> bool {
    let current_offset = state.curseforge_results_scroll.offset();
    let clamped_offset_y = clamped_curseforge_results_scroll_offset_y(state);

    if current_offset.y == clamped_offset_y && current_offset.x == px(0.) {
        return false;
    }

    state
        .curseforge_results_scroll
        .set_offset(point(px(0.), clamped_offset_y));
    true
}

#[derive(Clone, PartialEq)]
struct CurseForgeResourcePanelSignature {
    install_open: bool,
    install_stage: crate::ui::views::download::state::CurseForgeInstallStage,
    install_error: Option<SharedString>,
    install_file_count: usize,
    install_selected_file_id: Option<i32>,
    install_target_folder: Option<SharedString>,
    install_task_id: Option<SharedString>,
    install_downloaded_path: Option<SharedString>,
    install_conflict_message: Option<SharedString>,
    mod_page_open: bool,
    mod_page_loading: bool,
    mod_page_error: Option<SharedString>,
    mod_page_mod_id: Option<i32>,
    mod_page_mod: Option<crate::ui::views::download::state::CurseForgeModEntry>,
    mod_page_description_len: usize,
}

fn curseforge_resource_panel_signature(
    state: &DownloadPageState,
) -> CurseForgeResourcePanelSignature {
    CurseForgeResourcePanelSignature {
        install_open: state.curseforge_install_open,
        install_stage: state.curseforge_install_stage,
        install_error: state.curseforge_install_error.clone(),
        install_file_count: state.curseforge_install_files.len(),
        install_selected_file_id: state.curseforge_install_selected_file_id,
        install_target_folder: state.curseforge_install_target_folder.clone(),
        install_task_id: state.curseforge_install_task_id.clone(),
        install_downloaded_path: state.curseforge_install_downloaded_path.clone(),
        install_conflict_message: state.curseforge_install_conflict_message.clone(),
        mod_page_open: state.curseforge_mod_page_open,
        mod_page_loading: state.curseforge_mod_page_loading,
        mod_page_error: state.curseforge_mod_page_error.clone(),
        mod_page_mod_id: state.curseforge_mod_page_mod_id,
        mod_page_mod: state.curseforge_mod_page_mod.clone(),
        mod_page_description_len: state.curseforge_mod_page_description.as_ref().len(),
    }
}

#[hook_element]
pub(crate) struct CurseForgeResourcePanelView {
    tasks: HashMap<Arc<str>, Arc<TaskSnapshot>>,
    _subscriptions: Vec<Subscription>,
    task_updates_task: Option<Task<anyhow::Result<()>>>,
    focus_handle: FocusHandle,
    curseforge_sidebar: Entity<CurseForgeSidebarView>,
    curseforge_content: Entity<CurseForgeContentView>,
    detail_image_cache: Entity<BoundedImageCache>,
    initial_tab: crate::ui::views::download::state::DownloadTab,
    last_signature: CurseForgeResourcePanelSignature,
    active: bool,
}

impl CurseForgeResourcePanelView {
    fn spawn_task_updates(&mut self, cx: &mut Context<Self>) {
        let mut updates = crate::tasks::task_manager::subscribe_task_updates();
        let task = cx.spawn(async move |handle, cx| {
            loop {
                let first_snapshot = match updates.recv().await {
                    Ok(snapshot) => snapshot,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return Ok::<(), anyhow::Error>(());
                    }
                };

                let mut batch = HashMap::new();
                batch.insert(first_snapshot.id.clone(), first_snapshot);
                loop {
                    match updates.try_recv() {
                        Ok(snapshot) => {
                            batch.insert(snapshot.id.clone(), snapshot);
                        }
                        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                        Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                            return Ok::<(), anyhow::Error>(());
                        }
                    }
                }
                let snapshots = batch.into_values().collect::<Vec<_>>();

                match handle.update(cx, |this, cx| {
                    if !this.active {
                        return;
                    }

                    for snapshot in snapshots {
                        this.tasks.insert(snapshot.id.clone(), snapshot);
                    }
                    let should_notify = cx.read_global(|state: &DownloadPageState, _cx| {
                        state.curseforge_install_open
                    });
                    if should_notify {
                        cx.notify();
                    }
                }) {
                    Ok(()) => {}
                    Err(error) if is_entity_released_error(&error) => {
                        return Ok::<(), anyhow::Error>(());
                    }
                    Err(error) => return Err(error),
                }
            }
        });
        self.task_updates_task = Some(task);
    }

    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let cache = BoundedImageCache::new(curseforge_detail_image_cache_config(), cx);
        cx.set_global(GlobalCurseForgeDetailImageCache(cache.clone()));

        let (initial_tab, initial_signature) = cx.read_global(|state: &DownloadPageState, _cx| {
            (state.tab, curseforge_resource_panel_signature(state))
        });

        let mut subscriptions = vec![
            cx.observe_global::<DownloadPageState>(|this, cx| {
                let (current_tab, current_signature) =
                    cx.read_global(|state: &DownloadPageState, _cx| {
                        (state.tab, curseforge_resource_panel_signature(state))
                    });

                let mut should_notify = false;
                if current_tab != this.initial_tab {
                    this.initial_tab = current_tab;
                    should_notify = true;
                }
                if current_signature != this.last_signature {
                    this.last_signature = current_signature;
                    should_notify = true;
                }
                if should_notify {
                    cx.notify();
                }
            }),
            cx.observe_global::<crate::ui::views::manage::state::ManagePageState>(|_, cx| {
                let should_notify = cx.read_global(|state: &DownloadPageState, _cx| {
                    state.curseforge_install_open || state.curseforge_mod_page_open
                });
                if should_notify {
                    cx.notify();
                }
            }),
        ];
        subscriptions.shrink_to_fit();
        let mut this = Self {
            tasks: crate::tasks::task_manager::snapshot_arcs_map(),
            _subscriptions: subscriptions,
            task_updates_task: None,
            focus_handle: cx.focus_handle().tab_stop(true),
            curseforge_sidebar: cx.new(CurseForgeSidebarView::new),
            curseforge_content: cx.new(CurseForgeContentView::new),
            detail_image_cache: cache,
            initial_tab,
            last_signature: initial_signature,
            active: true,
            __gpui_hooks: RefCell::new(Vec::new()),
            __gpui_hook_index: Cell::new(0),
            __gpui_hook_count: Cell::new(0),
        };
        this.spawn_task_updates(cx);
        this
    }

    pub(crate) fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }

        self.active = active;
        if active {
            self.tasks = crate::tasks::task_manager::snapshot_arcs_map();
            if self.task_updates_task.is_none() {
                self.spawn_task_updates(cx);
            }
        } else {
            self.task_updates_task.take();
        }
    }
}

#[hook_render]
impl Render for CurseForgeResourcePanelView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let theme = cx.global::<crate::ui::state::theme::ThemeState>();
        let colors = crate::ui::theme::colors::lerp_theme_colors(
            &crate::ui::theme::colors::LightColors::colors(),
            &crate::ui::theme::colors::DarkColors::colors(),
            theme.factor(now),
            theme.accent,
        );
        let local_versions = use_local_versions(self, cx);
        let state = cx.global::<DownloadPageState>();
        let manage_state = cx.global::<crate::ui::views::manage::state::ManagePageState>();
        render_resource_panel(
            &colors,
            state,
            &self.curseforge_sidebar,
            &self.curseforge_content,
            &self.detail_image_cache,
            manage_state.selected_folder.clone(),
            &local_versions,
            &self.tasks,
        )
        .key_context("Download")
        .track_focus(&self.focus_handle)
        .on_mouse_down(MouseButton::Left, {
            let focus_handle = self.focus_handle.clone();
            move |_event, window, cx| {
                if !focus_handle.contains_focused(window, cx) {
                    focus_handle.focus(window);
                }
            }
        })
    }
}

pub(crate) struct CurseForgeSidebarView {
    _subscriptions: Vec<Subscription>,
    sidebar_image_cache: Entity<BoundedImageCache>,
    last_root_id: Option<i32>,
    last_sub_id: Option<i32>,
    last_category_count: usize,
    last_loaded: bool,
}

impl CurseForgeSidebarView {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        if !should_render_curseforge_sidebar() {
            return Self {
                _subscriptions: Vec::new(),
                sidebar_image_cache: BoundedImageCache::new(
                    curseforge_sidebar_image_cache_config(),
                    cx,
                ),
                last_root_id: None,
                last_sub_id: None,
                last_category_count: 0,
                last_loaded: false,
            };
        }

        let (last_root_id, last_sub_id, last_category_count, last_loaded) =
            cx.read_global(|state: &DownloadPageState, _cx| {
                (
                    state.curseforge_selected_root_id,
                    state.curseforge_selected_sub_id,
                    state.curseforge_categories.len(),
                    state.curseforge_loaded,
                )
            });
        let subscriptions = vec![cx.observe_global::<DownloadPageState>(|this, cx| {
            let (root_id, sub_id, category_count, loaded) =
                cx.read_global(|state: &DownloadPageState, _cx| {
                    (
                        state.curseforge_selected_root_id,
                        state.curseforge_selected_sub_id,
                        state.curseforge_categories.len(),
                        state.curseforge_loaded,
                    )
                });

            if root_id != this.last_root_id
                || sub_id != this.last_sub_id
                || category_count != this.last_category_count
                || loaded != this.last_loaded
            {
                this.last_root_id = root_id;
                this.last_sub_id = sub_id;
                this.last_category_count = category_count;
                this.last_loaded = loaded;
                cx.notify();
            }
        })];

        Self {
            _subscriptions: subscriptions,
            sidebar_image_cache: BoundedImageCache::new(
                curseforge_sidebar_image_cache_config(),
                cx,
            ),
            last_root_id,
            last_sub_id,
            last_category_count,
            last_loaded,
        }
    }
}

impl Render for CurseForgeSidebarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let theme = cx.global::<crate::ui::state::theme::ThemeState>();
        let colors = crate::ui::theme::colors::lerp_theme_colors(
            &crate::ui::theme::colors::LightColors::colors(),
            &crate::ui::theme::colors::DarkColors::colors(),
            theme.factor(now),
            theme.accent,
        );
        let state = cx.global::<DownloadPageState>();
        render_curseforge_sidebar(&colors, state, &self.sidebar_image_cache)
    }
}

pub(crate) struct CurseForgeContentView {
    _subscriptions: Vec<Subscription>,
    curseforge_results_list: Entity<CurseForgeResultsListView>,
    last_tab: crate::ui::views::download::state::DownloadTab,
}

impl CurseForgeContentView {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let initial_tab = cx.read_global(|state: &DownloadPageState, _cx| state.tab);

        let subscriptions = vec![cx.observe_global::<DownloadPageState>(|this, cx| {
            // 关键：只在标签页变化时刷新
            let current_tab = cx.read_global(|state: &DownloadPageState, _cx| state.tab);

            if current_tab != this.last_tab {
                this.last_tab = current_tab;
                cx.notify();
            }
        })];
        Self {
            _subscriptions: subscriptions,
            curseforge_results_list: cx.new(CurseForgeResultsListView::new),
            last_tab: initial_tab,
        }
    }
}

impl Render for CurseForgeContentView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = std::time::Instant::now();
        let theme = cx.global::<crate::ui::state::theme::ThemeState>();
        let colors = crate::ui::theme::colors::lerp_theme_colors(
            &crate::ui::theme::colors::LightColors::colors(),
            &crate::ui::theme::colors::DarkColors::colors(),
            theme.factor(now),
            theme.accent,
        );
        content::render_curseforge_content(window, cx, &colors, &self.curseforge_results_list, now)
    }
}

fn normalize_curseforge_tag_key(value: &str) -> String {
    let mut normalized = value
        .trim()
        .to_lowercase()
        .replace('+', " plus ")
        .replace('&', " and ")
        .replace(['’', '\''], "")
        .replace([',', '(', ')'], " ");
    normalized = normalized
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    normalized = normalized
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    if normalized
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_digit())
    {
        format!("tag_{normalized}")
    } else {
        normalized
    }
}

fn truncate_curseforge_list_text(
    value: &SharedString,
    available_width_px: f32,
    min_chars: usize,
    max_chars: usize,
    pixels_per_char: f32,
) -> SharedString {
    let computed = (available_width_px / pixels_per_char).floor() as usize;
    let limit = computed.clamp(min_chars, max_chars);
    truncate_with_ellipsis(value.as_ref(), limit)
}

fn build_curseforge_result_card_props(
    state: &DownloadPageState,
    visible_slice_start: usize,
    visible_slice_len: usize,
) -> Vec<CurseForgeResultCardProps> {
    let category_by_id = state
        .curseforge_categories
        .iter()
        .map(|category| (category.id, category))
        .collect::<HashMap<_, _>>();

    state
        .curseforge_mods
        .iter()
        .skip(visible_slice_start)
        .take(visible_slice_len)
        .map(|mod_entry| {
            let title = SharedString::from(sanitize_single_line(mod_entry.name.as_ref()));

            let summary = match mod_entry.summary.as_ref() {
                Some(summary) if !summary.trim().is_empty() => {
                    SharedString::from(sanitize_single_line(summary.as_ref()))
                }
                _ => SharedString::from("暂无简介"),
            };

            let authors = if mod_entry.author_names.is_empty() {
                SharedString::from("未知作者")
            } else {
                let joined = mod_entry
                    .author_names
                    .iter()
                    .take(3)
                    .map(|author_name| author_name.as_ref())
                    .collect::<Vec<_>>()
                    .join(", ");
                SharedString::from(sanitize_single_line(&joined))
            };

            let primary_tag_category = mod_entry
                .category_ids
                .iter()
                .find_map(|category_id| {
                    category_by_id
                        .get(category_id)
                        .copied()
                        .filter(|category| category.icon_url.is_some())
                })
                .or_else(|| {
                    mod_entry
                        .category_ids
                        .iter()
                        .find_map(|category_id| category_by_id.get(category_id).copied())
                });
            let primary_tag_label = primary_tag_category.map(|category| {
                localize_curseforge_tag(category.name.as_ref(), Some(category.slug.as_ref()))
            });

            CurseForgeResultCardProps {
                mod_id: mod_entry.id,
                title,
                summary,
                authors,
                primary_tag_label,
                logo_url: mod_entry.logo_url.clone(),
                download_count_label: format_count(mod_entry.download_count),
                date_modified_label: format_date_ymd(mod_entry.date_modified.as_ref()),
            }
        })
        .collect()
}

fn scroll_event_delta_y_with_line_height(event: &ScrollWheelEvent, line_height: Pixels) -> Pixels {
    event.delta.pixel_delta(line_height).y
}

fn localize_curseforge_tag(name: &str, slug: Option<&str>) -> SharedString {
    let key = slug
        .map(normalize_curseforge_tag_key)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| normalize_curseforge_tag_key(name));

    let localized = match key.as_str() {
        "miscellaneous" => "杂项",
        "pvp" => "PVP",
        "utility" => "实用",
        "adventure" => "冒险",
        "survival" => "生存",
        "horror" => "恐怖",
        "medieval" => "中世纪",
        "modern" => "现代",
        "technology" => "科技",
        "furniture" => "家具",
        "weapons" => "武器",
        "shaders" => "光影",
        "realistic" => "写实",
        "cartoon" => "卡通",
        "vanilla" => "原版风格",
        "mini_game" => "小游戏",
        "parkour" => "跑酷",
        "creation" => "建筑",
        "world_generation" => "世界生成",
        "gui" => "界面",
        "16x" => "16x",
        "32x" => "32x",
        "64x" => "64x",
        "128x" => "128x",
        _ => name,
    };

    SharedString::from(localized.to_string())
}

fn curseforge_file_version_label(
    file: &crate::ui::views::download::state::CurseForgeFileEntry,
) -> SharedString {
    file.game_versions
        .iter()
        .find(|version| version.chars().next().is_some_and(|ch| ch.is_ascii_digit()))
        .cloned()
        .unwrap_or_else(|| SharedString::from("未知版本"))
}

fn render_curseforge_detail_file_row(
    colors: &ThemeColors,
    file: &crate::ui::views::download::state::CurseForgeFileEntry,
) -> Div {
    let version_label = curseforge_file_version_label(file);
    let date_label = format_date_ymd(file.file_date.as_ref());

    div()
        .flex_1()
        .min_w(px(0.))
        .rounded(px(16.))
        .bg(Hsla {
            a: 0.42,
            ..colors.surface
        })
        .border_1()
        .border_color(Hsla {
            a: 0.08,
            ..colors.border
        })
        .p(px(14.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(14.))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(file.display_name.clone()),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .flex_wrap()
                        .text_size(px(11.))
                        .text_color(colors.text_muted)
                        .child(
                            div()
                                .h(px(24.))
                                .px(px(8.))
                                .rounded(px(999.))
                                .bg(Hsla {
                                    a: 0.10,
                                    ..colors.accent
                                })
                                .flex()
                                .items_center()
                                .child(version_label),
                        )
                        .child(format_bytes(file.file_length))
                        .child(date_label),
                ),
        )
}

fn render_curseforge_detail_files_panel(
    colors: &ThemeColors,
    state: &DownloadPageState,
    selected_folder: Option<SharedString>,
    local_versions: &LocalVersionsSnapshot,
) -> Div {
    let Some(mod_entry) = state.curseforge_mod_page_mod.as_ref().cloned() else {
        return div()
            .rounded(px(12.))
            .child(status_card(colors, "未选择资源", None));
    };

    let files_loading = matches!(
        state.curseforge_install_stage,
        crate::ui::views::download::state::CurseForgeInstallStage::LoadingFiles
    );
    let files_error = if state.curseforge_install_files.is_empty() {
        state.curseforge_install_error.clone()
    } else {
        None
    };
    div()
        .rounded(px(12.))
        .bg(Hsla {
            a: 0.72,
            ..colors.surface
        })
        .border_1()
        .border_color(colors.border)
        .p(px(22.))
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.text_primary)
                                .child("文件列表"),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(colors.text_muted)
                                .child("直接查看可安装文件、版本与更新时间"),
                        ),
                )
                .child(
                    div()
                        .h(px(30.))
                        .px(px(12.))
                        .rounded(px(999.))
                        .bg(Hsla {
                            a: 0.10,
                            ..colors.accent
                        })
                        .border_1()
                        .border_color(Hsla {
                            a: 0.10,
                            ..colors.border
                        })
                        .flex()
                        .items_center()
                        .text_size(px(11.))
                        .text_color(colors.text_secondary)
                        .child(format!("{} 个文件", state.curseforge_install_files.len())),
                ),
        )
        .when(files_loading, |this| {
            this.child(status_card(colors, "正在加载文件列表...", None))
        })
        .when_some(files_error, |this, error| {
            this.child(status_card(
                colors,
                &format!("文件列表加载失败: {error}"),
                Some(colors.danger),
            ))
        })
        .when(
            !files_loading
                && state.curseforge_install_error.is_none()
                && state.curseforge_install_files.is_empty(),
            |this| this.child(status_card(colors, "当前资源没有可展示的文件记录。", None)),
        )
        .when(!state.curseforge_install_files.is_empty(), |this| {
            let mut list = div().flex().flex_col().gap(px(10.));
            for file in state.curseforge_install_files.iter().take(12) {
                let file_id = file.id;
                let default_target = modals::default_install_target_for_file(
                    file,
                    selected_folder.clone(),
                    local_versions,
                );
                let row = render_curseforge_detail_file_row(colors, file);
                let action = div()
                    .h(px(40.))
                    .px(px(14.))
                    .rounded(px(12.))
                    .bg(colors.accent)
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(12.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.btn_primary_text)
                    .child(themed_icon(
                        lucide_icons::icon_download(),
                        16.0,
                        colors.btn_primary_text,
                    ))
                    .child("安装")
                    .on_mouse_down(MouseButton::Left, {
                        let mod_entry = mod_entry.clone();
                        let default_target = default_target.clone();
                        move |_ev, _window, cx| {
                            modals::open_curseforge_install_modal_for_file(
                                mod_entry.clone(),
                                file_id,
                                default_target.clone(),
                                cx,
                            );
                        }
                    });

                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(14.))
                        .child(row)
                        .child(action),
                );
            }
            this.child(list)
        })
}

fn render_curseforge_detail_description_panel(
    colors: &ThemeColors,
    state: &DownloadPageState,
) -> Div {
    let description_document = state.curseforge_mod_page_document.clone();
    let description_empty = description_document.blocks.is_empty();

    div()
        .rounded(px(12.))
        .bg(Hsla {
            a: 0.72,
            ..colors.surface
        })
        .border_1()
        .border_color(colors.border)
        .p(px(22.))
        .flex()
        .flex_col()
        .gap(px(18.))
        .when(description_empty, |this| {
            this.child(status_card(
                colors,
                "这个资源暂时没有提供更详细的项目介绍。",
                None,
            ))
        })
        .when(!description_empty, |this| {
            this.child(render_html_document(&description_document, colors, None))
        })
}

pub(super) fn render_resource_panel(
    _colors: &ThemeColors,
    state: &DownloadPageState,
    curseforge_sidebar: &Entity<CurseForgeSidebarView>,
    curseforge_content: &Entity<CurseForgeContentView>,
    detail_image_cache: &Entity<BoundedImageCache>,
    _selected_folder: Option<SharedString>,
    _local_versions: &LocalVersionsSnapshot,
    _tasks: &HashMap<Arc<str>, Arc<TaskSnapshot>>,
) -> Div {
    let body = div()
        .size_full()
        .flex()
        .overflow_hidden()
        .gap(px(20.))
        .p(px(12.))
        .min_h(px(0.))
        .min_w(px(0.))
        .when(should_render_curseforge_sidebar(), |this| {
            this.child(
                div()
                    .w(px(220.))
                    .flex_none()
                    .min_h(px(0.))
                    .overflow_hidden()
                    .child(curseforge_sidebar.clone().into_any_element()),
            )
        })
        .child(
            div()
                .min_w(px(0.))
                .min_h(px(0.))
                .flex_1()
                .overflow_hidden()
                .child(curseforge_content.clone().into_any_element()),
        );

    div()
        .size_full()
        .relative()
        .flex()
        .min_w(px(0.))
        .min_h(px(0.))
        .child(div().flex_1().min_w(px(0.)).min_h(px(0.)).child(body))
        .on_action(|_: &Paste, _window, cx| handle_clipboard_share_paste(cx))
}

pub(super) fn render_curseforge_install_overlay(
    colors: &ThemeColors,
    cx: &App,
) -> Option<AnyElement> {
    let state = cx.global::<DownloadPageState>();
    if state.tab != DownloadTab::ResourcePack {
        return None;
    }

    if !state.curseforge_install_open && !state.curseforge_mod_page_open {
        return None;
    }

    let selected_folder = cx.read_global(
        |state: &crate::ui::views::manage::state::ManagePageState, _cx| {
            state.selected_folder.clone()
        },
    );
    let local_versions = read_local_versions_snapshot(cx);
    let tasks = crate::tasks::task_manager::snapshot_arcs_map();

    if state.curseforge_install_open {
        return Some(
            modals::render_curseforge_install_modal(
                colors,
                state,
                selected_folder,
                &local_versions,
                &tasks,
            )
            .into_any_element(),
        );
    }

    if state.curseforge_mod_page_open {
        if let Some(cache_ref) = cx.try_global::<GlobalCurseForgeDetailImageCache>() {
            return Some(
                modals::render_curseforge_mod_page_modal(
                    colors,
                    state,
                    &cache_ref.0,
                    selected_folder,
                    &local_versions,
                    &tasks,
                )
                .into_any_element(),
            );
        }
    }

    None
}

fn render_curseforge_sidebar(
    colors: &ThemeColors,
    state: &DownloadPageState,
    sidebar_image_cache: &Entity<BoundedImageCache>,
) -> Div {
    let active_root = state.curseforge_selected_root_id;
    let active_sub = state.curseforge_selected_sub_id;

    let root_items: Vec<_> = state
        .curseforge_categories
        .iter()
        .filter(|c| c.is_class)
        .collect();

    let mut sub_items: Vec<_> = Vec::new();
    if let Some(root_id) = active_root {
        sub_items = state
            .curseforge_categories
            .iter()
            .filter(|c| c.class_id == Some(root_id) || c.parent_category_id == Some(root_id))
            .filter(|c| !c.is_class)
            .collect();
    }

    let sidebar_all_icon = |active: bool| {
        let fg = if active {
            colors.btn_primary_text
        } else {
            colors.text_primary
        };
        themed_icon(lucide_icons::icon_package(), 16.0, fg).into_any_element()
    };

    let sidebar_category_icon = |icon_url: Option<SharedString>, active: bool| -> AnyElement {
        match icon_url {
            Some(url) => {
                if should_mount_curseforge_sidebar_images() {
                    img(url)
                        .w(px(18.))
                        .h(px(18.))
                        .rounded(px(5.))
                        .object_fit(ObjectFit::Cover)
                        .image_cache(sidebar_image_cache)
                        .with_loading({
                            let colors = *colors;
                            move || {
                                div()
                                    .w(px(18.))
                                    .h(px(18.))
                                    .rounded(px(5.))
                                    .bg(Hsla {
                                        a: 0.12,
                                        ..colors.surface
                                    })
                                    .into_any_element()
                            }
                        })
                        .with_fallback({
                            let colors = *colors;
                            move || {
                                div()
                                    .w(px(18.))
                                    .h(px(18.))
                                    .rounded(px(5.))
                                    .bg(Hsla {
                                        a: 0.12,
                                        ..colors.surface
                                    })
                                    .into_any_element()
                            }
                        })
                        .into_any_element()
                } else {
                    div()
                        .w(px(18.))
                        .h(px(18.))
                        .rounded(px(5.))
                        .bg(Hsla {
                            a: 0.08,
                            ..colors.surface
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(themed_icon(
                            lucide_icons::icon_image(),
                            11.0,
                            colors.text_muted,
                        ))
                        .into_any_element()
                }
            }
            None => div().w(px(18.)).h(px(18.)).into_any_element(),
        }
    };

    let sidebar_item =
        |label: SharedString,
         icon: AnyElement,
         active: bool,
         on_click: Box<dyn Fn(&mut DownloadPageState) -> bool>| {
            let bg = if active {
                colors.accent
            } else {
                Hsla {
                    a: 0.0,
                    ..colors.accent
                }
            };
            let fg = if active {
                colors.btn_primary_text
            } else {
                colors.text_primary
            };
            div()
                .w_full()
                .px(px(12.))
                .py(px(9.))
                .rounded(px(10.))
                .bg(bg)
                .cursor_pointer()
                .flex()
                .items_center()
                .gap(px(10.))
                .overflow_hidden()
                .child(icon)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(13.))
                        .text_color(fg)
                        .child(label),
                )
                .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                    cx.update_global(|state: &mut DownloadPageState, cx| {
                        if on_click(state) {
                            state.curseforge_disable_result_logos = true;
                            invalidate_results_now_in_state(state, cx);
                        }
                    });
                    ensure_results_loaded(false, cx);
                })
        };

    let mut content = div().flex().flex_col().gap(px(2.));
    content = content.child(sidebar_item(
        SharedString::from("全部"),
        sidebar_all_icon(active_root.is_none()),
        active_root.is_none(),
        Box::new(|s: &mut DownloadPageState| {
            if s.curseforge_selected_root_id.is_none() && s.curseforge_selected_sub_id.is_none() {
                return false;
            }
            s.curseforge_selected_root_id = None;
            s.curseforge_selected_sub_id = None;
            s.curseforge_page_index = 0;
            true
        }),
    ));

    for c in root_items {
        let id = c.id;
        let label = localize_curseforge_tag(c.name.as_ref(), Some(c.slug.as_ref()));
        content = content.child(sidebar_item(
            label,
            sidebar_category_icon(c.icon_url.clone(), active_root == Some(id)),
            active_root == Some(id),
            Box::new(move |s| {
                if s.curseforge_selected_root_id == Some(id)
                    && s.curseforge_selected_sub_id.is_none()
                {
                    return false;
                }
                s.curseforge_selected_root_id = Some(id);
                s.curseforge_selected_sub_id = None;
                s.curseforge_page_index = 0;
                true
            }),
        ));
    }

    let sub_header = div()
        .w_full()
        .px(px(10.))
        .py(px(8.))
        .flex()
        .items_center()
        .justify_between()
        .cursor_pointer()
        .child(
            div()
                .text_size(px(11.))
                .font_weight(FontWeight::BOLD)
                .text_color(colors.text_secondary)
                .child("子分类"),
        )
        .child(themed_icon(
            lucide_icons::icon_chevron_down(),
            16.0,
            colors.text_secondary,
        ))
        .on_mouse_down(MouseButton::Left, |_ev, _window, cx| {
            cx.update_global(|s: &mut DownloadPageState, _cx| {
                s.curseforge_sub_collapsed = !s.curseforge_sub_collapsed;
            });
        });

    let mut sub_list = div().flex().flex_col().gap(px(2.)).pl(px(6.));
    if active_root.is_some() {
        sub_list = sub_list.child(sidebar_item(
            SharedString::from("全部子分类"),
            sidebar_category_icon(None, active_sub.is_none()),
            active_sub.is_none(),
            Box::new(|s| {
                if s.curseforge_selected_root_id.is_some() && s.curseforge_selected_sub_id.is_none()
                {
                    return false;
                }
                s.curseforge_selected_sub_id = None;
                s.curseforge_page_index = 0;
                true
            }),
        ));
        for c in sub_items {
            let id = c.id;
            let label = localize_curseforge_tag(c.name.as_ref(), Some(c.slug.as_ref()));
            sub_list = sub_list.child(sidebar_item(
                label,
                sidebar_category_icon(c.icon_url.clone(), active_sub == Some(id)),
                active_sub == Some(id),
                Box::new(move |s| {
                    if s.curseforge_selected_sub_id == Some(id) {
                        return false;
                    }
                    s.curseforge_selected_sub_id = Some(id);
                    s.curseforge_page_index = 0;
                    true
                }),
            ));
        }
    }

    let status: Option<AnyElement> = if let Some(err) = state.curseforge_error.as_ref() {
        Some(
            status_card(colors, &format!("加载失败: {err}"), Some(colors.danger))
                .into_any_element(),
        )
    } else {
        None
    };

    let share_card = div()
        .rounded(px(16.))
        .border_1()
        .border_color(colors.border)
        .bg(Hsla {
            a: 0.55,
            ..colors.surface
        })
        .p(px(12.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::BOLD)
                .text_color(colors.text_secondary)
                .child("分享导入"),
        )
        .child(
            div()
                .text_size(px(11.))
                .line_height(px(16.))
                .text_color(colors.text_muted)
                .whitespace_normal()
                .child("Ctrl+V 粘贴分享内容，或点击按钮读取剪贴板，通过 `ID:` 字段直接跳转。"),
        )
        .child(
            div()
                .h(px(36.))
                .rounded(px(12.))
                .bg(Hsla {
                    a: 0.06,
                    ..colors.text_secondary
                })
                .border_1()
                .border_color(Hsla {
                    a: 0.10,
                    ..colors.border
                })
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .child(themed_icon(
                    lucide_icons::icon_clipboard(),
                    16.0,
                    colors.text_secondary,
                ))
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_secondary)
                        .child("从剪贴板打开"),
                )
                .on_mouse_down(MouseButton::Left, |_ev, _window, cx| {
                    let text = cx
                        .read_from_clipboard()
                        .and_then(|item| item.text())
                        .unwrap_or_default();
                    share_actions::handle_curseforge_share_text(&text, cx);
                }),
        );

    let scroll_area = div()
        .id("curseforge-sidebar-scroll")
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .scrollbar_width(px(0.))
        .track_scroll(&state.curseforge_sidebar_scroll)
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(div().flex().flex_col().gap(px(2.)).child(content).when(
            active_root.is_some(),
            |this| {
                this.child(div().h(px(1.)).bg(Hsla {
                    a: 0.08,
                    ..colors.border
                }))
                .child(sub_header)
                .when(!state.curseforge_sub_collapsed, |inner| {
                    inner.child(sub_list)
                })
            },
        ))
        .child(share_card);

    div()
        .size_full()
        .rounded(px(8.))
        .border_1()
        .border_color(Hsla {
            a: 0.06,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.80,
            ..colors.surface
        })
        .p(px(10.))
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .text_size(px(11.))
                .font_weight(FontWeight::BOLD)
                .text_color(colors.text_secondary)
                .child("分类"),
        )
        .when_some(status, |this, status| this.child(status))
        .child(scroll_area)
}

fn render_curseforge_content(
    window: &mut Window,
    cx: &mut App,
    colors: &ThemeColors,
    curseforge_results_list: &Entity<CurseForgeResultsListView>,
    _now: Instant,
) -> Div {
    let state = cx.global::<DownloadPageState>();
    let skeleton_bar = |width: Pixels, height: Pixels| {
        div().w(width).h(height).rounded(px(999.)).bg(Hsla {
            a: 0.08,
            ..colors.text_secondary
        })
    };

    let skeleton_shimmer = || {
        div()
            .absolute()
            .top(px(0.))
            .bottom(px(0.))
            .w(px(140.))
            .bg(Hsla {
                a: 0.24,
                ..colors.surface
            })
            .with_animation(
                "curseforge-skeleton-shimmer",
                repeating_linear_motion(Duration::from_millis(1400)),
                |this, t| this.left(px(-180.0 + t * 440.0)),
            )
            .into_any_element()
    };

    let skeleton_card = || {
        div()
            .w_full()
            .rounded(px(8.))
            .bg(Hsla {
                a: 0.90,
                ..colors.surface
            })
            .border_1()
            .border_color(Hsla {
                a: 0.10,
                ..colors.border
            })
            .px(px(12.))
            .py(px(10.))
            .relative()
            .overflow_hidden()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(div().w(px(42.)).h(px(42.)).rounded(px(9.)).bg(Hsla {
                a: 0.10,
                ..colors.text_secondary
            }))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(skeleton_bar(px(250.), px(14.)))
                    .child(skeleton_bar(px(420.), px(10.)))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .min_w(px(0.))
                                    .flex_1()
                                    .overflow_hidden()
                                    .child(skeleton_bar(px(112.), px(10.)))
                                    .child(skeleton_bar(px(84.), px(18.)))
                                    .child(skeleton_bar(px(76.), px(10.))),
                            )
                            .child(skeleton_bar(px(90.), px(10.))),
                    ),
            )
            .child(div().w(px(92.)).h(px(32.)).rounded(px(10.)).bg(Hsla {
                a: 0.10,
                ..colors.accent
            }))
            .child(skeleton_shimmer())
    };

    let shell = |child: Div| {
        child
            .size_full()
            .rounded(px(12.))
            .border_1()
            .border_color(Hsla {
                a: 0.06,
                ..colors.border
            })
            .bg(Hsla {
                a: 0.85,
                ..colors.surface
            })
            .overflow_hidden()
            .min_w(px(0.))
            .min_h(px(0.))
    };

    let show_initial_loading = !state.curseforge_loaded
        && state.curseforge_error.is_none()
        && state.curseforge_mods.is_empty();

    let root_name = state
        .curseforge_selected_root_id
        .and_then(|id| state.curseforge_categories.iter().find(|c| c.id == id))
        .map(|c| c.name.clone())
        .unwrap_or_else(|| SharedString::from("全部"));

    let sub_name = state
        .curseforge_selected_sub_id
        .and_then(|id| state.curseforge_categories.iter().find(|c| c.id == id))
        .map(|c| c.name.clone());

    let version_label = if state.curseforge_selected_game_version.trim().is_empty() {
        SharedString::from("全部版本")
    } else {
        state.curseforge_selected_game_version.clone()
    };

    let (sort_label, sort_is_default) = match state.curseforge_sort_field {
        2 => (SharedString::from("热门"), false),
        3 => (SharedString::from("更新"), false),
        4 => (SharedString::from("名称"), false),
        6 => (SharedString::from("下载"), false),
        _ => (SharedString::from("精选"), true),
    };

    let search = state.search_query.trim();

    let status_chip = div()
        .px(px(12.))
        .h(px(28.))
        .rounded(px(999.))
        .bg(Hsla {
            a: 0.15,
            ..colors.accent
        })
        .border_1()
        .border_color(Hsla {
            a: 0.35,
            ..colors.accent
        })
        .flex()
        .items_center()
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.accent)
                .child("已更新"),
        );

    let cf_chip = |label: SharedString,
                   clickable: bool,
                   on_click: Option<Box<dyn Fn(&mut Window, &mut App)>>| {
        div()
            .h(px(24.))
            .px(px(10.))
            .rounded(px(999.))
            .border_1()
            .border_color(Hsla {
                a: 0.08,
                ..colors.border
            })
            .bg(Hsla {
                a: 0.70,
                ..colors.surface
            })
            .flex()
            .items_center()
            .text_size(px(12.))
            .font_weight(FontWeight::BOLD)
            .text_color(colors.text_secondary)
            .when(clickable, |this| this.cursor_pointer())
            .when_some(on_click, |this, on_click| {
                this.on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                    on_click(window, cx);
                })
            })
            .child(label)
    };

    let topbar = div()
        .flex_none()
        .m(px(12.))
        .rounded(px(8.))
        .bg(Hsla {
            a: 0.78,
            ..colors.surface
        })
        .border_1()
        .border_color(Hsla {
            a: 0.06,
            ..colors.border
        })
        .px(px(12.))
        .py(px(10.))
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .min_w(px(0.))
                .flex_1()
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .min_w(px(0.))
                        .overflow_hidden()
                        .child(
                            div()
                                .min_w(px(0.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(16.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.text_primary)
                                .cursor_pointer()
                                .child(root_name.clone())
                                .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                    cx.update_global(|state: &mut DownloadPageState, cx| {
                                        if state.curseforge_selected_root_id.is_none() {
                                            return;
                                        }
                                        state.curseforge_selected_sub_id = None;
                                        state.curseforge_page_index = 0;
                                        state.curseforge_disable_result_logos = true;
                                        invalidate_results_now_in_state(state, cx);
                                    });
                                    ensure_results_loaded(false, cx);
                                }),
                        )
                        .when_some(sub_name.clone(), |this, sub_name| {
                            this.child(
                                div()
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(colors.text_muted)
                                    .child("/"),
                            )
                            .child(
                                div()
                                    .min_w(px(0.))
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(colors.text_primary)
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(sub_name),
                            )
                        }),
                )
                .child(cf_chip(
                    version_label.clone(),
                    !state.curseforge_selected_game_version.trim().is_empty(),
                    Some(Box::new(move |_window, cx| {
                        cx.update_global(|state: &mut DownloadPageState, cx| {
                            if state.curseforge_selected_game_version.trim().is_empty() {
                                return;
                            }
                            state.curseforge_selected_game_version = SharedString::from("");
                            state.curseforge_page_index = 0;
                            state.curseforge_disable_result_logos = true;
                            invalidate_results_now_in_state(state, cx);
                        });
                        ensure_results_loaded(false, cx);
                    })),
                ))
                .child(cf_chip(
                    sort_label.clone(),
                    !sort_is_default,
                    Some(Box::new(move |_window, cx| {
                        cx.update_global(|state: &mut DownloadPageState, cx| {
                            if state.curseforge_sort_field == 1 {
                                return;
                            }
                            state.curseforge_sort_field = 1;
                            state.curseforge_page_index = 0;
                            state.curseforge_disable_result_logos = true;
                            invalidate_results_now_in_state(state, cx);
                        });
                        ensure_results_loaded(false, cx);
                    })),
                ))
                .when(!search.is_empty(), |this| {
                    let input = state.search_input.clone();
                    let clipped = truncate_with_ellipsis(search, 16);
                    let label = SharedString::from(format!("“{}”", clipped));
                    this.child(
                        div()
                            .h(px(24.))
                            .px(px(10.))
                            .rounded(px(999.))
                            .border_1()
                            .border_color(Hsla {
                                a: 0.12,
                                ..colors.accent
                            })
                            .bg(Hsla {
                                a: 0.10,
                                ..colors.accent
                            })
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .max_w(px(180.))
                            .min_w(px(0.))
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                                cx.update_global(|state: &mut DownloadPageState, cx| {
                                    state.search_query = SharedString::from("");
                                    state.curseforge_page_index = 0;
                                    state.curseforge_disable_result_logos = true;
                                    invalidate_results_now_in_state(state, cx);
                                });
                                ensure_results_loaded(false, cx);
                                if let Some(input) = input.clone() {
                                    let _ = input.update(cx, |st, cx| {
                                        st.set_value(SharedString::from(""), window, cx);
                                    });
                                }
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(colors.accent)
                                    .child(label),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(colors.accent)
                                    .child("×"),
                            ),
                    )
                }),
        )
        .child(status_chip);

    let list = div()
        .w_full()
        .flex_1()
        .min_h(px(0.))
        .overflow_hidden()
        .child(if show_initial_loading {
            render_curseforge_results_list_placeholder_aligned(colors, state).into_any_element()
        } else {
            curseforge_results_list.clone().into_any_element()
        });

    let page_size = state.curseforge_page_size.max(1) as usize;
    let total_items = state
        .curseforge_total_count
        .map(|v| v as usize)
        .unwrap_or_else(|| state.curseforge_mods.len());
    let total_pages = state
        .curseforge_total_count
        .map(|tot| ((tot as usize) + page_size - 1) / page_size)
        .unwrap_or_else(|| {
            if state.curseforge_has_more {
                state.curseforge_page_index + 2
            } else {
                state.curseforge_page_index + 1
            }
        });

    let footer = div()
        .flex_none()
        .px(px(16.))
        .py(px(12.))
        .bg(Hsla {
            a: 0.30,
            ..colors.surface
        })
        .child(render_curseforge_pager(window, cx, colors));

    shell(div())
        .flex()
        .flex_col()
        .overflow_hidden()
        .min_h(px(0.))
        .child(topbar)
        .child(list)
        .child(footer)
}

fn render_curseforge_results_list(
    this: &mut CurseForgeResultsListView,
    colors: &ThemeColors,
    window: &mut Window,
    cx: &mut Context<CurseForgeResultsListView>,
) -> Div {
    let (
        results_loading,
        results_error,
        disable_result_logos,
        results_epoch,
        results_transition_at,
        pending_page_index,
        page_index,
        mod_count,
    ) = cx.read_global(|state: &DownloadPageState, _cx| {
        (
            state.curseforge_results_loading,
            state.curseforge_results_error.clone(),
            state.curseforge_disable_result_logos,
            state.curseforge_results_epoch,
            state.curseforge_results_transition_at,
            state.curseforge_pending_page_index,
            state.curseforge_page_index,
            state.curseforge_mods.len(),
        )
    });

    let results_signature = (results_epoch, page_index, mod_count, 0, mod_count);
    if this.last_prepared_results_signature != results_signature {
        let started_at = std::time::Instant::now();
        let next_page_card_props = cx.read_global(|state: &DownloadPageState, _cx| {
            build_curseforge_result_card_props(state, 0, mod_count)
        });
        this.cached_page_card_props = next_page_card_props;
        let elapsed_ms = started_at.elapsed().as_secs_f64() * 1000.0;
        if elapsed_ms >= 8.0 {
            tracing::debug!(
                "curseforge results prepare slow: elapsed_ms={elapsed_ms:.3} results_epoch={} page_index={} mod_count={} cached_cards={}",
                results_epoch,
                page_index,
                mod_count,
                this.cached_page_card_props.len(),
            );
        }
        this.last_prepared_results_signature = results_signature;
    }

    let has_cached_cards = !this.cached_page_card_props.is_empty();

    let virtual_list_plan = cx.read_global(|state: &DownloadPageState, _cx| {
        crate::ui::components::virtual_list::compute_virtual_list_plan(
            this.cached_page_card_props.len(),
            CURSEFORGE_RESULT_CARD_PITCH_PX,
            state.curseforge_results_scroll.offset().y,
            state.curseforge_results_scroll.bounds().size.height,
            CURSEFORGE_RESULT_CARD_OVERSCAN,
            CURSEFORGE_RESULT_LOGO_RENDER_BUDGET,
        )
    });
    let logo_cache_items = virtual_list_plan
        .heavy_slice
        .len()
        .saturating_add(CURSEFORGE_RESULT_LOGO_RENDER_BUDGET)
        .clamp(4, this.cached_page_card_props.len().max(4));
    this.result_logo_cache.update(cx, |cache, cx| {
        cache.set_config(
            BoundedImageCacheConfig {
                max_items: logo_cache_items,
                max_bytes: logo_cache_items.saturating_mul(results::RESULT_LOGO_BYTES_PER_ITEM),
            },
            window,
            cx,
        );
    });

    let list = {
        let results_scroll = cx
            .global::<DownloadPageState>()
            .curseforge_results_scroll
            .clone();
        div()
            .size_full()
            .min_h(px(0.))
            .id("curseforge-results-scroll")
            .overflow_y_scroll()
            .track_scroll(&cx.global::<DownloadPageState>().curseforge_results_scroll)
            .scrollbar_width(px(0.))
            .on_scroll_wheel(move |event, window, cx| {
                let offset = results_scroll.offset();
                let max_offset = results_scroll.max_offset();
                let delta_y = scroll_event_delta_y_with_line_height(event, window.line_height());
                let at_bottom = offset.y <= -max_offset.height;
                let at_top = offset.y >= px(0.);

                if (at_bottom && delta_y < Pixels::ZERO) || (at_top && delta_y > Pixels::ZERO) {
                    results_scroll
                        .set_offset(point(offset.x, offset.y.clamp(-max_offset.height, px(0.))));
                    window.prevent_default();
                    cx.stop_propagation();
                }
            })
            .px(px(12.))
            .py(px(12.))
            .flex()
            .flex_col()
    };

    let show_loading_overlay =
        pending_page_index.is_some() || (results_loading && has_cached_cards);

    if results_loading && !has_cached_cards {
        let state = cx.global::<DownloadPageState>();
        return render_curseforge_results_list_placeholder_aligned(colors, state);
    }

    if let Some(err) = results_error.as_ref() {
        return div().size_full().child(list.child(status_card(
            colors,
            &format!("加载失败: {err}"),
            Some(colors.danger),
        )));
    }

    if !has_cached_cards {
        return div().size_full().child(list.child(status_card(
            colors,
            "没有找到匹配的资源",
            None,
        )));
    }

    let animate_cards = should_animate_curseforge_result_cards();

    let reveal_warmup_pending = if !animate_cards || results_loading {
        false
    } else if let Some(started_at) = results_transition_at {
        let elapsed_ms = std::time::Instant::now()
            .saturating_duration_since(started_at)
            .as_millis() as u64;
        elapsed_ms < CURSEFORGE_RESULTS_REVEAL_WARMUP_MS
    } else {
        false
    };

    if reveal_warmup_pending {
        let warmup_deadline = results_transition_at.map(|started_at| {
            started_at + Duration::from_millis(CURSEFORGE_RESULTS_REVEAL_WARMUP_MS)
        });
        crate::ui::animation::request_animation_frame_until_active(window, warmup_deadline);
        let state = cx.global::<DownloadPageState>();
        return render_curseforge_results_list_placeholder_aligned(colors, state);
    }

    let render_started_at = std::time::Instant::now();
    let mut visible_card_items = div().w_full().flex().flex_col().gap(px(6.));
    let default_install_target = default_install_target_for_results(cx);

    let transition_started_at = results_transition_at;
    let transition_now = std::time::Instant::now();
    for (visible_index, cached_card_props) in this
        .cached_page_card_props
        .iter()
        .enumerate()
        .skip(virtual_list_plan.render_slice.start_index)
        .take(virtual_list_plan.render_slice.visible_len())
    {
        let is_heavy_card = virtual_list_plan.heavy_slice.contains(visible_index);
        visible_card_items = visible_card_items.child(render_curseforge_result_card(
            colors,
            cached_card_props,
            &this.result_logo_cache,
            default_install_target.clone(),
            is_heavy_card,
            transition_started_at,
            transition_now,
            visible_index,
        ));
    }

    let transition_animating = animate_cards
        && !results_loading
        && results_transition_at.is_some_and(|started_at| {
            let visible_count = this.cached_page_card_props.len() as u64;
            let total_duration_ms = CURSEFORGE_RESULT_CARD_ANIMATION_MS
                + visible_count.saturating_sub(1) * CURSEFORGE_RESULT_CARD_STAGGER_MS;
            (transition_now
                .saturating_duration_since(started_at)
                .as_millis() as u64)
                < total_duration_ms.max(CURSEFORGE_RESULTS_TRANSITION_MS)
        });
    crate::ui::animation::request_animation_frame_if(window, transition_animating);

    let content = list.child(
        div()
            .w_full()
            .flex()
            .flex_col()
            .child(div().h(virtual_list_plan.render_slice.top_spacer))
            .child(visible_card_items)
            .child(div().h(virtual_list_plan.render_slice.bottom_spacer)),
    );

    let content = if show_loading_overlay {
        let state = cx.global::<DownloadPageState>();
        render_curseforge_results_list_placeholder_aligned(colors, state)
    } else {
        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .child(content)
    };

    let render_elapsed_ms = render_started_at.elapsed().as_secs_f64() * 1000.0;
    if render_elapsed_ms >= 8.0 {
        tracing::debug!(
            "curseforge results render slow: elapsed_ms={render_elapsed_ms:.3} page_index={} cached_cards={} render_start={} render_len={} visible_start={} visible_len={} heavy_start={} heavy_len={} loading_overlay={} disable_logos={}",
            page_index,
            this.cached_page_card_props.len(),
            virtual_list_plan.render_slice.start_index,
            virtual_list_plan.render_slice.visible_len(),
            virtual_list_plan.visible_slice.start_index,
            virtual_list_plan.visible_slice.len(),
            virtual_list_plan.heavy_slice.start_index,
            virtual_list_plan.heavy_slice.len(),
            show_loading_overlay,
            disable_result_logos
        );
    }

    content
}

fn default_install_target_for_results(cx: &App) -> Option<SharedString> {
    let selected_folder = cx.read_global(
        |state: &crate::ui::views::manage::state::ManagePageState, _cx| {
            state.selected_folder.clone()
        },
    );
    let local_versions = read_local_versions_snapshot(cx);
    modals::default_install_target(selected_folder, &local_versions)
}

fn render_curseforge_result_card(
    colors: &ThemeColors,
    props: &CurseForgeResultCardProps,
    result_logo_cache: &Entity<BoundedImageCache>,
    default_install_target: Option<SharedString>,
    is_heavy_card: bool,
    transition_started_at: Option<Instant>,
    now: Instant,
    visible_index: usize,
) -> AnyElement {
    let colors = *colors;
    let dark_mode = colors.bg.l < 0.5;
    let card_bg = if dark_mode {
        Hsla {
            a: 0.80,
            ..colors.surface
        }
    } else {
        Hsla {
            a: 0.95,
            ..colors.surface
        }
    };
    let card_hover_bg = if dark_mode {
        Hsla {
            a: 0.95,
            ..colors.surface
        }
    } else {
        Hsla {
            a: 1.0,
            ..colors.surface
        }
    };

    let result_element_id = u64::try_from(props.mod_id).ok().unwrap_or_default();
    let reveal_progress = if should_animate_curseforge_result_cards() {
        transition_started_at.map_or(1.0, |started_at| {
            let stagger_ms = visible_index as u64 * CURSEFORGE_RESULT_CARD_STAGGER_MS;
            let elapsed_ms = now.saturating_duration_since(started_at).as_millis() as u64;
            let local_elapsed_ms = elapsed_ms.saturating_sub(stagger_ms);
            let linear = (local_elapsed_ms as f32 / CURSEFORGE_RESULT_CARD_ANIMATION_MS as f32)
                .clamp(0.0, 1.0);
            crate::ui::animation::ease_out_cubic(linear)
        })
    } else {
        1.0
    };
    let reveal_opacity = (0.25 + reveal_progress * 0.75).clamp(0.0, 1.0);
    let reveal_translate_y = px((1.0 - reveal_progress) * 10.0);
    let primary_tag = props.primary_tag_label.clone().map(|primary_tag_label| {
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .flex_none()
            .min_w(px(0.))
            .max_w(px(132.))
            .overflow_hidden()
            .child(
                div()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(primary_tag_label),
            )
    });

    div()
        .id(("curseforge-result-card", result_element_id))
        .w_full()
        .min_w(px(0.))
        .h(px(78.))
        .rounded(px(8.))
        .hover(move |s| s.bg(card_hover_bg))
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, {
            let mod_id = props.mod_id;
            move |_ev, _window, cx| {
                modals::open_curseforge_mod_page(mod_id, cx);
            }
        })
        .opacity(reveal_opacity)
        .relative()
        .top(reveal_translate_y)
        .px(px(12.))
        .py(px(9.))
        .flex()
        .items_center()
        .gap(px(10.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .w(px(0.))
                .min_w(px(0.))
                .flex_1()
                .overflow_hidden()
                .child(div().flex_none().flex_shrink_0().child({
                    if is_heavy_card
                        && should_render_curseforge_result_images()
                        && should_mount_curseforge_result_images()
                    {
                        if let Some(logo_url) = props.logo_url.clone() {
                            div()
                                .id(("curseforge-result-logo", result_element_id))
                                .w(px(42.))
                                .h(px(42.))
                                .rounded(px(9.))
                                .overflow_hidden()
                                .bg(Hsla {
                                    a: 0.10,
                                    ..colors.surface
                                })
                                .child(
                                    img(logo_url)
                                        .image_cache(result_logo_cache)
                                        .w_full()
                                        .h_full()
                                        .rounded(px(9.))
                                        .bg(gpui::transparent_black())
                                        .object_fit(ObjectFit::Cover)
                                        .with_loading({
                                            let colors = colors;
                                            move || render_result_logo_placeholder(colors)
                                        })
                                        .with_fallback({
                                            let colors = colors;
                                            move || render_result_logo_placeholder(colors)
                                        })
                                        .into_any_element(),
                                )
                                .into_any_element()
                        } else {
                            render_result_logo_placeholder(colors)
                        }
                    } else {
                        render_result_logo_placeholder(colors)
                    }
                }))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .w(px(0.))
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .child(
                            div()
                                .w_full()
                                .text_size(px(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text_primary)
                                .min_w(px(0.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .line_height(relative(1.35))
                                .child(props.title.clone()),
                        )
                        .child(
                            div()
                                .w_full()
                                .text_size(px(11.))
                                .text_color(colors.text_secondary)
                                .min_w(px(0.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .line_height(relative(1.35))
                                .child(props.summary.clone()),
                        )
                        .when(is_heavy_card, |this| {
                            this.child({
                                let meta_icon = |icon: &'static str| {
                                    themed_icon(icon, 12.0, colors.text_muted).into_any_element()
                                };

                                let author_item = div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .flex_none()
                                    .min_w(px(0.))
                                    .max_w(px(180.))
                                    .overflow_hidden()
                                    .child(meta_icon(lucide_icons::icon_user()))
                                    .child(
                                        div()
                                            .min_w(px(0.))
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(props.authors.clone()),
                                    );

                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .w_full()
                                    .justify_start()
                                    .text_size(px(10.))
                                    .text_color(colors.text_muted)
                                    .min_w(px(0.))
                                    .overflow_hidden()
                                    .child(author_item)
                                    .when_some(primary_tag, |this, primary_tag| {
                                        this.child(primary_tag).child(
                                            div()
                                                .flex_none()
                                                .text_color(colors.text_muted)
                                                .opacity(0.35)
                                                .child("|"),
                                        )
                                    })
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(4.))
                                            .flex_none()
                                            .child(meta_icon(lucide_icons::icon_download()))
                                            .child(props.download_count_label.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(4.))
                                            .flex_none()
                                            .child(meta_icon(lucide_icons::icon_calendar()))
                                            .child(props.date_modified_label.clone()),
                                    )
                            })
                        }),
                ),
        )
        .child(
            div()
                .w(px(92.))
                .flex_none()
                .flex_shrink_0()
                .h(px(30.))
                .rounded(px(6.))
                .bg(Hsla {
                    a: 0.18,
                    ..colors.accent
                })
                .flex()
                .items_center()
                .justify_center()
                .text_color(colors.accent)
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, {
                    let mod_id = props.mod_id;
                    let default_install_target = default_install_target.clone();
                    move |_ev, _window, cx| {
                        cx.stop_propagation();
                        modals::open_curseforge_install_modal_for_mod_id(
                            mod_id,
                            default_install_target.clone(),
                            cx,
                        );
                    }
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .child("安装"),
                ),
        )
        .into_any_element()
}

fn render_curseforge_pager(window: &mut Window, cx: &mut App, colors: &ThemeColors) -> Div {
    let state = cx.global::<DownloadPageState>();

    let page_index = state.curseforge_page_index;
    let results_loading = state.curseforge_results_loading
        || state.curseforge_page_commit_task.is_some()
        || state.curseforge_pending_page_index.is_some();
    let showing = state.curseforge_mods.len();

    let page_size = state.curseforge_page_size.max(1) as usize;
    let total = state
        .curseforge_total_count
        .map(|v| v as usize)
        .unwrap_or_else(|| state.curseforge_mods.len());

    let total_pages = state
        .curseforge_total_count
        .map(|tot| ((tot as usize) + page_size - 1) / page_size)
        .unwrap_or_else(|| {
            if state.curseforge_has_more {
                state.curseforge_page_index + 2
            } else {
                state.curseforge_page_index + 1
            }
        });

    let page_jump_input = state.page_jump_input.clone();
    let page_index = page_index.min(total_pages.saturating_sub(1));

    if total_pages <= 1 {
        return div().w_full().h(px(0.));
    }

    let total_pages = total_pages.max(1);
    if let Some(input) = &page_jump_input {
        let placeholder = format!("{}/{}", page_index + 1, total_pages);
        let _ = input.update(cx, |st, cx| {
            st.set_placeholder(SharedString::from(placeholder), window, cx);
        });
    }

    let prev_enabled = page_index > 0;
    let next_enabled = page_index + 1 < total_pages;

    let request_page_change = move |target_page: usize, cx: &mut App| {
        let target_page = target_page.min(total_pages.saturating_sub(1));
        let should_load = cx.update_global(|state: &mut DownloadPageState, cx| {
            if state.curseforge_results_loading {
                return false;
            }

            let current_page = state
                .curseforge_page_index
                .min(total_pages.saturating_sub(1));
            if current_page == target_page {
                state.curseforge_page_commit_task.take();
                state.curseforge_pending_page_index = None;
                return false;
            }

            state.curseforge_page_commit_task.take();
            state.curseforge_pending_page_index = None;
            state.curseforge_page_index = target_page;
            begin_page_results_transition_in_state(state, cx);
            true
        });

        match should_load {
            true => {
                ensure_results_loaded_after_page_transition(false, target_page, cx);
            }
            false => {}
        }
    };

    let nav_btn = |icon: &'static str,
                   enabled: bool,
                   on_click_cb: Box<dyn Fn(&mut DownloadPageState)>,
                   _source: &'static str| {
        let enabled = enabled && !results_loading;
        div()
            .min_w(px(32.))
            .h(px(32.))
            .rounded(px(6.))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .text_color(if enabled {
                colors.text_primary
            } else {
                colors.text_muted
            })
            .when(!enabled, |this| this.opacity(0.35))
            .child(themed_icon(
                icon,
                16.0,
                if enabled {
                    colors.text_primary
                } else {
                    colors.text_muted
                },
            ))
            .hover(move |s| {
                if enabled {
                    s.bg(Hsla {
                        a: if colors.bg.l < 0.5 { 0.12 } else { 0.08 },
                        ..colors.text_primary
                    })
                } else {
                    s
                }
            })
            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                if !enabled {
                    return;
                }
                cx.update_global(|s: &mut DownloadPageState, _cx| {
                    on_click_cb(s);
                });
                let target_page =
                    cx.read_global(|state: &DownloadPageState, _cx| state.curseforge_page_index);
                request_page_change(target_page, cx);
            })
    };

    let page_btn = |label: SharedString, active: bool, page: usize| {
        div()
            .min_w(px(32.))
            .h(px(32.))
            .rounded(px(6.))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if active {
                colors.accent
            } else {
                Hsla {
                    a: 0.0,
                    ..colors.surface
                }
            })
            .text_color(if active {
                colors.btn_primary_text
            } else {
                colors.text_secondary
            })
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(label),
            )
            .hover(move |s| {
                if active {
                    s
                } else {
                    s.bg(Hsla {
                        a: if colors.bg.l < 0.5 { 0.12 } else { 0.08 },
                        ..colors.text_primary
                    })
                }
            })
            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                request_page_change(page, cx);
            })
    };

    let mut pages: Vec<Option<usize>> = Vec::new();
    if total_pages <= 7 {
        for p in 0..total_pages {
            pages.push(Some(p));
        }
    } else {
        let last = total_pages - 1;
        pages.push(Some(0));
        if page_index.saturating_sub(1) > 1 {
            pages.push(None);
        }
        for p in page_index.saturating_sub(1)..=(page_index + 1).min(last) {
            if p != 0 && p != last {
                pages.push(Some(p));
            }
        }
        if page_index + 2 < last {
            pages.push(None);
        }
        pages.push(Some(last));
    }

    let mut page_row = div().flex().items_center().gap(px(8.));
    for p in pages {
        match p {
            Some(p) => {
                page_row = page_row.child(page_btn(
                    SharedString::from((p + 1).to_string()),
                    p == page_index,
                    p,
                ));
            }
            None => {
                page_row = page_row.child(
                    div()
                        .px(px(6.))
                        .text_size(px(12.))
                        .text_color(colors.text_muted)
                        .child("..."),
                );
            }
        }
    }

    let jump = page_jump_input.map(|input| {
        let input_entity = input.clone();
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(colors.text_muted)
                    .child("跳转至"),
            )
            .child(
                Input::new(&input_entity)
                    .w(px(56.))
                    .px(px(4.))
                    .with_size(crate::ui::components::input::InputSize::Small),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(colors.text_muted)
                    .child("页"),
            )
    });

    div()
        .w_full()
        .flex()
        .items_center()
        .child(
            div()
                .flex_1()
                .flex()
                .justify_start()
                .text_size(px(12.))
                .text_color(colors.text_muted)
                .child(format!("结果: {showing} / {total}")),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(nav_btn(
                    lucide_icons::icon_chevron_left(),
                    prev_enabled,
                    Box::new(|s| {
                        s.curseforge_page_index = s.curseforge_page_index.saturating_sub(1)
                    }),
                    "prev",
                ))
                .child(page_row)
                .child(nav_btn(
                    lucide_icons::icon_chevron_right(),
                    next_enabled,
                    Box::new(|s| {
                        s.curseforge_page_index = s.curseforge_page_index.saturating_add(1)
                    }),
                    "next",
                )),
        )
        .child(
            div().flex_1().flex().justify_end().child(
                jump.map(IntoElement::into_any_element)
                    .unwrap_or_else(|| div().into_any_element()),
            ),
        )
}

fn close_curseforge_install_modal_from_ui(cx: &mut App) {
    let task_id = cx.read_global(|state: &DownloadPageState, _cx| {
        if matches!(
            state.curseforge_install_stage,
            crate::ui::views::download::state::CurseForgeInstallStage::Downloading
        ) {
            state.curseforge_install_task_id.clone()
        } else {
            None
        }
    });

    if let Some(task_id) = task_id {
        crate::tasks::task_manager::cancel_task(task_id.as_ref());
    }

    modals::close_curseforge_install_modal(cx);
}

fn render_curseforge_install_close_button(colors: &ThemeColors) -> Div {
    div()
        .size(px(34.))
        .rounded(px(10.))
        .bg(Hsla {
            a: 0.70,
            ..colors.surface
        })
        .border_1()
        .border_color(Hsla {
            a: 0.10,
            ..colors.border
        })
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .child(themed_icon(
            lucide_icons::icon_x(),
            16.0,
            colors.text_secondary,
        ))
        .hover(|style| {
            style.bg(Hsla {
                a: 0.95,
                ..colors.surface_hover
            })
        })
        .on_mouse_down(MouseButton::Left, |_ev, _window, cx| {
            close_curseforge_install_modal_from_ui(cx);
        })
}

fn render_curseforge_install_header(
    colors: &ThemeColors,
    kicker: &'static str,
    title: SharedString,
    subtitle: Option<SharedString>,
) -> Div {
    div()
        .px(px(18.))
        .py(px(16.))
        .border_b_1()
        .border_color(Hsla {
            a: 0.10,
            ..colors.border
        })
        .flex()
        .items_start()
        .justify_between()
        .gap(px(16.))
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(12.))
                .min_w(px(0.))
                .child(
                    div()
                        .size(px(48.))
                        .flex_none()
                        .rounded(px(12.))
                        .bg(Hsla {
                            a: 0.10,
                            ..colors.accent
                        })
                        .border_1()
                        .border_color(Hsla {
                            a: 0.10,
                            ..colors.border
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(themed_icon(
                            lucide_icons::icon_package(),
                            20.0,
                            colors.accent,
                        )),
                )
                .child(
                    div()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.text_muted)
                                .child(kicker),
                        )
                        .child(
                            div()
                                .text_size(px(17.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.text_primary)
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(title),
                        )
                        .when_some(subtitle, |this, subtitle| {
                            this.child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(colors.text_muted)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(subtitle),
                            )
                        }),
                ),
        )
        .child(render_curseforge_install_close_button(colors))
}

fn launch_version_dropdown_label(
    version: &crate::core::version::launch_versions::LaunchVersionEntry,
) -> SharedString {
    crate::ui::hooks::use_local_versions::launch_version_dropdown_label(version)
}

fn install_target_label(folder: &str, local_versions: &LocalVersionsSnapshot) -> SharedString {
    local_versions
        .versions
        .iter()
        .find(|version| version.folder.as_ref() == folder)
        .map(launch_version_dropdown_label)
        .unwrap_or_else(|| SharedString::from(folder.to_string()))
}

fn render_install_target_dropdown(
    colors: &ThemeColors,
    state: &DownloadPageState,
    local_versions: &LocalVersionsSnapshot,
    enabled: bool,
) -> Dropdown {
    let has_versions = !local_versions.versions.is_empty();
    let options = if has_versions {
        local_versions
            .versions
            .iter()
            .map(launch_version_dropdown_label)
            .map(DropdownOption::from)
            .collect::<Vec<_>>()
    } else {
        vec![DropdownOption::from(SharedString::from("暂无可用版本"))]
    };

    let selected_index = state
        .curseforge_install_target_folder
        .as_ref()
        .and_then(|selected| {
            local_versions
                .versions
                .iter()
                .position(|version| version.folder.as_ref() == selected.as_ref())
        })
        .unwrap_or(0);

    let label = state
        .curseforge_install_target_folder
        .as_ref()
        .map(|folder| install_target_label(folder.as_ref(), local_versions))
        .unwrap_or_else(|| SharedString::from("选择版本"));
    let label = if has_versions {
        label
    } else {
        SharedString::from("暂无可用版本")
    };

    let folders = local_versions
        .versions
        .iter()
        .map(|version| SharedString::from(version.folder.to_string()))
        .collect::<Vec<_>>();

    Dropdown::with_trigger(
        SharedString::from("curseforge-install-target-dropdown"),
        colors,
        px(656.),
        px(54.),
        label,
        options,
        selected_index,
        enabled && has_versions,
        |colors, _width, _height, enabled, open_k, label| {
            let chevron = svg()
                .path(lucide_icons::icon_chevron_down())
                .w(px(18.))
                .h(px(18.))
                .text_color(colors.text_secondary)
                .opacity(if enabled { 0.78 } else { 0.36 })
                .with_transformation(Transformation::rotate(radians(
                    open_k * std::f32::consts::PI,
                )));

            div()
                .size_full()
                .px(px(14.))
                .py(px(9.))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .child(
                    div()
                        .min_w(px(0.))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(themed_icon(
                            lucide_icons::icon_folder_open(),
                            16.0,
                            if enabled {
                                colors.accent
                            } else {
                                colors.text_muted
                            },
                        ))
                        .child(
                            div()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .text_size(px(13.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.text_primary)
                                        .child(label.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(colors.text_secondary)
                                        .child(SharedString::from("选择安装目标版本")),
                                ),
                        ),
                )
                .child(chevron)
                .into_any_element()
        },
        move |index, _window, cx| {
            let Some(folder) = folders.get(index).cloned() else {
                return;
            };
            cx.update_global(|state: &mut DownloadPageState, _cx| {
                state.curseforge_install_target_folder = Some(folder);
            });
        },
    )
    .rounded(px(10.))
}

fn render_curseforge_install_file_option(
    colors: &ThemeColors,
    file: &crate::ui::views::download::state::CurseForgeFileEntry,
    default_target: Option<SharedString>,
) -> Div {
    let file_id = file.id;
    let disabled = file.download_url.is_none();
    let version_label = curseforge_file_version_label(file);
    let date_label = format_date_ymd(file.file_date.as_ref());
    let action_label = if disabled {
        "无下载地址"
    } else {
        "安装"
    };

    div()
        .w_full()
        .min_h(px(60.))
        .rounded(px(10.))
        .bg(Hsla {
            a: 0.52,
            ..colors.surface
        })
        .border_1()
        .border_color(Hsla {
            a: 0.10,
            ..colors.border
        })
        .px(px(12.))
        .py(px(10.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(14.))
        .opacity(if disabled { 0.64 } else { 1.0 })
        .hover(|style| {
            style.border_color(Hsla {
                a: 0.42,
                ..colors.accent
            })
        })
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(5.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .line_height(relative(1.25))
                        .child(file.display_name.clone()),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(colors.text_muted)
                        .child(date_label),
                ),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(14.))
                .child(
                    div()
                        .h(px(26.))
                        .px(px(9.))
                        .rounded(px(7.))
                        .bg(Hsla {
                            a: 0.10,
                            ..colors.accent
                        })
                        .flex()
                        .items_center()
                        .text_size(px(12.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.accent)
                        .child(version_label),
                )
                .child(
                    div()
                        .w(px(72.))
                        .text_size(px(12.))
                        .text_color(colors.text_secondary)
                        .child(format_bytes(file.file_length)),
                )
                .child(
                    div()
                        .h(px(36.))
                        .px(px(13.))
                        .rounded(px(9.))
                        .bg(if disabled {
                            colors.surface_hover
                        } else {
                            colors.accent
                        })
                        .border_1()
                        .border_color(Hsla {
                            a: 0.10,
                            ..colors.border
                        })
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        .text_size(px(12.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(if disabled {
                            colors.text_muted
                        } else {
                            colors.btn_primary_text
                        })
                        .child(themed_icon(
                            lucide_icons::icon_download(),
                            15.0,
                            if disabled {
                                colors.text_muted
                            } else {
                                colors.btn_primary_text
                            },
                        ))
                        .child(action_label)
                        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                            if disabled {
                                return;
                            }
                            let default_target = default_target.clone();
                            cx.update_global(|state: &mut DownloadPageState, _cx| {
                                state.curseforge_install_selected_file_id = Some(file_id);
                                state.curseforge_install_stage =
                                    crate::ui::views::download::state::CurseForgeInstallStage::Idle;
                                state.curseforge_install_error = None;
                                state.curseforge_install_task_id = None;
                                state.curseforge_install_downloaded_path = None;
                                state.curseforge_install_conflict_message = None;
                                if default_target.is_some() {
                                    state.curseforge_install_target_folder = default_target;
                                }
                            });
                        }),
                ),
        )
}

fn render_curseforge_install_file_selection_modal(
    colors: &ThemeColors,
    state: &DownloadPageState,
    selected_folder: Option<SharedString>,
    local_versions: &LocalVersionsSnapshot,
) -> AnyElement {
    let mod_name = state
        .curseforge_install_mod
        .as_ref()
        .map(|mod_entry| mod_entry.name.clone())
        .unwrap_or_else(|| SharedString::from("CurseForge 安装"));
    let files_loading = matches!(
        state.curseforge_install_stage,
        crate::ui::views::download::state::CurseForgeInstallStage::LoadingFiles
    );
    let files_error = if state.curseforge_install_files.is_empty() {
        state.curseforge_install_error.clone()
    } else {
        None
    };

    let mut files_list = div().flex().flex_col().gap(px(8.));
    for file in &state.curseforge_install_files {
        let default_target =
            modals::default_install_target_for_file(file, selected_folder.clone(), local_versions);
        files_list = files_list.child(render_curseforge_install_file_option(
            colors,
            file,
            default_target,
        ));
    }

    let body = div()
        .flex_1()
        .min_h(px(0.))
        .px(px(16.))
        .py(px(14.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.text_muted)
                        .child("选择要下载的资源文件版本"),
                )
                .child(
                    div()
                        .h(px(28.))
                        .px(px(10.))
                        .rounded(px(999.))
                        .bg(Hsla {
                            a: 0.10,
                            ..colors.accent
                        })
                        .flex()
                        .items_center()
                        .text_size(px(11.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.accent)
                        .child(format!("{} 个文件", state.curseforge_install_files.len())),
                ),
        )
        .when(files_loading, |this| {
            this.child(status_card(colors, "正在加载文件列表...", None))
        })
        .when_some(files_error, |this, error| {
            this.child(status_card(
                colors,
                &format!("文件列表加载失败: {error}"),
                Some(colors.danger),
            ))
        })
        .when(
            !files_loading
                && state.curseforge_install_error.is_none()
                && state.curseforge_install_files.is_empty(),
            |this| this.child(status_card(colors, "当前资源没有可安装文件。", None)),
        )
        .when(!state.curseforge_install_files.is_empty(), |this| {
            this.child(
                div()
                    .id("curseforge-install-file-list-scroll")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .scrollbar_width(px(0.))
                    .child(files_list),
            )
        });

    modal::modal_layer_dismissible(
        modal::modal_surface(
            Hsla {
                a: 0.96,
                ..colors.surface
            },
            Hsla {
                a: 0.18,
                ..colors.border
            },
            px(820.),
            px(560.),
            px(18.),
        )
        .w(relative(0.88))
        .h(relative(0.85))
        .max_w(px(860.))
        .max_h(px(600.))
        .min_w(px(500.))
        .min_h(px(340.))
        .child(render_curseforge_install_header(
            colors, "文件", mod_name, None,
        ))
        .child(body),
        Hsla {
            a: 0.0,
            ..colors.backdrop
        },
        Rc::new(|cx| {
            close_curseforge_install_modal_from_ui(cx);
        }),
    )
}

fn render_curseforge_install_modal(
    colors: &ThemeColors,
    state: &DownloadPageState,
    selected_folder: Option<SharedString>,
    local_versions: &LocalVersionsSnapshot,
    tasks: &HashMap<Arc<str>, Arc<TaskSnapshot>>,
) -> AnyElement {
    let mod_name = state
        .curseforge_install_mod
        .as_ref()
        .map(|m| m.name.clone())
        .unwrap_or_else(|| SharedString::from("CurseForge 安装"));

    let stage = state.curseforge_install_stage;
    let task_snapshot = state
        .curseforge_install_task_id
        .as_ref()
        .and_then(|id| tasks.get(id.as_ref()));

    let Some(selected_file) = state.curseforge_install_selected_file_id.and_then(|id| {
        state
            .curseforge_install_files
            .iter()
            .find(|file| file.id == id)
    }) else {
        return render_curseforge_install_file_selection_modal(
            colors,
            state,
            selected_folder,
            local_versions,
        );
    };

    let header = render_curseforge_install_header(
        colors,
        "安装",
        mod_name,
        Some(selected_file.display_name.clone()),
    );
    let target_dropdown_enabled = matches!(
        stage,
        crate::ui::views::download::state::CurseForgeInstallStage::Idle
            | crate::ui::views::download::state::CurseForgeInstallStage::Error
            | crate::ui::views::download::state::CurseForgeInstallStage::Success
    );
    let target_dropdown =
        render_install_target_dropdown(colors, state, local_versions, target_dropdown_enabled);

    let error_line = state.curseforge_install_error.as_ref().map(|e| {
        status_card(colors, &format!("错误: {e}"), Some(colors.danger)).into_any_element()
    });

    let conflict_line = state
        .curseforge_install_conflict_message
        .as_ref()
        .map(|m| status_card(colors, m.as_ref(), Some(colors.accent)).into_any_element());

    let progress_bar = task_snapshot.map(|snap| {
        let pct = snap.percent.unwrap_or(0.0).clamp(0.0, 100.0) as f32;
        div()
            .w_full()
            .rounded(px(12.))
            .bg(colors.surface)
            .border_1()
            .border_color(colors.border)
            .p(px(12.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(colors.text_secondary)
                            .child(format!("{}  {}%", snap.stage, pct.round())),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(colors.text_muted)
                            .child(format!("ETA {}", snap.eta)),
                    ),
            )
            .child(
                div()
                    .mt(px(10.))
                    .h(px(8.))
                    .rounded(px(999.))
                    .w_full()
                    .bg(Hsla {
                        a: 0.10,
                        ..colors.text_secondary
                    })
                    .child(
                        div()
                            .h(px(8.))
                            .rounded(px(999.))
                            .bg(colors.accent)
                            .w(relative(pct / 100.0)),
                    ),
            )
            .into_any_element()
    });

    let can_install = state
        .curseforge_install_selected_file_id
        .and_then(|id| state.curseforge_install_files.iter().find(|f| f.id == id))
        .and_then(|f| f.download_url.clone())
        .is_some()
        && state.curseforge_install_target_folder.is_some()
        && matches!(
            stage,
            crate::ui::views::download::state::CurseForgeInstallStage::Idle
                | crate::ui::views::download::state::CurseForgeInstallStage::Error
        );

    let primary_btn_label = match stage {
        crate::ui::views::download::state::CurseForgeInstallStage::Conflict => "覆盖安装",
        crate::ui::views::download::state::CurseForgeInstallStage::Success => "完成",
        crate::ui::views::download::state::CurseForgeInstallStage::Downloading => "下载中...",
        crate::ui::views::download::state::CurseForgeInstallStage::Inspecting => "检查中...",
        crate::ui::views::download::state::CurseForgeInstallStage::CheckingConflict => {
            "检查冲突..."
        }
        crate::ui::views::download::state::CurseForgeInstallStage::Installing => "安装中...",
        _ => "下载并安装",
    };

    let primary_enabled = match stage {
        crate::ui::views::download::state::CurseForgeInstallStage::Conflict => true,
        crate::ui::views::download::state::CurseForgeInstallStage::Success => true,
        crate::ui::views::download::state::CurseForgeInstallStage::Downloading
        | crate::ui::views::download::state::CurseForgeInstallStage::Inspecting
        | crate::ui::views::download::state::CurseForgeInstallStage::CheckingConflict
        | crate::ui::views::download::state::CurseForgeInstallStage::Installing => false,
        _ => can_install,
    };

    let primary_btn = div()
        .px(px(16.))
        .py(px(10.))
        .rounded(px(12.))
        .bg(if primary_enabled {
            colors.accent
        } else {
            colors.surface_hover
        })
        .border_1()
        .border_color(colors.border)
        .cursor_pointer()
        .text_color(if primary_enabled {
            colors.btn_primary_text
        } else {
            colors.text_muted
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(themed_icon(
                    lucide_icons::icon_download(),
                    18.0,
                    if primary_enabled {
                        colors.btn_primary_text
                    } else {
                        colors.text_muted
                    },
                ))
                .child(primary_btn_label),
        )
        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
            if !primary_enabled {
                return;
            }

            if matches!(
                stage,
                crate::ui::views::download::state::CurseForgeInstallStage::Success
            ) {
                modals::close_curseforge_install_modal(cx);
                return;
            }

            let overwrite = matches!(
                stage,
                crate::ui::views::download::state::CurseForgeInstallStage::Conflict
            );

            let (download_url, file_name, target_folder) =
                cx.read_global(|s: &DownloadPageState, _cx| {
                    let file = s
                        .curseforge_install_selected_file_id
                        .and_then(|id| s.curseforge_install_files.iter().find(|f| f.id == id));
                    (
                        file.and_then(|f| f.download_url.clone())
                            .map(|u| u.to_string()),
                        file.map(|f| f.file_name.to_string()),
                        s.curseforge_install_target_folder
                            .as_ref()
                            .map(|f| f.to_string()),
                    )
                });

            let Some(download_url) = download_url else {
                return;
            };
            let Some(file_name) = file_name else {
                return;
            };
            let Some(target_folder) = target_folder else {
                return;
            };

            cx.update_global(|s: &mut DownloadPageState, _cx| {
                s.curseforge_install_stage =
                    crate::ui::views::download::state::CurseForgeInstallStage::Downloading;
                s.curseforge_install_error = None;
                s.curseforge_install_task_id = None;
                s.curseforge_install_downloaded_path = None;
                s.curseforge_install_conflict_message = None;
            });

            cx.spawn(async move |cx| {
                let result = async {
                    let task_id = crate::downloads::api::download_resource_to_cache(
                        download_url,
                        file_name.clone(),
                        None,
                        None,
                    )
                    .await?;

                    cx.update_global(|s: &mut DownloadPageState, _cx| {
                        s.curseforge_install_task_id = Some(SharedString::from(task_id.clone()));
                    })
                    .map_err(|e| e.to_string())?;

                    let snap = wait_task_finished(&task_id).await;
                    let snap = snap?;
                    if snap.status.as_ref() != "completed" {
                        return Err(format!(
                            "download {} ({})",
                            snap.status,
                            snap.message.clone().unwrap_or_default()
                        ));
                    }
                    let path = snap
                        .message
                        .clone()
                        .map(|message| message.to_string())
                        .ok_or_else(|| "download completed but no path returned".to_string())?;

                    cx.update_global(|s: &mut DownloadPageState, _cx| {
                        s.curseforge_install_downloaded_path =
                            Some(SharedString::from(path.clone()));
                        s.curseforge_install_stage =
                            crate::ui::views::download::state::CurseForgeInstallStage::Inspecting;
                    })
                    .map_err(|e| e.to_string())?;

                    let preview =
                        crate::core::minecraft::assets::inspect_import_file(path.to_string(), None)
                            .await?;
                    if !preview.valid {
                        let msg = preview
                            .invalid_reason
                            .unwrap_or_else(|| "无效的资源包".to_string());
                        cx.update_global(|s: &mut DownloadPageState, _cx| {
                            s.curseforge_install_stage =
                                crate::ui::views::download::state::CurseForgeInstallStage::Error;
                            s.curseforge_install_error = Some(SharedString::from(msg));
                        })
                        .map_err(|e| e.to_string())?;
                        return Ok::<(), String>(());
                    }

                    cx.update_global(|s: &mut DownloadPageState, _cx| {
                        s.curseforge_install_stage =
                            crate::ui::views::download::state::CurseForgeInstallStage::CheckingConflict;
                    })
                    .map_err(|e| e.to_string())?;

                    let conflict = crate::core::minecraft::assets::check_import_conflict(
                        crate::core::minecraft::assets::CheckImportRequest {
                            build_type: crate::core::minecraft::paths::BuildType::Uwp,
                            edition: crate::core::minecraft::paths::Edition::Release,
                            version_name: target_folder.clone(),
                            enable_isolation: true,
                            user_id: None,
                            file_path: path.to_string(),
                            allow_shared_fallback: false,
                        },
                    )
                    .await?;

                    if conflict.has_conflict && !overwrite {
                        cx.update_global(|s: &mut DownloadPageState, _cx| {
                            s.curseforge_install_stage =
                                crate::ui::views::download::state::CurseForgeInstallStage::Conflict;
                            s.curseforge_install_conflict_message =
                                Some(SharedString::from(conflict.message));
                        })
                        .map_err(|e| e.to_string())?;
                        return Ok::<(), String>(());
                    }

                    cx.update_global(|s: &mut DownloadPageState, _cx| {
                        s.curseforge_install_stage =
                            crate::ui::views::download::state::CurseForgeInstallStage::Installing;
                    })
                    .map_err(|e| e.to_string())?;

                    crate::core::minecraft::assets::import_assets(
                        crate::core::minecraft::assets::ImportAssetsRequest {
                            build_type: crate::core::minecraft::paths::BuildType::Uwp,
                            edition: crate::core::minecraft::paths::Edition::Release,
                            version_name: target_folder,
                            enable_isolation: true,
                            user_id: None,
                            file_paths: vec![path.to_string()],
                            overwrite,
                            allow_shared_fallback: false,
                        },
                    )
                    .await?;

                    cx.update_global(|s: &mut DownloadPageState, _cx| {
                        s.curseforge_install_stage =
                            crate::ui::views::download::state::CurseForgeInstallStage::Success;
                    })
                    .map_err(|e| e.to_string())?;

                    Ok::<(), String>(())
                }
                .await;

                if let Err(error) = result {
                    cx.update_global(|s: &mut DownloadPageState, _cx| {
                        s.curseforge_install_stage =
                            crate::ui::views::download::state::CurseForgeInstallStage::Error;
                        s.curseforge_install_error = Some(SharedString::from(error));
                    })
                    .map_err(|e| e.to_string())?;
                }

                Ok::<(), String>(())
            })
            .detach_and_log_err(cx);
        });

    let operation_line = match stage {
        crate::ui::views::download::state::CurseForgeInstallStage::Inspecting => {
            Some(status_card(colors, "正在检查资源包...", None).into_any_element())
        }
        crate::ui::views::download::state::CurseForgeInstallStage::CheckingConflict => {
            Some(status_card(colors, "正在检查已安装资源冲突...", None).into_any_element())
        }
        crate::ui::views::download::state::CurseForgeInstallStage::Installing => {
            Some(status_card(colors, "正在安装资源包...", None).into_any_element())
        }
        crate::ui::views::download::state::CurseForgeInstallStage::Success => {
            Some(status_card(colors, "安装完成", Some(colors.accent)).into_any_element())
        }
        _ => None,
    };

    let selected_version = state
        .curseforge_install_target_folder
        .as_ref()
        .and_then(|selected| {
            local_versions
                .versions
                .iter()
                .find(|version| version.folder.as_ref() == selected.as_ref())
        });

    let version_card = div()
        .w_full()
        .rounded(px(14.))
        .bg(Hsla {
            a: 0.44,
            ..colors.surface
        })
        .border_1()
        .border_color(Hsla {
            a: 0.10,
            ..colors.border
        })
        .p(px(14.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_secondary)
                .child("安装目标版本"),
        )
        .child(target_dropdown)
        .when_some(selected_version, |this, version| {
            let target_path =
                crate::ui::hooks::use_local_versions::version_target_root_path(version)
                    .unwrap_or_else(|| SharedString::from("-"));
            let isolation = crate::ui::hooks::use_local_versions::version_isolation_label(version);
            let version_type =
                crate::ui::hooks::use_local_versions::version_type_summary_label(version);

            this.child(
                div()
                    .rounded(px(10.))
                    .px(px(10.))
                    .py(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .bg(Hsla {
                        a: 0.20,
                        ..colors.settings_field_bg
                    })
                    .child(
                        // info summary row: 目标路径
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .w(px(70.))
                                    .flex_none()
                                    .text_size(px(11.))
                                    .text_color(colors.text_muted)
                                    .child("目标路径"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .child(themed_icon(
                                        lucide_icons::icon_folder_tree(),
                                        12.0,
                                        colors.text_secondary,
                                    ))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .text_size(px(11.))
                                            .text_color(colors.text_secondary)
                                            .child(target_path),
                                    ),
                            ),
                    )
                    .child(
                        // info summary row: 隔离状态
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .w(px(70.))
                                    .flex_none()
                                    .text_size(px(11.))
                                    .text_color(colors.text_muted)
                                    .child("隔离状态"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .child(themed_icon(
                                        lucide_icons::icon_hard_drive(),
                                        12.0,
                                        colors.text_secondary,
                                    ))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .text_size(px(11.))
                                            .text_color(colors.text_secondary)
                                            .child(isolation),
                                    ),
                            ),
                    )
                    .child(
                        // info summary row: 版本类型
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .w(px(70.))
                                    .flex_none()
                                    .text_size(px(11.))
                                    .text_color(colors.text_muted)
                                    .child("版本类型"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .child(themed_icon(
                                        lucide_icons::icon_boxes(),
                                        12.0,
                                        colors.text_secondary,
                                    ))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .text_size(px(11.))
                                            .text_color(colors.text_secondary)
                                            .child(version_type),
                                    ),
                            ),
                    ),
            )
        });

    let body = div()
        .flex_1()
        .min_h(px(0.))
        .px(px(18.))
        .py(px(16.))
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(version_card)
        .child(
            div()
                .rounded(px(14.))
                .bg(Hsla {
                    a: 0.44,
                    ..colors.surface
                })
                .border_1()
                .border_color(Hsla {
                    a: 0.10,
                    ..colors.border
                })
                .p(px(14.))
                .flex()
                .flex_col()
                .items_start()
                .gap(px(10.))
                .child(primary_btn)
                .when(
                    matches!(
                        stage,
                        crate::ui::views::download::state::CurseForgeInstallStage::Idle
                            | crate::ui::views::download::state::CurseForgeInstallStage::Error
                    ),
                    |this| {
                        this.child(
                            div()
                                .text_size(px(12.))
                                .text_color(colors.text_muted)
                                .child(format!(
                                    "默认下载到系统缓存 ({})",
                                    format_bytes(selected_file.file_length)
                                )),
                        )
                    },
                )
                .when_some(progress_bar, |this, bar| this.child(bar))
                .when_some(operation_line, |this, status| this.child(status))
                .when_some(error_line, |this, error| this.child(error))
                .when_some(conflict_line, |this, conflict| this.child(conflict)),
        );

    modal::modal_layer_dismissible(
        modal::modal_surface(
            Hsla {
                a: 0.96,
                ..colors.surface
            },
            Hsla {
                a: 0.18,
                ..colors.border
            },
            px(720.),
            px(460.),
            px(18.),
        )
        .w(relative(0.85))
        .h(relative(0.80))
        .max_w(px(760.))
        .max_h(px(500.))
        .min_w(px(460.))
        .min_h(px(300.))
        .child(header)
        .child(body),
        Hsla {
            a: 0.0,
            ..colors.backdrop
        },
        Rc::new(|cx| {
            close_curseforge_install_modal_from_ui(cx);
        }),
    )
}

fn render_curseforge_mod_page_modal(
    colors: &ThemeColors,
    state: &DownloadPageState,
    detail_image_cache: &Entity<BoundedImageCache>,
    selected_folder: Option<SharedString>,
    local_versions: &LocalVersionsSnapshot,
    _tasks: &HashMap<Arc<str>, Arc<TaskSnapshot>>,
) -> AnyElement {
    let toolbar_button = |label: &'static str, icon_path: &'static str, primary: bool| {
        div()
            .h(px(42.))
            .px(px(16.))
            .rounded(px(8.))
            .bg(if primary {
                colors.accent
            } else {
                Hsla {
                    a: 0.62,
                    ..colors.surface
                }
            })
            .border_1()
            .border_color(Hsla {
                a: 0.12,
                ..colors.border
            })
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(8.))
            .text_size(px(12.))
            .font_weight(FontWeight::BOLD)
            .text_color(if primary {
                colors.btn_primary_text
            } else {
                colors.text_primary
            })
            .child(themed_icon(
                icon_path,
                16.0,
                if primary {
                    colors.btn_primary_text
                } else {
                    colors.text_primary
                },
            ))
            .child(label)
    };

    let icon_button = |icon_path: &'static str| {
        div()
            .size(px(42.))
            .rounded(px(8.))
            .bg(Hsla {
                a: 0.62,
                ..colors.surface
            })
            .border_1()
            .border_color(Hsla {
                a: 0.12,
                ..colors.border
            })
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .child(themed_icon(icon_path, 16.0, colors.text_secondary))
    };

    let stat_card = |icon_path: &'static str, label: &'static str, value: SharedString| {
        div()
            .flex_1()
            .min_w(px(0.))
            .rounded(px(10.))
            .bg(Hsla {
                a: 0.58,
                ..colors.surface
            })
            .border_1()
            .border_color(Hsla {
                a: 0.10,
                ..colors.border
            })
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(11.))
                    .text_color(colors.text_muted)
                    .child(themed_icon(icon_path, 14.0, colors.text_muted))
                    .child(label),
            )
            .child(
                div()
                    .min_w(px(0.))
                    .text_size(px(18.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.text_primary)
                    .line_height(relative(1.3))
                    .child(value),
            )
    };

    let detail_row = |label: &'static str, value: SharedString| {
        div()
            .w_full()
            .rounded(px(8.))
            .bg(Hsla {
                a: 0.42,
                ..colors.surface
            })
            .border_1()
            .border_color(Hsla {
                a: 0.08,
                ..colors.border
            })
            .px(px(14.))
            .py(px(12.))
            .flex()
            .items_start()
            .justify_between()
            .gap(px(12.))
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(colors.text_muted)
                    .child(label),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_size(px(12.))
                    .text_align(TextAlign::Right)
                    .text_color(colors.text_secondary)
                    .child(value),
            )
    };

    let header = div()
        .px(px(28.))
        .py(px(18.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.))
        .border_1()
        .border_color(Hsla {
            a: 0.10,
            ..colors.border
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .min_w(px(0.))
                .child(
                    toolbar_button("返回列表", lucide_icons::icon_arrow_left(), false)
                        .on_mouse_down(MouseButton::Left, |_ev, _window, cx| {
                            modals::close_curseforge_mod_page(cx);
                        }),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .min_w(px(0.))
                        .child(
                            div()
                                .text_size(px(15.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.text_primary)
                                .child("CurseForge 详情"),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(colors.text_muted)
                                .child("资源信息、文件列表与直接下载"),
                        ),
                ),
        )
        .when_some(state.curseforge_mod_page_mod.clone(), |this, mod_entry| {
            let default_target =
                modals::default_install_target(selected_folder.clone(), local_versions);
            let localized_categories = state
                .curseforge_categories
                .iter()
                .filter(|category| mod_entry.category_ids.contains(&category.id))
                .map(|category| {
                    localize_curseforge_tag(category.name.as_ref(), Some(category.slug.as_ref()))
                })
                .collect::<Vec<_>>();
            let open_link = share_actions::curseforge_project_url(&mod_entry);
            this.child(
                div()
                    .flex()
                    .gap(px(10.))
                    .child(
                        toolbar_button("直接下载", lucide_icons::icon_download(), true)
                            .on_mouse_down(MouseButton::Left, {
                                let mod_entry = mod_entry.clone();
                                let default_target = default_target.clone();
                                move |_ev, _window, cx| {
                                    modals::open_curseforge_install_modal(
                                        mod_entry.clone(),
                                        default_target.clone(),
                                        cx,
                                    );
                                }
                            }),
                    )
                    .child(icon_button(lucide_icons::icon_globe()).on_mouse_down(
                        MouseButton::Left,
                        {
                            let open_link = open_link.clone();
                            move |_ev, _window, cx| {
                                cx.open_url(open_link.as_ref());
                            }
                        },
                    ))
                    .child(icon_button(lucide_icons::icon_copy()).on_mouse_down(
                        MouseButton::Left,
                        {
                            let mod_entry = mod_entry.clone();
                            move |_ev, _window, cx| {
                                share_actions::copy_curseforge_link(&mod_entry, cx);
                            }
                        },
                    ))
                    .child(icon_button(lucide_icons::icon_share_2()).on_mouse_down(
                        MouseButton::Left,
                        {
                            let mod_entry = mod_entry.clone();
                            move |_ev, _window, cx| {
                                share_actions::copy_curseforge_share_text(&mod_entry, cx);
                            }
                        },
                    ))
                    .child(icon_button(lucide_icons::icon_file_text()).on_mouse_down(
                        MouseButton::Left,
                        {
                            let mod_entry = mod_entry.clone();
                            let localized_categories = localized_categories.clone();
                            move |_ev, _window, cx| {
                                share_actions::copy_curseforge_analysis(
                                    &mod_entry,
                                    &localized_categories,
                                    cx,
                                );
                            }
                        },
                    )),
            )
        });

    let body: AnyElement = if state.curseforge_mod_page_loading {
        div()
            .flex_1()
            .min_h(px(0.))
            .p(px(20.))
            .child(status_card(colors, "正在加载资源详情...", None))
            .into_any_element()
    } else if let Some(err) = state.curseforge_mod_page_error.as_ref() {
        div()
            .flex_1()
            .min_h(px(0.))
            .p(px(20.))
            .child(status_card(
                colors,
                &format!("资源详情加载失败: {err}"),
                Some(colors.danger),
            ))
            .into_any_element()
    } else if let Some(mod_entry) = state.curseforge_mod_page_mod.as_ref() {
        let title = mod_entry.name.clone();
        let summary = mod_entry
            .summary
            .clone()
            .unwrap_or_else(|| SharedString::from("暂无简介"));
        let authors = if mod_entry.author_names.is_empty() {
            SharedString::from("未知作者")
        } else {
            SharedString::from(
                mod_entry
                    .author_names
                    .iter()
                    .take(5)
                    .map(|s| s.as_ref())
                    .collect::<Vec<_>>()
                    .join(", "),
            )
        };
        let description_document = state.curseforge_mod_page_document.clone();
        let updated_at = format_date_ymd(mod_entry.date_modified.as_ref());
        let downloads = format_count(mod_entry.download_count);
        let open_link = share_actions::curseforge_project_url(mod_entry);
        let highlight_lines: Vec<SharedString> = description_document
            .plain_text_lines
            .iter()
            .take(6)
            .cloned()
            .collect();

        let mut tag_row = div().flex().gap(px(8.)).flex_wrap();
        for category_id in mod_entry.category_ids.iter().take(12) {
            let Some(category) = state
                .curseforge_categories
                .iter()
                .find(|category| category.id == *category_id)
            else {
                continue;
            };

            tag_row = tag_row.child(
                div()
                    .h(px(28.))
                    .px(px(10.))
                    .max_w(px(220.))
                    .rounded(px(999.))
                    .bg(Hsla {
                        a: 0.70,
                        ..colors.surface
                    })
                    .border_1()
                    .border_color(Hsla {
                        a: 0.10,
                        ..colors.border
                    })
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(11.))
                    .text_color(colors.text_secondary)
                    .min_w(px(0.))
                    .overflow_hidden()
                    .when_some(category.icon_url.clone(), |this, icon_url| {
                        this.child(
                            img(icon_url)
                                .w(px(12.))
                                .h(px(12.))
                                .rounded(px(3.))
                                .object_fit(ObjectFit::Contain)
                                .image_cache(detail_image_cache),
                        )
                    })
                    .child(div().min_w(px(0.)).child(localize_curseforge_tag(
                        category.name.as_ref(),
                        Some(category.slug.as_ref()),
                    ))),
            );
        }

        div()
            .flex_1()
            .min_h(px(0.))
            .overflow_hidden()
            .child(
                div()
                    .id("cf-mod-page-scroll")
                    .h_full()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .scrollbar_width(px(0.))
                    .track_scroll(&state.curseforge_mod_page_scroll)
                    .on_scroll_wheel({
                        let scroll_handle = state.curseforge_mod_page_scroll.clone();
                        move |event, window, cx| {
                            let extra_delta_y =
                                scroll_event_delta_y_with_line_height(event, window.line_height())
                                    * (CURSEFORGE_DETAIL_SCROLL_ACCELERATION - 1.0);
                            if extra_delta_y == Pixels::ZERO {
                                return;
                            }

                            let offset = scroll_handle.offset();
                            let max_offset = scroll_handle.max_offset();
                            let next_y =
                                (offset.y + extra_delta_y).clamp(-max_offset.height, px(0.));
                            if next_y != offset.y {
                                scroll_handle.set_offset(point(offset.x, next_y));
                                window.refresh();
                                window.prevent_default();
                                cx.stop_propagation();
                            }
                        }
                    })
                    .px(px(28.))
                    .py(px(22.))
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(24.))
                            .w_full()
                            .child(
                                div()
                                    .w(px(400.))
                                    .min_w(px(400.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(16.))
                                    .child(
                                        div()
                                            .relative()
                                            .w_full()
                                            .h(px(280.))
                                            .rounded(px(12.))
                                            .overflow_hidden()
                                            .bg(Hsla {
                                                a: 0.18,
                                                ..colors.accent
                                            })
                                            .when_some(
                                                mod_entry.logo_url.clone(),
                                                |this, logo_url| {
                                                    this.child(
                                                        img(logo_url)
                                                            .absolute()
                                                            .inset_0()
                                                            .w_full()
                                                            .h_full()
                                                            .object_fit(ObjectFit::Cover)
                                                            .image_cache(detail_image_cache),
                                                    )
                                                },
                                            )
                                            .child(div().absolute().inset_0().bg(linear_gradient(
                                                180.0,
                                                linear_color_stop(
                                                    Hsla {
                                                        a: 0.0,
                                                        ..colors.surface
                                                    },
                                                    0.0,
                                                ),
                                                linear_color_stop(
                                                    Hsla {
                                                        a: 0.86,
                                                        ..rgb(0x111111).into()
                                                    },
                                                    1.0,
                                                ),
                                            )))
                                            .child(
                                                div()
                                                    .absolute()
                                                    .left(px(18.))
                                                    .right(px(18.))
                                                    .bottom(px(18.))
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(8.))
                                                    .child(
                                                        div()
                                                            .text_size(px(11.))
                                                            .text_color(rgb(0xffffff))
                                                            .child("CurseForge 资源"),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(24.))
                                                            .font_weight(FontWeight::BOLD)
                                                            .text_color(rgb(0xffffff))
                                                            .child(title),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(12.))
                                                            .line_height(relative(1.5))
                                                            .text_color(Hsla {
                                                                a: 0.88,
                                                                ..rgb(0xffffff).into()
                                                            })
                                                            .child(summary.clone()),
                                                    ),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .gap(px(10.))
                                            .child(stat_card(
                                                lucide_icons::icon_download(),
                                                "总下载量",
                                                downloads.clone(),
                                            ))
                                            .child(stat_card(
                                                lucide_icons::icon_calendar_days(),
                                                "最近更新",
                                                updated_at.clone(),
                                            )),
                                    )
                                    .child(
                                        div()
                                            .rounded(px(18.))
                                            .bg(Hsla {
                                                a: 0.56,
                                                ..colors.surface
                                            })
                                            .border_1()
                                            .border_color(Hsla {
                                                a: 0.10,
                                                ..colors.border
                                            })
                                            .p(px(14.))
                                            .flex()
                                            .flex_col()
                                            .gap(px(12.))
                                            .child(
                                                div()
                                                    .text_size(px(12.))
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(colors.text_primary)
                                                    .child("资源概览"),
                                            )
                                            .child(detail_row("作者", authors))
                                            .child(detail_row(
                                                "项目 ID",
                                                SharedString::from(mod_entry.id.to_string()),
                                            ))
                                            .child(detail_row("更新时间", updated_at.clone()))
                                            .child(detail_row("页面链接", open_link.clone())),
                                    )
                                    .when(!mod_entry.category_ids.is_empty(), |this| {
                                        this.child(
                                            div()
                                                .rounded(px(18.))
                                                .bg(Hsla {
                                                    a: 0.50,
                                                    ..colors.surface
                                                })
                                                .border_1()
                                                .border_color(Hsla {
                                                    a: 0.10,
                                                    ..colors.border
                                                })
                                                .p(px(14.))
                                                .flex()
                                                .flex_col()
                                                .gap(px(12.))
                                                .child(
                                                    div()
                                                        .text_size(px(12.))
                                                        .font_weight(FontWeight::BOLD)
                                                        .text_color(colors.text_primary)
                                                        .child("分类标签"),
                                                )
                                                .child(tag_row),
                                        )
                                    })
                                    .when(!highlight_lines.is_empty(), |this| {
                                        let mut notes = div().flex().flex_col().gap(px(8.));
                                        for line in &highlight_lines {
                                            notes = notes.child(
                                                div()
                                                    .rounded(px(12.))
                                                    .bg(Hsla {
                                                        a: 0.44,
                                                        ..colors.surface
                                                    })
                                                    .border_1()
                                                    .border_color(Hsla {
                                                        a: 0.08,
                                                        ..colors.border
                                                    })
                                                    .p(px(12.))
                                                    .text_size(px(12.))
                                                    .line_height(relative(1.5))
                                                    .text_color(colors.text_secondary)
                                                    .child(SharedString::from(line.to_string())),
                                            );
                                        }
                                        this.child(
                                            div()
                                                .rounded(px(18.))
                                                .bg(Hsla {
                                                    a: 0.50,
                                                    ..colors.surface
                                                })
                                                .border_1()
                                                .border_color(Hsla {
                                                    a: 0.10,
                                                    ..colors.border
                                                })
                                                .p(px(14.))
                                                .flex()
                                                .flex_col()
                                                .gap(px(12.))
                                                .child(
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .gap(px(8.))
                                                        .child(themed_icon(
                                                            lucide_icons::icon_scroll_text(),
                                                            14.0,
                                                            colors.accent,
                                                        ))
                                                        .child(
                                                            div()
                                                                .text_size(px(12.))
                                                                .font_weight(FontWeight::BOLD)
                                                                .text_color(colors.text_primary)
                                                                .child("详细介绍 & 快速了解"),
                                                        ),
                                                )
                                                .child(notes),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(16.))
                                    .child(render_curseforge_detail_files_panel(
                                        colors,
                                        state,
                                        selected_folder.clone(),
                                        local_versions,
                                    ))
                                    .child(render_curseforge_detail_description_panel(
                                        colors, state,
                                    )),
                            ),
                    ),
            )
            .into_any_element()
    } else {
        div()
            .p(px(16.))
            .child(status_card(colors, "未选择资源", None))
            .into_any_element()
    };

    modal::modal_layer_dismissible(
        modal::modal_surface(
            Hsla {
                a: 0.92,
                ..colors.surface
            },
            colors.border,
            px(1000.),
            px(580.),
            px(18.),
        )
        .w(relative(0.92))
        .h(relative(0.88))
        .max_w(px(1040.))
        .max_h(px(660.))
        .min_w(px(580.))
        .min_h(px(380.))
        .child(header)
        .child(body),
        Hsla {
            a: 0.55,
            ..colors.backdrop
        },
        Rc::new(|cx| {
            modals::close_curseforge_mod_page(cx);
        }),
    )
}
