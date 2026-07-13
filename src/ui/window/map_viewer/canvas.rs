use super::model::{
    MapViewport, Marker, OverlayOptions, PastePreview, PastePreviewImage,
    ProfessionalOverlayPaintCache, SlimeOverlayRunCache,
};
use super::paint::{draw_map_canvas, draw_professional_overlay_canvas};
use super::selection::ChunkSelection;
use super::state::MIN_CENTER_HEIGHT;
use super::tile_state::{MapRenderRange, PaintTile, RegionManager, TileLoadState};
use super::viewport::{
    TileBounds, paint_tile_bounds_for_viewport, region_render_range_for_viewport, ruler_blocks,
    screen_x_for_block, screen_y_for_block, tile_coords_for_paint_order, tile_paint_rect,
    tile_paint_sort_key, viewport_screen_for_block,
};
use crate::ui::theme::colors::ThemeColors;
use bedrock_render::RenderLayout;
use std::sync::atomic::{AtomicBool, Ordering};

static MAP_TILE_PAINT_RESOURCES_UNAVAILABLE: AtomicBool = AtomicBool::new(false);
const SCREEN_IMAGE_VIEWPORT_EPSILON: f32 = 0.01;
const MAP_TILE_INTERACTION_NEW_IMAGE_BUDGET_PER_FRAME: usize = 1;
const MAP_TILE_IDLE_NEW_IMAGE_BUDGET_PER_FRAME: usize = 8;
use bedrock_world::SlimeChunkWindow;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use std::sync::Arc;

#[derive(Clone)]
pub(super) struct TileDebugOverlay {
    pub(super) coord: (i32, i32),
    pub(super) label: SharedString,
}

#[derive(Clone)]
pub(super) struct ScreenPaintImage {
    pub(super) image: Arc<RenderImage>,
    pub(super) source_viewport: MapViewport,
    pub(super) left: f32,
    pub(super) top: f32,
    pub(super) width: f32,
    pub(super) height: f32,
    pub(super) estimated_bytes: usize,
}

#[derive(Clone)]
pub(super) struct TilePaintSnapshot {
    pub(super) tiles: Arc<Vec<PaintTile>>,
    pub(super) screen_images: Arc<Vec<ScreenPaintImage>>,
    pub(super) debug_overlays: Arc<Vec<TileDebugOverlay>>,
    pub(super) generation: u64,
    pub(super) estimated_bytes: usize,
    pub(super) paint_bounds: Option<TileBounds>,
}

impl Default for TilePaintSnapshot {
    fn default() -> Self {
        Self {
            tiles: Arc::new(Vec::new()),
            screen_images: Arc::new(Vec::new()),
            debug_overlays: Arc::new(Vec::new()),
            generation: 0,
            estimated_bytes: 0,
            paint_bounds: None,
        }
    }
}

#[derive(Clone)]
pub(super) struct MapCanvasSnapshot {
    pub(super) viewport: MapViewport,
    pub(super) layout: RenderLayout,
    pub(super) colors: ThemeColors,
    pub(super) overlays: OverlayOptions,
    pub(super) dragging: bool,
    pub(super) tiles: Arc<TilePaintSnapshot>,
    pub(super) overlay_paint: Option<Arc<ProfessionalOverlayPaintCache>>,
    pub(super) slime_runs: Option<Arc<SlimeOverlayRunCache>>,
    pub(super) selection: Option<ChunkSelection>,
    pub(super) paste_preview: Option<PastePreview>,
    pub(super) paste_preview_images: Arc<Vec<PastePreviewImage>>,
    pub(super) paste_preview_images_generation: u64,
    pub(super) highlighted_window: Option<SlimeChunkWindow>,
    pub(super) markers: Arc<Vec<Marker>>,
    pub(super) markers_generation: u64,
    pub(super) hover_label: SharedString,
}

