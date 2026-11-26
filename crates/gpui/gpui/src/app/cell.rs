use std::{
    cell::{Cell, Ref, RefCell, RefMut},
    panic::Location,
};

use anyhow::Result;
use derive_more::{Deref, DerefMut};

use super::App;

/// Temporary(?) wrapper around [`RefCell<App>`] to help us debug any double borrows.
/// Strongly consider removing after stabilization.
#[doc(hidden)]
pub struct AppCell {
    pub(in crate::app) app: RefCell<App>,
    pub(in crate::app) borrow_origin: Cell<Option<(&'static Location<'static>, &'static str)>>,
}

struct AppBorrowError {
    attempted: &'static Location<'static>,
    kind: &'static str,
    origin: Option<(&'static Location<'static>, &'static str)>,
}

impl std::fmt::Display for AppBorrowError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "RefCell already borrowed: attempted {} App borrow at {}; ",
            self.kind, self.attempted
        )?;
        match self.origin {
            Some((origin, kind)) => {
                write!(formatter, "existing {kind} borrow group began at {origin}")
            }
            None => write!(formatter, "origin unavailable"),
        }
    }
}

impl std::fmt::Debug for AppBorrowError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for AppBorrowError {}

impl AppCell {
    #[doc(hidden)]
    #[track_caller]
    pub fn borrow(&self) -> AppRef<'_> {
        if option_env!("TRACK_THREAD_BORROWS").is_some() {
            let thread_id = std::thread::current().id();
            eprintln!("borrowed {thread_id:?}");
        }
        // Preserve the first origin while shared borrows overlap. A new group replaces it.
        let starts_borrow_group = self.app.try_borrow_mut().is_ok();
        let attempted = Location::caller();
        let borrow = self.app.try_borrow().unwrap_or_else(|_| {
            panic!(
                "{}",
                AppBorrowError {
                    attempted,
                    kind: "shared",
                    origin: self.borrow_origin.get(),
                }
            )
        });
        if starts_borrow_group {
            self.borrow_origin.set(Some((Location::caller(), "shared")));
        }
        AppRef(borrow)
    }

    #[doc(hidden)]
    #[track_caller]
    pub fn borrow_mut(&self) -> AppRefMut<'_> {
        self.try_borrow_mut()
            .unwrap_or_else(|error| panic!("{error}"))
    }

    #[doc(hidden)]
    #[track_caller]
    pub fn try_borrow_mut(&self) -> Result<AppRefMut<'_>> {
        if option_env!("TRACK_THREAD_BORROWS").is_some() {
            let thread_id = std::thread::current().id();
            eprintln!("borrowed {thread_id:?}");
        }
        let attempted = Location::caller();
        let borrow = self.app.try_borrow_mut().map_err(|_| AppBorrowError {
            attempted,
            kind: "mutable",
            origin: self.borrow_origin.get(),
        })?;
        self.borrow_origin.set(Some((attempted, "mutable")));
        Ok(AppRefMut(borrow))
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

#[cfg(test)]
mod tests {
    #[crate::test]
    fn borrow_conflict_reports_first_shared_origin(cx: &mut crate::TestAppContext) {
        let first = cx.app.borrow();
        let origin = cx.app.borrow_origin.get().unwrap();
        let second = cx.app.borrow();
        assert_eq!(cx.app.borrow_origin.get(), Some(origin));
        drop(first);
        let error = cx
            .app
            .try_borrow_mut()
            .err()
            .expect("shared borrow is still live");
        assert!(
            error
                .to_string()
                .contains(&format!("shared borrow group began at {}", origin.0))
        );
        drop(second);
        let _mutable = cx.app.try_borrow_mut().unwrap();
        assert_eq!(cx.app.borrow_origin.get().unwrap().1, "mutable");
    }

    #[crate::test]
    fn shared_borrow_conflict_reports_mutable_origin(cx: &mut crate::TestAppContext) {
        let mutable = cx.app.borrow_mut();
        let origin = cx.app.borrow_origin.get().unwrap();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cx.app.borrow()));
        let panic = panic
            .err()
            .expect("mutable borrow prevents a shared borrow");
        let message = panic.downcast_ref::<String>().unwrap();
        assert!(message.contains("attempted shared App borrow"));
        assert!(message.contains(&format!("mutable borrow group began at {}", origin.0)));
        drop(mutable);
    }
}
