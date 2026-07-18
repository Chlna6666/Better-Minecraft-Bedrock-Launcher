use crate::archive::api::import_appx;
use crate::core::minecraft::gdk::unpack::start_unpack_gdk_task;
use crate::core::version::api::delete_version;
use crate::tasks::task_manager;
use crate::ui::components::code_editor::{CodeEditorEvent, CodeEditorLanguage};
use crate::ui::components::dropdown::{Dropdown, DropdownOption};
use crate::ui::components::input::{Input, InputEvent, InputSize, InputState};
use crate::ui::components::minecraft_text::MinecraftFormattedText;
use crate::ui::components::modal;
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::components::tabs::{AnimatedSegmentTabs, TabItem, UnderlineTabs};
use crate::ui::components::toast;
use crate::ui::components::virtual_list::compute_virtual_list_plan;
use crate::ui::hooks::use_launcher::{LaunchVersionDescriptor, start_launcher};
use crate::ui::hooks::use_local_versions::{
    ensure_local_versions_loaded, launch_version_icon_path, remove_local_version,
};
use crate::ui::navigation::AppRoute;
use crate::ui::state::i18n::I18n;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};
use crate::ui::views::manage::common::{
    empty_state, ghost_button, icon_action, page_shell, panel_shell, primary_button, subtle_badge,
    tonal_badge,
};
use crate::ui::views::manage::state::{
    ManageAssetEntry, ManageAssetSortKey, ManageGdkUser, ManagePackSubtype, ManagePageState,
    ManageScreenshotEntry, ManageServerEntry, ManageServerMotdStatus, ManageServerMotdTarget,
    ManageTab, ManageVersionConfig, ManagedVersionEntry,
};
use crate::utils::file_picker::{
    pick_file_path_with_filter, pick_file_path_with_filter_for_window, pick_file_paths_with_filter,
    pick_save_path_with_filter,
};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;
use std::collections::{HashMap, HashSet};
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
pub mod state;
mod thumbnail;
mod version_settings;
mod view;

use assets_tab::*;
use dialogs::*;
use layout::*;
use level_dat_bridge::*;
use lifecycle::*;
use maps_tab::*;
use mod_tab::*;
use screenshots_tab::*;
use servers_tab::*;
use shared::*;
use skin_pack_data::*;
use thumbnail::*;

pub use dialogs::render_manage_overlay;
pub use view::ManagePageView;
