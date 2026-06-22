use super::model::{
    MapViewport, Marker, OverlayOptions, PastePreview, PastePreviewImage,
    ProfessionalOverlayPaintCache, SlimeOverlayRunCache,
};
use super::paint::{draw_map_canvas, draw_professional_overlay_canvas};
use super::selection::ChunkSelection;
use super::state::MIN_CENTER_HEIGHT;
use super::tile_state::{PaintTile, RegionManager, TileLoadState};
use super::viewport::{
    region_render_range_for_viewport, ruler_blocks, screen_x_for_block, screen_y_for_block,
    tile_paint_rect, tile_paint_sort_key, viewport_screen_for_block,
    visible_tile_bounds_for_render_range,
};
use crate::ui::theme::colors::ThemeColors;
use bedrock_render::RenderLayout;
use bedrock_world::SlimeChunkWindow;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use std::hash::Hash;
use std::sync::Arc;

#[derive(Clone)]
pub(super) struct TileDebugOverlay {
    pub(super) coord: (i32, i32),
    pub(super) label: SharedString,
}

#[derive(Clone)]
pub(super) struct TilePaintSnapshot {
    pub(super) tiles: Arc<Vec<PaintTile>>,
    pub(super) debug_overlays: Arc<Vec<TileDebugOverlay>>,
    pub(super) generation: u64,
}

impl Default for TilePaintSnapshot {
    fn default() -> Self {
        Self {
            tiles: Arc::new(Vec::new()),
            debug_overlays: Arc::new(Vec::new()),
            generation: 0,
        }
    }
}

