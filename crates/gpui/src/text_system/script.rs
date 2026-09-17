use swash::text::{BidiClass, Codepoint, Script};

/// Unicode script metadata attached to shaped text.
///
/// This deliberately represents the full Unicode Script property instead of encoding one
/// language family (for example CJK) as a boolean. Platform shapers can preserve the script of a
/// glyph/cluster and raster backends can make narrowly-scoped policy decisions from that metadata.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TextScript(Script);

/// Strong visual direction discovered while inspecting a text cluster.
///
/// `Neutral` does not mean that bidi processing can be skipped. Explicit bidi controls, Arabic
/// numbers and isolates can still require the Unicode bidi algorithm without contributing a strong
/// left-to-right or right-to-left character themselves.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum TextDirection {
    /// The cluster contains a strong left-to-right code point or explicit LTR control.
    LeftToRight,
    /// The cluster contains a strong right-to-left code point or explicit RTL control.
    RightToLeft,
    /// No strong direction was found in the cluster.
    #[default]
    Neutral,
}

/// Unicode properties resolved for one shaping cluster or short text span.
///
/// This is deliberately script-neutral: Arabic, Hebrew, Indic, Southeast Asian, CJK, Latin and
/// every other Unicode script flow through the same metadata path. Platform backends may use only
/// the properties that affect their shaping/raster implementation.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct TextClusterProperties {
    /// Concrete Unicode script when one is present, otherwise Common/Inherited/Unknown.
    pub script: TextScript,
    /// First strong directional class observed in the cluster.
    pub direction: TextDirection,
    /// Whether any code point in the cluster requires Unicode bidi resolution.
    pub requires_bidi_resolution: bool,
    /// Whether the cluster contains a Unicode emoji code point.
    pub contains_emoji: bool,
    /// Whether the cluster contains an extended-pictographic code point.
    pub contains_extended_pictographic: bool,
}

impl TextClusterProperties {
    /// Whether the resolved script uses the complex shaping machinery.
    pub fn requires_complex_shaping(self) -> bool {
        self.script.requires_complex_shaping()
    }

    /// Whether the resolved script normally uses cursive joining behavior.
    pub fn uses_joined_forms(self) -> bool {
        self.script.uses_joined_forms()
    }

    /// Whether this cluster is script-neutral content such as ASCII, whitespace or controls.
    pub fn is_neutral_content(self) -> bool {
        !self.script.is_real()
            && self.direction == TextDirection::Neutral
            && !self.requires_bidi_resolution
            && !self.contains_emoji
            && !self.contains_extended_pictographic
    }

    /// Whether shaping/raster should probe font coverage for this cluster instead of assuming that
    /// the primary face can draw it. This is a Unicode-wide policy, not a CJK-only trigger.
    pub fn needs_font_coverage_probe(self) -> bool {
        self.script.is_real()
            || self.requires_bidi_resolution
            || self.contains_emoji
            || self.contains_extended_pictographic
    }

    /// Raster policy for dense square-script families. This is intentionally separate from script
    /// identity so the glyph/raster cache does not equate "international text" with one region.
    pub(crate) fn uses_stable_vertical_raster_frame(self) -> bool {
        self.script.uses_stable_vertical_raster_frame()
    }
}

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
    text_cluster_properties(text).script
}

