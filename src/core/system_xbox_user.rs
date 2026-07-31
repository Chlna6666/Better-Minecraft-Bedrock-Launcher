#![cfg(target_os = "windows")]

use core::ffi::{c_char, c_void};
use std::mem;
use std::ptr;
use std::sync::OnceLock;

const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x0000_0800;
const XUSER_ADD_DEFAULT_USER_SILENTLY: u32 = 0x01;
const XUSER_GAMER_PICTURE_MEDIUM: u32 = 1;
const XUSER_STATE_SIGNED_IN: u32 = 0;
const MAX_GAMERTAG_BYTES: usize = 256;
const MAX_WIDE_GAMERTAG_UNITS: usize = 256;
const MAX_WIDE_XUID_UNITS: usize = 32;
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

type XblGetUserInfoFn =
    unsafe extern "system" fn(u32, *mut *mut u16, *mut *mut u16) -> i32;
type QueryApiImplFn =
    unsafe extern "system" fn(*const Guid, *const Guid, *mut *mut c_void) -> i32;
type XGameRuntimeInitializeFn = unsafe extern "system" fn() -> i32;
type QueryInterfaceFn =
    unsafe extern "system" fn(*mut Interface, *const Guid, *mut *mut c_void) -> i32;
type ReleaseFn = unsafe extern "system" fn(*mut Interface) -> u32;
type XUserAddAsyncFn =
    unsafe extern "system" fn(*mut Interface, u32, *mut XAsyncBlock) -> i32;
type XUserAddResultFn =
    unsafe extern "system" fn(*mut Interface, *mut XAsyncBlock, *mut *mut c_void) -> i32;
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
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
}

/// Reads the Windows/Xbox-app local user without requiring a game process.
///
/// `XblGetUserInfo` is the primary source because a normal launcher process has
/// no GDK title/default-user association. The GDK path is retained only as a
/// fallback and as a best-effort source for the gamer picture.
pub(crate) fn probe_default_user() -> SystemXboxUserProbe {
    match probe_xbox_services_user() {
        Ok(Some((xuid, gamertag))) => {
            let gamer_picture_png = probe_gdk_default_user()
                .ok()
                .flatten()
                .filter(|user| user.xuid == xuid)
                .and_then(|user| user.gamer_picture_png);
            tracing::debug!(
                xbox_gamertag = %gamertag,
                xuid = %xuid,
                source = "xboxservices.dll!XblGetUserInfo",
                picture_from_gdk = gamer_picture_png.is_some(),
                "已读取 Windows 系统真实 Xbox 用户"
            );
            SystemXboxUserProbe::SignedIn(SystemXboxUser {
                xuid,
                gamertag,
                state: SystemXboxUserState::SignedIn,
                gamer_picture_png,
            })
        }
        Ok(None) => match probe_gdk_default_user() {
            Ok(Some(user)) => SystemXboxUserProbe::SignedIn(user),
            Ok(None) => SystemXboxUserProbe::SignedOut { hresult: None },
            Err(reason) => SystemXboxUserProbe::Unavailable { reason },
        },
        Err(xbox_services_error) => match probe_gdk_default_user() {
            Ok(Some(user)) => {
                tracing::debug!(
                    %xbox_services_error,
                    "Xbox Services 用户读取不可用，已使用 GDK 静默用户兜底"
                );
                SystemXboxUserProbe::SignedIn(user)
            }
            Ok(None) => SystemXboxUserProbe::SignedOut { hresult: None },
            Err(gdk_error) => SystemXboxUserProbe::Unavailable {
                reason: format!(
                    "Xbox Services 用户读取失败：{xbox_services_error}；GDK 兜底失败：{gdk_error}"
                ),
            },
        },
    }
}

fn probe_xbox_services_user() -> Result<Option<(u64, String)>, String> {
    let module_name = wide("xboxservices.dll");
    let module = unsafe {
        LoadLibraryExW(
            module_name.as_ptr(),
            ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    if module.is_null() {
        return Err("无法从 System32 加载 xboxservices.dll".to_string());
    }

    let proc = unsafe { GetProcAddress(module, b"XblGetUserInfo\0".as_ptr()) };
    if proc.is_null() {
        return Err("xboxservices.dll 不提供 XblGetUserInfo 导出".to_string());
    }
    let get_user_info: XblGetUserInfoFn = unsafe { mem::transmute(proc) };

    let mut gamertag_ptr = ptr::null_mut();
    let mut xuid_ptr = ptr::null_mut();
    let status = unsafe { get_user_info(0, &mut gamertag_ptr, &mut xuid_ptr) };

    let gamertag = unsafe { take_local_wide_string(gamertag_ptr, MAX_WIDE_GAMERTAG_UNITS) };
    let xuid_text = unsafe { take_local_wide_string(xuid_ptr, MAX_WIDE_XUID_UNITS) };

    if status < 0 {
        return Ok(None);
    }
    let gamertag = sanitize_gamertag(gamertag.unwrap_or_default());
    let xuid_text = xuid_text.unwrap_or_default();
    if gamertag.is_empty() || xuid_text.is_empty() {
        return Ok(None);
    }
    let xuid = xuid_text
        .trim()
        .parse::<u64>()
        .map_err(|_| "XblGetUserInfo 返回了无效 XUID".to_string())?;
    if xuid == 0 {
        return Ok(None);
    }
    Ok(Some((xuid, gamertag)))
}

unsafe fn take_local_wide_string(pointer: *mut u16, limit: usize) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    let mut length = 0usize;
    while length < limit && unsafe { *pointer.add(length) } != 0 {
        length += 1;
    }
    let value = if length == limit {
        None
    } else {
        let units = unsafe { std::slice::from_raw_parts(pointer, length) };
        Some(String::from_utf16_lossy(units))
    };
    unsafe {
        LocalFree(pointer.cast());
    }
    value
}

fn sanitize_gamertag(value: String) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(64)
        .collect::<String>()
        .trim()
        .to_string()
}

