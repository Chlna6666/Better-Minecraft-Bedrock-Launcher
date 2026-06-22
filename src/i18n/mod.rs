use std::borrow::Cow;
use std::fmt;
use std::fmt::Write as _;

include!(concat!(env!("OUT_DIR"), "/generated_locales.rs"));

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Locale {
    ZhCn,
    ZhTw,
    EnUs,
    JaJp,
    KoKr,
}

impl Locale {
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

#[derive(Clone)]
pub struct Translator {
    locale: Locale,
    entries: &'static [(&'static str, &'static str)],
}

impl Translator {
    pub fn new() -> Self {
        let locale = Locale::default();
        Self {
            locale,
            entries: locale_entries(locale.code()),
        }
    }

    pub fn locale(&self) -> Locale {
        self.locale
    }

    pub fn set_locale(&mut self, locale: Locale) {
        self.locale = locale;
        self.entries = locale_entries(locale.code());
    }

    pub fn ensure_loaded(&mut self) {}

    pub fn ensure_locale_loaded(&mut self, _locale: Locale) {}

    pub fn translate(&self, key: &str) -> Cow<'static, str> {
        lookup_entries_value(self.entries, key)
            .map(Cow::Borrowed)
            .unwrap_or_else(|| Cow::Owned(key.to_string()))
    }

    pub fn translate_args<const N: usize>(
        &self,
        key: &str,
        args: [I18nArg<'_>; N],
    ) -> Cow<'static, str> {
        if N == 0 {
            return self.translate(key);
        }

        let Some(value) = lookup_entries_value(self.entries, key) else {
            return Cow::Owned(key.to_string());
        };

        Cow::Owned(interpolate_args(value, args))
    }
}

pub struct I18nArg<'a> {
    key: &'a str,
    value: fmt::Arguments<'a>,
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

impl Default for Translator {
    fn default() -> Self {
        Self::new()
    }
}

fn lookup_entries_value(
    entries: &'static [(&'static str, &'static str)],
    key: &str,
) -> Option<&'static str> {
    entries
        .binary_search_by_key(&key, |(entry_key, _)| *entry_key)
        .ok()
        .map(|index| entries[index].1)
}

fn interpolate_args<const N: usize>(template: &'static str, args: [I18nArg<'_>; N]) -> String {
    let mut output = String::with_capacity(template.len());
    let mut cursor = 0;

    while let Some(open_offset) = template[cursor..].find("{{") {
        let open = cursor + open_offset;
        output.push_str(&template[cursor..open]);

        let Some(close_offset) = template[open + 2..].find("}}") else {
            output.push_str(&template[open..]);
            return output;
        };

        let close = open + 2 + close_offset;
        let placeholder = &template[open + 2..close];
        if let Some(argument) = args.iter().find(|argument| argument.key == placeholder) {
            let _ = output.write_fmt(argument.value);
        } else {
            output.push_str(&template[open..close + 2]);
        }

        cursor = close + 2;
    }

    output.push_str(&template[cursor..]);
    output
}