/// Resolve script, bidi and shaping-relevant Unicode properties for one cluster or short span.
pub(crate) fn text_cluster_properties(text: &str) -> TextClusterProperties {
    let mut properties = TextClusterProperties {
        script: TextScript::UNKNOWN,
        ..Default::default()
    };
    let mut fallback_script = TextScript::UNKNOWN;

    for character in text.chars() {
        let unicode = character.properties();
        let script = TextScript(unicode.script());
        if !properties.script.is_real() && script.is_real() {
            properties.script = script;
        } else if !properties.script.is_real()
            && (fallback_script == TextScript::UNKNOWN || script == TextScript::INHERITED)
        {
            fallback_script = script;
        }

        let bidi = unicode.bidi_class();
        properties.requires_bidi_resolution |= bidi.needs_resolution();
        if properties.direction == TextDirection::Neutral {
            properties.direction = strong_direction(bidi);
        }
        properties.contains_emoji |= unicode.is_emoji();
        properties.contains_extended_pictographic |= unicode.is_extended_pictographic();
    }

    if !properties.script.is_real() {
        properties.script = if fallback_script != TextScript::UNKNOWN {
            fallback_script
        } else if text.is_empty() {
            TextScript::UNKNOWN
        } else {
            TextScript::COMMON
        };
    }
    properties
}

/// Resolve script, bidi and shaping-relevant Unicode properties for a single scalar value without
/// making each platform backend hand-roll a temporary UTF-8 buffer.
pub(crate) fn text_cluster_properties_for_char(character: char) -> TextClusterProperties {
    let mut buffer = [0; 4];
    text_cluster_properties(character.encode_utf8(&mut buffer))
}

/// Returns whether a text span should be checked against the active font face for coverage.
///
/// ASCII remains on the fast path so ordinary UI labels do not force system font loading. Every
/// non-ASCII script, bidi control, emoji and pictographic cluster can trigger a targeted coverage
/// probe if the selected face does not contain the required glyphs.
pub(crate) fn text_needs_font_coverage_probe(text: &str) -> bool {
    text.chars().any(|character| {
        !character.is_ascii() && text_cluster_properties_for_char(character).needs_font_coverage_probe()
    })
}

/// Returns whether a text span contains a non-ASCII cluster that the current face cannot cover.
///
/// The callback should query the candidate face's character map. Keeping this logic in the Unicode
/// metadata module prevents platform backends from hard-coding CJK blocks as the only fallback path.
pub(crate) fn text_contains_missing_font_coverage(
    text: &str,
    mut covers: impl FnMut(char) -> bool,
) -> bool {
    text.chars().any(|character| {
        !character.is_ascii()
            && text_cluster_properties_for_char(character).needs_font_coverage_probe()
            && !covers(character)
    })
}

fn strong_direction(bidi: BidiClass) -> TextDirection {
    match bidi {
        BidiClass::L | BidiClass::LRE | BidiClass::LRI | BidiClass::LRO => {
            TextDirection::LeftToRight
        }
        BidiClass::AL | BidiClass::R | BidiClass::RLE | BidiClass::RLI | BidiClass::RLO => {
            TextDirection::RightToLeft
        }
        _ => TextDirection::Neutral,
    }
}

/// Resolve the script of a DirectWrite-style UTF-16 cluster without allocating a UTF-16 copy.
pub(crate) fn text_script_for_utf16_cluster(
    text: &str,
    utf8_start: usize,
    utf16_len: usize,
) -> TextScript {
    text_cluster_properties_for_utf16_cluster(text, utf8_start, utf16_len).script
}