#[derive(Clone)]
pub(super) struct MapCanvasSnapshot {
    pub(super) viewport: MapViewport,
    pub(super) layout: RenderLayout,
    pub(super) colors: ThemeColors,
    pub(super) overlays: OverlayOptions,
    pub(super) tiles: Arc<TilePaintSnapshot>,
    pub(super) overlay_paint: Option<Arc<ProfessionalOverlayPaintCache>>,
    pub(super) slime_runs: Option<Arc<SlimeOverlayRunCache>>,
    pub(super) selection: Option<ChunkSelection>,
    pub(super) paste_preview: Option<PastePreview>,
    pub(super) paste_preview_images: Arc<Vec<PastePreviewImage>>,
    pub(super) highlighted_window: Option<SlimeChunkWindow>,
    pub(super) markers: Arc<Vec<Marker>>,
    pub(super) hover_label: SharedString,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum MapCanvasAction {
    BeginDrag(Point<Pixels>),
    EndDrag(Point<Pixels>),
    ZoomAt(Point<Pixels>, f32),
    BeginRightSelection(Point<Pixels>),
    EndRightSelection(Point<Pixels>),
    BeginPastePreviewMove,
    ConfirmPastePreview,
    CancelPastePreview,
    RotatePastePreviewClockwise,
    RotatePastePreviewCounterClockwise,
    MirrorPastePreviewX,
    MirrorPastePreviewZ,
    TogglePastePreviewTools,
    ExportPastePreviewImage,
    OpenPastePreview3d,
    PointerMoved {
        position: Point<Pixels>,
        pressed_button: Option<MouseButton>,
    },
}

pub(super) struct MapCanvasView {
    tile_layer: Entity<MapTileLayerView>,
    grid_layer: Entity<MapGridLayerView>,
    overlay_layer: Entity<MapOverlayLayerView>,
    marker_layer: Entity<MapMarkerLayerView>,
    hud_layer: Entity<MapHudView>,
    paste_controls_layer: Entity<MapPasteControlsView>,
    map_focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl MapCanvasView {
    pub(super) fn new(map_focus_handle: FocusHandle, cx: &mut Context<Self>) -> Self {
        let paste_controls_layer = cx.new(|_cx| MapPasteControlsView::default());
        let subscriptions = vec![cx.subscribe(
            &paste_controls_layer,
            |_this, _controls, action: &MapCanvasAction, cx| {
                cx.emit(*action);
            },
        )];
        Self {
            tile_layer: cx.new(|_cx| MapTileLayerView::default()),
            grid_layer: cx.new(|_cx| MapGridLayerView::default()),
            overlay_layer: cx.new(|_cx| MapOverlayLayerView::default()),
            marker_layer: cx.new(|_cx| MapMarkerLayerView::default()),
            hud_layer: cx.new(|_cx| MapHudView::default()),
            paste_controls_layer,
            map_focus_handle,
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn set_snapshot(&mut self, snapshot: MapCanvasSnapshot, cx: &mut Context<Self>) {
        self.tile_layer.update(cx, |view, cx| {
            view.set_snapshot(TileLayerSnapshot::from_canvas(&snapshot), cx)
        });
        self.grid_layer.update(cx, |view, cx| {
            view.set_snapshot(GridLayerSnapshot::from_canvas(&snapshot), cx)
        });
        self.overlay_layer.update(cx, |view, cx| {
            view.set_snapshot(OverlayLayerSnapshot::from_canvas(&snapshot), cx)
        });
        self.marker_layer.update(cx, |view, cx| {
            view.set_snapshot(MarkerLayerSnapshot::from_canvas(&snapshot), cx)
        });
        self.hud_layer.update(cx, |view, cx| {
            view.set_snapshot(HudSnapshot::from_canvas(&snapshot), cx)
        });
        self.paste_controls_layer.update(cx, |view, cx| {
            view.set_snapshot(PasteControlsSnapshot::from_canvas(&snapshot), cx)
        });
    }
}

impl EventEmitter<MapCanvasAction> for MapCanvasView {}

impl Render for MapCanvasView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme_colors(cx);
        div()
            .relative()
            .flex_1()
            .min_h(px(MIN_CENTER_HEIGHT))
            .overflow_hidden()
            .bg(colors.surface)
            .child(self.tile_layer.clone())
            .child(self.grid_layer.clone())
            .child(self.overlay_layer.clone())
            .child(self.marker_layer.clone())
            .child(self.hud_layer.clone())
            .child(render_interaction_layer(&self.map_focus_handle, cx))
            .child(self.paste_controls_layer.clone())
            .into_any_element()
    }
}

#[derive(Clone)]
struct TileLayerSnapshot {
    viewport: MapViewport,
    layout: RenderLayout,
    colors: ThemeColors,
    tiles: Arc<TilePaintSnapshot>,
    snapshot_id: u64,
}

impl TileLayerSnapshot {
    fn from_canvas(snapshot: &MapCanvasSnapshot) -> Self {
        let mut hasher = RenderFingerprint::new();
        hash_viewport(snapshot.viewport, &mut hasher);
        hash_layout(snapshot.layout, &mut hasher);
        snapshot.tiles.generation.hash(&mut hasher);
        Self {
            viewport: snapshot.viewport,
            layout: snapshot.layout,
            colors: snapshot.colors,
            tiles: snapshot.tiles.clone(),
            snapshot_id: hasher.value(),
        }
    }
}

#[derive(Default)]
struct MapTileLayerView {
    snapshot: Option<TileLayerSnapshot>,
}

impl MapTileLayerView {
    fn set_snapshot(&mut self, snapshot: TileLayerSnapshot, cx: &mut Context<Self>) {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|current| current.snapshot_id == snapshot.snapshot_id)
        {
            return;
        }
        self.snapshot = Some(snapshot);
        cx.notify();
    }
}

impl Render for MapTileLayerView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.snapshot
            .as_ref()
            .map(render_tile_layer)
            .unwrap_or_else(|| div().absolute().inset_0())
    }
}

#[derive(Clone)]
struct GridLayerSnapshot {
    viewport: MapViewport,
    layout: RenderLayout,
    overlays: OverlayOptions,
    colors: ThemeColors,
    snapshot_id: u64,
}

impl GridLayerSnapshot {
    fn from_canvas(snapshot: &MapCanvasSnapshot) -> Self {
        let mut hasher = RenderFingerprint::new();
        hash_viewport(snapshot.viewport, &mut hasher);
        hash_layout(snapshot.layout, &mut hasher);
        hash_overlay_options(snapshot.overlays, &mut hasher);
        Self {
            viewport: snapshot.viewport,
            layout: snapshot.layout,
            overlays: snapshot.overlays,
            colors: snapshot.colors,
            snapshot_id: hasher.value(),
        }
    }
}

#[derive(Default)]
struct MapGridLayerView {
    snapshot: Option<GridLayerSnapshot>,
}

impl MapGridLayerView {
    fn set_snapshot(&mut self, snapshot: GridLayerSnapshot, cx: &mut Context<Self>) {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|current| current.snapshot_id == snapshot.snapshot_id)
        {
            return;
        }
        self.snapshot = Some(snapshot);
        cx.notify();
    }
}

impl Render for MapGridLayerView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.snapshot
            .as_ref()
            .map(render_grid_canvas)
            .unwrap_or_else(|| div().absolute().inset_0())
    }
}

