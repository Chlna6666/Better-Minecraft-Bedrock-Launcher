//! Calls to Rust functions exported by the BMCBL host.

use serde::{Serialize, de::DeserializeOwned};

use crate::{PluginError, PluginResult, bindings::bmcbl::plugin::host_app};

/// A cancellable operation running on BMCBL's runtime.
pub struct Job<T> {
    id: String,
    result: std::marker::PhantomData<T>,
}

/// Reads a small completed download submitted by the current plugin generation.
pub fn read_download_text(task_id: &str) -> PluginResult<Job<String>> {
    invoke("download.read-text", &(task_id,))
}

/// Fetches small text directly from an allowed HTTPS source, including allowed redirects.
pub fn fetch_download_text(url: &str) -> PluginResult<Job<String>> {
    invoke("download.fetch-text", &(url,))
}

/// Sources in priority order, all serving the same bytes.
#[derive(Clone, Debug, serde::Deserialize, Serialize)]
pub struct VerifiedDownload {
    pub urls: Vec<String>,
    pub filename: String,
    pub sha256: String,
    pub size: u64,
}

/// Starts a BMCBL transfer which checks each source and returns its task ID.
pub fn download_verified(request: &VerifiedDownload) -> PluginResult<Job<String>> {
    invoke("download.verified", request)
}

impl<T: DeserializeOwned> Job<T> {
    /// Returns whether BMCBL delivered the completion event for this job.
    pub fn is_ready(&self, event: &crate::PluginEvent) -> bool {
        matches!(&event.kind, crate::PluginEventKind::JobReady(event) if event.job_id == self.id)
    }

    /// Retrieves the result once. `None` means the operation is still running.
    pub fn try_take(&self) -> PluginResult<Option<T>> {
        host_app::take(&self.id)?
            .map(|bytes| {
                postcard::from_bytes(&bytes)
                    .map_err(|error| PluginError::new("invalid-result", error.to_string()))
            })
            .transpose()
    }

    /// Cancels the operation and discards any unread result.
    pub fn cancel(self) -> PluginResult<()> {
        host_app::cancel(&self.id)
    }
}

pub(crate) fn invoke<T: DeserializeOwned>(name: &str, arguments: &impl Serialize) -> PluginResult<Job<T>> {
    let bytes = postcard::to_allocvec(arguments)
        .map_err(|error| PluginError::new("invalid-input", error.to_string()))?;
    let id = host_app::invoke(name, &bytes)?;
    Ok(Job {
        id,
        result: std::marker::PhantomData,
    })
}

/// One catalogue drives the Rust SDK and the native call table. The SDK expansion
/// uses only signatures; the launcher expansion compiles the existing Rust calls.
#[doc(hidden)]
#[macro_export]
macro_rules! bmcbl_app_services {
    ($emit:ident) => {
        $emit! {
            system {
                /// Information about the running BMCBL build.
                fn build_info() -> String = "system.build-info"
                    => direct(crate::utils::app_info::build_info);
            }
            json {
                /// Validate and compact JSON using BMCBL's parser.
                fn normalize(text: &str) -> String = "json.normalize" => fallible(|text| {
                    serde_json::from_str::<serde_json::Value>(text)
                        .and_then(|value| serde_json::to_string(&value))
                });
                /// Select a JSON fragment using a JSON Pointer.
                fn select(text: &str, pointer: &str) -> Option<String> = "json.select" => fallible(|text, pointer| {
                    serde_json::from_str::<serde_json::Value>(text)
                        .and_then(|value| value.pointer(pointer).map(serde_json::to_string).transpose())
                });
            }
            world {
                /// Check whether Bedrock generates slimes at the chunk coordinates.
                fn is_slime_chunk(chunk_x: i32, chunk_z: i32) -> bool = "bedrock-world.query.is-slime-chunk"
                    => direct(bedrock_world::query::is_bedrock_slime_chunk);
            }
            map_render {
                /// Compute the native renderer's tile payload fingerprint.
                fn tile_payload_fingerprint(bytes: &[u8]) -> u128 = "bedrock-map-render.cache.tile-payload-fingerprint"
                    => direct(bedrock_map_render::cache::tile_payload_fingerprint);
            }
        }
    };
}

macro_rules! sdk {
    ($($module:ident { $($(#[$doc:meta])* fn $method:ident(
        $($arg:ident : $ty:ty),*
    ) -> $result:ty = $service:literal => $kind:ident($target:expr);)*
    })*) => {
        $(
            pub mod $module {
                $(
                    $(#[$doc])*
                    pub fn $method($($arg: $ty),*) -> crate::PluginResult<super::Job<$result>> {
                        super::invoke($service, &($($arg,)*))
                    }
                )*
            }
        )*
    };
}
crate::bmcbl_app_services!(sdk);