pub(super) enum TilePaintSnapshotPatch {
    Unchanged,
    Patched(TilePaintSnapshot),
    Rebuild,
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

pub(super) fn take_map_tile_paint_resources_unavailable() -> bool {
    MAP_TILE_PAINT_RESOURCES_UNAVAILABLE.swap(false, Ordering::Relaxed)
}

fn map_tile_paint_resources_unavailable() -> bool {
    MAP_TILE_PAINT_RESOURCES_UNAVAILABLE.load(Ordering::Relaxed)
}

fn record_map_tile_paint_error(error: &anyhow::Error, context: &'static str) {
    let message = error.to_string();
    let resource_unavailable = message.contains("graphics resource is unavailable")
        || message.contains("descriptor heap")
        || message.contains("resource is unavailable");
    if resource_unavailable {
        if !MAP_TILE_PAINT_RESOURCES_UNAVAILABLE.swap(true, Ordering::Relaxed) {
            tracing::warn!(%error, context, "map_viewer paint_resource_unavailable");
        } else {
            tracing::debug!(%error, context, "map_viewer paint_resource_unavailable_repeated");
        }
    } else {
        tracing::debug!(%error, context, "failed to paint map images");
    }
}

pub(super) const fn map_tile_new_image_budget(viewport_interacting: bool) -> usize {
    if viewport_interacting {
        MAP_TILE_INTERACTION_NEW_IMAGE_BUDGET_PER_FRAME
    } else {
        MAP_TILE_IDLE_NEW_IMAGE_BUDGET_PER_FRAME
    }
}

fn paint_map_images<'a>(
    window: &mut Window,
    requests: impl IntoIterator<Item = ImagePaintRequest<'a>>,
    viewport_interacting: bool,
    context: &'static str,
) {
    match window.paint_images_budgeted(requests, map_tile_new_image_budget(viewport_interacting)) {
        Ok(progress) => {
            if progress.deferred_requests > 0 {
                window.request_animation_frame();
            }
        }
        Err(error) => record_map_tile_paint_error(&error, context),
    }
}

pub(super) struct MapCanvasView {
    tile_layer: Entity<MapTileLayerView>,
    overlay_layer: Entity<MapOverlayLayerView>,
    marker_layer: Entity<MapMarkerLayerView>,
    hud_layer: Entity<MapHudView>,
    paste_controls_layer: Entity<MapPasteControlsView>,
    map_focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct InteractionViewportLayerPolicy {
    pub(super) overlay: bool,
    pub(super) markers: bool,
    pub(super) hud: bool,
    pub(super) paste_controls: bool,
}

pub(super) const fn interaction_viewport_layer_policy(
    has_paste_preview: bool,
) -> InteractionViewportLayerPolicy {
    InteractionViewportLayerPolicy {
        overlay: true,
        markers: true,
        hud: false,
        paste_controls: has_paste_preview,
    }
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

    pub(super) fn set_tile_snapshot(
        &mut self,
        viewport: MapViewport,
        layout: RenderLayout,
        colors: ThemeColors,
        overlays: OverlayOptions,
        dragging: bool,
        tiles: Arc<TilePaintSnapshot>,
        cx: &mut Context<Self>,
    ) {
        self.tile_layer.update(cx, |view, cx| {
            view.set_snapshot(
                TileLayerSnapshot {
                    viewport,
                    layout,
                    colors,
                    overlays,
                    dragging,
                    tiles,
                },
                cx,
            )
        });
    }

    pub(super) fn sync_viewport_bound_layers(
        &mut self,
        viewport: MapViewport,
        layout: RenderLayout,
        policy: InteractionViewportLayerPolicy,
        cx: &mut Context<Self>,
    ) -> bool {
        let overlay_changed = policy.overlay
            && self
                .overlay_layer
                .update(cx, |view, cx| view.set_viewport(viewport, layout, cx));
        let markers_changed = policy.markers
            && self
                .marker_layer
                .update(cx, |view, cx| view.set_viewport(viewport, layout, cx));
        let hud_changed = policy.hud
            && self
                .hud_layer
                .update(cx, |view, cx| view.set_viewport(viewport, layout, cx));
        let paste_controls_changed = policy.paste_controls
            && self
                .paste_controls_layer
                .update(cx, |view, cx| view.set_viewport(viewport, layout, cx));

        overlay_changed || markers_changed || hud_changed || paste_controls_changed
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
            .child(cached_absolute_layer(&self.tile_layer))
            .child(cached_absolute_layer(&self.overlay_layer))
            .child(cached_absolute_layer(&self.marker_layer))
            .child(cached_absolute_layer(&self.hud_layer))
            .child(render_interaction_layer(&self.map_focus_handle, cx))
            .child(cached_absolute_layer(&self.paste_controls_layer))
            .into_any_element()
    }
}

fn cached_absolute_layer<V: Render + 'static>(layer: &Entity<V>) -> AnyView {
    let cache_key = layer.entity_id().as_u64();
    AnyView::from(layer.clone())
        .cached_absolute_by(&cache_key)
        .reuse_on_window_refresh()
        .progressive()
}

