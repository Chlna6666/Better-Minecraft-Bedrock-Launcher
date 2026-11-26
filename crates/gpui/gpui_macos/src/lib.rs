//! macOS display and dispatch services used by GPUI.

#![cfg(target_os = "macos")]

mod display_link;

pub use display_link::DisplayLink;

#[doc(hidden)]
#[allow(
    non_upper_case_globals,
    non_camel_case_types,
    non_snake_case,
    unsafe_code
)]
pub mod dispatch_sys {
    include!(concat!(env!("OUT_DIR"), "/dispatch_sys.rs"));
}

/// Returns the process's main dispatch queue.
pub fn dispatch_get_main_queue() -> dispatch_sys::dispatch_queue_t {
    std::ptr::addr_of!(dispatch_sys::_dispatch_main_q) as *const _ as dispatch_sys::dispatch_queue_t
}
