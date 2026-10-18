//! Same-page atlas writes must refresh their captured source, not unrelated layers.
use super::*;
use crate::{AtlasKey, ImageId, ImagePixelFormat, RenderImageParams};
use std::borrow::Cow;

fn key(id: usize) -> AtlasKey {
    AtlasKey::Image(RenderImageParams {
        image_id: ImageId(id),
        frame_slot: 0,
        pixel_format: ImagePixelFormat::Rgba8,
    })
}

fn tile(atlas: &NovaAtlas, key: &AtlasKey, color: [u8; 4]) -> crate::AtlasTile {
    atlas
        .refresh_tile_with(key, &mut || {
            Ok(Some((
                crate::size(DevicePixels(4), DevicePixels(4)),
                Cow::Owned(color.repeat(16)),
            )))
        })
        .expect("queue test tile")
        .expect("allocate test tile")
}

fn page(tiles: &[crate::AtlasTile], depth: usize) -> Arc<crate::window::Paint> {
    let mut painter = crate::scene::Painter::default();
    let viewport = bounds(0., 0., 64., 64.);
    painter.insert_primitive(quad(viewport, crate::rgb(0x204060)));
    let parents = [
        "parent0", "parent1", "parent2", "parent3", "parent4", "parent5",
    ];
    for parent in parents.into_iter().take(depth) {
        begin_capture(&mut painter, viewport, parent);
    }
    let children = ["first", "second", "third", "fourth"];
    for (index, tile) in tiles.iter().copied().enumerate() {
        let region = bounds(
            4. + (index % 2) as f32 * 30.,
            4. + (index / 2) as f32 * 30.,
            24.,
            24.,
        );
        begin_capture(&mut painter, region, children[index]);
        painter.insert_primitive(crate::PolychromeSprite {
            order: 0,
            sampling: 0,
            grayscale: false,
            opacity: 1.,
            animation_id: None,
            bounds: region,
            content_mask: crate::ContentMask::new(region),
            corner_radii: Default::default(),
            tile,
        });
        painter.end_blur();
    }
    for _ in 0..depth {
        painter.end_blur();
    }
    painter.finish();
    painter.revision = 1;
    Arc::new(crate::window::Paint::new(
        painter.into_scene(),
        Arc::default(),
        Default::default(),
    ))
}

fn begin_capture(
    painter: &mut crate::scene::Painter,
    region: crate::Bounds<crate::ScaledPixels>,
    id: &'static str,
) {
    painter.begin_blur(crate::BlurCapture {
        animation_id: None,
        layer_id: Some(crate::GlobalElementId::from_path(&[id.into()])),
        bounds: region,
        content_mask: crate::ContentMask::new(region),
        radius: crate::ScaledPixels(2.),
        opacity: 1.,
    });
}

fn versions(renderer: &NovaRenderer) -> Vec<(u32, u64)> {
    renderer
        .frame_upload
        .element_blur_inputs
        .iter()
        .map(|(index, _)| {
            (
                *index,
                renderer
                    .filters
                    .layer_content_version(*index)
                    .expect("resident source")
                    .0,
            )
        })
        .collect()
}

struct Fixture {
    renderer: NovaRenderer,
    scene: Arc<crate::window::Paint>,
    target: (TextureId, TextureViewId),
    keys: Vec<AtlasKey>,
    tiles: Vec<crate::AtlasTile>,
    depth: usize,
    // Renderer resources must drop before their native HWND owner.
    _window: native_window::Window,
}

impl Fixture {
    fn new(backend: RendererBackend, depth: usize, siblings: usize) -> Self {
        let window = native_window::Window::renderer_fixture();
        let mut renderer = NovaRenderer::with_atlas(
            &window,
            backend,
            &RendererOptions::default(),
            GpuSubmissionMode::Deferred,
            crate::size(DevicePixels(SIDE as i32), DevicePixels(SIDE as i32)),
            false,
            NovaRendererAtlas::new(),
        )
        .expect("native renderer");
        let keys: Vec<_> = (0..siblings).map(|index| key(700_001 + index)).collect();
        let tiles: Vec<_> = keys
            .iter()
            .map(|key| tile(&renderer.atlas, key, [255, 0, 0, 255]))
            .collect();
        assert!(
            tiles
                .iter()
                .all(|tile| tile.texture_id == tiles[0].texture_id)
        );
        let scene = page(&tiles, depth);
        let target = super::super::target(&renderer);
        pixels(&mut renderer, &scene, target, 0., [], true);
        while renderer.swapchain_warmup_frames > 0 {
            pixels(&mut renderer, &scene, target, 0., [], true);
        }
        // The first post-warmup frame switches Interactive -> Full quality.
        // Settle that independent invalidation before testing atlas-only writes.
        pixels(&mut renderer, &scene, target, 0., [], true);
        Self {
            renderer,
            scene,
            target,
            keys,
            tiles,
            depth,
            _window: window,
        }
    }

