//! Calls to a plugin's separately approved native component.

use serde::{Deserialize, Serialize};

use crate::PluginResult;
use crate::app::{Job, invoke};

/// Owned by one Wasm plugin generation.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct ModuleHandle(pub u64);

/// Host-side ID for an instance. The library's own ID stays in the host.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct InstanceHandle(pub u64);

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub struct RequestId(pub u64);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Platform {
    pub os: String,
    pub arch: String,
    pub environment: String,
}

pub fn platform() -> PluginResult<Job<Platform>> {
    invoke("native.platform", &())
}

/// Stages a completed download owned by this plugin. Loading checks user approval separately.
pub fn stage_download(task_id: &str, sha256: &str) -> PluginResult<Job<()>> {
    invoke("native.stage-download", &(task_id, sha256))
}

/// Waits for a download started by this plugin, verifies its SHA-256, and asks
/// the user to approve the staged binary.
pub fn download(task_id: &str, sha256: &str) -> PluginResult<Job<()>> {
    invoke("native.install-download", &(task_id, sha256))
}

/// Resolves a component installed and approved for this plugin and platform.
pub fn load(sha256: &str) -> PluginResult<Job<ModuleHandle>> {
    invoke("native.load", &(sha256,))
}

pub fn create_instance(module: ModuleHandle, config: &[u8]) -> PluginResult<Job<InstanceHandle>> {
    invoke("native.create-instance", &(module, config))
}

pub fn submit_command(instance: InstanceHandle, command: &[u8]) -> PluginResult<Job<RequestId>> {
    invoke("native.submit-command", &(instance, command))
}

/// Waits for a batch, or returns an empty batch when the wait expires.
pub fn read_events(instance: InstanceHandle, wait_ms: u32) -> PluginResult<Job<Vec<u8>>> {
    invoke("native.read-events", &(instance, wait_ms))
}

pub fn cancel_command(instance: InstanceHandle, request: RequestId) -> PluginResult<Job<()>> {
    invoke("native.cancel-command", &(instance, request))
}

pub fn destroy_instance(instance: InstanceHandle) -> PluginResult<Job<()>> {
    invoke("native.destroy-instance", &(instance,))
}

pub fn unload(module: ModuleHandle) -> PluginResult<Job<()>> {
    invoke("native.unload", &(module,))
}

/// In-process C ABI shared by the native library and its host. All calls for
/// a plugin are serialized by the host. Libraries may notify from worker threads.
pub mod abi {
    use core::ffi::c_void;

    pub const MAJOR: u32 = 1;
    pub const MINOR: u32 = 0;
    pub const ENTRY: &[u8] = b"bmcbl_native_entry\0";
    pub const MAX_COMMAND: usize = 64 * 1024;
    pub const MAX_EVENT_BATCH: usize = 256 * 1024;

    pub const OK: i32 = 0;
    pub const INVALID_ARGUMENT: i32 = 1;
    pub const INVALID_HANDLE: i32 = 2;
    pub const BUSY: i32 = 3;
    pub const BUFFER_TOO_SMALL: i32 = 4;
    pub const CLOSED: i32 = 5;
    pub const INTERNAL: i32 = 6;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct AbiHeader {
        pub abi_major: u32,
        pub abi_minor: u32,
        pub struct_size: u32,
        pub reserved: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct EventWake {
        pub context: *mut c_void,
        pub notify: Option<unsafe extern "C" fn(*mut c_void)>,
    }

    pub type Entry = unsafe extern "C" fn() -> *const AbiHeader;
    pub type CreateInstance = unsafe extern "C" fn(*const u8, u32, EventWake, *mut u64) -> i32;
    pub type SubmitCommand = unsafe extern "C" fn(u64, u64, *const u8, u32) -> i32;
    pub type ReadEvents = unsafe extern "C" fn(u64, *mut u8, u32, *mut u32) -> i32;
    pub type CancelCommand = unsafe extern "C" fn(u64, u64) -> i32;
    pub type DestroyInstance = unsafe extern "C" fn(u64) -> i32;

    /// The entry points to a static header followed by these fields. Inputs and
    /// outputs are caller-owned for the duration of the call. A successful create
    /// leases its wake callback until a successful destroy, which must join all
    /// workers and quiesce every callback. On insufficient event capacity, write
    /// the needed length and retain the queued batch. All functions return status.
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct NativeModuleApi {
        pub header: AbiHeader,
        pub create_instance: Option<CreateInstance>,
        pub submit_command: Option<SubmitCommand>,
        pub read_events: Option<ReadEvents>,
        pub cancel_command: Option<CancelCommand>,
        pub destroy_instance: Option<DestroyInstance>,
    }

    impl NativeModuleApi {
        pub const HEADER: AbiHeader = AbiHeader {
            abi_major: MAJOR,
            abi_minor: MINOR,
            struct_size: size_of::<Self>() as u32,
            reserved: 0,
        };
    }
}
