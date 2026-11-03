//! Independent 3D world editor workspace.
//!
//! The editor is a separate native window with its own camera, focus, input capture,
//! streaming worker, textured meshes and GPU scene snapshot. It shares only read-only
//! world algorithms and the single map document edit/history service with the 2D viewer.

pub(super) mod camera;
pub(super) mod camera_overlay;
mod chunk_range;
pub(super) mod chunk_service;
pub(super) mod editor;
mod frame_display;
mod import;
pub(super) mod link;
#[cfg(test)]
mod offscreen_check;
pub(super) mod registry;
mod rendering;
pub(super) mod types;

pub(super) use link::World3dMapLink;
pub(super) use types::World3dSelection;
