use std::cell::{BorrowMutError, Ref, RefCell, RefMut};

use anyhow::Result;
use derive_more::{Deref, DerefMut};

use super::App;

/// Temporary(?) wrapper around [`RefCell<App>`] to help us debug any double borrows.
/// Strongly consider removing after stabilization.
#[doc(hidden)]
pub struct AppCell {
    pub(in crate::app) app: RefCell<App>,
}

impl AppCell {
    #[doc(hidden)]
    #[track_caller]
    pub fn borrow(&self) -> AppRef<'_> {
        if option_env!("TRACK_THREAD_BORROWS").is_some() {
            let thread_id = std::thread::current().id();
            eprintln!("borrowed {thread_id:?}");
        }
        AppRef(self.app.borrow())
    }

    #[doc(hidden)]
    #[track_caller]
    pub fn borrow_mut(&self) -> AppRefMut<'_> {
        if option_env!("TRACK_THREAD_BORROWS").is_some() {
            let thread_id = std::thread::current().id();
            eprintln!("borrowed {thread_id:?}");
        }
        AppRefMut(self.app.borrow_mut())
    }

    #[doc(hidden)]
    #[track_caller]
    pub fn try_borrow_mut(&self) -> Result<AppRefMut<'_>, BorrowMutError> {
        if option_env!("TRACK_THREAD_BORROWS").is_some() {
            let thread_id = std::thread::current().id();
            eprintln!("borrowed {thread_id:?}");
        }
        Ok(AppRefMut(self.app.try_borrow_mut()?))
    }
}

#[doc(hidden)]
#[derive(Deref, DerefMut)]
pub struct AppRef<'a>(Ref<'a, App>);

impl Drop for AppRef<'_> {
    fn drop(&mut self) {
        if option_env!("TRACK_THREAD_BORROWS").is_some() {
            let thread_id = std::thread::current().id();
            eprintln!("dropped borrow from {thread_id:?}");
        }
    }
}

#[doc(hidden)]
#[derive(Deref, DerefMut)]
pub struct AppRefMut<'a>(RefMut<'a, App>);

impl Drop for AppRefMut<'_> {
    fn drop(&mut self) {
        if option_env!("TRACK_THREAD_BORROWS").is_some() {
            let thread_id = std::thread::current().id();
            eprintln!("dropped {thread_id:?}");
        }
    }
}
