mod actions;
mod bottom_panel;
mod canvas;
mod editor;
#[cfg(debug_assertions)]
mod entity_debug_paint;
mod exact_selection_ops;
mod helpers;
mod history_panel;
mod import_preview;
mod interactions;
mod layout;
mod lifecycle;
pub(crate) mod map_history;
mod mcstructure;
mod menu_overlay;
mod menus;
mod model;
mod overlays;
mod paint;
mod panels;
mod player_item_menu;
mod player_panel;
mod player_workspace;
mod players;
mod prelude;

// The 3D preview and tile renderer are split by responsibility rather than by
// migration generation. No stable/legacy/current compatibility modules remain.
mod preview_3d;
mod preview_3d_obj;
mod preview_3d_source;
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
mod r#tests;
mod tile_cache;
mod tile_occupancy;
mod tile_plan;
mod tile_render;
mod tile_render_composite;
mod tile_render_core;
mod tile_state;
mod tool_stripe;
mod top_bar;
mod view;
mod viewport;

pub use actions::init;
pub use model::MapViewerWindowInit;
pub use view::open_map_viewer_window;
