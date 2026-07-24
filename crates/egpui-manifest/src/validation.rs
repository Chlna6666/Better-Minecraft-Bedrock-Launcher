use std::{collections::BTreeSet, fmt, path::Component};

use crate::{AppManifest, PackageFormat, SUPPORTED_SCHEMA_VERSION};

/// One actionable manifest validation problem.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestIssue {
    /// Dotted field path.
    pub path: String,
    /// Human-readable correction guidance.
    pub message: String,
}

impl ManifestIssue {
    fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

/// One or more semantic manifest validation failures.
#[derive(Clone, Debug)]
pub struct ManifestValidationError {
    issues: Vec<ManifestIssue>,
}

impl ManifestValidationError {
    /// Returns all validation failures in deterministic field order.
    #[must_use]
    pub fn issues(&self) -> &[ManifestIssue] {
        &self.issues
    }
}

impl fmt::Display for ManifestValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "application manifest contains {} validation issue(s)",
            self.issues.len()
        )
    }
}

impl std::error::Error for ManifestValidationError {}

impl AppManifest {
    /// Validates cross-field and cross-platform manifest invariants.
    ///
    /// # Errors
    ///
    /// Returns every deterministic validation issue rather than stopping at
    /// the first field.
    pub fn validate(&self) -> Result<(), ManifestValidationError> {
        let mut issues = Vec::new();
        self.validate_schema(&mut issues);
        self.validate_application(&mut issues);
        self.validate_runtime(&mut issues);
        self.validate_resources(&mut issues);
        self.validate_i18n(&mut issues);
        self.validate_windows(&mut issues);
        self.validate_bundle(&mut issues);

        if issues.is_empty() {
            Ok(())
        } else {
            issues.sort_by(|left, right| left.path.cmp(&right.path));
            Err(ManifestValidationError { issues })
        }
    }

    fn validate_schema(&self, issues: &mut Vec<ManifestIssue>) {
        if self.schema_version != SUPPORTED_SCHEMA_VERSION {
            issues.push(ManifestIssue::new(
                "schema_version",
                format!(
                    "expected schema version {SUPPORTED_SCHEMA_VERSION}, found {}",
                    self.schema_version
                ),
            ));
        }
    }

    fn validate_application(&self, issues: &mut Vec<ManifestIssue>) {
        validate_display_text("application.name", &self.application.name, issues);
        validate_display_text("application.publisher", &self.application.publisher, issues);
        if let Some(binary_name) = &self.application.binary_name {
            validate_file_name("application.binary_name", binary_name, issues);
        }
    }

    fn validate_runtime(&self, issues: &mut Vec<ManifestIssue>) {
        if self.runtime.provider.trim().is_empty() {
            issues.push(ManifestIssue::new(
                "runtime.provider",
                "runtime provider cannot be empty",
            ));
        }
        if self.runtime.shutdown_timeout_seconds == 0 {
            issues.push(ManifestIssue::new(
                "runtime.shutdown_timeout_seconds",
                "shutdown timeout must be greater than zero",
            ));
        }
        if self.runtime.ui_queue_capacity == 0 {
            issues.push(ManifestIssue::new(
                "runtime.ui_queue_capacity",
                "UI queue capacity must be greater than zero",
            ));
        }
    }

