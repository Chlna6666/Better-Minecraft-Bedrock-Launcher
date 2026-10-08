//! Opens a native GPUI window and draws a lit 3D scene through the Nova scene-view extension.

use gpui::{
    App, Application, Bounds, Context, RendererBackend, RendererFallback, RendererOptions, Timer,
    Window, WindowBounds, WindowOptions, div, prelude::*, px, rgb, size,
};
use gpui_3d::{
    AlphaMode, Keyframe, Light, Material, Mesh, Node, OrbitCamera, Projection, ProjectionRegion,
    Scene, SceneView, ShadingModel, SpotCone, SpotLight, TextureAsset, Transform, TransformTrack,
    TriangleEdgeMask, Vec2, Vec3, Vec3Track,
};
use std::{
    error::Error,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

struct SceneViewExample {
    scene_view: Arc<SceneView>,
}

impl Render for SceneViewExample {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0x141820))
            .p(px(24.0))
            .overflow_hidden()
            .child(gpui_3d::scene_view(self.scene_view.clone()))
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let scene_view = Arc::new(build_scene_view()?);
    let backend = std::env::args()
        .skip(1)
        .find_map(|argument| argument.strip_prefix("--backend=").map(str::to_owned))
        .unwrap_or_else(|| "nova-dx12".to_owned())
        .parse::<RendererBackend>()?;
    let timeout = std::env::args()
        .skip(1)
        .find_map(|argument| argument.strip_prefix("--auto-exit-ms=").map(str::to_owned))
        .map(|milliseconds| milliseconds.parse::<u64>().map(Duration::from_millis))
        .transpose()?;
    let observed = Arc::new(AtomicBool::new(false));
    let observed_by_window = observed.clone();
    let initial_presents = gpui::performance_metrics_snapshot().direct_present_count;
    Application::with_renderer_options(RendererOptions {
        backend,
        fallback: RendererFallback::Disabled,
        ..Default::default()
    })
    .run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(960.0), px(640.0)), cx);
        if let Err(error) = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            {
                let scene_view = scene_view.clone();
                move |_, cx| cx.new(|_| SceneViewExample { scene_view })
            },
        ) {
            eprintln!("failed to open 3D scene-view example: {error:#}");
            cx.quit();
            return;
        }
        if let Some(timeout) = timeout {
            quit_after_present(cx, observed_by_window, initial_presents, timeout);
        }
        cx.activate(true);
    });
    if timeout.is_some() && !observed.load(Ordering::Acquire) {
        return Err("3D scene-view example did not observe a native present".into());
    }
    Ok(())
}

fn quit_after_present(
    cx: &mut App,
    observed: Arc<AtomicBool>,
    initial_presents: usize,
    timeout: Duration,
) {
    cx.spawn(async move |cx| {
        let started_at = Instant::now();
        while started_at.elapsed() < timeout {
            Timer::after(Duration::from_millis(50)).await;
            let metrics = gpui::performance_metrics_snapshot();
            if metrics.direct_present_count > initial_presents {
                observed.store(true, Ordering::Release);
                println!(
                    "scene_view_native_presents={}",
                    metrics.direct_present_count - initial_presents
                );
                println!("actual_backend={}", metrics.renderer_backend);
                Timer::after(Duration::from_millis(750)).await;
                break;
            }
        }
        if let Err(error) = cx.update(|cx| cx.quit()) {
            eprintln!("3D scene-view example exit failed: {error:#}");
        }
    })
    .detach();
}

