//! Build-time support for egpui applications.

mod bundle;
pub mod manifest;
mod resource_index;

pub use bundle::{
    BundleExecutionError, BundleExecutor, BundlePlan, BundlePlanError, BundleRequest,
    BundleResource, BundleStep, BundlerBackend, HostPlatform, NativeBundleExecutor,
    NativeBundlerBackend,
};
pub use resource_index::{
    IndexedResource, ResourceIndex, ResourceIndexError, ResourceSource, build_resource_index,
    render_embedded_resource_module,
};