#[derive(Clone)]
struct OverlayLayerSnapshot {
    viewport: MapViewport,
    layout: RenderLayout,
    overlays: OverlayOptions,
    overlay_paint: Option<Arc<ProfessionalOverlayPaintCache>>,
    slime_runs: Option<Arc<SlimeOverlayRunCache>>,
    selection: Option<ChunkSelection>,
    paste_preview: Option<PastePreview>,
    paste_preview_images: Arc<Vec<PastePreviewImage>>,
    highlighted_window: Option<SlimeChunkWindow>,
    colors: ThemeColors,
    snapshot_id: u64,
}

impl OverlayLayerSnapshot {
    fn from_canvas(snapshot: &MapCanvasSnapshot) -> Self {
        let mut hasher = RenderFingerprint::new();
        hash_viewport(snapshot.viewport, &mut hasher);
        hash_layout(snapshot.layout, &mut hasher);
        hash_overlay_options(snapshot.overlays, &mut hasher);
        snapshot.selection.hash(&mut hasher);
        if let Some(preview) = snapshot.paste_preview.as_ref() {
            preview.hash_stable(&mut hasher);
        } else {
            0_u8.hash(&mut hasher);
        }
        for image in snapshot.paste_preview_images.iter() {
            image.target.hash(&mut hasher);
            image.image.id.hash(&mut hasher);
        }
        snapshot.highlighted_window.is_some().hash(&mut hasher);
        snapshot
            .overlay_paint
            .as_ref()
            .map(|cache| Arc::as_ptr(cache) as usize)
            .hash(&mut hasher);
        snapshot
            .slime_runs
            .as_ref()
            .map(|cache| Arc::as_ptr(cache) as usize)
            .hash(&mut hasher);
        Self {
            viewport: snapshot.viewport,
            layout: snapshot.layout,
            overlays: snapshot.overlays,
            overlay_paint: snapshot.overlay_paint.clone(),
            slime_runs: snapshot.slime_runs.clone(),
            selection: snapshot.selection,
            paste_preview: snapshot.paste_preview.clone(),
            paste_preview_images: snapshot.paste_preview_images.clone(),
            highlighted_window: snapshot.highlighted_window.clone(),
            colors: snapshot.colors,
            snapshot_id: hasher.value(),
        }
    }
}

#[derive(Default)]
struct MapOverlayLayerView {
    snapshot: Option<OverlayLayerSnapshot>,
}

impl MapOverlayLayerView {
    fn set_snapshot(&mut self, snapshot: OverlayLayerSnapshot, cx: &mut Context<Self>) {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|current| current.snapshot_id == snapshot.snapshot_id)
        {
            return;
        }
        self.snapshot = Some(snapshot);
        cx.notify();
    }
}

impl Render for MapOverlayLayerView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.snapshot
            .as_ref()
            .map(render_professional_overlay_layer)
            .unwrap_or_else(|| div().absolute().inset_0())
    }
}

#[derive(Clone)]
struct MarkerLayerSnapshot {
    viewport: MapViewport,
    layout: RenderLayout,
    colors: ThemeColors,
    markers: Arc<Vec<Marker>>,
    snapshot_id: u64,
}

impl MarkerLayerSnapshot {
    fn from_canvas(snapshot: &MapCanvasSnapshot) -> Self {
        let mut hasher = RenderFingerprint::new();
        hash_viewport(snapshot.viewport, &mut hasher);
        hash_layout(snapshot.layout, &mut hasher);
        for marker in snapshot.markers.iter() {
            marker.x.hash(&mut hasher);
            marker.z.hash(&mut hasher);
            marker.label.as_ref().hash(&mut hasher);
        }
        Self {
            viewport: snapshot.viewport,
            layout: snapshot.layout,
            colors: snapshot.colors,
            markers: snapshot.markers.clone(),
            snapshot_id: hasher.value(),
        }
    }
}

#[derive(Default)]
struct MapMarkerLayerView {
    snapshot: Option<MarkerLayerSnapshot>,
}

impl MapMarkerLayerView {
    fn set_snapshot(&mut self, snapshot: MarkerLayerSnapshot, cx: &mut Context<Self>) {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|current| current.snapshot_id == snapshot.snapshot_id)
        {
            return;
        }
        self.snapshot = Some(snapshot);
        cx.notify();
    }
}

impl Render for MapMarkerLayerView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.snapshot
            .as_ref()
            .map(render_markers)
            .unwrap_or_else(|| div().absolute().inset_0())
    }
}

