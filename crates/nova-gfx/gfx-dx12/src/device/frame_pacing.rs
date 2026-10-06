//! One-shot DXGI readiness waits. Only the device owner registers/cancels waits.

use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Mutex, PoisonError},
};

use gfx_core::{Error, Result};
use windows::Win32::{
    Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::Threading::{
        CloseThreadpoolWait, CreateThreadpoolWait, PTP_CALLBACK_INSTANCE, PTP_WAIT,
        SetThreadpoolWait, WaitForSingleObject, WaitForThreadpoolWaitCallbacks,
    },
};

type ReadyCallback = Box<dyn FnOnce() + Send + 'static>;

#[derive(Default)]
struct ReadyState {
    ready: bool,
    armed: bool,
    callback: Option<ReadyCallback>,
}

#[derive(Default)]
pub(super) struct FrameLatencyWait {
    wait: Option<PTP_WAIT>,
    // Stable allocation outlives the native wait and every drained callback.
    state: Box<Mutex<ReadyState>>,
}

impl FrameLatencyWait {
    pub(super) fn ready(&self, handle: HANDLE) -> Result<bool> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.ready || state.armed {
            return Ok(state.ready);
        }
        // SAFETY: The swapchain owns the live handle. Holding this lock prevents
        // the poll path from competing with an armed native wait for its signal.
        let result = unsafe { WaitForSingleObject(handle, 0) };
        if result == WAIT_OBJECT_0 {
            state.ready = true;
            Ok(true)
        } else if result == WAIT_TIMEOUT {
            Ok(false)
        } else {
            Err(Error::Backend(format!(
                "DX12 frame-latency readiness query failed: {result:?}"
            )))
        }
    }

    pub(super) fn arm(&mut self, handle: HANDLE, callback: ReadyCallback) -> Result<()> {
        self.cancel();
        if self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .ready
        {
            callback();
            return Ok(());
        }
        let wait = if let Some(wait) = self.wait {
            wait
        } else {
            let context = (&raw const *self.state).cast_mut().cast::<c_void>();
            // SAFETY: The boxed context is stable until Drop drains and closes
            // this wait. The callback accesses only its mutex-protected state.
            let wait = unsafe { CreateThreadpoolWait(Some(on_ready), Some(context), None) }
                .map_err(|error| Error::Backend(error.to_string()))?;
            self.wait = Some(wait);
            wait
        };
        {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.callback = Some(callback);
            state.armed = true;
        }
        // SAFETY: Both wait and handle remain live until cancel drains callbacks.
        // No timeout: this is a one-shot OS signal wait, not periodic polling.
        unsafe { SetThreadpoolWait(wait, Some(handle), None) };
        Ok(())
    }

    pub(super) fn cancel(&mut self) {
        if let Some(wait) = self.wait {
            // SAFETY: Stop new waits while the wait/context are still live.
            unsafe { SetThreadpoolWait(wait, None, None) };
            self.state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .callback = None;
            // SAFETY: Drain queued/running callbacks without holding their mutex.
            // Do not cancel queued native callbacks: the OS may already have
            // consumed the auto-reset signal. They must publish its ready credit,
            // while the removed user callback suppresses stale notifications.
            unsafe {
                WaitForThreadpoolWaitCallbacks(wait, false);
            }
        }
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.armed = false;
        state.callback = None;
    }

    pub(super) fn consume_ready(&mut self) {
        self.cancel();
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .ready = false;
    }

    pub(super) fn reset(&mut self) {
        self.consume_ready();
    }
}

impl Drop for FrameLatencyWait {
    fn drop(&mut self) {
        self.cancel();
        if let Some(wait) = self.wait.take() {
            // SAFETY: cancel drained every callback; the context is still live.
            unsafe { CloseThreadpoolWait(wait) };
        }
    }
}

