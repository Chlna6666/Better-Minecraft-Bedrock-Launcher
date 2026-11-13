#![doc = "Versioned application manifest schema shared by egpui tooling and runtime."]
#![deny(missing_docs)]

mod identifiers;
mod model;
mod validation;

pub use identifiers::{ApplicationId, ApplicationIdError, LocaleId, LocaleIdError};
pub use model::{
    AppManifest, ApplicationMetadata, BundleManifest, I18nManifest, IconManifest, LinuxBundle,
    MacOsBundle, PackageFormat, ResourceManifest, RuntimeManifest, WindowManifest, WindowsBundle,
};
pub use validation::{ManifestIssue, ManifestValidationError};

/// The only manifest schema currently accepted by this version of the framework.
pub const SUPPORTED_SCHEMA_VERSION: u32 = 1;
