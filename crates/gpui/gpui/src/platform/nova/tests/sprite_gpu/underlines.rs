use super::comparison::{self, Draw, native_binary};
use super::*;

pub(super) fn verify<D: gfx_core::Device + TextureTransferDevice>(
    device: &mut D,
    shaders: ShaderBinaries,
) {
    let module = gfx_shader::WgslModule::parse(concat!(
        include_str!("../../shaders/core.wgsl"),
        include_str!("reference/underline.wgsl")
    ))
    .expect("underline reference WGSL");
    let mut original = shaders.clone();
    original.underline_vertex = native_binary(
        module
            .compile_backend(D::BACKEND_KIND, ShaderStage::Vertex, "vs_underline")
            .expect("reference vertex"),
        D::BACKEND_KIND,
    );
    original.underline_fragment = native_binary(
        module
            .compile_backend(D::BACKEND_KIND, ShaderStage::Fragment, "fs_underline")
            .expect("reference fragment"),
        D::BACKEND_KIND,
    );
    let config =
        SurfaceConfig::new(256, 256, Format::Bgra8Unorm).expect("underline comparison extent");
    let resources = [original, shaders].map(|shaders| {
        let core = create_renderer_core(device, config, "underline comparison", shaders)
            .expect("underline core");
        let resources = create_renderer_resources(device, config, "underline comparison", &core)
            .expect("underline resources");
        upload_globals(device, resources.frame_resources[0].buffers, config.size);
        resources
    });
    let targets = [
        color_target(device, config.size),
        color_target(device, config.size),
    ];
    let draws = [0, 1].map(|index| Draw {
        pipeline: resources[index].pipelines.alpha.underlines,
        bind_group: resources[index].frame_resources[0]
            .bind_groups
            .underline_bind_group,
        target: targets[index].1,
    });
    compare(device, &resources, targets, draws);
    for (label, clip, mode, tiny) in [
        ("full", 0, 0, false),
        ("wavy", 0, 1, false),
        ("clipped", 1, 0, false),
        ("tiny", 0, 0, true),
    ] {
        for resource in &resources {
            upload(
                device,
                resource,
                Case {
                    dpi: 1.,
                    clip,
                    mode,
                    opacity: 1.,
                    tiny,
                },
            );
        }
        comparison::profile(
            device,
            &resources,
            targets,
            draws,
            &format!("Underline/{label}"),
        );
    }
}

fn compare<D: gfx_core::Device + TextureTransferDevice>(
    device: &mut D,
    resources: &[RendererResources; 2],
    targets: [(TextureId, TextureViewId); 2],
    draws: [Draw; 2],
) {
    let mut maximum_error = 0;
    for dpi in [1., 1.5, 2.] {
        for clip in 0..3 {
            for mode in 0..4 {
                let opacity = [0., 0.4, 1.][(clip + mode) % 3];
                let pixels = std::array::from_fn::<_, 2, _>(|index| {
                    upload(
                        device,
                        &resources[index],
                        Case {
                            dpi,
                            clip,
                            mode,
                            opacity,
                            tiny: false,
                        },
                    );
                    comparison::render(device, &resources[index], draws[index], None);
                    device
                        .read_texture(targets[index].0)
                        .expect("underline comparison readback")
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
                        "underline error={error} dpi={dpi} clip={clip} mode={mode}"
                    );
                }
            }
        }
    }
    println!(
        "Underline full-frame comparison: 36 cases, maximum channel error={maximum_error}/255"
    );
}

#[derive(Clone, Copy)]
struct Case {
    dpi: f32,
    clip: usize,
    mode: usize,
    opacity: f32,
    tiny: bool,
}

fn upload<D: ResourceDevice>(device: &mut D, resources: &RendererResources, case: Case) {
    let Case {
        dpi,
        clip,
        mode,
        opacity,
        tiny,
    } = case;
    let shape_size = if tiny {
        size(px(8.), px(4.))
    } else {
        size(px(180.5), px(4.5))
    };
    let area = bounds(point(px(16.25), px(24.25)), shape_size).scale(dpi);
    let mask = match clip {
        0 => bounds(point(px(0.), px(0.)), size(px(256.), px(256.))),
        1 => bounds(point(px(60.25), px(20.25)), size(px(35.5), px(100.5))),
        _ => bounds(point(px(1000.), px(1000.)), size(px(20.), px(20.))),
    }
    .scale(1.);
    let mut content_mask = crate::ContentMask::new(mask);
    content_mask.corner_radii = crate::Corners::all(crate::ScaledPixels(mode as f32 * 2.));
    let mut color: crate::Rgba = crate::rgb(0xcc4488).into();
    color.a = opacity;
    let mut bytes = Vec::new();
    write_underline(
        &mut bytes,
        &Underline {
            order: 0,
            pad: 0,
            animation_id: None,
            bounds: area,
            content_mask,
            color,
            thickness: crate::ScaledPixels(1.5 * dpi),
            wavy: (mode & 1) as u32,
        },
    );
    device
        .write_buffer(
            resources.frame_resources[0].buffers.underline_buffer,
            0,
            &bytes,
        )
        .expect("underline comparison upload");
}
