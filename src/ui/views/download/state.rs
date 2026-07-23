use gpui::{App, Entity, Global, ScrollHandle, SharedString, Task, point, px};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crate::ui::components::html_renderer::HtmlDocument;
use crate::ui::components::input::InputState;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DownloadTab {
    Game,
    ResourcePack,
    Mod,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceCategory {
    All,
    Addon,
    Map,
    Skin,
    Texture,
    Script,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceViewMode {
    List,
    Grid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DownloadChannelFilter {
    All,
    Release,
    Beta,
    Preview,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GameDialogKind {
    ConfirmDownload,
    LocalActions,
    ConfirmDelete,
}

#[derive(Clone, Debug)]
pub struct GameDialogState {
    pub kind: GameDialogKind,
    pub version: SharedString,
    pub package_id: SharedString,
    pub version_type: i32,
    pub md5: Option<SharedString>,
    pub is_gdk: bool,
    pub file_name: SharedString,
    pub local_path: Option<SharedString>,
}

#[derive(Clone, Debug)]
pub struct GameDialogCdnResult {
    pub base: SharedString,
    pub url: SharedString,
    pub latency_ms: Option<u64>,
    pub error: Option<SharedString>,
}

#[derive(Clone, Debug)]
pub struct DownloadRemoteVersion {
    pub version: SharedString,
    pub package_id: SharedString,
    pub version_type: i32,
    pub build_type: SharedString,
    pub archival_status: Option<i32>,
    pub meta_present: bool,
    pub md5: Option<SharedString>,
    pub is_gdk: bool,
}

#[derive(Clone, Debug)]
pub struct DownloadOperation {
    pub package_id: SharedString,
    pub file_name: SharedString,
    pub download_task_id: Option<SharedString>,
    pub extract_task_id: Option<SharedString>,
}

#[derive(Clone, Debug)]
pub struct CurseForgeCategoryEntry {
    pub id: i32,
    pub name: SharedString,
    pub slug: SharedString,
    pub icon_url: Option<SharedString>,
    pub is_class: bool,
    pub class_id: Option<i32>,
    pub parent_category_id: Option<i32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CurseForgeModEntry {
    pub id: i32,
    pub name: SharedString,
    pub summary: Option<SharedString>,
    pub author_names: Vec<SharedString>,
    pub logo_url: Option<SharedString>,
    pub download_count: f64,
    pub date_modified: SharedString,
    pub class_id: Option<i32>,
    pub category_ids: Vec<i32>,
}

#[derive(Clone, Debug)]
pub struct CurseForgeFileEntry {
    pub id: i32,
    pub display_name: SharedString,
    pub file_name: SharedString,
    pub file_length: u64,
    pub download_url: Option<SharedString>,
    pub game_versions: Vec<SharedString>,
    pub file_date: SharedString,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CurseForgeInstallStage {
    Idle,
    LoadingFiles,
    Downloading,
    Inspecting,
    CheckingConflict,
    Conflict,
    Installing,
    Success,
    Error,
}

pub struct DownloadPageState {
    pub tab: DownloadTab,
    pub loaded: bool,
    pub loading: bool,
    pub error: Option<SharedString>,
    pub versions: Vec<DownloadRemoteVersion>,
    pub local_path_by_package: HashMap<SharedString, SharedString>,
    pub local_files: HashSet<SharedString>,
    pub operations_by_package: HashMap<SharedString, DownloadOperation>,
    pub force_download_by_package: HashMap<SharedString, bool>,
    pub force_refresh_next: bool,
    pub downloads_index_loaded: bool,
    pub downloads_index_loading: bool,
    pub search_input: Option<Entity<InputState>>,
    pub search_query: SharedString,
    pub channel_filter: DownloadChannelFilter,
    pub page_index: usize,
    pub page_size: usize,
    pub page_jump_input: Option<Entity<InputState>>,
    pub game_dialog: Option<GameDialogState>,
    pub game_dialog_input: Option<Entity<InputState>>,
    pub game_dialog_folder_input: Option<Entity<InputState>>,
    pub game_dialog_cdn_loading: bool,
    pub game_dialog_cdn_error: Option<SharedString>,
    pub game_dialog_cdn_results: Vec<GameDialogCdnResult>,
    pub game_dialog_selected_cdn_base: Option<SharedString>,
    pub game_rows_scroll: ScrollHandle,
    pub curseforge_sidebar_scroll: ScrollHandle,
    pub curseforge_results_scroll: ScrollHandle,
    pub curseforge_mod_page_scroll: ScrollHandle,
    pub resource_category: ResourceCategory,
    pub resource_view_mode: ResourceViewMode,
    pub curseforge_loaded: bool,
    pub curseforge_loading: bool,
    pub curseforge_error: Option<SharedString>,
    pub curseforge_categories: Vec<CurseForgeCategoryEntry>,
    pub curseforge_versions: Vec<SharedString>,
    pub curseforge_selected_root_id: Option<i32>,
    pub curseforge_selected_sub_id: Option<i32>,
    pub curseforge_selected_game_version: SharedString,
    pub curseforge_sort_field: i32,
    pub curseforge_sort_order: SharedString,
    pub curseforge_page_index: usize,
    pub curseforge_page_size: u32,
    pub curseforge_results_loading: bool,
    pub curseforge_results_error: Option<SharedString>,
    pub curseforge_mods: Vec<CurseForgeModEntry>,
    pub curseforge_results_list_seq: u64,
    pub curseforge_total_count: Option<u32>,
    pub curseforge_has_more: bool,
    pub curseforge_sub_collapsed: bool,
    pub curseforge_last_query_key: SharedString,
    pub curseforge_view_epoch: u64,
    pub curseforge_results_epoch: u64,
    pub curseforge_results_transition_at: Option<Instant>,
    pub curseforge_results_abort_handle: Option<tokio::task::AbortHandle>,
    pub curseforge_disable_result_logos: bool,
    pub curseforge_result_logo_seq: u64,

    // 翻页时滚动重置标记：true 表示新数据加载完成后需要重置滚动条到顶部
    pub curseforge_pending_scroll_reset_to_top: bool,

    pub curseforge_invalidate_seq: u64,
    pub curseforge_invalidate_task: Option<Task<()>>,
    pub curseforge_page_commit_seq: u64,
    pub curseforge_page_commit_task: Option<Task<()>>,
    pub curseforge_pending_page_index: Option<usize>,
    pub curseforge_search_commit_seq: u64,
    pub curseforge_search_commit_task: Option<Task<()>>,
    pub curseforge_mod_page_open: bool,
    pub curseforge_mod_page_loading: bool,
    pub curseforge_mod_page_error: Option<SharedString>,
    pub curseforge_mod_page_mod_id: Option<i32>,
    pub curseforge_mod_page_mod: Option<CurseForgeModEntry>,
    pub curseforge_mod_page_description: SharedString,
    pub curseforge_mod_page_document: HtmlDocument,
    pub curseforge_install_open: bool,
    pub curseforge_install_stage: CurseForgeInstallStage,
    pub curseforge_install_error: Option<SharedString>,
    pub curseforge_install_mod: Option<CurseForgeModEntry>,
    pub curseforge_install_files: Vec<CurseForgeFileEntry>,
    pub curseforge_install_selected_file_id: Option<i32>,
    pub curseforge_install_target_folder: Option<SharedString>,
    pub curseforge_install_task_id: Option<SharedString>,
    pub curseforge_install_downloaded_path: Option<SharedString>,
    pub curseforge_install_conflict_message: Option<SharedString>,
    pub levilauncher_loaded: bool,
    pub levilauncher_loading: bool,
    pub levilauncher_error: Option<SharedString>,
    pub levilauncher_all_mods: Vec<crate::core::levilamina::LeviLaminaModEntry>,
    pub levilauncher_loaders: Vec<SharedString>,
    pub levilauncher_selected_loader: SharedString,
    pub levilauncher_loader_versions: Vec<SharedString>,
    pub levilauncher_selected_loader_version: SharedString,
    pub levilauncher_page_index: usize,
    pub levilauncher_page_size: usize,
    pub levilauncher_scroll: ScrollHandle,
    pub levilauncher_modal_open: bool,
    pub levilauncher_selected_mod: Option<crate::core::levilamina::LeviLaminaModEntry>,
    pub levilauncher_selected_version: SharedString,
    pub tab_anim_at: Option<Instant>,
    pub tab_anim_from: DownloadTab,
}

impl Default for DownloadPageState {
    fn default() -> Self {
        Self {
            tab: DownloadTab::Game,
            loaded: false,
            loading: false,
            error: None,
            versions: Vec::new(),
            local_path_by_package: HashMap::new(),
            local_files: HashSet::new(),
            operations_by_package: HashMap::new(),
            force_download_by_package: HashMap::new(),
            force_refresh_next: false,
            downloads_index_loaded: false,
            downloads_index_loading: false,
            search_input: None,
            search_query: SharedString::from(""),
            channel_filter: DownloadChannelFilter::Release,
            page_index: 0,
            page_size: 10,
            page_jump_input: None,
            game_dialog: None,
            game_dialog_input: None,
            game_dialog_folder_input: None,
            game_dialog_cdn_loading: false,
            game_dialog_cdn_error: None,
            game_dialog_cdn_results: Vec::new(),
            game_dialog_selected_cdn_base: None,
            game_rows_scroll: ScrollHandle::new(),
            curseforge_sidebar_scroll: ScrollHandle::new(),
            curseforge_results_scroll: ScrollHandle::new(),
            curseforge_mod_page_scroll: ScrollHandle::new(),
            resource_category: ResourceCategory::All,
            resource_view_mode: ResourceViewMode::List,
            curseforge_loaded: false,
            curseforge_loading: false,
            curseforge_error: None,
            curseforge_categories: Vec::new(),
            curseforge_versions: Vec::new(),
            curseforge_selected_root_id: None,
            curseforge_selected_sub_id: None,
            curseforge_selected_game_version: SharedString::from(""),
            curseforge_sort_field: 1,
            curseforge_sort_order: SharedString::from("desc"),
            curseforge_page_index: 0,
            curseforge_page_size: 12,
            curseforge_results_loading: false,
            curseforge_results_error: None,
            curseforge_mods: Vec::new(),
            curseforge_results_list_seq: 0,
            curseforge_total_count: None,
            curseforge_has_more: false,
            curseforge_sub_collapsed: false,
            curseforge_last_query_key: SharedString::from(""),
            curseforge_view_epoch: 0,
            curseforge_results_epoch: 0,
            curseforge_results_transition_at: None,
            curseforge_results_abort_handle: None,
            curseforge_disable_result_logos: false,
            curseforge_result_logo_seq: 0,
            curseforge_pending_scroll_reset_to_top: false,
            curseforge_invalidate_seq: 0,
            curseforge_invalidate_task: None,
            curseforge_page_commit_seq: 0,
            curseforge_page_commit_task: None,
            curseforge_pending_page_index: None,
            curseforge_search_commit_seq: 0,
            curseforge_search_commit_task: None,
            curseforge_mod_page_open: false,
            curseforge_mod_page_loading: false,
            curseforge_mod_page_error: None,
            curseforge_mod_page_mod_id: None,
            curseforge_mod_page_mod: None,
            curseforge_mod_page_description: SharedString::from(""),
            curseforge_mod_page_document: HtmlDocument::default(),
            curseforge_install_open: false,
            curseforge_install_stage: CurseForgeInstallStage::Idle,
            curseforge_install_error: None,
            curseforge_install_mod: None,
            curseforge_install_files: Vec::new(),
            curseforge_install_selected_file_id: None,
            curseforge_install_target_folder: None,
            curseforge_install_task_id: None,
            curseforge_install_downloaded_path: None,
            curseforge_install_conflict_message: None,
            levilauncher_loaded: false,
            levilauncher_loading: false,
            levilauncher_error: None,
            levilauncher_all_mods: Vec::new(),
            levilauncher_loaders: vec![
                SharedString::from("全部"),
                SharedString::from("LeviLamina"),
            ],
            levilauncher_selected_loader: SharedString::from("全部"),
            levilauncher_loader_versions: Vec::new(),
            levilauncher_selected_loader_version: SharedString::from("全部版本"),
            levilauncher_page_index: 0,
            levilauncher_page_size: 12,
            levilauncher_scroll: ScrollHandle::new(),
            levilauncher_modal_open: false,
            levilauncher_selected_mod: None,
            levilauncher_selected_version: SharedString::from(""),
            tab_anim_at: None,
            tab_anim_from: DownloadTab::Game,
        }
    }
}

impl Global for DownloadPageState {}

impl DownloadPageState {
    pub fn has_releasable_route_state(&self) -> bool {
        self.tab != DownloadTab::Game
            || self.tab_anim_at.is_some()
            || self.force_refresh_next
            || self.search_input.is_some()
            || self.page_jump_input.is_some()
            || self.curseforge_results_abort_handle.is_some()
            || self.curseforge_invalidate_task.is_some()
            || self.curseforge_page_commit_task.is_some()
            || self.curseforge_search_commit_task.is_some()
            || self.curseforge_pending_page_index.is_some()
            || self.curseforge_loaded
            || self.curseforge_loading
            || self.curseforge_error.is_some()
            || !self.curseforge_categories.is_empty()
            || !self.curseforge_versions.is_empty()
            || self.curseforge_selected_root_id.is_some()
            || self.curseforge_selected_sub_id.is_some()
            || !self.curseforge_selected_game_version.as_ref().is_empty()
            || self.curseforge_results_loading
            || self.curseforge_results_error.is_some()
            || !self.curseforge_mods.is_empty()
            || self.curseforge_total_count.is_some()
            || self.curseforge_has_more
            || self.curseforge_sub_collapsed
            || !self.curseforge_last_query_key.as_ref().is_empty()
            || self.curseforge_results_transition_at.is_some()
            || self.curseforge_disable_result_logos
            || self.curseforge_pending_scroll_reset_to_top
            || self.curseforge_mod_page_open
            || self.curseforge_mod_page_loading
            || self.curseforge_mod_page_error.is_some()
            || self.curseforge_mod_page_mod_id.is_some()
            || self.curseforge_mod_page_mod.is_some()
            || !self.curseforge_mod_page_description.as_ref().is_empty()
            || self.curseforge_install_open
            || self.curseforge_install_stage != CurseForgeInstallStage::Idle
            || self.curseforge_install_error.is_some()
            || self.curseforge_install_mod.is_some()
            || !self.curseforge_install_files.is_empty()
            || self.curseforge_install_selected_file_id.is_some()
            || self.curseforge_install_target_folder.is_some()
            || self.curseforge_install_task_id.is_some()
            || self.curseforge_install_downloaded_path.is_some()
            || self.curseforge_install_conflict_message.is_some()
    }

    pub fn release_route_state(&mut self, cx: &mut App) {
        self.release_game_tab_state();
        self.release_curseforge_tab_state(cx);
        *self = Self::default();
    }

    pub fn tab_anim_factor(&self, now: Instant) -> (f32, bool) {
        const DURATION_MS: u64 = 180;
        let Some(started_at) = self.tab_anim_at else {
            return (1.0, false);
        };
        let elapsed_ms = now.saturating_duration_since(started_at).as_millis() as u64;
        let factor = (elapsed_ms as f32 / DURATION_MS as f32).clamp(0.0, 1.0);
        (factor, factor < 1.0)
    }

    pub fn set_curseforge_mod_page_description(&mut self, description: SharedString) {
        self.curseforge_mod_page_document =
            crate::ui::components::html_renderer::parse_html_document(description.as_ref());
        self.curseforge_mod_page_description = description;
    }

    pub fn release_game_tab_state(&mut self) {
        self.loaded = false;
        self.loading = false;
        self.error = None;
        self.versions.clear();
        self.local_path_by_package.clear();
        self.local_files.clear();
        self.force_download_by_package.clear();
        self.downloads_index_loaded = false;
        self.downloads_index_loading = false;
        self.page_index = 0;
        self.game_dialog = None;
        self.game_dialog_input = None;
        self.game_dialog_folder_input = None;
        self.game_dialog_cdn_loading = false;
        self.game_dialog_cdn_error = None;
        self.game_dialog_cdn_results.clear();
        self.game_dialog_selected_cdn_base = None;
        self.game_rows_scroll.set_offset(point(px(0.), px(0.)));
    }

    pub fn release_curseforge_tab_state(&mut self, cx: &mut App) {
        if let Some(handle) = self.curseforge_results_abort_handle.take() {
            handle.abort();
        }
        self.curseforge_view_epoch = self.curseforge_view_epoch.wrapping_add(1);
        self.curseforge_results_epoch = self.curseforge_results_epoch.wrapping_add(1);
        self.curseforge_invalidate_seq = self.curseforge_invalidate_seq.wrapping_add(1);
        self.curseforge_page_commit_seq = self.curseforge_page_commit_seq.wrapping_add(1);
        self.curseforge_search_commit_seq = self.curseforge_search_commit_seq.wrapping_add(1);
        self.curseforge_invalidate_task.take();
        self.curseforge_page_commit_task.take();
        self.curseforge_search_commit_task.take();
        self.curseforge_pending_page_index = None;
        self.curseforge_loaded = false;
        self.curseforge_loading = false;
        self.curseforge_error = None;
        self.curseforge_categories.clear();
        self.curseforge_versions.clear();
        self.curseforge_selected_root_id = None;
        self.curseforge_selected_sub_id = None;
        self.curseforge_selected_game_version = SharedString::from("");
        self.curseforge_mod_page_loading = false;
        self.curseforge_last_query_key = SharedString::from("");
        self.curseforge_results_loading = false;
        self.curseforge_results_error = None;
        self.curseforge_mods.clear();
        self.curseforge_total_count = None;
        self.curseforge_has_more = false;
        self.curseforge_sub_collapsed = false;
        self.curseforge_disable_result_logos = false;
        self.curseforge_results_transition_at = None;
        self.curseforge_result_logo_seq = self.curseforge_result_logo_seq.wrapping_add(1);
        self.curseforge_pending_scroll_reset_to_top = false;
        self.curseforge_results_scroll
            .set_offset(point(px(0.), px(0.)));
        self.curseforge_sidebar_scroll
            .set_offset(point(px(0.), px(0.)));
        self.curseforge_mod_page_open = false;
        self.curseforge_mod_page_error = None;
        self.curseforge_mod_page_mod_id = None;
        self.curseforge_mod_page_mod = None;
        self.set_curseforge_mod_page_description(SharedString::from(""));
        self.curseforge_install_open = false;
        self.curseforge_install_stage = CurseForgeInstallStage::Idle;
        self.curseforge_install_error = None;
        self.curseforge_install_mod = None;
        self.curseforge_install_files.clear();
        self.curseforge_install_selected_file_id = None;
        self.curseforge_install_target_folder = None;
        self.curseforge_install_task_id = None;
        self.curseforge_install_downloaded_path = None;
        self.curseforge_install_conflict_message = None;
    }

    pub fn bump_curseforge_results_list_seq(&mut self) {
        self.curseforge_results_list_seq = self.curseforge_results_list_seq.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_game_versions_do_not_force_route_release() {
        let mut state = DownloadPageState::default();
        state.versions.push(DownloadRemoteVersion {
            version: SharedString::from("1.21.0"),
            package_id: SharedString::from("package"),
            version_type: 0,
            build_type: SharedString::from("release"),
            archival_status: None,
            meta_present: true,
            md5: None,
            is_gdk: false,
        });

        assert!(!state.has_releasable_route_state());
    }

    #[test]
    fn curseforge_runtime_state_forces_route_release() {
        let mut state = DownloadPageState::default();
        state.curseforge_loaded = true;

        assert!(state.has_releasable_route_state());
    }
}
