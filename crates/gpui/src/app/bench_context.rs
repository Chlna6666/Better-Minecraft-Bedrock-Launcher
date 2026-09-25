use std::{
    cell::RefCell,
    future::Future,
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use crate::{
    AnyView, AnyWindowHandle, App, AppCell, AppContext, BackgroundExecutor, Bounds, Context,
    Entity, ForegroundExecutor, Global, GpuiBorrow, Render, Reservation, Task, TestDispatcher,
    TestPlatform, Window, WindowBounds, WindowHandle, WindowOptions,
};
use rand::{SeedableRng, rngs::StdRng};

const DEFAULT_FPS: u64 = 120;
const NANOS_PER_SECOND: u64 = 1_000_000_000;

#[derive(Default)]
struct BenchReportState {
    frame_callbacks: u64,
}

/// Aggregate metadata emitted alongside Criterion's benchmark statistics.
///
/// Criterion remains the source of truth for wall-clock measurements. This report records how
/// many GPUI platform frame callbacks were actually delivered, making it possible to detect a
/// benchmark that accidentally measures only state mutation without exercising the renderer.
#[derive(Clone)]
pub struct BenchReport {
    frame_budget: Duration,
    state: Rc<RefCell<BenchReportState>>,
}

impl Default for BenchReport {
    fn default() -> Self {
        Self::with_fps(DEFAULT_FPS)
    }
}

impl BenchReport {
    /// Creates a report using one frame at `fps` as the comparison budget.
    ///
    /// # Panics
    ///
    /// Panics when `fps` is zero.
    pub fn with_fps(fps: u64) -> Self {
        assert!(fps > 0, "frame rate must be greater than zero");
        Self {
            frame_budget: Duration::from_nanos((NANOS_PER_SECOND / fps).max(1)),
            state: Rc::new(RefCell::new(BenchReportState::default())),
        }
    }

    fn record_frame_callback(&self) {
        let mut state = self.state.borrow_mut();
        state.frame_callbacks = state.frame_callbacks.saturating_add(1);
    }

    /// Prints GPUI-specific benchmark metadata to stderr.
    pub fn print(&self, benchmark_name: Option<&'static str>) {
        let state = self.state.borrow();
        if state.frame_callbacks == 0 {
            return;
        }
        eprintln!(
            "GPUI bench report (all observed iterations): {}",
            benchmark_name.unwrap_or("unknown benchmark")
        );
        eprintln!("  target frame budget: {:?}", self.frame_budget);
        eprintln!("  delivered platform frame callbacks: {}", state.frame_callbacks);
    }
}

/// GPUI application context used by Criterion benchmarks.\n///\n/// The benchmark platform exercises GPUI's CPU-side lifecycle, layout, retained replay and\n/// platform frame-callback scheduling. Nova GPU upload/submit cost remains covered by the\n/// dedicated renderer microbenchmarks and must not be inferred from this headless platform.
///
/// Unlike `TestAppContext`, this context does not make effect flushing draw dirty windows.
/// Updates request a platform frame and the benchmark harness explicitly delivers scheduled frame
/// callbacks between foreground task polls. This keeps renderer benchmarks shaped like production
/// without pulling the complete `test-support` feature into `bench-support`.
#[derive(Clone)]
pub struct BenchAppContext<'a, 'measurement> {
    app: Rc<AppCell>,
    background_executor: BackgroundExecutor,
    foreground_executor: ForegroundExecutor,
    dispatcher: Arc<TestDispatcher>,
    benchmark_name: Option<&'static str>,
    bencher: Rc<RefCell<Option<&'a mut criterion::Bencher<'measurement>>>>,
    report: BenchReport,
}

impl<'a, 'measurement> BenchAppContext<'a, 'measurement> {
    /// Creates a benchmark context with the default GPUI report.
    pub fn new(
        benchmark_name: Option<&'static str>,
        bencher: &'a mut criterion::Bencher<'measurement>,
    ) -> Self {
        Self::new_with_report(benchmark_name, bencher, BenchReport::default())
    }

    /// Creates a benchmark context with an explicit report configuration.
    #[doc(hidden)]
    pub fn new_with_report(
        benchmark_name: Option<&'static str>,
        bencher: &'a mut criterion::Bencher<'measurement>,
        report: BenchReport,
    ) -> Self {
        let dispatcher = Arc::new(TestDispatcher::with_seed(StdRng::seed_from_u64(0), 0));
        let background_executor = BackgroundExecutor::new(dispatcher.clone());
        let foreground_executor = ForegroundExecutor::new(dispatcher.clone());
        let platform =
            TestPlatform::new(background_executor.clone(), foreground_executor.clone());
        let app = App::new_app(
            platform,
            Arc::new(()),
            Arc::new(http_client::BlockedHttpClient::new()),
        );

        Self {
            app,
            background_executor,
            foreground_executor,
            dispatcher,
            benchmark_name,
            bencher: Rc::new(RefCell::new(Some(bencher))),
            report,
        }
    }

    /// Returns the benchmark function name that created this context.
    pub fn benchmark_name(&self) -> Option<&'static str> {
        self.benchmark_name
    }

    /// Returns the benchmark's background executor.
    pub fn background_executor(&self) -> &BackgroundExecutor {
        &self.background_executor
    }

    /// Returns the benchmark's foreground executor.
    pub fn foreground_executor(&self) -> &ForegroundExecutor {
        &self.foreground_executor
    }

    /// Updates application state and flushes synchronous GPUI effects.
    pub fn update<R>(&mut self, update: impl FnOnce(&mut App) -> R) -> R {
        self.app.borrow_mut().update(update)
    }

    /// Reads application state.
    pub fn read<R>(&self, read: impl FnOnce(&App) -> R) -> R {
        let app = self.app.borrow();
        read(&app)
    }

    /// Adds a benchmark window and settles its initial activation/frame work.
    pub fn add_window<V>(
        &mut self,
        build_root: impl FnOnce(&mut Window, &mut Context<V>) -> V,
    ) -> WindowHandle<V>
    where
        V: 'static + Render,
    {
        let bounds = {
            let app = self.app.borrow();
            Bounds::maximized(None, &app)
        };
        let window = self
            .app
            .borrow_mut()
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| build_root(window, cx)),
            )
            .expect("failed to open benchmark window");

        self.run_until_idle();
        window
    }

    /// Runs ready work while delivering scheduled platform frames after task polls.
    ///
    /// A final frame callback is delivered after the task queue empties. If that frame schedules
    /// only another animation frame, the method returns instead of running an unbounded animation
    /// loop. Work woken by the final frame is drained before returning.
    pub fn run_until_idle(&self) {
        loop {
            while self.dispatcher.tick(false) {
                self.dispatch_pending_frames();
            }

            if !self.dispatch_pending_frames() {
                break;
            }

            if !self.dispatcher.tick(false) {
                break;
            }
            self.dispatch_pending_frames();
        }
    }

    /// Measures a generic GPUI workload with Criterion and delivers the frame batch requested by
    /// each iteration after the workload returns.
    pub fn bench_iter(&mut self, mut benchmark: impl FnMut(&mut Self)) {
        let bencher = self.take_bencher("bench_iter");
        self.dispatch_pending_frames();
        let mut measured = || {
            benchmark(self);
            self.dispatch_pending_frames();
        };
        bencher.iter(&mut measured);
        self.replace_bencher(bencher);
    }

    /// Measures an unpaced root-view update and the platform frame it schedules.
    ///
    /// This measures GPUI update/render work, not display latency or vsync pacing. One ready task
    /// is serviced before the update so already-queued foreground work can delay the frame as it
    /// does in production. The scheduled frame is then delivered through the platform callback
    /// instead of calling `Window::draw` synchronously from the benchmark.
    pub fn bench_renderer<V>(
        &mut self,
        window: WindowHandle<V>,
        mut update: impl FnMut(&mut V, &mut Window, &mut Context<V>),
    ) where
        V: 'static + Render,
    {
        let bencher = self.take_bencher("bench_renderer");
        self.dispatch_pending_frames();

        let mut measured = || {
            if self.dispatcher.tick(false) {
                self.dispatch_pending_frames();
            }
            window
                .update(self, |view, window, cx| update(view, window, cx))
                .expect("benchmark window was unexpectedly closed");
            self.dispatch_pending_frames();
        };
        bencher.iter(&mut measured);
        self.replace_bencher(bencher);
    }

    fn dispatch_pending_frames(&self) -> bool {
        let pending = {
            let mut app = self.app.borrow_mut();
            app.windows
                .values_mut()
                .filter_map(|window| {
                    let window = window.as_deref_mut()?;
                    let platform_window = window.platform_window.as_test()?;
                    platform_window
                        .frame_scheduled()
                        .then(|| platform_window.clone())
                })
                .collect::<Vec<_>>()
        };

        let mut dispatched = false;
        for window in pending {
            if window.simulate_scheduled_frame() {
                self.report.record_frame_callback();
                dispatched = true;
            }
        }
        dispatched
    }

    fn take_bencher(&self, kind: &str) -> &'a mut criterion::Bencher<'measurement> {
        self.bencher.borrow_mut().take().unwrap_or_else(|| {
            panic!("cannot start {kind}: benchmark measurement is already running")
        })
    }

    fn replace_bencher(&self, bencher: &'a mut criterion::Bencher<'measurement>) {
        let previous = self.bencher.borrow_mut().replace(bencher);
        assert!(
            previous.is_none(),
            "benchmark bencher was unexpectedly present after measurement"
        );
    }

    /// Releases benchmark windows and drains work they wake.
    pub fn teardown(mut self) {
        self.run_until_idle();
        self.update(|app| {
            app.windows.clear();
            app.window_handles.clear();
        });
        self.run_until_idle();
    }
}

