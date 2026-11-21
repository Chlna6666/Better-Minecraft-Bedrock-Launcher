//! Job lifecycle shared by all public BMCBL service exports.

mod exports;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use bmcbl_plugin_runtime::PluginError;
use serde::Serialize;

const MAX_JOBS: usize = 8;
const MAX_BYTES: usize = 4 * 1024 * 1024;
static PERMITS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
static EVENT_READERS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();

pub(super) type ServiceResult = Result<Vec<u8>, PluginError>;

#[derive(Default)]
struct Job {
    abort: Option<tokio::task::AbortHandle>,
    result: Option<ServiceResult>,
}

#[derive(Default)]
struct State {
    jobs: BTreeMap<String, Job>,
    revoked: bool,
}

pub(super) struct ServiceJobs {
    generation: u64,
    state: Arc<Mutex<State>>,
    plugin_id: String,
    native: Arc<super::native::Bridge>,
}

impl ServiceJobs {
    pub(super) fn new(manifest: &super::manifest::PluginManifest, generation: u64) -> Self {
        Self {
            generation,
            state: Arc::default(),
            native: Arc::new(super::native::Bridge::new(
                manifest.id.clone(),
                generation,
                manifest
                    .root_dir
                    .join(super::manifest::PLUGIN_MANIFEST_FILE),
            )),
            plugin_id: manifest.id.clone(),
        }
    }

