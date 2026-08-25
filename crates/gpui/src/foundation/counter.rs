use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

/// Increment the given atomic counter if it is not zero and return its new value.
pub(crate) fn atomic_incr_if_not_zero(counter: &AtomicUsize) -> usize {
    let mut loaded = counter.load(SeqCst);
    loop {
        if loaded == 0 {
            return 0;
        }
        match counter.compare_exchange_weak(loaded, loaded + 1, SeqCst, SeqCst) {
            Ok(previous) => return previous + 1,
            Err(actual) => loaded = actual,
        }
    }
}
