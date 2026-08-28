use crate::i18n::{I18nArg, I18nKey, Locale, LocalizedText, catalog, interpolate_owned_args};
use gpui::Global;
use gpui::SharedString;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

static GLOBAL_I18N: OnceLock<I18n> = OnceLock::new();

#[derive(Clone)]
pub struct I18n {
    locale: Arc<AtomicU8>,
    revision: Arc<AtomicU64>,
}

impl I18n {
    pub fn new() -> Self {
        catalog::initialize();
        Self {
            locale: Arc::new(AtomicU8::new(Locale::default().index())),
            revision: Arc::new(AtomicU64::new(0)),
        }
    }

    pub(crate) fn register_global(self) -> Result<(), Self> {
        GLOBAL_I18N.set(self)
    }

    pub fn locale(&self) -> Locale {
        Locale::from_index(self.locale.load(Ordering::Acquire))
    }

    pub(crate) fn set_locale(&mut self, locale: Locale) {
        let previous = self.locale.swap(locale.index(), Ordering::AcqRel);
        if previous != locale.index() {
            self.revision.fetch_add(1, Ordering::AcqRel);
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    /// Looks up a dynamic key supplied by an external or data-driven source.
    ///
    /// Missing translations fall back to English; an unknown key returns `None`.
    pub fn lookup(&self, key: &str) -> Option<SharedString> {
        Some(SharedString::from(
            catalog::lookup(self.locale(), key)?.text,
        ))
    }

    /// Resolves embedded text, falling back to the key when it is unknown.
    pub fn t_key(&self, key: I18nKey) -> SharedString {
        self.lookup(key.as_str())
            .unwrap_or_else(|| SharedString::from(key.as_str()))
    }

    /// Resolves a dynamic key with transient formatting arguments.
    pub fn lookup_args<const N: usize>(
        &self,
        key: &str,
        args: [I18nArg<'_>; N],
    ) -> Option<SharedString> {
        let translation = catalog::lookup(self.locale(), key)?;
        if N == 0 {
            return Some(SharedString::from(translation.text));
        }
        Some(SharedString::from(crate::i18n::interpolate_args(
            translation.parts,
            args,
        )))
    }

    /// Resolves embedded text with transient formatting arguments.
    pub fn t_key_args<const N: usize>(&self, key: I18nKey, args: [I18nArg<'_>; N]) -> SharedString {
        self.lookup_args(key.as_str(), args)
            .unwrap_or_else(|| SharedString::from(key.as_str()))
    }

    pub fn t_key_positional<const N: usize>(
        &self,
        key: I18nKey,
        args: [crate::i18n::I18nPositionalArg<'_>; N],
    ) -> SharedString {
        if N == 0 {
            return self.t_key(key);
        }
        let Some(translation) = catalog::lookup(self.locale(), key.as_str()) else {
            return SharedString::from(key.as_str());
        };
        SharedString::from(crate::i18n::interpolate_positional_args(
            translation.parts,
            args,
        ))
    }

    /// Resolves an owned semantic message against the current locale.
    pub fn resolve(&self, text: &LocalizedText) -> SharedString {
        match text {
            LocalizedText::Raw(value) => SharedString::from(value.clone()),
            LocalizedText::Key(key) => self.t_key(*key),
            LocalizedText::Args { key, args } => catalog::lookup(self.locale(), key.as_str())
                .map(|translation| {
                    SharedString::from(interpolate_owned_args(translation.parts, args))
                })
                .unwrap_or_else(|| SharedString::from(key.as_str())),
        }
    }
}

impl Default for I18n {
    fn default() -> Self {
        Self::new()
    }
}

impl Global for I18n {}

pub(crate) fn global_i18n() -> &'static I18n {
    GLOBAL_I18N
        .get()
        .expect("I18n must be registered before using t!")
}

/// Version pair for an Entity-owned text cache.
///
/// A cache is valid only while both its domain data and the application
/// language are unchanged. Keeping this stamp beside the cache avoids
/// rebuilding translated text on unrelated renders while still invalidating
/// it when a language switch is observed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct I18nCacheStamp {
    pub language_revision: u64,
    pub data_revision: u64,
}

impl I18nCacheStamp {
    pub fn new(i18n: &I18n, data_revision: u64) -> Self {
        Self {
            language_revision: i18n.revision(),
            data_revision,
        }
    }

    pub fn needs_refresh(self, i18n: &I18n, data_revision: u64) -> bool {
        self != Self::new(i18n, data_revision)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LanguagePreference {
    Auto,
    Explicit(Locale),
}

impl LanguagePreference {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Explicit(locale) => locale.code(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LanguageRequest {
    pub preference: LanguagePreference,
    pub locale: Locale,
}

#[derive(Debug)]
pub struct LanguageCompletion {
    pub next: Option<(u64, LanguageRequest)>,
    pub rollback: Option<LanguageRequest>,
}

/// Application-lifetime coordinator for language preference persistence.
///
/// `I18n` remains only the read/notify global. This state owns ordering and
/// persistence bookkeeping so settings windows cannot start competing saves.
pub struct LanguageController {
    saved: LanguageRequest,
    latest: LanguageRequest,
    active: Option<(u64, LanguageRequest)>,
    pending: Option<LanguageRequest>,
    next_id: u64,
}

impl Default for LanguageController {
    fn default() -> Self {
        let request = LanguageRequest {
            preference: LanguagePreference::Explicit(Locale::default()),
            locale: Locale::default(),
        };
        Self {
            saved: request.clone(),
            latest: request,
            active: None,
            pending: None,
            next_id: 0,
        }
    }
}

impl LanguageController {
    pub fn initialize(&mut self, code: &str, locale: Locale) {
        let request = LanguageRequest {
            preference: if code.trim().eq_ignore_ascii_case("auto") {
                LanguagePreference::Auto
            } else {
                LanguagePreference::Explicit(locale)
            },
            locale,
        };
        self.saved = request.clone();
        self.latest = request;
        self.active = None;
        self.pending = None;
    }

    pub fn submit(&mut self, request: LanguageRequest) -> Option<(u64, LanguageRequest)> {
        if self.latest == request {
            return None;
        }
        self.latest = request.clone();
        if self.active.is_some() {
            self.pending = Some(request);
            return None;
        }
        self.next_id = self.next_id.wrapping_add(1);
        let id = self.next_id;
        self.active = Some((id, request.clone()));
        Some((id, request))
    }

    pub fn complete(&mut self, id: u64, success: bool) -> LanguageCompletion {
        let Some((active_id, active_request)) = self.active.take() else {
            return LanguageCompletion {
                next: None,
                rollback: None,
            };
        };
        if active_id != id {
            self.active = Some((active_id, active_request));
            return LanguageCompletion {
                next: None,
                rollback: None,
            };
        }

        if success {
            self.saved = active_request;
        }

        if let Some(next_request) = self.pending.take() {
            if next_request == self.saved {
                self.latest = self.saved.clone();
                return LanguageCompletion {
                    next: None,
                    rollback: None,
                };
            }
            self.next_id = self.next_id.wrapping_add(1);
            let next_id = self.next_id;
            self.active = Some((next_id, next_request.clone()));
            return LanguageCompletion {
                next: Some((next_id, next_request)),
                rollback: None,
            };
        }

        let rollback = (!success).then(|| {
            self.latest = self.saved.clone();
            self.saved.clone()
        });
        LanguageCompletion {
            next: None,
            rollback,
        }
    }
}

impl Global for LanguageController {}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(code: &'static str, locale: Locale) -> LanguageRequest {
        LanguageRequest {
            preference: if code == "auto" {
                LanguagePreference::Auto
            } else {
                LanguagePreference::Explicit(locale)
            },
            locale,
        }
    }

    #[test]
    fn controller_serializes_latest_pending_request() {
        let mut controller = LanguageController::default();
        controller.initialize("zh-CN", Locale::ZhCn);

        let first = controller.submit(request("en-US", Locale::EnUs));
        assert!(first.is_some());
        assert!(controller.submit(request("ja-JP", Locale::JaJp)).is_none());

        let first_id = first.expect("first request must start").0;
        let completion = controller.complete(first_id, true);
        let (second_id, second) = completion.next.expect("latest request must continue");
        assert_eq!(second.preference.code(), "ja-JP");

        let completion = controller.complete(second_id, false);
        assert_eq!(
            completion
                .rollback
                .expect("failed latest request")
                .preference
                .code(),
            "en-US"
        );
    }

    #[test]
    fn cloned_handles_observe_locale_and_revision_changes() {
        let mut current = I18n::default();
        let clone = current.clone();
        assert_eq!(clone.locale(), Locale::ZhCn);
        assert_eq!(clone.revision(), 0);

        current.set_locale(Locale::EnUs);

        assert_eq!(clone.locale(), Locale::EnUs);
        assert_eq!(clone.revision(), 1);
        assert_eq!(clone.t_key(crate::i18n_key!("common.cancel")), "Cancel");
    }

    #[test]
    fn unknown_keys_do_not_require_compile_time_validation() {
        const UNKNOWN: I18nKey = crate::i18n_key!("missing.translation.for.test");
        let i18n = I18n::new();
        assert!(i18n.lookup(UNKNOWN.as_str()).is_none());
        assert_eq!(i18n.t_key(UNKNOWN), UNKNOWN.as_str());
        assert_eq!(
            i18n.t_key_args(UNKNOWN, crate::i18n_args![("extra", "value")]),
            UNKNOWN.as_str()
        );
        assert_eq!(
            i18n.t_key_positional(UNKNOWN, crate::i18n_positional_args!["value"]),
            UNKNOWN.as_str()
        );
        let text = crate::localized_text!("missing.translation.for.test", extra = "value");
        assert_eq!(i18n.resolve(&text), UNKNOWN.as_str());
    }

    #[test]
    fn embedded_templates_support_named_and_positional_arguments() {
        let i18n = I18n::new();
        let key = crate::i18n_key!("LauncherSettings.language_save_failed");
        let named = i18n.t_key_args(key, crate::i18n_args![("error", "disk full")]);
        let positional = i18n.t_key_positional(key, crate::i18n_positional_args!["disk full"]);
        assert!(named.contains("disk full"));
        assert_eq!(named, positional);
    }

    #[test]
    fn cache_stamp_tracks_language_and_domain_versions() {
        let i18n = I18n::default();
        let stamp = I18nCacheStamp::new(&i18n, 7);
        assert!(!stamp.needs_refresh(&i18n, 7));
        assert!(stamp.needs_refresh(&i18n, 8));

        let mut i18n = i18n;
        i18n.set_locale(Locale::EnUs);
        assert!(stamp.needs_refresh(&i18n, 7));
    }

    #[test]
    fn stale_failure_does_not_rollback_newer_pending_selection() {
        let mut controller = LanguageController::default();
        controller.initialize("zh-CN", Locale::ZhCn);

        let first = controller
            .submit(request("en-US", Locale::EnUs))
            .expect("first save must start");
        assert!(controller.submit(request("ja-JP", Locale::JaJp)).is_none());

        let completion = controller.complete(first.0, false);
        let next = completion.next.expect("newer selection must continue");
        assert!(completion.rollback.is_none());
        assert_eq!(next.1.preference.code(), "ja-JP");

        let completion = controller.complete(next.0, true);
        assert!(completion.next.is_none());
        assert!(completion.rollback.is_none());
    }

    #[test]
    fn duplicate_selection_does_not_start_another_save() {
        let mut controller = LanguageController::default();
        controller.initialize("zh-CN", Locale::ZhCn);

        assert!(controller.submit(request("en-US", Locale::EnUs)).is_some());
        assert!(controller.submit(request("en-US", Locale::EnUs)).is_none());
    }
}
