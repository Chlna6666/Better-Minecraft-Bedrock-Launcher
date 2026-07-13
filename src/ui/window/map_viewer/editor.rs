use super::helpers::*;
use super::mcstructure;
use super::model::*;
use super::panels::*;
use super::players::*;
use super::prelude::*;
use super::tile_state::*;

impl MapViewerWindowView {
    pub(super) fn handle_editor_event(
        &mut self,
        editor: Entity<CodeEditorState>,
        event: &CodeEditorEvent,
        cx: &mut Context<Self>,
    ) {
        if editor.entity_id() != self.editor_state.entity_id() {
            return;
        }
        match event {
            CodeEditorEvent::Change => {
                self.editor_document.text = editor.read(cx).value();
                self.editor_document.dirty = true;
                cx.notify();
            }
            CodeEditorEvent::PointerInteractionStarted => {
                self.cancel_pointer_captures_for_panel_interaction("code editor pointer down", cx);
            }
            CodeEditorEvent::PointerInteractionEnded => {
                self.release_pointer_captures("code editor pointer up", cx);
            }
            CodeEditorEvent::SaveRequested => self.request_editor_save(cx),
            CodeEditorEvent::FormatRequested => self.format_editor_document(cx),
        }
    }

    pub(super) fn format_editor_document(&mut self, cx: &mut Context<Self>) {
        let text = self.editor_state.read(cx).value();
        let Ok(value) = serde_json::from_str::<serde_json::Value>(text.as_ref()) else {
            self.status = SharedString::from("JSON 格式化失败：当前文本不是有效 JSON");
            cx.notify();
            return;
        };
        let formatted = pretty_json(value);
        self.editor_state.update(cx, |editor, cx| {
            editor.set_value(formatted.clone(), cx);
        });
        self.editor_document.text = formatted;
        self.editor_document.dirty = true;
        self.status = SharedString::from("JSON 已格式化；第一版不会从文本回写任意 NBT");
        cx.notify();
    }