unsafe extern "system" fn on_ready(
    _instance: PTP_CALLBACK_INSTANCE,
    context: *mut c_void,
    _wait: PTP_WAIT,
    result: u32,
) {
    // SAFETY: arm passes the stable boxed mutex; Drop drains us before freeing it.
    let state = unsafe { &*context.cast::<Mutex<ReadyState>>() };
    let callback = {
        let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
        state.armed = false;
        if result != WAIT_OBJECT_0.0 {
            state.callback = None;
            return;
        }
        state.ready = true;
        state.callback.take()
    };
    if let Some(callback) = callback {
        // A panic must not unwind through a native callback boundary.
        if catch_unwind(AssertUnwindSafe(callback)).is_err() {
            log::error!("DX12 frame-ready notification panicked");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};
    use windows::Win32::{
        Foundation::CloseHandle,
        System::Threading::{CreateEventW, SetEvent},
    };

    struct Event(HANDLE);

    impl Event {
        fn new() -> Self {
            // SAFETY: unnamed auto-reset event, owned until Drop.
            Self(unsafe { CreateEventW(None, false, false, None) }.unwrap())
        }

        fn signal(&self) {
            // SAFETY: the event remains live.
            unsafe { SetEvent(self.0) }.unwrap();
        }
    }

    impl Drop for Event {
        fn drop(&mut self) {
            // SAFETY: tests drop their waiter before this event.
            unsafe { CloseHandle(self.0) }.unwrap();
        }
    }

    #[test]
    fn signal_is_consumed_once_and_credit_survives_until_present() {
        let event = Event::new();
        let mut pacing = FrameLatencyWait::default();
        let (sender, receiver) = mpsc::channel();
        assert!(!pacing.ready(event.0).unwrap());
        pacing
            .arm(event.0, Box::new(move || sender.send(()).unwrap()))
            .unwrap();
        assert!(!pacing.ready(event.0).unwrap());
        event.signal();
        receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(pacing.ready(event.0).unwrap());
        assert!(pacing.ready(event.0).unwrap());
        // SAFETY: live auto-reset event; the registered wait consumed its signal.
        assert_eq!(unsafe { WaitForSingleObject(event.0, 0) }, WAIT_TIMEOUT);
        pacing.consume_ready();
        assert!(!pacing.ready(event.0).unwrap());
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn cancellation_does_not_lose_a_signal_already_consumed_by_the_native_wait() {
        let event = Event::new();
        let mut pacing = FrameLatencyWait::default();
        for _ in 0..128 {
            pacing.arm(event.0, Box::new(|| {})).unwrap();
            event.signal();
            pacing.cancel();
            assert!(pacing.ready(event.0).unwrap());
            pacing.consume_ready();
        }
    }

    #[test]
    fn replacement_reset_and_drop_cancel_pending_callbacks() {
        let event = Event::new();
        let mut pacing = FrameLatencyWait::default();
        let (old_sender, old_receiver) = mpsc::channel();
        pacing
            .arm(event.0, Box::new(move || old_sender.send(()).unwrap()))
            .unwrap();
        let (sender, receiver) = mpsc::channel();
        pacing
            .arm(event.0, Box::new(move || sender.send(()).unwrap()))
            .unwrap();
        event.signal();
        receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(
            old_receiver
                .recv_timeout(Duration::from_millis(20))
                .is_err()
        );
        pacing.reset();
        let (sender, receiver) = mpsc::channel();
        pacing
            .arm(event.0, Box::new(move || sender.send(()).unwrap()))
            .unwrap();
        pacing.reset();
        event.signal();
        assert!(receiver.recv_timeout(Duration::from_millis(20)).is_err());
        assert!(pacing.ready(event.0).unwrap());
        pacing.consume_ready();
        let (sender, receiver) = mpsc::channel();
        pacing
            .arm(event.0, Box::new(move || sender.send(()).unwrap()))
            .unwrap();
        drop(pacing);
        event.signal();
        assert!(receiver.recv_timeout(Duration::from_millis(20)).is_err());
    }
}
