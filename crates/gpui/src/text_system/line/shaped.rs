use super::{DecorationRun, background::paint_line_background, paint::paint_line};
use crate::{App, LineLayout, Pixels, Point, Result, SharedString, TextAlign, Window, point, px};
use derive_more::{Deref, DerefMut};
use smallvec::SmallVec;
use std::sync::Arc;

/// A shaped and decorated line of text.
#[derive(Clone, Default, Debug, Deref, DerefMut)]
pub struct ShapedLine {
    #[deref]
    #[deref_mut]
    pub(crate) layout: Arc<LineLayout>,
    /// Original text.
    pub text: SharedString,
    pub(crate) decoration_runs: SmallVec<[DecorationRun; 32]>,
}

impl ShapedLine {
    /// Returns a forward-only cursor for incrementally splitting this shaped line.
    ///
    /// Byte-ordered glyphs are traversed in linear time. Visually reordered glyphs fall back to
    /// the general split path so bidirectional shaping order remains correct.
    pub fn cursor(&self) -> ShapedLineCursor<'_> {
        assert_eq!(
            self.len(),
            self.text.len(),
            "cannot split a shaped line with an adjusted length"
        );
        let byte_ordered = self
            .layout
            .runs
            .iter()
            .flat_map(|run| run.glyphs.iter().map(|glyph| glyph.index))
            .is_sorted();
        ShapedLineCursor {
            line: self,
            unordered_remainder: (!byte_ordered).then(|| self.clone()),
            byte_index: 0,
            run_index: 0,
            glyph_index: 0,
            decoration_index: 0,
            decoration_offset: 0,
            x_offset: px(0.0),
        }
    }

    /// Return the UTF-8 byte length.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.layout.len
    }

    /// Returns the shaped advance width.
    pub fn width(&self) -> Pixels {
        self.layout.width
    }

    /// Splits this shaped line at a UTF-8 byte boundary.
    ///
    /// Decorations are partitioned at the same boundary, while suffix glyph positions and byte
    /// indices are rebased to zero.
    pub fn split_at(&self, byte_index: usize) -> (ShapedLine, ShapedLine) {
        assert_eq!(
            self.len(),
            self.text.len(),
            "cannot split a shaped line with an adjusted length"
        );
        assert!(
            self.text.is_char_boundary(byte_index),
            "split boundary is not a UTF-8 character boundary"
        );

        let (left_layout, right_layout) = self.layout.split_at(byte_index);
        let split_point = byte_index as u32;
        let mut left_decorations = SmallVec::new();
        let mut right_decorations = SmallVec::new();
        let mut decoration_offset = 0u32;

        for decoration in &self.decoration_runs {
            let run_end = decoration_offset.saturating_add(decoration.len);
            if run_end <= split_point {
                left_decorations.push(decoration.clone());
            } else if decoration_offset >= split_point {
                right_decorations.push(decoration.clone());
            } else {
                let mut left = decoration.clone();
                left.len = split_point - decoration_offset;
                let mut right = decoration.clone();
                right.len = run_end - split_point;
                left_decorations.push(left);
                right_decorations.push(right);
            }
            decoration_offset = run_end;
        }

        let left_text = if byte_index == self.text.len() {
            self.text.clone()
        } else {
            SharedString::new(&self.text[..byte_index])
        };
        let right_text = if byte_index == 0 {
            self.text.clone()
        } else {
            SharedString::new(&self.text[byte_index..])
        };

        (
            ShapedLine {
                layout: Arc::new(left_layout),
                text: left_text,
                decoration_runs: left_decorations,
            },
            ShapedLine {
                layout: Arc::new(right_layout),
                text: right_text,
                decoration_runs: right_decorations,
            },
        )
    }
    /// Override the rendered byte length.
    pub fn with_len(mut self, len: usize) -> Self {
        let layout = self.layout.as_ref();
        self.layout = Arc::new(LineLayout {
            font_size: layout.font_size,
            width: layout.width,
            ascent: layout.ascent,
            descent: layout.descent,
            runs: layout.runs.clone(),
            len,
        });
        self
    }

    /// Paint the line.
    pub fn paint(
        &self,
        origin: crate::Point<Pixels>,
        line_height: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<()> {
        paint_line(
            origin,
            &self.layout,
            line_height,
            TextAlign::default(),
            None,
            &self.decoration_runs,
            &[],
            window,
            cx,
        )
    }

    /// Paint the line background.
    pub fn paint_background(
        &self,
        origin: crate::Point<Pixels>,
        line_height: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<()> {
        paint_line_background(
            origin,
            &self.layout,
            line_height,
            TextAlign::default(),
            None,
            &self.decoration_runs,
            &[],
            window,
            cx,
        )
    }
}

impl LineLayout {
    /// Paints this layout using the supplied decoration runs.
    ///
    /// This lower-level path avoids rebuilding a `ShapedLine` when callers already cache shaping
    /// separately from color/underline decoration state.
    pub fn paint(
        &self,
        origin: Point<Pixels>,
        line_height: Pixels,
        align: TextAlign,
        align_width: Option<Pixels>,
        decoration_runs: &[DecorationRun],
        window: &mut Window,
        cx: &mut App,
    ) -> Result<()> {
        paint_line(
            origin,
            self,
            line_height,
            align,
            align_width,
            decoration_runs,
            &[],
            window,
            cx,
        )
    }

    /// Paints only the decorated text backgrounds for this layout.
    ///
    /// Like [`LineLayout::paint`], this is intended for callers that retain the shaped layout and
    /// update decoration state independently.
    pub fn paint_background(
        &self,
        origin: Point<Pixels>,
        line_height: Pixels,
        align: TextAlign,
        align_width: Option<Pixels>,
        decoration_runs: &[DecorationRun],
        window: &mut Window,
        cx: &mut App,
    ) -> Result<()> {
        paint_line_background(
            origin,
            self,
            line_height,
            align,
            align_width,
            decoration_runs,
            &[],
            window,
            cx,
        )
    }
}

/// Incrementally splits a ShapedLine at increasing UTF-8 byte boundaries.
///
/// For byte-ordered text, every glyph is visited at most once across successive calls. Visually
/// reordered text preserves correctness by delegating each chunk to ShapedLine::split_at.
pub struct ShapedLineCursor<'a> {
    line: &'a ShapedLine,
    unordered_remainder: Option<ShapedLine>,
    byte_index: usize,
    run_index: usize,
    glyph_index: usize,
    decoration_index: usize,
    decoration_offset: u32,
    x_offset: Pixels,
}

