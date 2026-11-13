use crate::core::minecraft::local_package::{
    LOCAL_GAME_PACKAGE_EXTENSIONS, start_import,
};
use crate::core::version::api::{start_delete_version_task, start_rename_version_task};
use crate::tasks::task_manager;
use crate::ui::components::code_editor::{CodeEditorEvent, CodeEditorLanguage};
use crate::ui::components::dropdown::{Dropdown, DropdownOption};
use crate::ui::components::input::{Input, InputEvent, InputSize, InputState};
use crate::ui::components::minecraft_text::MinecraftFormattedText;
use crate::ui::components::modal;
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::components::tabs::{AnimatedSegmentTabs, TabItem, UnderlineTabs};
use crate::ui::components::toast;
use crate::ui::components::virtual_list::slice_virtual_list;
use crate::ui::hooks::launcher;
use crate::ui::hooks::launcher::LaunchVersionDescriptor;
use crate::ui::hooks::local_versions::{VersionRefresh, ensure_loaded, icon, remove};
use crate::ui::navigation::AppRoute;
use crate::ui::state::i18n::I18n;
use crate::ui::state::import::ImportCompletionState;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};
use crate::ui::views::manage::common::{
    empty_state, ghost_button, icon_action, page_shell, panel_shell, primary_button, subtle_badge,
    tonal_badge,
};
use crate::ui::views::manage::state::{
    ManageAssetEntry, ManageAssetSortKey, ManageGdkUser, ManagePackSubtype, ManagePageState,
    ManageScreenshotEntry, ManageServerEntry, ManageServerMotdStatus, ManageServerMotdTarget,
    ManageTab, ManageVersionConfig, ManagedInstanceRevision, ManagedModLoader, ManagedVersionEntry,
    preferred_gdk_user,
};
use crate::utils::file_picker::{
    pick_file_path, pick_file_paths,
    pick_save_path,
};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;
use tracing::warn;
use url::form_urlencoded::byte_serialize;

mod actions;
mod assets_tab;
mod common;
pub(crate) mod data;
mod dialogs;
mod imports;
mod layout;
mod level_dat_bridge;
pub(crate) mod level_dat_editor;
mod level_dat_schema;
mod lifecycle;
mod maps_tab;
mod mod_tab;
mod screenshots_tab;
mod servers_tab;
mod shared;
mod skin_pack_data;
pub(crate) mod snapshots;
mod snapshot_events;
pub mod state;
mod statistics_tab;
mod tab_body;
mod thumbnail;
mod version_settings;
mod view;

use assets_tab::*;
use dialogs::*;
use imports::*;
use layout::*;
use level_dat_bridge::*;
use lifecycle::*;
use maps_tab::*;
use mod_tab::*;
use screenshots_tab::*;
use servers_tab::*;
use shared::*;
use skin_pack_data::*;
use statistics_tab::*;
use tab_body::ManageTabBodyView;
use thumbnail::*;

pub use dialogs::render_manage_overlay;
pub use view::ManagePageView;
