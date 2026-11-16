use super::model::*;
use super::panels::*;
use super::prelude::*;
use futures::channel::mpsc::channel;
use futures_util::SinkExt as _;

impl View {
    pub(super) fn invalidate_preview_3d_mesh(&mut self) {
        self.clear_preview_3d_resources(false);
    }

    pub(super) fn clear_preview_3d_resources(&mut self, clear_pipeline: bool) {
        self.preview_3d.generation = self.preview_3d.generation.saturating_add(1);
        self.preview_3d.clear_resources(clear_pipeline);
    }

    pub(super) fn current_preview_3d_signature(&self) -> Option<Preview3dSelectionSignature> {
        self.professional
            .selection
            .map(|selection| Preview3dSelectionSignature {
                bounds: selection.bounds(),
            })
    }

    pub(super) fn refresh_preview_3d(&mut self, cx: &mut Context<Self>) {
        self.preview_3d.source = Preview3dSource::Selection;
        let Some(signature) = self.current_preview_3d_signature() else {
            self.clear_preview_3d_resources(false);
            self.status = t!("MapViewer.import.preview_panel.select_chunk_range_first");
            cx.notify();
            return;
        };
        if self.preview_3d.signature == Some(signature) && self.preview_3d.render_in_flight {
            self.preview_3d.status = Preview3dStatus::Loading(Preview3dBuildStatus::new(
                t!("MapViewer.import.preview_panel.loading"),
                t!("MapViewer.import.preview_panel.preview_task_running"),
            ));
            self.status = t!("MapViewer.import.preview_panel.loading_preview_3d");
            cx.notify();
            return;
        }
        self.preview_3d.generation = self.preview_3d.generation.saturating_add(1);
        let generation = self.preview_3d.generation;
        let world_path = self.world_path.clone();
        if let Some(cancel) = self.preview_3d.cancel.take() {
            cancel.cancel();
        }
        let preview_cancel = CancelFlag::new();
        let preview_cancel_for_load = preview_cancel.clone();
        let preview_cancel_for_owner = preview_cancel.clone();
        let is_same_signature = self.preview_3d.signature == Some(signature);
        self.preview_3d.status = Preview3dStatus::Loading(Preview3dBuildStatus::new(
            t!("MapViewer.import.preview_panel.preparing"),
            self.selection_status(),
        ));
        self.preview_3d.signature = Some(signature);
        if !is_same_signature {
            self.preview_3d.mesh = None;
            #[cfg(target_os = "windows")]
            self.preview_3d.clear_surface();
            self.preview_3d.reset_view_and_model();
        }
        self.preview_3d.render_in_flight = true;
        self.preview_3d.cancel = Some(preview_cancel.clone());
        self.status = t!("MapViewer.import.preview_panel.loading_preview_3d");
        cx.notify();

        let query_budget = self.map_query_budget.clone();
        cx.spawn(async move |handle, cx| {
            let _query_permit = query_budget.acquire().await;
            let (event_sender, mut event_receiver) = channel::<Preview3dLoadEvent>(1);
            let mut complete_sender = event_sender.clone();
            let load_task = cx.background_spawn(async move {
                let result = super::preview_3d::load_mesh(
                    Preview3dLoadOptions {
                        world_path: &world_path,
                        bounds: signature.bounds,
                        block_models: None,
                        cancel: Some(preview_cancel_for_load),
                        for_export: false,
                    },
                    {
                        let mut event_sender = event_sender.clone();
                        move |mesh, status| {
                            // A newer full snapshot can be skipped while the UI still owns one.
                            // The final result is sent separately and must always be delivered.
                            let _ =
                                event_sender.try_send(Preview3dLoadEvent::Chunk { mesh, status });
                        }
                    },
                )
                .map(Arc::new);
                if complete_sender
                    .send(Preview3dLoadEvent::Complete(result))
                    .await
                    .is_err()
                {
                    tracing::debug!("preview 3d completion receiver was dropped");
                }
            });
            while let Some(event) = event_receiver.next().await {
                let is_complete = matches!(event, Preview3dLoadEvent::Complete(_));
                let Some(view) = handle.upgrade() else {
                    preview_cancel_for_owner.cancel();
                    load_task.detach();
                    return Ok(());
                };
                view.update(cx, move |this, cx| {
                    if this.preview_3d.generation != generation {
                        return;
                    }
                    match event {
                        Preview3dLoadEvent::Chunk { mesh, status } => {
                            this.preview_3d.mesh = Some(mesh);
                            this.preview_3d.status = Preview3dStatus::Loading(status.clone());
                            this.status = t!(
                                "MapViewer.import.preview_panel.assembling_preview_3d",
                                phase = status.phase,
                                detail = status.detail,
                            );
                        }
                        Preview3dLoadEvent::Complete(result) => {
                            this.finish_preview_3d_load(result);
                            this.preview_3d.cancel = None;
                        }
                    }
                    cx.notify();
                })?;
                if is_complete {
                    break;
                }
            }
            load_task.await;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn refresh_import_preview_3d(&mut self, cx: &mut Context<Self>) {
        let imported_structure = self.professional.imported_structure.clone();
        let copied_chunk = self.professional.copied_chunk.clone();
        if imported_structure.is_none() && copied_chunk.is_none() {
            self.status = t!("MapViewer.import.preview_panel.no_import_preview");
            cx.notify();
            return;
        }
        let import_already_loading = self.preview_3d.source == Preview3dSource::ImportPreview
            && self.preview_3d.render_in_flight;
        self.preview_3d.source = Preview3dSource::ImportPreview;
        if import_already_loading {
            self.preview_3d.status = Preview3dStatus::Loading(Preview3dBuildStatus::new(
                t!("MapViewer.import.preview_panel.loading"),
                t!("MapViewer.import.preview_panel.import_preview_task_running"),
            ));
            self.status = t!("MapViewer.import.preview_panel.loading_import_preview_3d");
            cx.notify();
            return;
        }

        self.preview_3d.generation = self.preview_3d.generation.saturating_add(1);
        let generation = self.preview_3d.generation;
        if let Some(cancel) = self.preview_3d.cancel.take() {
            cancel.cancel();
        }
        self.preview_3d.status = Preview3dStatus::Loading(Preview3dBuildStatus::new(
            t!("MapViewer.import.preview_panel.preparing"),
            t!("MapViewer.import.preview_panel.import_model"),
        ));
        self.preview_3d.mesh = None;
        self.world3d.import_mesh_origin_y = imported_structure.as_ref().map(|data| data.origin_y);
        self.sync_world3d_import_preview(cx);
        self.preview_3d.signature = None;
        #[cfg(target_os = "windows")]
        self.preview_3d.clear_surface();
        self.preview_3d.render_in_flight = true;
        self.status = t!("MapViewer.import.preview_panel.loading_import_preview_3d");
        cx.notify();

        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    if let Some(imported_structure) = imported_structure {
                        super::preview_3d::build_preview_mesh(Preview3dMeshSource::Structure {
                            structure: &imported_structure.structure,
                            anchor_chunk: imported_structure.source_anchor,
                            origin_y: imported_structure.origin_y,
                        })
                        .map(Arc::new)
                    } else if let Some(copied_chunk) = copied_chunk {
                        super::preview_3d::build_preview_mesh(Preview3dMeshSource::CopiedChunk(
                            &copied_chunk,
                        ))
                        .map(Arc::new)
                    } else {
                        Err(t!("MapViewer.import.preview_panel.no_import_preview").to_string())
                    }
                })
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if this.preview_3d.generation != generation {
                    return;
                }
                this.finish_preview_3d_load(result);
                this.preview_3d.cancel = None;
                let transform = this
                    .professional
                    .paste_preview
                    .as_ref()
                    .map_or(PasteTransform::default(), |preview| preview.transform);
                this.sync_import_preview_3d_transform(transform);
                this.sync_world3d_import_preview(cx);
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn finish_preview_3d_load(&mut self, result: Result<Arc<Preview3dMesh>, String>) {
        match result {
            Ok(mesh) => {
                let face_count = mesh.face_count;
                let glass_faces = mesh.glass_face_count;
                let block_count = mesh.solid_block_count;
                let glass_blocks = mesh.glass_block_count;
                let water_blocks = mesh.water_block_count;
                let lava_blocks = mesh.lava_block_count;
                let water_faces = mesh.water_face_count;
                let lava_faces = mesh.lava_face_count;
                let omitted_faces = mesh.omitted_face_count;
                let truncated_chunks = mesh.truncated_chunk_count;
                self.preview_3d.mesh = Some(mesh);
                self.preview_3d.render_in_flight = false;
                if face_count == 0 && glass_faces == 0 && water_faces == 0 && lava_faces == 0 {
                    self.preview_3d.status = Preview3dStatus::NoSurface(t!(
                        "MapViewer.import.preview_panel.selection_no_visible_faces",
                        face_count = face_count,
                        glass_faces = glass_faces,
                        water_faces = water_faces,
                        lava_faces = lava_faces,
                    ));
                    self.status = t!("MapViewer.import.preview_panel.preview_no_visible_faces");
                } else {
                    self.preview_3d.status = Preview3dStatus::Ready;
                    let clipped = if omitted_faces > 0 {
                        t!(
                            "MapViewer.import.preview_panel.clipped_faces_suffix",
                            count = omitted_faces
                        )
                        .to_string()
                    } else {
                        String::new()
                    };
                    let truncated = if truncated_chunks > 0 {
                        t!(
                            "MapViewer.import.preview_panel.truncated_chunks_suffix",
                            count = truncated_chunks
                        )
                        .to_string()
                    } else {
                        String::new()
                    };
                    self.status = t!(
                        "MapViewer.import.preview_panel.preview_ready_summary",
                        blocks = block_count,
                        glass_blocks = glass_blocks,
                        water_blocks = water_blocks,
                        lava_blocks = lava_blocks,
                        faces = face_count,
                        glass_faces = glass_faces,
                        water_faces = water_faces,
                        lava_faces = lava_faces,
                        clipped = clipped,
                        truncated = truncated,
                    );
                }
            }
            Err(error) => {
                self.preview_3d.status = Preview3dStatus::Error(SharedString::from(error.clone()));
                self.preview_3d.mesh = None;
                #[cfg(target_os = "windows")]
                self.preview_3d.clear_surface();
                self.preview_3d.render_in_flight = false;
                self.status = t!(
                    "MapViewer.import.preview_panel.preview_failed",
                    error = error,
                );
            }
        }
    }

    pub(super) fn reset_preview_3d_camera(&mut self, cx: &mut Context<Self>) {
        self.preview_3d.reset_view_and_model();
        let transform = self
            .professional
            .paste_preview
            .as_ref()
            .map_or(PasteTransform::default(), |preview| preview.transform);
        self.sync_import_preview_3d_transform(transform);
        cx.notify();
    }

    pub(super) fn preview_3d_begin_drag(
        &mut self,
        mode: Preview3dDragMode,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.preview_3d.drag = Some(Preview3dDragState { mode, position });
        cx.notify();
    }

    pub(super) fn preview_3d_orbit_camera_to(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.preview_3d.drag else {
            return;
        };
        if drag.mode != Preview3dDragMode::OrbitCamera {
            return;
        }
        let delta_x = (position.x - drag.position.x) / px(1.0);
        let delta_y = (position.y - drag.position.y) / px(1.0);
        self.preview_3d.camera.rotate_view(delta_x, delta_y);
        self.preview_3d.drag = Some(Preview3dDragState {
            mode: drag.mode,
            position,
        });
        cx.notify();
    }

    pub(super) fn preview_3d_rotate_model_to(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.preview_3d.drag else {
            return;
        };
        if drag.mode != Preview3dDragMode::RotateModel {
            return;
        }
        let delta_x = (position.x - drag.position.x) / px(1.0);
        let delta_y = (position.y - drag.position.y) / px(1.0);
        self.preview_3d.model_rotation.rotate_drag(delta_x, delta_y);
        self.preview_3d.drag = Some(Preview3dDragState {
            mode: drag.mode,
            position,
        });
        cx.notify();
    }

    pub(super) fn preview_3d_zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        if let Some(mesh) = self.preview_3d.mesh.as_ref() {
            let span = mesh.horizontal_span().max(1.0);
            let dynamic_max = 64.0_f32.max(span / 4.0).min(4096.0);
            self.preview_3d.camera.zoom =
                (self.preview_3d.camera.zoom * factor).clamp(0.05, dynamic_max);
        } else {
            self.preview_3d.camera.zoom_by(factor);
        }
        cx.notify();
    }

    pub(super) fn press_navigation_key(
        &mut self,
        key: &str,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
            return;
        }
        if self.preview_3d.movement_input.set_key_pressed(key, true) != Some(true) {
            return;
        }
        self.preview_3d.last_motion_frame_at = None;
        request_layout_frame(window);
        cx.notify();
    }

