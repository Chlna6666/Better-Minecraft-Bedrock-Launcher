//! Repeated camera updates must consume fresh GPU visibility, across camera updates.
use super::*;

pub(super) fn verify_indirect_updates<D: Device + TextureTransferDevice>(
    device: &mut D,
    renderer: &mut EffectsRenderer,
    context: &RendererExtensionContext,
    target: &Target,
    depth: &Target,
) {
    let mut scene = Scene::new();
    let cube = Mesh::cube();
    let mesh = Arc::new(
        Mesh::from_voxel_quads_without_raycast(
            cube.vertices()
                .chunks_exact(4)
                .map(|quad| std::array::from_fn(|index| quad[index])),
            vec![crate::MeshPart::new(0, 36, 0)],
        )
        .unwrap(),
    );
    for index in 0..300 {
        let mut material = Material::new();
        material.shading_model = crate::ShadingModel::Unlit;
        material.base_color = [0.4, 0.2, 0.1, 1.];
        material.roughness = 1.;
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(mesh.clone())
                    .with_materials([Arc::new(material)])
                    .with_transform(crate::Transform {
                        translation: Vec3::new(
                            (index % 20) as f32 - 10.,
                            0.,
                            (index / 20) as f32 - 7.,
                        ),
                        ..crate::Transform::IDENTITY
                    }),
            )
            .unwrap();
    }
    let camera = Camera::perspective(Vec3::new(0., 8., 22.), Vec3::ZERO, Vec3::Y, 1., 0.1, 100.);
    let view = SceneView::new(Arc::new(scene), camera)
        .with_voxel_shader()
        .with_voxel_shading(VoxelShading {
            illumination: VoxelIllumination::Sunlight,
            sun_intensity: 0.,
            shadow_strength: 0.,
            ..VoxelShading::default()
        })
        .unwrap()
        .with_screen_effects(Some(ScreenEffects {
            background_color: Some([0.; 3]),
            resolution_scale: 1.,
            reflections: false,
            water: false,
            clouds: false,
            volumetric_fog: false,
            ..ScreenEffects::default()
        }))
        .unwrap();
    for index in 0..6 {
        let sample = view.with_camera(Camera {
            eye: camera.eye + Vec3::new(index as f32 * 0.01, 0., 0.),
            ..camera
        });
        let pixels = draw(device, renderer, context, &sample, target, depth);
        let maximum = pixels.chunks_exact(4).map(|pixel| pixel[0]).max().unwrap();
        assert!(
            maximum > 0,
            "visible indirect scene is dark at frame {index}: max_red={maximum}"
        );
    }
    let invisible = view.with_camera(Camera {
        target: camera.eye + Vec3::Z,
        ..camera
    });
    let pixels = draw(device, renderer, context, &invisible, target, depth);
    assert!(
        pixels.chunks_exact(4).all(|pixel| pixel[..3] == [0; 3]),
        "indirect commands must consume the new all-invisible GPU arguments"
    );
    let visible = draw(device, renderer, context, &view, target, depth);
    assert!(visible.chunks_exact(4).any(|pixel| pixel[0] > 0));
    let mut submissions = Vec::new();
    for index in 0..4 {
        let sample = if index == 3 {
            invisible.clone()
        } else {
            view.with_camera(Camera {
                eye: camera.eye + Vec3::new(index as f32 * 0.03, 0., 0.),
                ..camera
            })
        };
        if let Some(submission) = queue_draw(device, renderer, context, &sample, target, depth) {
            submissions.push(submission);
        }
    }
    for submission in submissions {
        SubmissionDevice::wait_submission(device, submission).unwrap();
    }
    let pixels = TextureTransferDevice::read_texture(device, target.texture)
        .unwrap()
        .bytes;
    assert!(
        pixels.chunks_exact(4).all(|pixel| pixel[..3] == [0; 3]),
        "in-flight fallback must preserve the last queued camera and GPU visibility"
    );
}
