use super::editor::*;
use super::model::*;
use super::panels::*;
use super::prelude::*;
use crate::ui::components::icon::themed_icon;
use lucide_gpui::icons as lucide_icons;

impl MapViewerWindowView {
    pub(super) fn render_bottom_dock(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        div()
            .h(px(self.ui_state.bottom_panel_height))
            .flex_none()
            .min_h(px(BOTTOM_PANEL_MIN_HEIGHT))
            .flex()
            .flex_col()
            .bg(Hsla {
                a: CHROME_SURFACE_ALPHA,
                ..colors.surface
            })
            .child(self.render_bottom_dock_header(colors, cx))
            .child(match self.ui_state.active_bottom_tab {
                MapViewerBottomTab::ChunkTree => {
                    self.render_db_tree_panel(colors, cx).into_any_element()
                }
                MapViewerBottomTab::Details => self
                    .render_bottom_details_panel(colors, cx)
                    .into_any_element(),
                MapViewerBottomTab::Players => {
                    self.render_players_panel(colors, cx).into_any_element()
                }
                MapViewerBottomTab::Diagnostics => {
                    self.render_diagnostics_panel(colors).into_any_element()
                }
                MapViewerBottomTab::History => {
                    self.render_history_panel(colors, cx).into_any_element()
                }
            })
    }

