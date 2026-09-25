use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::{
    borrow::{Borrow, Cow},
    sync::Arc,
};

/// A shared string is an immutable string that can be cheaply cloned in GPUI tasks.
///
/// `SmolStr` keeps short UI labels inline while retaining cheap clones for longer strings, which
/// avoids the heap allocation/atomic Arc traffic that the previous `ArcCow<str>` representation
/// paid for every non-static short string.
#[derive(Eq, PartialEq, PartialOrd, Ord, Hash, Clone)]
pub struct SharedString(SmolStr);

impl std::ops::Deref for SharedString {
    type Target = str;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.0.as_str()
    }
}

impl SharedString {
    /// Creates a static [`SharedString`] from a `&'static str`.
    pub const fn new_static(str: &'static str) -> Self {
        Self(SmolStr::new_static(str))
    }

    /// Creates a [`SharedString`], inlining short strings when possible.
    #[inline]
    pub fn new(str: impl AsRef<str>) -> Self {
        Self(SmolStr::new(str))
    }

    /// Get a `&str` from the underlying string.
    #[inline]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl JsonSchema for SharedString {
    fn inline_schema() -> bool {
        String::inline_schema()
    }

    fn schema_name() -> Cow<'static, str> {
        String::schema_name()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        String::json_schema(generator)
    }
}

impl Default for SharedString {
    fn default() -> Self {
        Self::new_static("")
    }
}

impl AsRef<str> for SharedString {
    #[inline]
    fn as_ref(&self) -> &str {
        self.0.as_str()
    }
}

impl Borrow<str> for SharedString {
    #[inline]
    fn borrow(&self) -> &str {
        self.as_ref()
    }
}

impl std::fmt::Debug for SharedString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::fmt::Display for SharedString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl PartialEq<String> for SharedString {
    fn eq(&self, other: &String) -> bool {
        self.as_ref() == other
    }
}

impl PartialEq<SharedString> for String {
    fn eq(&self, other: &SharedString) -> bool {
        self == other.as_ref()
    }
}

impl PartialEq<str> for SharedString {
    fn eq(&self, other: &str) -> bool {
        self.as_ref() == other
    }
}

impl<'a> PartialEq<&'a str> for SharedString {
    fn eq(&self, other: &&'a str) -> bool {
        self.as_ref() == *other
    }
}

impl From<&SharedString> for SharedString {
    #[inline]
    fn from(value: &SharedString) -> Self {
        value.clone()
    }
}

impl From<&str> for SharedString {
    #[inline]
    fn from(value: &str) -> Self {
        Self(SmolStr::from(value))
    }
}

impl From<&mut str> for SharedString {
    #[inline]
    fn from(value: &mut str) -> Self {
        Self(SmolStr::from(value))
    }
}

impl From<&String> for SharedString {
    #[inline]
    fn from(value: &String) -> Self {
        Self(SmolStr::from(value))
    }
}

impl From<String> for SharedString {
    #[inline(always)]
    fn from(value: String) -> Self {
        Self(SmolStr::from(value))
    }
}

impl From<Box<str>> for SharedString {
    #[inline]
    fn from(value: Box<str>) -> Self {
        Self(SmolStr::from(value))
    }
}

impl From<Arc<str>> for SharedString {
    #[inline]
    fn from(value: Arc<str>) -> Self {
        Self(SmolStr::from(value))
    }
}

impl From<&Arc<str>> for SharedString {
    #[inline]
    fn from(value: &Arc<str>) -> Self {
        Self(SmolStr::from(value.clone()))
    }
}

impl<'a> From<Cow<'a, str>> for SharedString {
    #[inline]
    fn from(value: Cow<'a, str>) -> Self {
        Self(SmolStr::from(value))
    }
}

impl From<SharedString> for Arc<str> {
    #[inline(always)]
    fn from(value: SharedString) -> Self {
        value.0.into()
    }
}

impl From<SharedString> for String {
    #[inline(always)]
    fn from(value: SharedString) -> Self {
        value.0.into()
    }
}

impl Serialize for SharedString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_ref())
    }
}

impl<'de> Deserialize<'de> for SharedString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(Self::from(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_string_preserves_static_and_owned_conversions() {
        let static_text = SharedString::new_static("settings");
        let owned = SharedString::from(String::from("download"));

        assert_eq!(static_text.as_str(), "settings");
        assert_eq!(owned.as_str(), "download");
        assert_eq!(String::from(owned.clone()), "download");
        assert_eq!(Arc::<str>::from(owned).as_ref(), "download");
    }
}
