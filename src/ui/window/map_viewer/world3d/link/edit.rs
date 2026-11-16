//! 3D writes participate in the map document's existing write ownership.
use crate::tasks::TaskStage;
use super::*;

fn publish_outcome(
    editor: Option<WeakEntity<World3dEditorView>>,
    outcome: World3dEditOutcome,
    cx: &mut Context<View>,
) {
    if let Some(editor) = editor {
        // A rejected request may still be inside the editor's mouse event update.
        cx.defer(move |cx| {
            let _ = editor.update(cx, |editor, cx| editor.on_edit_finished(outcome, cx));
        });
    }
}

impl View {
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
            self.status = t!("MapViewer.world3d.edit.status_document_busy");
            cx.notify();
            publish_outcome(
                editor,
                World3dEditOutcome {
                    request_id: request.request_id,
                    ok: false,
                    message: t!("MapViewer.world3d.edit.outcome_document_busy").to_string(),
                    affected_chunks: Vec::new(),
                },
                cx,
            );
            return;
        }
        let label = match &request.kind {
            World3dBlockEditKind::Fill { block_name } => {
                t!("MapViewer.world3d.edit.label_fill", block = block_name).to_string()
            }
            World3dBlockEditKind::Ellipsoid { block_name } => {
                t!("MapViewer.world3d.edit.label_ellipsoid_fill", block = block_name).to_string()
            }
            World3dBlockEditKind::Clear => {
                t!("MapViewer.world3d.edit.label_clear_selection").to_string()
            }
        };
        let task_id = task_manager::create_task(
            task_manager::TaskOptions::new(label.clone(), TaskStage::MAP_WRITE).detail(
                t!(
                    "MapViewer.world3d.edit.task_detail",
                    dimension = request.dimension.id()
                )
                .to_string(),
            ),
        )
        .expect("task id must be unique");
        self.professional.active_write_task_id = Some(task_id.clone());
        if let Some(snapshot) = task_manager::snapshot(&task_id) {
            self.task_snapshots
                .insert(snapshot.id.clone(), Arc::new(snapshot));
        }
        self.status = t!("MapViewer.world3d.edit.applying", label = label);
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
                .background_spawn(async move { apply_edit(&path, &request) })
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
                            this.apply_map_edit_invalidation(
                                &invalidation,
                                super::super::super::tile_state::TilePriority::Visible,
                                false,
                                cx,
                            );
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
