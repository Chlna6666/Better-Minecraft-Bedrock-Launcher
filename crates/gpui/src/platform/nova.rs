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
mod blur_damage;
mod composite_target;
mod diagnostics;
mod draw;
mod frame_upload;
mod limits;
mod pipeline;
mod prelude;
mod renderer;
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

use crate::{DirtyRegion, ImageId};
use atlas::*;
use atlas_resources::*;
use backend::*;
use blur_damage::*;
use composite_target::*;
use diagnostics::*;
use draw::*;
use frame_upload::*;
use limits::*;
use pipeline::*;
use prelude::*;
#[cfg(test)]
use renderer::nova_present_mode_for_backend;
use renderer::{DrawableSize, MeshCacheEntry};
pub(crate) use renderer::{NovaRenderer, NovaRendererAtlas};
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

#[cfg(feature = "bench")]
pub(crate) use frame_upload::{
    upload_encoding::AtlasPixelEncodingBenchmarkCore, upload_queue::AtlasUploadBenchmarkCore,
};

#[cfg(feature = "bench")]
pub(crate) struct FrameUploadBenchmarkCore {
    scene: crate::Scene,
    upload: FrameUpload,
    rendering_parameters: RenderingParameters,
}

#[cfg(feature = "bench")]
impl FrameUploadBenchmarkCore {
    pub(crate) fn new(scene: crate::Scene) -> Self {
        Self {
            scene,
            upload: FrameUpload::default(),
            rendering_parameters: RenderingParameters::from_env(),
        }
    }

    pub(crate) fn next_frame(&mut self) -> (usize, usize, usize, usize, usize) {
        let summary = self.upload.encode(
            &self.scene,
            DrawableSize {
                width: 1_920,
                height: 1_080,
            },
            &self.rendering_parameters,
            true,
            BackdropBlurQuality::Full,
        );
        (
            usize::try_from(summary.quad_count)
                .unwrap_or(usize::MAX)
                .saturating_add(usize::try_from(summary.backdrop_blur_count).unwrap_or(usize::MAX)),
            self.upload.batches.len(),
            self.upload.uploaded_bytes(),
            self.upload.retained_byte_capacity(),
            self.upload.backdrop_blur_configs().len(),
        )
    }
}

#[cfg(test)]
mod tests;
