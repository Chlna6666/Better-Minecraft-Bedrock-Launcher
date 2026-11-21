//! A plugin generation's module and instance table. The host never decodes commands or events.
#![expect(unsafe_code, reason = "Audited native plugin C ABI calls and wake callbacks require unsafe FFI")]

mod module;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bmcbl_plugin_api::native::{InstanceHandle, ModuleHandle, Platform, RequestId, abi};
use bmcbl_plugin_runtime::PluginError;
use serde::Deserialize;
use tokio::sync::{Notify, Semaphore};

use self::module::Module;

const MAX_MODULES: usize = 4;
const MAX_INSTANCES: usize = 16;
pub(super) const OPERATIONS: &[&str] = &[
    "download.read-text",
    "native.platform",
    "native.stage-download",
    "native.install-download",
    "native.load",
    "native.create-instance",
    "native.submit-command",
    "native.read-events",
    "native.cancel-command",
    "native.destroy-instance",
    "native.unload",
];
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);

pub(super) struct Bridge {
    plugin: String,
    generation: u64,
    manifest: PathBuf,
    closed: AtomicBool,
    table: Mutex<Table>,
    downloads: Mutex<BTreeSet<String>>,
}

#[derive(Default)]
struct Table {
    modules: BTreeMap<ModuleHandle, (String, Arc<Module>)>,
    instances: BTreeMap<InstanceHandle, Instance>,
}

struct Instance {
    module_id: ModuleHandle,
    module: Arc<Module>,
    native_id: u64,
    ready: Arc<Notify>,
    readers: Arc<Semaphore>,
    buffer: Vec<u8>,
}

impl Bridge {
    pub(super) fn new(plugin: String, generation: u64, manifest: PathBuf) -> Self {
        Self {
            plugin,
            generation,
            manifest,
            closed: AtomicBool::new(false),
            table: Mutex::default(),
            downloads: Mutex::default(),
        }
    }

    pub(super) async fn invoke(
        self: Arc<Self>,
        operation: &str,
        data: &[u8],
    ) -> super::app_services::ServiceResult {
        if operation == "native.platform" {
            decode::<()>(data)?;
            return super::app_services::encode(&Platform {
                os: std::env::consts::OS.to_owned(),
                arch: std::env::consts::ARCH.to_owned(),
                environment: if cfg!(target_env = "musl") {
                    "musl"
                } else if cfg!(target_env = "gnu") {
                    "gnu"
                } else if cfg!(target_env = "msvc") {
                    "msvc"
                } else {
                    ""
                }
                .to_owned(),
            });
        }
        if operation == "download.read-text" {
            let (task,): (&str,) = decode(data)?;
            let path = self.wait_for_download(task).await?;
            return tokio::task::spawn_blocking(move || {
                let _table = self.table()?;
                super::app_services::encode(&module::read_download_text(&path)?)
            })
            .await
            .map_err(|error| PluginError::app(error.to_string()))?;
        }
        if matches!(operation, "native.stage-download" | "native.install-download") {
            let (task, sha): (&str, &str) = decode(data)?;
            self.clone().stage_download(task, sha).await?;
            if operation == "native.install-download" {
                super::runtime::confirm_native_download(&self.plugin, self.generation, sha).await?;
                let sha = sha.to_owned();
                tokio::task::spawn_blocking(move || {
                    let _table = self.table()?;
                    module::approve_staged(&self.plugin, &self.manifest, &sha)
                })
                .await
                .map_err(|error| PluginError::app(error.to_string()))??;
            }
            return super::app_services::encode(&());
        }
        if operation == "native.read-events" {
            let (instance, wait_ms) = decode(data)?;
            return super::app_services::encode(&self.read_events(instance, wait_ms).await?);
        }
        let operation = operation.to_owned();
        let data = data.to_vec();
        tokio::task::spawn_blocking(move || self.command(&operation, &data))
            .await
            .map_err(|error| PluginError::app(error.to_string()))?
    }

    pub(super) fn register_download(&self, task: &str) {
        if let Ok(mut downloads) = self.downloads.lock()
            && downloads.len() < 64
        {
            downloads.insert(task.to_owned());
        }
    }

    async fn wait_for_download(&self, task: &str) -> Result<PathBuf, PluginError> {
        if !self
            .downloads
            .lock()
            .is_ok_and(|mut downloads| downloads.remove(task))
        {
            return Err(PluginError::denied(
                "download belongs to another plugin generation",
            ));
        }
        let snapshot = crate::tasks::task_manager::wait_for_task(task)
            .await
            .map_err(PluginError::app)?;
        if snapshot.status.as_ref() != "completed" {
            return Err(PluginError::app(
                "native component download did not complete",
            ));
        }
        snapshot
            .message
            .as_deref()
            .map(PathBuf::from)
            .ok_or_else(|| PluginError::app("completed download has no file"))
    }

