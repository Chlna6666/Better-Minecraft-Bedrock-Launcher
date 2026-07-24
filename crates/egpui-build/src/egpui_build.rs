//! Build-time support for egpui applications.

mod bundle;
mod manifest_loader;
mod resource_index;

pub use bundle::{
    BundleExecutionError, BundleExecutor, BundlePlan, BundlePlanError, BundleRequest,
    BundleResource, BundleStep, BundlerBackend, HostPlatform, NativeBundleExecutor,
    NativeBundlerBackend,
};
pub use manifest_loader::{
    ManifestLoadError, load_manifest, load_manifest_from_str, manifest_schema_json,
};
pub use resource_index::{
    IndexedResource, ResourceIndex, ResourceIndexError, ResourceSource, build_resource_index,
    render_embedded_resource_module,
};