fn probe_gdk_default_user() -> Result<Option<SystemXboxUser>, String> {
    let runtime = runtime_api()?;
    let user_provider = InterfaceHandle::query(
        runtime.query_api,
        &CLSID_XUSER_IMPL,
        &IID_IXUSER_BASE,
    )
    .map_err(|status| format!("无法获取系统 IXUser 接口：HRESULT=0x{:08X}", status as u32))?;
    let threading = InterfaceHandle::query(
        runtime.query_api,
        &CLSID_XTHREADING_IMPL,
        &IID_IXTHREADING_IMPL,
    )
    .map_err(|status| {
        format!(
            "无法获取系统 IXThreading 接口：HRESULT=0x{:08X}",
            status as u32
        )
    })?;

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
        return Ok(None);
    }
    let completion_status = unsafe {
        let get_status: XAsyncGetStatusFn = threading.slot(3);
        get_status(threading.ptr, &mut add_async, 1)
    };
    if completion_status < 0 {
        return Ok(None);
    }

    let mut user = ptr::null_mut();
    let result_status = unsafe {
        let result: XUserAddResultFn = user_provider.slot(8);
        result(user_provider.ptr, &mut add_async, &mut user)
    };
    if result_status < 0 || user.is_null() {
        return Ok(None);
    }
    let user = UserHandle {
        provider: user_provider.ptr,
        user,
    };

    let state = read_state(&user_provider, user.user)
        .map_err(|status| format!("读取 XUserState 失败：HRESULT=0x{:08X}", status as u32))?;
    if state != SystemXboxUserState::SignedIn {
        return Ok(None);
    }
    let xuid = read_xuid(&user_provider, user.user)
        .map_err(|status| format!("读取 XUID 失败：HRESULT=0x{:08X}", status as u32))?;
    if xuid == 0 {
        return Ok(None);
    }
    let gamertag = sanitize_gamertag(
        read_gamertag(&user_provider, user.user)
            .map_err(|status| format!("读取 Gamertag 失败：HRESULT=0x{:08X}", status as u32))?,
    );
    if gamertag.is_empty() {
        return Ok(None);
    }
    let gamer_picture_png = read_gamer_picture(&user_provider, &threading, user.user).ok();
    Ok(Some(SystemXboxUser {
        xuid,
        gamertag,
        state,
        gamer_picture_png,
    }))
}

fn runtime_api() -> Result<&'static RuntimeApi, String> {
    match RUNTIME.get_or_init(initialize_runtime) {
        Ok(runtime) => Ok(runtime),
        Err(error) => Err(error.clone()),
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
            return Err(if status < 0 {
                status
            } else {
                0x8000_4003_u32 as i32
            });
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
        if !self.ptr.is_null() {
            unsafe {
                let release: ReleaseFn = self.slot(2);
                release(self.ptr);
            }
        }
    }
}

struct UserHandle {
    provider: *mut Interface,
    user: *mut c_void,
}

impl Drop for UserHandle {
    fn drop(&mut self) {
        if !self.provider.is_null() && !self.user.is_null() {
            unsafe {
                let vtable = (*self.provider).vtable;
                let close: XUserCloseHandleFn = mem::transmute_copy(&*vtable.add(4));
                close(self.provider, self.user);
            }
        }
    }
}

fn read_xuid(provider: &InterfaceHandle, user: *mut c_void) -> Result<u64, i32> {
    let mut xuid = 0;
    let status = unsafe {
        let get_id: XUserGetIdFn = provider.slot(11);
        get_id(provider.ptr, user, &mut xuid)
    };
    if status < 0 { Err(status) } else { Ok(xuid) }
}

fn read_state(provider: &InterfaceHandle, user: *mut c_void) -> Result<SystemXboxUserState, i32> {
    let mut state = 0;
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
        return Err(if status < 0 {
            status
        } else {
            0x8000_4002_u32 as i32
        });
    }
    let gamertag_interface = InterfaceHandle {
        ptr: gamertag_interface.cast(),
    };
    let mut buffer = [0u8; MAX_GAMERTAG_BYTES];
    let mut used = 0;
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
    let used = used.min(buffer.len());
    let length = used.saturating_sub(usize::from(used > 0 && buffer[used - 1] == 0));
    Ok(String::from_utf8_lossy(&buffer[..length]).to_string())
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
    let mut required = 0;
    let status = unsafe {
        let size: XUserGetGamerPictureResultSizeFn = provider.slot(16);
        size(provider.ptr, &mut async_block, &mut required)
    };
    if status < 0 || required == 0 || required > MAX_GAMER_PICTURE_BYTES {
        return Err(if status < 0 {
            status
        } else {
            0x8007_0057_u32 as i32
        });
    }
    let mut bytes = vec![0; required];
    let mut used = 0;
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
    if status < 0 || used == 0 || used > bytes.len() {
        return Err(if status < 0 {
            status
        } else {
            0x8007_0057_u32 as i32
        });
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
    }

    #[test]
    fn gamertag_sanitizer_removes_controls_and_limits_length() {
        let input = format!("  A\n{}  ", "B".repeat(100));
        let output = sanitize_gamertag(input);
        assert!(!output.contains('\n'));
        assert!(output.chars().count() <= 64);
    }
}