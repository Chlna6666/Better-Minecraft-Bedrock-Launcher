//! Compositor frame callbacks on a dedicated native queue, independent of GPUI UI dispatch.
//! One connection-owned thread serves every window, sleeps in calloop when idle, and joins on
//! connection teardown. Coalesced frame-only requests are armed before GPU submission; only the
//! actual buffer commit applies them, so this queue cannot prematurely commit pending UI state.

use anyhow::{Context, Result, anyhow};
use calloop::{EventLoop, channel};
use calloop_wayland_source::WaylandSource;
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
};
use wayland_client::{
    Connection, Dispatch, QueueHandle,
    protocol::{wl_callback, wl_surface},
};

type Tick = Arc<dyn Fn() + Send + Sync>;

enum Command {
    Register(u64, Entry),
    Request(u64, u64, mpsc::SyncSender<()>),
    Remove(u64, mpsc::SyncSender<()>),
    Shutdown,
}

#[derive(Default)]
struct Signal {
    state: Mutex<RequestState>,
}

#[derive(Default)]
struct RequestState {
    visible: bool,
    requested: bool,
    generation: u64,
}

impl Signal {
    fn request(&self) -> Option<u64> {
        let mut state = self.state.lock();
        if !state.visible || state.requested {
            return None;
        }
        state.requested = true;
        Some(state.generation)
    }

    fn set_visible(&self, visible: bool) -> bool {
        let mut state = self.state.lock();
        if state.visible == visible {
            return false;
        }
        state.visible = visible;
        state.generation = state.generation.wrapping_add(1);
        state.requested = false;
        visible
    }

    fn is_current(&self, generation: u64) -> bool {
        let state = self.state.lock();
        state.visible && state.requested && state.generation == generation
    }

    fn complete(&self, generation: u64) -> bool {
        let mut state = self.state.lock();
        if !state.visible || state.generation != generation || !state.requested {
            return false;
        }
        state.requested = false;
        true
    }
}

pub(super) struct Clock {
    sender: channel::Sender<Command>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Clock {
    pub(super) fn new(connection: Connection) -> Result<Arc<Self>> {
        let (sender, receiver) = channel::channel();
        let (ready, initialized) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("gpui-wayland-presentation".into())
            .spawn(move || {
                let result = run(connection, receiver, ready);
                if let Err(error) = result {
                    log::error!("Wayland presentation clock stopped: {error:#}");
                }
            })?;
        let clock = Arc::new(Self {
            sender,
            thread: Mutex::new(Some(thread)),
        });
        initialized
            .recv()
            .context("Wayland presentation clock initialization stopped")??;
        Ok(clock)
    }

    pub(super) fn register(
        self: &Arc<Self>,
        surface: wl_surface::WlSurface,
        tick: Tick,
    ) -> Result<FrameClock> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let signal = Arc::new(Signal::default());
        self.sender
            .send(Command::Register(
                id,
                Entry {
                    surface,
                    signal: signal.clone(),
                    tick: tick.clone(),
                    callback_pending: None,
                },
            ))
            .map_err(|_| anyhow!("Wayland presentation clock stopped before registration"))?;
        Ok(FrameClock {
            clock: self.clone(),
            id,
            signal,
            tick,
            closed: false,
        })
    }
}

impl Drop for Clock {
    fn drop(&mut self) {
        if self.sender.send(Command::Shutdown).is_err() {
            log::trace!("Wayland presentation clock already stopped");
        }
        if let Some(thread) = self.thread.get_mut().take()
            && thread.join().is_err()
        {
            log::error!("Wayland presentation clock panicked during shutdown");
        }
    }
}

pub(super) struct FrameClock {
    clock: Arc<Clock>,
    id: u64,
    signal: Arc<Signal>,
    tick: Tick,
    closed: bool,
}

impl FrameClock {
    pub(super) fn request_callback(&self) -> Tick {
        let sender = self.clock.sender.clone();
        let id = self.id;
        let signal = self.signal.clone();
        Arc::new(move || {
            let Some(generation) = signal.request() else {
                return;
            };
            let (reply, armed) = mpsc::sync_channel(1);
            if sender
                .send(Command::Request(id, generation, reply))
                .is_err()
                || armed.recv().is_err()
            {
                signal.complete(generation);
                log::trace!("discarding Wayland frame request after shutdown");
            }
        })
    }

    pub(super) fn set_visible(&self, visible: bool) {
        if self.signal.set_visible(visible) {
            // A restored retained animation may have no producer report to restart it.
            // GPU submission will arm its callback; a static scene rejects this tick.
            (self.tick)();
        }
    }