#[derive(Clone)]
struct HudSnapshot {
    viewport: MapViewport,
    layout: RenderLayout,
    colors: ThemeColors,
    hover_label: SharedString,
    snapshot_id: u64,
}

impl HudSnapshot {
    fn from_canvas(snapshot: &MapCanvasSnapshot) -> Self {
        let mut hasher = RenderFingerprint::new();
        snapshot.viewport.scale.to_bits().hash(&mut hasher);
        hash_layout(snapshot.layout, &mut hasher);
        snapshot.hover_label.as_ref().hash(&mut hasher);
        Self {
            viewport: snapshot.viewport,
            layout: snapshot.layout,
            colors: snapshot.colors,
            hover_label: snapshot.hover_label.clone(),
            snapshot_id: hasher.value(),
        }
    }
}

#[derive(Default)]
struct MapHudView {
    snapshot: Option<HudSnapshot>,
}

impl MapHudView {
    fn set_snapshot(&mut self, snapshot: HudSnapshot, cx: &mut Context<Self>) {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|current| current.snapshot_id == snapshot.snapshot_id)
        {
            return;
        }
        self.snapshot = Some(snapshot);
        cx.notify();
    }
}

impl Render for MapHudView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.snapshot
            .as_ref()
            .map(render_hud_stack)
            .unwrap_or_else(|| div().absolute().inset_0())
    }
}

#[derive(Clone)]
struct PasteControlsSnapshot {
    viewport: MapViewport,
    layout: RenderLayout,
    colors: ThemeColors,
    paste_preview: Option<PastePreview>,
    snapshot_id: u64,
}

impl PasteControlsSnapshot {
    fn from_canvas(snapshot: &MapCanvasSnapshot) -> Self {
        let mut hasher = RenderFingerprint::new();
        hash_viewport(snapshot.viewport, &mut hasher);
        hash_layout(snapshot.layout, &mut hasher);
        if let Some(preview) = snapshot.paste_preview.as_ref() {
            preview.hash_stable(&mut hasher);
        } else {
            0_u8.hash(&mut hasher);
        }
        Self {
            viewport: snapshot.viewport,
            layout: snapshot.layout,
            colors: snapshot.colors,
            paste_preview: snapshot.paste_preview.clone(),
            snapshot_id: hasher.value(),
        }
    }
}

#[derive(Default)]
struct MapPasteControlsView {
    snapshot: Option<PasteControlsSnapshot>,
}

impl MapPasteControlsView {
    fn set_snapshot(&mut self, snapshot: PasteControlsSnapshot, cx: &mut Context<Self>) {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|current| current.snapshot_id == snapshot.snapshot_id)
        {
            return;
        }
        self.snapshot = Some(snapshot);
        cx.notify();
    }
}

impl EventEmitter<MapCanvasAction> for MapPasteControlsView {}

impl Render for MapPasteControlsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| render_paste_controls(snapshot, cx))
            .unwrap_or_else(div)
    }
}

fn render_interaction_layer(
    map_focus_handle: &FocusHandle,
    cx: &mut Context<MapCanvasView>,
) -> Div {
    let focus_for_scroll = map_focus_handle.clone();
    let focus_for_left_down = map_focus_handle.clone();
    let focus_for_right_down = map_focus_handle.clone();
    let focus_for_move = map_focus_handle.clone();
    div()
        .absolute()
        .inset_0()
        .key_context("MapViewer")
        .track_focus(map_focus_handle)
        .on_scroll_wheel(
            cx.listener(move |_this, event: &ScrollWheelEvent, window, cx| {
                focus_for_scroll.focus(window);
                let delta = event.delta.pixel_delta(px(48.0));
                let factor = if delta.y > px(0.0) { 1.15 } else { 0.87 };
                cx.emit(MapCanvasAction::ZoomAt(event.position, factor));
                cx.stop_propagation();
            }),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |_this, event: &MouseDownEvent, window, cx| {
                focus_for_left_down.focus(window);
                cx.emit(MapCanvasAction::BeginDrag(event.position));
                cx.stop_propagation();
            }),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |_this, event: &MouseDownEvent, window, cx| {
                focus_for_right_down.focus(window);
                cx.emit(MapCanvasAction::BeginRightSelection(event.position));
                cx.stop_propagation();
            }),
        )
        .on_mouse_up(
            MouseButton::Right,
            cx.listener(|_this, event: &MouseUpEvent, _window, cx| {
                cx.emit(MapCanvasAction::EndRightSelection(event.position));
                cx.stop_propagation();
            }),
        )
        .on_mouse_up_out(
            MouseButton::Right,
            cx.listener(|_this, event: &MouseUpEvent, _window, cx| {
                cx.emit(MapCanvasAction::EndRightSelection(event.position));
                cx.stop_propagation();
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|_this, event: &MouseUpEvent, _window, cx| {
                cx.emit(MapCanvasAction::EndDrag(event.position));
                cx.stop_propagation();
            }),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|_this, event: &MouseUpEvent, _window, cx| {
                cx.emit(MapCanvasAction::EndDrag(event.position));
                cx.stop_propagation();
            }),
        )
        .on_mouse_move(
            cx.listener(move |_this, event: &MouseMoveEvent, window, cx| {
                focus_for_move.focus(window);
                cx.emit(MapCanvasAction::PointerMoved {
                    position: event.position,
                    pressed_button: event.pressed_button,
                });
                cx.stop_propagation();
            }),
        )
}

