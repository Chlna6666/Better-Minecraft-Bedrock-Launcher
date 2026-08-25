use crate::{App, FocusHandle};

impl App {
    /// Create a handle for tracking and manipulating keyboard focus.
    #[track_caller]
    pub fn focus_handle(&self) -> FocusHandle {
        FocusHandle::new(&self.focus_handles)
    }
}
