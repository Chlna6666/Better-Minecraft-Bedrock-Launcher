mod fragment;
mod truncate;

#[cfg(test)]
mod tests;

pub use fragment::{Boundary, LineFragment};

use crate::{FontId, FontRun, Pixels, PlatformTextSystem, px};
use collections::HashMap;
use smallvec::SmallVec;
use std::{borrow::Cow, iter, sync::Arc};
use unicode_segmentation::UnicodeSegmentation;

use fragment::WrapBoundaryCandidate;

/// The GPUI line wrapper, used to wrap lines of text to a given width.
pub struct LineWrapper {
    platform_text_system: Arc<dyn PlatformTextSystem>,
    pub(crate) font_id: FontId,
    pub(crate) font_size: Pixels,
    cached_ascii_char_widths: [Option<Pixels>; 128],
    cached_other_char_widths: HashMap<char, Pixels>,
}

impl LineWrapper {
    /// The maximum indent that can be applied to a line.
    pub const MAX_INDENT: u32 = 256;

    pub(crate) fn new(
        font_id: FontId,
        font_size: Pixels,
        text_system: Arc<dyn PlatformTextSystem>,
    ) -> Self {
        Self {
            platform_text_system: text_system,
            font_id,
            font_size,
            cached_ascii_char_widths: [None; 128],
            cached_other_char_widths: HashMap::default(),
        }
    }

    /// Wrap a line of text to the given width with this wrapper's font and font size.
    pub fn wrap_line<'a>(
        &'a mut self,
        fragments: &'a [LineFragment],
        wrap_width: Pixels,
    ) -> impl Iterator<Item = Boundary> + 'a {
        let text = match fragments {
            [LineFragment::Text { text }] => Cow::Borrowed(*text),
            _ => {
                let mut text = String::new();
                for fragment in fragments {
                    match fragment {
                        LineFragment::Text { text: segment } => text.push_str(segment),
                        // Match the byte count of the inline element and keep it
                        // attached to adjacent words, as upstream GPUI does.
                        LineFragment::Element { len_utf8, .. } => {
                            text.extend(iter::repeat_n('a', *len_utf8));
                        }
                    }
                }
                Cow::Owned(text)
            }
        };
        let breaks: SmallVec<[usize; 32]> = Self::break_indices(&text).collect();
        let graphemes: SmallVec<[usize; 64]> = text
            .grapheme_indices(true)
            .map(|(index, _)| index)
            .collect();
        let mut breaks = breaks.into_iter().peekable();
        let mut graphemes = graphemes.into_iter().peekable();
        let mut candidates = fragments
            .iter()
            .flat_map(LineFragment::wrap_boundary_candidates);
        let mut width = px(0.);
        let mut width_before_grapheme = px(0.);
        let mut first_non_whitespace = None;
        let mut indent = None;
        let mut last_candidate = None;
        let mut last_wrap = 0;
        let mut grapheme = 0;
        let mut index = 0;
        iter::from_fn(move || {
            for candidate in candidates.by_ref() {
                let ix = index;
                index += candidate.len_utf8();
                while graphemes.next_if(|position| *position < ix).is_some() {}
                let grapheme_start = graphemes.next_if_eq(&ix).is_some();
                if grapheme_start {
                    grapheme = ix;
                    width_before_grapheme = width;
                }
                while breaks.next_if(|position| *position < ix).is_some() {}
                if breaks.next_if_eq(&ix).is_some()
                    && grapheme_start
                    && first_non_whitespace.is_some()
                {
                    last_candidate = Some((ix, width));
                }
                width += match candidate {
                    WrapBoundaryCandidate::Char { character: '\n' } => continue,
                    WrapBoundaryCandidate::Char { character } => {
                        if character != ' ' {
                            first_non_whitespace.get_or_insert(ix);
                        }
                        self.width_for_char(character)
                    }
                    WrapBoundaryCandidate::Element { width, .. } => {
                        first_non_whitespace.get_or_insert(ix);
                        width
                    }
                };
                if width <= wrap_width || grapheme <= last_wrap {
                    continue;
                }

                if let (None, Some(first)) = (indent, first_non_whitespace) {
                    indent = Some(Self::MAX_INDENT.min(first.saturating_sub(last_wrap) as u32));
                }
                if let Some((position, prefix_width)) = last_candidate
                    .take()
                    .filter(|&(pos, prefix)| pos > last_wrap && width - prefix <= wrap_width)
                {
                    last_wrap = position;
                    width -= prefix_width;
                    width_before_grapheme -= prefix_width;
                } else {
                    last_wrap = grapheme;
                    width -= width_before_grapheme;
                    width_before_grapheme = px(0.);
                }
                let columns = indent.unwrap_or(0);
                let indent_width = self.width_for_char(' ') * columns as f32;
                width += indent_width;
                width_before_grapheme += indent_width;
                return Some(Boundary::new(last_wrap, columns));
            }
            None
        })
    }

    /// Truncate a line of text to the given width with this wrapper's font and font size.
    pub fn truncate_line(
        &mut self,
        line: crate::SharedString,
        truncate_width: Pixels,
        truncation_suffix: &str,
        runs: &mut Vec<crate::TextRun>,
    ) -> crate::SharedString {
        truncate::line(self, line, truncate_width, truncation_suffix, runs)
    }

    /// Shared Unicode line-break opportunities for fragment and shaped text.
    /// Derived from Zed f126950f9d and GPUI Fast fd15f82 (Apache-2.0).
    pub(super) fn break_indices(text: &str) -> impl Iterator<Item = usize> + '_ {
        icu_segmenter::LineSegmenter::new_for_non_complex_scripts(Default::default())
            .segment_str(text)
    }

    #[inline(always)]
    pub(super) fn width_for_char(&mut self, c: char) -> Pixels {
        if (c as u32) < 128 {
            if let Some(cached_width) = self.cached_ascii_char_widths[c as usize] {
                cached_width
            } else {
                let width = self.compute_width_for_char(c);
                self.cached_ascii_char_widths[c as usize] = Some(width);
                width
            }
        } else if let Some(cached_width) = self.cached_other_char_widths.get(&c) {
            *cached_width
        } else {
            let width = self.compute_width_for_char(c);
            self.cached_other_char_widths.insert(c, width);
            width
        }
    }

    fn compute_width_for_char(&self, c: char) -> Pixels {
        let _work = crate::window::element_work::begin_named(
            "text_character_shape",
            crate::window::element_work::ElementWorkPhase::Layout,
        );
        let mut buffer = [0; 4];
        let buffer = c.encode_utf8(&mut buffer);
        self.platform_text_system
            .layout_line(
                buffer,
                self.font_size,
                &[FontRun {
                    len: buffer.len(),
                    font_id: self.font_id,
                }],
            )
            .width
    }
}