impl AppContext for BenchAppContext<'_, '_> {
    type Result<T> = T;

    fn new<T: 'static>(
        &mut self,
        build_entity: impl FnOnce(&mut Context<T>) -> T,
    ) -> Entity<T> {
        self.app.borrow_mut().new(build_entity)
    }

    fn reserve_entity<T: 'static>(&mut self) -> Reservation<T> {
        self.app.borrow_mut().reserve_entity()
    }

    fn insert_entity<T: 'static>(
        &mut self,
        reservation: Reservation<T>,
        build_entity: impl FnOnce(&mut Context<T>) -> T,
    ) -> Entity<T> {
        self.app.borrow_mut().insert_entity(reservation, build_entity)
    }

    fn update_entity<T: 'static, R>(
        &mut self,
        handle: &Entity<T>,
        update: impl FnOnce(&mut T, &mut Context<T>) -> R,
    ) -> R {
        self.app.borrow_mut().update_entity(handle, update)
    }

    fn as_mut<'b, T>(&'b mut self, _: &Entity<T>) -> GpuiBorrow<'b, T>
    where
        T: 'static,
    {
        panic!("Cannot use as_mut with BenchAppContext. Call update() instead.")
    }

    fn read_entity<T, R>(
        &self,
        handle: &Entity<T>,
        read: impl FnOnce(&T, &App) -> R,
    ) -> R
    where
        T: 'static,
    {
        self.app.borrow().read_entity(handle, read)
    }

    fn update_window<T, F>(&mut self, window: AnyWindowHandle, update: F) -> anyhow::Result<T>
    where
        F: FnOnce(AnyView, &mut Window, &mut App) -> T,
    {
        self.app.borrow_mut().update_window(window, update)
    }

    fn read_window<T, R>(
        &self,
        window: &WindowHandle<T>,
        read: impl FnOnce(Entity<T>, &App) -> R,
    ) -> anyhow::Result<R>
    where
        T: 'static,
    {
        self.app.borrow().read_window(window, read)
    }

    fn background_spawn<R>(&self, future: impl Future<Output = R> + Send + 'static) -> Task<R>
    where
        R: Send + 'static,
    {
        self.background_executor.spawn(future)
    }

    fn read_global<G, R>(&self, callback: impl FnOnce(&G, &App) -> R) -> R
    where
        G: Global,
    {
        self.app.borrow().read_global(callback)
    }
}
