//! Native GPU pixel tests for the multi-pass offscreen scene renderer.
//!
//! These tests create real Nova devices, render through the public gpui-3d API, read the
//! resolved frame back, and assert visible pixels, directional shadows, resize behavior,
//! steady-state cache reuse, and cross-backend pixel agreement. They are ignored by default
//! because they require local graphics hardware; run them with:
//!
//!   cargo test -p gpui-3d --test offscreen_gpu -- --ignored --test-threads=1
//!
//! A backend that fails to create a device fails the test instead of being skipped, so the
//! results state exactly which backends were verified on the machine. The matrix test creates
//! and destroys devices in one process; backend filters are not needed.

#![allow(
    clippy::chunks_exact_to_as_chunks,
    clippy::pedantic,
    reason = "GPU pixel assertions use explicit chunked comparisons and one linear scenario per backend; keeping the checks inline keeps the failure messages readable."
)]

use gpui_3d::{
    Camera, CapturedImage, FrameStatistics, Light, Material, Mesh, Node, OffscreenRenderer,
    RenderOptions, Scene, ShadowOptions, Transform, Vec3,
};
use std::collections::HashMap;
use std::sync::Arc;

fn test_scene() -> Scene {
    let mut scene = Scene::new();
    let mut floor = Material::new();
    floor.base_color = [0.55, 0.55, 0.58, 1.0];
    floor.roughness = 0.82;
    let mut block = Material::new();
    block.base_color = [0.85, 0.35, 0.2, 1.0];
    block.metallic = 0.7;
    block.roughness = 0.28;
    scene
        .insert(
            None,
            Node::new()
                .with_mesh(Arc::new(Mesh::plane(12.0, 12.0).expect("plane")))
                .with_materials([Arc::new(floor.validated().expect("floor"))])
                .with_transform(Transform {
                    translation: Vec3::new(0.0, -1.0, 0.0),
                    ..Transform::default()
                }),
        )
        .expect("floor node");
    scene
        .insert(
            None,
            Node::new()
                .with_mesh(Arc::new(Mesh::cube()))
                .with_materials([Arc::new(block.validated().expect("block"))])
                .with_transform(Transform {
                    translation: Vec3::new(0.0, -0.15, 0.0),
                    scale: Vec3::new(0.9, 0.9, 0.9),
                    ..Transform::default()
                }),
        )
        .expect("block node");
    scene
        .insert(
            None,
            Node::new().with_light(Light::Directional {
                direction: Vec3::new(-0.45, -0.8, -0.35),
                color: [1.0, 0.94, 0.85],
                intensity: 4.0,
            }),
        )
        .expect("sun");
    scene
        .insert(
            None,
            Node::new().with_light(Light::Ambient {
                color: [0.4, 0.5, 0.7],
                intensity: 0.3,
            }),
        )
        .expect("ambient");
    scene
}

fn test_camera() -> Camera {
    Camera::perspective(
        Vec3::new(3.6, 2.4, 4.6),
        Vec3::new(0.0, -0.1, 0.0),
        Vec3::Y,
        45.0_f32.to_radians(),
        0.1,
        100.0,
    )
}

fn shadowed_options(shadows: bool) -> RenderOptions {
    RenderOptions {
        shadows: ShadowOptions {
            enabled: shadows,
            resolution: 1024,
            ..ShadowOptions::default()
        },
        ..RenderOptions::default()
    }
}

fn differing_pixels(left: &CapturedImage, right: &CapturedImage) -> usize {
    left.rgba()
        .chunks_exact(4)
        .zip(right.rgba().chunks_exact(4))
        .filter(|(a, b)| a.iter().zip(b.iter()).any(|(a, b)| a.abs_diff(*b) > 3))
        .count()
}

fn pixel(image: &CapturedImage, x: u32, y: u32) -> [u8; 4] {
    let offset = (y as usize * image.width() as usize + x as usize) * 4;
    image.rgba()[offset..offset + 4]
        .try_into()
        .expect("pixel slice")
}

