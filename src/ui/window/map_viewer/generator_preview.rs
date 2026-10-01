use std::sync::Arc;

use super::mcstructure;
use super::model::{MapViewerWindowView, PasteRotation, PasteTransform, paste_rotation_degrees};
use super::prelude::*;
use crate::core::minecraft::map::generator::{get_structure_preview, release_structure_preview};

impl MapViewerWindowView {
    pub(super) fn clear_live_structure_preview(&mut self, cx: &mut Context<Self>) {
        if !self.generator.preview_result_active && !self.image_generator.preview_result_active {
            return;
        }
        if let Some(anchor) = self
            .professional
            .paste_preview
            .as_ref()
            .map(|preview| preview.target_anchor)
        {
            if self.generator.preview_result_active {
                self.generator.preview_anchor = Some(anchor);
            }
            if self.image_generator.preview_result_active {
                self.image_generator.preview_anchor = Some(anchor);
            }
        }
        self.professional.imported_structure = None;
        self.professional.copied_chunk = None;
        self.professional.copied_chunk_preview_images.clear();
        self.professional.imported_region_package = false;
        self.generator.preview_result_active = false;
        self.image_generator.preview_result_active = false;
        self.clear_paste_preview_state(cx);
        self.invalidate_preview_3d_mesh();
    }

    pub(super) fn release_live_preview(&mut self, panel: MapViewerRightPanel) {
        if panel == MapViewerRightPanel::Generator {
            self.cancel_obj_preview_refresh();
        } else if panel == MapViewerRightPanel::ImageGenerator {
            self.cancel_image_preview_refresh();
        }
        let task_id = match panel {
            MapViewerRightPanel::Generator => self.generator.preview_task_id.take(),
            MapViewerRightPanel::ImageGenerator => self.image_generator.preview_task_id.take(),
            _ => None,
        };
        if let Some(task_id) = task_id {
            task_manager::cancel_task(&task_id);
            release_structure_preview(&task_id);
        }
    }

    pub(super) fn accept_structure_preview_task_snapshot(
        &mut self,
        snapshot: &TaskSnapshot,
        cx: &mut Context<Self>,
    ) {
        let image = self.image_generator.preview_task_id.as_deref() == Some(snapshot.id.as_ref());
        let obj = self.generator.preview_task_id.as_deref() == Some(snapshot.id.as_ref());
        if !image && !obj {
            return;
        }
        if image && self.image_generator.preview_processed
            || obj && self.generator.preview_processed
        {
            return;
        }
        if !snapshot.is_terminal() {
            return;
        }
        if image {
            self.image_generator.preview_processed = true;
        }
        if obj {
            self.generator.preview_processed = true;
        }
        if snapshot.status.as_ref() != "completed" {
            return;
        }
        let task_id = snapshot.id.to_string();
        let Some(bytes) = get_structure_preview(&task_id) else {
            self.status = SharedString::from("结构预览结果已过期，请调整设置重新生成");
            return;
        };
        let remembered_target = if image {
            self.image_generator.preview_anchor
        } else {
            self.generator.preview_anchor
        };
        let (target, should_center) =
            structure_preview_target(remembered_target, self.viewport_center_chunk_pos());
        let y = self.y_layer;
        let generation = self.metadata_generation;
        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    let structure = bedrock_world::McStructureFile::from_bytes(&bytes)
                        .map_err(|error| error.to_string())?;
                    mcstructure::structure_as_copied_chunk(Arc::new(structure), target, y)
                })
                .await;
            release_structure_preview(&task_id);
            let Some(view) = handle.upgrade() else {
                return Ok::<(), anyhow::Error>(());
            };
            view.update(cx, move |this, cx| {
                if this.metadata_generation != generation {
                    return;
                }
                let still_current = this.image_generator.preview_task_id.as_deref()
                    == Some(task_id.as_str())
                    || this.generator.preview_task_id.as_deref() == Some(task_id.as_str());
                if !still_current {
                    return;
                }
                match result {
                    Ok(import) => {
                        let size = import.size;
                        if image {
                            this.image_generator.preview_size = Some([size.x, size.y, size.z]);
                            this.image_generator.preview_result_active = true;
                            this.image_generator.preview_anchor = Some(target);
                            this.generator.preview_result_active = false;
                        } else {
                            this.generator.preview_size = Some([size.x, size.y, size.z]);
                            this.generator.preview_result_active = true;
                            this.generator.preview_anchor = Some(target);
                            this.image_generator.preview_result_active = false;
                        }
                        this.professional.copied_chunk = Some(import.copied_chunk);
                        this.professional.imported_region_package = false;
                        this.professional.imported_structure = Some(import.imported_structure);
                        this.professional.copied_chunk_preview_images = import.preview_images;
                        this.clear_paste_preview_state(cx);
                        this.invalidate_preview_3d_mesh();
                        if this.set_paste_preview(
                            target,
                            PasteTransform::default(),
                            paste_rotation_degrees(PasteRotation::NoRotation),
                            None,
                            cx,
                        ) {
                            if should_center {
                                this.center_paste_preview_in_view(cx);
                            }
                            this.refresh_import_preview_3d(cx);
                            this.status = SharedString::from(format!(
                                "实时预览 {}×{}×{} 方块；地图中的瓦片可移动、旋转并确认放置",
                                size.x, size.y, size.z
                            ));
                        }
                    }
                    Err(error) => {
                        this.status = SharedString::from(format!("结构预览加载失败：{error}"))
                    }
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }
}

fn structure_preview_target(
    remembered: Option<ChunkPos>,
    viewport_center: ChunkPos,
) -> (ChunkPos, bool) {
    remembered.map_or((viewport_center, true), |target| (target, false))
}

#[cfg(test)]
mod tests {
    use super::structure_preview_target;
    use bedrock_world::{ChunkPos, Dimension};

    #[test]
    fn structure_preview_refresh_keeps_its_anchor_without_recentering() {
        let anchor = ChunkPos {
            x: -12,
            z: 7,
            dimension: Dimension::Overworld,
        };
        let changed_viewport = ChunkPos {
            x: 80,
            z: 90,
            dimension: Dimension::Overworld,
        };
        assert_eq!(
            structure_preview_target(Some(anchor), changed_viewport),
            (anchor, false)
        );
        assert_eq!(
            structure_preview_target(None, changed_viewport),
            (changed_viewport, true)
        );
    }
}
