#![cfg_attr(
    not(any(
        all(feature = "nova-gfx-dx12", target_os = "windows"),
        all(feature = "nova-gfx-metal", target_os = "macos"),
        all(
            feature = "nova-gfx-vulkan",
            any(target_os = "windows", target_os = "linux", target_os = "freebsd")
        )
    )),
    allow(
        dead_code,
        unreachable_code,
        unused_assignments,
        unused_imports,
        unused_variables
    )
)]

mod atlas;
mod atlas_resources;
mod backend;
mod diagnostics;
#[path = "nova/draw_paged.rs"]
mod draw;
#[path = "nova/draw.rs"]
mod draw_legacy;
mod frame_upload;
mod limits;
mod nova_renderer;
#[path = "nova/pipeline_material.rs"]
mod pipeline;
#[path = "nova/pipeline.rs"]
mod pipeline_legacy;
mod prelude;
mod rendering_parameters;
mod resource_bindings;
mod resource_layouts;
mod resources;
mod shader;
mod surface;
mod surface_plan;
mod swapchain;
mod targets;
mod upload_metrics;
mod upload_packing;

use atlas::*;
use atlas_resources::*;
use backend::*;
use diagnostics::*;
use draw::*;
use frame_upload::*;
use limits::*;
#[cfg(test)]
use nova_renderer::partial_present_scissor;
use nova_renderer::{DrawableSize, NovaMeshCacheEntry, nova_present_mode_for_backend};
pub(crate) use nova_renderer::{NovaRenderer, NovaRendererAtlas};
use pipeline::*;
use prelude::*;
use rendering_parameters::*;
use resource_bindings::*;
use resource_layouts::*;
use resources::*;
use shader::*;
use surface::*;
use surface_plan::*;
use swapchain::*;
use targets::*;
use upload_metrics::*;
use upload_packing::*;

#[cfg(test)]
mod tests;