    pub(super) fn release_navigation_key(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.preview_3d.movement_input.set_key_pressed(key, false) != Some(true) {
            return;
        }
        if !self.preview_3d.movement_input.any_active() {
            self.preview_3d.last_motion_frame_at = None;
        }
        cx.notify();
    }

    pub(super) fn sync_modifier_navigation(
        &mut self,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
            self.release_navigation_key("shift", cx);
            return;
        }

        if modifiers.shift {
            self.press_navigation_key("shift", modifiers, window, cx);
        } else {
            self.release_navigation_key("shift", cx);
        }
    }

    pub(super) fn selection_status(&self) -> SharedString {
        if self.preview_3d.source == Preview3dSource::ImportPreview {
            if let Some(import) = self.professional.imported_structure.as_ref() {
                return t!(
                    "MapViewer.import.preview_panel.imported_structure_status",
                    x = import.structure.size.x,
                    y = import.structure.size.y,
                    z = import.structure.size.z,
                    anchor_x = import.source_anchor.x,
                    anchor_z = import.source_anchor.z,
                    origin_y = import.origin_y,
                );
            }
            if self.professional.imported_region_package {
                if let Some(copied_chunk) = self.professional.copied_chunk.as_ref() {
                    return t!(
                        "MapViewer.import.preview_panel.imported_region_package_status",
                        chunks = copied_chunk.chunk_count(),
                        anchor_x = copied_chunk.source.x,
                        anchor_z = copied_chunk.source.z,
                    );
                }
            }
        }
        let Some(signature) = self.current_preview_3d_signature() else {
            return t!("MapViewer.import.preview_panel.no_chunk_selection_hint");
        };
        let width = super::preview_3d_source::bounds_width(signature.bounds);
        let depth = super::preview_3d_source::bounds_depth(signature.bounds);
        SharedString::from(format!(
            "{}: {}x{} chunks",
            dimension_label(signature.bounds.dimension),
            width,
            depth
        ))
    }

    pub(super) fn preview_3d_status_label(&self) -> SharedString {
        match &self.preview_3d.status {
            Preview3dStatus::Idle => t!("MapViewer.import.preview_panel.status_idle"),
            Preview3dStatus::Loading(progress) => {
                if progress.phase.is_empty() && progress.detail.is_empty() {
                    t!("MapViewer.import.preview_panel.loading")
                } else if progress.phase.is_empty() {
                    progress.detail.clone()
                } else if progress.detail.is_empty() {
                    progress.phase.clone()
                } else {
                    SharedString::from(format!("{} {}", progress.phase, progress.detail))
                }
            }
            Preview3dStatus::Ready => t!("MapViewer.import.preview_panel.status_ready"),
            Preview3dStatus::NoSurface(message) => t!(
                "MapViewer.import.preview_panel.status_no_surface",
                message = message,
            ),
            Preview3dStatus::Error(error) => {
                t!("MapViewer.import.preview_panel.status_error", error = error)
            }
        }
    }

    pub(super) fn preview_3d_stats_label(&self) -> SharedString {
        let Some(mesh) = self.preview_3d.mesh.as_ref() else {
            return t!("MapViewer.import.preview_panel.stats_mesh_missing");
        };
        let clipped = if mesh.omitted_face_count > 0 {
            t!(
                "MapViewer.import.preview_panel.stats_clipped_suffix",
                faces = mesh.omitted_face_count,
                budget = mesh.vertex_budget,
            )
            .to_string()
        } else {
            String::new()
        };
        let truncated = if mesh.truncated_chunk_count > 0 {
            t!(
                "MapViewer.import.preview_panel.stats_truncated_suffix",
                count = mesh.truncated_chunk_count,
            )
            .to_string()
        } else {
            String::new()
        };
        t!(
            "MapViewer.import.preview_panel.stats_summary",
            rendered = mesh.rendered_chunk_count(),
            chunks = mesh.chunk_count,
            processed = mesh.processed_chunk_count,
            subchunks = mesh.subchunk_count,
            blocks = mesh.solid_block_count,
            glass = mesh.glass_block_count,
            water = mesh.water_block_count,
            lava = mesh.lava_block_count,
            faces = mesh.face_count,
            glass_faces = mesh.glass_face_count,
            water_faces = mesh.water_face_count,
            lava_faces = mesh.lava_face_count,
            meshes = mesh.chunk_meshes.len(),
            vertices = mesh.vertex_count(),
            budget = mesh.vertex_budget,
            culled = mesh.culled_face_count,
            clipped = clipped,
            truncated = truncated,
            min_y = mesh.min_y,
            max_y = mesh.max_y,
            missing = mesh.missing_chunks,
            cam_x = format!("{:.2}", self.preview_3d.camera.position[0]),
            cam_y = format!("{:.2}", self.preview_3d.camera.position[1]),
            cam_z = format!("{:.2}", self.preview_3d.camera.position[2]),
            cam_yaw = format!("{:.1}", self.preview_3d.camera.yaw.to_degrees()),
            cam_pitch = format!("{:.1}", self.preview_3d.camera.pitch.to_degrees()),
            model_yaw = format!("{:.1}", self.preview_3d.model_rotation.yaw.to_degrees()),
            model_pitch = format!("{:.1}", self.preview_3d.model_rotation.pitch.to_degrees()),
            zoom = format!("{:.2}", self.preview_3d.camera.zoom),
        )
    }

    pub(super) fn preview_3d_empty_label(&self) -> SharedString {
        match &self.preview_3d.status {
            Preview3dStatus::Idle => t!("MapViewer.import.preview_panel.empty_idle_hint"),
            Preview3dStatus::Loading(progress) => {
                SharedString::from(format!("{}: {}", progress.phase, progress.detail))
            }
            Preview3dStatus::Ready => t!("MapViewer.import.preview_panel.empty_frame"),
            Preview3dStatus::NoSurface(message) => message.clone(),
            Preview3dStatus::Error(error) => error.clone(),
        }
    }
}