    async fn stage_download(self: Arc<Self>, task: &str, sha: &str) -> Result<(), PluginError> {
        let path = self.wait_for_download(task).await?;
        let sha = sha.to_owned();
        tokio::task::spawn_blocking(move || {
            // Serialize installation against module operations and generation cleanup.
            let _table = self.table()?;
            module::stage_download(&self.plugin, &self.manifest, &path, &sha)
        })
        .await
        .map_err(|error| PluginError::app(error.to_string()))?
    }

    fn command(&self, operation: &str, data: &[u8]) -> super::app_services::ServiceResult {
        use super::app_services::encode;
        let mut table = self.table()?;
        match operation {
            "native.load" => {
                let (sha256,): (&str,) = decode(data)?;
                if let Some((&id, _)) = table
                    .modules
                    .iter()
                    .find(|(_, (digest, _))| digest == sha256)
                {
                    return encode(&id);
                }
                if table.modules.len() >= MAX_MODULES {
                    return Err(PluginError::new("busy", "native module limit reached"));
                }
                let module = Module::load(&self.plugin, sha256)?;
                let id = ModuleHandle(next_id());
                table
                    .modules
                    .insert(id, (sha256.to_owned(), Arc::new(module)));
                encode(&id)
            }
            "native.create-instance" => {
                let (module_id, config) = decode(data)?;
                encode(&table.create_instance(module_id, config)?)
            }
            "native.submit-command" => {
                let (id, bytes): (InstanceHandle, &[u8]) = decode(data)?;
                let len = command_length(bytes)?;
                let instance = table.instance(id)?;
                let request = RequestId(next_id());
                let submit = instance.module.api.submit_command.expect("validated ABI");
                // SAFETY: the instance is live, calls are serialized, and the input is borrowed for the call.
                status(unsafe { submit(instance.native_id, request.0, bytes.as_ptr(), len) })?;
                encode(&request)
            }
            "native.cancel-command" => {
                let (id, request): (InstanceHandle, RequestId) = decode(data)?;
                let instance = table.instance(id)?;
                let cancel = instance.module.api.cancel_command.expect("validated ABI");
                // SAFETY: the instance is live and the table serializes calls.
                status(unsafe { cancel(instance.native_id, request.0) })?;
                encode(&())
            }
            "native.destroy-instance" => {
                let (id,) = decode(data)?;
                table.instance(id)?.destroy()?;
                table.instances.remove(&id);
                encode(&())
            }
            "native.unload" => {
                let (module,) = decode(data)?;
                if table
                    .instances
                    .values()
                    .any(|instance| instance.module_id == module)
                {
                    return Err(PluginError::new(
                        "busy",
                        "destroy native instances before unloading their module",
                    ));
                }
                table.modules.remove(&module).ok_or_else(invalid_handle)?;
                encode(&())
            }
            _ => Err(PluginError::not_found("unknown native bridge operation")),
        }
    }

    async fn read_events(
        self: Arc<Self>,
        id: InstanceHandle,
        wait_ms: u32,
    ) -> Result<Vec<u8>, PluginError> {
        let bridge = self.clone();
        let (notification, _reader) = tokio::task::spawn_blocking(move || {
            let mut table = bridge.table()?;
            let instance = table.instance(id)?;
            let reader = instance.readers.clone().try_acquire_owned().map_err(|_| {
                PluginError::new("busy", "the native instance already has an event reader")
            })?;
            Ok::<_, PluginError>((instance.ready.clone(), reader))
        })
        .await
        .map_err(|error| PluginError::app(error.to_string()))??;
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(u64::from(wait_ms.min(10_000)));
        loop {
            let wake = notification.notified();
            tokio::pin!(wake);
            wake.as_mut().enable();
            let bridge = self.clone();
            let batch =
                tokio::task::spawn_blocking(move || bridge.table()?.instance(id)?.read_events())
                    .await
                    .map_err(|error| PluginError::app(error.to_string()))??;
            if !batch.is_empty() || tokio::time::timeout_at(deadline, wake).await.is_err() {
                return Ok(batch);
            }
        }
    }

