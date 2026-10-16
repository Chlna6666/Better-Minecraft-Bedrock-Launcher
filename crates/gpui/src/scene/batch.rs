use crate::{AtlasTextureId, ScaledPixels};
use std::{iter::Peekable, ops::Range, slice};

use super::{
    DrawOrder, MonochromeSprite, MonochromeSpriteSampling, PaintBackdropBlur, PaintBlur,
    PaintRendererExtension, PaintSurface, Path, PolychromeSprite, PrimitiveKind, Quad, Shadow,
    Underline,
};

type BatchCandidate = (Option<DrawOrder>, PrimitiveKind);
type BatchSortKey = (DrawOrder, PrimitiveKind);

fn batch_sort_key((order, kind): BatchCandidate) -> BatchSortKey {
    (order.unwrap_or(u32::MAX), kind)
}

fn first_two_batch_candidates(
    candidates: [BatchCandidate; 10],
) -> (BatchCandidate, BatchCandidate) {
    let [first, second, rest @ ..] = candidates;
    let (mut first, mut second) = if batch_sort_key(second) < batch_sort_key(first) {
        (second, first)
    } else {
        (first, second)
    };

    for candidate in rest {
        let candidate_key = batch_sort_key(candidate);
        if candidate_key < batch_sort_key(first) {
            second = first;
            first = candidate;
        } else if candidate_key < batch_sort_key(second) {
            second = candidate;
        }
    }

    (first, second)
}

pub(super) struct BatchIterator<'a> {
    pub(super) shadows_iter: super::primitive_stream::Cursor<'a, Shadow>,
    pub(super) quads_iter: super::primitive_stream::Cursor<'a, Quad>,
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
    pub(super) blurs: &'a [PaintBlur],
    pub(super) blurs_start: usize,
    pub(super) blurs_iter: Peekable<slice::Iter<'a, PaintBlur>>,
    pub(super) renderer_extensions: &'a [PaintRendererExtension],
    pub(super) renderer_extensions_start: usize,
    pub(super) renderer_extensions_iter: Peekable<slice::Iter<'a, PaintRendererExtension>>,
}

impl<'a> Iterator for BatchIterator<'a> {
    type Item = PrimitiveBatch<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let (first, second) = first_two_batch_candidates([
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
                self.blurs_iter.peek().map(|blur| blur.order),
                PrimitiveKind::Blur,
            ),
            (
                self.renderer_extensions_iter
                    .peek()
                    .map(|extension| extension.order),
                PrimitiveKind::RendererExtension,
            ),
        ]);

        let (batch_kind, max_order_and_kind) = if first.0.is_some() {
            (first.1, batch_sort_key(second))
        } else {
            return None;
        };

        match batch_kind {
            PrimitiveKind::Shadow => {
                let start = self.shadows_iter.position();
                let shadows = self.shadows_iter.remaining();
                let count = shadows
                    .partition_point(|shadow| (shadow.order, batch_kind) < max_order_and_kind);
                debug_assert!(
                    count > 0,
                    "selected Shadow precedes the next batch candidate"
                );
                self.shadows_iter.advance(count);
                Some(PrimitiveBatch::Shadows {
                    shadows: &shadows[..count],
                    range: start..start + count,
                })
            }
            PrimitiveKind::Quad => {
                let start = self.quads_iter.position();
                let quads = self.quads_iter.remaining();
                let count =
                    quads.partition_point(|quad| (quad.order, batch_kind) < max_order_and_kind);
                debug_assert!(count > 0, "selected Quad precedes the next batch candidate");
                self.quads_iter.advance(count);
                Some(PrimitiveBatch::Quads {
                    quads: &quads[..count],
                    range: start..start + count,
                })
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
            PrimitiveKind::Blur => {
                let blurs_start = self.blurs_start;
                let mut blurs_end = blurs_start + 1;
                self.blurs_iter.next();
                while self
                    .blurs_iter
                    .next_if(|blur| (blur.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    blurs_end += 1;
                }
                self.blurs_start = blurs_end;
                Some(PrimitiveBatch::Blurs(&self.blurs[blurs_start..blurs_end]))
            }
            PrimitiveKind::RendererExtension => {
                let start = self.renderer_extensions_start;
                let mut end = start + 1;
                self.renderer_extensions_iter.next();
                while self
                    .renderer_extensions_iter
                    .next_if(|extension| (extension.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    end += 1;
                }
                self.renderer_extensions_start = end;
                Some(PrimitiveBatch::RendererExtensions(
                    &self.renderer_extensions[start..end],
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
    Shadows {
        shadows: &'a [Shadow],
        range: Range<usize>,
    },
    Quads {
        quads: &'a [Quad],
        range: Range<usize>,
    },
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
    Blurs(&'a [PaintBlur]),
    RendererExtensions(&'a [PaintRendererExtension]),
}