fn build_scene_view() -> Result<SceneView, Box<dyn Error>> {
    let mut scene = Scene::new();
    let albedo = Arc::new(TextureAsset::rgba8_mip_chain(
        2,
        2,
        [
            Arc::<[u8]>::from([
                226, 134, 52, 255, 34, 102, 170, 255, 34, 102, 170, 255, 226, 134, 52, 255,
            ]),
            Arc::<[u8]>::from([130, 118, 111, 255]),
        ],
    )?);
    let normal = Arc::new(TextureAsset::linear_rgba8(
        2,
        2,
        Arc::<[u8]>::from([
            204, 128, 230, 255, 128, 64, 230, 255, 76, 128, 230, 255, 128, 192, 230, 255,
        ]),
    )?);
    let occlusion = Arc::new(TextureAsset::linear_rgba8(
        2,
        2,
        Arc::<[u8]>::from([
            220, 255, 255, 255, 150, 255, 255, 255, 92, 255, 255, 255, 180, 255, 255, 255,
        ]),
    )?);
    scene.insert(
        None,
        Node::new().with_light(Light::Ambient {
            color: [1.0, 0.94, 0.86],
            intensity: 0.22,
        }),
    )?;
    scene.insert(
        None,
        Node::new().with_light(Light::Directional {
            direction: Vec3::new(-0.4, -1.0, -0.3),
            color: [1.0, 0.95, 0.88],
            intensity: 2.8,
        }),
    )?;
    scene.insert(
        None,
        Node::new().with_light(Light::Point {
            position: Vec3::new(1.2, 1.8, 1.4),
            color: [0.58, 0.72, 1.0],
            intensity: 18.0,
            range: 7.0,
        }),
    )?;
    let spot = SpotLight::new(
        Vec3::new(-1.7, 3.2, 2.4),
        Vec3::new(0.35, -1.0, -0.62),
        8.0,
        SpotCone::new(0.18, 0.48)?,
    )?
    .with_color([1.0, 0.72, 0.46])?
    .with_intensity(32.0)?;
    scene.insert(None, Node::new().with_light(Light::Spot(spot)))?;

    let mut copper = Material::new();
    copper.base_color = [0.82, 0.24, 0.08, 1.0];
    copper.metallic = 0.72;
    copper.roughness = 0.3;
    copper.albedo_texture = Some(albedo.id());
    copper.normal_texture = Some(normal.id());
    scene.insert(
        None,
        Node::new()
            .with_mesh(Arc::new(Mesh::cube().generate_tangents()?.into_parts().0))
            .with_materials([Arc::new(copper)])
            .with_transform(Transform {
                translation: Vec3::new(-0.9, 0.0, 0.0),
                ..Transform::IDENTITY
            }),
    )?;

    let mut marker = Material::new();
    marker.base_color = [1.0, 0.12, 0.62, 1.0];
    marker.shading_model = ShadingModel::Unlit;
    scene.insert(
        None,
        Node::new()
            .with_mesh(Arc::new(Mesh::cube()))
            .with_materials([Arc::new(marker)])
            .with_transform(Transform {
                translation: Vec3::new(0.0, 1.0, 0.0),
                scale: Vec3::new(0.18, 0.18, 0.18),
                ..Transform::IDENTITY
            }),
    )?;

    let mut edge_panel = Material::new();
    edge_panel.base_color = [0.86, 0.56, 0.2, 0.8];
    edge_panel.alpha_mode = AlphaMode::Blend;
    edge_panel.shading_model = ShadingModel::Unlit;
    let edge_panel_mesh = Mesh::plane(0.9, 0.9)?.with_edge_masks(vec![
        TriangleEdgeMask::new([true, true, false]),
        TriangleEdgeMask::new([true, false, true]),
    ])?;
    scene.insert(
        None,
        Node::new()
            .with_mesh(Arc::new(edge_panel_mesh))
            .with_materials([Arc::new(edge_panel)])
            .with_transform(Transform {
                translation: Vec3::new(-1.4, 0.25, 0.8),
                ..Transform::IDENTITY
            })
            .with_pixel_offset(Vec2::new(0.45, -0.45))?
            .with_depth_bias(0.002)?,
    )?;

    let mut glass = Material::new();
    glass.base_color = [0.2, 0.78, 0.9, 0.38];
    glass.roughness = 0.12;
    glass.alpha_mode = AlphaMode::Blend;
    scene.insert(
        None,
        Node::new()
            .with_mesh(Arc::new(Mesh::uv_sphere(0.48, [24, 12])?))
            .with_materials([Arc::new(glass)])
            .with_transform(Transform {
                translation: Vec3::new(0.0, 0.05, 1.0),
                ..Transform::IDENTITY
            }),
    )?;

    let mut ceramic = Material::new();
    ceramic.base_color = [0.12, 0.45, 0.82, 1.0];
    ceramic.roughness = 0.18;
    ceramic.occlusion_texture = Some(occlusion.id());
    ceramic.occlusion_strength = 0.82;
    let sphere = scene.insert(
        None,
        Node::new()
            .with_mesh(Arc::new(Mesh::uv_sphere(0.7, [32, 16])?))
            .with_materials([Arc::new(ceramic)])
            .with_transform(Transform {
                translation: Vec3::new(0.9, 0.0, 0.0),
                ..Transform::IDENTITY
            }),
    )?;
    let sphere_motion = TransformTrack::new(sphere).with_translation(Vec3Track::new([
        Keyframe::new(Duration::ZERO, Vec3::new(0.9, 0.0, 0.0)),
        Keyframe::new(Duration::from_secs(2), Vec3::new(0.9, 0.6, 0.0)),
    ])?);

    let instance_mesh = Arc::new(Mesh::uv_sphere(0.14, [12, 8])?);
    let mut instance_material = Material::new();
    instance_material.base_color = [1.0, 0.62, 0.12, 1.0];
    instance_material.shading_model = ShadingModel::Unlit;
    let instance_material = Arc::new(instance_material);
    for x in [-1.2, 0.0, 1.2] {
        scene.insert(
            None,
            Node::new()
                .with_mesh(instance_mesh.clone())
                .with_materials([instance_material.clone()])
                .with_transform(Transform {
                    translation: Vec3::new(x, -0.56, 1.35),
                    ..Transform::IDENTITY
                }),
        )?;
    }

    let mut ground = Material::new();
    ground.base_color = [0.18, 0.2, 0.24, 1.0];
    ground.roughness = 0.88;
    scene.insert(
        None,
        Node::new()
            .with_mesh(Arc::new(Mesh::plane(12.0, 12.0)?))
            .with_materials([Arc::new(ground)])
            .with_transform(Transform {
                translation: Vec3::new(0.0, -0.72, 0.0),
                ..Transform::IDENTITY
            }),
    )?;

    let camera = OrbitCamera::new(
        Vec3::new(0.0, -0.05, 0.0),
        0.58,
        0.39,
        7.5,
        Projection::Perspective {
            fov_y_radians: 0.82,
            near: 0.05,
            far: 100.0,
        },
    )?
    .camera();
    let scene_view = SceneView::new(Arc::new(scene), camera)
        .with_textures([albedo, normal, occlusion])?
        .with_anisotropy(true)
        .with_animation([sphere_motion], Duration::from_secs(1))?
        .with_projection_region(ProjectionRegion::VisibleContent)
        .with_projection_inset(6.0, 0.08)?
        .with_blend_edge_feather(1.0)?;
    Ok(scene_view)
}
