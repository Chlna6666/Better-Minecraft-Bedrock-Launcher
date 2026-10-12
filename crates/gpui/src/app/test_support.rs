//! Test support for GPUI.
//!
//! GPUI provides first-class support for testing, which includes a macro to run test that rely on having a context,
//! and a test implementation of the `ForegroundExecutor` and `BackgroundExecutor` which ensure that your tests run
//! deterministically even in the face of arbitrary parallelism.
//!
//! The output of the `gpui::test` macro is understood by other rust test runners, so you can use it with `cargo test`
//! or `cargo-nextest`, or another runner of your choice.
//!
//! To make it possible to test collaborative user interfaces (like Zed) you can ask for as many different contexts
//! as you need.
//!
//! ## Example
//!
//! ```
//! use gpui;
//!
//! #[gpui::test]
//! async fn test_example(cx: &TestAppContext) {
//!   assert!(true)
//! }
//!
//! #[gpui::test]
//! async fn test_collaboration_example(cx_a: &TestAppContext, cx_b: &TestAppContext) {
//!   assert!(true)
//! }
//! ```
use crate::{Entity, Subscription, TestAppContext, TestDispatcher};
use futures::StreamExt as _;
use rand::prelude::*;
use smol::channel;
use std::{
    panic::{self, RefUnwindSafe},
    pin::Pin,
};

/// Run the given test function with the configured parameters.
/// This is intended for use with the `gpui::test` macro
/// and generally should not be used directly.
pub fn run_test(
    num_iterations: usize,
    explicit_seeds: &[u64],
    max_retries: usize,
    test_fn: &mut (dyn RefUnwindSafe + Fn(TestDispatcher, u64)),
    on_fail_fn: Option<fn()>,
) {
    let (seeds, is_multiple_runs) = calculate_seeds(num_iterations as u64, explicit_seeds);

    for seed in seeds {
        let mut attempt = 0;
        loop {
            if is_multiple_runs {
                eprintln!("seed = {seed}");
            }
            let result = panic::catch_unwind(|| {
                let dispatcher = TestDispatcher::with_seed(StdRng::seed_from_u64(seed), seed);
                test_fn(dispatcher, seed);
            });

            match result {
                Ok(_) => break,
                Err(error) => {
                    if attempt < max_retries {
                        println!("attempt {} failed, retrying", attempt);
                        attempt += 1;
                        // The panic payload might itself trigger an unwind on drop:
                        // https://doc.rust-lang.org/std/panic/fn.catch_unwind.html#notes
                        std::mem::forget(error);
                    } else {
                        if is_multiple_runs {
                            eprintln!("failing seed: {}", seed);
                        }
                        if let Some(on_fail_fn) = on_fail_fn {
                            on_fail_fn()
                        }
                        panic::resume_unwind(error);
                    }
                }
            }
        }
    }
}

fn calculate_seeds(
    iterations: u64,
    explicit_seeds: &[u64],
) -> (impl Iterator<Item = u64> + '_, bool) {
    // The macro's explicit iteration/seed arguments are the sole source of
    // scheduling inputs. A process environment cannot change this test run.
    let iterations_range = if iterations == 1 && !explicit_seeds.is_empty() {
        0..0
    } else {
        0..iterations
    };
    let iter = iterations_range.chain(explicit_seeds.iter().copied());
    let is_multiple_runs = iter.clone().nth(1).is_some();
    (iter, is_multiple_runs)
}

/// A test struct for converting an observation callback into a stream.
pub struct Observation<T> {
    rx: Pin<Box<channel::Receiver<T>>>,
    _subscription: Subscription,
}

impl<T: 'static> futures::Stream for Observation<T> {
    type Item = T;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.rx.poll_next_unpin(cx)
    }
}

/// observe returns a stream of the change events from the given `Entity`
pub fn observe<T: 'static>(entity: &Entity<T>, cx: &mut TestAppContext) -> Observation<()> {
    let (tx, rx) = smol::channel::unbounded();
    let _subscription = cx.update(|cx| {
        cx.observe(entity, move |_, _| {
            let _ = smol::block_on(tx.send(()));
        })
    });
    let rx = Box::pin(rx);

    Observation { rx, _subscription }
}