    fn validate_resources(&self, issues: &mut Vec<ManifestIssue>) {
        if self
            .resources
            .namespace
            .as_deref()
            .is_some_and(|namespace| {
                namespace.is_empty()
                    || !namespace.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                    })
            })
        {
            issues.push(ManifestIssue::new(
                "resources.namespace",
                "resource namespace must use lowercase ASCII letters, digits, or hyphens",
            ));
        }
        for (kind, patterns) in [
            ("embedded", &self.resources.embedded),
            ("bundled", &self.resources.bundled),
        ] {
            for (index, pattern) in patterns.iter().enumerate() {
                if pattern.trim().is_empty()
                    || pattern.contains('\\')
                    || pattern.starts_with('/')
                    || pattern.split('/').any(|component| component == "..")
                {
                    issues.push(ManifestIssue::new(
                        format!("resources.{kind}[{index}]"),
                        "resource patterns must be relative, use `/`, and cannot contain `..`",
                    ));
                }
            }
        }

        for (index, path) in self.resources.development_overlays.iter().enumerate() {
            if path.is_absolute()
                || path
                    .components()
                    .any(|component| component == Component::ParentDir)
            {
                issues.push(ManifestIssue::new(
                    format!("resources.development_overlays[{index}]"),
                    "development overlays must be project-relative and cannot contain `..`",
                ));
            }
        }
    }

    fn validate_windows(&self, issues: &mut Vec<ManifestIssue>) {
        for (name, window) in &self.windows {
            if name.trim().is_empty() {
                issues.push(ManifestIssue::new(
                    "windows",
                    "window identifiers cannot be empty",
                ));
            }
            if window.width == 0 || window.height == 0 {
                issues.push(ManifestIssue::new(
                    format!("windows.{name}"),
                    "window width and height must be greater than zero",
                ));
            }
        }
    }

    fn validate_i18n(&self, issues: &mut Vec<ManifestIssue>) {
        let locales = self
            .i18n
            .locales
            .iter()
            .map(|locale| locale.as_str())
            .collect::<BTreeSet<_>>();
        if locales.len() != self.i18n.locales.len() {
            issues.push(ManifestIssue::new(
                "i18n.locales",
                "locale identifiers must be unique",
            ));
        }
        if !locales.contains(self.i18n.source_locale.as_str()) {
            issues.push(ManifestIssue::new(
                "i18n.source_locale",
                "source locale must be included in i18n.locales",
            ));
        }
        if !locales.contains(self.application.default_locale.as_str()) {
            issues.push(ManifestIssue::new(
                "application.default_locale",
                "default locale must be included in i18n.locales",
            ));
        }
        if !self.i18n.catalog_pattern.contains("{locale}")
            || self.i18n.catalog_pattern.contains('\\')
            || self.i18n.catalog_pattern.starts_with('/')
            || self
                .i18n
                .catalog_pattern
                .split('/')
                .any(|component| component == "..")
        {
            issues.push(ManifestIssue::new(
                "i18n.catalog_pattern",
                "catalog pattern must contain `{locale}`, be relative, and use `/`",
            ));
        }
    }

    fn validate_bundle(&self, issues: &mut Vec<ManifestIssue>) {
        let unique_targets = self
            .bundle
            .targets
            .iter()
            .copied()
            .collect::<BTreeSet<PackageFormat>>();
        if unique_targets.len() != self.bundle.targets.len() {
            issues.push(ManifestIssue::new(
                "bundle.targets",
                "package targets must be unique",
            ));
        }
        for (path, value) in [
            ("bundle.icons.source", &self.bundle.icons.source),
            ("bundle.icons.windows_ico", &self.bundle.icons.windows_ico),
            ("bundle.icons.macos_icns", &self.bundle.icons.macos_icns),
            (
                "bundle.icons.linux_png_directory",
                &self.bundle.icons.linux_png_directory,
            ),
        ] {
            if let Some(value) = value {
                if value.is_absolute()
                    || value
                        .components()
                        .any(|component| component == Component::ParentDir)
                {
                    issues.push(ManifestIssue::new(
                        path,
                        "bundle paths must be project-relative and cannot contain `..`",
                    ));
                }
            }
        }
        if self
            .bundle
            .windows
            .execution_level
            .as_deref()
            .is_some_and(|level| {
                !matches!(
                    level,
                    "asInvoker" | "highestAvailable" | "requireAdministrator"
                )
            })
        {
            issues.push(ManifestIssue::new(
                "bundle.windows.execution_level",
                "execution level must be asInvoker, highestAvailable, or requireAdministrator",
            ));
        }
        if self.bundle.targets.contains(&PackageFormat::WindowsMsi)
            && self.bundle.windows.upgrade_code.is_none()
        {
            issues.push(ManifestIssue::new(
                "bundle.windows.upgrade_code",
                "Windows MSI targets require a stable WiX upgrade code",
            ));
        }
    }
}

fn validate_display_text(path: &str, value: &str, issues: &mut Vec<ManifestIssue>) {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        issues.push(ManifestIssue::new(
            path,
            "value cannot be empty or contain control characters",
        ));
    }
}

fn validate_file_name(path: &str, value: &str, issues: &mut Vec<ManifestIssue>) {
    let invalid = value.trim().is_empty()
        || value
            .chars()
            .any(|character| character.is_control() || r#"\/:*?"<>|"#.contains(character));
    if invalid || value == "." || value == ".." {
        issues.push(ManifestIssue::new(
            path,
            "binary name contains characters unsupported by a target platform",
        ));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use semver::Version;

    use crate::{
        AppManifest, ApplicationId, ApplicationMetadata, BundleManifest, I18nManifest, LocaleId,
        ResourceManifest, RuntimeManifest,
    };

    fn valid_manifest() -> AppManifest {
        AppManifest {
            schema_version: 1,
            application: ApplicationMetadata {
                id: ApplicationId::new("com.example.app").expect("id"),
                name: "Example".to_owned(),
                version: Version::new(1, 0, 0),
                publisher: "Example".to_owned(),
                copyright: None,
                default_locale: LocaleId::new("en-US").expect("locale"),
                binary_name: Some("example".to_owned()),
            },
            runtime: RuntimeManifest::default(),
            resources: ResourceManifest::default(),
            i18n: I18nManifest {
                source_locale: LocaleId::new("en-US").expect("locale"),
                locales: vec![LocaleId::new("en-US").expect("locale")],
                catalog_pattern: "locales/{locale}/main.ftl".to_owned(),
            },
            windows: BTreeMap::new(),
            bundle: BundleManifest::default(),
        }
    }

    #[test]
    fn accepts_valid_manifest() {
        valid_manifest().validate().expect("valid manifest");
    }

    #[test]
    fn reports_cross_field_errors_together() {
        let mut manifest = valid_manifest();
        manifest.schema_version = 99;
        manifest.runtime.ui_queue_capacity = 0;

        let error = manifest.validate().expect_err("invalid manifest");
        assert!(error.issues().len() >= 2);
        assert!(
            error
                .issues()
                .iter()
                .any(|issue| issue.path == "schema_version")
        );
    }
}
