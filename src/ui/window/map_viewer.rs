mod actions;
mod bottom_panel;
#[path = "map_viewer/canvas.rs"]
mod canvas_legacy;
#[path = "map_viewer/canvas_stable.rs"]
mod canvas_base;
#[path = "map_viewer/canvas_frontend_stable.rs"]
mod canvas;
mod editor;
#[path = "map_viewer/helpers.rs"]
mod helpers_legacy;
#[path = "map_viewer/helpers_stable.rs"]
mod helpers;
mod history_panel;
mod import_preview;
mod interactions;
mod layout;
#[path = "map_viewer/lifecycle_stable.rs"]
mod lifecycle;
pub(crate) mod map_history;
mod mcstructure;
mod menu_overlay;
mod menus;
mod model;
mod overlays;
mod paint;
mod panels;
mod player_panel;
mod players;
mod prelude;
#[path = "map_viewer/preview_3d.rs"]
mod preview_3d_legacy;
#[path = "map_viewer/preview_3d_stable.rs"]
mod preview_3d;
mod preview_3d_obj;
mod preview_panel;
mod preview_panel_render;
mod professional_panel;
mod query_cache;
mod region_package;
mod right_panel;
mod selection;
mod state;
mod status_bar;
#[cfg(test)]
mod tests;
mod tile_cache;
mod tile_manifest_legacy;
mod tile_manifest;
#[path = "map_viewer/tile_plan.rs"]
mod tile_plan_legacy;
#[path = "map_viewer/tile_plan_stable.rs"]
mod tile_plan;
mod tile_render_legacy;
#[path = "map_viewer/tile_render_stable.rs"]
mod tile_render_stable;
#[path = "map_viewer/tile_render_current.rs"]
mod tile_render;
mod tile_state;
mod tool_stripe;
mod top_bar;
#[path = "map_viewer/view.rs"]
mod view_legacy;
#[path = "map_viewer/view_stable.rs"]
mod view;
#[path = "map_viewer/viewport.rs"]
mod viewport_base;
#[path = "map_viewer/viewport_stable.rs"]
mod viewport;

pub use actions::init;
pub use model::MapViewerWindowInit;
pub use view::open_map_viewer_window;
