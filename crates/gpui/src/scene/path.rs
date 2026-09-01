use crate::{Background, Bounds, ContentMask, Pixels, Point, ScaledPixels, point};
use fearless_simd::{Level, Simd, dispatch, f32x4, prelude::*};
use std::{
    fmt::Debug,
    ops::{Add, Sub},
    sync::{
        LazyLock,
        atomic::{AtomicUsize, Ordering::SeqCst},
    },
};

use super::{DrawOrder, Primitive};

#[allow(non_camel_case_types, unused)]
pub(crate) type PathVertex_ScaledPixels = PathVertex<ScaledPixels>;

type PathVerticesTransformFn =
    fn(&[PathVertex<Pixels>], f32, f32, Point<ScaledPixels>) -> Vec<PathVertex<ScaledPixels>>;

static PATH_SIMD_LEVEL: LazyLock<Level> = LazyLock::new(Level::new);

#[inline(always)]
fn transform_point(
    point: Point<Pixels>,
    device_scale: f32,
    visual_scale: f32,
    translation: Point<ScaledPixels>,
) -> Point<ScaledPixels> {
    point.scale(device_scale) * visual_scale + translation
}

fn transform_path_vertices_scalar(
    vertices: &[PathVertex<Pixels>],
    device_scale: f32,
    visual_scale: f32,
    translation: Point<ScaledPixels>,
) -> Vec<PathVertex<ScaledPixels>> {
    vertices
        .iter()
        .map(|vertex| PathVertex {
            xy_position: transform_point(
                vertex.xy_position,
                device_scale,
                visual_scale,
                translation,
            ),
            st_position: vertex.st_position,
            content_mask: vertex.content_mask,
        })
        .collect()
}

fn transform_path_vertices_selected(
    vertices: &[PathVertex<Pixels>],
    device_scale: f32,
    visual_scale: f32,
    translation: Point<ScaledPixels>,
) -> Vec<PathVertex<ScaledPixels>> {
    let level = *PATH_SIMD_LEVEL;
    if level.is_fallback() {
        return transform_path_vertices_scalar(vertices, device_scale, visual_scale, translation);
    }

    // Cap x86 at AVX2 so a UI frame does not opt into AVX-512 frequency and power trade-offs.
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    if let Some(avx2) = level.as_avx2() {
        return dispatch!(
            Level::Avx2(avx2),
            simd => transform_path_vertices_simd(
                simd,
                vertices,
                device_scale,
                visual_scale,
                translation
            )
        );
    }

    dispatch!(
        level,
        simd => transform_path_vertices_simd(
            simd,
            vertices,
            device_scale,
            visual_scale,
            translation
        )
    )
}

