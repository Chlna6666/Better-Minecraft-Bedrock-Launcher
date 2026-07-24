//! Runtime localization service for egpui applications.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, RwLock},
};

use egpui_manifest::{LocaleId, LocaleIdError};
use fluent_bundle::{FluentArgs, FluentError, FluentResource, concurrent::FluentBundle};
use thiserror::Error;
use tokio::sync::watch;
use unic_langid::LanguageIdentifier;

use crate::{ResourceId, ResourceResolver, ResourceResolverError};

type ConcurrentBundle = FluentBundle<FluentResource>;

/// Text direction associated with a locale.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocaleDirection {
    /// Left-to-right writing direction.
    LeftToRight,
    /// Right-to-left writing direction.
    RightToLeft,
}

/// A Fluent catalog registered under an application namespace and locale.
#[derive(Clone, Debug)]
pub struct I18nCatalog {
    /// Namespace used to prevent message collisions.
    pub namespace: Arc<str>,
    /// Canonical BCP 47 locale.
    pub locale: LocaleId,
    /// Explicit text direction.
    pub direction: LocaleDirection,
    /// Fluent source.
    pub source: String,
}

/// An owned message value that can cross execution domains.
#[derive(Clone, Debug, PartialEq)]
pub enum MessageValue {
    /// String argument.
    String(String),
    /// Numeric or plural argument.
    Number(f64),
}

impl From<String> for MessageValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for MessageValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<f64> for MessageValue {
    fn from(value: f64) -> Self {
        Self::Number(value)
    }
}

impl From<i64> for MessageValue {
    fn from(value: i64) -> Self {
        Self::Number(value as f64)
    }
}

/// Owned Fluent arguments.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MessageArguments {
    values: BTreeMap<String, MessageValue>,
}

impl MessageArguments {
    /// Creates empty arguments.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces an argument.
    #[must_use]
    pub fn with(mut self, name: impl Into<String>, value: impl Into<MessageValue>) -> Self {
        self.values.insert(name.into(), value.into());
        self
    }

    /// Adds or replaces an argument.
    pub fn insert(
        &mut self,
        name: impl Into<String>,
        value: impl Into<MessageValue>,
    ) -> Option<MessageValue> {
        self.values.insert(name.into(), value.into())
    }

    fn fluent(&self) -> FluentArgs<'_> {
        let mut arguments = FluentArgs::with_capacity(self.values.len());
        for (name, value) in &self.values {
            match value {
                MessageValue::String(value) => arguments.set(name.as_str(), value.as_str()),
                MessageValue::Number(value) => arguments.set(name.as_str(), *value),
            }
        }
        arguments
    }
}

/// Observable locale state for a GPUI snapshot consumer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocaleSnapshot {
    /// Active locale.
    pub locale: LocaleId,
    /// Active writing direction.
    pub direction: LocaleDirection,
    /// Monotonic update revision.
    pub revision: u64,
}