    /// Bottom dock header: iconified tab strip + collapse button.
    fn render_bottom_dock_header(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        let active = self.ui_state.active_bottom_tab;
        let tabs: [(&'static str, &'static str, MapViewerBottomTab); 5] = [
            (
                lucide_icons::icon_layers(),
                "区块树",
                MapViewerBottomTab::ChunkTree,
            ),
            (
                lucide_icons::icon_info(),
                "详情",
                MapViewerBottomTab::Details,
            ),
            (
                lucide_icons::icon_users(),
                "玩家",
                MapViewerBottomTab::Players,
            ),
            (
                lucide_icons::icon_activity(),
                "诊断",
                MapViewerBottomTab::Diagnostics,
            ),
            (
                lucide_icons::icon_history(),
                "历史",
                MapViewerBottomTab::History,
            ),
        ];
        div()
            .h(px(40.0))
            .flex_none()
            .px(px(10.0))
            .border_b_1()
            .border_color(Hsla {
                a: CHROME_HAIRLINE_ALPHA,
                ..colors.border
            })
            .flex()
            .items_center()
            .gap(px(4.0))
            .children(tabs.into_iter().map(|(icon, label, tab)| {
                tab_button_with_icon(colors, icon, label, active == tab)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            this.set_bottom_tab(tab, cx);
                        }),
                    )
                    .into_any_element()
            }))
            .child(div().flex_1())
            .child(
                div()
                    .id("bottom-collapse")
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(10.0))
                    .py(px(5.0))
                    .rounded(px(5.0))
                    .cursor_pointer()
                    .text_size(px(12.0))
                    .text_color(colors.text_secondary)
                    .hover(|style| {
                        style.bg(Hsla {
                            a: CHROME_ELEVATED_ALPHA,
                            ..colors.surface_hover
                        })
                    })
                    .child(SharedString::from("收起"))
                    .child(themed_icon(
                        lucide_icons::icon_chevron_down(),
                        CHROME_TAB_ICON_SIZE,
                        colors.text_muted,
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.toggle_bottom_panel(cx);
                        }),
                    ),
            )
    }

    pub(super) fn refresh_chunk_tree_for_tile(&mut self, tile: (i32, i32)) {
        self.db_tree.generation = self.db_tree.generation.saturating_add(1);
        self.db_tree.selected_tile = Some(tile);
        let chunks = self
            .tile_chunk_index
            .get(&tile)
            .map_or([].as_slice(), |positions| positions.as_ref());
        self.db_tree.nodes = Arc::new(chunk_tree_nodes_for_tile(self.dimension, tile, chunks));
        self.db_tree.loading = self.metadata_loading || self.manifest_probe_in_flight;
        self.db_tree.selection = Default::default();
        self.db_tree.error = None;
    }

    pub(super) fn refresh_chunk_tree_if_selected(&mut self) {
        if let Some(tile) = self.db_tree.selected_tile {
            self.refresh_chunk_tree_for_tile(tile);
        }
    }

    pub(super) fn render_db_tree_panel(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .gap(px(10.0))
            .p(px(10.0))
            .child(
                div()
                    .w(px(380.0))
                    .flex_none()
                    .min_h(px(0.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(Hsla {
                        a: 0.24,
                        ..colors.border
                    })
                    .bg(Hsla {
                        a: 0.42,
                        ..colors.surface_hover
                    })
                    .overflow_y_scrollbar()
                    .when(self.db_tree.nodes.is_empty(), |this| {
                        this.child(
                            div()
                                .p(px(12.0))
                                .text_size(px(12.0))
                                .line_height(px(18.0))
                                .text_color(colors.text_muted)
                                .child("点击地图瓦片后显示该瓦片的区块树。默认不扫描视口。"),
                        )
                    })
                    .children(self.db_tree.nodes.iter().map(|node| {
                        self.render_db_tree_node(colors, node, cx)
                            .into_any_element()
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(Hsla {
                        a: 0.24,
                        ..colors.border
                    })
                    .bg(Hsla {
                        a: 0.38,
                        ..colors.surface_hover
                    })
                    .p(px(10.0))
                    .overflow_y_scrollbar()
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(colors.text_secondary)
                    .child(self.db_tree.selection.detail.clone().unwrap_or_else(|| {
                        SharedString::from("选择左侧 chunk 查看详情。区块树只在点击瓦片后加载。")
                    })),
            )
            .child(self.render_operation_progress_panel(colors))
    }

    pub(super) fn render_db_tree_node(
        &self,
        colors: &ThemeColors,
        node: &DbTreeNode,
        cx: &mut Context<Self>,
    ) -> Div {
        let selected = self
            .db_tree
            .selection
            .node_id
            .as_ref()
            .is_some_and(|id| id == &node.id);
        let kind = node.kind.clone();
        let id = node.id.clone();
        div()
            .px(px(8.0))
            .py(px(6.0))
            .ml(px((node.depth as f32) * 14.0))
            .rounded(px(5.0))
            .cursor(CursorStyle::PointingHand)
            .text_size(px(12.0))
            .text_color(if selected {
                colors.text_primary
            } else {
                colors.text_secondary
            })
            .bg(if selected {
                Hsla {
                    a: 0.22,
                    ..colors.accent
                }
            } else {
                Hsla {
                    a: 0.0,
                    ..colors.surface
                }
            })
            .hover(move |style| {
                if selected {
                    style
                } else {
                    style.bg(Hsla {
                        a: 0.62,
                        ..colors.surface_hover
                    })
                }
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| {
                    this.select_db_tree_node(id.clone(), kind.clone(), cx)
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(db_node_icon(&node.kind))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(node.label.clone())
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(colors.text_muted)
                                    .child(node.description.clone()),
                            ),
                    ),
            )
    }

    pub(super) fn select_db_tree_node(
        &mut self,
        id: SharedString,
        kind: DbTreeNodeKind,
        cx: &mut Context<Self>,
    ) {
        self.db_tree.selection.node_id = Some(id);
        self.db_tree.selection.detail = Some(match kind {
            DbTreeNodeKind::Dimension(dimension) => SharedString::from(format!(
                "Dimension {}\n选中瓦片: {:?}\n索引瓦片: {}\n当前节点: {}",
                dimension.id(),
                self.db_tree.selected_tile,
                self.tile_chunk_index.len(),
                self.db_tree.nodes.len().saturating_sub(1)
            )),
            DbTreeNodeKind::Chunk(chunk) => pretty_json(chunk_json(chunk)),
        });
        cx.notify();
    }

    pub(super) fn render_bottom_details_panel(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        div().flex_1().min_h(px(0.0)).p(px(10.0)).child(
            self.professional.detail.as_ref().map_or_else(
                || {
                    div()
                        .size_full()
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(Hsla {
                            a: 0.24,
                            ..colors.border
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.0))
                        .text_color(colors.text_muted)
                        .child("还没有打开记录详情。")
                },
                |detail| {
                    self.render_professional_detail_panel(colors, detail, cx)
                        .relative()
                        .top(px(0.0))
                        .right(px(0.0))
                        .w_full()
                },
            ),
        )
    }

    pub(super) fn render_diagnostics_panel(&self, colors: &ThemeColors) -> Div {
        div().flex_1().min_h(px(0.0)).p(px(10.0)).child(
            self.render_status_bar(colors)
                .relative()
                .left(px(0.0))
                .bottom(px(0.0))
                .max_w(relative(1.0))
                .w_full(),
        )
    }
}
