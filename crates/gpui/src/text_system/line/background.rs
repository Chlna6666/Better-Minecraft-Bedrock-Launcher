use crate::{
    App, Bounds, Half, Hsla, LineLayout, Pixels, Point, Result, TextAlign, TextBackgroundPadding,
    Window, WrapBoundary, fill, point, px, size,
};

use super::alignment::aligned_origin_x;
use super::types::DecorationRun;

pub(super) fn paint_line_background(
    origin: Point<Pixels>,
    layout: &LineLayout,
    line_height: Pixels,
    align: TextAlign,
    align_width: Option<Pixels>,
    decoration_runs: &[DecorationRun],
    wrap_boundaries: &[WrapBoundary],
    window: &mut Window,
    cx: &mut App,
) -> Result<()> {
    let line_bounds = Bounds::new(
        origin,
        size(
            layout.width,
            line_height * (wrap_boundaries.len() as f32 + 1.),
        ),
    );
    window.paint_layer(line_bounds, |window| {
        let mut decoration_runs = decoration_runs.iter();
        let mut wraps = wrap_boundaries.iter().peekable();
        let mut run_end = 0;
        let mut current_background: Option<(
            Point<Pixels>,
            Hsla,
            Option<Pixels>,
            Option<TextBackgroundPadding>,
        )> = None;
        let text_system = cx.text_system().clone();
        let mut glyph_origin = point(
            aligned_origin_x(
                origin,
                align_width.unwrap_or(layout.width),
                px(0.0),
                &align,
                layout,
                wraps.peek(),
            ),
            origin.y,
        );
        let mut prev_glyph_position = Point::default();
        let mut max_glyph_size = size(px(0.), px(0.));
        for (run_ix, run) in layout.runs.iter().enumerate() {
            max_glyph_size = text_system.bounding_box(run.font_id, layout.font_size).size;

            for (glyph_ix, glyph) in run.glyphs.iter().enumerate() {
                glyph_origin.x += glyph.position.x - prev_glyph_position.x;

                if wraps.peek() == Some(&&WrapBoundary { run_ix, glyph_ix }) {
                    wraps.next();
                    if let Some((
                        background_origin,
                        background_color,
                        background_corner_radius,
                        background_padding,
                    )) = current_background.as_mut()
                    {
                        if glyph_origin.x == background_origin.x {
                            background_origin.x -= max_glyph_size.width.half()
                        }
                        paint_text_background(
                            window,
                            Bounds {
                                origin: *background_origin,
                                size: size(glyph_origin.x - background_origin.x, line_height),
                            },
                            *background_color,
                            *background_corner_radius,
                            *background_padding,
                            line_height,
                            layout.ascent + layout.descent,
                        );
                        background_origin.x = origin.x;
                        background_origin.y += line_height;
                    }

                    glyph_origin.x = aligned_origin_x(
                        origin,
                        align_width.unwrap_or(layout.width),
                        glyph.position.x,
                        &align,
                        layout,
                        wraps.peek(),
                    );
                    glyph_origin.y += line_height;
                }
                prev_glyph_position = glyph.position;

                let mut finished_background: Option<(
                    Point<Pixels>,
                    Hsla,
                    Option<Pixels>,
                    Option<TextBackgroundPadding>,
                )> = None;
                if glyph.index >= run_end {
                    let mut style_run = decoration_runs.next();

                    // ignore style runs that apply to a partial glyph
                    while let Some(run) = style_run {
                        if glyph.index < run_end + (run.len as usize) {
                            break;
                        }
                        run_end += run.len as usize;
                        style_run = decoration_runs.next();
                    }

                    if let Some(style_run) = style_run {
                        if let Some((
                            _,
                            background_color,
                            background_corner_radius,
                            background_padding,
                        )) = &mut current_background
                            && (style_run.background_color.as_ref() != Some(background_color)
                                || style_run.background_corner_radius != *background_corner_radius
                                || style_run.background_padding != *background_padding)
                        {
                            finished_background = current_background.take();
                        }
                        if let Some(run_background) = style_run.background_color {
                            if current_background.is_none() {
                                current_background = Some((
                                    point(glyph_origin.x, glyph_origin.y),
                                    run_background,
                                    style_run.background_corner_radius,
                                    style_run.background_padding,
                                ));
                            }
                        }
                        run_end += style_run.len as usize;
                    } else {
                        run_end = layout.len;
                        finished_background = current_background.take();
                    }
                }

                if let Some((
                    mut background_origin,
                    background_color,
                    background_corner_radius,
                    background_padding,
                )) = finished_background
                {
                    if background_origin.x == glyph_origin.x {
                        background_origin.x -= max_glyph_size.width.half();
                    };
                    paint_text_background(
                        window,
                        Bounds {
                            origin: background_origin,
                            size: size(glyph_origin.x - background_origin.x, line_height),
                        },
                        background_color,
                        background_corner_radius,
                        background_padding,
                        line_height,
                        layout.ascent + layout.descent,
                    );
                }
            }
        }

        let mut last_line_end_x = origin.x + layout.width;
        if let Some(boundary) = wrap_boundaries.last() {
            let run = &layout.runs[boundary.run_ix];
            let glyph = &run.glyphs[boundary.glyph_ix];
            last_line_end_x -= glyph.position.x;
        }

        if let Some((
            mut background_origin,
            background_color,
            background_corner_radius,
            background_padding,
        )) = current_background.take()
        {
            if last_line_end_x == background_origin.x {
                background_origin.x -= max_glyph_size.width.half()
            };
            paint_text_background(
                window,
                Bounds {
                    origin: background_origin,
                    size: size(last_line_end_x - background_origin.x, line_height),
                },
                background_color,
                background_corner_radius,
                background_padding,
                line_height,
                layout.ascent + layout.descent,
            );
        }

        Ok(())
    })
}

fn paint_text_background(
    window: &mut Window,
    mut bounds: Bounds<Pixels>,
    background_color: Hsla,
    background_corner_radius: Option<Pixels>,
    background_padding: Option<TextBackgroundPadding>,
    line_height: Pixels,
    text_height: Pixels,
) {
    if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
        return;
    }

    if let Some(background_padding) = background_padding {
        let height = (text_height + background_padding.top + background_padding.bottom)
            .min(line_height)
            .max(px(0.));
        if height <= px(0.) {
            return;
        }

        bounds.origin.x -= background_padding.left;
        bounds.origin.y += (line_height - height) / 2.;
        bounds.size.width += background_padding.left + background_padding.right;
        bounds.size.height = height;
    }

    if bounds.size.width <= px(0.) {
        return;
    }

    window.paint_quad(
        fill(bounds, background_color).corner_radii(background_corner_radius.unwrap_or(px(0.))),
    );
}

pub(super) fn max_background_padding(decoration_runs: &[DecorationRun]) -> TextBackgroundPadding {
    let mut max_padding = TextBackgroundPadding::default();
    for run in decoration_runs {
        if let Some(padding) = run.background_padding {
            max_padding.top = max_padding.top.max(padding.top);
            max_padding.right = max_padding.right.max(padding.right);
            max_padding.bottom = max_padding.bottom.max(padding.bottom);
            max_padding.left = max_padding.left.max(padding.left);
        }
    }
    max_padding
}