#[derive(Clone)]
struct TileLayerSnapshot {
    viewport: MapViewport,
    layout: RenderLayout,
    colors: ThemeColors,
    overlays: OverlayOptions,
    dragging: bool,
    tiles: Arc<TilePaintSnapshot>,
}

impl TileLayerSnapshot {
    fn from_canvas(snapshot: &MapCanvasSnapshot) -> Self {
        Self {
            viewport: snapshot.viewport,
            layout: snapshot.layout,
            colors: snapshot.colors,
            overlays: snapshot.overlays,
            dragging: snapshot.dragging,
            tiles: snapshot.tiles.clone(),
        }
    }

    fn same_as(&self, other: &Self) -> bool {
        self.viewport == other.viewport
            && self.layout == other.layout
            && self.colors == other.colors
            && self.overlays == other.overlays
            && self.dragging == other.dragging
            && self.tiles.generation == other.tiles.generation
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
            .is_some_and(|current| current.same_as(&snapshot))
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
struct OverlayLayerSnapshot {
    viewport: MapViewport,
    layout: RenderLayout,
    overlays: OverlayOptions,
    overlay_paint: Option<Arc<ProfessionalOverlayPaintCache>>,
    slime_runs: Option<Arc<SlimeOverlayRunCache>>,
    selection: Option<ChunkSelection>,
    paste_preview: Option<PastePreview>,
    paste_preview_images: Arc<Vec<PastePreviewImage>>,
    paste_preview_images_generation: u64,
    highlighted_window: Option<SlimeChunkWindow>,
    overlay_paint_ptr: Option<usize>,
    slime_runs_ptr: Option<usize>,
    colors: ThemeColors,
}

impl OverlayLayerSnapshot {
    fn from_canvas(snapshot: &MapCanvasSnapshot) -> Self {
        Self {
            viewport: snapshot.viewport,
            layout: snapshot.layout,
            overlays: snapshot.overlays,
            overlay_paint: snapshot.overlay_paint.clone(),
            slime_runs: snapshot.slime_runs.clone(),
            selection: snapshot.selection,
            paste_preview: snapshot.paste_preview.clone(),
            paste_preview_images: snapshot.paste_preview_images.clone(),
            paste_preview_images_generation: snapshot.paste_preview_images_generation,
            highlighted_window: snapshot.highlighted_window.clone(),
            overlay_paint_ptr: arc_option_ptr(&snapshot.overlay_paint),
            slime_runs_ptr: arc_option_ptr(&snapshot.slime_runs),
            colors: snapshot.colors,
        }
    }

    fn same_as(&self, other: &Self) -> bool {
        self.viewport == other.viewport
            && self.layout == other.layout
            && self.overlays == other.overlays
            && self.selection == other.selection
            && self.paste_preview == other.paste_preview
            && self.paste_preview_images_generation == other.paste_preview_images_generation
            && self.highlighted_window == other.highlighted_window
            && self.overlay_paint_ptr == other.overlay_paint_ptr
            && self.slime_runs_ptr == other.slime_runs_ptr
            && self.colors == other.colors
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
            .is_some_and(|current| current.same_as(&snapshot))
        {
            return;
        }
        self.snapshot = Some(snapshot);
        cx.notify();
    }

    fn set_viewport(
        &mut self,
        viewport: MapViewport,
        layout: RenderLayout,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(mut snapshot) = self.snapshot.clone() else {
            return false;
        };
        if snapshot.viewport == viewport && snapshot.layout == layout {
            return false;
        }
        snapshot.viewport = viewport;
        snapshot.layout = layout;
        self.set_snapshot(snapshot, cx);
        true
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
    markers_generation: u64,
}

impl MarkerLayerSnapshot {
    fn from_canvas(snapshot: &MapCanvasSnapshot) -> Self {
        Self {
            viewport: snapshot.viewport,
            layout: snapshot.layout,
            colors: snapshot.colors,
            markers: snapshot.markers.clone(),
            markers_generation: snapshot.markers_generation,
        }
    }

    fn same_as(&self, other: &Self) -> bool {
        self.viewport == other.viewport
            && self.layout == other.layout
            && self.colors == other.colors
            && self.markers_generation == other.markers_generation
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
            .is_some_and(|current| current.same_as(&snapshot))
        {
            return;
        }
        self.snapshot = Some(snapshot);
        cx.notify();
    }

    fn set_viewport(
        &mut self,
        viewport: MapViewport,
        layout: RenderLayout,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(mut snapshot) = self.snapshot.clone() else {
            return false;
        };
        if snapshot.viewport == viewport && snapshot.layout == layout {
            return false;
        }
        snapshot.viewport = viewport;
        snapshot.layout = layout;
        self.set_snapshot(snapshot, cx);
        true
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
}

impl HudSnapshot {
    fn from_canvas(snapshot: &MapCanvasSnapshot) -> Self {
        Self {
            viewport: snapshot.viewport,
            layout: snapshot.layout,
            colors: snapshot.colors,
            hover_label: snapshot.hover_label.clone(),
        }
    }

    fn same_as(&self, other: &Self) -> bool {
        self.viewport == other.viewport
            && self.layout == other.layout
            && self.colors == other.colors
            && self.hover_label == other.hover_label
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
            .is_some_and(|current| current.same_as(&snapshot))
        {
            return;
        }
        self.snapshot = Some(snapshot);
        cx.notify();
    }

    fn set_viewport(
        &mut self,
        viewport: MapViewport,
        layout: RenderLayout,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(mut snapshot) = self.snapshot.clone() else {
            return false;
        };
        if snapshot.viewport == viewport && snapshot.layout == layout {
            return false;
        }
        snapshot.viewport = viewport;
        snapshot.layout = layout;
        self.set_snapshot(snapshot, cx);
        true
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
}

impl PasteControlsSnapshot {
    fn from_canvas(snapshot: &MapCanvasSnapshot) -> Self {
        Self {
            viewport: snapshot.viewport,
            layout: snapshot.layout,
            colors: snapshot.colors,
            paste_preview: snapshot.paste_preview.clone(),
        }
    }

    fn same_as(&self, other: &Self) -> bool {
        self.viewport == other.viewport
            && self.layout == other.layout
            && self.colors == other.colors
            && self.paste_preview == other.paste_preview
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
            .is_some_and(|current| current.same_as(&snapshot))
        {
            return;
        }
        self.snapshot = Some(snapshot);
        cx.notify();
    }

    fn set_viewport(
        &mut self,
        viewport: MapViewport,
        layout: RenderLayout,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(mut snapshot) = self.snapshot.clone() else {
            return false;
        };
        if snapshot.viewport == viewport && snapshot.layout == layout {
            return false;
        }
        snapshot.viewport = viewport;
        snapshot.layout = layout;
        self.set_snapshot(snapshot, cx);
        true
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
            cx.listener(move |_this, event: &MouseMoveEvent, _window, cx| {
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
    let screen_images = snapshot.tiles.screen_images.clone();
    let debug_overlays = if snapshot.dragging {
        Arc::new(Vec::new())
    } else {
        snapshot.tiles.debug_overlays.clone()
    };
    let colors = snapshot.colors;
    let viewport = snapshot.viewport;
    let layout = snapshot.layout;
    let viewport_interacting = snapshot.dragging;
    let mut overlays = snapshot.overlays;
    if snapshot.dragging {
        overlays.dense_grid = false;
        overlays.ruler = false;
    }
    let render_range = region_render_range_for_viewport(viewport, layout);

    div()
        .absolute()
        .inset_0()
        .child(
            canvas(
                move |_bounds, _window, _cx| paint_tiles.clone(),
                move |bounds, paint_tiles, window, _cx| {
                    let screen_requests = screen_images
                        .iter()
                        .filter_map(|image| {
                            let image_bounds = screen_image_bounds(bounds, viewport, image)?;
                            Some(ImagePaintRequest::new(image_bounds, image.image.as_ref()))
                        })
                        .collect::<Vec<_>>();
                    let Some(render_range) = render_range else {
                        paint_map_images(
                            window,
                            screen_requests,
                            viewport_interacting,
                            "screen_images",
                        );
                        draw_map_canvas(bounds, viewport, layout, overlays, colors, window);
                        return;
                    };
                    if map_tile_paint_resources_unavailable() {
                        paint_map_images(
                            window,
                            screen_requests,
                            viewport_interacting,
                            "screen_images",
                        );
                        draw_map_canvas(bounds, viewport, layout, overlays, colors, window);
                        return;
                    }
                    let requests = paint_tiles.iter().filter_map(|tile| {
                        let Some(rect) = tile_paint_rect(
                            viewport,
                            layout,
                            render_range,
                            tile.coord.0,
                            tile.coord.1,
                        ) else {
                            return None;
                        };
                        let Some(image_bounds) = rect.to_bounds(bounds) else {
                            return None;
                        };
                        Some(ImagePaintRequest::new(image_bounds, tile.image.as_ref()))
                    });
                    paint_map_images(
                        window,
                        screen_requests.into_iter().chain(requests),
                        viewport_interacting,
                        "tile_images",
                    );
                    draw_map_canvas(bounds, viewport, layout, overlays, colors, window);
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

pub(super) fn screen_image_bounds(
    canvas_bounds: Bounds<Pixels>,
    current_viewport: MapViewport,
    image: &ScreenPaintImage,
) -> Option<Bounds<Pixels>> {
    if image.width <= 0.0 || image.height <= 0.0 {
        return None;
    }
    if !screen_image_viewports_transformable(current_viewport, image.source_viewport) {
        return None;
    }
    let scale_ratio = current_viewport.scale / image.source_viewport.scale;
    let left = canvas_bounds.left() / px(1.0)
        + current_viewport.offset_x
        + (image.left - image.source_viewport.offset_x) * scale_ratio;
    let top = canvas_bounds.top() / px(1.0)
        + current_viewport.offset_y
        + (image.top - image.source_viewport.offset_y) * scale_ratio;
    let width = image.width * scale_ratio;
    let height = image.height * scale_ratio;
    if width <= 0.0 || height <= 0.0 || !width.is_finite() || !height.is_finite() {
        return None;
    }
    Some(Bounds {
        origin: point(px(left), px(top)),
        size: size(px(width), px(height)),
    })
}

pub(super) fn screen_image_viewports_transformable(
    current_viewport: MapViewport,
    source_viewport: MapViewport,
) -> bool {
    current_viewport.scale.is_finite()
        && source_viewport.scale.is_finite()
        && current_viewport.scale >= SCREEN_IMAGE_VIEWPORT_EPSILON
        && source_viewport.scale >= SCREEN_IMAGE_VIEWPORT_EPSILON
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
        .top(px(16.0))
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

fn theme_colors(cx: &App) -> ThemeColors {
    let theme = cx.global::<crate::ui::state::theme::ThemeState>();
    crate::ui::theme::colors::lerp_theme_colors(
        &crate::ui::theme::colors::LightColors::colors(),
        &crate::ui::theme::colors::DarkColors::colors(),
        theme.factor(std::time::Instant::now()),
        theme.accent,
    )
}

fn arc_option_ptr<T>(value: &Option<Arc<T>>) -> Option<usize> {
    value.as_ref().map(|value| Arc::as_ptr(value) as usize)
}

pub(super) fn build_tile_paint_snapshot(
    tile_manager: &RegionManager,
    viewport: MapViewport,
    layout: RenderLayout,
    diagnostics_open: bool,
    paint_radius: i32,
    generation: u64,
) -> TilePaintSnapshot {
    let mut paint_tiles = Vec::new();
    let mut debug_overlays = Vec::new();
    let mut estimated_bytes = 0usize;
    let Some(render_range) = region_render_range_for_viewport(viewport, layout) else {
        return TilePaintSnapshot {
            tiles: Arc::new(paint_tiles),
            screen_images: Arc::new(Vec::new()),
            debug_overlays: Arc::new(debug_overlays),
            generation,
            estimated_bytes,
            paint_bounds: None,
        };
    };
    let Some(paint_bounds) = paint_tile_bounds_for_viewport(viewport, layout, paint_radius) else {
        return TilePaintSnapshot {
            tiles: Arc::new(paint_tiles),
            screen_images: Arc::new(Vec::new()),
            debug_overlays: Arc::new(debug_overlays),
            generation,
            estimated_bytes,
            paint_bounds: None,
        };
    };
    for (tile_x, tile_z) in tile_coords_for_paint_order(paint_bounds) {
        let Some(entry) = tile_manager.entries.get(&(tile_x, tile_z)) else {
            continue;
        };
        if let Some(tile) = &entry.image {
            estimated_bytes = estimated_bytes.saturating_add(tile.estimated_bytes);
            paint_tiles.push(PaintTile {
                coord: (tile_x, tile_z),
                image: tile.image.clone(),
                pixel_format: tile.pixel_format,
                width: tile.width,
                height: tile.height,
                estimated_bytes: tile.estimated_bytes,
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
    debug_assert!(paint_tiles.windows(2).all(|tiles| {
        tile_paint_sort_key(tiles[0].coord, render_range)
            <= tile_paint_sort_key(tiles[1].coord, render_range)
    }));
    debug_assert!(debug_overlays.windows(2).all(|overlays| {
        tile_paint_sort_key(overlays[0].coord, render_range)
            <= tile_paint_sort_key(overlays[1].coord, render_range)
    }));

    TilePaintSnapshot {
        tiles: Arc::new(paint_tiles),
        screen_images: Arc::new(Vec::new()),
        debug_overlays: Arc::new(debug_overlays),
        generation,
        estimated_bytes,
        paint_bounds: Some(paint_bounds),
    }
}

pub(super) fn patch_tile_paint_snapshot(
    current: &TilePaintSnapshot,
    tile_manager: &RegionManager,
    viewport: MapViewport,
    layout: RenderLayout,
    diagnostics_open: bool,
    paint_radius: i32,
    changed_tiles: &[(i32, i32)],
    generation: u64,
) -> TilePaintSnapshotPatch {
    if changed_tiles.is_empty() {
        return TilePaintSnapshotPatch::Unchanged;
    }
    let Some(render_range) = region_render_range_for_viewport(viewport, layout) else {
        return TilePaintSnapshotPatch::Rebuild;
    };
    let Some(paint_bounds) = paint_tile_bounds_for_viewport(viewport, layout, paint_radius) else {
        return TilePaintSnapshotPatch::Rebuild;
    };
    let has_composite_underlay = !current.screen_images.is_empty();
    let (mut tiles, mut debug_overlays, mut estimated_bytes) =
        if current.paint_bounds == Some(paint_bounds) {
            (
                current.tiles.as_ref().clone(),
                current.debug_overlays.as_ref().clone(),
                current.estimated_bytes,
            )
        } else if has_composite_underlay {
            (
                Vec::new(),
                Vec::new(),
                current
                    .screen_images
                    .iter()
                    .map(|image| image.estimated_bytes)
                    .sum(),
            )
        } else {
            return TilePaintSnapshotPatch::Rebuild;
        };
    let mut changed = false;
    for coord in changed_tiles.iter().copied() {
        if !paint_bounds_contains(paint_bounds, coord) {
            continue;
        }
        if let Some(change) = patch_paint_tile(&mut tiles, tile_manager, coord, render_range) {
            estimated_bytes = estimated_bytes
                .saturating_sub(change.old_bytes)
                .saturating_add(change.new_bytes);
            changed = true;
        }
        changed |= patch_debug_overlay(
            &mut debug_overlays,
            tile_manager,
            coord,
            diagnostics_open,
            render_range,
        );
    }

    if !changed {
        return TilePaintSnapshotPatch::Unchanged;
    }
    debug_assert!(paint_tiles_are_ordered(&tiles, render_range));
    debug_assert!(debug_overlays_are_ordered(&debug_overlays, render_range));
    TilePaintSnapshotPatch::Patched(TilePaintSnapshot {
        tiles: Arc::new(tiles),
        screen_images: current.screen_images.clone(),
        debug_overlays: Arc::new(debug_overlays),
        generation,
        estimated_bytes,
        paint_bounds: Some(paint_bounds),
    })
}

#[derive(Clone, Copy)]
struct PaintTilePatchChange {
    old_bytes: usize,
    new_bytes: usize,
}

fn patch_paint_tile(
    tiles: &mut Vec<PaintTile>,
    tile_manager: &RegionManager,
    coord: (i32, i32),
    render_range: MapRenderRange,
) -> Option<PaintTilePatchChange> {
    match paint_tile_for_coord(tile_manager, coord) {
        Some(tile) => insert_or_replace_paint_tile(tiles, tile, render_range),
        None => remove_paint_tile(tiles, coord),
    }
}

fn patch_debug_overlay(
    debug_overlays: &mut Vec<TileDebugOverlay>,
    tile_manager: &RegionManager,
    coord: (i32, i32),
    diagnostics_open: bool,
    render_range: MapRenderRange,
) -> bool {
    match debug_overlay_for_coord(tile_manager, coord, diagnostics_open) {
        Some(overlay) => insert_or_replace_debug_overlay(debug_overlays, overlay, render_range),
        None => remove_debug_overlay(debug_overlays, coord),
    }
}

fn paint_tile_for_coord(tile_manager: &RegionManager, coord: (i32, i32)) -> Option<PaintTile> {
    let entry = tile_manager.entries.get(&coord)?;
    let tile = entry.image.as_ref()?;
    Some(PaintTile {
        coord,
        image: tile.image.clone(),
        pixel_format: tile.pixel_format,
        width: tile.width,
        height: tile.height,
        estimated_bytes: tile.estimated_bytes,
    })
}

fn debug_overlay_for_coord(
    tile_manager: &RegionManager,
    coord: (i32, i32),
    diagnostics_open: bool,
) -> Option<TileDebugOverlay> {
    let entry = tile_manager.entries.get(&coord)?;
    if !diagnostics_open || !matches!(entry.state, TileLoadState::Failed | TileLoadState::Invalid) {
        return None;
    }
    Some(TileDebugOverlay {
        coord,
        label: if entry.state == TileLoadState::Invalid {
            SharedString::from("空")
        } else {
            SharedString::from("失败")
        },
    })
}

fn insert_or_replace_paint_tile(
    tiles: &mut Vec<PaintTile>,
    tile: PaintTile,
    render_range: MapRenderRange,
) -> Option<PaintTilePatchChange> {
    if let Some(index) = tiles
        .iter()
        .position(|existing| existing.coord == tile.coord)
    {
        if paint_tile_same(&tiles[index], &tile) {
            return None;
        }
        let old_bytes = tiles[index].estimated_bytes;
        let new_bytes = tile.estimated_bytes;
        tiles.remove(index);
        let key = tile_paint_sort_key(tile.coord, render_range);
        let insert_index = tiles
            .binary_search_by_key(&key, |existing| {
                tile_paint_sort_key(existing.coord, render_range)
            })
            .unwrap_or_else(|index| index);
        tiles.insert(insert_index, tile);
        return Some(PaintTilePatchChange {
            old_bytes,
            new_bytes,
        });
    }
    let key = tile_paint_sort_key(tile.coord, render_range);
    match tiles.binary_search_by_key(&key, |existing| {
        tile_paint_sort_key(existing.coord, render_range)
    }) {
        Ok(index) => {
            if paint_tile_same(&tiles[index], &tile) {
                return None;
            }
            let old_bytes = tiles[index].estimated_bytes;
            let new_bytes = tile.estimated_bytes;
            tiles[index] = tile;
            Some(PaintTilePatchChange {
                old_bytes,
                new_bytes,
            })
        }
        Err(index) => {
            let new_bytes = tile.estimated_bytes;
            tiles.insert(index, tile);
            Some(PaintTilePatchChange {
                old_bytes: 0,
                new_bytes,
            })
        }
    }
}

fn remove_paint_tile(
    tiles: &mut Vec<PaintTile>,
    coord: (i32, i32),
) -> Option<PaintTilePatchChange> {
    let Some(index) = tiles.iter().position(|tile| tile.coord == coord) else {
        return None;
    };
    let tile = tiles.remove(index);
    Some(PaintTilePatchChange {
        old_bytes: tile.estimated_bytes,
        new_bytes: 0,
    })
}

fn insert_or_replace_debug_overlay(
    debug_overlays: &mut Vec<TileDebugOverlay>,
    overlay: TileDebugOverlay,
    render_range: MapRenderRange,
) -> bool {
    if let Some(index) = debug_overlays
        .iter()
        .position(|existing| existing.coord == overlay.coord)
    {
        if debug_overlay_same(&debug_overlays[index], &overlay) {
            return false;
        }
        debug_overlays.remove(index);
        let key = tile_paint_sort_key(overlay.coord, render_range);
        let insert_index = debug_overlays
            .binary_search_by(|existing| {
                tile_paint_sort_key(existing.coord, render_range).cmp(&key)
            })
            .unwrap_or_else(|index| index);
        debug_overlays.insert(insert_index, overlay);
        return true;
    }
    let key = tile_paint_sort_key(overlay.coord, render_range);
    match debug_overlays
        .binary_search_by(|existing| tile_paint_sort_key(existing.coord, render_range).cmp(&key))
    {
        Ok(index) => {
            if debug_overlay_same(&debug_overlays[index], &overlay) {
                return false;
            }
            debug_overlays[index] = overlay;
            true
        }
        Err(index) => {
            debug_overlays.insert(index, overlay);
            true
        }
    }
}

fn remove_debug_overlay(debug_overlays: &mut Vec<TileDebugOverlay>, coord: (i32, i32)) -> bool {
    let Some(index) = debug_overlays
        .iter()
        .position(|overlay| overlay.coord == coord)
    else {
        return false;
    };
    debug_overlays.remove(index);
    true
}

fn paint_bounds_contains(bounds: super::viewport::TileBounds, coord: (i32, i32)) -> bool {
    coord.0 >= bounds.min_x
        && coord.0 <= bounds.max_x
        && coord.1 >= bounds.min_z
        && coord.1 <= bounds.max_z
}

fn paint_tile_same(left: &PaintTile, right: &PaintTile) -> bool {
    left.coord == right.coord
        && Arc::ptr_eq(&left.image, &right.image)
        && left.pixel_format == right.pixel_format
        && left.width == right.width
        && left.height == right.height
        && left.estimated_bytes == right.estimated_bytes
}

fn debug_overlay_same(left: &TileDebugOverlay, right: &TileDebugOverlay) -> bool {
    left.coord == right.coord && left.label == right.label
}

fn paint_tiles_are_ordered(tiles: &[PaintTile], render_range: MapRenderRange) -> bool {
    tiles.windows(2).all(|tiles| {
        tile_paint_sort_key(tiles[0].coord, render_range)
            <= tile_paint_sort_key(tiles[1].coord, render_range)
    })
}

fn debug_overlays_are_ordered(
    debug_overlays: &[TileDebugOverlay],
    render_range: MapRenderRange,
) -> bool {
    debug_overlays.windows(2).all(|overlays| {
        tile_paint_sort_key(overlays[0].coord, render_range)
            <= tile_paint_sort_key(overlays[1].coord, render_range)
    })
}
