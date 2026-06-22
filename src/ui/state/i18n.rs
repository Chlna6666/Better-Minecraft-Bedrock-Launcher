use crate::i18n::{I18nArg, Locale, Translator};
use gpui::Global;
use gpui::SharedString;
use std::borrow::Cow;

#[derive(Clone)]
pub struct I18n {
    translator: Translator,
}

impl I18n {
    pub fn new() -> Self {
        Self {
            translator: Translator::new(),
        }
    }

    pub fn locale(&self) -> Locale {
        self.translator.locale()
    }

    pub fn set_locale(&mut self, locale: Locale) {
        self.translator.set_locale(locale);
    }

    pub fn ensure_loaded(&mut self) {
        self.translator.ensure_loaded();
    }

    pub fn ensure_locale_loaded(&mut self, locale: Locale) {
        self.translator.ensure_locale_loaded(locale);
    }

    pub fn t(&self, key: &str) -> SharedString {
        shared_string_from_cow(self.translator.translate(key))
    }

    pub fn t_args<const N: usize>(&self, key: &str, args: [I18nArg<'_>; N]) -> SharedString {
        shared_string_from_cow(self.translator.translate_args(key, args))
    }
}

impl Default for I18n {
    fn default() -> Self {
        Self::new()
    }
}

impl Global for I18n {}

fn shared_string_from_cow(value: Cow<'static, str>) -> SharedString {
    match value {
        Cow::Borrowed(value) => SharedString::from(value),
        Cow::Owned(value) => SharedString::from(value),
    }
}
