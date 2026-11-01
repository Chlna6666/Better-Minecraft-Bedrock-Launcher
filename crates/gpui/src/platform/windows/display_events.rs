#![expect(
    unsafe_code,
    reason = "a window subclass receives Win32 monitor broadcasts"
)]

use anyhow::{Result, bail};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::{
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::WM_DISPLAYCHANGE,
    },
};
use winit::{
    event_loop::{ActiveEventLoop, EventLoopProxy},
    platform::windows::WindowAttributesExtWindows,
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{Window, WindowAttributes},
};

use super::WindowsUserEvent;

const SUBCLASS_ID: usize = 1;

/// A hidden top-level window receives broadcasts even with no GPUI windows open.
/// Message-only windows do not receive WM_DISPLAYCHANGE.
pub(super) struct DisplayEventListener {
    _window: Window,
    hwnd: HWND,
    _proxy: Box<EventLoopProxy<WindowsUserEvent>>,
}

impl DisplayEventListener {
    pub(super) fn new(
        event_loop: &ActiveEventLoop,
        proxy: EventLoopProxy<WindowsUserEvent>,
    ) -> Result<Self> {
        let window = event_loop.create_window(
            WindowAttributes::default()
                .with_title("GPUI display observer")
                .with_visible(false)
                .with_active(false)
                .with_skip_taskbar(true),
        )?;
        let RawWindowHandle::Win32(handle) = window.window_handle()?.as_raw() else {
            bail!("display observer requires a Win32 window");
        };
        let hwnd = HWND(handle.hwnd.get() as *mut _);
        let proxy = Box::new(proxy);
        let data = (&*proxy as *const EventLoopProxy<WindowsUserEvent>) as usize;
        // SAFETY: The subclass is installed on this thread's live owned window.
        // The boxed proxy has a stable address until Drop removes the subclass.
        if !unsafe { SetWindowSubclass(hwnd, Some(display_event_proc), SUBCLASS_ID, data) }
            .as_bool()
        {
            bail!("failed to subclass the display observer window");
        }
        Ok(Self {
            _window: window,
            hwnd,
            _proxy: proxy,
        })
    }
}

impl Drop for DisplayEventListener {
    fn drop(&mut self) {
        // SAFETY: The window and proxy remain owned until after removal. Drop runs
        // on the owning event-loop thread; an already destroyed HWND has no subclass.
        unsafe {
            RemoveWindowSubclass(self.hwnd, Some(display_event_proc), SUBCLASS_ID);
        }
    }
}

unsafe extern "system" fn display_event_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    if message == WM_DISPLAYCHANGE {
        // SAFETY: Only DisplayEventListener installs this procedure, and it keeps
        // the boxed proxy alive until the subclass is removed on this thread.
        let proxy = unsafe { &*(data as *const EventLoopProxy<WindowsUserEvent>) };
        let _ = proxy.send_event(WindowsUserEvent::DisplaysChanged);
    }
    // SAFETY: Forward this unchanged message to winit's original window procedure.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use winit::{
        application::ApplicationHandler,
        event::StartCause,
        event_loop::{ControlFlow, EventLoop},
        platform::windows::EventLoopBuilderExtWindows,
    };

    struct Probe {
        listener: Option<DisplayEventListener>,
        received: bool,
        deadline: Instant,
        proxy: EventLoopProxy<WindowsUserEvent>,
    }
    impl ApplicationHandler<WindowsUserEvent> for Probe {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            let listener = DisplayEventListener::new(event_loop, self.proxy.clone()).unwrap();
            // SAFETY: The hidden window is owned by this event-loop thread;
            // this synthetic broadcast only queues a user event, without changing a mode.
            unsafe {
                windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                    listener.hwnd,
                    WM_DISPLAYCHANGE,
                    Some(WPARAM(0)),
                    Some(LPARAM(0)),
                );
            }
            self.listener = Some(listener);
        }
        fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WindowsUserEvent) {
            if matches!(event, WindowsUserEvent::DisplaysChanged) {
                self.received = true;
                event_loop.exit();
            }
        }
        fn new_events(&mut self, event_loop: &ActiveEventLoop, _: StartCause) {
            if Instant::now() >= self.deadline {
                event_loop.exit();
            }
        }
        fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.deadline));
        }
        fn window_event(
            &mut self,
            _: &ActiveEventLoop,
            _: winit::window::WindowId,
            _: winit::event::WindowEvent,
        ) {
        }
    }
    #[test]
    #[ignore = "requires a Windows desktop event loop"]
    fn receives_display_broadcast_without_a_gpui_window() {
        let event_loop = EventLoop::<WindowsUserEvent>::with_user_event()
            .with_any_thread(true)
            .build()
            .unwrap();
        let mut probe = Probe {
            listener: None,
            received: false,
            deadline: Instant::now() + Duration::from_secs(5),
            proxy: event_loop.create_proxy(),
        };
        event_loop.run_app(&mut probe).unwrap();
        assert!(probe.received, "monitor broadcast was not delivered");
    }
}
