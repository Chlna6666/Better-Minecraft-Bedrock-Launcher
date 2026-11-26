use std::time::Duration;

#[cfg(target_os = "windows")]
#[allow(
    unsafe_code,
    reason = "Win32 thread CPU counters require FFI with live FILETIME outputs"
)]
pub(super) fn thread_time() -> Option<Duration> {
    use windows::Win32::{
        Foundation::FILETIME,
        System::Threading::{GetCurrentThread, GetThreadTimes},
    };
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: The pseudo-handle refers to this thread; all four output pointers are live,
    // aligned FILETIME values. No handle ownership is transferred.
    unsafe {
        GetThreadTimes(
            GetCurrentThread(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    }
    .ok()?;
    let ticks =
        |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    Some(Duration::from_nanos(
        ticks(kernel)
            .saturating_add(ticks(user))
            .saturating_mul(100),
    ))
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[allow(
    unsafe_code,
    reason = "POSIX thread CPU counters require FFI with a live timespec output"
)]
pub(super) fn thread_time() -> Option<Duration> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: CLOCK_THREAD_CPUTIME_ID queries this thread; time is a live, aligned
    // timespec output. A failed query leaves the observation unavailable.
    if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) } != 0 {
        return None;
    }
    Some(Duration::new(
        u64::try_from(time.tv_sec).ok()?,
        u32::try_from(time.tv_nsec).ok()?,
    ))
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "freebsd")))]
pub(super) fn thread_time() -> Option<Duration> {
    None
}