    fn table(&self) -> Result<std::sync::MutexGuard<'_, Table>, PluginError> {
        let table = self
            .table
            .lock()
            .map_err(|_| PluginError::app("native bridge state unavailable"))?;
        if self.closed.load(Ordering::Acquire) {
            return Err(PluginError::denied("native bridge session was closed"));
        }
        Ok(table)
    }

    pub(super) fn close(self: Arc<Self>) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        // If shutdown already stopped the task runtime, Table::drop pins any still-live module.
        let _ = crate::tasks::runtime::spawn_io(async move {
            let _ = tokio::task::spawn_blocking(move || {
                let mut table = self.table.lock().unwrap_or_else(|error| error.into_inner());
                for (id, instance) in std::mem::take(&mut table.instances) {
                    if let Err(error) = instance.destroy() {
                        tracing::error!(plugin = %self.plugin, instance = id.0, error = %error.message, "pin native library after failed destroy");
                        std::mem::forget(instance);
                    }
                }
                table.modules.clear();
            }).await;
        });
    }
}

pub(super) fn remove_components(plugin: &str) -> std::io::Result<()> {
    module::mark_removed(plugin)
}

pub(crate) fn approve_component(
    plugin: &str,
    source: &std::path::Path,
) -> Result<String, PluginError> {
    super::manifest::validate_plugin_id(plugin)
        .map_err(|error| PluginError::invalid_input(error.to_string()))?;
    module::approve_file(plugin, source)
}

impl Table {
    fn instance(&mut self, id: InstanceHandle) -> Result<&mut Instance, PluginError> {
        self.instances.get_mut(&id).ok_or_else(invalid_handle)
    }

    fn create_instance(
        &mut self,
        module_id: ModuleHandle,
        config: &[u8],
    ) -> Result<InstanceHandle, PluginError> {
        let len = command_length(config)?;
        if self.instances.len() >= MAX_INSTANCES {
            return Err(PluginError::new("busy", "native instance limit reached"));
        }
        let (_, module) = self.modules.get(&module_id).ok_or_else(invalid_handle)?;
        let ready = Arc::new(Notify::new());
        let wake = abi::EventWake {
            context: Arc::as_ptr(&ready).cast_mut().cast(),
            notify: Some(wake_reader),
        };
        let mut native_id = 0;
        let create = module.api.create_instance.expect("validated ABI");
        // SAFETY: the module borrows config and out-ID for the call. The table keeps ready alive until successful destroy.
        status(unsafe { create(config.as_ptr(), len, wake, &mut native_id) })?;
        let id = InstanceHandle(next_id());
        self.instances.insert(
            id,
            Instance {
                module_id,
                module: module.clone(),
                native_id,
                ready,
                readers: Arc::new(Semaphore::new(1)),
                buffer: vec![0; 4096],
            },
        );
        Ok(id)
    }
}

impl Drop for Table {
    fn drop(&mut self) {
        // Live instances are never implicitly unmapped. Cleanup ordinarily runs in Bridge::close.
        for (_, instance) in std::mem::take(&mut self.instances) {
            std::mem::forget(instance);
        }
    }
}

impl Instance {
    fn destroy(&self) -> Result<(), PluginError> {
        let destroy = self.module.api.destroy_instance.expect("validated ABI");
        // SAFETY: the instance is live. Success joins its workers before returning.
        status(unsafe { destroy(self.native_id) })?;
        self.ready.notify_waiters();
        Ok(())
    }

    fn read_events(&mut self) -> Result<Vec<u8>, PluginError> {
        let read = self.module.api.read_events.expect("validated ABI");
        for _ in 0..2 {
            let mut length = 0;
            // SAFETY: buffer and length point to live, writable storage for this call.
            let result = unsafe {
                read(
                    self.native_id,
                    self.buffer.as_mut_ptr(),
                    self.buffer.len() as u32,
                    &mut length,
                )
            };
            let length = length as usize;
            if result == abi::BUFFER_TOO_SMALL
                && (self.buffer.len() < length && length <= abi::MAX_EVENT_BATCH)
            {
                self.buffer.resize(abi::MAX_EVENT_BATCH, 0);
                continue;
            }
            status(result)?;
            return self
                .buffer
                .get(..length)
                .map(<[u8]>::to_vec)
                .ok_or_else(|| PluginError::app("native ABI returned an invalid event length"));
        }
        Err(PluginError::app(
            "native ABI changed the retained event batch",
        ))
    }
}

unsafe extern "C" fn wake_reader(context: *mut c_void) {
    // SAFETY: the ABI retains this pointer only until destroy has joined all callbacks.
    let ready = unsafe { &*context.cast::<Notify>() };
    ready.notify_one();
}

fn next_id() -> u64 {
    NEXT_HANDLE.fetch_add(1, Ordering::Relaxed)
}

fn decode<'a, T: Deserialize<'a>>(data: &'a [u8]) -> Result<T, PluginError> {
    let (value, remaining) = postcard::take_from_bytes(data)
        .map_err(|error| PluginError::invalid_input(error.to_string()))?;
    if !remaining.is_empty() {
        return Err(PluginError::invalid_input(
            "unexpected native bridge arguments",
        ));
    }
    Ok(value)
}

