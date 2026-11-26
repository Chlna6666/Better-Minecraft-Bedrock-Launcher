//! Compare the production shadow with the original four-sample Gaussian shader.
use super::comparison::{self, Draw, native_binary};
use super::*;

pub(super) fn verify<D: gfx_core::Device + TextureTransferDevice>(
    device: &mut D,
    shaders: ShaderBinaries,
) {
    let reference = gfx_shader::WgslModule::parse(concat!(
        include_str!("../../shaders/core.wgsl"),
        include_str!("../../shaders/animation.wgsl"),
        include_str!("../../shaders/shape.wgsl"),
        include_str!("reference/shadow.wgsl"),
    ))
    .expect("reference Gaussian shader");
    let mut original = shaders.clone();
    original.shadow_vertex = native_binary(
        reference
            .compile_backend(D::BACKEND_KIND, ShaderStage::Vertex, "vs_shadow")
            .expect("reference vertex"),
        D::BACKEND_KIND,
    );
    original.shadow_fragment = native_binary(
        reference
            .compile_backend(D::BACKEND_KIND, ShaderStage::Fragment, "fs_shadow")
            .expect("reference fragment"),
        D::BACKEND_KIND,
    );
    let config = SurfaceConfig::new(256, 256, Format::Bgra8Unorm).expect("shadow target");
    let resources = [original, shaders].map(|shaders| {
        let core = create_renderer_core(device, config, "shadow comparison", shaders)
            .expect("shadow core");
        let resources = create_renderer_resources(device, config, "shadow comparison", &core)
            .expect("shadow resources");
        upload_globals(device, resources.frame_resources[0].buffers, config.size);
        resources
    });
    let targets = [
        color_target(device, config.size),
        color_target(device, config.size),
    ];
    let mut maximum_error = 0;
    for dpi in [1., 1.5, 2.] {
        for sigma in [0., 0.25, 1., 4., 12.] {
            for radius in [0., 4., 24.] {
                let shadow = sample_shadow(dpi, sigma, radius);
                let pixels = std::array::from_fn::<_, 2, _>(|index| {
                    upload(device, &resources[index], shadow);
                    comparison::render(
                        device,
                        &resources[index],
                        Draw {
                            pipeline: resources[index].pipelines.alpha.shadows,
                            bind_group: resources[index].frame_resources[0]
                                .bind_groups
                                .shadow_bind_group,
                            target: targets[index].1,
                        },
                        None,
                    );
                    device
                        .read_texture(targets[index].0)
                        .expect("shadow comparison readback")
                });
                for y in 0..256 {
                    let rows = [0, 1].map(|i| {
                        let offset = y * pixels[i].bytes_per_row as usize;
                        &pixels[i].bytes[offset..offset + 256 * 4]
                    });
                    let error = rows[0]
                        .iter()
                        .zip(rows[1])
                        .map(|(a, b)| a.abs_diff(*b))
                        .max()
                        .unwrap();
                    maximum_error = maximum_error.max(error);
                    assert!(
                        error <= 1,
                        "shadow error={error} dpi={dpi} sigma={sigma} radius={radius}"
                    );
                }
            }
        }
    }
    println!("Gaussian shadow comparison: 45 cases, maximum channel error={maximum_error}/255");
    let draws = [0, 1].map(|index| Draw {
        pipeline: resources[index].pipelines.alpha.shadows,
        bind_group: resources[index].frame_resources[0]
            .bind_groups
            .shadow_bind_group,
        target: targets[index].1,
    });
    let mut full = sample_shadow(1., 12., 24.);
    full.content_mask =
        crate::ContentMask::new(bounds(point(px(0.), px(0.)), size(px(256.), px(256.))).scale(1.));
    let mut clipped = full;
    clipped.content_mask = crate::ContentMask::new(
        bounds(point(px(90.25), px(70.25)), size(px(35.5), px(35.5))).scale(1.),
    );
    let mut tiny = sample_shadow(1., 1., 2.);
    tiny.bounds.size = size(crate::ScaledPixels(8.), crate::ScaledPixels(8.));
    for (label, shadow) in [
        ("Shadow/full", full),
        ("Shadow/clipped", clipped),
        ("Shadow/tiny", tiny),
    ] {
        for resources in &resources {
            upload(device, resources, shadow);
        }
        comparison::profile(device, &resources, targets, draws, label);
    }
}

fn sample_shadow(dpi: f32, sigma: f32, radius: f32) -> Shadow {
    Shadow {
        bounds: bounds(point(px(40.), px(48.)), size(px(128.), px(64.))).scale(dpi),
        content_mask: crate::ContentMask::new(
            bounds(point(px(12.25), px(16.25)), size(px(220.5), px(220.5))).scale(1.),
        ),
        blur_radius: crate::ScaledPixels(sigma * dpi),
        corner_radii: crate::Corners {
            top_left: crate::ScaledPixels(radius * dpi),
            top_right: crate::ScaledPixels(radius * dpi * 0.5),
            bottom_right: crate::ScaledPixels(radius * dpi * 0.75),
            bottom_left: crate::ScaledPixels(radius * dpi * 0.25),
        },
        color: crate::rgba(0xcc440088).into(),
        ..Default::default()
    }
}

fn upload<D: ResourceDevice>(device: &mut D, resources: &RendererResources, shadow: Shadow) {
    let mut bytes = Vec::new();
    write_shadow(&mut bytes, &shadow);
    device
        .write_buffer(
            resources.frame_resources[0].buffers.shadow_buffer,
            0,
            &bytes,
        )
        .expect("shadow upload");
}
