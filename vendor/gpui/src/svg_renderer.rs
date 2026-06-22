use crate::{AssetSource, DevicePixels, IsZero, Result, SharedString, Size, size};
use resvg::tiny_skia::Pixmap;
use std::{
    hash::Hash,
    sync::{Arc, LazyLock},
};

/// When rendering SVGs, we render them at twice the size to get a higher-quality result.
pub const SMOOTH_SVG_SCALE_FACTOR: f32 = 2.;

#[derive(Clone, PartialEq, Hash, Eq)]
pub(crate) struct RenderSvgParams {
    pub(crate) path: SharedString,
    pub(crate) size: Size<DevicePixels>,
}

#[derive(Clone)]
/// Renders SVG bytes and embedded SVG assets into pixmaps.
pub struct SvgRenderer {
    asset_source: Arc<dyn AssetSource>,
    usvg_options: Arc<usvg::Options<'static>>,
}

/// The target size used when rasterizing an SVG.
pub enum SvgSize {
    /// Render the SVG to an explicit device-pixel size.
    Size(Size<DevicePixels>),
    /// Render the SVG at a scale factor relative to its natural size.
    ScaleFactor(f32),
}

impl SvgRenderer {
    /// Creates an SVG renderer that can load embedded assets from the provided source.
    pub fn new(asset_source: Arc<dyn AssetSource>) -> Self {
        static FONT_DB: LazyLock<Arc<usvg::fontdb::Database>> = LazyLock::new(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        });
        let default_font_resolver = usvg::FontResolver::default_font_selector();
        let font_resolver = Box::new(
            move |font: &usvg::Font, db: &mut Arc<usvg::fontdb::Database>| {
                if db.is_empty() {
                    *db = FONT_DB.clone();
                }
                default_font_resolver(font, db)
            },
        );
        let options = usvg::Options {
            font_resolver: usvg::FontResolver {
                select_font: font_resolver,
                select_fallback: usvg::FontResolver::default_fallback_selector(),
            },
            ..Default::default()
        };
        Self {
            asset_source,
            usvg_options: Arc::new(options),
        }
    }

    pub(crate) fn render(
        &self,
        params: &RenderSvgParams,
    ) -> Result<Option<(Size<DevicePixels>, Vec<u8>)>> {
        anyhow::ensure!(!params.size.is_zero(), "can't render at a zero size");

        // Load the tree.
        let Some(bytes) = self.asset_source.load(&params.path)? else {
            return Ok(None);
        };

        let pixmap = self.render_pixmap(&bytes, SvgSize::Size(params.size))?;

        // Convert the pixmap's pixels into an alpha mask.
        let size = Size::new(
            DevicePixels(pixmap.width() as i32),
            DevicePixels(pixmap.height() as i32),
        );
        let alpha_mask = pixmap
            .pixels()
            .iter()
            .map(|p| p.alpha())
            .collect::<Vec<_>>();
        Ok(Some((size, alpha_mask)))
    }

    /// Renders SVG bytes into a pixmap using the provided target size.
    pub fn render_pixmap(&self, bytes: &[u8], size: SvgSize) -> Result<Pixmap, usvg::Error> {
        let tree = usvg::Tree::from_data(bytes, &self.usvg_options)?;
        let svg_size = tree.size();
        let svg_width = svg_size.width();
        let svg_height = svg_size.height();

        if !is_valid_svg_dimension(svg_width) || !is_valid_svg_dimension(svg_height) {
            return Err(usvg::Error::InvalidSize);
        }

        let (pixmap_width, pixmap_height, transform) = match size {
            SvgSize::Size(size) => {
                let width = u32::try_from(size.width.0).map_err(|_| usvg::Error::InvalidSize)?;
                let height = u32::try_from(size.height.0).map_err(|_| usvg::Error::InvalidSize)?;
                if width == 0 || height == 0 {
                    return Err(usvg::Error::InvalidSize);
                }

                (
                    width,
                    height,
                    resvg::tiny_skia::Transform::from_scale(
                        width as f32 / svg_width,
                        height as f32 / svg_height,
                    ),
                )
            }
            SvgSize::ScaleFactor(scale) => {
                if !scale.is_finite() || scale <= 0.0 {
                    return Err(usvg::Error::InvalidSize);
                }

                (
                    pixmap_dimension(svg_width * scale)?,
                    pixmap_dimension(svg_height * scale)?,
                    resvg::tiny_skia::Transform::from_scale(scale, scale),
                )
            }
        };

        let mut pixmap = resvg::tiny_skia::Pixmap::new(pixmap_width, pixmap_height)
            .ok_or(usvg::Error::InvalidSize)?;

        resvg::render(&tree, transform, &mut pixmap.as_mut());

        Ok(pixmap)
    }

    /// Returns the natural SVG size in device pixels.
    pub fn natural_size(&self, bytes: &[u8]) -> Result<Size<DevicePixels>, usvg::Error> {
        let tree = usvg::Tree::from_data(bytes, &self.usvg_options)?;
        let svg_size = tree.size();
        Ok(size(
            DevicePixels(pixmap_dimension(svg_size.width())? as i32),
            DevicePixels(pixmap_dimension(svg_size.height())? as i32),
        ))
    }
}

fn is_valid_svg_dimension(dimension: f32) -> bool {
    dimension.is_finite() && dimension > 0.0
}

fn pixmap_dimension(dimension: f32) -> Result<u32, usvg::Error> {
    if !is_valid_svg_dimension(dimension) || dimension > u32::MAX as f32 {
        return Err(usvg::Error::InvalidSize);
    }

    Ok(dimension.ceil() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECT_SVG: &[u8] = br#"
        <svg xmlns="http://www.w3.org/2000/svg" width="10" height="20" viewBox="0 0 10 20">
            <rect width="10" height="20" fill="black"/>
        </svg>
    "#;

    fn renderer() -> SvgRenderer {
        SvgRenderer::new(Arc::new(()))
    }

    #[test]
    fn explicit_size_uses_requested_width_and_height() {
        let pixmap = renderer()
            .render_pixmap(
                RECT_SVG,
                SvgSize::Size(Size::new(DevicePixels(30), DevicePixels(30))),
            )
            .expect("svg renders");

        assert_eq!(pixmap.width(), 30);
        assert_eq!(pixmap.height(), 30);
    }

    #[test]
    fn scale_factor_preserves_natural_aspect_ratio() {
        let pixmap = renderer()
            .render_pixmap(RECT_SVG, SvgSize::ScaleFactor(2.0))
            .expect("svg renders");

        assert_eq!(pixmap.width(), 20);
        assert_eq!(pixmap.height(), 40);
    }

    #[test]
    fn natural_size_uses_svg_dimensions() {
        let size = renderer().natural_size(RECT_SVG).expect("svg parses");

        assert_eq!(u32::from(size.width), 10);
        assert_eq!(u32::from(size.height), 20);
    }
}