#[inline(always)]
fn transform_path_vertices_simd<S: Simd>(
    simd: S,
    vertices: &[PathVertex<Pixels>],
    device_scale: f32,
    visual_scale: f32,
    translation: Point<ScaledPixels>,
) -> Vec<PathVertex<ScaledPixels>> {
    let mut transformed = Vec::with_capacity(vertices.len());
    let device_scale_value = device_scale;
    let visual_scale_value = visual_scale;
    let device_scale = f32x4::splat(simd, device_scale_value);
    let visual_scale = f32x4::splat(simd, visual_scale_value);
    let translation_x = f32x4::splat(simd, translation.x.0);
    let translation_y = f32x4::splat(simd, translation.y.0);
    let mut chunks = vertices.chunks_exact(4);

    for chunk in &mut chunks {
        let x = f32x4::from_fn(simd, |index| chunk[index].xy_position.x.0);
        let y = f32x4::from_fn(simd, |index| chunk[index].xy_position.y.0);
        let x = (x * device_scale) * visual_scale + translation_x;
        let y = (y * device_scale) * visual_scale + translation_y;
        let mut transformed_x = [0.0; 4];
        let mut transformed_y = [0.0; 4];
        x.store_slice(&mut transformed_x);
        y.store_slice(&mut transformed_y);

        for ((vertex, x), y) in chunk.iter().zip(transformed_x).zip(transformed_y) {
            transformed.push(PathVertex {
                xy_position: point(ScaledPixels(x), ScaledPixels(y)),
                st_position: vertex.st_position,
                content_mask: vertex.content_mask,
            });
        }
    }

    transformed.extend(transform_path_vertices_scalar(
        chunks.remainder(),
        device_scale_value,
        visual_scale_value,
        translation,
    ));
    transformed
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PathId(pub(crate) usize);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PathCacheId(pub(crate) usize);

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct PathGeometryGeneration(pub(crate) u64);

/// A line made up of a series of vertices and control points.
#[derive(Clone, Debug, PartialEq)]
pub struct Path<P: Clone + Debug + Default + PartialEq> {
    pub(crate) id: PathId,
    pub(crate) cache_id: PathCacheId,
    pub(crate) geometry_generation: PathGeometryGeneration,
    pub(crate) order: DrawOrder,
    pub(crate) bounds: Bounds<P>,
    pub(crate) content_mask: ContentMask<P>,
    pub(crate) vertices: Vec<PathVertex<P>>,
    pub(crate) color: Background,
    start: Point<P>,
    current: Point<P>,
    contour_count: usize,
}

impl Path<Pixels> {
    /// Create a new path with the given starting point.
    pub fn new(start: Point<Pixels>) -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

        Self {
            id: PathId(0),
            cache_id: PathCacheId(NEXT_ID.fetch_add(1, SeqCst)),
            geometry_generation: PathGeometryGeneration::default(),
            order: DrawOrder::default(),
            vertices: Vec::new(),
            start,
            current: start,
            bounds: Bounds {
                origin: start,
                size: Default::default(),
            },
            content_mask: Default::default(),
            color: Default::default(),
            contour_count: 0,
        }
    }

    /// Scale this path by the given factor.
    pub fn scale(&self, factor: f32) -> Path<ScaledPixels> {
        Path {
            id: self.id,
            cache_id: self.cache_id,
            geometry_generation: self.geometry_generation,
            order: self.order,
            bounds: self.bounds.scale(factor),
            content_mask: self.content_mask.scale(factor),
            // Rasterizers consume the path-level mask. The per-vertex field is only a tiny legacy
            // marker retained for internal struct-literal compatibility, so scaling vertices now
            // copies only xy/st data instead of duplicating a large ContentMask for every vertex.
            vertices: self
                .vertices
                .iter()
                .map(|vertex| PathVertex {
                    xy_position: vertex.xy_position.scale(factor),
                    st_position: vertex.st_position,
                    content_mask: vertex.content_mask,
                })
                .collect(),
            start: self.start.map(|start| start.scale(factor)),
            current: self.current.scale(factor),
            contour_count: self.contour_count,
            color: self.color,
        }
    }

    /// Converts a logical path into final device-space geometry for painting in one vertex pass.
    ///
    /// The arithmetic order intentionally matches `scale(device_scale).transform_uniform(...)`:
    /// DPI scaling happens before the renderer-owned visual scale and translation. This preserves
    /// the previous floating-point results while avoiding an intermediate scaled vertex vector.
    pub(crate) fn scale_and_transform_for_paint(
        &self,
        device_scale: f32,
        visual_scale: f32,
        translation: Point<ScaledPixels>,
    ) -> Path<ScaledPixels> {
        self.scale_and_transform_for_paint_with(
            device_scale,
            visual_scale,
            translation,
            transform_path_vertices_scalar,
        )
    }

    #[cfg(feature = "bench")]
    pub(crate) fn scale_and_transform_for_paint_scalar(
        &self,
        device_scale: f32,
        visual_scale: f32,
        translation: Point<ScaledPixels>,
    ) -> Path<ScaledPixels> {
        self.scale_and_transform_for_paint_with(
            device_scale,
            visual_scale,
            translation,
            transform_path_vertices_scalar,
        )
    }

    #[cfg(feature = "bench")]
    pub(crate) fn scale_and_transform_for_paint_simd(
        &self,
        device_scale: f32,
        visual_scale: f32,
        translation: Point<ScaledPixels>,
    ) -> Path<ScaledPixels> {
        self.scale_and_transform_for_paint_with(
            device_scale,
            visual_scale,
            translation,
            transform_path_vertices_selected,
        )
    }

    fn scale_and_transform_for_paint_with(
        &self,
        device_scale: f32,
        visual_scale: f32,
        translation: Point<ScaledPixels>,
        transform_vertices: PathVerticesTransformFn,
    ) -> Path<ScaledPixels> {
        let scaled_bounds = self.bounds.scale(device_scale);
        let bounds = Bounds {
            origin: scaled_bounds.origin * visual_scale + translation,
            size: scaled_bounds.size.map(|value| value * visual_scale),
        };
        let scaled_mask = self.content_mask.scale(device_scale);
        let content_mask = ContentMask {
            bounds: Bounds {
                origin: scaled_mask.bounds.origin * visual_scale + translation,
                size: scaled_mask.bounds.size.map(|value| value * visual_scale),
            },
            corner_bounds: Bounds {
                origin: scaled_mask.corner_bounds.origin * visual_scale + translation,
                size: scaled_mask
                    .corner_bounds
                    .size
                    .map(|value| value * visual_scale),
            },
            corner_radii: scaled_mask.corner_radii.map(|value| *value * visual_scale),
        };

        Path {
            id: self.id,
            cache_id: self.cache_id,
            geometry_generation: self.geometry_generation,
            order: self.order,
            bounds,
            content_mask,
            vertices: transform_vertices(&self.vertices, device_scale, visual_scale, translation),
            start: transform_point(self.start, device_scale, visual_scale, translation),
            current: transform_point(self.current, device_scale, visual_scale, translation),
            contour_count: self.contour_count,
            color: self.color,
        }
    }

    /// Move the start, current point to the given point.
    pub fn move_to(&mut self, to: Point<Pixels>) {
        self.bump_geometry_generation();
        self.contour_count += 1;
        self.start = to;
        self.current = to;
    }

    /// Draw a straight line from the current point to the given point.
    pub fn line_to(&mut self, to: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle(
                (self.start, self.current, to),
                (point(0., 1.), point(0., 1.), point(0., 1.)),
            );
        }
        self.current = to;
    }

    /// Draw a curve from the current point to the given point, using the given control point.
    pub fn curve_to(&mut self, to: Point<Pixels>, ctrl: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle(
                (self.start, self.current, to),
                (point(0., 1.), point(0., 1.), point(0., 1.)),
            );
        }

        self.push_triangle(
            (self.current, ctrl, to),
            (point(0., 0.), point(0.5, 0.), point(1., 1.)),
        );
        self.current = to;
    }

    /// Push a triangle to the Path.
    pub fn push_triangle(
        &mut self,
        xy: (Point<Pixels>, Point<Pixels>, Point<Pixels>),
        st: (Point<f32>, Point<f32>, Point<f32>),
    ) {
        self.bump_geometry_generation();
        self.bounds = self
            .bounds
            .union(&Bounds {
                origin: xy.0,
                size: Default::default(),
            })
            .union(&Bounds {
                origin: xy.1,
                size: Default::default(),
            })
            .union(&Bounds {
                origin: xy.2,
                size: Default::default(),
            });

        self.vertices.push(PathVertex {
            xy_position: xy.0,
            st_position: st.0,
            content_mask: 0,
        });
        self.vertices.push(PathVertex {
            xy_position: xy.1,
            st_position: st.1,
            content_mask: 0,
        });
        self.vertices.push(PathVertex {
            xy_position: xy.2,
            st_position: st.2,
            content_mask: 0,
        });
    }

    fn bump_geometry_generation(&mut self) {
        self.geometry_generation.0 = self.geometry_generation.0.saturating_add(1);
    }
}

