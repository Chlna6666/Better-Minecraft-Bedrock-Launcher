//! Exercise production shader artifacts, packing, atlas encoding and resource layouts together.
#![expect(
    unsafe_code,
    reason = "the OpenGL hardware test owns a temporary native HWND"
)]

use super::*;
use gfx_core::TextureTransferDevice;

#[test]
#[cfg(feature = "nova-gfx-dx11")]
#[ignore = "requires Windows hardware D3D11"]
fn dx11_production_glyphs_and_images_reach_native_pixels() {
    let mut device = Dx11Device::new(&DeviceDescriptor::default()).expect("DX11 device");
    println!("GPUI DX11 sprite adapter: {}", device.adapter_name());
    verify_sprite_pixels(
        &mut device,
        cached_nova_dx11_shader_binaries().expect("DX11 artifacts"),
    );
}

#[test]
#[cfg(feature = "nova-gfx-opengl")]
#[ignore = "requires Windows hardware OpenGL 4.5"]
fn opengl_production_glyphs_and_images_reach_native_pixels() {
    let window = native_window::Window::new();
    // SAFETY: the HWND lives throughout initialization; the device owns its WGL bootstrap.
    let mut device = unsafe {
        OpenGlDevice::new(
            &DeviceDescriptor::default(),
            window.display(),
            window.handle(),
        )
    }
    .expect("OpenGL 4.5 device");
    verify_sprite_pixels(
        &mut device,
        cached_nova_opengl_shader_binaries().expect("GLSL artifacts"),
    );
}

fn verify_sprite_pixels<D>(device: &mut D, shaders: ShaderBinaries)
where
    D: BackendResources + BackendPipelines + BackendPresentationCompat + TextureTransferDevice,
{
    let config = SurfaceConfig::new(16, 16, Format::Bgra8Unorm).expect("config");
    let core = create_renderer_core(device, config, "sprite hardware gate", shaders)
        .expect("production core");
    let resources = create_renderer_resources(device, config, "sprite hardware gate", &core)
        .expect("production resources");
    let (target, view) = color_target(device, config.size);
    upload_globals(device, resources.frame_resources[0].buffers);
    primitives::verify_pixels(device, &resources, target, view);
    paths::verify_pixels(device, &resources, target, view);
    filters::verify_pixels(device, &resources, target, view);
    for kind in [
        AtlasTextureKind::Monochrome,
        AtlasTextureKind::Subpixel,
        AtlasTextureKind::Bgra,
        AtlasTextureKind::Rgba,
    ] {
        let atlas = atlas_texture(device, &resources, kind);
        for sampling in [0, 1] {
            let (pipeline, set) = upload_sprite(device, &resources, &atlas, kind, sampling);
            let step = RenderStepDescriptor::Draw(DrawStepDescriptor {
                pipeline,
                resource_sets: resource_set_list([set]),
                vertex_count: 4,
                instance_count: 1,
                first_vertex: 0,
                first_instance: 0,
                scissor: None,
            });
            device
                .render_steps_to_texture_compat(
                    view,
                    resources.render_pass,
                    &[step],
                    LoadOp::Clear(ClearColor {
                        red: 0.0,
                        green: 0.0,
                        blue: 0.0,
                        alpha: 1.0,
                    }),
                    None,
                )
                .expect("production sprite draw");
            let pixels = device.read_texture(target).expect("GPU sprite readback");
            let expected = if matches!(
                kind,
                AtlasTextureKind::Monochrome | AtlasTextureKind::Subpixel
            ) {
                [255, 255, 255, 255]
            } else {
                [0, 0, 255, 255]
            };
            // Stay away from tile edges: this tests atlas coordinates, not filter edge coverage.
            for y in 4..12 {
                for x in 4..12 {
                    let offset = y * pixels.bytes_per_row as usize + x * 4;
                    assert_eq!(
                        &pixels.bytes[offset..offset + 4],
                        &expected,
                        "{kind:?} sampling={sampling} at ({x},{y})"
                    );
                }
            }
            println!("production {kind:?} sampling={sampling}: native pixels verified");
        }
    }
}