    pub(super) fn load_edit_detail(&mut self, target: EditTarget, cx: &mut Context<Self>) {
        self.context_menu = None;
        self.ui_state.active_right_panel = MapViewerRightPanel::Nbt;
        self.ui_state.set_right_panel_open(true);
        self.editor_document.loading = true;
        self.editor_document.saving = false;
        self.editor_document.dirty = false;
        self.editor_document.target = Some(target.clone());
        self.editor_document.title = SharedString::from(target.operation_label());
        self.professional.edit_loading = true;
        self.professional.edit_generation = self.professional.edit_generation.saturating_add(1);
        let edit_generation = self.professional.edit_generation;
        let metadata_generation = self.metadata_generation;
        let world_path = self.world_path.clone();
        let target_for_task = target.clone();
        self.status = SharedString::from(format!("正在读取 {}...", target.operation_label()));
        cx.notify();

        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    let editor = MapWorldEditor::open_with_options(
                        &world_path,
                        bedrock_world::OpenOptions::default(),
                    )
                    .map_err(|error| error.to_string())?;
                    load_edit_detail_blocking(&editor, target_for_task)
                        .map_err(|error| error.to_string())
                })
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if this.metadata_generation != metadata_generation
                    || this.professional.edit_generation != edit_generation
                {
                    return;
                }
                this.professional.edit_loading = false;
                match result {
                    Ok(detail) => {
                        this.set_professional_detail(Some(detail), cx);
                        this.status = SharedString::from("编辑记录已加载");
                    }
                    Err(error) => {
                        this.status = SharedString::from(error);
                    }
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn request_editor_save(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self
            .professional
            .detail
            .as_ref()
            .and_then(ProfessionalDetail::edit_target)
        else {
            self.status = SharedString::from("没有可保存的编辑记录");
            cx.notify();
            return;
        };
        self.confirm_or_run_edit(target, EditAction::Save, cx);
    }

    pub(super) fn request_editor_delete(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self
            .professional
            .detail
            .as_ref()
            .and_then(ProfessionalDetail::edit_target)
        else {
            self.status = SharedString::from("没有可删除的编辑记录");
            cx.notify();
            return;
        };
        self.confirm_or_run_edit(target, EditAction::Delete, cx);
    }

    pub(super) fn confirm_or_run_edit(
        &mut self,
        target: EditTarget,
        action: EditAction,
        cx: &mut Context<Self>,
    ) {
        if !self.professional.write_mode {
            self.status = SharedString::from("编辑世界记录需要先开启写入模式");
            cx.notify();
            return;
        }
        let pending_matches = self
            .professional
            .pending_edit_confirmation
            .as_ref()
            .is_some_and(|pending| pending.target == target && pending.action == action);
        if !pending_matches {
            self.professional.pending_edit_confirmation = Some(PendingEditConfirmation {
                target: target.clone(),
                action: action.clone(),
            });
            self.status = SharedString::from(format!(
                "再次点击以确认 {}",
                edit_action_status(&action, &target)
            ));
            cx.notify();
            return;
        }
        self.professional.pending_edit_confirmation = None;
        self.run_confirmed_edit(target, action, cx);
    }

    pub(super) fn run_confirmed_edit(
        &mut self,
        target: EditTarget,
        action: EditAction,
        cx: &mut Context<Self>,
    ) {
        self.editor_document.saving = true;
        self.professional.edit_loading = true;
        self.professional.edit_generation = self.professional.edit_generation.saturating_add(1);
        let edit_generation = self.professional.edit_generation;
        let metadata_generation = self.metadata_generation;
        let world_path = self.world_path.clone();
        let target_for_task = target.clone();
        let action_for_task = action.clone();
        let document_text = matches!(action, EditAction::Save)
            .then(|| self.editor_state.read(cx).value().to_string());
        self.status =
            SharedString::from(format!("正在{}...", edit_action_status(&action, &target)));
        cx.notify();

        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    let editor = MapWorldEditor::open_writable(&world_path)
                        .map_err(|error| error.to_string())?;
                    let operation =
                        format!("{} via BMCBL map_viewer", target_for_task.operation_label());
                    let _guard = WriteGuard::confirmed(world_path.clone(), operation);
                    let history_capture = capture_before(edit_history_spec(
                        &world_path,
                        &target_for_task,
                        &action_for_task,
                    )?);
                    let result = run_edit_action_blocking(
                        &editor,
                        target_for_task,
                        action_for_task,
                        document_text,
                    )
                    .map_err(|error| error.to_string());
                    match (history_capture, result) {
                        (Ok(capture), Ok(invalidation)) => {
                            complete_after(capture, "世界记录已写入并刷新地图状态")?;
                            Ok(invalidation)
                        }
                        (Ok(capture), Err(error)) => {
                            let _ = complete_failed(capture, error.clone());
                            Err(error)
                        }
                        (Err(error), Ok(invalidation)) => {
                            tracing::warn!(%error, "map history capture failed after record edit");
                            Ok(invalidation)
                        }
                        (Err(history_error), Err(write_error)) => {
                            Err(format!("{write_error}；历史捕获失败: {history_error}"))
                        }
                    }
                })
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if this.metadata_generation != metadata_generation
                    || this.professional.edit_generation != edit_generation
                {
                    return;
                }
                this.professional.edit_loading = false;
                this.editor_document.saving = false;
                match result {
                    Ok(invalidation) => {
                        this.apply_map_edit_invalidation(&invalidation, cx);
                        this.status = SharedString::from("世界记录已写入并刷新地图状态");
                    }
                    Err(error) => {
                        this.status = SharedString::from(error);
                    }
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn apply_map_edit_invalidation(
        &mut self,
        invalidation: &MapEditInvalidation,
        cx: &mut Context<Self>,
    ) {
        self.apply_map_edit_invalidation_with_tile_priority(
            invalidation,
            TilePriority::Visible,
            cx,
        );
    }

    pub(super) fn apply_map_edit_invalidation_with_tile_priority(
        &mut self,
        invalidation: &MapEditInvalidation,
        tile_priority: TilePriority,
        cx: &mut Context<Self>,
    ) {
        self.apply_map_edit_invalidation_with_options(invalidation, tile_priority, false, cx);
    }

    pub(super) fn apply_map_edit_invalidation_with_options(
        &mut self,
        invalidation: &MapEditInvalidation,
        tile_priority: TilePriority,
        reuse_known_tile_index: bool,
        cx: &mut Context<Self>,
    ) {
        let affected_chunks = invalidation.affected_chunks().clone();
        self.record_manifest_probe_edit(&affected_chunks, invalidation);
        if invalidation.clear_tile_cache() {
            self.invalidate_tiles_for_chunks_with_options(
                &affected_chunks,
                tile_priority,
                reuse_known_tile_index,
                cx,
            );
        }
        if invalidation.refresh_metadata() {
            self.refresh_chunk_tree_for_chunks(&affected_chunks);
        }
        if invalidation.refresh_overlays() {
            self.cancel_professional_overlay_query();
            self.professional.overlays = None;
            self.professional.overlay_paint = None;
            self.professional.selection_stats = None;
            self.professional.pending_overlay_refresh = true;
            self.refresh_professional_overlays(cx);
        }
        self.set_professional_detail(None, cx);
        self.refresh_chunk_tree_if_selected();
    }

    fn record_manifest_probe_edit(
        &mut self,
        chunks: &BTreeSet<ChunkPos>,
        invalidation: &MapEditInvalidation,
    ) {
        let affected_tiles = tile_coords_for_chunks(chunks, self.active_layout);
        self.manifest_probe_diagnostics.record_edit(format!(
            "chunks={} tiles={} cache={} metadata={} overlays={}",
            chunks.len(),
            affected_tiles.len(),
            invalidation.clear_tile_cache(),
            invalidation.refresh_metadata(),
            invalidation.refresh_overlays()
        ));
    }

    pub(super) fn invalidate_tiles_for_chunks_with_options(
        &mut self,
        chunks: &BTreeSet<ChunkPos>,
        tile_priority: TilePriority,
        reuse_known_tile_index: bool,
        cx: &mut Context<Self>,
    ) {
        if chunks.is_empty() {
            return;
        }
        let affected_tiles = tile_coords_for_chunks(chunks, self.active_layout);
        if affected_tiles.is_empty() {
            self.invalidate_tiles(cx);
            self.refresh_render_session_after_edit(
                Vec::new(),
                chunks.clone(),
                tile_priority,
                reuse_known_tile_index,
                cx,
            );
            self.refresh_chunk_tree_if_selected();
            return;
        }
        for coord in &affected_tiles {
            if reuse_known_tile_index {
                merge_chunks_into_tile_index(
                    &mut self.tile_chunk_index,
                    *coord,
                    chunks,
                    self.active_layout,
                );
            }
            self.available_tiles.remove(coord);
            if !reuse_known_tile_index {
                self.manifest_scanned_tiles.remove(coord);
            }
            if !reuse_known_tile_index {
                Self::drop_render_image(self.tile_manager.remove_tile(*coord), cx);
            }
        }
        if !reuse_known_tile_index {
            let colors = self.theme_colors(cx);
            self.remove_canvas_tiles(&affected_tiles, colors, cx);
        }
        self.refresh_render_session_after_edit(
            affected_tiles,
            chunks.clone(),
            tile_priority,
            reuse_known_tile_index,
            cx,
        );
        self.refresh_chunk_tree_if_selected();
    }

    fn refresh_chunk_tree_for_chunks(&mut self, chunks: &BTreeSet<ChunkPos>) {
        let Some(selected_tile) = self.db_tree.selected_tile else {
            return;
        };
        let selected_tile_set = [selected_tile].into_iter().collect::<BTreeSet<_>>();
        let affected_tiles = tile_coords_for_chunks(chunks, self.active_layout)
            .into_iter()
            .collect::<BTreeSet<_>>();
        if affected_tiles.contains(&selected_tile)
            || affected_tiles.is_empty()
            || selected_tile_set.is_subset(&affected_tiles)
        {
            self.refresh_chunk_tree_if_selected();
        }
    }

    pub(super) fn highlight_slime_window(
        &mut self,
        window: SlimeChunkWindow,
        cx: &mut Context<Self>,
    ) {
        self.viewport.center_on_block(
            window.center.x * 16 + 8,
            window.center.z * 16 + 8,
            self.active_layout,
        );
        self.professional.highlighted_window = Some(window);
        self.ensure_visible_tiles(cx);
        self.refresh_professional_render_caches();
        self.refresh_professional_overlays(cx);
        cx.notify();
    }

    pub(super) fn query_context_block(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.context_menu else {
            return;
        };
        self.context_menu = None;
        self.status = SharedString::from("正在查询方块信息...");
        cx.notify();

        let generation = self.metadata_generation;
        let world_path = self.world_path.clone();
        let dimension = self.dimension;
        let block = BlockPos {
            x: menu.block_x,
            y: self.y_layer,
            z: menu.block_z,
        };
        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    let world = BedrockWorld::open_blocking(
                        &world_path,
                        bedrock_world::OpenOptions::default(),
                    )
                    .map_err(|error| error.to_string())?;
                    let tip = query_block_tip_blocking(&world, block, dimension)
                        .map_err(|error| error.to_string())?;
                    Ok::<_, String>(block_tip_detail(tip))
                })
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if this.metadata_generation != generation {
                    return;
                }
                match result {
                    Ok(detail) => {
                        this.set_professional_detail(Some(detail), cx);
                        this.status = SharedString::from("方块查询完成");
                    }
                    Err(error) => this.status = SharedString::from(error),
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn open_context_chunk_detail(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.context_menu else {
            return;
        };
        let chunk = context_menu_chunk(menu, self.dimension);
        self.context_menu = None;
        self.status = SharedString::from("正在读取 chunk 详情...");
        cx.notify();

        let generation = self.metadata_generation;
        let world_path = self.world_path.clone();
        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    let world = BedrockWorld::open_blocking(
                        &world_path,
                        bedrock_world::OpenOptions::default(),
                    )
                    .map_err(|error| error.to_string())?;
                    let detail = query_chunk_detail_blocking(&world, chunk)
                        .map_err(|error| error.to_string())?;
                    Ok::<_, String>(chunk_detail_panel(detail))
                })
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if this.metadata_generation != generation {
                    return;
                }
                match result {
                    Ok(detail) => {
                        this.set_professional_detail(Some(detail), cx);
                        this.status = SharedString::from("chunk 详情已打开");
                    }
                    Err(error) => this.status = SharedString::from(error),
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn query_selection_stats(&mut self, cx: &mut Context<Self>) {
        let Some(bounds) = self.professional_query_bounds() else {
            self.status = SharedString::from("当前没有可查询的视口或选区");
            cx.notify();
            return;
        };
        self.context_menu = None;
        self.status = SharedString::from("正在统计专业查询选区...");
        cx.notify();

        let generation = self.metadata_generation;
        let world_path = self.world_path.clone();
        let options = self.professional_overlay_query_options();
        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    let world = BedrockWorld::open_blocking(
                        &world_path,
                        bedrock_world::OpenOptions::default(),
                    )
                    .map_err(|error| error.to_string())?;
                    let stats = query_selection_stats_blocking(&world, bounds, options)
                        .map_err(|error| error.to_string())?;
                    Ok::<_, String>((stats.clone(), selection_stats_panel(stats)))
                })
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if this.metadata_generation != generation {
                    return;
                }
                match result {
                    Ok((stats, detail)) => {
                        this.professional.selection_stats = Some(stats);
                        this.set_professional_detail(Some(detail), cx);
                        this.status = SharedString::from("选区统计完成");
                    }
                    Err(error) => this.status = SharedString::from(error),
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn delete_selection_chunks(&mut self, cx: &mut Context<Self>) {
        if !self.professional.write_mode {
            self.status = SharedString::from("删除 chunk 需要先开启写入模式");
            toast::error(cx, self.status.clone());
            cx.notify();
            return;
        }
        let Some(selection) = self.professional.selection else {
            self.status = SharedString::from("删除 chunk 需要先设置选区");
            toast::error(cx, self.status.clone());
            cx.notify();
            return;
        };
        if !self.professional.pending_delete_confirmation {
            self.professional.pending_delete_confirmation = true;
            self.status = SharedString::from("再次点击删除选区 chunk 以确认写入操作");
            cx.notify();
            return;
        }
        self.professional.pending_delete_confirmation = false;
        self.context_menu = None;
        let bounds = selection.bounds();
        let affected_chunks = (bounds.min_chunk_z..=bounds.max_chunk_z)
            .flat_map(|chunk_z| {
                (bounds.min_chunk_x..=bounds.max_chunk_x).map(move |chunk_x| ChunkPos {
                    x: chunk_x,
                    z: chunk_z,
                    dimension: bounds.dimension,
                })
            })
            .collect::<BTreeSet<_>>();
        let affected_chunk_list = affected_chunks.iter().copied().collect::<Vec<_>>();
        let progress_total = affected_chunk_list.len().max(1);
        let task_id = task_manager::create_task_with_details(
            None,
            "删除选区区块",
            Some(format!("{progress_total} 个 chunk")),
            "map_delete",
            Some(task_progress_units(progress_total)),
            false,
        );
        let cancel = CancelFlag::new();
        task_manager::register_task_cancel_hook(task_id.clone(), {
            let cancel = cancel.clone();
            move || cancel.cancel()
        });
        self.set_chunk_transfer_progress(ChunkTransferProgress {
            phase: SharedString::from("删除区块"),
            completed: 0,
            total: progress_total,
        });
        self.begin_task_toast(
            &task_id,
            SharedString::from(format!("正在删除 {progress_total} 个选区 chunk...")),
            cx,
        );
        self.status = SharedString::from("正在删除选区 chunk records...");
        cx.notify();

        let generation = self.metadata_generation;
        let world_path = self.world_path.clone();
        cx.spawn(async move |handle, cx| {
            enum DeleteSelectionEvent {
                Progress(ChunkTransferProgress),
                Complete(Result<usize, String>),
            }

            let (event_sender, mut event_receiver) = unbounded::<DeleteSelectionEvent>();
            let progress_sender = event_sender.clone();
            let completion_sender = event_sender.clone();
            let affected_chunks_for_task = affected_chunks.clone();
            let task_id_for_background = task_id.clone();
            let cancel_for_background = cancel.clone();
            let task = cx.background_spawn(async move {
                let task_id_for_task = task_id_for_background;
                let cancel_for_task = cancel_for_background;
                let result = (|| {
                    check_map_operation_cancelled(&cancel_for_task, &task_id_for_task)?;
                    let mut options = bedrock_world::OpenOptions::default();
                    options.read_only = false;
                    let world = BedrockWorld::open_blocking(&world_path, options)
                        .map_err(|error| error.to_string())?;
                    let operation = format!(
                        "clear chunks dim={} x={}..{} z={}..{}",
                        bounds.dimension.id(),
                        bounds.min_chunk_x,
                        bounds.max_chunk_x,
                        bounds.min_chunk_z,
                        bounds.max_chunk_z
                    );
                    let guard = WriteGuard::confirmed(world_path.clone(), operation);
                    let history_capture = capture_before(MapHistoryCaptureSpec {
                        kind: MapHistoryEntryKind::ChunkDelete,
                        label: format!("删除选区 {} 个 chunk（清空为空气）", progress_total),
                        world_path: world_path.clone(),
                        chunks: affected_chunks_for_task.clone(),
                        raw_keys: BTreeSet::new(),
                        include_level_dat: false,
                    });
                    let total = affected_chunk_list.len().max(1);
                    let mut cleared = 0usize;
                    let result = (|| {
                        let mut progress_sync = ChunkTransferTaskProgressSync::default();
                        for (index, chunk) in affected_chunk_list.into_iter().enumerate() {
                            check_map_operation_cancelled(&cancel_for_task, &task_id_for_task)?;
                            let bounds = SlimeChunkBounds {
                                dimension: chunk.dimension,
                                min_chunk_x: chunk.x,
                                max_chunk_x: chunk.x,
                                min_chunk_z: chunk.z,
                                max_chunk_z: chunk.z,
                            };
                            cleared = cleared.saturating_add(
                                clear_chunks_blocking(&world, bounds, &guard)
                                    .map_err(|error| error.to_string())?,
                            );
                            let progress = ChunkTransferProgress {
                                phase: SharedString::from("删除区块"),
                                completed: index + 1,
                                total,
                            };
                            sync_map_operation_task_progress(
                                &task_id_for_task,
                                &mut progress_sync,
                                &progress,
                                "map_delete",
                            );
                            if progress_sender
                                .unbounded_send(DeleteSelectionEvent::Progress(progress))
                                .is_err()
                            {
                                break;
                            }
                        }
                        Ok::<usize, String>(cleared)
                    })();
                    match (history_capture, result) {
                        (Ok(capture), Ok(cleared)) => {
                            complete_after(capture, format!("已清空 {cleared} 个选区 chunk 为空气"))?;
                            Ok(cleared)
                        }
                        (Ok(capture), Err(error)) => {
                            let _ = complete_failed(capture, error.clone());
                            Err(error)
                        }
                        (Err(error), Ok(cleared)) => {
                            tracing::warn!(%error, "map history capture failed after selection delete");
                            Ok(cleared)
                        }
                        (Err(history_error), Err(write_error)) => Err(format!(
                            "{write_error}；历史捕获失败: {history_error}"
                        )),
                    }
                })();
                let status =
                    map_operation_status_for_result(&result, &cancel_for_task, &task_id_for_task);
                let message = match (&result, status) {
                    (Ok(cleared), "completed") => Some(format!("已清空 {cleared} 个 chunk")),
                    (Err(error), _) => Some(error.clone()),
                    _ => None,
                };
                task_manager::finish_task(&task_id_for_task, status, message);
                if completion_sender
                    .unbounded_send(DeleteSelectionEvent::Complete(result))
                    .is_err()
                {
                    tracing::debug!("delete selection completion receiver dropped");
                }
            });
            task.detach();
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            while let Some(event) = event_receiver.next().await {
                let is_complete = matches!(&event, DeleteSelectionEvent::Complete(_));
                let task_id_for_ui = task_id.clone();
                view.update(cx, {
                    let affected_chunks = affected_chunks.clone();
                    move |this, cx| {
                        if this.metadata_generation != generation {
                            return;
                        }
                        match event {
                            DeleteSelectionEvent::Progress(progress) => {
                                this.set_chunk_transfer_progress(progress);
                            }
                            DeleteSelectionEvent::Complete(result) => {
                                match result {
                                    Ok(cleared) => {
                                        this.complete_chunk_transfer_progress();
                                        let invalidation =
                                            MapEditInvalidation::chunks(affected_chunks)
                                                .with_metadata();
                                        this.apply_map_edit_invalidation_with_tile_priority(
                                            &invalidation,
                                            TilePriority::EditRefresh,
                                            cx,
                                        );
                                        let message =
                                            format!("已清空 {cleared} 个选区 chunk 为空气");
                                        this.status = SharedString::from(message.clone());
                                        this.resolve_task_toast(
                                            &task_id_for_ui,
                                            toast::ToastKind::Success,
                                            SharedString::from(message),
                                            cx,
                                        );
                                    }
                                    Err(error) => {
                                        this.finish_chunk_transfer_progress();
                                        if is_map_operation_cancelled_error(&error) {
                                            let message = SharedString::from("删除选区已取消");
                                            this.status = message.clone();
                                            this.resolve_task_toast(
                                                &task_id_for_ui,
                                                toast::ToastKind::Info,
                                                message,
                                                cx,
                                            );
                                        } else {
                                            this.status = SharedString::from(error.clone());
                                            this.resolve_task_toast(
                                                &task_id_for_ui,
                                                toast::ToastKind::Error,
                                                SharedString::from(error),
                                                cx,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        cx.notify();
                    }
                })?;
                if is_complete {
                    break;
                }
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn run_quick_write_action(
        &mut self,
        action: QuickWriteAction,
        cx: &mut Context<Self>,
    ) {
        if !self.professional.write_mode && !action.is_paste() {
            self.status = SharedString::from("快捷写入需要先开启写入模式");
            toast::error(cx, self.status.clone());
            cx.notify();
            return;
        }
        let pending_matches = self
            .professional
            .pending_quick_write_confirmation
            .as_ref()
            .is_some_and(|pending| pending == &action);
        if !pending_matches {
            self.professional.pending_quick_write_confirmation = Some(action.clone());
            self.professional.paste_preview = self.paste_preview_for_action(&action);
            self.status = SharedString::from(format!(
                "再次点击以确认 {} · 写入后会刷新对应瓦片",
                action.label()
            ));
            let colors = self.theme_colors(cx);
            self.sync_canvas_snapshot(colors, cx);
            cx.notify();
            return;
        }
        self.professional.pending_quick_write_confirmation = None;
        self.professional.paste_preview = None;
        self.replace_paste_preview_images(Vec::new(), cx);
        self.context_menu = None;
        let colors = self.theme_colors(cx);
        self.sync_canvas_snapshot(colors, cx);
        let task_stage = match &action {
            QuickWriteAction::PasteCopiedChunk { .. }
            | QuickWriteAction::PasteCopiedChunks { .. }
            | QuickWriteAction::PasteImportedStructure { .. } => "map_paste",
            QuickWriteAction::DeleteCurrentChunk(_)
            | QuickWriteAction::DeleteCurrentChunkBlockEntities(_)
            | QuickWriteAction::DeleteCurrentChunkActors(_) => "map_delete",
            QuickWriteAction::ResetCurrentChunk(_) => "map_write",
        };
        let progress = action
            .progress_seed()
            .map(|(phase, total)| ChunkTransferProgress {
                phase: SharedString::from(phase),
                completed: 0,
                total,
            });
        let task_total = progress
            .as_ref()
            .map(|progress| task_progress_units(progress.total.max(1)));
        let task_id = task_manager::create_task_with_details(
            None,
            action.label(),
            None,
            task_stage,
            task_total,
            false,
        );
        let cancel = CancelFlag::new();
        task_manager::register_task_cancel_hook(task_id.clone(), {
            let cancel = cancel.clone();
            move || cancel.cancel()
        });
        if let Some(progress) = progress {
            self.set_chunk_transfer_progress(progress);
        } else {
            self.finish_chunk_transfer_progress();
        }
        self.begin_task_toast(
            &task_id,
            SharedString::from(format!("正在{}...", action.label())),
            cx,
        );
        self.status = SharedString::from(format!("正在{}...", action.label()));
        cx.notify();

        let generation = self.metadata_generation;
        let world_path = self.world_path.clone();
        let tile_refresh_priority = if action.prioritizes_tile_refresh() {
            TilePriority::EditRefresh
        } else {
            TilePriority::Visible
        };
        let reuse_known_tile_index = action.reuses_known_tile_index_after_write();
        let action_for_task = action.clone();
        let copied_chunk = self.professional.copied_chunk.clone();
        let imported_structure = self.professional.imported_structure.clone();
        cx.spawn(async move |handle, cx| {
            enum QuickWriteEvent {
                Progress(ChunkTransferProgress),
                Complete(Result<(String, MapEditInvalidation), String>),
            }

            let (event_sender, mut event_receiver) = unbounded::<QuickWriteEvent>();
            let progress_sender = event_sender.clone();
            let completion_sender = event_sender.clone();
            let task_id_for_background = task_id.clone();
            let cancel_for_background = cancel.clone();
            let task = cx.background_spawn(async move {
                let task_id_for_task = task_id_for_background;
                let cancel_for_task = cancel_for_background;
                let result = (|| {
                    check_map_operation_cancelled(&cancel_for_task, &task_id_for_task)?;
                    let mut options = bedrock_world::OpenOptions::default();
                    options.read_only = false;
                    let world = BedrockWorld::open_blocking(&world_path, options)
                        .map_err(|error| error.to_string())?;
                    let operation = format!("{} via BMCBL map_viewer", action_for_task.label());
                    let guard = WriteGuard::confirmed(world_path.clone(), operation);
                    let history_capture = capture_before(quick_write_history_spec(
                        &world_path,
                        &action_for_task,
                        copied_chunk.as_ref(),
                        imported_structure.as_ref(),
                    )?);
                    let mut progress_sync = ChunkTransferTaskProgressSync::default();
                    let result = run_quick_write_action_blocking(
                        &world,
                        action_for_task.clone(),
                        &guard,
                        copied_chunk.as_ref(),
                        imported_structure.as_ref(),
                        Some(&cancel_for_task),
                        |progress| {
                            sync_map_operation_task_progress(
                                &task_id_for_task,
                                &mut progress_sync,
                                &progress,
                                task_stage,
                            );
                            let _ =
                                progress_sender.unbounded_send(QuickWriteEvent::Progress(progress));
                        },
                    )
                    .map_err(|error| error.to_string());
                    match (history_capture, result) {
                        (Ok(capture), Ok((message, invalidation))) => {
                            complete_after(capture, message.clone())?;
                            Ok((message, invalidation))
                        }
                        (Ok(capture), Err(error)) => {
                            let _ = complete_failed(capture, error.clone());
                            Err(error)
                        }
                        (Err(error), Ok((message, invalidation))) => {
                            tracing::warn!(%error, "map history capture failed after quick write");
                            Ok((message, invalidation))
                        }
                        (Err(history_error), Err(write_error)) => {
                            Err(format!("{write_error}；历史捕获失败: {history_error}"))
                        }
                    }
                })();
                let status =
                    map_operation_status_for_result(&result, &cancel_for_task, &task_id_for_task);
                let message = match (&result, status) {
                    (Ok((message, _)), "completed") => Some(message.clone()),
                    (Err(error), _) => Some(error.clone()),
                    _ => None,
                };
                task_manager::finish_task(&task_id_for_task, status, message);
                if completion_sender
                    .unbounded_send(QuickWriteEvent::Complete(result))
                    .is_err()
                {
                    tracing::debug!("quick write completion receiver dropped");
                }
            });
            task.detach();
            while let Some(event) = event_receiver.next().await {
                let is_complete = matches!(&event, QuickWriteEvent::Complete(_));
                let task_id_for_ui = task_id.clone();
                let Some(view) = handle.upgrade() else {
                    return Ok(());
                };
                view.update(cx, move |this, cx| {
                    if this.metadata_generation != generation {
                        return;
                    }
                    match event {
                        QuickWriteEvent::Progress(progress) => {
                            this.set_chunk_transfer_progress(progress);
                        }
                        QuickWriteEvent::Complete(result) => match result {
                            Ok((message, invalidation)) => {
                                this.complete_chunk_transfer_progress();
                                this.apply_map_edit_invalidation_with_options(
                                    &invalidation,
                                    tile_refresh_priority,
                                    reuse_known_tile_index,
                                    cx,
                                );
                                this.status = SharedString::from(message.clone());
                                this.resolve_task_toast(
                                    &task_id_for_ui,
                                    toast::ToastKind::Success,
                                    SharedString::from(message),
                                    cx,
                                );
                            }
                            Err(error) => {
                                this.finish_chunk_transfer_progress();
                                if is_map_operation_cancelled_error(&error) {
                                    let message = SharedString::from("地图写入已取消");
                                    this.status = message.clone();
                                    this.resolve_task_toast(
                                        &task_id_for_ui,
                                        toast::ToastKind::Info,
                                        message,
                                        cx,
                                    );
                                } else {
                                    this.status = SharedString::from(error.clone());
                                    this.resolve_task_toast(
                                        &task_id_for_ui,
                                        toast::ToastKind::Error,
                                        SharedString::from(error),
                                        cx,
                                    );
                                }
                            }
                        },
                    }
                    cx.notify();
                })?;
                if is_complete {
                    break;
                }
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn paste_preview_for_action(&self, action: &QuickWriteAction) -> Option<PastePreview> {
        let (source_anchor, target_anchor, transform) = match action {
            QuickWriteAction::PasteCopiedChunks {
                source_anchor,
                target_anchor,
                transform,
                ..
            }
            | QuickWriteAction::PasteImportedStructure {
                source_anchor,
                target_anchor,
                transform,
                ..
            }
            | QuickWriteAction::PasteCopiedChunk {
                source: source_anchor,
                target: target_anchor,
                transform,
            } => (*source_anchor, *target_anchor, *transform),
            _ => return None,
        };
        let copied_chunk = self.professional.copied_chunk.as_ref()?;
        Some(PastePreview {
            source_anchor,
            target_anchor,
            rotation: transform.rotation,
            transform,
            display_degrees: paste_rotation_degrees(transform.rotation),
            drag: None,
            targets: pasted_chunk_targets(copied_chunk, source_anchor, target_anchor, transform),
            tools_expanded: false,
            auto_pan: None,
        })
    }
}

pub(super) fn block_tip_detail(tip: bedrock_world::BlockTip) -> ProfessionalDetail {
    let title = SharedString::from(format!(
        "方块 {}, {}, {}",
        tip.block.x, tip.block.y, tip.block.z
    ));
    let value = serde_json::json!({
        "block": {
            "x": tip.block.x,
            "y": tip.block.y,
            "z": tip.block.z,
        },
        "chunk": {
            "x": tip.chunk.x,
            "z": tip.chunk.z,
            "dimension": tip.chunk.dimension.id(),
        },
        "local": {
            "x": tip.local_x,
            "z": tip.local_z,
        },
        "height": tip.height,
        "biome_id": tip.biome_id,
        "is_slime_chunk": tip.is_slime_chunk,
        "surface": tip.surface.as_ref().map(|surface| format!("{surface:?}")),
    });
    ProfessionalDetail::BlockTip {
        title,
        json: pretty_json(value),
    }
}

pub(super) fn chunk_detail_panel(detail: ChunkDetail) -> ProfessionalDetail {
    let title = SharedString::from(format!(
        "Chunk {}, {} · {}",
        detail.pos.x,
        detail.pos.z,
        dimension_label(detail.pos.dimension)
    ));
    let records = detail
        .records
        .iter()
        .map(|record| {
            serde_json::json!({
                "tag": format!("{:?}", record.tag),
                "raw_value_len": record.raw_value_len,
                "writable_nbt": record.writable_nbt,
                "roots": record.roots,
            })
        })
        .collect::<Vec<_>>();
    ProfessionalDetail::Chunk {
        title,
        json: pretty_json(serde_json::json!({
            "chunk": {
                "x": detail.pos.x,
                "z": detail.pos.z,
                "dimension": detail.pos.dimension.id(),
            },
            "records": records,
        })),
    }
}

pub(super) fn selection_stats_panel(stats: SelectionStats) -> ProfessionalDetail {
    ProfessionalDetail::Selection {
        title: SharedString::from("选区统计"),
        json: pretty_json(serde_json::json!(stats)),
    }
}

pub(super) fn player_editor_detail(detail: PlayerDetail) -> ProfessionalDetail {
    ProfessionalDetail::Editor {
        target: EditTarget::Player(detail.id.clone()),
        title: SharedString::from(format!("玩家 {}", player_id_label(&detail.id))),
        sections: vec![EditSection {
            title: SharedString::from("玩家数据"),
            rows: player_detail_rows(&detail),
        }],
        json: detail.json,
    }
}

pub(super) fn load_edit_detail_blocking(
    editor: &MapWorldEditor,
    target: EditTarget,
) -> bedrock_render::Result<ProfessionalDetail> {
    match target {
        EditTarget::HsaChunk(pos) => hsa_editor_detail(editor, pos),
        EditTarget::Player(id) => {
            let data = editor.world().get_player_blocking(&id)?.ok_or_else(|| {
                bedrock_render::BedrockRenderError::Validation(
                    "player record does not exist".to_string(),
                )
            })?;
            player_detail_from_data(data)
                .map(player_editor_detail)
                .map_err(|error| bedrock_render::BedrockRenderError::Validation(error.to_string()))
        }
        EditTarget::BlockEntities(pos) => block_entities_editor_detail(editor, pos),
        EditTarget::BlockEntityAt { chunk, block } => {
            block_entity_at_editor_detail(editor, chunk, block)
        }
        EditTarget::Actors(pos) => actors_editor_detail(editor, pos),
        EditTarget::HeightMap(pos) => heightmap_editor_detail(editor, pos),
        EditTarget::BiomeStorage(pos) => biome_storage_editor_detail(editor, pos),
        EditTarget::MapRecord(id) => map_record_editor_detail(editor, &id),
        EditTarget::GlobalRecord(kind) => global_record_editor_detail(editor, kind),
    }
}

pub(super) fn run_edit_action_blocking(
    editor: &MapWorldEditor,
    target: EditTarget,
    action: EditAction,
    document_text: Option<String>,
) -> bedrock_render::Result<MapEditInvalidation> {
    let invalidation = match (target, action) {
        (EditTarget::Player(id), EditAction::Save) => {
            let text = document_text.ok_or_else(|| {
                bedrock_render::BedrockRenderError::Validation(
                    "missing player JSON document".to_string(),
                )
            })?;
            let tag = serde_json::from_str::<NbtTag>(&text).map_err(|error| {
                bedrock_render::BedrockRenderError::Validation(format!(
                    "player JSON is not valid NBT JSON: {error}"
                ))
            })?;
            let player = PlayerData::from_nbt(id, tag).map_err(|error| {
                bedrock_render::BedrockRenderError::Validation(format!(
                    "player NBT serialize failed: {error}"
                ))
            })?;
            editor.world().put_player_blocking(&player)?;
            Ok(MapEditInvalidation::metadata())
        }
        (EditTarget::HsaChunk(pos), EditAction::Save) => {
            let areas = editor
                .scan_hsa_records(WorldScanOptions::default())?
                .into_iter()
                .find_map(|(chunk, areas)| (chunk == pos).then_some(areas))
                .unwrap_or_default();
            editor.put_hsa_for_chunk(pos, &areas)
        }
        (EditTarget::HsaChunk(pos), EditAction::Delete) => editor.delete_hsa_for_chunk(pos),
        (EditTarget::BlockEntities(pos), EditAction::Save) => {
            let entities = editor
                .block_entities_in_chunk(pos)?
                .into_iter()
                .map(|record| record.entity)
                .collect::<Vec<_>>();
            editor.put_block_entities(pos, &entities)
        }
        (EditTarget::BlockEntityAt { chunk, block }, EditAction::Delete) => {
            editor.delete_block_entity_at(chunk, block)
        }
        (EditTarget::Actors(pos), EditAction::Delete) => {
            let Some(uid) = editor
                .actors_in_chunk(pos)?
                .into_iter()
                .find_map(|actor| actor.uid)
            else {
                return Err(bedrock_render::BedrockRenderError::Validation(
                    "chunk has no modern actor UID to delete".to_string(),
                ));
            };
            editor.delete_actor(pos, uid)
        }
        (EditTarget::HeightMap(pos), EditAction::Save) => {
            let height_map = editor.heightmap(pos)?.unwrap_or_else(default_heightmap);
            editor.put_heightmap(pos, bedrock_world::ChunkVersion::New, height_map)
        }
        (EditTarget::BiomeStorage(pos), EditAction::Save) => {
            let height_map = editor.heightmap(pos)?.unwrap_or_else(default_heightmap);
            let biome = Biome3d::new(
                height_map.values,
                vec![ParsedBiomeStorage {
                    y: Some(-64),
                    palette: vec![0],
                    indices: Some(vec![0; 4096]),
                    counts: vec![4096],
                }],
            )?;
            editor.put_biome_storage(pos, biome)
        }
        (EditTarget::MapRecord(id), EditAction::Delete) => editor.delete_map_record(&id),
        (EditTarget::GlobalRecord(kind), EditAction::Delete) => editor.delete_global_record(kind),
        (target, action) => Err(bedrock_render::BedrockRenderError::Validation(format!(
            "{} does not support {} yet",
            target.operation_label(),
            edit_action_label(&action)
        ))),
    }?;
    Ok(invalidation)
}

fn edit_history_spec(
    world_path: &PathBuf,
    target: &EditTarget,
    action: &EditAction,
) -> Result<MapHistoryCaptureSpec, String> {
    let mut chunks = BTreeSet::new();
    let mut raw_keys = BTreeSet::new();
    match target {
        EditTarget::MapRecord(id) => {
            raw_keys.insert(id.storage_key().to_vec());
        }
        EditTarget::GlobalRecord(kind) => {
            raw_keys.insert(kind.storage_key().to_vec());
        }
        EditTarget::Player(id) => {
            let Some(key) = id.storage_key() else {
                return Err("玩家记录没有 LevelDB key".to_string());
            };
            raw_keys.insert(key.as_ref().to_vec());
        }
        EditTarget::HsaChunk(chunk)
        | EditTarget::BlockEntities(chunk)
        | EditTarget::Actors(chunk)
        | EditTarget::HeightMap(chunk)
        | EditTarget::BiomeStorage(chunk) => {
            chunks.insert(*chunk);
        }
        EditTarget::BlockEntityAt { chunk, .. } => {
            chunks.insert(*chunk);
        }
    }
    Ok(MapHistoryCaptureSpec {
        kind: match action {
            EditAction::Save => MapHistoryEntryKind::RecordSave,
            EditAction::Delete => MapHistoryEntryKind::RecordDelete,
        },
        label: edit_action_status(action, target),
        world_path: world_path.clone(),
        chunks,
        raw_keys,
        include_level_dat: false,
    })
}

pub(super) fn hsa_editor_detail(
    editor: &MapWorldEditor,
    pos: ChunkPos,
) -> bedrock_render::Result<ProfessionalDetail> {
    let areas = editor
        .scan_hsa_records(WorldScanOptions::default())?
        .into_iter()
        .find_map(|(chunk, areas)| (chunk == pos).then_some(areas))
        .unwrap_or_default();
    let rows = areas
        .iter()
        .enumerate()
        .flat_map(|(index, area)| hsa_rows(index, area))
        .collect::<Vec<_>>();
    Ok(ProfessionalDetail::Editor {
        target: EditTarget::HsaChunk(pos),
        title: SharedString::from(format!("HSA chunk {}, {}", pos.x, pos.z)),
        sections: vec![EditSection {
            title: SharedString::from("硬编码生成区域"),
            rows: if rows.is_empty() {
                vec![readonly_row("记录", "无")]
            } else {
                rows
            },
        }],
        json: pretty_json(serde_json::json!({
            "chunk": chunk_json(pos),
            "areas": areas.iter().map(hsa_json).collect::<Vec<_>>(),
        })),
    })
}

pub(super) fn block_entities_editor_detail(
    editor: &MapWorldEditor,
    pos: ChunkPos,
) -> bedrock_render::Result<ProfessionalDetail> {
    let records = editor.block_entities_in_chunk(pos)?;
    Ok(ProfessionalDetail::Editor {
        target: EditTarget::BlockEntities(pos),
        title: SharedString::from(format!("方块实体 chunk {}, {}", pos.x, pos.z)),
        sections: vec![EditSection {
            title: SharedString::from("方块实体"),
            rows: block_entity_rows(&records),
        }],
        json: pretty_json(serde_json::json!({
            "chunk": chunk_json(pos),
            "records": records.iter().map(block_entity_json).collect::<Vec<_>>(),
        })),
    })
}

pub(super) fn block_entity_at_editor_detail(
    editor: &MapWorldEditor,
    chunk: ChunkPos,
    block: BlockPos,
) -> bedrock_render::Result<ProfessionalDetail> {
    let records = editor.block_entities_in_chunk(chunk)?;
    let matching = records
        .iter()
        .filter(|record| record.entity.position == Some([block.x, block.y, block.z]))
        .cloned()
        .collect::<Vec<_>>();
    Ok(ProfessionalDetail::Editor {
        target: EditTarget::BlockEntityAt { chunk, block },
        title: SharedString::from(format!("方块实体 {}, {}, {}", block.x, block.y, block.z)),
        sections: vec![EditSection {
            title: SharedString::from("当前位置"),
            rows: block_entity_rows(&matching),
        }],
        json: pretty_json(serde_json::json!({
            "chunk": chunk_json(chunk),
            "block": block_json(block),
            "records": matching.iter().map(block_entity_json).collect::<Vec<_>>(),
        })),
    })
}

pub(super) fn actors_editor_detail(
    editor: &MapWorldEditor,
    pos: ChunkPos,
) -> bedrock_render::Result<ProfessionalDetail> {
    let actors = editor.actors_in_chunk(pos)?;
    Ok(ProfessionalDetail::Editor {
        target: EditTarget::Actors(pos),
        title: SharedString::from(format!("Actors chunk {}, {}", pos.x, pos.z)),
        sections: vec![EditSection {
            title: SharedString::from("实体 / Actors"),
            rows: actor_rows(&actors),
        }],
        json: pretty_json(serde_json::json!({
            "chunk": chunk_json(pos),
            "actors": actors.iter().map(actor_json).collect::<Vec<_>>(),
        })),
    })
}

pub(super) fn heightmap_editor_detail(
    editor: &MapWorldEditor,
    pos: ChunkPos,
) -> bedrock_render::Result<ProfessionalDetail> {
    let heightmap = editor.heightmap(pos)?;
    let rows = heightmap
        .as_ref()
        .map_or_else(|| vec![readonly_row("高度图", "无")], heightmap_rows);
    Ok(ProfessionalDetail::Editor {
        target: EditTarget::HeightMap(pos),
        title: SharedString::from(format!("高度图 chunk {}, {}", pos.x, pos.z)),
        sections: vec![EditSection {
            title: SharedString::from("Data2D/Data3D 高度图"),
            rows,
        }],
        json: pretty_json(serde_json::json!({
            "chunk": chunk_json(pos),
            "heightmap": heightmap.as_ref().map(|map| &map.values),
        })),
    })
}

pub(super) fn biome_storage_editor_detail(
    editor: &MapWorldEditor,
    pos: ChunkPos,
) -> bedrock_render::Result<ProfessionalDetail> {
    let heightmap = editor.heightmap(pos)?;
    Ok(ProfessionalDetail::Editor {
        target: EditTarget::BiomeStorage(pos),
        title: SharedString::from(format!("生物群系 chunk {}, {}", pos.x, pos.z)),
        sections: vec![EditSection {
            title: SharedString::from("Biome Storage"),
            rows: vec![
                readonly_row(
                    "高度图",
                    if heightmap.is_some() {
                        "存在"
                    } else {
                        "缺失"
                    },
                ),
                editable_row("默认 palette[0]", "0"),
                editable_row("索引模式", "uniform/full-indices"),
            ],
        }],
        json: pretty_json(serde_json::json!({
            "chunk": chunk_json(pos),
            "heightmap_present": heightmap.is_some(),
            "write_model": "Data3D with validated Biome3d payload",
        })),
    })
}

pub(super) fn map_record_editor_detail(
    editor: &MapWorldEditor,
    id: &MapRecordId,
) -> bedrock_render::Result<ProfessionalDetail> {
    let record = editor.read_map_record(id)?;
    let rows = record.as_ref().map_or_else(
        || vec![readonly_row("记录", "不存在")],
        |record| {
            vec![
                readonly_row("id", record.record_id.as_str().to_string()),
                readonly_row("roots", record.roots.len().to_string()),
                readonly_row(
                    "pixels",
                    record.pixels.as_ref().map_or_else(
                        || "无".to_string(),
                        |pixels| {
                            format!("{}x{} {}", pixels.width, pixels.height, pixels.colors.len())
                        },
                    ),
                ),
            ]
        },
    );
    Ok(ProfessionalDetail::Editor {
        target: EditTarget::MapRecord(id.clone()),
        title: SharedString::from(format!("Map {}", id.as_str())),
        sections: vec![EditSection {
            title: SharedString::from("地图记录"),
            rows,
        }],
        json: pretty_json(serde_json::json!(record.as_ref().map(map_record_json))),
    })
}

pub(super) fn global_record_editor_detail(
    editor: &MapWorldEditor,
    kind: GlobalRecordKind,
) -> bedrock_render::Result<ProfessionalDetail> {
    let record = editor.read_global_record(kind.clone())?;
    Ok(ProfessionalDetail::Editor {
        target: EditTarget::GlobalRecord(kind.clone()),
        title: SharedString::from(format!("Global {}", global_kind_label(&kind))),
        sections: vec![EditSection {
            title: SharedString::from("全局记录"),
            rows: record.as_ref().map_or_else(
                || vec![readonly_row("记录", "不存在")],
                |record| {
                    vec![
                        readonly_row("name", &record.name),
                        readonly_row("kind", global_kind_label(&record.kind)),
                        readonly_row("roots", record.roots.len().to_string()),
                    ]
                },
            ),
        }],
        json: pretty_json(serde_json::json!(record.as_ref().map(global_record_json))),
    })
}

pub(super) fn render_editor_sections(colors: &ThemeColors, sections: &[EditSection]) -> Div {
    sections.iter().fold(
        div().flex().flex_col().gap(px(8.0)),
        |container, section| {
            let rows = section
                .rows
                .iter()
                .fold(div().flex().flex_col().gap(px(5.0)), |rows, row| {
                    rows.child(render_edit_row(colors, row))
                });
            container.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text_secondary)
                            .child(section.title.clone()),
                    )
                    .child(rows),
            )
        },
    )
}

pub(super) fn render_edit_row(colors: &ThemeColors, row: &EditRow) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .px(px(8.0))
        .py(px(5.0))
        .rounded(px(6.0))
        .bg(Hsla {
            a: if row.editable { 0.28 } else { 0.16 },
            ..colors.surface_hover
        })
        .child(
            div()
                .text_size(px(11.0))
                .text_color(colors.text_muted)
                .child(row.label.clone()),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_color(if row.editable {
                    colors.text_primary
                } else {
                    colors.text_secondary
                })
                .child(row.value.clone()),
        )
}

pub(super) fn editor_action_buttons(
    colors: &ThemeColors,
    target: EditTarget,
    write_mode: bool,
    pending: Option<&PendingEditConfirmation>,
    cx: &mut Context<MapViewerWindowView>,
) -> Vec<Div> {
    let mut buttons = Vec::new();
    if supports_editor_save(&target) {
        let pending_save = pending
            .is_some_and(|pending| pending.target == target && pending.action == EditAction::Save);
        let label = if pending_save {
            "确认保存"
        } else {
            "保存"
        };
        buttons.push(toolbar_button(colors, label).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _event, _window, cx| this.request_editor_save(cx)),
        ));
    }
    if supports_editor_delete(&target) {
        let pending_delete = pending.is_some_and(|pending| {
            pending.target == target && pending.action == EditAction::Delete
        });
        let label = if pending_delete {
            "确认删除"
        } else {
            "删除"
        };
        buttons.push(toolbar_button(colors, label).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _event, _window, cx| this.request_editor_delete(cx)),
        ));
    }
    if !write_mode {
        buttons.push(
            div()
                .text_size(px(11.0))
                .text_color(colors.text_muted)
                .child("写入前需开启写入模式"),
        );
    }
    buttons
}

pub(super) fn supports_editor_save(target: &EditTarget) -> bool {
    matches!(
        target,
        EditTarget::Player(_)
            | EditTarget::HsaChunk(_)
            | EditTarget::BlockEntities(_)
            | EditTarget::HeightMap(_)
            | EditTarget::BiomeStorage(_)
    )
}

pub(super) fn supports_editor_delete(target: &EditTarget) -> bool {
    matches!(
        target,
        EditTarget::HsaChunk(_)
            | EditTarget::BlockEntityAt { .. }
            | EditTarget::Actors(_)
            | EditTarget::MapRecord(_)
            | EditTarget::GlobalRecord(_)
    )
}

pub(super) fn readonly_row(
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> EditRow {
    EditRow {
        label: label.into(),
        value: value.into(),
        editable: false,
    }
}

pub(super) fn editable_row(
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> EditRow {
    EditRow {
        label: label.into(),
        value: value.into(),
        editable: true,
    }
}

pub(super) fn hsa_rows(index: usize, area: &ParsedHardcodedSpawnArea) -> Vec<EditRow> {
    vec![
        readonly_row(format!("#{index} kind"), hsa_kind_label(area.kind)),
        editable_row(
            format!("#{index} min"),
            format!("{},{},{}", area.min[0], area.min[1], area.min[2]),
        ),
        editable_row(
            format!("#{index} max"),
            format!("{},{},{}", area.max[0], area.max[1], area.max[2]),
        ),
    ]
}

pub(super) fn block_entity_rows(records: &[BlockEntityRecord]) -> Vec<EditRow> {
    if records.is_empty() {
        return vec![readonly_row("记录", "无")];
    }
    records
        .iter()
        .flat_map(|record| {
            let pos = record.entity.position.map_or_else(
                || "unknown".to_string(),
                |pos| format!("{},{},{}", pos[0], pos[1], pos[2]),
            );
            vec![
                readonly_row(
                    format!("#{} id", record.index),
                    record.entity.id.clone().unwrap_or_default(),
                ),
                editable_row(format!("#{} pos", record.index), pos),
                editable_row(
                    format!("#{} name", record.index),
                    record.entity.custom_name.clone().unwrap_or_default(),
                ),
            ]
        })
        .collect()
}

pub(super) fn actor_rows(records: &[ActorRecord]) -> Vec<EditRow> {
    if records.is_empty() {
        return vec![readonly_row("记录", "无")];
    }
    records
        .iter()
        .enumerate()
        .flat_map(|(index, record)| {
            let pos = record.entity.position.map_or_else(
                || "unknown".to_string(),
                |pos| format!("{:.2},{:.2},{:.2}", pos[0], pos[1], pos[2]),
            );
            vec![
                readonly_row(
                    format!("#{index} uid"),
                    record
                        .uid
                        .map_or_else(|| "legacy/unknown".to_string(), |uid| uid.0.to_string()),
                ),
                readonly_row(
                    format!("#{index} source"),
                    actor_source_label(&record.source),
                ),
                editable_row(format!("#{index} pos"), pos),
                readonly_row(
                    format!("#{index} id"),
                    record.entity.identifier.clone().unwrap_or_default(),
                ),
            ]
        })
        .collect()
}

pub(super) fn player_detail_rows(detail: &PlayerDetail) -> Vec<EditRow> {
    vec![
        readonly_row("id", player_id_label(&detail.id)),
        readonly_row(
            "unique_id",
            detail
                .unique_id
                .map_or_else(|| "unknown".to_string(), |value| value.to_string()),
        ),
        editable_row(
            "position",
            detail.position.map_or_else(
                || "unknown".to_string(),
                |position| format!("{:.2},{:.2},{:.2}", position[0], position[1], position[2]),
            ),
        ),
        editable_row(
            "dimension",
            detail
                .dimension_id
                .map_or_else(|| "unknown".to_string(), |value| value.to_string()),
        ),
        readonly_row("items", detail.item_count.to_string()),
    ]
}

pub(super) fn heightmap_rows(heightmap: &HeightMap2d) -> Vec<EditRow> {
    let min = heightmap.values.iter().min().copied().unwrap_or_default();
    let max = heightmap.values.iter().max().copied().unwrap_or_default();
    vec![
        readonly_row("columns", heightmap.values.len().to_string()),
        editable_row("min", min.to_string()),
        editable_row("max", max.to_string()),
    ]
}

pub(super) fn default_heightmap() -> HeightMap2d {
    HeightMap2d::new(vec![0; 256]).expect("default heightmap has 256 values")
}

pub(super) fn chunk_json(pos: ChunkPos) -> serde_json::Value {
    serde_json::json!({
        "x": pos.x,
        "z": pos.z,
        "dimension": pos.dimension.id(),
    })
}

pub(super) fn block_json(pos: BlockPos) -> serde_json::Value {
    serde_json::json!({
        "x": pos.x,
        "y": pos.y,
        "z": pos.z,
    })
}

pub(super) fn hsa_json(area: &ParsedHardcodedSpawnArea) -> serde_json::Value {
    serde_json::json!({
        "kind": hsa_kind_label(area.kind),
        "min": area.min,
        "max": area.max,
    })
}

pub(super) fn block_entity_json(record: &BlockEntityRecord) -> serde_json::Value {
    serde_json::json!({
        "chunk": chunk_json(record.chunk),
        "index": record.index,
        "id": record.entity.id,
        "position": record.entity.position,
        "is_movable": record.entity.is_movable,
        "custom_name": record.entity.custom_name,
        "items": record.entity.items.len(),
    })
}

pub(super) fn actor_json(record: &ActorRecord) -> serde_json::Value {
    serde_json::json!({
        "uid": record.uid.map(|uid| uid.0),
        "source": actor_source_label(&record.source),
        "identifier": record.entity.identifier,
        "definitions": record.entity.definitions,
        "position": record.entity.position,
        "items": record.entity.items.len(),
    })
}

pub(super) fn map_record_json(record: &ParsedMapData) -> serde_json::Value {
    serde_json::json!({
        "id": record.record_id.as_str(),
        "roots": record.roots.len(),
        "known_fields": {
            "dimension": record.known_fields.dimension,
            "center_x": record.known_fields.center_x,
            "center_z": record.known_fields.center_z,
            "scale": record.known_fields.scale,
            "width": record.known_fields.width,
            "height": record.known_fields.height,
            "locked": record.known_fields.locked,
        },
        "pixels": record.pixels.as_ref().map(|pixels| serde_json::json!({
            "width": pixels.width,
            "height": pixels.height,
            "colors": pixels.colors.len(),
        })),
    })
}

pub(super) fn global_record_json(record: &ParsedGlobalData) -> serde_json::Value {
    serde_json::json!({
        "name": record.name,
        "kind": global_kind_label(&record.kind),
        "roots": record.roots.len(),
    })
}

pub(super) fn run_quick_write_action_blocking(
    world: &BedrockWorld,
    action: QuickWriteAction,
    guard: &WriteGuard,
    copied_chunk: Option<&CopiedChunkData>,
    imported_structure: Option<&ImportedStructureData>,
    cancel: Option<&CancelFlag>,
    mut progress: impl FnMut(ChunkTransferProgress),
) -> bedrock_world::Result<(String, MapEditInvalidation)> {
    check_bedrock_operation_cancelled(cancel)?;
    let chunk = action.chunk();
    let result = match action {
        QuickWriteAction::DeleteCurrentChunk(_) | QuickWriteAction::ResetCurrentChunk(_) => {
            let phase = if matches!(action, QuickWriteAction::ResetCurrentChunk(_)) {
                "重置区块"
            } else {
                "删除区块"
            };
            progress(ChunkTransferProgress {
                phase: SharedString::from(phase),
                completed: 0,
                total: 1,
            });
            let bounds = SlimeChunkBounds {
                dimension: chunk.dimension,
                min_chunk_x: chunk.x,
                max_chunk_x: chunk.x,
                min_chunk_z: chunk.z,
                max_chunk_z: chunk.z,
            };
            let (message, invalidation) =
                if matches!(action, QuickWriteAction::ResetCurrentChunk(_)) {
                    let deleted = delete_chunks_blocking(world, bounds, guard)?;
                    check_bedrock_operation_cancelled(cancel)?;
                    (
                        format!("已重置当前 chunk，删除 {deleted} 条记录并允许游戏重新加载"),
                        MapEditInvalidation::chunk(chunk).with_metadata(),
                    )
                } else {
                    let cleared = clear_chunks_blocking(world, bounds, guard)?;
                    check_bedrock_operation_cancelled(cancel)?;
                    (
                        format!("已清空当前 chunk 为空气（{} 个 chunk）", cleared),
                        MapEditInvalidation::chunk(chunk).with_metadata(),
                    )
                };
            progress(ChunkTransferProgress {
                phase: SharedString::from(phase),
                completed: 1,
                total: 1,
            });
            Ok((message, invalidation))
        }
        QuickWriteAction::DeleteCurrentChunkBlockEntities(_) => {
            check_bedrock_operation_cancelled(cancel)?;
            delete_chunk_record_with_guard(world, chunk, ChunkRecordTag::BlockEntity, guard)?;
            Ok((
                format!("已删除 chunk {},{} 方块实体记录", chunk.x, chunk.z),
                MapEditInvalidation::chunk(chunk).with_metadata(),
            ))
        }
        QuickWriteAction::DeleteCurrentChunkActors(_) => {
            let actors = world.actors_in_chunk_blocking(chunk)?;
            let mut deleted = 0usize;
            for uid in actors.into_iter().filter_map(|actor| actor.uid) {
                check_bedrock_operation_cancelled(cancel)?;
                world.delete_actor_blocking(chunk, uid)?;
                deleted = deleted.saturating_add(1);
            }
            check_bedrock_operation_cancelled(cancel)?;
            delete_chunk_record_with_guard(world, chunk, ChunkRecordTag::Entity, guard)?;
            Ok((
                format!(
                    "已删除 chunk {},{} 的 {deleted} 个现代实体",
                    chunk.x, chunk.z
                ),
                MapEditInvalidation::chunk(chunk).with_metadata(),
            ))
        }
        QuickWriteAction::PasteCopiedChunk {
            source,
            target,
            transform,
        } => {
            let copied_chunk = copied_chunk.ok_or_else(|| {
                bedrock_world::BedrockWorldError::Validation("没有可粘贴的区块副本".to_string())
            })?;
            paste_copied_chunk_blocking(
                world,
                copied_chunk,
                source,
                target,
                transform,
                guard,
                cancel,
                &mut progress,
            )
        }
        QuickWriteAction::PasteCopiedChunks {
            source_anchor,
            target_anchor,
            chunk_count: _,
            transform,
        } => {
            let copied_chunk = copied_chunk.ok_or_else(|| {
                bedrock_world::BedrockWorldError::Validation("没有可粘贴的区块副本".to_string())
            })?;
            paste_copied_chunk_blocking(
                world,
                copied_chunk,
                source_anchor,
                target_anchor,
                transform,
                guard,
                cancel,
                &mut progress,
            )
        }
        QuickWriteAction::PasteImportedStructure {
            source_anchor: _,
            target_anchor,
            chunk_count: _,
            transform,
        } => {
            let imported_structure = imported_structure.ok_or_else(|| {
                bedrock_world::BedrockWorldError::Validation("没有可粘贴的结构文件".to_string())
            })?;
            mcstructure::paste_imported_structure_blocking(
                world,
                imported_structure,
                target_anchor,
                transform,
                guard,
                cancel,
                &mut progress,
            )
        }
    }?;
    Ok(result)
}

fn quick_write_history_spec(
    world_path: &PathBuf,
    action: &QuickWriteAction,
    copied_chunk: Option<&CopiedChunkData>,
    imported_structure: Option<&ImportedStructureData>,
) -> Result<MapHistoryCaptureSpec, String> {
    let mut chunks = BTreeSet::new();
    let kind = match action {
        QuickWriteAction::DeleteCurrentChunk(chunk) => {
            chunks.insert(*chunk);
            MapHistoryEntryKind::ChunkDelete
        }
        QuickWriteAction::ResetCurrentChunk(chunk) => {
            chunks.insert(*chunk);
            MapHistoryEntryKind::ChunkReset
        }
        QuickWriteAction::DeleteCurrentChunkBlockEntities(chunk)
        | QuickWriteAction::DeleteCurrentChunkActors(chunk) => {
            chunks.insert(*chunk);
            MapHistoryEntryKind::RecordDelete
        }
        QuickWriteAction::PasteCopiedChunk { target, .. } => {
            chunks.insert(*target);
            MapHistoryEntryKind::ChunkPaste
        }
        QuickWriteAction::PasteCopiedChunks {
            source_anchor,
            target_anchor,
            transform,
            ..
        } => {
            let copied_chunk = copied_chunk.ok_or_else(|| "没有可粘贴的区块副本".to_string())?;
            chunks.extend(pasted_chunk_targets(
                copied_chunk,
                *source_anchor,
                *target_anchor,
                *transform,
            ));
            MapHistoryEntryKind::ChunkPaste
        }
        QuickWriteAction::PasteImportedStructure {
            target_anchor,
            transform,
            ..
        } => {
            let imported_structure =
                imported_structure.ok_or_else(|| "没有可粘贴的结构文件".to_string())?;
            chunks.extend(mcstructure::imported_structure_targets(
                imported_structure,
                *target_anchor,
                *transform,
            ));
            MapHistoryEntryKind::ChunkPaste
        }
    };
    Ok(MapHistoryCaptureSpec {
        kind,
        label: action.label(),
        world_path: world_path.clone(),
        chunks,
        raw_keys: BTreeSet::new(),
        include_level_dat: false,
    })
}

pub(super) fn copy_chunks_blocking(
    editor: &MapWorldEditor,
    source_anchor: ChunkPos,
    chunks: Vec<ChunkPos>,
    cancel: Option<&CancelFlag>,
    mut progress: impl FnMut(ChunkTransferProgress),
) -> bedrock_world::Result<CopiedChunkData> {
    if chunks.is_empty() {
        return Err(bedrock_world::BedrockWorldError::Validation(
            "没有可复制的 chunk".to_string(),
        ));
    }

    let total = chunks.len();
    let mut copied_chunks = Vec::with_capacity(total);
    for (index, chunk) in chunks.into_iter().enumerate() {
        check_bedrock_operation_cancelled(cancel)?;
        let records = copy_safe_chunk_records(editor.world().get_chunk_blocking(chunk)?.records);
        let parsed_chunk = editor
            .world()
            .parse_chunk_with_options_blocking(chunk, copy_chunk_parse_options())?;
        let mut block_entities = Vec::new();
        let mut hardcoded_spawn_areas = Vec::new();
        for record in parsed_chunk.records {
            match record.value {
                bedrock_world::ParsedChunkRecordValue::BlockEntities(entities) => {
                    block_entities.extend(entities);
                }
                bedrock_world::ParsedChunkRecordValue::HardcodedSpawnAreas(areas) => {
                    hardcoded_spawn_areas.extend(areas);
                }
                _ => {}
            }
        }

        copied_chunks.push(CopiedChunkSnapshot {
            chunk,
            records,
            block_entities,
            hardcoded_spawn_areas,
        });
        progress(ChunkTransferProgress {
            phase: SharedString::from("复制区块"),
            completed: index + 1,
            total,
        });
    }

    Ok(CopiedChunkData {
        source: source_anchor,
        chunks: copied_chunks,
    })
}

fn check_bedrock_operation_cancelled(cancel: Option<&CancelFlag>) -> bedrock_world::Result<()> {
    if cancel.is_some_and(CancelFlag::is_cancelled) {
        return Err(bedrock_world::BedrockWorldError::Validation(
            MAP_OPERATION_CANCELLED_MESSAGE.to_string(),
        ));
    }
    Ok(())
}

pub(super) fn paste_copied_chunk_blocking(
    world: &BedrockWorld,
    copied_chunk: &CopiedChunkData,
    source_anchor: ChunkPos,
    target_anchor: ChunkPos,
    transform: PasteTransform,
    guard: &WriteGuard,
    cancel: Option<&CancelFlag>,
    progress: &mut impl FnMut(ChunkTransferProgress),
) -> bedrock_world::Result<(String, MapEditInvalidation)> {
    check_bedrock_operation_cancelled(cancel)?;
    if !transform.is_default() {
        return paste_transformed_copied_chunks_blocking(
            world,
            copied_chunk,
            source_anchor,
            target_anchor,
            transform,
            guard,
            cancel,
            progress,
        );
    }

    let total = copied_chunk.chunks.len();
    let mut affected_chunks = BTreeSet::new();

    for (index, (snapshot, target_chunk)) in copied_chunk
        .chunks
        .iter()
        .zip(pasted_chunk_targets(
            copied_chunk,
            source_anchor,
            target_anchor,
            transform,
        ))
        .enumerate()
    {
        check_bedrock_operation_cancelled(cancel)?;
        delete_chunks_blocking(
            world,
            SlimeChunkBounds {
                dimension: target_chunk.dimension,
                min_chunk_x: target_chunk.x,
                max_chunk_x: target_chunk.x,
                min_chunk_z: target_chunk.z,
                max_chunk_z: target_chunk.z,
            },
            guard,
        )?;

        let mut transaction = world.transaction();
        for record in &snapshot.records {
            let mut key = record.key.clone();
            key.pos = target_chunk;
            transaction.put_raw_record(&key, record.value.clone());
        }
        transaction.commit()?;

        let shifted_block_entities = snapshot
            .block_entities
            .iter()
            .map(|entity| pasted_block_entity_for_target(entity, snapshot.chunk, target_chunk))
            .collect::<Vec<_>>();
        if !shifted_block_entities.is_empty() {
            world.put_block_entities_blocking(target_chunk, &shifted_block_entities)?;
        }

        let shifted_hsa = snapshot
            .hardcoded_spawn_areas
            .iter()
            .cloned()
            .map(|area| {
                pasted_hardcoded_spawn_area_for_target(
                    area,
                    snapshot.chunk,
                    target_chunk,
                    transform,
                )
            })
            .collect::<Vec<_>>();
        if !shifted_hsa.is_empty() {
            world.put_hsa_for_chunk_blocking(target_chunk, &shifted_hsa)?;
        }

        affected_chunks.insert(target_chunk);
        progress(ChunkTransferProgress {
            phase: SharedString::from("粘贴区块"),
            completed: index + 1,
            total,
        });
    }
    check_bedrock_operation_cancelled(cancel)?;

    Ok((
        format!(
            "已粘贴 {} 个 chunk（{},{} -> {},{}）",
            total, source_anchor.x, source_anchor.z, target_anchor.x, target_anchor.z
        ),
        MapEditInvalidation::chunks(affected_chunks).with_metadata(),
    ))
}

pub(super) struct CopiedChunkStructurePlacement {
    pub(super) structure: bedrock_world::McStructureFile,
    pub(super) source_anchor: ChunkPos,
    pub(super) target_anchor: ChunkPos,
    pub(super) origin_y: i32,
}

pub(super) fn copied_chunk_snapshot_structure_placement(
    snapshot: &CopiedChunkSnapshot,
    target_chunk: ChunkPos,
) -> bedrock_world::Result<CopiedChunkStructurePlacement> {
    let (min_y, max_y) = snapshot.chunk.y_range(ChunkVersion::New);
    let height = max_y.saturating_sub(min_y).saturating_add(1);
    let size = bedrock_world::McStructureSize::new(16, height, 16)?;
    let source_origin_x = snapshot.chunk.x.saturating_mul(16);
    let source_origin_z = snapshot.chunk.z.saturating_mul(16);
    let mut structure =
        bedrock_world::McStructureFile::new_air(size, [source_origin_x, min_y, source_origin_z])?;
    let mut palette_indices = HashMap::new();
    let air_key = mcstructure_palette_key(&structure.palette[0]);
    palette_indices.insert(air_key, 0_i32);
    let chunk = bedrock_world::Chunk {
        pos: snapshot.chunk,
        version: None,
        records: snapshot.records.clone(),
    };
    let decoded_subchunks = copied_chunk_decoded_subchunks(&chunk, min_y, max_y)?;
    let legacy_terrain = chunk.legacy_terrain()?;

    for x in 0..size.x {
        let local_x = u8::try_from(x).map_err(|_| {
            bedrock_world::BedrockWorldError::Validation(format!(
                "chunk copy source x has invalid local value: {x}"
            ))
        })?;
        for z in 0..size.z {
            let local_z = u8::try_from(z).map_err(|_| {
                bedrock_world::BedrockWorldError::Validation(format!(
                    "chunk copy source z has invalid local value: {z}"
                ))
            })?;
            for y in 0..size.y {
                let world_y = min_y.saturating_add(y);
                let block_index = size.index(x, y, z)?;
                if let Some(entry) = copied_chunk_structure_primary_entry_at(
                    &decoded_subchunks,
                    legacy_terrain.as_ref(),
                    local_x,
                    world_y,
                    local_z,
                )? {
                    structure.primary_indices[block_index] =
                        mcstructure_palette_index(&mut structure, &mut palette_indices, entry)?;
                }
                if let Some(entry) = copied_chunk_structure_secondary_entry_at(
                    &decoded_subchunks,
                    local_x,
                    world_y,
                    local_z,
                )? {
                    structure.secondary_indices[block_index] =
                        mcstructure_palette_index(&mut structure, &mut palette_indices, entry)?;
                }
            }
        }
    }

    for entity in &snapshot.block_entities {
        insert_copied_block_entity_into_structure(
            &mut structure,
            entity,
            source_origin_x,
            min_y,
            source_origin_z,
        )?;
    }

    Ok(CopiedChunkStructurePlacement {
        structure,
        source_anchor: snapshot.chunk,
        target_anchor: target_chunk,
        origin_y: min_y,
    })
}

fn paste_transformed_copied_chunks_blocking(
    world: &BedrockWorld,
    copied_chunk: &CopiedChunkData,
    source_anchor: ChunkPos,
    target_anchor: ChunkPos,
    transform: PasteTransform,
    guard: &WriteGuard,
    cancel: Option<&CancelFlag>,
    progress: &mut impl FnMut(ChunkTransferProgress),
) -> bedrock_world::Result<(String, MapEditInvalidation)> {
    check_bedrock_operation_cancelled(cancel)?;
    let targets = pasted_chunk_targets(copied_chunk, source_anchor, target_anchor, transform);
    let total = targets.len().max(1);
    let mut affected_chunks = BTreeSet::new();

    for (index, (snapshot, target_chunk)) in copied_chunk
        .chunks
        .iter()
        .zip(targets.iter().copied())
        .enumerate()
    {
        check_bedrock_operation_cancelled(cancel)?;
        delete_chunks_blocking(
            world,
            SlimeChunkBounds {
                dimension: target_chunk.dimension,
                min_chunk_x: target_chunk.x,
                max_chunk_x: target_chunk.x,
                min_chunk_z: target_chunk.z,
                max_chunk_z: target_chunk.z,
            },
            guard,
        )?;
        progress(ChunkTransferProgress {
            phase: SharedString::from("清空目标区块"),
            completed: index + 1,
            total,
        });

        let structure_placement =
            copied_chunk_snapshot_structure_placement(snapshot, target_chunk)?;
        check_bedrock_operation_cancelled(cancel)?;
        let result = structure_placement.structure.write_to_world_blocking(
            world,
            bedrock_world::McStructurePlacement {
                source_anchor: structure_placement.source_anchor,
                target_anchor: structure_placement.target_anchor,
                origin_y: structure_placement.origin_y,
                rotation: mcstructure_rotation_for_paste(transform.rotation),
                mirror_x: transform.mirror_x,
                mirror_z: transform.mirror_z,
            },
            guard,
            |write_progress| {
                progress(ChunkTransferProgress {
                    phase: SharedString::from(match write_progress.phase {
                        bedrock_world::McStructureWritePhase::Prepare => "准备变换区块",
                        bedrock_world::McStructureWritePhase::WriteChunks => "写入变换区块",
                    }),
                    completed: write_progress.completed,
                    total: write_progress.total,
                });
            },
        )?;
        check_bedrock_operation_cancelled(cancel)?;
        let shifted_hsa = snapshot
            .hardcoded_spawn_areas
            .iter()
            .cloned()
            .map(|area| {
                pasted_hardcoded_spawn_area_for_target(
                    area,
                    snapshot.chunk,
                    target_chunk,
                    transform,
                )
            })
            .collect::<Vec<_>>();
        if !shifted_hsa.is_empty() {
            world.put_hsa_for_chunk_blocking(target_chunk, &shifted_hsa)?;
        }

        affected_chunks.extend(result.affected_chunks);
        affected_chunks.insert(target_chunk);
    }
    check_bedrock_operation_cancelled(cancel)?;

    Ok((
        format!(
            "已粘贴 {} 个 chunk（{},{} -> {},{}，{}）",
            copied_chunk.chunks.len(),
            source_anchor.x,
            source_anchor.z,
            target_anchor.x,
            target_anchor.z,
            transform.label()
        ),
        MapEditInvalidation::chunks(affected_chunks).with_metadata(),
    ))
}

const fn mcstructure_rotation_for_paste(
    rotation: PasteRotation,
) -> bedrock_world::McStructureRotation {
    match rotation {
        PasteRotation::NoRotation => bedrock_world::McStructureRotation::None,
        PasteRotation::Clockwise90 => bedrock_world::McStructureRotation::Clockwise90,
        PasteRotation::Rotate180 => bedrock_world::McStructureRotation::Rotate180,
        PasteRotation::CounterClockwise90 => bedrock_world::McStructureRotation::CounterClockwise90,
    }
}

fn copied_chunk_decoded_subchunks(
    chunk: &bedrock_world::Chunk,
    min_y: i32,
    max_y: i32,
) -> bedrock_world::Result<BTreeMap<i8, bedrock_world::SubChunk>> {
    let mut subchunks = BTreeMap::new();
    for subchunk_y in min_y.div_euclid(16)..=max_y.div_euclid(16) {
        let subchunk_y = i8::try_from(subchunk_y).map_err(|_| {
            bedrock_world::BedrockWorldError::Validation(format!(
                "chunk copy source subchunk y={subchunk_y} cannot be represented as i8"
            ))
        })?;
        if let Some(subchunk) = chunk.get_subchunk(subchunk_y)? {
            subchunks.insert(subchunk_y, subchunk);
        }
    }
    Ok(subchunks)
}

fn copied_chunk_structure_primary_entry_at(
    subchunks: &BTreeMap<i8, bedrock_world::SubChunk>,
    legacy_terrain: Option<&bedrock_world::LegacyTerrain>,
    local_x: u8,
    world_y: i32,
    local_z: u8,
) -> bedrock_world::Result<Option<bedrock_world::McStructurePaletteEntry>> {
    let (subchunk_y, local_y) = copied_chunk_structure_subchunk_coords(world_y)?;
    if let Some(state) = subchunks.get(&subchunk_y).and_then(|subchunk| {
        copied_chunk_subchunk_layer_state_at(subchunk, 0, local_x, local_y, local_z)
    }) {
        return Ok(Some(
            bedrock_world::McStructurePaletteEntry::from_block_state(state),
        ));
    }
    Ok(legacy_terrain
        .and_then(|terrain| copied_chunk_legacy_block_state_at(terrain, local_x, world_y, local_z))
        .map(|state| bedrock_world::McStructurePaletteEntry::from_block_state(&state)))
}

fn copied_chunk_structure_secondary_entry_at(
    subchunks: &BTreeMap<i8, bedrock_world::SubChunk>,
    local_x: u8,
    world_y: i32,
    local_z: u8,
) -> bedrock_world::Result<Option<bedrock_world::McStructurePaletteEntry>> {
    let (subchunk_y, local_y) = copied_chunk_structure_subchunk_coords(world_y)?;
    Ok(subchunks
        .get(&subchunk_y)
        .and_then(|subchunk| {
            copied_chunk_subchunk_layer_state_at(subchunk, 1, local_x, local_y, local_z)
        })
        .filter(|state| !copied_chunk_is_air_block_state_name(&state.name))
        .map(bedrock_world::McStructurePaletteEntry::from_block_state))
}

fn copied_chunk_structure_subchunk_coords(world_y: i32) -> bedrock_world::Result<(i8, u8)> {
    let subchunk_y = i8::try_from(world_y.div_euclid(16)).map_err(|_| {
        bedrock_world::BedrockWorldError::Validation(format!(
            "chunk copy source y={world_y} cannot be represented as a subchunk index"
        ))
    })?;
    let local_y = u8::try_from(world_y.rem_euclid(16)).map_err(|_| {
        bedrock_world::BedrockWorldError::Validation(format!(
            "chunk copy source y={world_y} has invalid local subchunk offset"
        ))
    })?;
    Ok((subchunk_y, local_y))
}

fn copied_chunk_subchunk_layer_state_at(
    subchunk: &bedrock_world::SubChunk,
    layer: usize,
    local_x: u8,
    local_y: u8,
    local_z: u8,
) -> Option<&bedrock_world::BlockState> {
    let bedrock_world::SubChunkFormat::Paletted { storages, .. } = &subchunk.format else {
        return None;
    };
    storages
        .get(layer)
        .and_then(|storage| storage.block_state_at(local_x, local_y, local_z))
}

fn copied_chunk_legacy_block_state_at(
    terrain: &bedrock_world::LegacyTerrain,
    local_x: u8,
    world_y: i32,
    local_z: u8,
) -> Option<bedrock_world::BlockState> {
    if !(0..=127).contains(&world_y) {
        return None;
    }
    let local_y = u8::try_from(world_y).ok()?;
    let id = terrain.block_id_at(local_x, local_y, local_z)?;
    let mut states = BTreeMap::new();
    if let Some(data) = terrain.block_data_at(local_x, local_y, local_z) {
        states.insert("data".to_string(), bedrock_world::NbtTag::Byte(data as i8));
    }
    Some(bedrock_world::BlockState {
        name: format!("legacy:{id}"),
        states,
        version: None,
    })
}

fn copied_chunk_is_air_block_state_name(name: &str) -> bool {
    matches!(
        name,
        "air"
            | "cave_air"
            | "void_air"
            | "minecraft:air"
            | "minecraft:cave_air"
            | "minecraft:void_air"
            | "minecraft:structure_void"
            | "minecraft:light_block"
            | "minecraft:light"
    )
}

fn mcstructure_palette_index(
    structure: &mut bedrock_world::McStructureFile,
    palette_indices: &mut HashMap<String, i32>,
    entry: bedrock_world::McStructurePaletteEntry,
) -> bedrock_world::Result<i32> {
    let palette_key = mcstructure_palette_key(&entry);
    if let Some(index) = palette_indices.get(&palette_key) {
        return Ok(*index);
    }
    let index = i32::try_from(structure.palette.len()).map_err(|_| {
        bedrock_world::BedrockWorldError::Validation("结构 palette 超过 i32 上限".to_string())
    })?;
    structure.palette.push(entry);
    palette_indices.insert(palette_key, index);
    Ok(index)
}

fn mcstructure_palette_key(entry: &bedrock_world::McStructurePaletteEntry) -> String {
    let states = entry
        .states
        .iter()
        .map(|(key, value)| format!("{key}={value:?}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("{}|{}|{:?}", entry.name, states, entry.version)
}

fn insert_copied_block_entity_into_structure(
    structure: &mut bedrock_world::McStructureFile,
    entity: &ParsedBlockEntity,
    source_origin_x: i32,
    origin_y: i32,
    source_origin_z: i32,
) -> bedrock_world::Result<()> {
    let Some([world_x, world_y, world_z]) = entity.position else {
        return Ok(());
    };
    let local_x = world_x.saturating_sub(source_origin_x);
    let local_y = world_y.saturating_sub(origin_y);
    let local_z = world_z.saturating_sub(source_origin_z);
    let block_index = structure.size.index(local_x, local_y, local_z)?;
    let NbtTag::Compound(block_entity_data) = &entity.nbt else {
        return Ok(());
    };
    structure.block_position_data.insert(
        block_index.to_string(),
        NbtTag::Compound(indexmap::IndexMap::from([(
            "block_entity_data".to_string(),
            NbtTag::Compound(block_entity_data.clone()),
        )])),
    );
    Ok(())
}

pub(super) fn pasted_block_entity_for_target(
    entity: &ParsedBlockEntity,
    source_chunk: ChunkPos,
    target_chunk: ChunkPos,
) -> ParsedBlockEntity {
    let mut pasted = entity.clone();
    let Some([x, y, z]) = pasted.position else {
        return pasted;
    };
    let position = [
        x + (target_chunk.x - source_chunk.x) * 16,
        y,
        z + (target_chunk.z - source_chunk.z) * 16,
    ];
    pasted.position = Some(position);
    set_block_entity_nbt_position(&mut pasted.nbt, position);
    pasted
}

pub(super) fn pasted_hardcoded_spawn_area_for_target(
    area: ParsedHardcodedSpawnArea,
    source_chunk: ChunkPos,
    target_chunk: ChunkPos,
    transform: PasteTransform,
) -> ParsedHardcodedSpawnArea {
    let corners = [
        [area.min[0], area.min[1], area.min[2]],
        [area.min[0], area.min[1], area.max[2]],
        [area.min[0], area.max[1], area.min[2]],
        [area.min[0], area.max[1], area.max[2]],
        [area.max[0], area.min[1], area.min[2]],
        [area.max[0], area.min[1], area.max[2]],
        [area.max[0], area.max[1], area.min[2]],
        [area.max[0], area.max[1], area.max[2]],
    ];
    let mut min = [i32::MAX; 3];
    let mut max = [i32::MIN; 3];
    for corner in corners {
        let position =
            transform_chunk_block_position(corner, source_chunk, target_chunk, transform);
        for axis in 0..3 {
            min[axis] = min[axis].min(position[axis]);
            max[axis] = max[axis].max(position[axis]);
        }
    }
    ParsedHardcodedSpawnArea {
        kind: area.kind,
        min,
        max,
    }
}

fn set_block_entity_nbt_position(nbt: &mut NbtTag, position: [i32; 3]) {
    let NbtTag::Compound(root) = nbt else {
        return;
    };
    root.insert("x".to_string(), NbtTag::Int(position[0]));
    root.insert("y".to_string(), NbtTag::Int(position[1]));
    root.insert("z".to_string(), NbtTag::Int(position[2]));
}

fn transform_chunk_block_position(
    position: [i32; 3],
    source_chunk: ChunkPos,
    target_chunk: ChunkPos,
    transform: PasteTransform,
) -> [i32; 3] {
    let source_origin_x = source_chunk.x.saturating_mul(16);
    let source_origin_z = source_chunk.z.saturating_mul(16);
    let relative_x = position[0].saturating_sub(source_origin_x);
    let relative_z = position[2].saturating_sub(source_origin_z);
    let (relative_x, relative_z) =
        transform.transform_chunk_delta(relative_x.div_euclid(16), relative_z.div_euclid(16));
    let local_x = relative_x
        .saturating_mul(16)
        .saturating_add(position[0].rem_euclid(16));
    let local_z = relative_z
        .saturating_mul(16)
        .saturating_add(position[2].rem_euclid(16));
    [
        target_chunk.x.saturating_mul(16).saturating_add(local_x),
        position[1],
        target_chunk.z.saturating_mul(16).saturating_add(local_z),
    ]
}

pub(super) fn pasted_chunk_targets(
    copied_chunk: &CopiedChunkData,
    source_anchor: ChunkPos,
    target_anchor: ChunkPos,
    transform: PasteTransform,
) -> Vec<ChunkPos> {
    copied_chunk
        .chunks
        .iter()
        .map(|snapshot| {
            let delta_chunk_x = snapshot.chunk.x.saturating_sub(source_anchor.x);
            let delta_chunk_z = snapshot.chunk.z.saturating_sub(source_anchor.z);
            let (delta_chunk_x, delta_chunk_z) =
                transform.transform_chunk_delta(delta_chunk_x, delta_chunk_z);
            ChunkPos {
                x: target_anchor.x.saturating_add(delta_chunk_x),
                z: target_anchor.z.saturating_add(delta_chunk_z),
                dimension: target_anchor.dimension,
            }
        })
        .collect()
}

pub(super) fn chunk_record_tag_is_copy_safe(tag: ChunkRecordTag) -> bool {
    matches!(
        tag,
        ChunkRecordTag::Data3D
            | ChunkRecordTag::Data2D
            | ChunkRecordTag::Data2DLegacy
            | ChunkRecordTag::SubChunkPrefix
            | ChunkRecordTag::LegacyTerrain
            | ChunkRecordTag::BlockExtraData
            | ChunkRecordTag::BiomeState
            | ChunkRecordTag::FinalizedState
            | ChunkRecordTag::ConversionData
            | ChunkRecordTag::BorderBlocks
            | ChunkRecordTag::RandomTicks
            | ChunkRecordTag::Checksums
            | ChunkRecordTag::GenerationSeed
            | ChunkRecordTag::GeneratedPreCavesAndCliffsBlending
            | ChunkRecordTag::BlendingBiomeHeight
            | ChunkRecordTag::MetaDataHash
            | ChunkRecordTag::BlendingData
            | ChunkRecordTag::Version
            | ChunkRecordTag::VersionOld
            | ChunkRecordTag::LegacyVersion
    )
}

pub(super) fn copy_safe_chunk_records(records: Vec<ChunkRecord>) -> Vec<ChunkRecord> {
    records
        .into_iter()
        .filter(|record| chunk_record_tag_is_copy_safe(record.key.tag))
        .collect()
}

fn copy_chunk_parse_options() -> bedrock_world::WorldParseOptions {
    bedrock_world::WorldParseOptions {
        categories: bedrock_world::WorldParseCategories {
            chunks: true,
            players: false,
            actors: false,
            maps: false,
            villages: false,
            globals: false,
        },
        retention: bedrock_world::RetentionMode::Structured,
        subchunk_decode_mode: bedrock_world::SubChunkDecodeMode::CountsOnly,
        actor_resolution: bedrock_world::ActorResolution::None,
    }
}

pub(super) fn delete_chunk_record_with_guard(
    world: &BedrockWorld,
    chunk: ChunkPos,
    tag: ChunkRecordTag,
    _guard: &WriteGuard,
) -> bedrock_world::Result<()> {
    let key = bedrock_world::ChunkKey::new(chunk, tag);
    world.delete_raw_record_blocking(&key)
}

pub(super) fn clear_chunks_blocking(
    world: &BedrockWorld,
    bounds: SlimeChunkBounds,
    guard: &WriteGuard,
) -> bedrock_world::Result<usize> {
    bounds.validate()?;
    let mut cleared = 0usize;
    for chunk_z in bounds.min_chunk_z..=bounds.max_chunk_z {
        for chunk_x in bounds.min_chunk_x..=bounds.max_chunk_x {
            let chunk = ChunkPos {
                x: chunk_x,
                z: chunk_z,
                dimension: bounds.dimension,
            };
            clear_chunk_blocking(world, chunk, guard)?;
            cleared = cleared.saturating_add(1);
        }
    }
    Ok(cleared)
}

fn clear_chunk_blocking(
    world: &BedrockWorld,
    chunk: ChunkPos,
    _guard: &WriteGuard,
) -> bedrock_world::Result<()> {
    let actor_uids = world
        .actors_in_chunk_blocking(chunk)?
        .into_iter()
        .filter_map(|actor| actor.uid)
        .collect::<Vec<_>>();
    let mut transaction = world.transaction();
    for record in world.get_chunk_blocking(chunk)?.records {
        if chunk_record_tag_is_clear_target(record.key.tag) {
            transaction.delete_raw_record(&record.key);
        }
    }
    let actor_digest_key = ActorDigestKey::new(chunk).storage_key();
    for actor_uid in actor_uids {
        transaction.delete_raw_key(actor_uid.storage_key());
    }
    transaction.delete_raw_key(actor_digest_key);

    let empty_heightmap = default_heightmap();
    let chunk_version = chunk_version_for_empty_heightmap(chunk);
    let value = match chunk_version {
        ChunkVersion::Old => {
            bedrock_world::Biome2d::new(empty_heightmap.values.clone(), vec![0; 256])?.encode()?
        }
        ChunkVersion::New => {
            bedrock_world::Biome3d::new(empty_heightmap.values.clone(), Vec::new())?.encode()?
        }
    };
    let tag = match chunk_version {
        ChunkVersion::Old => ChunkRecordTag::Data2D,
        ChunkVersion::New => ChunkRecordTag::Data3D,
    };
    transaction.put_raw_record(&bedrock_world::ChunkKey::new(chunk, tag), value);
    let (min_subchunk_y, max_subchunk_y) = chunk.subchunk_index_range(chunk_version);
    if min_subchunk_y <= max_subchunk_y {
        let air_subchunk = air_subchunk_bytes()?;
        for subchunk_y in min_subchunk_y..=max_subchunk_y {
            transaction.put_raw_record(
                &bedrock_world::ChunkKey::subchunk(chunk, subchunk_y),
                air_subchunk.clone(),
            );
        }
    }
    transaction.put_raw_record(
        &bedrock_world::ChunkKey::new(chunk, ChunkRecordTag::FinalizedState),
        2_i32.to_le_bytes().to_vec(),
    );
    transaction.commit()
}

fn air_subchunk_bytes() -> bedrock_world::Result<Vec<u8>> {
    let mut bytes = vec![8, 1, 0];
    let air_palette = NbtTag::Compound(indexmap::IndexMap::from([
        (
            "name".to_string(),
            NbtTag::String("minecraft:air".to_string()),
        ),
        (
            "states".to_string(),
            NbtTag::Compound(indexmap::IndexMap::new()),
        ),
        ("version".to_string(), NbtTag::Int(1)),
    ]));
    bytes.extend_from_slice(&bedrock_world::NbtWriter::write_root(&air_palette)?);
    Ok(bytes)
}

fn chunk_version_for_empty_heightmap(chunk: ChunkPos) -> ChunkVersion {
    match chunk.dimension {
        Dimension::Nether | Dimension::End => ChunkVersion::Old,
        Dimension::Overworld | Dimension::Unknown(_) => ChunkVersion::New,
    }
}

pub(super) fn chunk_record_tag_is_clear_target(tag: ChunkRecordTag) -> bool {
    matches!(
        tag,
        ChunkRecordTag::Data3D
            | ChunkRecordTag::Data2D
            | ChunkRecordTag::Data2DLegacy
            | ChunkRecordTag::SubChunkPrefix
            | ChunkRecordTag::LegacyTerrain
            | ChunkRecordTag::BlockEntity
            | ChunkRecordTag::Entity
            | ChunkRecordTag::PendingTicks
            | ChunkRecordTag::BlockExtraData
            | ChunkRecordTag::BiomeState
            | ChunkRecordTag::ConversionData
            | ChunkRecordTag::HardcodedSpawners
            | ChunkRecordTag::BorderBlocks
            | ChunkRecordTag::RandomTicks
            | ChunkRecordTag::Checksums
            | ChunkRecordTag::GenerationSeed
            | ChunkRecordTag::GeneratedPreCavesAndCliffsBlending
            | ChunkRecordTag::BlendingBiomeHeight
            | ChunkRecordTag::BlendingData
            | ChunkRecordTag::ActorDigestVersion
            | ChunkRecordTag::MetaDataHash
    )
}

pub(super) fn confirming_quick_label(
    pending: Option<&QuickWriteAction>,
    action: QuickWriteAction,
    label: &'static str,
) -> String {
    if pending == Some(&action) {
        format!("确认{label}")
    } else {
        label.to_string()
    }
}

pub(super) fn hsa_kind_label(kind: HardcodedSpawnAreaKind) -> String {
    match kind {
        HardcodedSpawnAreaKind::NetherFortress => "NetherFortress".to_string(),
        HardcodedSpawnAreaKind::SwampHut => "SwampHut".to_string(),
        HardcodedSpawnAreaKind::OceanMonument => "OceanMonument".to_string(),
        HardcodedSpawnAreaKind::PillagerOutpost => "PillagerOutpost".to_string(),
        HardcodedSpawnAreaKind::Unknown(value) => format!("Unknown({value})"),
    }
}

pub(super) fn actor_source_label(source: &ActorSource) -> String {
    match source {
        ActorSource::InlineChunk(_) => "InlineChunk".to_string(),
        ActorSource::ActorPrefix(uid) => format!("ActorPrefix({})", uid.0),
    }
}

pub(super) fn global_kind_label(kind: &GlobalRecordKind) -> String {
    match kind {
        GlobalRecordKind::MobEvents => "mobevents".to_string(),
        GlobalRecordKind::Dimension(dimension) => dimension_label(*dimension),
        GlobalRecordKind::Scoreboard => "scoreboard".to_string(),
        GlobalRecordKind::LocalPlayer => "LocalPlayer".to_string(),
        GlobalRecordKind::AutonomousEntities => "autonomousentities".to_string(),
        GlobalRecordKind::BiomeData => "BiomeData".to_string(),
        GlobalRecordKind::LevelChunkMetaDataDictionary => {
            "LevelChunkMetaDataDictionary".to_string()
        }
        GlobalRecordKind::WorldClocks => "WorldClocks".to_string(),
        GlobalRecordKind::Other(name) => name.clone(),
    }
}

pub(super) fn edit_action_label(action: &EditAction) -> &'static str {
    match action {
        EditAction::Save => "save",
        EditAction::Delete => "delete",
    }
}

pub(super) fn edit_action_status(action: &EditAction, target: &EditTarget) -> String {
    let verb = match action {
        EditAction::Save => "保存",
        EditAction::Delete => "删除",
    };
    format!("{verb} {}", target.operation_label())
}

pub(super) fn tile_coords_for_chunks(
    chunks: &BTreeSet<ChunkPos>,
    layout: RenderLayout,
) -> Vec<(i32, i32)> {
    let chunks_per_tile = i32::try_from(layout.chunks_per_tile)
        .unwrap_or(CHUNKS_PER_TILE as i32)
        .max(1);
    chunks
        .iter()
        .map(|chunk| {
            (
                chunk.x.div_euclid(chunks_per_tile),
                chunk.z.div_euclid(chunks_per_tile),
            )
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(super) fn merge_chunks_into_tile_index(
    tile_chunk_index: &mut TileChunkIndex,
    tile_coord: (i32, i32),
    chunks: &BTreeSet<ChunkPos>,
    layout: RenderLayout,
) {
    let chunks_per_tile = i32::try_from(layout.chunks_per_tile)
        .unwrap_or(CHUNKS_PER_TILE as i32)
        .max(1);
    let mut positions = tile_chunk_index
        .remove(&tile_coord)
        .unwrap_or_default()
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    positions.extend(chunks.iter().copied().filter(|chunk| {
        (
            chunk.x.div_euclid(chunks_per_tile),
            chunk.z.div_euclid(chunks_per_tile),
        ) == tile_coord
    }));
    if positions.is_empty() {
        tile_chunk_index.remove(&tile_coord);
    } else {
        let positions = positions.into_iter().collect::<Vec<_>>();
        tile_chunk_index.insert(tile_coord, TileChunkPositions::from(positions));
    }
}

pub(super) fn pretty_json(value: serde_json::Value) -> SharedString {
    SharedString::from(
        serde_json::to_string_pretty(&value)
            .unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}")),
    )
}

pub(super) fn context_more_edit_entries(
    expanded: bool,
    items: Vec<ContextMenuItem>,
    on_toggle: impl Fn(&mut App) + 'static,
) -> Vec<ContextMenuEntry> {
    let mut entries = vec![ContextMenuEntry::item(
        ContextMenuItem::new(if expanded {
            "收起更多编辑操作"
        } else {
            "更多编辑操作"
        })
        .on_click(on_toggle),
    )];
    if expanded {
        entries.extend(items.into_iter().map(ContextMenuEntry::item));
    }
    entries
}