/// i18n registration, fallback, or formatting failure.
#[derive(Debug, Error)]
pub enum I18nError {
    /// Invalid locale.
    #[error(transparent)]
    InvalidLocale(#[from] LocaleIdError),
    /// Fluent parser failure.
    #[error("failed to parse catalog `{namespace}` for `{locale}`: {errors}")]
    Parse {
        /// Catalog namespace.
        namespace: Arc<str>,
        /// Catalog locale.
        locale: LocaleId,
        /// Parser diagnostics.
        errors: String,
    },
    /// Fluent registration failure.
    #[error("failed to register catalog `{namespace}` for `{locale}`: {errors}")]
    Register {
        /// Catalog namespace.
        namespace: Arc<str>,
        /// Catalog locale.
        locale: LocaleId,
        /// Registration diagnostics.
        errors: String,
    },
    /// Conflicting direction metadata.
    #[error("catalog direction for locale `{0}` conflicts with an existing catalog")]
    DirectionConflict(LocaleId),
    /// No exact or parent catalog is available.
    #[error("locale `{0}` is not available")]
    LocaleUnavailable(LocaleId),
    /// Message was not found.
    #[error("message `{namespace}:{message}` was not found for locale `{locale}`")]
    MissingMessage {
        /// Namespace.
        namespace: Arc<str>,
        /// Message key.
        message: Arc<str>,
        /// Requested locale.
        locale: LocaleId,
    },
    /// Message has no value or requested attribute.
    #[error("message `{namespace}:{message}` has no value for locale `{locale}`")]
    MissingValue {
        /// Namespace.
        namespace: Arc<str>,
        /// Message key.
        message: Arc<str>,
        /// Resolved locale.
        locale: LocaleId,
    },
    /// Fluent resolver failure.
    #[error("failed to format `{namespace}:{message}` for `{locale}`: {errors}")]
    Format {
        /// Namespace.
        namespace: Arc<str>,
        /// Message key.
        message: Arc<str>,
        /// Resolved locale.
        locale: LocaleId,
        /// Resolver diagnostics.
        errors: String,
    },
    /// Catalog resource was absent.
    #[error("i18n catalog resource `{0}` was not found")]
    ResourceMissing(ResourceId),
    /// Catalog resource was not UTF-8.
    #[error("i18n catalog resource `{0}` is not valid UTF-8")]
    ResourceEncoding(ResourceId),
    /// Resource resolver failure.
    #[error(transparent)]
    Resource(#[from] ResourceResolverError),
    /// Catalog resource read failure.
    #[error("failed to read i18n catalog `{resource}`: {source}")]
    ResourceRead {
        /// Resource identifier.
        resource: ResourceId,
        /// Underlying IO failure.
        source: std::io::Error,
    },
    /// Service state was poisoned.
    #[error("i18n service coordination state is poisoned")]
    Poisoned,
}

struct I18nState {
    source_locale: LocaleId,
    active_locale: LocaleId,
    directions: BTreeMap<LocaleId, LocaleDirection>,
    catalogs: BTreeMap<Arc<str>, BTreeMap<LocaleId, ConcurrentBundle>>,
    revision: u64,
}

/// Thread-safe Fluent service with deterministic locale fallback.
#[derive(Clone)]
pub struct I18nService {
    state: Arc<RwLock<I18nState>>,
    changes: watch::Sender<LocaleSnapshot>,
}

impl I18nService {
    /// Creates a service with source and active locales.
    #[must_use]
    pub fn new(source_locale: LocaleId, active_locale: LocaleId) -> Self {
        let snapshot = LocaleSnapshot {
            locale: active_locale.clone(),
            direction: LocaleDirection::LeftToRight,
            revision: 0,
        };
        let (changes, _receiver) = watch::channel(snapshot);
        Self {
            state: Arc::new(RwLock::new(I18nState {
                source_locale,
                active_locale,
                directions: BTreeMap::new(),
                catalogs: BTreeMap::new(),
                revision: 0,
            })),
            changes,
        }
    }

    /// Registers or replaces one Fluent catalog.
    ///
    /// # Errors
    ///
    /// Returns parser, direction, or state errors.
    pub fn register_catalog(&self, catalog: I18nCatalog) -> Result<(), I18nError> {
        let language = catalog
            .locale
            .as_str()
            .parse::<LanguageIdentifier>()
            .map_err(|_| LocaleIdError(catalog.locale.to_string()))?;
        let resource = FluentResource::try_new(catalog.source).map_err(|(_resource, errors)| {
            I18nError::Parse {
                namespace: catalog.namespace.clone(),
                locale: catalog.locale.clone(),
                errors: join_debug(errors),
            }
        })?;
        let mut bundle = ConcurrentBundle::new_concurrent(vec![language]);
        bundle.set_use_isolating(false);
        bundle
            .add_resource(resource)
            .map_err(|errors| I18nError::Register {
                namespace: catalog.namespace.clone(),
                locale: catalog.locale.clone(),
                errors: join_fluent(errors),
            })?;

        let mut state = self.state.write().map_err(|_| I18nError::Poisoned)?;
        if state
            .directions
            .get(&catalog.locale)
            .is_some_and(|direction| *direction != catalog.direction)
        {
            return Err(I18nError::DirectionConflict(catalog.locale));
        }
        state
            .directions
            .insert(catalog.locale.clone(), catalog.direction);
        state
            .catalogs
            .entry(catalog.namespace)
            .or_default()
            .insert(catalog.locale, bundle);
        publish_snapshot(&mut state, &self.changes);
        Ok(())
    }

    /// Loads and registers a UTF-8 catalog from the resource resolver.
    ///
    /// # Errors
    ///
    /// Returns resource, encoding, parser, or registration errors.
    pub fn register_catalog_resource(
        &self,
        resolver: &ResourceResolver,
        resource: &ResourceId,
        namespace: impl Into<Arc<str>>,
        locale: LocaleId,
        direction: LocaleDirection,
        maximum_bytes: usize,
    ) -> Result<(), I18nError> {
        let Some(handle) = resolver.resolve(resource)? else {
            return Err(I18nError::ResourceMissing(resource.clone()));
        };
        let bytes =
            handle
                .read_to_end(maximum_bytes)
                .map_err(|source| I18nError::ResourceRead {
                    resource: resource.clone(),
                    source,
                })?;
        let source =
            String::from_utf8(bytes).map_err(|_| I18nError::ResourceEncoding(resource.clone()))?;
        self.register_catalog(I18nCatalog {
            namespace: namespace.into(),
            locale,
            direction,
            source,
        })
    }

    /// Lists locales present in at least one catalog.
    ///
    /// # Errors
    ///
    /// Returns an error when state is poisoned.
    pub fn available_locales(&self) -> Result<Vec<LocaleId>, I18nError> {
        let state = self.state.read().map_err(|_| I18nError::Poisoned)?;
        Ok(state
            .catalogs
            .values()
            .flat_map(|catalogs| catalogs.keys().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect())
    }

    /// Switches locale only when an exact or parent catalog is installed.
    ///
    /// # Errors
    ///
    /// Returns [`I18nError::LocaleUnavailable`] when no catalog can serve it.
    pub fn set_locale(&self, locale: LocaleId) -> Result<(), I18nError> {
        let mut state = self.state.write().map_err(|_| I18nError::Poisoned)?;
        let available = locale_parents(&locale).iter().any(|candidate| {
            state
                .catalogs
                .values()
                .any(|catalogs| catalogs.contains_key(candidate))
        });
        if !available {
            return Err(I18nError::LocaleUnavailable(locale));
        }
        if state.active_locale != locale {
            state.active_locale = locale;
            publish_snapshot(&mut state, &self.changes);
        }
        Ok(())
    }

    /// Returns the current locale snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error when state is poisoned.
    pub fn snapshot(&self) -> Result<LocaleSnapshot, I18nError> {
        let state = self.state.read().map_err(|_| I18nError::Poisoned)?;
        Ok(snapshot_for(&state))
    }

    /// Subscribes to locale changes.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<LocaleSnapshot> {
        self.changes.subscribe()
    }

    /// Formats a `message` or `message.attribute` through locale fallback.
    ///
    /// # Errors
    ///
    /// Returns missing-value, formatting, or state errors.
    pub fn format(
        &self,
        namespace: impl AsRef<str>,
        message: impl AsRef<str>,
        arguments: &MessageArguments,
    ) -> Result<String, I18nError> {
        let namespace = namespace.as_ref();
        let message = message.as_ref();
        let (message_id, attribute) = message
            .split_once('.')
            .map_or((message, None), |(id, attribute)| (id, Some(attribute)));
        let state = self.state.read().map_err(|_| I18nError::Poisoned)?;
        let Some(catalogs) = state.catalogs.get(namespace) else {
            return Err(I18nError::MissingMessage {
                namespace: Arc::from(namespace),
                message: Arc::from(message),
                locale: state.active_locale.clone(),
            });
        };
        for locale in locale_fallbacks(&state.active_locale, &state.source_locale) {
            let Some(bundle) = catalogs.get(&locale) else {
                continue;
            };
            let Some(fluent_message) = bundle.get_message(message_id) else {
                continue;
            };
            let pattern = match attribute {
                Some(attribute) => fluent_message
                    .get_attribute(attribute)
                    .map(|attribute| attribute.value()),
                None => fluent_message.value(),
            }
            .ok_or_else(|| I18nError::MissingValue {
                namespace: Arc::from(namespace),
                message: Arc::from(message),
                locale: locale.clone(),
            })?;
            let fluent_arguments = arguments.fluent();
            let mut errors = Vec::new();
            let formatted = bundle.format_pattern(pattern, Some(&fluent_arguments), &mut errors);
            if !errors.is_empty() {
                return Err(I18nError::Format {
                    namespace: Arc::from(namespace),
                    message: Arc::from(message),
                    locale,
                    errors: join_fluent(errors),
                });
            }
            return Ok(formatted.into_owned());
        }
        Err(I18nError::MissingMessage {
            namespace: Arc::from(namespace),
            message: Arc::from(message),
            locale: state.active_locale.clone(),
        })
    }
}

fn locale_parents(locale: &LocaleId) -> Vec<LocaleId> {
    let mut parts = locale.as_str().split('-').collect::<Vec<_>>();
    let mut values = Vec::new();
    while !parts.is_empty() {
        if let Ok(candidate) = LocaleId::new(parts.join("-")) {
            values.push(candidate);
        }
        parts.pop();
    }
    values
}

fn locale_fallbacks(locale: &LocaleId, source: &LocaleId) -> Vec<LocaleId> {
    let mut values = locale_parents(locale);
    if !values.contains(source) {
        values.push(source.clone());
    }
    values
}

fn direction_for(state: &I18nState) -> LocaleDirection {
    locale_fallbacks(&state.active_locale, &state.source_locale)
        .into_iter()
        .find_map(|locale| state.directions.get(&locale).copied())
        .unwrap_or(LocaleDirection::LeftToRight)
}

fn snapshot_for(state: &I18nState) -> LocaleSnapshot {
    LocaleSnapshot {
        locale: state.active_locale.clone(),
        direction: direction_for(state),
        revision: state.revision,
    }
}

fn publish_snapshot(state: &mut I18nState, sender: &watch::Sender<LocaleSnapshot>) {
    state.revision = state.revision.saturating_add(1);
    sender.send_replace(snapshot_for(state));
}

fn join_fluent(errors: Vec<FluentError>) -> String {
    errors
        .into_iter()
        .map(|error| error.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

fn join_debug<T: std::fmt::Debug>(errors: Vec<T>) -> String {
    errors
        .into_iter()
        .map(|error| format!("{error:?}"))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use egpui_manifest::LocaleId;

    use super::{I18nCatalog, I18nError, I18nService, LocaleDirection, MessageArguments};

    #[test]
    fn formats_selectors_and_arguments() {
        let service = I18nService::new(
            LocaleId::new("en-US").expect("locale"),
            LocaleId::new("en-US").expect("locale"),
        );
        service
            .register_catalog(I18nCatalog {
                namespace: Arc::from("app"),
                locale: LocaleId::new("en-US").expect("locale"),
                direction: LocaleDirection::LeftToRight,
                source: "files = { $count ->\n  [one] One file\n *[other] { $count } files\n}"
                    .to_owned(),
            })
            .expect("catalog");
        assert_eq!(
            service
                .format(
                    "app",
                    "files",
                    &MessageArguments::new().with("count", 3_i64)
                )
                .expect("format"),
            "3 files"
        );
    }

    #[test]
    fn switch_requires_catalog_and_publishes_snapshot() {
        let service = I18nService::new(
            LocaleId::new("en-US").expect("locale"),
            LocaleId::new("en-US").expect("locale"),
        );
        service
            .register_catalog(I18nCatalog {
                namespace: Arc::from("app"),
                locale: LocaleId::new("en-US").expect("locale"),
                direction: LocaleDirection::LeftToRight,
                source: "title = English".to_owned(),
            })
            .expect("catalog");
        let receiver = service.subscribe();
        service
            .set_locale(LocaleId::new("en-US").expect("locale"))
            .expect("switch");
        assert_eq!(receiver.borrow().locale.as_str(), "en-US");
        assert!(matches!(
            service.set_locale(LocaleId::new("fr-FR").expect("locale")),
            Err(I18nError::LocaleUnavailable(_))
        ));
    }
}
