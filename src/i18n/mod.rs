use std::fmt;
use std::fmt::Write as _;
use std::sync::Arc;

pub(crate) mod catalog;

#[derive(Clone, Copy)]
pub(crate) struct TemplatePart<'a> {
    literal: &'a str,
    placeholder: Option<&'a str>,
    argument_index: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Locale {
    ZhCn,
    ZhTw,
    EnUs,
    JaJp,
    KoKr,
}

/// A borrowed key for text in the embedded language files.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct I18nKey(&'static str);

impl I18nKey {
    pub const fn new(key: &'static str) -> Self {
        Self(key)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// A UI message that can outlive the render pass without capturing a
/// translated string or `fmt::Arguments` from an earlier language.
#[derive(Clone, Debug)]
pub enum LocalizedText {
    Raw(Arc<str>),
    Key(I18nKey),
    Args {
        key: I18nKey,
        args: Arc<[(&'static str, Arc<str>)]>,
    },
}

impl LocalizedText {
    pub fn raw(text: impl Into<Arc<str>>) -> Self {
        Self::Raw(text.into())
    }

    pub const fn key(key: I18nKey) -> Self {
        Self::Key(key)
    }

    pub fn args<const N: usize>(key: I18nKey, args: [(&'static str, String); N]) -> Self {
        let args = args
            .into_iter()
            .map(|(name, value)| (name, Arc::<str>::from(value)))
            .collect::<Vec<_>>();
        Self::Args {
            key,
            args: Arc::from(args),
        }
    }
}

#[macro_export]
macro_rules! i18n_key {
    ($key:literal) => {
        $crate::i18n::I18nKey::new($key)
    };
}

#[macro_export]
macro_rules! t {
    ($key:literal $(,)?) => {{
        const KEY: $crate::i18n::I18nKey = $crate::i18n::I18nKey::new($key);
        $crate::ui::state::i18n::global_i18n().t_key(KEY)
    }};
    ($key:literal, $($name:ident = $value:expr),+ $(,)?) => {{
        const KEY: $crate::i18n::I18nKey = $crate::i18n::I18nKey::new($key);
        $crate::ui::state::i18n::global_i18n()
            .t_key_args(KEY, $crate::i18n_args![$((stringify!($name), $value)),+])
    }};
    ($key:literal, $($value:expr),+ $(,)?) => {{
        const KEY: $crate::i18n::I18nKey = $crate::i18n::I18nKey::new($key);
        $crate::ui::state::i18n::global_i18n()
            .t_key_positional(KEY, $crate::i18n_positional_args![$($value),+])
    }};
}

#[macro_export]
macro_rules! localized_text {
    ($key:literal) => {{
        const KEY: $crate::i18n::I18nKey = $crate::i18n::I18nKey::new($key);
        $crate::i18n::LocalizedText::key(KEY)
    }};
    ($key:literal $(, $name:ident = $value:expr)* $(,)?) => {{
        const KEY: $crate::i18n::I18nKey = $crate::i18n::I18nKey::new($key);
        $crate::i18n::LocalizedText::args(
            KEY,
            [$( (stringify!($name), ($value).to_string()) ),*],
        )
    }};
}

impl Locale {
    pub(crate) const fn index(self) -> u8 {
        match self {
            Locale::ZhCn => 0,
            Locale::ZhTw => 1,
            Locale::EnUs => 2,
            Locale::JaJp => 3,
            Locale::KoKr => 4,
        }
    }

    pub(crate) const fn from_index(index: u8) -> Self {
        match index {
            1 => Locale::ZhTw,
            2 => Locale::EnUs,
            3 => Locale::JaJp,
            4 => Locale::KoKr,
            _ => Locale::ZhCn,
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        let normalized = code.trim().replace('_', "-");
        let lower = normalized.to_ascii_lowercase();
        match lower.as_str() {
            "zh-cn" => Some(Locale::ZhCn),
            "zh-tw" => Some(Locale::ZhTw),
            "en-us" => Some(Locale::EnUs),
            "ja-jp" => Some(Locale::JaJp),
            "ko-kr" => Some(Locale::KoKr),
            _ => {
                if lower.starts_with("zh-tw") || lower.starts_with("zh-hk") {
                    return Some(Locale::ZhTw);
                }
                if lower.starts_with("zh-") || lower == "zh" {
                    return Some(Locale::ZhCn);
                }
                if lower.starts_with("en-") || lower == "en" {
                    return Some(Locale::EnUs);
                }
                if lower.starts_with("ja-") || lower == "ja" {
                    return Some(Locale::JaJp);
                }
                if lower.starts_with("ko-") || lower == "ko" {
                    return Some(Locale::KoKr);
                }
                None
            }
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Locale::ZhCn => "zh-CN",
            Locale::ZhTw => "zh-TW",
            Locale::EnUs => "en-US",
            Locale::JaJp => "ja-JP",
            Locale::KoKr => "ko-KR",
        }
    }

    pub fn all() -> &'static [Locale] {
        const ALL: &[Locale] = &[
            Locale::ZhCn,
            Locale::ZhTw,
            Locale::EnUs,
            Locale::JaJp,
            Locale::KoKr,
        ];
        ALL
    }

    pub fn next(self) -> Self {
        let all = Self::all();
        let index = all.iter().position(|locale| *locale == self).unwrap_or(0);
        all[(index + 1) % all.len()]
    }
}

impl Default for Locale {
    fn default() -> Self {
        Locale::ZhCn
    }
}

pub struct I18nArg<'a> {
    key: &'a str,
    value: fmt::Arguments<'a>,
}

pub struct I18nPositionalArg<'a> {
    value: fmt::Arguments<'a>,
}

impl<'a> I18nPositionalArg<'a> {
    pub fn new(value: fmt::Arguments<'a>) -> Self {
        Self { value }
    }
}

impl<'a> I18nArg<'a> {
    pub fn new(key: &'a str, value: fmt::Arguments<'a>) -> Self {
        Self { key, value }
    }
}

#[macro_export]
macro_rules! i18n_args {
    ($(($key:expr, $value:expr)),* $(,)?) => {
        [
            $(
                $crate::i18n::I18nArg::new($key, format_args!("{}", $value))
            ),*
        ]
    };
}

#[macro_export]
macro_rules! i18n_positional_args {
    ($($value:expr),+ $(,)?) => {
        [$(
            $crate::i18n::I18nPositionalArg::new(format_args!("{}", $value))
        ),+]
    };
}

pub(crate) fn interpolate_args<'a, const N: usize>(
    parts: impl Iterator<Item = TemplatePart<'a>> + Clone,
    args: [I18nArg<'_>; N],
) -> String {
    let capacity = parts.clone().map(|part| part.literal.len()).sum();
    let mut output = String::with_capacity(capacity);
    for part in parts {
        output.push_str(part.literal);
        if let Some(placeholder) = part.placeholder {
            if let Some(argument) = args.iter().find(|argument| argument.key == placeholder) {
                let _ = output.write_fmt(argument.value);
            } else {
                output.push_str("{{");
                output.push_str(placeholder);
                output.push_str("}}");
            }
        }
    }
    output
}

pub(crate) fn interpolate_positional_args<'a, const N: usize>(
    parts: impl Iterator<Item = TemplatePart<'a>> + Clone,
    args: [I18nPositionalArg<'_>; N],
) -> String {
    let capacity = parts.clone().map(|part| part.literal.len()).sum();
    let mut output = String::with_capacity(capacity);
    for part in parts {
        output.push_str(part.literal);
        if let Some(argument_index) = part.argument_index {
            if let Some(argument) = args.get(argument_index) {
                let _ = output.write_fmt(argument.value);
            } else if let Some(placeholder) = part.placeholder {
                output.push_str("{{");
                output.push_str(placeholder);
                output.push_str("}}");
            }
        }
    }
    output
}

pub(crate) fn interpolate_owned_args<'a>(
    parts: impl Iterator<Item = TemplatePart<'a>> + Clone,
    args: &[(&'static str, Arc<str>)],
) -> String {
    let capacity = parts.clone().map(|part| part.literal.len()).sum();
    let mut output = String::with_capacity(capacity);
    for part in parts {
        output.push_str(part.literal);
        if let Some(placeholder) = part.placeholder {
            if let Some((_, value)) = args.iter().find(|(name, _)| *name == placeholder) {
                output.push_str(value);
            } else {
                output.push_str("{{");
                output.push_str(placeholder);
                output.push_str("}}");
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    fn positional_t_macro_compiles() -> gpui::SharedString {
        t!("LauncherSettings.language_save_failed", "disk full")
    }

    #[test]
    fn locale_codes_round_trip() {
        for locale in Locale::all() {
            assert_eq!(Locale::from_code(locale.code()), Some(*locale));
        }
    }

    #[test]
    fn embedded_key_resolves_to_borrowed_catalog_text() {
        catalog::initialize();
        let key = I18nKey::new("common.cancel");
        assert_eq!(
            catalog::lookup(Locale::ZhCn, key.as_str())
                .expect("embedded key")
                .text,
            "取消"
        );
    }

    #[test]
    fn interpolation_preserves_unknown_placeholders() {
        catalog::initialize();
        let value = interpolate_args(
            catalog::lookup(Locale::ZhCn, "LauncherSettings.language_save_failed")
                .expect("embedded key")
                .parts,
            crate::i18n_args![("error", "disk")],
        );
        assert!(value.contains("disk"));
        assert!(!value.contains("{{error}}"));
    }

    #[test]
    fn localized_text_owns_dynamic_arguments() {
        let text = localized_text!("LauncherSettings.language_save_failed", error = "disk full");
        let LocalizedText::Args { args, .. } = text else {
            panic!("expected argument-bearing localized text");
        };
        assert_eq!(args[0].0, "error");
        assert_eq!(args[0].1.as_ref(), "disk full");
    }

    #[test]
    fn localized_text_without_arguments_keeps_only_the_key() {
        let text = localized_text!("common.cancel");
        assert!(matches!(text, LocalizedText::Key(_)));
    }

    #[test]
    fn owned_interpolation_repeats_unicode_arguments() {
        let args: Arc<[(&'static str, Arc<str>)]> = Arc::from([("value", Arc::<str>::from("雪"))]);
        const PARTS: &[TemplatePart<'static>] = &[
            TemplatePart {
                literal: "",
                placeholder: Some("value"),
                argument_index: None,
            },
            TemplatePart {
                literal: " / ",
                placeholder: Some("value"),
                argument_index: None,
            },
        ];
        assert_eq!(
            interpolate_owned_args(PARTS.iter().copied(), &args),
            "雪 / 雪"
        );
    }

    #[test]
    fn positional_interpolation_repeats_unicode_arguments() {
        const PARTS: &[TemplatePart<'static>] = &[
            TemplatePart {
                literal: "",
                placeholder: Some("value"),
                argument_index: Some(0),
            },
            TemplatePart {
                literal: " / ",
                placeholder: Some("value"),
                argument_index: Some(0),
            },
        ];
        assert_eq!(
            interpolate_positional_args(PARTS.iter().copied(), crate::i18n_positional_args!["雪"]),
            "雪 / 雪"
        );
    }
}