fn color_target<D: BackendResources>(device: &mut D, size: Extent2d) -> (TextureId, TextureViewId) {
    let texture = device
        .create_texture(&TextureDescriptor {
            label: None,
            size,
            mip_level_count: 1,
            format: Format::Bgra8Unorm,
            usage: TextureUsage::COLOR_ATTACHMENT
                | TextureUsage::COPY_SRC
                | TextureUsage::COPY_DST
                | TextureUsage::SAMPLED,
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
        })
        .expect("readback target");
    let view = device
        .create_texture_view(&TextureViewDescriptor {
            label: None,
            texture,
            format: Format::Bgra8Unorm,
            base_mip_level: 0,
            mip_level_count: 1,
        })
        .expect("target view");
    (texture, view)
}

fn upload_globals<D: BackendResources>(device: &mut D, buffers: resources::FrameResourceBuffers) {
    let mut upload = FrameUpload::default();
    upload.encode(
        &crate::Scene::default(),
        &[],
        DrawableSize {
            width: 16,
            height: 16,
        },
        &RenderingParameters::from_env(),
        false,
        BackdropBlurQuality::Full,
    );
    device
        .write_buffer(buffers.global_buffer, 0, &upload.globals)
        .expect("globals");
    device
        .write_buffer(buffers.text_raster_buffer, 0, &upload.text_raster_params)
        .expect("text parameters");
}

fn atlas_texture<D: BackendResources>(
    device: &mut D,
    resources: &RendererResources,
    kind: AtlasTextureKind,
) -> NovaGpuAtlasTexture {
    let atlas = create_atlas_texture_resources(
        device,
        "sprite hardware gate",
        AtlasTextureId { index: 1, kind },
        size(DevicePixels(16), DevicePixels(16)),
        &AtlasResourceDescriptor {
            mono_sprite_resource_set_layout: resources.mono_sprite_resource_set_layout,
            poly_sprite_resource_set_layout: resources.poly_sprite_resource_set_layout,
            frame_buffers: vec![resources.frame_resources[0].buffers],
            sampler: resources.atlas_sampler,
        },
        NovaAtlasResourceSetMode::UsedByTextureKind,
    )
    .expect("production atlas");
    let source = match kind {
        AtlasTextureKind::Monochrome => vec![255; 64],
        AtlasTextureKind::Subpixel => vec![255; 256],
        AtlasTextureKind::Bgra => [0, 0, 255, 255].repeat(64),
        AtlasTextureKind::Rgba => [255, 0, 0, 255].repeat(64),
    };
    let mut tile = vec![0; 256];
    frame_upload::upload_encoding::encode_bgra_upload(
        &mut tile,
        size(DevicePixels(8), DevicePixels(8)),
        &source,
        kind,
    )
    .expect("production pixel encoder");
    let mut pixels = vec![0; 16 * 16 * 4];
    for y in 0..8 {
        pixels[(y + 4) * 64 + 16..(y + 4) * 64 + 48].copy_from_slice(&tile[y * 32..y * 32 + 32]);
    }
    device
        .write_texture(
            TextureWriteDescriptor {
                texture: atlas.texture,
                mip_level: 0,
                origin: Origin2d { x: 0, y: 0 },
                size: Extent2d::new(16, 16).expect("atlas extent"),
                layout: TextureDataLayout {
                    offset: 0,
                    bytes_per_row: std::num::NonZeroU32::new(64).expect("row pitch"),
                    rows_per_image: std::num::NonZeroU32::new(16).expect("row count"),
                },
            },
            &pixels,
        )
        .expect("atlas upload");
    atlas
}