impl Path<ScaledPixels> {
    pub(crate) fn transform_uniform(
        mut self,
        scale: f32,
        translation: Point<ScaledPixels>,
    ) -> Self {
        if scale == 1.0 && translation == Point::default() {
            return self;
        }

        self.bounds = Bounds {
            origin: self.bounds.origin * scale + translation,
            size: self.bounds.size.map(|value| value * scale),
        };
        self.content_mask = ContentMask {
            bounds: Bounds {
                origin: self.content_mask.bounds.origin * scale + translation,
                size: self.content_mask.bounds.size.map(|value| value * scale),
            },
            corner_bounds: Bounds {
                origin: self.content_mask.corner_bounds.origin * scale + translation,
                size: self
                    .content_mask
                    .corner_bounds
                    .size
                    .map(|value| value * scale),
            },
            corner_radii: self.content_mask.corner_radii.map(|value| *value * scale),
        };
        for vertex in &mut self.vertices {
            vertex.xy_position = vertex.xy_position * scale + translation;
        }
        self.start = self.start * scale + translation;
        self.current = self.current * scale + translation;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::px;

    #[test]
    fn bulk_transform_matches_scalar_path() {
        let mut path = Path::new(point(px(0.0), px(0.0)));
        for index in 0..1_365 {
            let x = index as f32;
            path.push_triangle(
                (
                    point(px(x), px(x * 0.5)),
                    point(px(x + 1.0), px(x * 0.5 + 1.0)),
                    point(px(x + 2.0), px(x * 0.5)),
                ),
                (point(0.0, 0.0), point(0.5, 1.0), point(1.0, 0.0)),
            );
        }
        path.content_mask = ContentMask::new(path.bounds);

        let scalar_vertices = transform_path_vertices_scalar(
            &path.vertices,
            1.25,
            0.875,
            point(ScaledPixels(4.0), ScaledPixels(-3.0)),
        );
        let selected_vertices = transform_path_vertices_selected(
            &path.vertices,
            1.25,
            0.875,
            point(ScaledPixels(4.0), ScaledPixels(-3.0)),
        );

        assert_eq!(selected_vertices, scalar_vertices);
        let transformed = path.scale_and_transform_for_paint(
            1.25,
            0.875,
            point(ScaledPixels(4.0), ScaledPixels(-3.0)),
        );
        assert_eq!(transformed.vertices, scalar_vertices);
    }
}

impl<T> Path<T>
where
    T: Clone + Debug + Default + PartialEq + PartialOrd + Add<T, Output = T> + Sub<Output = T>,
{
    #[allow(unused)]
    pub(crate) fn clipped_bounds(&self) -> Bounds<T> {
        self.bounds.intersect(&self.content_mask.bounds)
    }
}

impl From<Path<ScaledPixels>> for Primitive {
    fn from(path: Path<ScaledPixels>) -> Self {
        Primitive::Path(path)
    }
}

#[derive(Clone, Debug, PartialEq)]
#[repr(C)]
pub(crate) struct PathVertex<P: Clone + Debug + Default + PartialEq> {
    pub(crate) xy_position: Point<P>,
    pub(crate) st_position: Point<f32>,
    /// Legacy marker retained only so existing internal struct literals remain source-compatible.
    /// Path clipping is represented once on `Path::content_mask`; no renderer reads this field.
    pub(crate) content_mask: u8,
}

impl PathVertex<Pixels> {
    pub fn scale(&self, factor: f32) -> PathVertex<ScaledPixels> {
        PathVertex {
            xy_position: self.xy_position.scale(factor),
            st_position: self.st_position,
            content_mask: self.content_mask,
        }
    }
}
