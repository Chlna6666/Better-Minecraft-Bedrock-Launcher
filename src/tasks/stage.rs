use std::borrow::Cow;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Stable, machine-readable task stage code.
///
/// Task stages cross module and plugin boundaries, so they must never carry
/// localized text: producers select one of the associated constants (or
/// [`TaskStage::custom`] for externally supplied codes) and the UI resolves the
/// display name through [`TaskStage::label_key`].
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TaskStage(Cow<'static, str>);

impl TaskStage {
    const fn new(code: &'static str) -> Self {
        Self(Cow::Borrowed(code))
    }

    /// Wraps a stage produced outside this crate, such as a plugin request.
    pub fn custom(code: impl Into<String>) -> Self {
        Self(Cow::Owned(code.into()))
    }

    pub fn code(&self) -> &str {
        self.0.as_ref()
    }

    /// Translation key for the display name, or `None` for custom codes.
    pub fn label_key(&self) -> Option<&'static str> {
        Some(match self.0.as_ref() {
            "queued" => "TaskStage.queued",
            "ready" => "TaskStage.ready",
            "running" => "TaskStage.running",
            "starting" => "TaskStage.starting",
            "downloading" => "TaskStage.downloading",
            "verifying" => "TaskStage.verifying",
            "extracting" => "TaskStage.extracting",
            "decrypting" => "TaskStage.decrypting",
            "merging" => "TaskStage.merging",
            "patching" => "TaskStage.patching",
            "preparing_files" => "TaskStage.preparing_files",
            "initializing" => "TaskStage.initializing",
            "launching" => "TaskStage.launching",
            "testing" => "TaskStage.testing",
            "renaming" => "TaskStage.renaming",
            "parsing" => "TaskStage.parsing",
            "resolving_url" => "TaskStage.resolving_url",
            "reading_body" => "TaskStage.reading_body",
            "url_resolved" => "TaskStage.url_resolved",
            "single_thread_fallback" => "TaskStage.single_thread_fallback",
            "installing_game" => "TaskStage.installing_game",
            "installing_mod" => "TaskStage.installing_mod",
            "installing_native_mod" => "TaskStage.installing_native_mod",
            "installing_levilamina" => "TaskStage.installing_levilamina",
            "downloading_levilamina" => "TaskStage.downloading_levilamina",
            "resolving_levilamina" => "TaskStage.resolving_levilamina",
            "preparing_game_install" => "TaskStage.preparing_game_install",
            "inspecting_imports" => "TaskStage.inspecting_imports",
            "deleting_assets" => "TaskStage.deleting_assets",
            "deleting_screenshot" => "TaskStage.deleting_screenshot",
            "updating_servers" => "TaskStage.updating_servers",
            "backing_up_uwp" => "TaskStage.backing_up_uwp",
            "verifying_uwp_backup" => "TaskStage.verifying_uwp_backup",
            "resolving_runner" => "TaskStage.resolving_runner",
            "preparing_prefix" => "TaskStage.preparing_prefix",
            "running_game" => "TaskStage.running_game",
            "downloading_game" => "TaskStage.downloading_game",
            "resolving_proton_gdk" => "TaskStage.resolving_proton_gdk",
            "downloading_proton_gdk" => "TaskStage.downloading_proton_gdk",
            "extracting_proton_gdk" => "TaskStage.extracting_proton_gdk",
            "awaiting_linux_authorization" => "TaskStage.awaiting_linux_authorization",
            "installing_linux_packages" => "TaskStage.installing_linux_packages",
            "map_paste" => "TaskStage.map_paste",
            "map_write" => "TaskStage.map_write",
            "map_delete" => "TaskStage.map_delete",
            "map_copy" => "TaskStage.map_copy",
            "map_refresh" => "TaskStage.map_refresh",
            "map_export" => "TaskStage.map_export",
            "map_import" => "TaskStage.map_import",
            "map_open" => "TaskStage.map_open",
            "map_index" => "TaskStage.map_index",
            "map_probe_tiles" => "TaskStage.map_probe_tiles",
            "map_partial_refresh" => "TaskStage.map_partial_refresh",
            "map_parse_obj" => "TaskStage.map_parse_obj",
            "map_read_model" => "TaskStage.map_read_model",
            "map_decode_image" => "TaskStage.map_decode_image",
            "map_scale_and_slice" => "TaskStage.map_scale_and_slice",
            "map_generate_maps" => "TaskStage.map_generate_maps",
            "map_merge_preview" => "TaskStage.map_merge_preview",
            "map_voxelize" => "TaskStage.map_voxelize",
            "map_save_preview" => "TaskStage.map_save_preview",
            "map_match_blocks" => "TaskStage.map_match_blocks",
            "map_read_source" => "TaskStage.map_read_source",
            "map_read_chunks_and_packs" => "TaskStage.map_read_chunks_and_packs",
            "map_crop_and_slice" => "TaskStage.map_crop_and_slice",
            "map_crop_scale_match" => "TaskStage.map_crop_scale_match",
            "map_write_mcstructure" => "TaskStage.map_write_mcstructure",
            "map_generate_records" => "TaskStage.map_generate_records",
            "map_write_bundle" => "TaskStage.map_write_bundle",
            "map_read_bundle" => "TaskStage.map_read_bundle",
            "map_check_target" => "TaskStage.map_check_target",
            "map_check_frame_area" => "TaskStage.map_check_frame_area",
            "map_create_undo_snapshot" => "TaskStage.map_create_undo_snapshot",
            "map_commit" => "TaskStage.map_commit",
            "map_save_undo_history" => "TaskStage.map_save_undo_history",
            "installing" => "TaskStage.installing",
            "inspecting" => "TaskStage.inspecting",
            "checking_conflict" => "TaskStage.checking_conflict",
            "preparing_update" => "TaskStage.preparing_update",
            "installing_assets" => "TaskStage.installing_assets",
            "installing_plugin" => "TaskStage.installing_plugin",
            "uninstalling_plugin" => "TaskStage.uninstalling_plugin",
            "map_serialize_model" => "TaskStage.map_serialize_model",
            "map_build_block_mesh" => "TaskStage.map_build_block_mesh",
            "map_output_model_assets" => "TaskStage.map_output_model_assets",
            "map_encode_gltf" => "TaskStage.map_encode_gltf",
            "history_operation" => "TaskStage.history_operation",
            "completed" => "TaskStage.completed",
            "deleting_version" => "TaskStage.deleting_version",
            "renaming_version" => "TaskStage.renaming_version",
            "updating_mod_state" => "TaskStage.updating_mod_state",
            "updating_mod_manifest" => "TaskStage.updating_mod_manifest",
            "deleting_mods" => "TaskStage.deleting_mods",
            "installing_uwp_dependencies" => "TaskStage.installing_uwp_dependencies",
            "installing_game_input" => "TaskStage.installing_game_input",
            "installing_windows_app_sdk" => "TaskStage.installing_windows_app_sdk",
            "map_match_blocks_luminance" => "TaskStage.map_match_blocks_luminance",
            _ => return None,
        })
    }

    pub const QUEUED: Self = Self::new("queued");
    pub const READY: Self = Self::new("ready");
    pub const RUNNING: Self = Self::new("running");
    pub const STARTING: Self = Self::new("starting");
    pub const DOWNLOADING: Self = Self::new("downloading");
    pub const VERIFYING: Self = Self::new("verifying");
    pub const EXTRACTING: Self = Self::new("extracting");
    pub const DECRYPTING: Self = Self::new("decrypting");
    pub const MERGING: Self = Self::new("merging");
    pub const PATCHING: Self = Self::new("patching");
    pub const PREPARING_FILES: Self = Self::new("preparing_files");
    pub const INITIALIZING: Self = Self::new("initializing");
    pub const LAUNCHING: Self = Self::new("launching");
    pub const TESTING: Self = Self::new("testing");
    pub const RENAMING: Self = Self::new("renaming");
    pub const PARSING: Self = Self::new("parsing");
    pub const RESOLVING_URL: Self = Self::new("resolving_url");
    pub const READING_BODY: Self = Self::new("reading_body");
    pub const URL_RESOLVED: Self = Self::new("url_resolved");
    pub const SINGLE_THREAD_FALLBACK: Self = Self::new("single_thread_fallback");
    pub const INSTALLING_GAME: Self = Self::new("installing_game");
    pub const INSTALLING_MOD: Self = Self::new("installing_mod");
    pub const INSTALLING_NATIVE_MOD: Self = Self::new("installing_native_mod");
    pub const INSTALLING_LEVILAMINA: Self = Self::new("installing_levilamina");
    pub const DOWNLOADING_LEVILAMINA: Self = Self::new("downloading_levilamina");
    pub const RESOLVING_LEVILAMINA: Self = Self::new("resolving_levilamina");
    pub const PREPARING_GAME_INSTALL: Self = Self::new("preparing_game_install");
    pub const INSPECTING_IMPORTS: Self = Self::new("inspecting_imports");
    pub const DELETING_ASSETS: Self = Self::new("deleting_assets");
    pub const DELETING_SCREENSHOT: Self = Self::new("deleting_screenshot");
    pub const UPDATING_SERVERS: Self = Self::new("updating_servers");
    pub const BACKING_UP_UWP: Self = Self::new("backing_up_uwp");
    pub const VERIFYING_UWP_BACKUP: Self = Self::new("verifying_uwp_backup");
    pub const RESOLVING_RUNNER: Self = Self::new("resolving_runner");
    pub const PREPARING_PREFIX: Self = Self::new("preparing_prefix");
    pub const RUNNING_GAME: Self = Self::new("running_game");
    pub const DOWNLOADING_GAME: Self = Self::new("downloading_game");
    pub const RESOLVING_PROTON_GDK: Self = Self::new("resolving_proton_gdk");
    pub const DOWNLOADING_PROTON_GDK: Self = Self::new("downloading_proton_gdk");
    pub const EXTRACTING_PROTON_GDK: Self = Self::new("extracting_proton_gdk");
    pub const AWAITING_LINUX_AUTHORIZATION: Self = Self::new("awaiting_linux_authorization");
    pub const INSTALLING_LINUX_PACKAGES: Self = Self::new("installing_linux_packages");
    pub const MAP_PASTE: Self = Self::new("map_paste");
    pub const MAP_WRITE: Self = Self::new("map_write");
    pub const MAP_DELETE: Self = Self::new("map_delete");
    pub const MAP_COPY: Self = Self::new("map_copy");
    pub const MAP_REFRESH: Self = Self::new("map_refresh");
    pub const MAP_EXPORT: Self = Self::new("map_export");
    pub const MAP_IMPORT: Self = Self::new("map_import");
    pub const MAP_OPEN: Self = Self::new("map_open");
    pub const MAP_INDEX: Self = Self::new("map_index");
    pub const MAP_PROBE_TILES: Self = Self::new("map_probe_tiles");
    pub const MAP_PARTIAL_REFRESH: Self = Self::new("map_partial_refresh");
    pub const MAP_PARSE_OBJ: Self = Self::new("map_parse_obj");
    pub const MAP_READ_MODEL: Self = Self::new("map_read_model");
    pub const MAP_DECODE_IMAGE: Self = Self::new("map_decode_image");
    pub const MAP_SCALE_AND_SLICE: Self = Self::new("map_scale_and_slice");
    pub const MAP_GENERATE_MAPS: Self = Self::new("map_generate_maps");
    pub const MAP_MERGE_PREVIEW: Self = Self::new("map_merge_preview");
    pub const MAP_VOXELIZE: Self = Self::new("map_voxelize");
    pub const MAP_SAVE_PREVIEW: Self = Self::new("map_save_preview");
    pub const MAP_MATCH_BLOCKS: Self = Self::new("map_match_blocks");
    pub const MAP_READ_SOURCE: Self = Self::new("map_read_source");
    pub const MAP_READ_CHUNKS_AND_PACKS: Self = Self::new("map_read_chunks_and_packs");
    pub const MAP_CROP_AND_SLICE: Self = Self::new("map_crop_and_slice");
    pub const MAP_CROP_SCALE_MATCH: Self = Self::new("map_crop_scale_match");
    pub const MAP_WRITE_MCSTRUCTURE: Self = Self::new("map_write_mcstructure");
    pub const MAP_GENERATE_RECORDS: Self = Self::new("map_generate_records");
    pub const MAP_WRITE_BUNDLE: Self = Self::new("map_write_bundle");
    pub const MAP_READ_BUNDLE: Self = Self::new("map_read_bundle");
    pub const MAP_CHECK_TARGET: Self = Self::new("map_check_target");
    pub const MAP_CHECK_FRAME_AREA: Self = Self::new("map_check_frame_area");
    pub const MAP_CREATE_UNDO_SNAPSHOT: Self = Self::new("map_create_undo_snapshot");
    pub const MAP_COMMIT: Self = Self::new("map_commit");
    pub const MAP_SAVE_UNDO_HISTORY: Self = Self::new("map_save_undo_history");
    pub const INSTALLING: Self = Self::new("installing");
    pub const INSPECTING: Self = Self::new("inspecting");
    pub const CHECKING_CONFLICT: Self = Self::new("checking_conflict");
    pub const PREPARING_UPDATE: Self = Self::new("preparing_update");
    pub const INSTALLING_ASSETS: Self = Self::new("installing_assets");
    pub const INSTALLING_PLUGIN: Self = Self::new("installing_plugin");
    pub const UNINSTALLING_PLUGIN: Self = Self::new("uninstalling_plugin");
    pub const MAP_SERIALIZE_MODEL: Self = Self::new("map_serialize_model");
    pub const MAP_BUILD_BLOCK_MESH: Self = Self::new("map_build_block_mesh");
    pub const MAP_OUTPUT_MODEL_ASSETS: Self = Self::new("map_output_model_assets");
    pub const MAP_ENCODE_GLTF: Self = Self::new("map_encode_gltf");
    pub const HISTORY_OPERATION: Self = Self::new("history_operation");
    pub const DELETING_VERSION: Self = Self::new("deleting_version");
    pub const RENAMING_VERSION: Self = Self::new("renaming_version");
    pub const UPDATING_MOD_STATE: Self = Self::new("updating_mod_state");
    pub const UPDATING_MOD_MANIFEST: Self = Self::new("updating_mod_manifest");
    pub const DELETING_MODS: Self = Self::new("deleting_mods");
    pub const INSTALLING_UWP_DEPENDENCIES: Self = Self::new("installing_uwp_dependencies");
    pub const INSTALLING_GAME_INPUT: Self = Self::new("installing_game_input");
    pub const INSTALLING_WINDOWS_APP_SDK: Self = Self::new("installing_windows_app_sdk");
    pub const MAP_MATCH_BLOCKS_LUMINANCE: Self = Self::new("map_match_blocks_luminance");

    pub const COMPLETED: Self = Self::new("completed");
}

impl fmt::Display for TaskStage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl Serialize for TaskStage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.code())
    }
}

impl<'de> Deserialize<'de> for TaskStage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::custom)
    }
}