    fn update(&mut self, changed: usize) {
        let before = versions(&self.renderer);
        assert_eq!(
            tile(&self.renderer.atlas, &self.keys[changed], [0, 0, 255, 255]),
            self.tiles[changed]
        );
        let actual = pixels(&mut self.renderer, &self.scene, self.target, 0., [], false);
        let source_pixels = crate::performance_metrics_snapshot().backdrop_blur_source_pixels;
        let after = versions(&self.renderer);
        for (position, ((index, previous), (_, current))) in before.iter().zip(&after).enumerate() {
            let dependent = position < self.depth || position == changed + self.depth;
            assert_eq!(
                previous != current,
                dependent,
                "atlas write must refresh exactly its source and ancestors: depth={} changed={changed} index={index}",
                self.depth
            );
        }
        let reference = pixels(&mut self.renderer, &self.scene, self.target, 0., [], true);
        let reference_source_pixels =
            crate::performance_metrics_snapshot().backdrop_blur_source_pixels;
        assert!(
            actual == reference,
            "atlas refresh must match complete redraw: depth={} changed={changed}",
            self.depth
        );
        assert!(
            source_pixels < reference_source_pixels,
            "clean sibling source must not rasterize"
        );
        eprintln!(
            "atlas layer gate backend={} depth={} siblings={} changed={} source_pixels={} full_source_pixels={} pixels_equal=true",
            self.renderer.backend_info.label(),
            self.depth,
            self.tiles.len(),
            changed,
            source_pixels,
            reference_source_pixels
        );
    }

    fn unreferenced(&mut self) {
        let before = versions(&self.renderer);
        tile(&self.renderer.atlas, &key(700_100), [255, 255, 0, 255]);
        pixels(&mut self.renderer, &self.scene, self.target, 0., [], false);
        assert_eq!(
            versions(&self.renderer),
            before,
            "unreferenced tile must preserve every layer"
        );
    }

    fn finish(self) {
        device!(self.renderer, device, {
            device
                .destroy_texture_view(self.target.1)
                .expect("destroy readback view");
            device
                .destroy_texture(self.target.0)
                .expect("destroy readback texture");
        });
    }
}

fn verify(backend: RendererBackend) {
    for (depth, siblings) in [(0, 2), (1, 2), (3, 4), (6, 4)] {
        let mut fixture = Fixture::new(backend, depth, siblings);
        for changed in 0..siblings {
            fixture.update(changed);
        }
        fixture.unreferenced();
        fixture.finish();
    }
}

#[test]
#[cfg(feature = "nova-gfx-dx11")]
#[ignore = "requires Windows hardware D3D11"]
fn dx11_atlas_updates_refresh_only_dependent_layers() {
    verify(RendererBackend::NovaDx11);
}

#[test]
#[cfg(feature = "nova-gfx-dx12")]
#[ignore = "requires Windows hardware D3D12"]
fn dx12_atlas_updates_refresh_only_dependent_layers() {
    verify(RendererBackend::NovaDx12);
}

#[test]
#[cfg(feature = "nova-gfx-vulkan")]
#[ignore = "requires Windows hardware Vulkan"]
fn vulkan_atlas_updates_refresh_only_dependent_layers() {
    verify(RendererBackend::NovaVulkan);
}

#[test]
#[cfg(feature = "nova-gfx-opengl")]
#[ignore = "requires Windows hardware OpenGL 4.5"]
fn opengl_atlas_updates_refresh_only_dependent_layers() {
    verify(RendererBackend::NovaOpenGl);
}
