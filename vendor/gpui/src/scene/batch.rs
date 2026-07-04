use crate::{AtlasTextureId, ScaledPixels};
use std::{iter::Peekable, slice};

use super::{
    MonochromeSprite, MonochromeSpriteSampling, PaintBackdropBlur, PaintGpuMesh3d, PaintSurface,
    Path, PolychromeSprite, PrimitiveKind, Quad, Shadow, Underline,
};

pub(super) struct BatchIterator<'a> {
    pub(super) shadows: &'a [Shadow],
    pub(super) shadows_start: usize,
    pub(super) shadows_iter: Peekable<slice::Iter<'a, Shadow>>,
    pub(super) quads: &'a [Quad],
    pub(super) quads_start: usize,
    pub(super) quads_iter: Peekable<slice::Iter<'a, Quad>>,
    pub(super) paths: &'a [Path<ScaledPixels>],
    pub(super) paths_start: usize,
    pub(super) paths_iter: Peekable<slice::Iter<'a, Path<ScaledPixels>>>,
    pub(super) underlines: &'a [Underline],
    pub(super) underlines_start: usize,
    pub(super) underlines_iter: Peekable<slice::Iter<'a, Underline>>,
    pub(super) monochrome_sprites: &'a [MonochromeSprite],
    pub(super) monochrome_sprites_start: usize,
    pub(super) monochrome_sprites_iter: Peekable<slice::Iter<'a, MonochromeSprite>>,
    pub(super) polychrome_sprites: &'a [PolychromeSprite],
    pub(super) polychrome_sprites_start: usize,
    pub(super) polychrome_sprites_iter: Peekable<slice::Iter<'a, PolychromeSprite>>,
    pub(super) surfaces: &'a [PaintSurface],
    pub(super) surfaces_start: usize,
    pub(super) surfaces_iter: Peekable<slice::Iter<'a, PaintSurface>>,
    pub(super) backdrop_blurs: &'a [PaintBackdropBlur],
    pub(super) backdrop_blurs_start: usize,
    pub(super) backdrop_blurs_iter: Peekable<slice::Iter<'a, PaintBackdropBlur>>,
    pub(super) gpu_meshes_3d: &'a [PaintGpuMesh3d],
    pub(super) gpu_meshes_3d_start: usize,
    pub(super) gpu_meshes_3d_iter: Peekable<slice::Iter<'a, PaintGpuMesh3d>>,
}

impl<'a> Iterator for BatchIterator<'a> {
    type Item = PrimitiveBatch<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut orders_and_kinds = [
            (
                self.shadows_iter.peek().map(|s| s.order),
                PrimitiveKind::Shadow,
            ),
            (self.quads_iter.peek().map(|q| q.order), PrimitiveKind::Quad),
            (self.paths_iter.peek().map(|q| q.order), PrimitiveKind::Path),
            (
                self.underlines_iter.peek().map(|u| u.order),
                PrimitiveKind::Underline,
            ),
            (
                self.monochrome_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::MonochromeSprite,
            ),
            (
                self.polychrome_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::PolychromeSprite,
            ),
            (
                self.surfaces_iter.peek().map(|s| s.order),
                PrimitiveKind::Surface,
            ),
            (
                self.backdrop_blurs_iter.peek().map(|blur| blur.order),
                PrimitiveKind::BackdropBlur,
            ),
            (
                self.gpu_meshes_3d_iter.peek().map(|mesh| mesh.order),
                PrimitiveKind::GpuMesh3d,
            ),
        ];
        orders_and_kinds.sort_by_key(|(order, kind)| (order.unwrap_or(u32::MAX), *kind));

        let first = orders_and_kinds[0];
        let second = orders_and_kinds[1];
        let (batch_kind, max_order_and_kind) = if first.0.is_some() {
            (first.1, (second.0.unwrap_or(u32::MAX), second.1))
        } else {
            return None;
        };