fn upload_sprite<D: BackendResources>(
    device: &mut D,
    resources: &RendererResources,
    atlas: &NovaGpuAtlasTexture,
    kind: AtlasTextureKind,
    sampling: u32,
) -> (RenderPipelineId, ResourceSetId) {
    let bounds = bounds(point(px(0.0), px(0.0)), size(px(16.0), px(16.0))).scale(1.0);
    let mask = crate::ContentMask {
        bounds,
        corner_bounds: bounds,
        ..Default::default()
    };
    let tile = AtlasTile {
        texture_id: AtlasTextureId { index: 1, kind },
        bounds: bounds_device_pixels(),
        ..test_atlas_tile()
    };
    let buffers = resources.frame_resources[0].buffers;
    let pipelines = resources.pipelines.alpha;
    let mut bytes = Vec::new();
    if matches!(
        kind,
        AtlasTextureKind::Monochrome | AtlasTextureKind::Subpixel
    ) {
        write_monochrome_sprite(
            &mut bytes,
            &MonochromeSprite {
                order: 0,
                pad: 0,
                animation_id: None,
                bounds,
                content_mask: mask,
                color: crate::white().into(),
                tile,
                transformation: Default::default(),
            },
        );
        device
            .write_buffer(buffers.mono_sprite_buffer, 0, &bytes)
            .expect("glyph packer upload");
        (
            if kind == AtlasTextureKind::Subpixel {
                pipelines.subpixel_sprites
            } else {
                pipelines.mono_sprites
            },
            atlas.mono_resource_sets[0],
        )
    } else {
        write_polychrome_sprite(
            &mut bytes,
            &PolychromeSprite {
                order: 0,
                sampling,
                grayscale: false,
                opacity: 1.0,
                animation_id: None,
                bounds,
                content_mask: mask,
                corner_radii: Default::default(),
                tile,
            },
        );
        device
            .write_buffer(buffers.poly_sprite_buffer, 0, &bytes)
            .expect("image packer upload");
        (pipelines.poly_sprites, atlas.poly_resource_sets[0])
    }
}

fn bounds_device_pixels() -> Bounds<DevicePixels> {
    bounds(
        point(DevicePixels(4), DevicePixels(4)),
        size(DevicePixels(8), DevicePixels(8)),
    )
}

mod filters;
mod paths;
mod primitives;

#[cfg(feature = "nova-gfx-opengl")]
mod native_window {
    use windows::{
        Win32::{
            Foundation::{HINSTANCE, HWND},
            System::LibraryLoader::GetModuleHandleW,
            UI::WindowsAndMessaging::*,
        },
        core::w,
    };
    use winit::raw_window_handle::{
        RawDisplayHandle, RawWindowHandle, Win32WindowHandle, WindowsDisplayHandle,
    };

    pub(super) struct Window(HWND);
    impl Window {
        pub(super) fn new() -> Self {
            // SAFETY: built-in STATIC class; a hidden HWND is owned and destroyed on this thread.
            let handle = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("GPUI sprite GPU test"),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    32,
                    32,
                    None,
                    None,
                    Some(HINSTANCE(GetModuleHandleW(None).expect("module").0)),
                    None,
                )
            }
            .expect("test HWND");
            Self(handle)
        }
        pub(super) fn display(&self) -> RawDisplayHandle {
            RawDisplayHandle::Windows(WindowsDisplayHandle::new())
        }
        pub(super) fn handle(&self) -> RawWindowHandle {
            let mut handle = Win32WindowHandle::new(
                std::num::NonZeroIsize::new(self.0.0 as isize).expect("non-null HWND"),
            );
            // SAFETY: the process module exists for the whole test.
            handle.hinstance = std::num::NonZeroIsize::new(
                unsafe { GetModuleHandleW(None).expect("module") }.0 as isize,
            );
            RawWindowHandle::Win32(handle)
        }
    }
    impl Drop for Window {
        fn drop(&mut self) {
            // SAFETY: this test still owns the HWND on its creating thread.
            unsafe { DestroyWindow(self.0) }.expect("destroy test HWND");
        }
    }
}