fn verify_backend<D>(device: &mut D) -> Result<CapturedImage, String>
where
    D: gfx_core::Device + gfx_core::TextureTransferDevice + ?Sized,
{
    let scene = test_scene();
    let textures: HashMap<gpui_3d::TextureAssetId, Arc<gpui_3d::TextureAsset>> = HashMap::new();
    let mut renderer = OffscreenRenderer::new(device, 160, 120)
        .map_err(|error| format!("create renderer: {error}"))?;
    let lit = renderer
        .render_to_image(
            device,
            &scene,
            &textures,
            test_camera(),
            &shadowed_options(true),
        )
        .map_err(|error| format!("render lit frame: {error}"))?;
    assert_eq!((lit.width(), lit.height()), (160, 120));
    assert_eq!(lit.rgba().len(), 160 * 120 * 4);
    let lit_colors = lit
        .rgba()
        .chunks_exact(4)
        .map(|pixel| u32::from_le_bytes([pixel[0], pixel[1], pixel[2], 0]))
        .collect::<std::collections::HashSet<_>>();
    assert!(
        lit_colors.len() > 16,
        "expected a shaded frame, got {} distinct colors",
        lit_colors.len()
    );
    let brightest = lit
        .rgba()
        .chunks_exact(4)
        .map(|pixel| pixel[0].max(pixel[1]).max(pixel[2]))
        .max()
        .unwrap_or(0);
    assert!(
        brightest > 90,
        "expected a lit surface, brightest channel {brightest}"
    );
    let first = pixel(&lit, 80, 60);
    assert_ne!(first, [0, 0, 0, 0], "center pixel must be written");

    let unshadowed = renderer
        .render_to_image(
            device,
            &scene,
            &textures,
            test_camera(),
            &shadowed_options(false),
        )
        .map_err(|error| format!("render unshadowed frame: {error}"))?;
    let shadow_differences = differing_pixels(&lit, &unshadowed);
    assert!(
        shadow_differences > 100,
        "directional shadows changed only {shadow_differences} pixels"
    );

    // Render target resize must recreate size-dependent passes and keep rendering.
    renderer
        .resize(device, 192, 128)
        .map_err(|error| format!("resize: {error}"))?;
    let resized = renderer
        .render_to_image(
            device,
            &scene,
            &textures,
            test_camera(),
            &shadowed_options(true),
        )
        .map_err(|error| format!("render resized frame: {error}"))?;
    assert_eq!((resized.width(), resized.height()), (192, 128));

    // The second identical frame must not re-upload meshes, textures, or buffers.
    let first_frame: FrameStatistics = renderer
        .render(
            device,
            &scene,
            &textures,
            test_camera(),
            &shadowed_options(true),
        )
        .map_err(|error| format!("render steady frame: {error}"))?;
    let steady: FrameStatistics = renderer
        .render(
            device,
            &scene,
            &textures,
            test_camera(),
            &shadowed_options(true),
        )
        .map_err(|error| format!("render steady frame: {error}"))?;
    assert_eq!(
        first_frame.mesh_uploads, 0,
        "meshes re-uploaded after warm-up"
    );
    assert_eq!(
        first_frame.texture_uploads, 0,
        "textures re-uploaded after warm-up"
    );
    assert_eq!(first_frame.upload_bytes, 0, "steady frame uploaded data");
    assert_eq!(steady.upload_bytes, 0, "steady frame uploaded data");

    let statistics = renderer.statistics();
    assert_eq!(statistics.live_meshes, 2);
    assert!(statistics.resident_texture_bytes > 0);
    assert_eq!(statistics.target_width, 192);

    // Trimming caches must not break subsequent frames, and aggressive trim forces re-upload.
    renderer
        .trim(device, gfx_core::MemoryTrimLevel::Aggressive)
        .map_err(|error| format!("trim: {error}"))?;
    let after_trim = renderer
        .render(
            device,
            &scene,
            &textures,
            test_camera(),
            &shadowed_options(true),
        )
        .map_err(|error| format!("render after trim: {error}"))?;
    assert!(
        after_trim.mesh_uploads >= 2,
        "aggressive trim should drop mesh caches"
    );

    renderer
        .destroy(device)
        .map_err(|error| format!("destroy: {error}"))?;
    Ok(lit)
}

fn descriptor() -> gfx_core::DeviceDescriptor {
    gfx_core::DeviceDescriptor {
        application_name: "gpui-3d offscreen GPU test".to_string(),
        power_preference: gfx_core::PowerPreference::HighPerformance,
        ..gfx_core::DeviceDescriptor::default()
    }
}

/// Pixels may differ by a small amount between backends because of shading precision and
/// texture filtering rounding; a mismatch this large would mean a structural rendering bug.
fn assert_backends_agree(reference: &CapturedImage, candidate: &CapturedImage, label: &str) {
    assert_eq!(reference.rgba().len(), candidate.rgba().len());
    let differences = differing_pixels(reference, candidate);
    let total = reference.rgba().len() / 4;
    assert!(
        differences * 100 < total,
        "{label} differs from the reference backend in {differences} of {total} pixels"
    );
}

#[cfg(windows)]
#[test]
#[ignore = "requires native DX12, DX11, and Vulkan hardware"]
fn native_backend_matrix_renders_shadows_and_reuses_caches() {
    let reference = {
        let mut device = gfx_dx12::Dx12Device::new(&descriptor()).expect("hardware DX12 device");
        let image = verify_backend(&mut device).expect("DX12 offscreen frame");
        println!("dx12: pixels verified");
        image
    };
    {
        let mut device = gfx_dx11::Dx11Device::new(&descriptor()).expect("hardware DX11 device");
        let image = verify_backend(&mut device).expect("DX11 offscreen frame");
        assert_backends_agree(&reference, &image, "DX11");
        println!("dx11: pixels verified and agree with DX12");
    }
    {
        let mut device = gfx_vulkan::VulkanDevice::new(&descriptor()).expect("Vulkan device");
        let image = verify_backend(&mut device).expect("Vulkan offscreen frame");
        assert_backends_agree(&reference, &image, "Vulkan");
        println!("vulkan: pixels verified and agree with DX12");
    }
}
