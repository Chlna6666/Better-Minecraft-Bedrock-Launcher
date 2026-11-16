use super::*;
use crate::ui::components::button::{ghost_button, primary_button, secondary_button};

impl View {
    /// Primary actions and target feedback stay beside the world canvas.
    pub(in super::super) fn render_map_import_actions(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let busy = self.map_image_operation_busy() || self.map_write_busy();
        let converting = self.map_image_busy();
        let ready = self.map_image.preview.is_some()
            && self.map_image.task_id.as_deref().is_some_and(|id| {
                self.task_snapshots
                    .get(id)
                    .is_some_and(|task| task.status.as_ref() == "completed")
            });
        let selected = self.map_image.install_destination;
        let pending = self.map_image.pending_bundle.as_ref();
        let frame_preview = selected == MapImageInstallDestination::NewUpFrame && pending.is_none();
        let needs_target_review = frame_preview
            && self.ui_state.import_workspace_mode
                == super::super::state::ImportWorkspaceMode::Preview;
        let check = if frame_preview {
            self.map_frame_preflight_snapshot()
        } else {
            None
        };
        let needs_position = selected == MapImageInstallDestination::NewUpFrame
            && !needs_target_review
            && (self.map_frame_confirmed_center().is_none() || self.map_image.picking_frame_center);
        let check_running = check.is_some_and(|task| !task.is_terminal());
        let check_passed = self.map_frame_preflight_passed();
        let action_enabled = (ready || pending.is_some())
            && !busy
            && self.map_image.frame_drag_position.is_none()
            && (!check_running || needs_position)
            && (!frame_preview || self.map_image.placement_preview.is_some());
        let (primary, target, target_ready) = match selected {
            MapImageInstallDestination::Player => (
                t!("MapViewer.imagegen.actions.write_player_inventory"),
                t!(
                    "MapViewer.imagegen.actions.target_player",
                    player =
                        super::super::players::player_id_label(&self.map_image.selected_player)
                )
                .to_string(),
                self.map_image.selected_player.storage_key().is_some(),
            ),
            MapImageInstallDestination::SelectedBlock => (
                t!("MapViewer.imagegen.actions.write_selected_container"),
                self.map_image.selected_block.map_or_else(
                    || t!("MapViewer.imagegen.actions.no_container_selected").to_string(),
                    |(chunk, block)| {
                        if chunk.dimension == self.dimension {
                            t!(
                                "MapViewer.imagegen.actions.selected_target",
                                x = block.x,
                                y = block.y,
                                z = block.z,
                            )
                            .to_string()
                        } else {
                            t!("MapViewer.imagegen.actions.target_other_dimension").to_string()
                        }
                    },
                ),
                self.map_image
                    .selected_block
                    .is_some_and(|(chunk, _)| chunk.dimension == self.dimension),
            ),
            MapImageInstallDestination::NewUpFrame => {
                let target = if let Some((x, z)) = self.map_frame_confirmed_center() {
                    t!(
                        "MapViewer.imagegen.actions.frame_target_summary",
                        x = x,
                        y = self.y_layer,
                        z = z,
                        columns = self.map_image.options.columns,
                        rows = self.map_image.options.rows,
                    )
                    .to_string()
                } else {
                    t!("MapViewer.imagegen.actions.no_frame_center").to_string()
                };
                (
                    t!("MapViewer.imagegen.actions.write_world_frame"),
                    target,
                    true,
                )
            }
        };
        let status = if busy {
            t!("MapViewer.imagegen.actions.status_busy")
        } else if needs_position {
            if self.map_image.picking_frame_center {
                t!("MapViewer.imagegen.actions.status_picking_center")
            } else {
                t!("MapViewer.imagegen.actions.status_needs_position")
            }
        } else if pending.is_some() {
            t!("MapViewer.imagegen.actions.status_pending_nbt")
        } else if self.map_image.source.is_none() {
            t!("MapViewer.imagegen.actions.status_choose_source")
        } else if converting {
            t!("MapViewer.imagegen.actions.status_converting")
        } else if !ready {
            t!("MapViewer.imagegen.actions.status_pixels_not_ready")
        } else if !target_ready {
            t!("MapViewer.imagegen.actions.status_target_not_writable")
        } else if needs_target_review {
            t!("MapViewer.imagegen.actions.status_needs_target_review")
        } else if frame_preview && check_running {
            t!("MapViewer.imagegen.actions.status_check_running")
        } else if frame_preview && !check_passed {
            t!("MapViewer.imagegen.actions.status_check_required")
        } else if frame_preview {
            t!("MapViewer.imagegen.actions.status_check_passed")
        } else {
            t!("MapViewer.imagegen.actions.status_ready")
        };
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .when_some(
                check
                    .filter(|task| task.status.as_ref() == "error")
                    .and_then(|task| task.message.as_deref()),
                |bar, error| {
                    let reason = error.strip_prefix("validation failed: ").unwrap_or(error);
                    bar.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(5.0))
                            .w_full()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(colors.danger)
                                    .child(t!(
                                        "MapViewer.imagegen.actions.cannot_place",
                                        reason = reason
                                    )),
                            )
                            .child(
                                secondary_button(
                                    colors,
                                    "map-frame-raise-and-recheck",
                                    t!("MapViewer.imagegen.actions.raise_and_recheck"),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.step_y(1, cx);
                                        this.check_map_frame_grid(cx);
                                    }),
                                ),
                            ),
                    )
                },
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(colors.text_primary)
                            .child(target),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(colors.text_secondary)
                            .child(status),
                    ),
            )
            .when_some(pending, |bar, path| {
                bar.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(colors.text_primary)
                        .child(t!(
                            "MapViewer.imagegen.actions.pending_file",
                            path = path.display()
                        )),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        primary_button(
                            colors,
                            "map-import-primary-action",
                            if needs_position {
                                if self.map_image.picking_frame_center {
                                    t!("MapViewer.imagegen.actions.cancel_pick_mode")
                                } else {
                                    t!("MapViewer.imagegen.actions.pick_center")
                                }
                            } else if pending.is_some() {
                                t!("MapViewer.imagegen.actions.confirm_nbt_import")
                            } else if needs_target_review {
                                t!("MapViewer.imagegen.actions.next_target_preview")
                            } else if frame_preview && !check_passed {
                                if check_running {
                                    t!("MapViewer.imagegen.actions.checking")
                                } else {
                                    t!("MapViewer.imagegen.actions.check_placement")
                                }
                            } else {
                                primary
                            },
                        )
                        .opacity(if action_enabled && target_ready {
                            1.0
                        } else {
                            0.45
                        })
                        .when(action_enabled && target_ready, |button| {
                            button.on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    if needs_target_review {
                                        this.set_import_workspace_mode(
                                            super::super::state::ImportWorkspaceMode::Placement,
                                            cx,
                                        );
                                    } else if needs_position {
                                        if this.map_image.picking_frame_center {
                                            this.cancel_map_frame_center_pick(cx);
                                        } else {
                                            this.begin_map_frame_center_pick(cx);
                                        }
                                    } else if this.map_image.pending_bundle.is_some() {
                                        this.confirm_pending_map_bundle(cx);
                                    } else if this.map_image.install_destination
                                        == MapImageInstallDestination::NewUpFrame
                                        && !this.map_frame_preflight_passed()
                                    {
                                        this.check_map_frame_grid(cx);
                                    } else {
                                        this.install_map_bundle_selected(cx);
                                    }
                                }),
                            )
                        }),
                    )
                    .when(
                        frame_preview && !needs_target_review && !needs_position && !busy,
                        |bar| {
                            bar.child(
                                secondary_button(
                                    colors,
                                    "map-import-repick-center",
                                    if self.map_image.picking_frame_center {
                                        t!("MapViewer.imagegen.actions.cancel_pick")
                                    } else {
                                        t!("MapViewer.imagegen.actions.repick_center")
                                    },
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        if this.map_image.picking_frame_center {
                                            this.cancel_map_frame_center_pick(cx);
                                        } else {
                                            this.begin_map_frame_center_pick(cx);
                                        }
                                    }),
                                ),
                            )
                            .when_some(
                                self.map_frame_confirmed_center(),
                                |bar, (x, z)| {
                                    bar.child(
                                        secondary_button(
                                            colors,
                                            "map-import-focus-center",
                                            t!("MapViewer.imagegen.actions.focus_placement"),
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.center_viewport_on_block(x, z, cx);
                                                cx.notify();
                                            }),
                                        ),
                                    )
                                },
                            )
                        },
                    )
                    .when(ready && pending.is_none() && !busy, |bar| {
                        bar.child(
                            secondary_button(
                                colors,
                                "map-import-export-bundle",
                                t!("MapViewer.imagegen.actions.export_bundle"),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.export_map_bundle(cx)),
                            ),
                        )
                    })
                    .when(!busy, |bar| {
                        bar.child(
                            secondary_button(
                                colors,
                                "map-import-select-nbt",
                                if pending.is_some() {
                                    t!("MapViewer.imagegen.actions.replace_nbt")
                                } else {
                                    t!("MapViewer.imagegen.actions.import_bundle")
                                },
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.import_map_bundle_file_selected(cx)
                                }),
                            ),
                        )
                    })
                    .when(pending.is_some() && !busy, |bar| {
                        bar.child(
                            ghost_button(
                                colors,
                                "map-import-cancel-staged-nbt",
                                t!("MapViewer.imagegen.actions.cancel_staged_nbt"),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.cancel_pending_map_bundle(cx)),
                            ),
                        )
                    })
                    .when(
                        self.map_image.source.is_some()
                            && pending.is_none()
                            && !converting
                            && !busy,
                        |bar| {
                            bar.child(
                                ghost_button(
                                    colors,
                                    "map-import-regenerate-preview",
                                    t!("MapViewer.imagegen.actions.regenerate"),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.release_map_image_preview();
                                        this.start_map_preview(cx);
                                    }),
                                ),
                            )
                        },
                    ),
            )
    }
}
