use std::{fmt, str::FromStr};

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use thiserror::Error;
use unic_langid::LanguageIdentifier;

/// A reverse-domain application identifier shared by desktop platforms.
#[derive(Clone, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd)]
#[schemars(transparent)]
pub struct ApplicationId(String);

impl ApplicationId {
    /// Parses and normalizes a reverse-domain identifier.
    ///
    /// # Errors
    ///
    /// Returns an error for identifiers that cannot be mapped safely to all
    /// supported desktop platforms.
    pub fn new(value: impl Into<String>) -> Result<Self, ApplicationIdError> {
        let value = value.into();
        validate_application_id(&value)?;
        Ok(Self(value))
    }

    /// Returns the normalized identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ApplicationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for ApplicationId {
    type Err = ApplicationIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl Serialize for ApplicationId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ApplicationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Validation failures for [`ApplicationId`].
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ApplicationIdError {
    /// An identifier must contain at least two domain components.
    #[error("application id must contain at least two dot-separated components")]
    MissingDomain,
    /// A component was empty or contained a character unsupported by a target platform.
    #[error("application id component `{0}` is invalid")]
    InvalidComponent(String),
}

fn validate_application_id(value: &str) -> Result<(), ApplicationIdError> {
    let components = value.split('.').collect::<Vec<_>>();
    if components.len() < 2 {
        return Err(ApplicationIdError::MissingDomain);
    }

    for component in components {
        let valid = !component.is_empty()
            && !component.starts_with('-')
            && !component.ends_with('-')
            && component
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
        if !valid {
            return Err(ApplicationIdError::InvalidComponent(component.to_owned()));
        }
    }
    Ok(())
}

/// A normalized BCP 47 language identifier.
#[derive(Clone, Debug, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd)]
#[schemars(transparent)]
pub struct LocaleId(String);

impl LocaleId {
    /// Parses and canonicalizes a BCP 47 language identifier.
    ///
    /// # Errors
    ///
    /// Returns an error when the identifier is not valid Unicode locale syntax.
    pub fn new(value: impl AsRef<str>) -> Result<Self, LocaleIdError> {
        let parsed = value
            .as_ref()
            .parse::<LanguageIdentifier>()
            .map_err(|_| LocaleIdError(value.as_ref().to_owned()))?;
        Ok(Self(parsed.to_string()))
    }

    /// Returns the canonical language identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LocaleId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for LocaleId {
    type Err = LocaleIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl Serialize for LocaleId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for LocaleId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// A malformed locale identifier.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("`{0}` is not a valid BCP 47 language identifier")]
pub struct LocaleIdError(pub String);

#[cfg(test)]
mod tests {
    use super::{ApplicationId, LocaleId};

    #[test]
    fn normalizes_identifiers() {
        assert_eq!(
            ApplicationId::new("com.example.App")
                .expect("application id")
                .as_str(),
            "com.example.App"
        );
        assert_eq!(
            LocaleId::new("ZH-hans-cn").expect("locale").as_str(),
            "zh-Hans-CN"
        );
    }

    #[test]
    fn rejects_non_portable_application_ids() {
        assert!(ApplicationId::new("single").is_err());
        assert!(ApplicationId::new("com.example_bad.app").is_err());
        assert!(ApplicationId::new("com..app").is_err());
    }
}
