use swash::text::{Codepoint, Script};

/// Unicode script metadata attached to shaped text.
///
/// This deliberately represents the full Unicode Script property instead of encoding one
/// language family (for example CJK) as a boolean. Platform shapers can preserve the script of a
/// glyph/cluster and raster backends can make narrowly-scoped policy decisions from that metadata.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TextScript(Script);

impl TextScript {
    /// Script used when a cluster contains only Common characters such as punctuation or emoji.
    pub const COMMON: Self = Self(Script::Common);
    /// Script used by combining marks whose script is inherited from surrounding text.
    pub const INHERITED: Self = Self(Script::Inherited);
    /// Script used when Unicode does not assign a known script.
    pub const UNKNOWN: Self = Self(Script::Unknown);

    /// Returns the Unicode script name.
    pub fn name(self) -> &'static str {
        self.0.name()
    }

    /// Returns whether this is a concrete Unicode script rather than Common/Inherited/Unknown.
    pub fn is_real(self) -> bool {
        self.0.is_real()
    }

    /// Returns whether the script requires complex shaping.
    pub fn requires_complex_shaping(self) -> bool {
        self.0.is_complex()
    }

    /// Returns whether the script normally uses cursive joining behavior.
    pub fn uses_joined_forms(self) -> bool {
        self.0.is_joined()
    }

    /// Some dense square-script families benefit from a stable vertical raster frame so glyph
    /// texture bounds do not visibly jump between adjacent glyphs. This is a raster policy, not a
    /// substitute for script detection: all scripts are still represented by `TextScript`.
    pub(crate) fn uses_stable_vertical_raster_frame(self) -> bool {
        matches!(
            self.0,
            Script::Bopomofo
                | Script::Hangul
                | Script::Han
                | Script::Hiragana
                | Script::Katakana
                | Script::KhitanSmallScript
                | Script::Nushu
                | Script::Tangut
                | Script::Yi
        )
    }
}

impl Default for TextScript {
    fn default() -> Self {
        Self::COMMON
    }
}

impl From<char> for TextScript {
    fn from(character: char) -> Self {
        Self(character.script())
    }
}

/// Resolve the script for one shaped cluster or other short text span.
///
/// Common/Inherited/Unknown code points are skipped while a concrete script is available in the
/// span. If the span contains no concrete script, preserve Inherited/Common rather than pretending
/// that it belongs to Latin or another arbitrary fallback script.
pub(crate) fn text_script(text: &str) -> TextScript {
    let mut fallback = TextScript::UNKNOWN;
    for character in text.chars() {
        let script = TextScript::from(character);
        if script.is_real() {
            return script;
        }
        if fallback == TextScript::UNKNOWN || script == TextScript::INHERITED {
            fallback = script;
        }
    }

    if fallback == TextScript::UNKNOWN && !text.is_empty() {
        TextScript::COMMON
    } else {
        fallback
    }
}

/// Resolve the script of a DirectWrite-style UTF-16 cluster without allocating a UTF-16 copy.
pub(crate) fn text_script_for_utf16_cluster(
    text: &str,
    utf8_start: usize,
    utf16_len: usize,
) -> TextScript {
    let Some(rest) = text.get(utf8_start..) else {
        return TextScript::UNKNOWN;
    };
    if utf16_len == 0 {
        return TextScript::UNKNOWN;
    }

    let mut consumed_utf16 = 0usize;
    let mut end_utf8 = 0usize;
    for (byte_index, character) in rest.char_indices() {
        if consumed_utf16 >= utf16_len {
            break;
        }
        let next_utf16 = consumed_utf16.saturating_add(character.len_utf16());
        if next_utf16 > utf16_len {
            break;
        }
        consumed_utf16 = next_utf16;
        end_utf8 = byte_index + character.len_utf8();
    }

    text_script(rest.get(..end_utf8).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::{TextScript, text_script, text_script_for_utf16_cluster};

    #[test]
    fn classifies_unicode_scripts_without_regional_special_cases() {
        for (text, expected) in [
            ("hello", "Latin"),
            ("Привет", "Cyrillic"),
            ("Ελληνικά", "Greek"),
            ("العربية", "Arabic"),
            ("עברית", "Hebrew"),
            ("हिन्दी", "Devanagari"),
            ("ไทย", "Thai"),
            ("မြန်မာ", "Myanmar"),
            ("中文", "Han"),
            ("日本語", "Han"),
            ("한국어", "Hangul"),
        ] {
            assert_eq!(text_script(text).name(), expected, "text={text:?}");
        }
    }

    #[test]
    fn preserves_common_and_inherited_when_no_concrete_script_exists() {
        assert_eq!(text_script("😀"), TextScript::COMMON);
        assert_eq!(text_script("\u{0301}"), TextScript::INHERITED);
        assert_eq!(text_script(""), TextScript::UNKNOWN);
    }

    #[test]
    fn resolves_utf16_clusters_without_splitting_surrogate_pairs() {
        let text = "A😀العربية";
        let emoji = text.find('😀').unwrap();
        let arabic = text.find('ا').unwrap();
        assert_eq!(text_script_for_utf16_cluster(text, 0, 1).name(), "Latin");
        assert_eq!(
            text_script_for_utf16_cluster(text, emoji, 2),
            TextScript::COMMON
        );
        assert_eq!(
            text_script_for_utf16_cluster(text, arabic, 1).name(),
            "Arabic"
        );
    }

    #[test]
    fn exposes_shaping_properties_for_all_scripts() {
        let arabic = text_script("العربية");
        let devanagari = text_script("हिन्दी");
        let latin = text_script("Latin");
        assert!(arabic.requires_complex_shaping());
        assert!(arabic.uses_joined_forms());
        assert!(devanagari.requires_complex_shaping());
        assert!(!latin.uses_joined_forms());
    }
}
