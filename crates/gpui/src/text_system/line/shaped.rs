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