fn command_length(bytes: &[u8]) -> Result<u32, PluginError> {
    if bytes.len() > abi::MAX_COMMAND {
        return Err(PluginError::invalid_input("native command exceeds 64 KiB"));
    }
    Ok(bytes.len() as u32)
}

fn invalid_handle() -> PluginError {
    PluginError::not_found("native handle is closed or belongs to another plugin generation")
}

fn status(status: i32) -> Result<(), PluginError> {
    match status {
        abi::OK => Ok(()),
        abi::INVALID_ARGUMENT => Err(PluginError::invalid_input("native module rejected input")),
        abi::INVALID_HANDLE | abi::CLOSED => Err(invalid_handle()),
        abi::BUSY => Err(PluginError::new("busy", "native command queue is full")),
        abi::BUFFER_TOO_SMALL => Err(PluginError::app(
            "native event batch exceeds the allowed capacity",
        )),
        _ => Err(PluginError::app(format!(
            "native operation failed with status {status}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires BMCBL_NATIVE_FIXTURE_LIBRARY and its opaque test-command environment"]
    async fn native_library_lifecycle() {
        let source = PathBuf::from(
            std::env::var_os("BMCBL_NATIVE_FIXTURE_LIBRARY").expect("native fixture library"),
        );
        let command =
            std::env::var("BMCBL_NATIVE_FIXTURE_COMMAND").expect("native fixture command");
        let expected = std::env::var("BMCBL_NATIVE_FIXTURE_EVENT").expect("native fixture event");
        let plugin = format!("native-fixture-{}", uuid::Uuid::new_v4().simple());
        let root = module::root(&plugin);
        let bytes = std::fs::read(&source).unwrap();
        let digest = bmcbl_plugin_runtime::engine::wasm_sha256_hex(&bytes);
        let version = root.join(&digest);
        std::fs::create_dir_all(&version).unwrap();
        let path = version.join(format!(
            "{}module{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_SUFFIX
        ));
        std::fs::write(&path, bytes).unwrap();
        let bridge = Arc::new(Bridge::new(plugin, 0, root.join("plugin.toml")));
        let load = postcard::to_allocvec(&(digest.as_str(),)).unwrap();
        assert!(bridge.clone().invoke("native.load", &load).await.is_err());
        assert!(module::approve_staged(&bridge.plugin, &bridge.manifest, &digest).is_err());
        std::fs::write(&bridge.manifest, []).unwrap();
        module::approve_staged(&bridge.plugin, &bridge.manifest, &digest).unwrap();
        let module: ModuleHandle =
            postcard::from_bytes(&bridge.clone().invoke("native.load", &load).await.unwrap())
                .unwrap();
        let create = postcard::to_allocvec(&(module, &[] as &[u8])).unwrap();
        let instance: InstanceHandle = postcard::from_bytes(
            &bridge
                .clone()
                .invoke("native.create-instance", &create)
                .await
                .unwrap(),
        )
        .unwrap();
        let unload = postcard::to_allocvec(&(module,)).unwrap();
        assert!(
            bridge
                .clone()
                .invoke("native.unload", &unload)
                .await
                .is_err()
        );
        assert!(
            Arc::new(Bridge::new("other".into(), 0, root.join("other.toml")))
                .read_events(instance, 0)
                .await
                .is_err()
        );
        let submission = postcard::to_allocvec(&(instance, command.as_bytes())).unwrap();
        let request: RequestId = postcard::from_bytes(
            &bridge
                .clone()
                .invoke("native.submit-command", &submission)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_ne!(request.0, 0);
        let mut received = String::new();
        for _ in 0..5 {
            let batch = bridge.clone().read_events(instance, 500).await.unwrap();
            received.push_str(std::str::from_utf8(&batch).unwrap());
            if received.contains(&expected) {
                break;
            }
        }
        assert!(received.contains(&expected), "{received}");
        remove_components(&bridge.plugin).unwrap();
        assert!(path.is_file());
        assert!(approve_component(&bridge.plugin, &source).is_err());
        let destroy = postcard::to_allocvec(&(instance,)).unwrap();
        bridge
            .clone()
            .invoke("native.destroy-instance", &destroy)
            .await
            .unwrap();
        assert!(
            bridge
                .clone()
                .invoke("native.destroy-instance", &destroy)
                .await
                .is_err()
        );
        bridge
            .clone()
            .invoke("native.unload", &unload)
            .await
            .unwrap();
        assert!(!root.exists());
        assert_eq!(approve_component(&bridge.plugin, &source).unwrap(), digest);
        std::fs::remove_dir_all(root).unwrap();
    }
}
