//! 3D writes participate in the map document's existing write ownership.
use super::*;

fn publish_outcome(
    editor: Option<WeakEntity<World3dEditorView>>,
    outcome: World3dEditOutcome,
    cx: &mut Context<MapViewerWindowView>,
) {
    if let Some(editor) = editor {
        // A rejected request may still be inside the editor's mouse event update.
        cx.defer(move |cx| {
            let _ = editor.update(cx, |editor, cx| editor.on_edit_finished(outcome, cx));
        });
    }
}

impl MapViewerWindowView {
    /// Executes a 3D edit under the same write ownership as 2D operations.
    pub(crate) fn apply_world3d_edit_request(
        &mut self,
        request: World3dBlockEditRequest,
        cx: &mut Context<Self>,
    ) {
        let editor = self
            .world3d
            .editor
            .clone()
            .filter(|_| request.request_id != 0);
        if self.map_write_busy() {
            self.status = "文档已有写入或历史操作，编辑未提交".into();
            cx.notify();
            publish_outcome(
                editor,
                World3dEditOutcome {
                    request_id: request.request_id,
                    ok: false,
                    message: "2D 文档已有写入任务，3D 编辑未提交".into(),
                    affected_chunks: Vec::new(),
                },
                cx,
            );
            return;
        }
        let label = match &request.kind {
            World3dBlockEditKind::Fill { block_name } => format!("3D 填充 {block_name}"),
            World3dBlockEditKind::Clear => "3D 清除选区".into(),
        };
        let task_id = task_manager::create_task_with_details(
            None,
            label.clone(),
            Some(format!("via 2D 文档事务 · 维度 {}", request.dimension.id())),
            "map_write",
            None,
            false,
        );
        self.professional.active_write_task_id = Some(task_id.clone());
        if let Some(snapshot) = task_manager::get_snapshot(&task_id) {
            self.task_snapshots
                .insert(snapshot.id.clone(), Arc::new(snapshot));
        }
        self.status = format!("正在{label}...").into();
        self.spawn_world3d_write(request, task_id, editor, cx);
        cx.notify();
    }

    fn spawn_world3d_write(
        &self,
        request: World3dBlockEditRequest,
        task_id: String,
        editor: Option<WeakEntity<World3dEditorView>>,
        cx: &mut Context<Self>,
    ) {
        let path = self.world_path.clone();
        let request_id = request.request_id;
        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move { apply_world3d_edit_blocking(&path, &request) })
                .await;
            // Always finish the task, even if the UI was closed or metadata reloaded.
            let (status, message) = match &result {
                Ok((_, message, _)) => ("completed", message.clone()),
                Err(error) => ("failed", error.clone()),
            };
            task_manager::finish_task(&task_id, status, Some(message));
            if let Some(view) = handle.upgrade() {
                view.update(cx, move |this, cx| {
                    let outcome = match result {
                        Ok((invalidation, message, affected_chunks)) => {
                            this.apply_map_edit_invalidation(&invalidation, cx);
                            World3dEditOutcome {
                                request_id,
                                ok: true,
                                message,
                                affected_chunks,
                            }
                        }
                        Err(message) => World3dEditOutcome {
                            request_id,
                            ok: false,
                            message,
                            affected_chunks: Vec::new(),
                        },
                    };
                    this.status = outcome.message.clone().into();
                    // The completion belongs to the editor which submitted it, even
                    // if the user closed and reopened a 3D window during the write.
                    publish_outcome(editor, outcome, cx);
                    cx.notify();
                })?;
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }
}
