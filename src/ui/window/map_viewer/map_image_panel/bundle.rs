use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use crate::core::minecraft::map::generator::{
    bundle,
    install::{self, MapBundleInstallTarget, MapInstallHistory},
};
use ::bedrock_world::block::BlockState;

impl View {
    /// Explicit read-only check: the write path rechecks world state atomically.
    pub(super) fn check_map_frame_grid(&mut self, cx: &mut Context<Self>) {
        if self.map_image_operation_busy()
            || self.map_image.source.is_none()
            || self.map_image.placement_preview.is_none()
            || self.map_frame_confirmed_center().is_none()
            || self.map_image.picking_frame_center
            || self.map_image.frame_drag_position.is_some()
        {
            return;
        }
        let Some(key) = self.map_frame_preflight_key() else {
            return;
        };
        if self.map_image.frame_check_key.as_deref() == Some(key.as_str())
            && self
                .map_frame_preflight_snapshot()
                .is_some_and(|task| matches!(task.status.as_ref(), "running" | "completed"))
        {
            return;
        }
        if let Some(previous) = self.map_image.frame_check_task_id.take() {
            task_manager::cancel_task(&previous);
        }
        let Some((x, z)) = self.map_frame_confirmed_center() else {
            return;
        };
        let center = BlockPos {
            x,
            y: self.y_layer,
            z,
        };
        let support = BlockState {
            name: self.map_image.frame_support.to_owned(),
            states: BTreeMap::new(),
            version: Some(18_168_865),
        };
        match install::start_frame_grid_preflight(
            self.world_path.clone(),
            center.to_chunk_pos(self.dimension),
            center,
            self.map_image.options.columns,
            self.map_image.options.rows,
            support,
        ) {
            Ok(task_id) => {
                self.map_image.frame_check_key = Some(key);
                self.map_image.frame_check_task_id = Some(task_id.clone());
                if let Some(snapshot) = task_manager::snapshot(&task_id) {
                    self.task_snapshots
                        .insert(snapshot.id.clone(), Arc::new(snapshot));
                }
                self.status = t!("MapViewer.imagegen.bundle.checking_status");
            }
            Err(error) => {
                self.status = SharedString::from(error.clone());
                toast::error(cx, SharedString::from(error));
            }
        }
        cx.notify();
    }

    pub(in super::super) fn install_map_bundle_selected(&mut self, cx: &mut Context<Self>) {
        if self.map_image.install_destination == MapImageInstallDestination::NewUpFrame
            && !self.frame_placement_ready()
        {
            self.check_map_frame_grid(cx);
            return;
        }
        self.install_map_bundle(self.map_image.install_destination, None, cx);
    }

    /// File selection is a read-only staging action, never an implicit world write.
    pub(super) fn import_map_bundle_file_selected(&mut self, cx: &mut Context<Self>) {
        if self.map_image_operation_busy() {
            return;
        }
        if let Some(path) = pick_file_path(
            None,
            t!("MapViewer.imagegen.bundle.filter_name").as_ref(),
            &["nbt"],
        ) {
            self.map_image.pending_bundle = Some(PathBuf::from(path));
            self.status = t!("MapViewer.imagegen.bundle.staged_status");
            self.set_import_workspace_mode(super::super::state::ImportWorkspaceMode::Placement, cx);
            cx.notify();
        }
    }

    pub(super) fn confirm_pending_map_bundle(&mut self, cx: &mut Context<Self>) {
        let Some(input) = self.map_image.pending_bundle.clone() else {
            return;
        };
        self.install_map_bundle(self.map_image.install_destination, Some(input), cx);
    }

    pub(super) fn cancel_pending_map_bundle(&mut self, cx: &mut Context<Self>) {
        if self.map_image_operation_busy() {
            return;
        }
        self.map_image.pending_bundle = None;
        cx.notify();
    }