/// Resolve Unicode properties for a DirectWrite-style UTF-16 cluster without allocating a UTF-16
/// copy or splitting a surrogate pair.
pub(crate) fn text_cluster_properties_for_utf16_cluster(
    text: &str,
    utf8_start: usize,
    utf16_len: usize,
) -> TextClusterProperties {
    let Some(rest) = text.get(utf8_start..) else {
        return TextClusterProperties {
            script: TextScript::UNKNOWN,
            ..Default::default()
        };
    };
    if utf16_len == 0 {
        return TextClusterProperties {
            script: TextScript::UNKNOWN,
            ..Default::default()
        };
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

    text_cluster_properties(rest.get(..end_utf8).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::{
        TextDirection, TextScript, text_cluster_properties,
        text_cluster_properties_for_utf16_cluster, text_contains_missing_font_coverage,
        text_needs_font_coverage_probe, text_script, text_script_for_utf16_cluster,
    };

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
    fn classifies_direction_bidi_and_shaping_independently_of_script_region() {
        let latin = text_cluster_properties("hello");
        assert_eq!(latin.direction, TextDirection::LeftToRight);
        assert!(!latin.requires_bidi_resolution);
        assert!(!latin.requires_complex_shaping());

        let arabic = text_cluster_properties("العربية");
        assert_eq!(arabic.direction, TextDirection::RightToLeft);
        assert!(arabic.requires_bidi_resolution);
        assert!(arabic.requires_complex_shaping());
        assert!(arabic.uses_joined_forms());

        let hebrew = text_cluster_properties("עברית");
        assert_eq!(hebrew.direction, TextDirection::RightToLeft);
        assert!(hebrew.requires_bidi_resolution);

        let devanagari = text_cluster_properties("हिन्दी");
        assert_eq!(devanagari.direction, TextDirection::LeftToRight);
        assert!(devanagari.requires_complex_shaping());
        assert!(!devanagari.uses_joined_forms());
    }

    #[test]
    fn common_text_preserves_emoji_and_pictographic_metadata() {
        let emoji = text_cluster_properties("😀");
        assert_eq!(emoji.script, TextScript::COMMON);
        assert_eq!(emoji.direction, TextDirection::Neutral);
        assert!(emoji.contains_emoji);
        assert!(emoji.contains_extended_pictographic);
    }

    #[test]
    fn bidi_controls_do_not_get_mistaken_for_script_identity() {
        let rtl_isolate = text_cluster_properties("\u{2067}");
        assert_eq!(rtl_isolate.script, TextScript::COMMON);
        assert_eq!(rtl_isolate.direction, TextDirection::RightToLeft);
        assert!(rtl_isolate.requires_bidi_resolution);

        let first_strong_isolate = text_cluster_properties("\u{2068}");
        assert_eq!(first_strong_isolate.direction, TextDirection::Neutral);
        assert!(first_strong_isolate.requires_bidi_resolution);
    }

    #[test]
    fn coverage_probe_is_unicode_wide_not_cjk_only() {
        assert!(!text_needs_font_coverage_probe("ASCII only"));
        assert!(text_needs_font_coverage_probe("中文"));
        assert!(text_needs_font_coverage_probe("العربية"));
        assert!(text_needs_font_coverage_probe("עברית"));
        assert!(text_needs_font_coverage_probe("ไทย"));
        assert!(text_needs_font_coverage_probe("हिन्दी"));
        assert!(text_needs_font_coverage_probe("😀"));
    }

    #[test]
    fn missing_coverage_detection_is_unicode_wide() {
        assert!(!text_contains_missing_font_coverage("ASCII only", |_| false));
        assert!(!text_contains_missing_font_coverage("中文", |_| true));
        assert!(text_contains_missing_font_coverage("中文", |_| false));
        assert!(text_contains_missing_font_coverage("العربية", |_| false));
        assert!(text_contains_missing_font_coverage("हिन्दी", |_| false));
        assert!(text_contains_missing_font_coverage("😀", |_| false));
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

        let emoji_properties = text_cluster_properties_for_utf16_cluster(text, emoji, 2);
        assert!(emoji_properties.contains_emoji);
        assert_eq!(emoji_properties.direction, TextDirection::Neutral);
        let arabic_properties = text_cluster_properties_for_utf16_cluster(text, arabic, 1);
        assert_eq!(arabic_properties.direction, TextDirection::RightToLeft);
        assert!(arabic_properties.requires_bidi_resolution);
    }

    #[test]
    fn stable_raster_frame_is_a_narrow_policy_not_script_classification() {
        assert!(text_cluster_properties("中文").uses_stable_vertical_raster_frame());
        assert!(text_cluster_properties("한글").uses_stable_vertical_raster_frame());
        assert!(!text_cluster_properties("العربية").uses_stable_vertical_raster_frame());
        assert!(!text_cluster_properties("हिन्दी").uses_stable_vertical_raster_frame());
    }
}