    pub(super) fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.signal.set_visible(false);
        let (reply, removed) = mpsc::sync_channel(1);
        if self
            .clock
            .sender
            .send(Command::Remove(self.id, reply))
            .is_err()
            || removed.recv().is_err()
        {
            log::trace!("Wayland frame source already stopped before window destruction");
        }
    }
}

impl Drop for FrameClock {
    fn drop(&mut self) {
        self.close();
    }
}

struct Entry {
    surface: wl_surface::WlSurface,
    signal: Arc<Signal>,
    tick: Tick,
    callback_pending: Option<u64>,
}

struct State {
    qh: QueueHandle<Self>,
    entries: HashMap<u64, Entry>,
    closing: bool,
}

impl State {
    fn command(&mut self, command: Command) {
        match command {
            Command::Register(id, entry) => {
                self.entries.insert(id, entry);
            }
            Command::Request(id, generation, reply) => {
                if let Some(entry) = self.entries.get_mut(&id) {
                    if entry.signal.is_current(generation)
                        && entry.callback_pending != Some(generation)
                    {
                        entry.callback_pending = Some(generation);
                        entry.surface.frame(&self.qh, (id, generation));
                    }
                }
                // The GPU owner commits this callback with its next buffer. An empty commit here
                // could apply UI scale/region/CSD state before that UI transaction is complete.
                if reply.send(()).is_err() {
                    log::trace!("Wayland frame arming caller dropped");
                }
            }
            Command::Remove(id, reply) => {
                self.entries.remove(&id);
                if reply.send(()).is_err() {
                    log::trace!("Wayland frame source removal caller dropped");
                }
            }
            Command::Shutdown => {
                self.entries.clear();
                self.closing = true;
            }
        }
    }
}

impl Dispatch<wl_callback::WlCallback, (u64, u64)> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        &(id, generation): &(u64, u64),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let Some(entry) = state.entries.get_mut(&id) {
            if entry.callback_pending == Some(generation) {
                entry.callback_pending = None;
                if entry.signal.complete(generation) {
                    (entry.tick)();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Signal;

    #[test]
    fn requests_coalesce_until_callback_completes() {
        let signal = Signal::default();
        assert_eq!(signal.request(), None);
        assert!(signal.set_visible(true));
        let generation = signal.request().expect("visible request");
        assert_eq!(signal.request(), None);
        assert!(signal.is_current(generation));
        assert!(signal.complete(generation));
        assert!(!signal.complete(generation));
        assert_eq!(signal.request(), Some(generation));
    }

    #[test]
    fn restored_window_rejects_callback_from_before_hiding() {
        let signal = Signal::default();
        signal.set_visible(true);
        let old = signal.request().expect("initial request");
        signal.set_visible(false);
        assert!(!signal.is_current(old));
        assert_eq!(signal.request(), None);
        signal.set_visible(true);
        let current = signal.request().expect("restored request");
        assert_ne!(old, current);
        assert!(!signal.complete(old));
        assert_eq!(signal.request(), None);
        assert!(signal.complete(current));
    }
}

fn run(
    connection: Connection,
    receiver: channel::Channel<Command>,
    ready: mpsc::SyncSender<Result<()>>,
) -> Result<()> {
    let initialized = (|| {
        let mut event_loop = EventLoop::<State>::try_new()?;
        let queue = connection.new_event_queue::<State>();
        let qh = queue.handle();
        WaylandSource::new(connection, queue).insert(event_loop.handle())?;
        event_loop
            .handle()
            .insert_source(receiver, |event, _, state| match event {
                channel::Event::Msg(command) => state.command(command),
                channel::Event::Closed => state.closing = true,
            })
            .map_err(|error| anyhow!("failed to register Wayland frame commands: {error}"))?;
        Ok::<_, anyhow::Error>((
            event_loop,
            State {
                qh,
                entries: HashMap::new(),
                closing: false,
            },
        ))
    })();
    let (mut event_loop, mut state) = match initialized {
        Ok(initialized) => {
            ready
                .send(Ok(()))
                .map_err(|_| anyhow!("Wayland frame clock caller dropped"))?;
            initialized
        }
        Err(error) => {
            if ready.send(Err(error)).is_err() {
                log::trace!("Wayland clock error receiver dropped");
            }
            return Ok(());
        }
    };
    while !state.closing {
        event_loop.dispatch(None, &mut state)?;
    }
    Ok(())
}
