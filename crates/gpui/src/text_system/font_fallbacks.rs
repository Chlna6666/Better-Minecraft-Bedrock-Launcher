use std::{collections::HashSet, sync::Arc};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::SharedString;

/// The fallback fonts that can be configured for a given font.
/// Fallback fonts family names are stored here.
#[derive(Default, Clone, Eq, PartialEq, Hash, Debug, Deserialize, Serialize, JsonSchema)]
pub struct FontFallbacks(pub Arc<Vec<SharedString>>);

impl FontFallbacks {
    /// Get the fallback font family names.
    pub fn fallback_list(&self) -> &[SharedString] {
        self.0.as_slice()
    }

    /// Create a fallback list, removing blank and duplicate family names.
    pub fn from_fonts(fonts: Vec<String>) -> Self {
        Self::from_families(fonts.into_iter().map(SharedString::from).collect())
    }

    /// Create a fallback list from shared family names without copying their contents.
    pub fn from_families(fonts: Vec<SharedString>) -> Self {
        let mut seen = HashSet::with_capacity(fonts.len());
        let fonts = fonts
            .into_iter()
            .filter_map(|font| {
                let trimmed = font.trim();
                if trimmed.is_empty() || !seen.insert(trimmed.to_lowercase()) {
                    None
                } else if trimmed.len() == font.len() {
                    Some(font)
                } else {
                    Some(SharedString::from(trimmed.to_owned()))
                }
            })
            .collect();
        Self(Arc::new(fonts))
    }

    /// Returns whether this fallback list contains no font families.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::FontFallbacks;

    #[test]
    fn from_fonts_removes_blank_and_case_insensitive_duplicates() {
        let fallbacks = FontFallbacks::from_fonts(vec![
            " Noto Sans ".to_owned(),
            String::new(),
            "noto sans".to_owned(),
            "Segoe UI".to_owned(),
        ]);

        assert_eq!(fallbacks.fallback_list(), ["Noto Sans", "Segoe UI"]);
    }
}