    fn install_map_bundle(
        &mut self,
        destination: MapImageInstallDestination,
        input: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if self.map_image_operation_busy() {
            return;
        }
        if destination == MapImageInstallDestination::NewUpFrame
            && (self.map_frame_confirmed_center().is_none()
                || self.map_image.picking_frame_center
                || self.map_image.frame_drag_position.is_some())
        {
            toast::error(cx, t!("MapViewer.imagegen.bundle.frame_center_required"));
            return;
        }
        let world_path = self.world_path.clone();
        let refresh_chunks = Arc::new(Mutex::new(BTreeSet::new()));
        let (target, refresh, kind, player_key) = match destination {
            MapImageInstallDestination::Player => {
                let player = self.map_image.selected_player.clone();
                let Some(key) = player.storage_key() else {
                    toast::error(cx, t!("MapViewer.imagegen.bundle.level_dat_player_error"));
                    return;
                };
                let key = key.as_ref().to_vec();
                (
                    MapBundleInstallTarget::Player(player.clone()),
                    MapImageInstallRefresh::Player(player),
                    MapHistoryEntryKind::PlayerEdit,
                    Some(key),
                )
            }
            MapImageInstallDestination::SelectedBlock => {
                let Some((chunk, position)) = self.map_image.selected_block else {
                    toast::error(cx, t!("MapViewer.imagegen.bundle.select_target_error"));
                    return;
                };
                if chunk.dimension != self.dimension {
                    toast::error(
                        cx,
                        t!("MapViewer.imagegen.bundle.target_other_dimension_error"),
                    );
                    return;
                }
                (
                    MapBundleInstallTarget::Block(chunk, position),
                    MapImageInstallRefresh::Chunks(refresh_chunks.clone()),
                    MapHistoryEntryKind::RecordSave,
                    None,
                )
            }
            MapImageInstallDestination::NewUpFrame => {
                let Some((x, z)) = self.map_frame_confirmed_center() else {
                    return;
                };
                let position = BlockPos {
                    x,
                    y: self.y_layer,
                    z,
                };
                let chunk = position.to_chunk_pos(self.dimension);
                let support = BlockState {
                    name: self.map_image.frame_support.to_owned(),
                    states: BTreeMap::new(),
                    version: Some(18_168_865),
                };
                (
                    MapBundleInstallTarget::NewUpFrame(chunk, position, support),
                    MapImageInstallRefresh::Chunks(refresh_chunks.clone()),
                    MapHistoryEntryKind::RecordSave,
                    None,
                )
            }
        };
        let history_path = world_path.clone();
        let history_chunks = refresh_chunks;
        let install_committed = self.map_image.install_committed.clone();
        install_committed.store(false, std::sync::atomic::Ordering::Release);
        let history: MapInstallHistory = Box::new(move |ids, affected_chunks| {
            *history_chunks.lock().map_err(|_| {
                t!("MapViewer.imagegen.bundle.refresh_scope_unavailable").to_string()
            })? = affected_chunks.clone();
            let mut raw_keys = ids
                .into_iter()
                .map(|id| format!("map_{id}").into_bytes())
                .collect::<BTreeSet<_>>();
            if let Some(key) = player_key {
                raw_keys.insert(key);
            }
            let capture = capture_before(MapHistoryCaptureSpec {
                kind,
                label: t!("MapViewer.imagegen.bundle.history_import_label").to_string(),
                world_path: history_path,
                chunks: affected_chunks,
                raw_keys,
                include_level_dat: false,
            })?;
            Ok(Box::new(move || {
                install_committed.store(true, std::sync::atomic::Ordering::Release);
                complete_after(
                    capture,
                    t!("MapViewer.imagegen.bundle.bundle_written").to_string(),
                )
                .map(|_| ())
            }))
        });
        let result = if let Some(input) = input {
            install::start_bundle_file(input, world_path, target, history)
        } else {
            let Some(preview_id) = self.map_image.task_id.as_deref() else {
                return;
            };
            let Some(source) = self.map_image.source.as_ref() else {
                return;
            };
            let title = source
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_owned)
                .unwrap_or_else(|| t!("MapViewer.imagegen.bundle.default_title").to_string());
            install::start_bundle(preview_id, world_path, title, target, history)
        };
        match result {
            Ok(task_id) => {
                // Clear the staged input only once a visible, cancellable Task owns the write.
                self.map_image.pending_bundle = None;
                if let Some(snapshot) = task_manager::snapshot(&task_id) {
                    self.task_snapshots
                        .insert(snapshot.id.clone(), Arc::new(snapshot));
                }
                self.map_image.install_task_id = Some(task_id);
                self.map_image.install_refresh = Some(refresh);
                self.status = t!("MapViewer.imagegen.bundle.writing_status");
            }
            Err(error) => {
                self.status = SharedString::from(error.clone());
                toast::error(cx, SharedString::from(error));
            }
        }
        cx.notify();
    }

    pub(super) fn export_map_bundle(&mut self, cx: &mut Context<Self>) {
        if self.map_image_operation_busy() {
            return;
        }
        let Some(preview_id) = self.map_image.task_id.as_deref() else {
            return;
        };
        let Some(source) = self.map_image.source.as_ref() else {
            return;
        };
        let title = source
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| t!("MapViewer.imagegen.bundle.default_title").to_string());
        let default_name = format!("{title}.bmcbl-map.nbt");
        let Some(path) = pick_save_path(
            t!("MapViewer.imagegen.bundle.save_filter_name").as_ref(),
            &["nbt"],
            &default_name,
        ) else {
            return;
        };
        match bundle::start_export(preview_id, PathBuf::from(path), title) {
            Ok(task_id) => {
                if let Some(snapshot) = task_manager::snapshot(&task_id) {
                    self.task_snapshots
                        .insert(snapshot.id.clone(), Arc::new(snapshot));
                }
                self.map_image.export_task_id = Some(task_id);
                self.status = t!("MapViewer.imagegen.bundle.exporting_status");
            }
            Err(error) => {
                self.status = SharedString::from(error.clone());
                toast::error(cx, SharedString::from(error));
            }
        }
        cx.notify();
    }
}
