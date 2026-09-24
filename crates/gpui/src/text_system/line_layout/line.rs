use crate::{Pixels, point, px};
use smallvec::SmallVec;

use super::super::LineWrapper;
use super::{ShapedRun, WrapBoundary};

/// A laid out and styled line of text.
#[derive(Default, Debug)]
pub struct LineLayout {
    /// Font size.
    pub font_size: Pixels,
    /// Line width.
    pub width: Pixels,
    /// Line ascent.
    pub ascent: Pixels,
    /// Line descent.
    pub descent: Pixels,
    /// Shaped runs.
    pub runs: Vec<ShapedRun>,
    /// UTF-8 byte length.
    pub len: usize,
}

impl LineLayout {
    /// Return the character index at the given x coordinate.
    pub fn index_for_x(&self, x: Pixels) -> Option<usize> {
        if x >= self.width {
            return None;
        }
        for run in self.runs.iter().rev() {
            for glyph in run.glyphs.iter().rev() {
                if glyph.position.x <= x {
                    return Some(glyph.index);
                }
            }
        }
        Some(0)
    }

    /// Return the closest character boundary to the given x coordinate.
    pub fn closest_index_for_x(&self, x: Pixels) -> usize {
        let mut previous_index = 0;
        let mut previous_x = px(0.);
        for run in &self.runs {
            for glyph in &run.glyphs {
                if glyph.position.x >= x {
                    return if glyph.position.x - x < x - previous_x {
                        glyph.index
                    } else {
                        previous_index
                    };
                }
                previous_index = glyph.index;
                previous_x = glyph.position.x;
            }
        }
        if self.len == 1 && x <= self.width / 2. {
            0
        } else {
            self.len
        }
    }

    /// Return the x coordinate for a character index.
    pub fn x_for_index(&self, index: usize) -> Pixels {
        self.runs
            .iter()
            .flat_map(|run| &run.glyphs)
            .find(|glyph| glyph.index >= index)
            .map_or(self.width, |glyph| glyph.position.x)
    }

    /// Splits this layout at a UTF-8 byte index into prefix and suffix layouts.
    ///
    /// Glyph positions and byte indices in the suffix are rebased to the new origin.
    pub fn split_at(&self, byte_index: usize) -> (LineLayout, LineLayout) {
        assert!(byte_index <= self.len, "split boundary exceeds line length");
        let x_offset = self.x_for_index(byte_index);
        let mut left_runs = Vec::new();
        let mut right_runs = Vec::new();

        for run in &self.runs {
            let split_pos = run.glyphs.partition_point(|glyph| glyph.index < byte_index);
            if split_pos > 0 {
                left_runs.push(ShapedRun {
                    font_id: run.font_id,
                    glyphs: run.glyphs[..split_pos].to_vec(),
                });
            }
            if split_pos < run.glyphs.len() {
                let glyphs = run.glyphs[split_pos..]
                    .iter()
                    .map(|glyph| {
                        let mut glyph = glyph.clone();
                        glyph.position = point(glyph.position.x - x_offset, glyph.position.y);
                        glyph.index -= byte_index;
                        glyph
                    })
                    .collect();
                right_runs.push(ShapedRun { font_id: run.font_id, glyphs });
            }
        }

        (
            LineLayout {
                font_size: self.font_size,
                width: x_offset,
                ascent: self.ascent,
                descent: self.descent,
                runs: left_runs,
                len: byte_index,
            },
            LineLayout {
                font_size: self.font_size,
                width: self.width - x_offset,
                ascent: self.ascent,
                descent: self.descent,
                runs: right_runs,
                len: self.len - byte_index,
            },
        )
    }
    /// Return the font used at a character index.
    pub fn font_id_for_index(&self, index: usize) -> Option<crate::FontId> {
        self.runs.iter().find_map(|run| {
            run.glyphs
                .iter()
                .any(|glyph| glyph.index >= index)
                .then_some(run.font_id)
        })
    }

    pub(super) fn compute_wrap_boundaries(
        &self,
        text: &str,
        wrap_width: Pixels,
        max_lines: Option<usize>,
    ) -> SmallVec<[WrapBoundary; 1]> {
        let mut boundaries = SmallVec::new();
        let mut first_non_whitespace = None;
        let mut last_candidate = None;
        let mut last_candidate_x = px(0.);
        let mut last_boundary = WrapBoundary {
            run_ix: 0,
            glyph_ix: 0,
        };
        let mut last_boundary_x = px(0.);
        let mut previous = '\0';
        let mut glyphs = self
            .runs
            .iter()
            .enumerate()
            .flat_map(|(run_ix, run)| {
                run.glyphs.iter().enumerate().map(move |(glyph_ix, glyph)| {
                    (
                        WrapBoundary { run_ix, glyph_ix },
                        text[glyph.index..].chars().next().unwrap(),
                        glyph.position.x,
                    )
                })
            })
            .peekable();

        while let Some((boundary, character, x)) = glyphs.next() {
            if character == '\n' {
                continue;
            }
            if (LineWrapper::is_word_char(character)
                && previous == ' '
                && first_non_whitespace.is_some())
                || (!LineWrapper::is_word_char(character)
                    && character != ' '
                    && first_non_whitespace.is_some())
            {
                last_candidate = Some(boundary);
                last_candidate_x = x;
            }
            if character != ' ' && first_non_whitespace.is_none() {
                first_non_whitespace = Some(boundary);
            }
            let next_x = glyphs.peek().map_or(self.width, |(_, _, x)| *x);
            if next_x - last_boundary_x > wrap_width && boundary > last_boundary {
                if max_lines.is_some_and(|limit| boundaries.len() >= limit - 1) {
                    break;
                }
                if let Some(candidate) = last_candidate.take() {
                    last_boundary = candidate;
                    last_boundary_x = last_candidate_x;
                } else {
                    last_boundary = boundary;
                    last_boundary_x = x;
                }
                boundaries.push(last_boundary);
            }
            previous = character;
        }
        boundaries
    }
}