        match batch_kind {
            PrimitiveKind::Shadow => {
                let shadows_start = self.shadows_start;
                let mut shadows_end = shadows_start + 1;
                self.shadows_iter.next();
                while self
                    .shadows_iter
                    .next_if(|shadow| (shadow.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    shadows_end += 1;
                }
                self.shadows_start = shadows_end;
                Some(PrimitiveBatch::Shadows(
                    &self.shadows[shadows_start..shadows_end],
                ))
            }
            PrimitiveKind::Quad => {
                let quads_start = self.quads_start;
                let mut quads_end = quads_start + 1;
                self.quads_iter.next();
                while self
                    .quads_iter
                    .next_if(|quad| (quad.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    quads_end += 1;
                }
                self.quads_start = quads_end;
                Some(PrimitiveBatch::Quads(&self.quads[quads_start..quads_end]))
            }
            PrimitiveKind::Path => {
                let paths_start = self.paths_start;
                let mut paths_end = paths_start + 1;
                self.paths_iter.next();
                while self
                    .paths_iter
                    .next_if(|path| (path.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    paths_end += 1;
                }
                self.paths_start = paths_end;
                Some(PrimitiveBatch::Paths(&self.paths[paths_start..paths_end]))
            }
            PrimitiveKind::Underline => {
                let underlines_start = self.underlines_start;
                let mut underlines_end = underlines_start + 1;
                self.underlines_iter.next();
                while self
                    .underlines_iter
                    .next_if(|underline| (underline.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    underlines_end += 1;
                }
                self.underlines_start = underlines_end;
                Some(PrimitiveBatch::Underlines(
                    &self.underlines[underlines_start..underlines_end],
                ))
            }
            PrimitiveKind::MonochromeSprite => {
                let first_sprite = self.monochrome_sprites_iter.peek().unwrap();
                let texture_id = first_sprite.tile.texture_id;
                let sampling = first_sprite.sampling();
                let sprites_start = self.monochrome_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.monochrome_sprites_iter.next();
                while self
                    .monochrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                            && sprite.sampling() == sampling
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.monochrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::MonochromeSprites {
                    texture_id,
                    sampling,
                    sprites: &self.monochrome_sprites[sprites_start..sprites_end],
                })
            }
            PrimitiveKind::PolychromeSprite => {
                let texture_id = self.polychrome_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.polychrome_sprites_start;
                let mut sprites_end = self.polychrome_sprites_start + 1;
                self.polychrome_sprites_iter.next();
                while self
                    .polychrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.polychrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::PolychromeSprites {
                    texture_id,
                    sprites: &self.polychrome_sprites[sprites_start..sprites_end],
                })
            }
            PrimitiveKind::Surface => {
                let surfaces_start = self.surfaces_start;
                let mut surfaces_end = surfaces_start + 1;
                self.surfaces_iter.next();
                while self
                    .surfaces_iter
                    .next_if(|surface| (surface.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    surfaces_end += 1;
                }
                self.surfaces_start = surfaces_end;
                Some(PrimitiveBatch::Surfaces(
                    &self.surfaces[surfaces_start..surfaces_end],
                ))
            }
            PrimitiveKind::BackdropBlur => {
                let blurs_start = self.backdrop_blurs_start;
                let mut blurs_end = blurs_start + 1;
                self.backdrop_blurs_iter.next();
                while self
                    .backdrop_blurs_iter
                    .next_if(|blur| (blur.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    blurs_end += 1;
                }
                self.backdrop_blurs_start = blurs_end;
                Some(PrimitiveBatch::BackdropBlurs(
                    &self.backdrop_blurs[blurs_start..blurs_end],
                ))
            }
            PrimitiveKind::GpuMesh3d => {
                let meshes_start = self.gpu_meshes_3d_start;
                let mut meshes_end = meshes_start + 1;
                self.gpu_meshes_3d_iter.next();
                while self
                    .gpu_meshes_3d_iter
                    .next_if(|mesh| (mesh.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    meshes_end += 1;
                }
                self.gpu_meshes_3d_start = meshes_end;
                Some(PrimitiveBatch::GpuMeshes3d(
                    &self.gpu_meshes_3d[meshes_start..meshes_end],
                ))
            }
        }
    }
}

#[derive(Debug)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
pub(crate) enum PrimitiveBatch<'a> {
    Shadows(&'a [Shadow]),
    Quads(&'a [Quad]),
    Paths(&'a [Path<ScaledPixels>]),
    Underlines(&'a [Underline]),
    MonochromeSprites {
        texture_id: AtlasTextureId,
        sampling: MonochromeSpriteSampling,
        sprites: &'a [MonochromeSprite],
    },
    PolychromeSprites {
        texture_id: AtlasTextureId,
        sprites: &'a [PolychromeSprite],
    },
    Surfaces(&'a [PaintSurface]),
    BackdropBlurs(&'a [PaintBackdropBlur]),
    GpuMeshes3d(&'a [PaintGpuMesh3d]),
}
