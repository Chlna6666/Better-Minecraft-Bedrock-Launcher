#![cfg(target_os = "windows")]

use core::ffi::{c_char, c_void};
use std::mem;
use std::ptr;
use std::sync::OnceLock;

const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x0000_0800;
const S_OK: i32 = 0;
const XUSER_ADD_DEFAULT_USER_SILENTLY: u32 = 0x01;
const XUSER_GAMER_PICTURE_MEDIUM: u32 = 1;
const XUSER_STATE_SIGNED_IN: u32 = 0;
const MAX_GAMERTAG_BYTES: usize = 256;
const MAX_GAMER_PICTURE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SystemXboxUserState {
    SignedIn,
    SigningOut,
    SignedOut,
    Unknown(u32),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SystemXboxUser {
    pub(crate) xuid: u64,
    pub(crate) gamertag: String,
    pub(crate) state: SystemXboxUserState,
    pub(crate) gamer_picture_png: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SystemXboxUserProbe {
    SignedIn(SystemXboxUser),
    SignedOut { hresult: Option<i32> },
    Unavailable { reason: String },
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

impl Guid {
    const fn new(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Self {
        Self {
            data1,
            data2,
            data3,
            data4,
        }
    }
}

const CLSID_XUSER_IMPL: Guid = Guid::new(
    0x01ac_d177,
    0x91f9,
    0x4763,
    [0xa3, 0x8e, 0xcc, 0xbb, 0x55, 0xce, 0x32, 0xe0],
);
const IID_IXUSER_BASE: Guid = CLSID_XUSER_IMPL;
const IID_IXUSER_GAMERTAG: Guid = Guid::new(
    0xcef4_fac0,
    0x7676,
    0x4a94,
    [0xa1, 0x19, 0x4c, 0x43, 0xf9, 0xeb, 0x5b, 0x74],
);
const CLSID_XTHREADING_IMPL: Guid = Guid::new(
    0x073b_7dcb,
    0x1fcf,
    0x4030,
    [0x94, 0xbe, 0xe3, 0xc9, 0xeb, 0x62, 0x34, 0x28],
);
const IID_IXTHREADING_IMPL: Guid = CLSID_XTHREADING_IMPL;

#[repr(C)]
struct XAsyncBlock {
    queue: *mut c_void,
    context: *mut c_void,
    callback: Option<unsafe extern "system" fn(*mut XAsyncBlock)>,
    internal: [usize; 4],
}

impl XAsyncBlock {
    fn new() -> Self {
        Self {
            queue: ptr::null_mut(),
            context: ptr::null_mut(),
            callback: None,
            internal: [0; 4],
        }
    }
}

#[repr(C)]
struct Interface {
    vtable: *const usize,
}

type QueryApiImplFn =
    unsafe extern "system" fn(*const Guid, *const Guid, *mut *mut c_void) -> i32;
type XGameRuntimeInitializeFn = unsafe extern "system" fn() -> i32;
type QueryInterfaceFn =
    unsafe extern "system" fn(*mut Interface, *const Guid, *mut *mut c_void) -> i32;
type ReleaseFn = unsafe extern "system" fn(*mut Interface) -> u32;
type XUserAddAsyncFn =
    unsafe extern "system" fn(*mut Interface, u32, *mut XAsyncBlock) -> i32;
type XUserAddResultFn = unsafe extern "system" fn(
    *mut Interface,
    *mut XAsyncBlock,
    *mut *mut c_void,
) -> i32;
type XUserCloseHandleFn = unsafe extern "system" fn(*mut Interface, *mut c_void);
type XUserGetIdFn = unsafe extern "system" fn(*mut Interface, *mut c_void, *mut u64) -> i32;
type XUserGetStateFn =
    unsafe extern "system" fn(*mut Interface, *mut c_void, *mut u32) -> i32;
type XUserGetGamertagFn = unsafe extern "system" fn(
    *mut Interface,
    *mut c_void,
    u32,
    usize,
    *mut c_char,
    *mut usize,
) -> i32;
type XUserGetGamerPictureAsyncFn = unsafe extern "system" fn(
    *mut Interface,
    *mut c_void,
    u32,
    *mut XAsyncBlock,
) -> i32;
type XUserGetGamerPictureResultSizeFn =
    unsafe extern "system" fn(*mut Interface, *mut XAsyncBlock, *mut usize) -> i32;
type XUserGetGamerPictureResultFn = unsafe extern "system" fn(
    *mut Interface,
    *mut XAsyncBlock,
    usize,
    *mut c_void,
    *mut usize,
) -> i32;
type XAsyncGetStatusFn =
    unsafe extern "system" fn(*mut Interface, *mut XAsyncBlock, u8) -> i32;

struct RuntimeApi {
    query_api: QueryApiImplFn,
}

unsafe impl Send for RuntimeApi {}
unsafe impl Sync for RuntimeApi {}

static RUNTIME: OnceLock<Result<RuntimeApi, String>> = OnceLock::new();

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryExW(file_name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

pub(crate) fn probe_default_user() -> SystemXboxUserProbe {
    let runtime = match runtime_api() {
        Ok(runtime) => runtime,
        Err(reason) => {
            return SystemXboxUserProbe::Unavailable {
                reason: reason.clone(),
            };
        }
    };

    let user_provider = match InterfaceHandle::query(runtime.query_api, &CLSID_XUSER_IMPL, &IID_IXUSER_BASE) {
        Ok(provider) => provider,
        Err(status) => {
            return SystemXboxUserProbe::Unavailable {
                reason: format!("无法获取系统 IXUser 接口：HRESULT=0x{:08X}", status as u32),
            };
        }
    };
    let threading = match InterfaceHandle::query(
        runtime.query_api,
        &CLSID_XTHREADING_IMPL,
        &IID_IXTHREADING_IMPL,
    ) {
        Ok(threading) => threading,
        Err(status) => {
            return SystemXboxUserProbe::Unavailable {
                reason: format!("无法获取系统 IXThreading 接口：HRESULT=0x{:08X}", status as u32),
            };
        }
    };

    let mut add_async = XAsyncBlock::new();
    let start_status = unsafe {
        let add: XUserAddAsyncFn = user_provider.slot(7);
        add(
            user_provider.ptr,
            XUSER_ADD_DEFAULT_USER_SILENTLY,
            &mut add_async,
        )
    };
    if start_status < 0 {
        return SystemXboxUserProbe::Unavailable {
            reason: format!(
                "系统默认 Xbox 用户静默探测启动失败：HRESULT=0x{:08X}",
                start_status as u32
            ),
        };
    }

    let completion_status = unsafe {
        let get_status: XAsyncGetStatusFn = threading.slot(3);
        get_status(threading.ptr, &mut add_async, 1)
    };
    if completion_status < 0 {
        return SystemXboxUserProbe::SignedOut {
            hresult: Some(completion_status),
        };
    }

    let mut user = ptr::null_mut();
    let result_status = unsafe {
        let result: XUserAddResultFn = user_provider.slot(8);
        result(user_provider.ptr, &mut add_async, &mut user)
    };
    if result_status < 0 || user.is_null() {
        return SystemXboxUserProbe::SignedOut {
            hresult: Some(result_status),
        };
    }
    let user = UserHandle {
        provider: user_provider.ptr,
        user,
    };

    let state = match read_state(&user_provider, user.user) {
        Ok(state) => state,
        Err(status) => {
            return SystemXboxUserProbe::Unavailable {
                reason: format!("读取系统 Xbox 用户状态失败：HRESULT=0x{:08X}", status as u32),
            };
        }
    };
    if state != SystemXboxUserState::SignedIn {
        return SystemXboxUserProbe::SignedOut { hresult: None };
    }

    let xuid = match read_xuid(&user_provider, user.user) {
        Ok(xuid) if xuid != 0 => xuid,
        Ok(_) => {
            return SystemXboxUserProbe::Unavailable {
                reason: "系统 Xbox 用户返回了空 XUID".to_string(),
            };
        }
        Err(status) => {
            return SystemXboxUserProbe::Unavailable {
                reason: format!("读取系统 Xbox XUID 失败：HRESULT=0x{:08X}", status as u32),
            };
        }
    };
    let gamertag = match read_gamertag(&user_provider, user.user) {
        Ok(gamertag) if !gamertag.is_empty() => gamertag,
        Ok(_) => {
            return SystemXboxUserProbe::Unavailable {
                reason: "系统 Xbox 用户返回了空 Gamertag".to_string(),
            };
        }
        Err(status) => {
            return SystemXboxUserProbe::Unavailable {
                reason: format!("读取系统 Xbox Gamertag 失败：HRESULT=0x{:08X}", status as u32),
            };
        }
    };
    let gamer_picture_png = read_gamer_picture(&user_provider, &threading, user.user).ok();

    SystemXboxUserProbe::SignedIn(SystemXboxUser {
        xuid,
        gamertag,
        state,
        gamer_picture_png,
    })
}

fn runtime_api() -> Result<&'static RuntimeApi, &'static String> {
    match RUNTIME.get_or_init(initialize_runtime) {
        Ok(runtime) => Ok(runtime),
        Err(error) => Err(error),
    }
}

fn initialize_runtime() -> Result<RuntimeApi, String> {
    let module_name = wide("xgameruntime.dll");
    let module = unsafe {
        LoadLibraryExW(
            module_name.as_ptr(),
            ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    if module.is_null() {
        return Err("无法从 System32 加载系统 xgameruntime.dll".to_string());
    }

    let initialize = unsafe { GetProcAddress(module, b"XGameRuntimeInitialize\0".as_ptr()) };
    let query_api = unsafe { GetProcAddress(module, b"QueryApiImpl\0".as_ptr()) };
    if initialize.is_null() || query_api.is_null() {
        return Err("系统 xgameruntime.dll 缺少初始化或 QueryApiImpl 导出".to_string());
    }

    let initialize: XGameRuntimeInitializeFn = unsafe { mem::transmute(initialize) };
    let status = unsafe { initialize() };
    if status < 0 {
        return Err(format!(
            "初始化系统 Gaming Runtime 失败：HRESULT=0x{:08X}",
            status as u32
        ));
    }

    Ok(RuntimeApi {
        query_api: unsafe { mem::transmute(query_api) },
    })
}

struct InterfaceHandle {
    ptr: *mut Interface,
}

impl InterfaceHandle {
    fn query(query_api: QueryApiImplFn, class_id: &Guid, interface_id: &Guid) -> Result<Self, i32> {
        let mut value = ptr::null_mut();
        let status = unsafe { query_api(class_id, interface_id, &mut value) };
        if status < 0 || value.is_null() {
            return Err(if status < 0 { status } else { 0x8000_4003_u32 as i32 });
        }
        Ok(Self { ptr: value.cast() })
    }

    unsafe fn slot<T: Copy>(&self, index: usize) -> T {
        let address = unsafe { *(*self.ptr).vtable.add(index) };
        unsafe { mem::transmute_copy(&address) }
    }
}

impl Drop for InterfaceHandle {
    fn drop(&mut self) {
        if self.ptr.is_null() {
            return;
        }
        unsafe {
            let release: ReleaseFn = self.slot(2);
            release(self.ptr);
        }
    }
}

struct UserHandle {
    provider: *mut Interface,
    user: *mut c_void,
}

impl Drop for UserHandle {
    fn drop(&mut self) {
        if self.provider.is_null() || self.user.is_null() {
            return;
        }
        unsafe {
            let vtable = (*self.provider).vtable;
            let close: XUserCloseHandleFn = mem::transmute_copy(&*vtable.add(4));
            close(self.provider, self.user);
        }
    }
}

fn read_xuid(provider: &InterfaceHandle, user: *mut c_void) -> Result<u64, i32> {
    let mut xuid = 0_u64;
    let status = unsafe {
        let get_id: XUserGetIdFn = provider.slot(11);
        get_id(provider.ptr, user, &mut xuid)
    };
    if status < 0 { Err(status) } else { Ok(xuid) }
}

fn read_state(provider: &InterfaceHandle, user: *mut c_void) -> Result<SystemXboxUserState, i32> {
    let mut state = 0_u32;
    let status = unsafe {
        let get_state: XUserGetStateFn = provider.slot(14);
        get_state(provider.ptr, user, &mut state)
    };
    if status < 0 {
        return Err(status);
    }
    Ok(match state {
        XUSER_STATE_SIGNED_IN => SystemXboxUserState::SignedIn,
        1 => SystemXboxUserState::SigningOut,
        2 => SystemXboxUserState::SignedOut,
        value => SystemXboxUserState::Unknown(value),
    })
}

fn read_gamertag(provider: &InterfaceHandle, user: *mut c_void) -> Result<String, i32> {
    let query_interface: QueryInterfaceFn = unsafe { provider.slot(0) };
    let mut gamertag_interface = ptr::null_mut();
    let status = unsafe {
        query_interface(
            provider.ptr,
            &IID_IXUSER_GAMERTAG,
            &mut gamertag_interface,
        )
    };
    if status < 0 || gamertag_interface.is_null() {
        return Err(if status < 0 { status } else { 0x8000_4002_u32 as i32 });
    }
    let gamertag_interface = InterfaceHandle {
        ptr: gamertag_interface.cast(),
    };

    let mut buffer = [0_u8; MAX_GAMERTAG_BYTES];
    let mut used = 0_usize;
    let status = unsafe {
        let get_gamertag: XUserGetGamertagFn = gamertag_interface.slot(3);
        get_gamertag(
            gamertag_interface.ptr,
            user,
            0,
            buffer.len(),
            buffer.as_mut_ptr().cast(),
            &mut used,
        )
    };
    if status < 0 {
        return Err(status);
    }
    let length = used
        .min(buffer.len())
        .saturating_sub(usize::from(used > 0 && buffer[used.min(buffer.len()) - 1] == 0));
    Ok(String::from_utf8_lossy(&buffer[..length])
        .trim_matches(char::from(0))
        .trim()
        .to_string())
}

fn read_gamer_picture(
    provider: &InterfaceHandle,
    threading: &InterfaceHandle,
    user: *mut c_void,
) -> Result<Vec<u8>, i32> {
    let mut async_block = XAsyncBlock::new();
    let status = unsafe {
        let begin: XUserGetGamerPictureAsyncFn = provider.slot(15);
        begin(
            provider.ptr,
            user,
            XUSER_GAMER_PICTURE_MEDIUM,
            &mut async_block,
        )
    };
    if status < 0 {
        return Err(status);
    }
    let status = unsafe {
        let get_status: XAsyncGetStatusFn = threading.slot(3);
        get_status(threading.ptr, &mut async_block, 1)
    };
    if status < 0 {
        return Err(status);
    }

    let mut required = 0_usize;
    let status = unsafe {
        let result_size: XUserGetGamerPictureResultSizeFn = provider.slot(16);
        result_size(provider.ptr, &mut async_block, &mut required)
    };
    if status < 0 {
        return Err(status);
    }
    if required == 0 || required > MAX_GAMER_PICTURE_BYTES {
        return Err(0x8007_0057_u32 as i32);
    }

    let mut bytes = vec![0_u8; required];
    let mut used = 0_usize;
    let status = unsafe {
        let result: XUserGetGamerPictureResultFn = provider.slot(17);
        result(
            provider.ptr,
            &mut async_block,
            bytes.len(),
            bytes.as_mut_ptr().cast(),
            &mut used,
        )
    };
    if status < 0 {
        return Err(status);
    }
    if used == 0 || used > bytes.len() {
        return Err(0x8007_0057_u32 as i32);
    }
    bytes.truncate(used);
    Ok(bytes)
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(core::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn async_block_layout_matches_gdk_x64() {
        assert_eq!(mem::size_of::<XAsyncBlock>(), 56);
        assert_eq!(mem::align_of::<XAsyncBlock>(), mem::align_of::<usize>());
    }

    #[test]
    fn known_runtime_guids_are_not_zero() {
        assert_ne!(CLSID_XUSER_IMPL.data1, 0);
        assert_ne!(CLSID_XTHREADING_IMPL.data1, 0);
        assert_ne!(IID_IXUSER_GAMERTAG.data1, 0);
    }
}
