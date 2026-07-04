use crate::SharedString;
use anyhow::Result;
use std::fmt;

/// A datastructure for resolving whether an action should be dispatched
/// at this point in the element tree. Contains a set of identifiers
/// and/or key value pairs representing the current context for the
/// keymap.
#[derive(Clone, Default, Eq, PartialEq, Hash)]
pub struct KeyContext(pub(super) Vec<ContextEntry>);

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
/// An entry in a KeyContext
pub struct ContextEntry {
    /// The key (or name if no value)
    pub key: SharedString,
    /// The value
    pub value: Option<SharedString>,
}

impl<'a> TryFrom<&'a str> for KeyContext {
    type Error = anyhow::Error;

    fn try_from(value: &'a str) -> Result<Self> {
        Self::parse(value)
    }
}

impl KeyContext {
    /// Initialize a new [`KeyContext`] that contains an `os` key set to either `macos`, `linux`, `windows` or `unknown`.
    pub fn new_with_defaults() -> Self {
        let mut context = Self::default();
        #[cfg(target_os = "macos")]
        context.set("os", "macos");
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        context.set("os", "linux");
        #[cfg(target_os = "windows")]
        context.set("os", "windows");
        #[cfg(not(any(
            target_os = "macos",
            target_os = "linux",
            target_os = "freebsd",
            target_os = "windows"
        )))]
        context.set("os", "unknown");
        context
    }

    /// Returns the primary context entry (usually the name of the component)
    pub fn primary(&self) -> Option<&ContextEntry> {
        self.0.iter().find(|p| p.value.is_none())
    }

    /// Returns everything except the primary context entry.
    pub fn secondary(&self) -> impl Iterator<Item = &ContextEntry> {
        let primary = self.primary();
        self.0.iter().filter(move |&p| Some(p) != primary)
    }

    /// Check if this context is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Clear this context.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Extend this context with another context.
    pub fn extend(&mut self, other: &Self) {
        for entry in &other.0 {
            if !self.contains(&entry.key) {
                self.0.push(entry.clone());
            }
        }
    }

    /// Add an identifier to this context, if it's not already in this context.
    pub fn add<I: Into<SharedString>>(&mut self, identifier: I) {
        let key = identifier.into();

        if !self.contains(&key) {
            self.0.push(ContextEntry { key, value: None })
        }
    }

    /// Set a key value pair in this context, if it's not already set.
    pub fn set<S1: Into<SharedString>, S2: Into<SharedString>>(&mut self, key: S1, value: S2) {
        let key = key.into();
        if !self.contains(&key) {
            self.0.push(ContextEntry {
                key,
                value: Some(value.into()),
            })
        }
    }

    /// Check if this context contains a given identifier or key.
    pub fn contains(&self, key: &str) -> bool {
        self.0.iter().any(|entry| entry.key.as_ref() == key)
    }

    /// Get the associated value for a given identifier or key.
    pub fn get(&self, key: &str) -> Option<&SharedString> {
        self.0
            .iter()
            .find(|entry| entry.key.as_ref() == key)?
            .value
            .as_ref()
    }
}

impl fmt::Debug for KeyContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut entries = self.0.iter().peekable();
        while let Some(entry) = entries.next() {
            if let Some(ref value) = entry.value {
                write!(f, "{}={}", entry.key, value)?;
            } else {
                write!(f, "{}", entry.key)?;
            }
            if entries.peek().is_some() {
                write!(f, " ")?;
            }
        }
        Ok(())
    }
}