fn render_paste_controls(
    snapshot: &PasteControlsSnapshot,
    cx: &mut Context<MapPasteControlsView>,
) -> Option<Div> {
    let preview = snapshot.paste_preview.as_ref()?;
    let rect = paste_preview_screen_rect(snapshot, preview)?;
    let controls_width = 292.0_f32.min((snapshot.viewport.width - 16.0).max(180.0));
    let controls_left = (rect.right() / px(1.0) + 10.0)
        .min(snapshot.viewport.width - controls_width - 8.0)
        .max(8.0);
    let controls_top_max = (snapshot.viewport.height - 50.0).max(12.0);
    let controls_top = (rect.top() / px(1.0)).clamp(12.0, controls_top_max);
    let tools_height = 152.0_f32.min((snapshot.viewport.height - 16.0).max(96.0));
    let tools_top_max = (snapshot.viewport.height - tools_height - 8.0).max(8.0);
    let tools_top = if controls_top + 50.0 + tools_height > snapshot.viewport.height - 8.0 {
        controls_top - tools_height - 8.0
    } else {
        controls_top + 48.0
    }
    .clamp(8.0, tools_top_max);
    let angle_label = preview.transform.label();
    let colors = snapshot.colors;

    Some(
        div()
            .absolute()
            .inset_0()
            .child(
                div()
                    .absolute()
                    .left(px(
                        (rect.left() / px(1.0) + 8.0).clamp(8.0, snapshot.viewport.width - 64.0)
                    ))
                    .top(px(
                        (rect.top() / px(1.0) + 8.0).clamp(8.0, snapshot.viewport.height - 28.0)
                    ))
                    .px(px(7.0))
                    .py(px(3.0))
                    .rounded(px(6.0))
                    .bg(Hsla {
                        a: 0.86,
                        ..colors.surface
                    })
                    .text_size(px(11.0))
                    .text_color(colors.text_primary)
                    .child(angle_label),
            )
            .child(
                div()
                    .absolute()
                    .left(px(controls_left.max(8.0)))
                    .top(px(controls_top.max(8.0)))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(8.0))
                    .py(px(7.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(Hsla {
                        a: 0.26,
                        ..colors.border
                    })
                    .bg(Hsla {
                        a: 0.90,
                        ..colors.surface
                    })
                    .child(paste_control_button(&colors, "移动").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_this, _event: &MouseDownEvent, _window, cx| {
                            cx.emit(MapCanvasAction::BeginPastePreviewMove);
                            cx.stop_propagation();
                        }),
                    ))
                    .child(paste_control_button(&colors, "↺").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_this, _event, _window, cx| {
                            cx.emit(MapCanvasAction::RotatePastePreviewCounterClockwise);
                            cx.stop_propagation();
                        }),
                    ))
                    .child(paste_control_button(&colors, "↻").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_this, _event, _window, cx| {
                            cx.emit(MapCanvasAction::RotatePastePreviewClockwise);
                            cx.stop_propagation();
                        }),
                    ))
                    .child(
                        paste_control_button(
                            &colors,
                            if preview.tools_expanded {
                                "工具▴"
                            } else {
                                "工具▾"
                            },
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_this, _event, _window, cx| {
                                cx.emit(MapCanvasAction::TogglePastePreviewTools);
                                cx.stop_propagation();
                            }),
                        ),
                    )
                    .child(paste_control_button(&colors, "确认").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_this, _event, _window, cx| {
                            cx.emit(MapCanvasAction::ConfirmPastePreview);
                            cx.stop_propagation();
                        }),
                    ))
                    .child(paste_control_button(&colors, "取消").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_this, _event, _window, cx| {
                            cx.emit(MapCanvasAction::CancelPastePreview);
                            cx.stop_propagation();
                        }),
                    )),
            )
            .when(preview.tools_expanded, |this| {
                this.child(
                    div()
                        .absolute()
                        .left(px(controls_left))
                        .top(px(tools_top))
                        .w(px(196.0))
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .px(px(8.0))
                        .py(px(8.0))
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(Hsla {
                            a: 0.26,
                            ..colors.border
                        })
                        .bg(Hsla {
                            a: 0.93,
                            ..colors.surface
                        })
                        .child(
                            div()
                                .flex()
                                .gap(px(6.0))
                                .child(paste_control_button(&colors, "镜像X").on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|_this, _event, _window, cx| {
                                        cx.emit(MapCanvasAction::MirrorPastePreviewX);
                                        cx.stop_propagation();
                                    }),
                                ))
                                .child(paste_control_button(&colors, "镜像Z").on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|_this, _event, _window, cx| {
                                        cx.emit(MapCanvasAction::MirrorPastePreviewZ);
                                        cx.stop_propagation();
                                    }),
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(6.0))
                                .child(paste_control_button(&colors, "导出").on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|_this, _event, _window, cx| {
                                        cx.emit(MapCanvasAction::ExportPastePreviewImage);
                                        cx.stop_propagation();
                                    }),
                                ))
                                .child(paste_control_button(&colors, "预览3D").on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|_this, _event, _window, cx| {
                                        cx.emit(MapCanvasAction::OpenPastePreview3d);
                                        cx.stop_propagation();
                                    }),
                                )),
                        )
                        .child(paste_control_button(&colors, "收起").on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_this, _event, _window, cx| {
                                cx.emit(MapCanvasAction::TogglePastePreviewTools);
                                cx.stop_propagation();
                            }),
                        )),
                )
            }),
    )
}

