use std::collections::HashMap;
use std::thread;
use std::time::Duration;
use tracing::{info, warn};
use windows::core::BOOL;
use windows::Win32::Foundation::{CloseHandle, FALSE, HWND, LPARAM, RECT, TRUE};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{ClipCursor, EnumWindows, GetAncestor, GetClassNameW, GetForegroundWindow, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, GA_ROOTOWNER};

fn get_class_name(hwnd: HWND) -> Option<String> {
    let mut class_name = [0u16; 256];
    unsafe {
        let len = GetClassNameW(hwnd, &mut class_name);
        if len == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&class_name[..len as usize]))
    }
}
/// 找到 UWP 应用的“宿主”或“CoreWindow”
/// 如果找到 CoreWindow，就返回它；否则返回宿主
fn find_uwp_frame(title_substring: &str) -> Option<HWND> {
    struct D<'a> { title: &'a str, hwnd: HWND }

    unsafe extern "system" fn enum_host(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let data = &mut *(lparam.0 as *mut D);
        if !IsWindowVisible(hwnd).as_bool() { return TRUE; }

        if let Some(class_name) = get_class_name(hwnd) {
            if class_name != "ApplicationFrameWindow" {
                return TRUE;
            }
        } else {
            return TRUE;
        }

        let len = GetWindowTextLengthW(hwnd);
        if len > 0 {
            let mut buf = vec![0u16; (len + 1) as usize];
            if GetWindowTextW(hwnd, &mut buf) > 0 {
                let title = String::from_utf16_lossy(&buf[..len as usize]);
                if title.contains(data.title) {
                    data.hwnd = hwnd;
                    return FALSE;
                }
            }
        }

        TRUE
    }

    let mut data = D { title: title_substring, hwnd: HWND(std::ptr::null_mut()) };
    unsafe { let _ = EnumWindows(Some(enum_host), LPARAM(&mut data as *mut _ as isize)); }
    if data.hwnd.0 != std::ptr::null_mut() { Some(data.hwnd) } else { None }
}


/// 每次都实时获取窗口外框并裁剪鼠标区域，减少指定像素
fn confine_to_window(hwnd: HWND, reduce_pixels: i32) {
    unsafe {
        let mut rc = RECT::default();
        // GetWindowRect 返回 Result<(), Error>
        if GetWindowRect(hwnd, &mut rc).is_ok() {
            // 减少指定像素
            rc.left += reduce_pixels;
            rc.right -= reduce_pixels;
            rc.top += reduce_pixels;
            rc.bottom -= reduce_pixels;

            let _ = ClipCursor(Some(&rc));
        }
    }
}

/// 释放鼠标限制
fn release_cursor() {
    unsafe { let _ = ClipCursor(None); }
}


/// 判断前台窗口是否属于 target（或它的顶层拥有者）
/// 避免子窗口、CoreWindow、宿主都能算作“在内部”
fn foreground_is_target(target: HWND) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        GetAncestor(fg, GA_ROOTOWNER) == target
    }
}

/// 映射可用于解除锁定的按键名到其 Virtual-Key 码
fn build_unlock_key_map() -> HashMap<&'static str, i32> {
    let mut m = HashMap::new();
    // VK_MENU.0 是 u16，需要显式转为 i32
    m.insert("ALT",    VK_MENU.0 as i32);
    m.insert("CTRL",   VK_CONTROL.0 as i32);
    m.insert("SHIFT",  VK_SHIFT.0 as i32);
    m.insert("LWIN",   VK_LWIN.0 as i32);
    m.insert("RWIN",   VK_RWIN.0 as i32);
    m
}
/// 检测指定按键是否被按下
fn key_is_down(vk: i32) -> bool {
    // GetAsyncKeyState 返回 i16，其中高位(0x8000)为1表示按下
    unsafe {
        let state = GetAsyncKeyState(vk);
        // 将 mask 也作为 i16
        (state & 0x8000u16 as i16) != 0
    }
}

/// 启动监控线程：每 100ms 重新枚举宿主、重新获取外框，并根据前台状态、最小化状态、
/// 以及自定义解除键锁/解锁鼠标
pub fn start_window_monitor(title_substring: &str, unlock_key_name: &str, reduce_pixels: i32) {
    let key = title_substring.to_owned();
    let unlock_name = unlock_key_name.to_owned();
    let map = build_unlock_key_map();
    let vk_unlock = map.get(unlock_name.as_str()).copied().unwrap_or(VK_MENU.0 as i32);

    thread::spawn(move || {
        let mut confined = false;
        let mut not_found_count = 0;
        loop {
            if key_is_down(vk_unlock) {
                if confined {
                    release_cursor();
                    info!("检测到 '{}' 长按，临时解除鼠标锁定", unlock_name);
                    confined = false;
                }
                thread::sleep(Duration::from_millis(50));
                continue;
            }

            if let Some(host) = find_uwp_frame(&key) {
                not_found_count = 0;
                let mut pid: u32 = 0;
                unsafe { GetWindowThreadProcessId(host, Some(&mut pid)) };
                if pid == 0 || !process_exists(pid) {
                    if confined {
                        release_cursor();
                        info!("UWP 进程已退出，解除锁定");
                        confined = false;
                    }
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }

                if unsafe { IsIconic(host).as_bool() } {
                    if confined {
                        release_cursor();
                        info!("窗口《{}》已最小化，解除锁定", key);
                        confined = false;
                    }
                } else if foreground_is_target(host) {
                    if !confined {
                        confine_to_window(host,reduce_pixels);
                        info!("已锁定鼠标到 UWP 窗口《{}》", key);
                        confined = true;
                    } else {
                        confine_to_window(host,reduce_pixels);
                    }
                } else if confined {
                    release_cursor();
                    info!("窗口《{}》失去前台，解除锁定", key);
                    confined = false;
                }
            } else {
                not_found_count += 1;
                if confined {
                    release_cursor();
                    info!("未找到 UWP 窗口《{}》，解除锁定", key);
                    confined = false;
                }
                // 如果连续找不到窗口，则跳出线程
                if not_found_count > 20 {
                    warn!("长时间未找到窗口《{}》，退出监控线程", key);
                    break;
                }
            }

            thread::sleep(Duration::from_millis(500));
        }
    });
}

fn process_exists(pid: u32) -> bool {
    unsafe {
        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(handle) => {
                let _ = CloseHandle(handle);
                true
            },
            Err(_) => false,
        }
    }
}