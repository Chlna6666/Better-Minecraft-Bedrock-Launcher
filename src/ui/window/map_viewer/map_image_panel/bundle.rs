use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use crate::core::minecraft::map::generator::{
    MapBundleInstallTarget, MapInstallHistory, start_map_bundle_export,
    start_map_bundle_file_install, start_map_bundle_install,
};
use ::bedrock_world::block::BlockState;

impl MapViewerWindowView {
    pub(super) fn install_map_bundle_selected(&mut self, cx: &mut Context<Self>) {
        self.install_map_bundle(self.map_image.install_destination, None, cx);
    }

    pub(super) fn import_map_bundle_file_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = pick_file_path_with_filter("地图包或 Bedrock 容器 NBT", &["nbt"])
        {
            self.install_map_bundle(
                self.map_image.install_destination,
                Some(PathBuf::from(path)),
                cx,
            );
        }
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
        let world_path = self.world_path.clone();
        let refresh_chunks = Arc::new(Mutex::new(BTreeSet::new()));
        let (target, refresh, kind, player_key) = match destination {
            MapImageInstallDestination::Player => {
                let player = self.map_image.selected_player.clone();
                let Some(key) = player.storage_key() else {
                    toast::error(cx, "该玩家存于 level.dat，不能与地图记录原子写入".into());
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
                    toast::error(
                        cx,
                        "请在地图上右键，选择地图写入目标（箱子、潜影盒或空展示框）".into(),
                    );
                    return;
                };
                (
                    MapBundleInstallTarget::Block(chunk, position),
                    MapImageInstallRefresh::Chunks(refresh_chunks.clone()),
                    MapHistoryEntryKind::RecordSave,
                    None,
                )
            }
            MapImageInstallDestination::NewUpFrame => {
                let (x, z) = self.viewport.center_block(self.active_layout);
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
        let history: MapInstallHistory = Box::new(move |ids, affected_chunks| {
            *history_chunks
                .lock()
                .map_err(|_| "地图写入影响范围状态不可用".to_owned())? = affected_chunks.clone();
            let mut raw_keys = ids
                .into_iter()
                .map(|id| format!("map_{id}").into_bytes())
                .collect::<BTreeSet<_>>();
            if let Some(key) = player_key {
                raw_keys.insert(key);
            }
            let capture = capture_before(MapHistoryCaptureSpec {
                kind,
                label: "导入地图包".to_owned(),
                world_path: history_path,
                chunks: affected_chunks,
                raw_keys,
                include_level_dat: false,
            })?;
            Ok(Box::new(move || {
                complete_after(capture, "地图包已写入").map(|_| ())
            }))
        });
        let result = if let Some(input) = input {
            start_map_bundle_file_install(input, world_path, target, history)
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
                .unwrap_or("地图")
                .to_owned();
            start_map_bundle_install(preview_id, world_path, title, target, history)
        };
        match result {
            Ok(task_id) => {
                if let Some(snapshot) = task_manager::get_snapshot(&task_id) {
                    self.task_snapshots
                        .insert(snapshot.id.clone(), Arc::new(snapshot));
                }
                self.map_image.install_task_id = Some(task_id);
                self.map_image.install_refresh = Some(refresh);
                self.status = SharedString::from("正在预检并写入地图包");
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
            .unwrap_or("地图")
            .to_owned();
        let default_name = format!("{title}.bmcbl-map.nbt");
        let Some(path) = pick_save_path_with_filter("BMCBL 地图包", &["nbt"], &default_name)
        else {
            return;
        };
        match start_map_bundle_export(preview_id, PathBuf::from(path), title) {
            Ok(task_id) => {
                if let Some(snapshot) = task_manager::get_snapshot(&task_id) {
                    self.task_snapshots
                        .insert(snapshot.id.clone(), Arc::new(snapshot));
                }
                self.map_image.export_task_id = Some(task_id);
                self.status = SharedString::from("正在导出可移植地图包");
            }
            Err(error) => {
                self.status = SharedString::from(error.clone());
                toast::error(cx, SharedString::from(error));
            }
        }
        cx.notify();
    }
}
