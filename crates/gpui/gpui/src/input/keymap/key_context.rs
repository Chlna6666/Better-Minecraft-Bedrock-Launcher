use crate::SharedString;
use anyhow::Result;
use std::{fmt, sync::Arc};

/// A datastructure for resolving whether an action should be dispatched
/// at this point in the element tree. Contains a set of identifiers
/// and/or key value pairs representing the current context for the
/// keymap.
#[derive(Clone, Default, Eq, PartialEq, Hash)]
pub struct KeyContext(Option<Arc<Vec<ContextEntry>>>);

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
    #[inline]
    fn entries(&self) -> &[ContextEntry] {
        self.0.as_deref().map_or(&[], Vec::as_slice)
    }

    #[inline]
    fn entries_mut(&mut self) -> &mut Vec<ContextEntry> {
        Arc::make_mut(self.0.get_or_insert_with(Default::default))
    }

    /// Initialize a new [`KeyContext`] that contains an `os` key set to either `macos`, `linux`, `windows` or `unknown`.
    pub fn with_default_context() -> Self {
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
        self.entries().iter().find(|p| p.value.is_none())
    }

    /// Returns everything except the primary context entry.
    pub fn secondary(&self) -> impl Iterator<Item = &ContextEntry> {
        let primary = self.primary();
        self.entries().iter().filter(move |&p| Some(p) != primary)
    }

    /// Check if this context is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    /// Clear this context.
    pub fn clear(&mut self) {
        self.0 = None;
    }

    /// Extend this context with another context.
    pub fn extend(&mut self, other: &Self) {
        if self.is_empty() {
            // Context propagation and dispatch replay need no entry copy.
            self.0 = other.0.clone();
            return;
        }
        for entry in other.entries() {
            if !self.contains(&entry.key) {
                self.entries_mut().push(entry.clone());
            }
        }
    }

    /// Add an identifier to this context, if it's not already in this context.
    pub fn add<I: Into<SharedString>>(&mut self, identifier: I) {
        let key = identifier.into();

        if !self.contains(&key) {
            self.entries_mut().push(ContextEntry { key, value: None })
        }
    }

    /// Set a key value pair in this context, if it's not already set.
    pub fn set<S1: Into<SharedString>, S2: Into<SharedString>>(&mut self, key: S1, value: S2) {
        let key = key.into();
        if !self.contains(&key) {
            self.entries_mut().push(ContextEntry {
                key,
                value: Some(value.into()),
            })
        }
    }

    /// Check if this context contains a given identifier or key.
    pub fn contains(&self, key: &str) -> bool {
        self.entries().iter().any(|entry| entry.key.as_ref() == key)
    }

    /// Get the associated value for a given identifier or key.
    pub fn get(&self, key: &str) -> Option<&SharedString> {
        self.entries()
            .iter()
            .find(|entry| entry.key.as_ref() == key)?
            .value
            .as_ref()
    }
}

impl fmt::Debug for KeyContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut entries = self.entries().iter().peekable();
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


#[cfg(test)]
mod shared_context_tests {
    use super::*;
    use std::hash::{Hash, Hasher};

    #[test]
    fn contexts_share_entries_and_detach_only_on_mutation() {
        let mut source = KeyContext::default();
        assert!(source.is_empty());
        source.add("Editor");
        source.set("mode", "insert");
        let mut copy = source.clone();
        assert!(Arc::ptr_eq(source.0.as_ref().unwrap(), copy.0.as_ref().unwrap()));
        copy.add("Editor"); // No mutation; keep allocation identity.
        assert!(Arc::ptr_eq(source.0.as_ref().unwrap(), copy.0.as_ref().unwrap()));
        copy.set("extra", "1");
        assert!(!Arc::ptr_eq(source.0.as_ref().unwrap(), copy.0.as_ref().unwrap()));
        assert!(!source.contains("extra"));
        let mut extended = KeyContext::default();
        extended.extend(&source);
        assert!(Arc::ptr_eq(source.0.as_ref().unwrap(), extended.0.as_ref().unwrap()));
        copy.clear();
        assert_eq!(copy, KeyContext::default());
        let hash = |context: &KeyContext| {
            let mut state = std::collections::hash_map::DefaultHasher::new();
            context.hash(&mut state);
            state.finish()
        };
        assert_eq!(hash(&copy), hash(&KeyContext::default()));
        assert_eq!(source, extended);
        assert_eq!(hash(&source), hash(&extended));
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn empty_key_context_is_one_pointer() {
        assert_eq!(std::mem::size_of::<KeyContext>(), 8);
        assert_eq!(std::mem::size_of::<Option<KeyContext>>(), 16);
    }
}
