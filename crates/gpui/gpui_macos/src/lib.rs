//! AppKit platform provider for GPUI.

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

mod dispatcher;
mod display;
mod events;
mod keyboard;


#[cfg(all(not(feature = "macos-blade"), not(feature = "nova-gfx-metal")))]
use gpui::platform::metal as renderer;

#[cfg(all(not(feature = "macos-blade"), feature = "nova-gfx-metal"))]
mod nova_renderer;
#[cfg(all(not(feature = "macos-blade"), feature = "nova-gfx-metal"))]
use nova_renderer as renderer;

#[cfg(feature = "macos-blade")]
use gpui::platform::blade as renderer;

mod attributed_string;

// Text shaping/rasterization is provided by the shared CosmicTextSystem.
// The legacy font-kit implementation is not part of the active dependency graph.

mod implementation;
mod window;
mod window_appearance;

use gpui::{Pixels, Size, px, size};
use cocoa::{
    base::{id, nil},
    foundation::{NSAutoreleasePool, NSNotFound, NSSize, NSString, NSUInteger},
};

use objc::runtime::{BOOL, NO, YES};
use std::{
    ffi::{CStr, c_char},
    ops::Range,
};

pub(crate) use dispatcher::*;
pub(crate) use display::*;
pub(crate) use keyboard::*;
pub(crate) use implementation::{
    LMGetKbdType, TISCopyCurrentKeyboardLayoutInputSource,
    TISCopyCurrentKeyboardInputSource, TISGetInputSourceProperty, UCKeyTranslate,
    kTISPropertyUnicodeKeyLayoutData, kTISPropertyInputSourceID,
    kTISPropertyLocalizedName, kTISPropertyInputSourceIsASCIICapable,
    kTISPropertyInputSourceType, kTISTypeKeyboardInputMode,
};
pub use implementation::MacPlatform;
pub(crate) use window::*;

trait BoolExt {
    fn to_objc(self) -> BOOL;
}

impl BoolExt for bool {
    fn to_objc(self) -> BOOL {
        if self { YES } else { NO }
    }
}

trait NSStringExt {
    unsafe fn as_str(&self) -> &str;
}

impl NSStringExt for id {
    unsafe fn as_str(&self) -> &str {
        unsafe {
            let cstr = self.UTF8String();
            if cstr.is_null() {
                ""
            } else {
                CStr::from_ptr(cstr as *mut c_char).to_str().unwrap()
            }
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
struct NSRange {
    pub location: NSUInteger,
    pub length: NSUInteger,
}

impl NSRange {
    fn invalid() -> Self {
        Self {
            location: NSNotFound as NSUInteger,
            length: 0,
        }
    }

    fn is_valid(&self) -> bool {
        self.location != NSNotFound as NSUInteger
    }

    fn to_range(self) -> Option<Range<usize>> {
        if self.is_valid() {
            let start = self.location as usize;
            let end = start + self.length as usize;
            Some(start..end)
        } else {
            None
        }
    }
}

impl From<Range<usize>> for NSRange {
    fn from(range: Range<usize>) -> Self {
        NSRange {
            location: range.start as NSUInteger,
            length: range.len() as NSUInteger,
        }
    }
}

unsafe impl objc::Encode for NSRange {
    fn encode() -> objc::Encoding {
        let encoding = format!(
            "{{NSRange={}{}}}",
            NSUInteger::encode().as_str(),
            NSUInteger::encode().as_str()
        );
        unsafe { objc::Encoding::from_str(&encoding) }
    }
}

unsafe fn ns_string(string: &str) -> id {
    unsafe { NSString::alloc(nil).init_str(string).autorelease() }
}

fn logical_size(value: NSSize) -> Size<Pixels> {
    size(px(value.width as f32), px(value.height as f32))
}