    pub(super) fn submit(
        &self,
        manifest: &super::manifest::PluginManifest,
        service: &str,
        arguments: Vec<u8>,
    ) -> Result<String, PluginError> {
        let service = exports::names()
            .iter()
            .copied()
            .find(|name| *name == service)
            .or((service == "download.fetch-text").then_some("download.fetch-text"))
            .or((service == "download.verified").then_some("download.verified"))
            .or((service == "game-mod.call").then_some("game-mod.call"))
            .or((service == "game-mod.read-events").then_some("game-mod.read-events"))
            .or_else(|| {
                super::native::OPERATIONS
                    .iter()
                    .copied()
                    .find(|name| *name == service)
            })
            .ok_or_else(|| PluginError::not_found("unknown public BMCBL service"))?;
        let permit_pool = if service == "native.read-events" {
            EVENT_READERS.get_or_init(|| Arc::new(tokio::sync::Semaphore::new(64)))
        } else {
            PERMITS.get_or_init(|| Arc::new(tokio::sync::Semaphore::new(MAX_JOBS)))
        };
        let permit = permit_pool
            .clone()
            .try_acquire_owned()
            .map_err(|_| PluginError::new("busy", "BMCBL service workers are busy"))?;
        let mut state = lock(&self.state);
        if state.revoked || state.jobs.len() >= MAX_JOBS {
            return Err(PluginError::new(
                "busy",
                "retrieve or cancel outstanding service jobs",
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let job_id = id.clone();
        let plugin_id = self.plugin_id.clone();
        let generation = self.generation;
        let jobs = self.state.clone();
        let native = self.native.clone();
        let manifest = (service.starts_with("download.") || service.starts_with("game-mod."))
            .then(|| manifest.clone());
        let task = crate::tasks::runtime::spawn_io(async move {
            let _permit = permit;
            let timeout = if matches!(service, "native.stage-download" | "native.install-download") {
                600
            } else {
                30
            };
            let result = tokio::time::timeout(Duration::from_secs(timeout), async {
                if service == "download.verified" {
                    let manifest = manifest.as_ref().expect("download service has a manifest");
                    let request: crate::downloads::verified::Request = postcard::from_bytes(&arguments)
                        .map_err(|error| PluginError::invalid_input(error.to_string()))?;
                    use super::manifest::PluginCapability;
                    if !manifest.has_capability(&PluginCapability::DownloadStart)
                        || request.urls.iter().any(|url| !manifest.allows_network_url(url))
                    {
                        return Err(PluginError::denied("URL is outside the plugin download permissions"));
                    }
                    let task = crate::downloads::verified::start(request).map_err(PluginError::app)?;
                    native.register_download(&task);
                    encode(&task)
                } else if service == "download.fetch-text" && let Some(manifest) = manifest.as_ref() {
                    let (url,): (&str,) = postcard::from_bytes(&arguments)
                        .map_err(|error| PluginError::invalid_input(error.to_string()))?;
                    use super::manifest::PluginCapability;
                    if !manifest.has_capability(&PluginCapability::NetworkHttp)
                        || !manifest.allows_network_url(url)
                    {
                        return Err(PluginError::denied("URL is outside the plugin HTTP permissions"));
                    }
                    let (text, _) = super::runtime::fetch_http_text(
                        url,
                        manifest.limits.max_http_bytes as usize,
                        Some(manifest),
                    ).await.map_err(PluginError::app)?;
                    encode(&text)
                } else if service == "game-mod.read-events" {
                    use super::manifest::PluginCapability;
                    let manifest = manifest.as_ref().expect("game-mod.read-events requires manifest");
                    if !manifest.has_capability(&PluginCapability::GameModCall) {
                        return Err(PluginError::denied("plugin lacks game-mod.call capability"));
                    }
                    if !arguments.is_empty() {
                        let _: () = postcard::from_bytes(&arguments)
                            .map_err(|error| PluginError::invalid_input(error.to_string()))?;
                    }
                    let events = crate::core::minecraft::launcher::virtual_mods::take_events(
                        &plugin_id, generation,
                    ).map_err(PluginError::app)?;
                    encode(&events)
                } else if service == "game-mod.call" {
                    use super::manifest::PluginCapability;
                    let manifest = manifest.as_ref().expect("game-mod.call requires manifest");
                    if !manifest.has_capability(&PluginCapability::GameModCall) {
                        return Err(PluginError::denied("plugin lacks game-mod.call capability"));
                    }
                    let (module_id, method, body): (String, String, Vec<u8>) =
                        postcard::from_bytes(&arguments)
                            .map_err(|error| PluginError::invalid_input(error.to_string()))?;
                    if !manifest.game_mods.iter().any(|decl| decl.id == module_id) {
                        return Err(PluginError::denied(
                            "module is not declared by the calling plugin"
                        ));
                    }
                    if body.len() > 32 * 1024 {
                        return Err(PluginError::invalid_input("native Mod RPC payload exceeds 32 KiB"));
                    }
                    let response = crate::core::minecraft::launcher::virtual_mods::call_native(
                        &plugin_id, generation, &module_id, &method, body,
                    ).await.map_err(PluginError::app)?;
                    encode(&response)
                } else if service.starts_with("native.") || service == "download.read-text" {
                    native.invoke(service, &arguments).await
                } else {
                    exports::invoke(service, &arguments).await
                }
            })
            .await
            .unwrap_or_else(|_| Err(PluginError::timeout("BMCBL service deadline exceeded")));
            if let Some(job) = lock(&jobs).jobs.get_mut(&job_id) {
                job.result = Some(result);
                super::runtime::notify_job_ready(&plugin_id, generation, job_id);
            }
        })
        .map_err(PluginError::app)?;
        state.jobs.insert(
            id.clone(),
            Job {
                abort: Some(task.abort_handle()),
                result: None,
            },
        );
        Ok(id)
    }

    pub(super) fn take(&self, id: &str) -> Result<Option<Vec<u8>>, PluginError> {
        let mut state = lock(&self.state);
        let job = state
            .jobs
            .get(id)
            .ok_or_else(|| PluginError::not_found("unknown service job"))?;
        if job.result.is_none() {
            return Ok(None);
        }
        state.jobs.remove(id).and_then(|job| job.result).transpose()
    }

    pub(super) fn register_download(&self, id: &str) {
        self.native.register_download(id);
    }

    pub(super) fn cancel(&self, id: &str) -> Result<(), PluginError> {
        let job = lock(&self.state)
            .jobs
            .remove(id)
            .ok_or_else(|| PluginError::not_found("unknown service job"))?;
        abort(job);
        Ok(())
    }

    pub(super) fn revoke(&self) {
        let mut state = lock(&self.state);
        if state.revoked {
            return;
        }
        state.revoked = true;
        for job in std::mem::take(&mut state.jobs).into_values() {
            abort(job);
        }
        self.native.clone().close();
    }
}

impl Drop for ServiceJobs {
    fn drop(&mut self) {
        self.revoke();
    }
}

fn abort(job: Job) {
    if let Some(handle) = job.abort {
        handle.abort();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

pub(super) fn encode(value: &impl Serialize) -> ServiceResult {
    let size = postcard::experimental::serialized_size(value)
        .map_err(|error| PluginError::app(error.to_string()))?;
    if size > MAX_BYTES {
        return Err(PluginError::invalid_input("service result exceeds 4 MiB"));
    }
    postcard::to_allocvec(value).map_err(|error| PluginError::app(error.to_string()))
}
