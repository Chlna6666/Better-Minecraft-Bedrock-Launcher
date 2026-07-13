use super::model::*;
use super::panels::*;
use super::prelude::*;

impl MapViewerWindowView {
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
            self.status = SharedString::from("请先选择 chunk 范围");
            cx.notify();
            return;
        };
        if self.preview_3d.signature == Some(signature) && self.preview_3d.render_in_flight {
            self.preview_3d.status = Preview3dStatus::Loading(Preview3dBuildStatus::new(
                "加载中",
                "已有预览任务正在运行",
            ));
            self.status = SharedString::from("正在加载 3D 预览...");
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
            "准备",
            self.preview_3d_selection_status(),
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
        self.status = SharedString::from("正在加载 3D 预览...");
        cx.notify();

        cx.spawn(async move |handle, cx| {
            let (event_sender, mut event_receiver) = unbounded::<Preview3dLoadEvent>();
            let complete_sender = event_sender.clone();
            let load_task = cx.background_spawn(async move {
                let result = load_preview_3d_mesh_blocking_incremental(
                    &world_path,
                    signature.bounds,
                    Some(preview_cancel_for_load),
                    {
                        let event_sender = event_sender.clone();
                        move |mesh, status| {
                            if event_sender
                                .unbounded_send(Preview3dLoadEvent::Chunk { mesh, status })
                                .is_err()
                            {
                                tracing::debug!("preview 3d incremental receiver was dropped");
                            }
                        }
                    },
                )
                .map(Arc::new);
                if complete_sender
                    .unbounded_send(Preview3dLoadEvent::Complete(result))
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
                            this.status = SharedString::from(format!(
                                "正在拼接 3D 预览: {} {}",
                                status.phase, status.detail
                            ));
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
        let imported_region = self
            .professional
            .imported_region_package
            .then(|| self.professional.copied_chunk.clone())
            .flatten();
        if imported_structure.is_none() && imported_region.is_none() {
            self.status = SharedString::from("没有可加载的导入预览");
            cx.notify();
            return;
        }
        self.preview_3d.source = Preview3dSource::ImportPreview;
        if self.preview_3d.render_in_flight {
            self.preview_3d.status = Preview3dStatus::Loading(Preview3dBuildStatus::new(
                "加载中",
                "已有导入预览任务正在运行",
            ));
            self.status = SharedString::from("正在加载导入 3D 预览...");
            cx.notify();
            return;
        }

        self.preview_3d.generation = self.preview_3d.generation.saturating_add(1);
        let generation = self.preview_3d.generation;
        if let Some(cancel) = self.preview_3d.cancel.take() {
            cancel.cancel();
        }
        self.preview_3d.status =
            Preview3dStatus::Loading(Preview3dBuildStatus::new("准备", "导入模型"));
        self.preview_3d.mesh = None;
        self.preview_3d.signature = None;
        #[cfg(target_os = "windows")]
        self.preview_3d.clear_surface();
        self.preview_3d.render_in_flight = true;
        self.status = SharedString::from("正在加载导入 3D 预览...");
        cx.notify();

        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    if let Some(imported_structure) = imported_structure {
                        load_preview_3d_mesh_from_mcstructure_blocking(
                            &imported_structure.structure,
                            imported_structure.source_anchor,
                            imported_structure.origin_y,
                        )
                        .map(Arc::new)
                    } else if let Some(imported_region) = imported_region {
                        load_preview_3d_mesh_from_copied_chunk_blocking(&imported_region)
                            .map(Arc::new)
                    } else {
                        Err("没有可加载的导入预览".to_string())
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
                    self.preview_3d.status = Preview3dStatus::NoSurface(SharedString::from(
                        format!(
                            "选区没有可显示体素面，面 {face_count}+{glass_faces}+{water_faces}+{lava_faces}"
                        ),
                    ));
                    self.status = SharedString::from("3D 预览没有可显示体素面");
                } else {
                    self.preview_3d.status = Preview3dStatus::Ready;
                    let clipped = if omitted_faces > 0 {
                        format!("，动态裁剪 {omitted_faces} 面")
                    } else {
                        String::new()
                    };
                    let truncated = if truncated_chunks > 0 {
                        format!("，动态限制 {truncated_chunks} chunks")
                    } else {
                        String::new()
                    };
                    self.status = SharedString::from(format!(
                        "3D 预览已生成，方块 {block_count}，玻璃 {glass_blocks}，水 {water_blocks}，岩浆 {lava_blocks}，面 {face_count}+{glass_faces}+{water_faces}+{lava_faces}{clipped}{truncated}，后端 GPUI GPU"
                    ));
                }
            }
            Err(error) => {
                self.preview_3d.status = Preview3dStatus::Error(SharedString::from(error.clone()));
                self.preview_3d.mesh = None;
                #[cfg(target_os = "windows")]
                self.preview_3d.clear_surface();
                self.preview_3d.render_in_flight = false;
                self.status = SharedString::from(format!("3D 预览失败: {error}"));
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
            self.preview_3d.camera.zoom_by_for_mesh(factor, mesh);
        } else {
            self.preview_3d.camera.zoom_by(factor);
        }
        cx.notify();
    }

    pub(super) fn preview_3d_press_navigation_key(
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
        request_animation_frame_if(window, true);
        cx.notify();
    }

    pub(super) fn preview_3d_release_navigation_key(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.preview_3d.movement_input.set_key_pressed(key, false) != Some(true) {
            return;
        }
        if !self.preview_3d.movement_input.any_active() {
            self.preview_3d.last_motion_frame_at = None;
        }
        cx.notify();
    }

    pub(super) fn preview_3d_sync_modifier_navigation(
        &mut self,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
            self.preview_3d_release_navigation_key("shift", cx);
            return;
        }

        if modifiers.shift {
            self.preview_3d_press_navigation_key("shift", modifiers, window, cx);
        } else {
            self.preview_3d_release_navigation_key("shift", cx);
        }
    }

    pub(super) fn preview_3d_selection_status(&self) -> SharedString {
        if self.preview_3d.source == Preview3dSource::ImportPreview {
            if let Some(import) = self.professional.imported_structure.as_ref() {
                return SharedString::from(format!(
                    "导入结构: {}x{}x{} · 锚点 {},{} · Y {}",
                    import.structure.size.x,
                    import.structure.size.y,
                    import.structure.size.z,
                    import.source_anchor.x,
                    import.source_anchor.z,
                    import.origin_y
                ));
            }
            if self.professional.imported_region_package {
                if let Some(copied_chunk) = self.professional.copied_chunk.as_ref() {
                    return SharedString::from(format!(
                        "导入区域包: {} chunks · 锚点 {},{}",
                        copied_chunk.chunk_count(),
                        copied_chunk.source.x,
                        copied_chunk.source.z
                    ));
                }
            }
        }
        let Some(signature) = self.current_preview_3d_signature() else {
            return SharedString::from("未选择 chunk，右键地图设置选区起点和终点");
        };
        let width = preview_3d_bounds_width(signature.bounds);
        let depth = preview_3d_bounds_depth(signature.bounds);
        SharedString::from(format!(
            "{}: {}x{} chunks",
            dimension_label(signature.bounds.dimension),
            width,
            depth
        ))
    }

    pub(super) fn preview_3d_status_label(&self) -> SharedString {
        match &self.preview_3d.status {
            Preview3dStatus::Idle => SharedString::from("待加载"),
            Preview3dStatus::Loading(progress) => {
                SharedString::from(format!("{} {}", progress.phase, progress.detail))
            }
            Preview3dStatus::Ready => SharedString::from("已就绪"),
            Preview3dStatus::NoSurface(message) => SharedString::from(format!("无表面: {message}")),
            Preview3dStatus::Error(error) => SharedString::from(format!("错误: {error}")),
        }
    }

    pub(super) fn preview_3d_stats_label(&self) -> SharedString {
        let Some(mesh) = self.preview_3d.mesh.as_ref() else {
            return SharedString::from("体素网格未生成");
        };
        let clipped = if mesh.omitted_face_count > 0 {
            format!(
                " · 已裁剪面 {} · 顶点预算 {}",
                mesh.omitted_face_count, mesh.vertex_budget
            )
        } else {
            String::new()
        };
        let truncated = if mesh.truncated_chunk_count > 0 {
            format!(" · 已截断区块 {}", mesh.truncated_chunk_count)
        } else {
            String::new()
        };
        SharedString::from(format!(
            "区块 已绘制 {}/{} · 已处理 {} · 子区块 {} · 方块 {} 玻璃 {} 水 {} 岩浆 {} · 面 {} 玻璃 {} 水 {} 岩浆 {} · 分片 {} · 顶点 {} · 总预算 {} 顶点 · 剔除内部面 {}{}{} · Y {}..{} · 缺失 {} · GPUI GPU · 视角 {:.2},{:.2},{:.2} · 镜头偏航 {:.1} 俯仰 {:.1} · 模型偏航 {:.1} 俯仰 {:.1} · 缩放 {:.2}",
            mesh.rendered_chunk_count(),
            mesh.chunk_count,
            mesh.processed_chunk_count,
            mesh.subchunk_count,
            mesh.solid_block_count,
            mesh.glass_block_count,
            mesh.water_block_count,
            mesh.lava_block_count,
            mesh.face_count,
            mesh.glass_face_count,
            mesh.water_face_count,
            mesh.lava_face_count,
            mesh.chunk_meshes.len(),
            mesh.vertex_count(),
            mesh.vertex_budget,
            mesh.culled_face_count,
            clipped,
            truncated,
            mesh.min_y,
            mesh.max_y,
            mesh.missing_chunks,
            self.preview_3d.camera.position[0],
            self.preview_3d.camera.position[1],
            self.preview_3d.camera.position[2],
            self.preview_3d.camera.yaw.to_degrees(),
            self.preview_3d.camera.pitch.to_degrees(),
            self.preview_3d.model_rotation.yaw.to_degrees(),
            self.preview_3d.model_rotation.pitch.to_degrees(),
            self.preview_3d.camera.zoom,
        ))
    }

    pub(super) fn preview_3d_empty_label(&self) -> SharedString {
        match &self.preview_3d.status {
            Preview3dStatus::Idle => {
                SharedString::from("点击“加载/刷新”生成 3D 预览；未加载时不占用 3D 构建资源")
            }
            Preview3dStatus::Loading(progress) => {
                SharedString::from(format!("{}: {}", progress.phase, progress.detail))
            }
            Preview3dStatus::Ready => SharedString::from("预览帧为空"),
            Preview3dStatus::NoSurface(message) => message.clone(),
            Preview3dStatus::Error(error) => error.clone(),
        }
    }
}