impl ShapedLineCursor<'_> {
    /// Takes the shaped bytes since the previous boundary.
    ///
    /// Panics if the boundary moves backwards, exceeds the line, or falls inside a UTF-8 scalar.
    pub fn take_until(&mut self, byte_index: usize) -> ShapedLine {
        assert!(byte_index >= self.byte_index, "split boundary moved backwards");
        assert!(byte_index <= self.line.len(), "split boundary exceeds line length");
        assert!(
            self.line.text.is_char_boundary(byte_index),
            "split boundary is not a UTF-8 character boundary"
        );

        let previous_index = self.byte_index;
        let previous_x = self.x_offset;
        if let Some(remainder) = &mut self.unordered_remainder {
            let (piece, rest) = remainder.split_at(byte_index - previous_index);
            *remainder = rest;
            self.byte_index = byte_index;
            self.x_offset = self.line.layout.x_for_index(byte_index);
            return piece;
        }

        let mut runs = Vec::new();
        let mut next_x = self.line.layout.width;
        while let Some(run) = self.line.layout.runs.get(self.run_index) {
            let start = self.glyph_index;
            while let Some(glyph) = run.glyphs.get(self.glyph_index) {
                if glyph.index >= byte_index { break; }
                self.glyph_index += 1;
            }
            let end = self.glyph_index;
            if start < end {
                runs.push(crate::ShapedRun {
                    font_id: run.font_id,
                    glyphs: run.glyphs[start..end]
                        .iter()
                        .map(|glyph| {
                            let mut glyph = glyph.clone();
                            glyph.position = point(glyph.position.x - previous_x, glyph.position.y);
                            glyph.index -= previous_index;
                            glyph
                        })
                        .collect(),
                });
            }
            if let Some(glyph) = run.glyphs.get(self.glyph_index) {
                next_x = glyph.position.x;
                break;
            }
            self.run_index += 1;
            self.glyph_index = 0;
        }

        let mut decorations = SmallVec::new();
        while let Some(decoration) = self.line.decoration_runs.get(self.decoration_index)
            && (self.decoration_offset < byte_index as u32
                || (decoration.len == 0 && self.decoration_offset == byte_index as u32))
        {
            let end = self.decoration_offset.saturating_add(decoration.len);
            let start = self.decoration_offset.max(previous_index as u32);
            let len = end.min(byte_index as u32).saturating_sub(start);
            if len > 0 || decoration.len == 0 {
                let mut chunk = decoration.clone();
                chunk.len = len;
                decorations.push(chunk);
            }
            if end <= byte_index as u32 {
                self.decoration_index += 1;
                self.decoration_offset = end;
            } else {
                break;
            }
        }

        self.byte_index = byte_index;
        self.x_offset = next_x;
        ShapedLine {
            layout: Arc::new(LineLayout {
                font_size: self.line.layout.font_size,
                width: next_x - previous_x,
                ascent: self.line.layout.ascent,
                descent: self.line.layout.descent,
                runs,
                len: byte_index - previous_index,
            }),
            text: SharedString::new(&self.line.text[previous_index..byte_index]),
            decoration_runs: decorations,
        }
    }

    /// Returns the original line's x coordinate at the current split boundary.
    pub fn x_offset(&self) -> Pixels {
        self.x_offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FontId, GlyphId, Hsla, ShapedGlyph, ShapedRun, black};

    /// Helper: build a ShapedLine from glyph descriptors without the platform text system.
    /// Each glyph is described as (byte_index, x_position).
    fn make_shaped_line(
        text: &str,
        glyphs: &[(usize, f32)],
        width: f32,
        decorations: &[DecorationRun],
    ) -> ShapedLine {
        let shaped_glyphs: Vec<ShapedGlyph> = glyphs
            .iter()
            .map(|&(index, x)| ShapedGlyph {
                id: GlyphId(0),
                position: point(px(x), px(0.0)),
                index,
                is_emoji: false,
            })
            .collect();

        ShapedLine {
            layout: Arc::new(LineLayout {
                font_size: px(16.0),
                width: px(width),
                ascent: px(12.0),
                descent: px(4.0),
                runs: vec![ShapedRun {
                    font_id: FontId(0),
                    glyphs: shaped_glyphs,
                }],
                len: text.len(),
            }),
            text: SharedString::new(text),
            decoration_runs: SmallVec::from(decorations.to_vec()),
        }
    }

    #[test]
    fn test_split_at_invariants() {
        // Split "abcdef" at every possible byte index and verify structural invariants.
        let line = make_shaped_line(
            "abcdef",
            &[
                (0, 0.0),
                (1, 10.0),
                (2, 20.0),
                (3, 30.0),
                (4, 40.0),
                (5, 50.0),
            ],
            60.0,
            &[],
        );

        for i in 0..=6 {
            let (left, right) = line.split_at(i);

            assert_eq!(
                left.width() + right.width(),
                line.width(),
                "widths must sum at split={i}"
            );
            assert_eq!(
                left.len() + right.len(),
                line.len(),
                "lengths must sum at split={i}"
            );
            assert_eq!(
                format!("{}{}", left.text.as_ref(), right.text.as_ref()),
                "abcdef",
                "text must concatenate at split={i}"
            );
            assert_eq!(left.font_size, line.font_size, "font_size at split={i}");
            assert_eq!(right.ascent, line.ascent, "ascent at split={i}");
            assert_eq!(right.descent, line.descent, "descent at split={i}");
        }

        // Edge: split at 0 produces no left runs, full content on right
        let (left, right) = line.split_at(0);
        assert_eq!(left.runs.len(), 0);
        assert_eq!(right.runs[0].glyphs.len(), 6);

        // Edge: split at end produces full content on left, no right runs
        let (left, right) = line.split_at(6);
        assert_eq!(left.runs[0].glyphs.len(), 6);
        assert_eq!(right.runs.len(), 0);
    }

    #[test]
    fn test_split_at_glyph_rebasing() {
        // Two font runs (simulating a font fallback boundary at byte 3):
        //   run A (FontId 0): glyphs at bytes 0,1,2  positions 0,10,20
        //   run B (FontId 1): glyphs at bytes 3,4,5  positions 30,40,50
        // Successive splits simulate the incremental splitting done during wrap.
        let line = ShapedLine {
            layout: Arc::new(LineLayout {
                font_size: px(16.0),
                width: px(60.0),
                ascent: px(12.0),
                descent: px(4.0),
                runs: vec![
                    ShapedRun {
                        font_id: FontId(0),
                        glyphs: vec![
                            ShapedGlyph {
                                id: GlyphId(0),
                                position: point(px(0.0), px(0.0)),
                                index: 0,
                                is_emoji: false,
                            },
                            ShapedGlyph {
                                id: GlyphId(0),
                                position: point(px(10.0), px(0.0)),
                                index: 1,
                                is_emoji: false,
                            },
                            ShapedGlyph {
                                id: GlyphId(0),
                                position: point(px(20.0), px(0.0)),
                                index: 2,
                                is_emoji: false,
                            },
                        ],
                    },
                    ShapedRun {
                        font_id: FontId(1),
                        glyphs: vec![
                            ShapedGlyph {
                                id: GlyphId(0),
                                position: point(px(30.0), px(0.0)),
                                index: 3,
                                is_emoji: false,
                            },
                            ShapedGlyph {
                                id: GlyphId(0),
                                position: point(px(40.0), px(0.0)),
                                index: 4,
                                is_emoji: false,
                            },
                            ShapedGlyph {
                                id: GlyphId(0),
                                position: point(px(50.0), px(0.0)),
                                index: 5,
                                is_emoji: false,
                            },
                        ],
                    },
                ],
                len: 6,
            }),
            text: "abcdef".into(),
            decoration_runs: SmallVec::new(),
        };

        // First split at byte 2 — mid-run in run A
        let (first, remainder) = line.split_at(2);
        assert_eq!(first.text.as_ref(), "ab");
        assert_eq!(first.runs.len(), 1);
        assert_eq!(first.runs[0].font_id, FontId(0));

        // Remainder "cdef" should have two runs: tail of A (1 glyph) + all of B (3 glyphs)
        assert_eq!(remainder.text.as_ref(), "cdef");
        assert_eq!(remainder.runs.len(), 2);
        assert_eq!(remainder.runs[0].font_id, FontId(0));
        assert_eq!(remainder.runs[0].glyphs.len(), 1);
        assert_eq!(remainder.runs[0].glyphs[0].index, 0);
        assert_eq!(remainder.runs[0].glyphs[0].position.x, px(0.0));
        assert_eq!(remainder.runs[1].font_id, FontId(1));
        assert_eq!(remainder.runs[1].glyphs[0].index, 1);
        assert_eq!(remainder.runs[1].glyphs[0].position.x, px(10.0));

        // Second split at byte 2 within remainder — crosses the run boundary
        let (second, final_part) = remainder.split_at(2);
        assert_eq!(second.text.as_ref(), "cd");
        assert_eq!(final_part.text.as_ref(), "ef");
        assert_eq!(final_part.runs[0].glyphs[0].index, 0);
        assert_eq!(final_part.runs[0].glyphs[0].position.x, px(0.0));

        // Widths must sum across all three pieces
        assert_eq!(
            first.width() + second.width() + final_part.width(),
            line.width()
        );
    }

    #[test]
    fn test_split_at_decorations() {
        // Three decoration runs: red [0..2), green [2..5), blue [5..6).
        // Split at byte 3 — red goes entirely left, green straddles, blue goes entirely right.
        let red = Hsla {
            h: 0.0,
            s: 1.0,
            l: 0.5,
            a: 1.0,
        };
        let green = Hsla {
            h: 0.3,
            s: 1.0,
            l: 0.5,
            a: 1.0,
        };
        let blue = Hsla {
            h: 0.6,
            s: 1.0,
            l: 0.5,
            a: 1.0,
        };

        let line = make_shaped_line(
            "abcdef",
            &[
                (0, 0.0),
                (1, 10.0),
                (2, 20.0),
                (3, 30.0),
                (4, 40.0),
                (5, 50.0),
            ],
            60.0,
            &[
                DecorationRun {
                    len: 2,
                    color: red,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                },
                DecorationRun {
                    len: 3,
                    color: green,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                },
                DecorationRun {
                    len: 1,
                    color: blue,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                },
            ],
        );

        let (left, right) = line.split_at(3);

        // Left: red(2) + green(1) — green straddled, left portion has len 1
        assert_eq!(left.decoration_runs.len(), 2);
        assert_eq!(left.decoration_runs[0].len, 2);
        assert_eq!(left.decoration_runs[0].color, red);
        assert_eq!(left.decoration_runs[1].len, 1);
        assert_eq!(left.decoration_runs[1].color, green);

        // Right: green(2) + blue(1) — green straddled, right portion has len 2
        assert_eq!(right.decoration_runs.len(), 2);
        assert_eq!(right.decoration_runs[0].len, 2);
        assert_eq!(right.decoration_runs[0].color, green);
        assert_eq!(right.decoration_runs[1].len, 1);
        assert_eq!(right.decoration_runs[1].color, blue);
    }

    #[test]
    fn test_cursor_preserves_shaping_metadata_across_runs() {
        let line = ShapedLine {
            layout: Arc::new(LineLayout {
                font_size: px(16.0),
                width: px(50.0),
                ascent: px(12.0),
                descent: px(4.0),
                runs: vec![
                    ShapedRun {
                        font_id: FontId(3),
                        glyphs: vec![
                            ShapedGlyph {
                                id: GlyphId(11),
                                position: point(px(0.0), px(1.0)),
                                index: 0,
                                is_emoji: true,
                            },
                            ShapedGlyph {
                                id: GlyphId(12),
                                position: point(px(17.0), px(1.0)),
                                index: 1,
                                is_emoji: false,
                            },
                            ShapedGlyph {
                                id: GlyphId(13),
                                position: point(px(19.0), px(-1.0)),
                                index: 1,
                                is_emoji: false,
                            },
                        ],
                    },
                    ShapedRun {
                        font_id: FontId(8),
                        glyphs: vec![
                            ShapedGlyph {
                                id: GlyphId(21),
                                position: point(px(25.0), px(1.0)),
                                index: 5,
                                is_emoji: true,
                            },
                            ShapedGlyph {
                                id: GlyphId(22),
                                position: point(px(41.0), px(1.0)),
                                index: 7,
                                is_emoji: false,
                            },
                        ],
                    },
                ],
                len: 10,
            }),
            text: "a😀bcdef".into(),
            decoration_runs: SmallVec::new(),
        };
        let mut cursor = line.cursor();
        let first = cursor.take_until(5);
        assert_eq!(first.text.as_ref(), "a😀");
        assert_eq!(first.runs[0].font_id, FontId(3));
        assert_eq!(first.runs[0].glyphs[0].id, GlyphId(11));
        assert!(first.runs[0].glyphs[0].is_emoji);
        assert_eq!(first.runs[0].glyphs[1].index, 1);
        assert_eq!(first.runs[0].glyphs[1].position, point(px(17.0), px(1.0)));
        assert_eq!(first.runs[0].glyphs.len(), 3);
        assert_eq!(first.runs[0].glyphs[2].index, 1);
        assert_eq!(first.runs[0].glyphs[2].position, point(px(19.0), px(-1.0)));
        assert_eq!(cursor.x_offset(), px(25.0));

        let second = cursor.take_until(7);
        assert_eq!(second.text.as_ref(), "bc");
        assert_eq!(second.runs[0].font_id, FontId(8));
        assert_eq!(second.runs[0].glyphs[0].id, GlyphId(21));
        assert_eq!(second.runs[0].glyphs[0].index, 0);
        assert_eq!(second.runs[0].glyphs[0].position, point(px(0.0), px(1.0)));
        assert_eq!(cursor.x_offset(), px(41.0));

        let final_part = cursor.take_until(10);
        assert_eq!(final_part.text.as_ref(), "def");
        assert_eq!(final_part.runs[0].font_id, FontId(8));
        assert_eq!(final_part.runs[0].glyphs[0].id, GlyphId(22));
        assert_eq!(final_part.runs[0].glyphs[0].index, 0);
        assert_eq!(
            final_part.runs[0].glyphs[0].position,
            point(px(0.0), px(1.0))
        );
    }

    #[test]
    fn test_cursor_preserves_existing_visual_order_splitting() {
        let line = make_shaped_line("abc", &[(0, 0.0), (2, 10.0), (1, 20.0)], 30.0, &[]);
        let mut cursor = line.cursor();
        let mut remainder = line.clone();
        let mut previous_boundary = 0;
        for boundary in [0, 1, 2, 3] {
            let (expected, rest) = remainder.split_at(boundary - previous_boundary);
            let actual = cursor.take_until(boundary);
            assert_eq!(actual.text, expected.text);
            assert_eq!(actual.width(), expected.width());
            assert_eq!(actual.runs.len(), expected.runs.len());
            for (actual, expected) in actual.runs.iter().zip(&expected.runs) {
                assert_eq!(actual.font_id, expected.font_id);
                assert_eq!(actual.glyphs.len(), expected.glyphs.len());
                for (actual, expected) in actual.glyphs.iter().zip(&expected.glyphs) {
                    assert_eq!(actual.id, expected.id);
                    assert_eq!(actual.index, expected.index);
                    assert_eq!(actual.position, expected.position);
                }
            }
            assert_eq!(cursor.x_offset(), line.x_for_index(boundary));
            remainder = rest;
            previous_boundary = boundary;
        }
    }

    #[test]
    fn test_cursor_partitions_one_decoration_across_three_chunks() {
        let line = make_shaped_line(
            "abcdef",
            &[
                (0, 0.0),
                (1, 10.0),
                (2, 20.0),
                (3, 30.0),
                (4, 40.0),
                (5, 50.0),
            ],
            60.0,
            &[DecorationRun {
                len: 6,
                color: Hsla {
                    h: 0.2,
                    s: 0.4,
                    l: 0.6,
                    a: 1.0,
                },
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
        );
        let mut cursor = line.cursor();
        assert_eq!(cursor.take_until(2).decoration_runs[0].len, 2);
        assert_eq!(cursor.take_until(4).decoration_runs[0].len, 2);
        assert_eq!(cursor.take_until(6).decoration_runs[0].len, 2);
    }

    #[test]
    fn test_cursor_matches_successive_splits_at_ordered_boundaries() {
        let decorations: Vec<_> = [2, 0, 3, 1]
            .into_iter()
            .map(|len| DecorationRun {
                len,
                color: Hsla {
                    h: len as f32 / 10.0,
                    s: 0.5,
                    l: 0.5,
                    a: 1.0,
                },
                background_color: Some(black()),
                underline: None,
                strikethrough: None,
            })
            .collect();
        let line = make_shaped_line(
            "abcdef",
            &[(0, 5.0), (0, 5.0), (2, 15.0), (4, 25.0), (5, 35.0)],
            45.0,
            &decorations,
        );
        for first in 0..=line.len() {
            for second in first..=line.len() {
                let mut cursor = line.cursor();
                let mut remainder = line.clone();
                let mut previous_boundary = 0;
                let mut total_width = px(0.0);
                let mut text = String::new();
                for boundary in [first, second, line.len(), line.len()] {
                    let (expected, rest) = remainder.split_at(boundary - previous_boundary);
                    let actual = cursor.take_until(boundary);
                    assert_eq!(actual.text, expected.text);
                    assert_eq!(actual.len(), expected.len());
                    assert_eq!(actual.width(), expected.width());
                    assert_eq!(actual.runs.len(), expected.runs.len());
                    for (actual, expected) in actual.runs.iter().zip(&expected.runs) {
                        assert_eq!(actual.font_id, expected.font_id);
                        assert_eq!(actual.glyphs.len(), expected.glyphs.len());
                        for (actual, expected) in actual.glyphs.iter().zip(&expected.glyphs) {
                            assert_eq!(actual.id, expected.id);
                            assert_eq!(actual.index, expected.index);
                            assert_eq!(actual.position, expected.position);
                        }
                    }
                    assert_eq!(actual.decoration_runs.len(), expected.decoration_runs.len());
                    for (actual, expected) in
                        actual.decoration_runs.iter().zip(&expected.decoration_runs)
                    {
                        assert_eq!(actual.len, expected.len);
                        assert_eq!(actual.color, expected.color);
                        assert_eq!(actual.background_color, expected.background_color);
                    }
                    total_width += actual.width();
                    text.push_str(&actual.text);
                    remainder = rest;
                    previous_boundary = boundary;
                }
                assert_eq!(total_width, line.width());
                assert_eq!(text, line.text.as_ref());
            }
        }
    }

    #[test]
    fn test_cursor_empty_chunks_and_repeated_boundaries() {
        let line = make_shaped_line("ab", &[(0, 5.0), (1, 15.0)], 20.0, &[]);
        let mut cursor = line.cursor();
        assert_eq!(cursor.take_until(0).text.as_ref(), "");
        assert_eq!(cursor.take_until(0).text.as_ref(), "");
        assert_eq!(cursor.take_until(1).text.as_ref(), "a");
        assert_eq!(cursor.take_until(2).text.as_ref(), "b");
        assert_eq!(cursor.take_until(2).text.as_ref(), "");
        let empty = make_shaped_line("", &[], 0.0, &[]);
        let piece = empty.cursor().take_until(0);
        assert!(piece.text.is_empty());
        assert!(piece.runs.is_empty());
        assert_eq!(piece.width(), px(0.0));
    }

    #[test]
    fn test_cursor_rejects_invalid_boundaries() {
        let line = make_shaped_line("é", &[(0, 0.0)], 10.0, &[]);
        let mut cursor = line.cursor();
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                cursor.take_until(1);
            }))
            .is_err()
        );
        let mut cursor = line.cursor();
        cursor.take_until(2);
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                cursor.take_until(0);
            }))
            .is_err()
        );
        let mut cursor = line.cursor();
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                cursor.take_until(3);
            }))
            .is_err()
        );
    }
}