fn paste_control_button(colors: &ThemeColors, label: impl Into<SharedString>) -> Div {
    div()
        .px(px(8.0))
        .py(px(5.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(Hsla {
            a: 0.24,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.62,
            ..colors.surface_hover
        })
        .hover(|style| {
            style.bg(Hsla {
                a: 0.84,
                ..colors.surface_hover
            })
        })
        .cursor_pointer()
        .text_size(px(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.text_primary)
        .child(label.into())
}

fn paste_preview_screen_rect(
    snapshot: &PasteControlsSnapshot,
    preview: &PastePreview,
) -> Option<Bounds<Pixels>> {
    let min_x = preview.targets.iter().map(|chunk| chunk.x).min()?;
    let max_x = preview.targets.iter().map(|chunk| chunk.x).max()?;
    let min_z = preview.targets.iter().map(|chunk| chunk.z).min()?;
    let max_z = preview.targets.iter().map(|chunk| chunk.z).max()?;
    let canvas_bounds = Bounds::new(
        point(px(0.0), px(0.0)),
        size(px(snapshot.viewport.width), px(snapshot.viewport.height)),
    );
    let left = screen_x_for_block(
        canvas_bounds,
        snapshot.viewport,
        snapshot.layout,
        min_x.saturating_mul(16),
    );
    let top = screen_y_for_block(
        canvas_bounds,
        snapshot.viewport,
        snapshot.layout,
        min_z.saturating_mul(16),
    );
    let right = screen_x_for_block(
        canvas_bounds,
        snapshot.viewport,
        snapshot.layout,
        max_x.saturating_add(1).saturating_mul(16),
    );
    let bottom = screen_y_for_block(
        canvas_bounds,
        snapshot.viewport,
        snapshot.layout,
        max_z.saturating_add(1).saturating_mul(16),
    );
    if right <= left || bottom <= top {
        return None;
    }
    Some(Bounds::new(
        point(px(left), px(top)),
        size(px(right - left), px(bottom - top)),
    ))
}

fn render_tile_layer(snapshot: &TileLayerSnapshot) -> Div {
    let paint_tiles = snapshot.tiles.tiles.clone();
    let debug_overlays = snapshot.tiles.debug_overlays.clone();
    let colors = snapshot.colors;
    let viewport = snapshot.viewport;
    let layout = snapshot.layout;
    let render_range = region_render_range_for_viewport(viewport, layout);

    div()
        .absolute()
        .inset_0()
        .child(
            canvas(
                move |_bounds, _window, _cx| paint_tiles.clone(),
                move |bounds, paint_tiles, window, _cx| {
                    let Some(render_range) = render_range else {
                        return;
                    };
                    for tile in paint_tiles.iter() {
                        let Some(rect) = tile_paint_rect(
                            viewport,
                            layout,
                            render_range,
                            tile.coord.0,
                            tile.coord.1,
                        ) else {
                            continue;
                        };
                        let Some(image_bounds) = rect.to_bounds(bounds) else {
                            continue;
                        };
                        if let Err(error) = window.paint_image(
                            image_bounds,
                            Corners::all(px(0.0)),
                            tile.image.clone(),
                            0,
                            false,
                        ) {
                            tracing::debug!(?error, "failed to paint map tile image");
                        }
                    }
                },
            )
            .size_full(),
        )
        .children(debug_overlays.iter().map(|overlay| {
            let rect = render_range.and_then(|range| {
                tile_paint_rect(viewport, layout, range, overlay.coord.0, overlay.coord.1)
            });
            div()
                .absolute()
                .left(px(rect.map_or(-10_000.0, |rect| rect.left)))
                .top(px(rect.map_or(-10_000.0, |rect| rect.top)))
                .w(px(rect.map_or(1.0, |rect| rect.width().max(1.0))))
                .h(px(rect.map_or(1.0, |rect| rect.height().max(1.0))))
                .border_1()
                .border_color(Hsla {
                    a: 0.38,
                    ..colors.danger
                })
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.0))
                .text_color(colors.danger)
                .child(overlay.label.clone())
        }))
}

fn render_grid_canvas(snapshot: &GridLayerSnapshot) -> Div {
    let viewport = snapshot.viewport;
    let layout = snapshot.layout;
    let overlays = snapshot.overlays;
    let colors = snapshot.colors;
    div().absolute().inset_0().child(
        canvas(
            move |bounds, _window, _cx| bounds,
            move |bounds, _prepaint, window, _cx| {
                draw_map_canvas(bounds, viewport, layout, overlays, colors, window);
            },
        )
        .size_full(),
    )
}

fn render_professional_overlay_layer(snapshot: &OverlayLayerSnapshot) -> Div {
    let viewport = snapshot.viewport;
    let layout = snapshot.layout;
    let overlays = snapshot.overlays;
    let overlay_paint = snapshot.overlay_paint.clone();
    let slime_runs = snapshot.slime_runs.clone();
    let selection = snapshot.selection;
    let paste_preview = snapshot.paste_preview.clone();
    let paste_preview_images = snapshot.paste_preview_images.clone();
    let highlighted_window = snapshot.highlighted_window.clone();
    let colors = snapshot.colors;
    div().absolute().inset_0().child(
        canvas(
            move |bounds, _window, _cx| bounds,
            move |bounds, _prepaint, window, _cx| {
                draw_professional_overlay_canvas(
                    bounds,
                    viewport,
                    layout,
                    overlays,
                    overlay_paint.as_deref(),
                    slime_runs.as_deref(),
                    selection,
                    paste_preview.as_ref(),
                    &paste_preview_images,
                    highlighted_window.as_ref(),
                    colors,
                    window,
                );
            },
        )
        .size_full(),
    )
}

fn render_markers(snapshot: &MarkerLayerSnapshot) -> Div {
    let mut layer = div().absolute().inset_0();
    for marker in snapshot.markers.iter() {
        let Some((screen_x, screen_y)) =
            viewport_screen_for_block(snapshot.viewport, snapshot.layout, marker.x, marker.z)
        else {
            continue;
        };
        let left = px(screen_x);
        let top = px(screen_y);
        layer = layer.child(
            div()
                .absolute()
                .left(left - px(7.0))
                .top(top - px(7.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .w(px(14.0))
                        .h(px(14.0))
                        .rounded_full()
                        .border_2()
                        .border_color(rgb(0xffffff))
                        .bg(snapshot.colors.danger),
                )
                .child(
                    div()
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(6.0))
                        .bg(Hsla {
                            a: 0.78,
                            ..snapshot.colors.surface
                        })
                        .text_size(px(11.0))
                        .text_color(snapshot.colors.text_primary)
                        .child(marker.label.clone()),
                ),
        );
    }
    layer
}

fn render_hud_stack(snapshot: &HudSnapshot) -> Div {
    let ruler = ruler_blocks(snapshot.viewport.scale, snapshot.layout);
    div()
        .absolute()
        .right(px(16.0))
        .bottom(px(16.0))
        .flex()
        .flex_col()
        .items_end()
        .gap(px(8.0))
        .max_w(px(240.0))
        .child(hud_pill(&snapshot.colors, format!("{ruler} blocks")))
        .child(hud_pill(&snapshot.colors, snapshot.hover_label.clone()))
}

fn hud_pill(colors: &ThemeColors, text: impl Into<SharedString>) -> Div {
    div()
        .px(px(10.0))
        .py(px(6.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(Hsla {
            a: 0.28,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.84,
            ..colors.surface
        })
        .text_size(px(12.0))
        .text_color(colors.text_primary)
        .child(text.into())
}

fn hash_viewport(viewport: MapViewport, hasher: &mut RenderFingerprint) {
    viewport.offset_x.to_bits().hash(hasher);
    viewport.offset_y.to_bits().hash(hasher);
    viewport.scale.to_bits().hash(hasher);
    viewport.width.to_bits().hash(hasher);
    viewport.height.to_bits().hash(hasher);
}

fn hash_layout(layout: RenderLayout, hasher: &mut RenderFingerprint) {
    layout.chunks_per_tile.hash(hasher);
    layout.blocks_per_pixel.hash(hasher);
    layout.pixels_per_block.hash(hasher);
}

fn hash_overlay_options(overlays: OverlayOptions, hasher: &mut RenderFingerprint) {
    overlays.axis.hash(hasher);
    overlays.dense_grid.hash(hasher);
    overlays.ruler.hash(hasher);
    overlays.slime_chunks.hash(hasher);
    overlays.entities.hash(hasher);
    overlays.block_entities.hash(hasher);
    overlays.villages.hash(hasher);
    overlays.hardcoded_spawn_areas.hash(hasher);
}

fn theme_colors(cx: &App) -> ThemeColors {
    let theme = cx.global::<crate::ui::state::theme::ThemeState>();
    crate::ui::theme::colors::lerp_theme_colors(
        &crate::ui::theme::colors::LightColors::colors(),
        &crate::ui::theme::colors::DarkColors::colors(),
        theme.factor(std::time::Instant::now()),
        theme.accent,
    )
}

pub(super) fn build_tile_paint_snapshot(
    tile_manager: &RegionManager,
    viewport: MapViewport,
    layout: RenderLayout,
    diagnostics_open: bool,
    generation: u64,
) -> TilePaintSnapshot {
    let mut paint_tiles = Vec::new();
    let mut debug_overlays = Vec::new();
    let Some(render_range) = region_render_range_for_viewport(viewport, layout) else {
        return TilePaintSnapshot {
            tiles: Arc::new(paint_tiles),
            debug_overlays: Arc::new(debug_overlays),
            generation,
        };
    };
    let center = viewport.center_tile(layout);
    let Some(visible_bounds) = visible_tile_bounds_for_render_range(render_range, center) else {
        return TilePaintSnapshot {
            tiles: Arc::new(paint_tiles),
            debug_overlays: Arc::new(debug_overlays),
            generation,
        };
    };
    let paint_bounds = visible_bounds.expand(1);
    for tile_z in paint_bounds.min_z..=paint_bounds.max_z {
        for tile_x in paint_bounds.min_x..=paint_bounds.max_x {
            let Some(entry) = tile_manager.entries.get(&(tile_x, tile_z)) else {
                continue;
            };
            if let Some(tile) = &entry.image {
                paint_tiles.push(PaintTile {
                    coord: (tile_x, tile_z),
                    image: tile.image.clone(),
                    pixels: tile.pixels.clone(),
                    pixel_format: tile.pixel_format,
                    width: tile.width,
                    height: tile.height,
                });
            } else if diagnostics_open
                && matches!(entry.state, TileLoadState::Failed | TileLoadState::Invalid)
            {
                debug_overlays.push(TileDebugOverlay {
                    coord: (tile_x, tile_z),
                    label: if entry.state == TileLoadState::Invalid {
                        SharedString::from("空")
                    } else {
                        SharedString::from("失败")
                    },
                });
            }
        }
    }
    paint_tiles.sort_by_key(|tile| tile_paint_sort_key(tile.coord, render_range));

    TilePaintSnapshot {
        tiles: Arc::new(paint_tiles),
        debug_overlays: Arc::new(debug_overlays),
        generation,
    }
}
