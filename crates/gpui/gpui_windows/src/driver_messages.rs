#[allow(unsafe_code)]
/// Dispatches pending synchronous Win32 driver messages on the current thread.
pub fn service_driver_messages() {
    use windows::Win32::UI::WindowsAndMessaging::{
        MSG, PM_NOREMOVE, PM_QS_SENDMESSAGE, PeekMessageW, WM_NULL,
    };
    // DXGI presentation and COM release can synchronously send to the HWND
    // thread. Keep those sends moving while retaining the destruction barrier;
    // posted input and application callbacks are not consumed here.
    let mut message = MSG::default();
    // SAFETY: The output is stack-owned. PeekMessage itself dispatches sends;
    // its BOOL only reports queued-message availability, not success/failure.
    let _ = unsafe {
        PeekMessageW(
            &mut message,
            None,
            WM_NULL,
            WM_NULL,
            PM_NOREMOVE | PM_QS_SENDMESSAGE,
        )
    };
}
