use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

/// Observable lifecycle stages owned by [`crate::ApplicationHost`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum LifecycleState {
    /// The host and its services have been constructed.
    Constructed = 0,
    /// The native event loop is starting.
    Starting = 1,
    /// GPUI has finished launching.
    Running = 2,
    /// The host no longer accepts new application work.
    ShutdownRequested = 3,
    /// The GUI and application runtime have stopped.
    Stopped = 4,
}

/// A cheap, thread-safe lifecycle observer.
#[derive(Clone, Debug)]
pub struct ApplicationLifecycle {
    state: Arc<AtomicU8>,
}

impl ApplicationLifecycle {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(AtomicU8::new(LifecycleState::Constructed as u8)),
        }
    }

    /// Returns the latest lifecycle state.
    #[must_use]
    pub fn state(&self) -> LifecycleState {
        match self.state.load(Ordering::Acquire) {
            0 => LifecycleState::Constructed,
            1 => LifecycleState::Starting,
            2 => LifecycleState::Running,
            3 => LifecycleState::ShutdownRequested,
            _ => LifecycleState::Stopped,
        }
    }

    pub(crate) fn transition_to(&self, state: LifecycleState) {
        self.state.store(state as u8, Ordering::Release);
    }
}
