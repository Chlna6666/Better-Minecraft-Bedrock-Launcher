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
    Connection, Dispatch, QueueHandle, globals::GlobalList,
    protocol::{wl_callback, wl_surface},
};

use wayland_protocols::wp::presentation_time::client::{wp_presentation, wp_presentation_feedback};

type Tick = Arc<dyn Fn() + Send + Sync>;
type FeedbackMailbox = Arc<Mutex<Option<gfx_core::PresentationFeedback>>>;

enum Command {
    Register(u64, Entry, mpsc::SyncSender<()>),
    Remove(u64, mpsc::SyncSender<()>),
    Shutdown,
}

#[derive(Default)]
struct Signal {
    state: Mutex<RequestState>,
    feedback_mailbox: Option<FeedbackMailbox>,
}

#[derive(Default)]
struct RequestState {
    visible: bool,
    requested: bool,
    generation: u64,
    // Keep at most one outstanding feedback object per surface, independently of frame callbacks.
    feedback_pending: Option<(u64, bool)>,
}

impl Signal {
    /// Issue the native request while holding the visibility/lifetime gate.
    /// close() cannot destroy the surface concurrently with this operation.
    fn arm(&self, feedback_available: bool, request: impl FnOnce(u64, bool)) -> Option<u64> {
        let mut state = self.state.lock();
        if !state.visible || state.requested {
            return None;
        }
        state.generation = state.generation.wrapping_add(1);
        state.requested = true;
        let feedback = feedback_available && state.feedback_pending.is_none();
        if feedback {
            state.feedback_pending = Some((state.generation, true));
        }
        request(state.generation, feedback);
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
        if let Some(mailbox) = &self.feedback_mailbox {
            *mailbox.lock() = None;
        }
        if let Some((_, valid)) = state.feedback_pending.as_mut() {
            *valid = false;
        }
        visible
    }

    fn complete_feedback(&self, generation: u64, feedback: Option<gfx_core::PresentationFeedback>) -> bool {
        let mut state = self.state.lock();
        match state.feedback_pending {
            Some((pending, valid)) if pending == generation => {
                state.feedback_pending = None;
                let accepted = valid && state.visible;
                // Publish under the same visibility gate so hide/close cannot
                // clear the mailbox and then race with a stale event.
                if accepted && let Some(sample) = feedback && let Some(mailbox) = &self.feedback_mailbox {
                    *mailbox.lock() = Some(sample);
                }
                accepted
            }
            _ => false,
        }
    }

    #[cfg(test)]
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
    qh: QueueHandle<State>,
    presentation: Option<wp_presentation::WpPresentation>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Clock {
    pub(super) fn new(connection: Connection, globals: &GlobalList) -> Result<Arc<Self>> {
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
        let qh = initialized
            .recv()
            .context("Wayland presentation clock initialization stopped")??;
        // Reuse the UI's existing registry; no additional roundtrip or event thread.
        let presentation = globals.bind(&qh, 1..=1, ()).ok();
        Ok(Arc::new(Self {
            sender,
            qh,
            presentation,
            thread: Mutex::new(Some(thread)),
        }))
    }

    pub(super) fn register(
        self: &Arc<Self>,
        surface: wl_surface::WlSurface,
        tick: Tick,
        metrics_id: u64,
        feedback_mailbox: FeedbackMailbox,
    ) -> Result<FrameClock> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let signal = Arc::new(Signal {
            feedback_mailbox: Some(feedback_mailbox),
            ..Signal::default()
        });
        let (reply, registered) = mpsc::sync_channel(1);
        self.sender
            .send(Command::Register(
                id,
                Entry {
                    signal: signal.clone(),
                    tick: tick.clone(),
                    metrics_id,
                },
                reply,
            ))
            .map_err(|_| anyhow!("Wayland presentation clock stopped before registration"))?;
        // Only window creation has a cross-thread handshake. The entry must
        // exist before any frame callback is delivered to this event queue.
        registered.recv().context("Wayland presentation registration stopped")?;
        Ok(FrameClock {
            clock: self.clone(),
            id,
            surface,
            signal,
            tick,
            closed: false,
        })
    }
}

impl Drop for Clock {
    fn drop(&mut self) {
        if let Some(presentation) = self.presentation.take() {
            presentation.destroy();
        }
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
    surface: wl_surface::WlSurface,
    signal: Arc<Signal>,
    tick: Tick,
    closed: bool,
}

impl FrameClock {
    pub(super) fn request_callback(&self) -> Tick {
        let qh = self.clock.qh.clone();
        let surface = self.surface.clone();
        let id = self.id;
        let signal = self.signal.clone();
        let presentation = self.clock.presentation.clone();
        Arc::new(move || {
            // QueueHandle is thread-safe: the callback is still dispatched on
            // the dedicated event queue, but its request is sent by the GPU
            // owner itself, in order before the corresponding WSI commit.
            let _ = signal.arm(presentation.is_some(), |generation, feedback| {
                surface.frame(&qh, (id, generation));
                if feedback && let Some(presentation) = &presentation {
                    presentation.feedback(&surface, &qh, (id, generation));
                }
            });
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
    signal: Arc<Signal>,
    tick: Tick,
    metrics_id: u64,
}

struct State {
    qh: QueueHandle<Self>,
    entries: HashMap<u64, Entry>,
    presentation_clock_id: Option<u32>,
    closing: bool,
}

impl State {
    fn command(&mut self, command: Command) {
        match command {
            Command::Register(id, entry, reply) => {
                self.entries.insert(id, entry);
                if reply.send(()).is_err() {
                    self.entries.remove(&id);
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
        if let Some(entry) = state.entries.get(&id)
            && entry.signal.complete(generation)
        {
            (entry.tick)();
        }
    }
}

impl Dispatch<wp_presentation::WpPresentation, ()> for State {
    fn event(
        state: &mut Self,
        _: &wp_presentation::WpPresentation,
        event: wp_presentation::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_presentation::Event::ClockId { clk_id } = event {
            state.presentation_clock_id = Some(clk_id);
        }
    }
}

impl Dispatch<wp_presentation_feedback::WpPresentationFeedback, (u64, u64)> for State {
    fn event(
        state: &mut Self,
        _: &wp_presentation_feedback::WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        &(id, generation): &(u64, u64),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use crate::diagnostics::performance_metrics::window::record_wayland_presentation_feedback;
        use wp_presentation_feedback::Event;
        let Some(entry) = state.entries.get(&id) else { return };
        match event {
            Event::Presented { tv_sec_hi, tv_sec_lo, tv_nsec, refresh, seq_hi, seq_lo, .. } => {
                let seconds = (u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo);
                let timestamp = seconds.checked_mul(1_000_000_000)
                    .and_then(|value| value.checked_add(u64::from(tv_nsec)));
                let sequence = (u64::from(seq_hi) << 32) | u64::from(seq_lo);
                let refresh = (refresh != 0).then_some(refresh);
                let sample = state.presentation_clock_id.zip(timestamp).map(|(clock_id, actual_present_ns)| {
                    gfx_core::PresentationFeedback::Wayland {
                        clock_id, actual_present_ns, refresh_duration_ns: refresh.map(u64::from), sequence,
                    }
                });
                if entry.signal.complete_feedback(generation, sample) {
                    record_wayland_presentation_feedback(entry.metrics_id, state.presentation_clock_id,
                        true, timestamp, refresh, Some(sequence));
                }
            }
            Event::Discarded => {
                if entry.signal.complete_feedback(generation, None) {
                    record_wayland_presentation_feedback(entry.metrics_id, state.presentation_clock_id,
                        false, None, None, None);
                }
            }
            // SyncOutput is informational and must not consume the one-shot feedback.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Signal;

    #[test]
    fn requests_coalesce_until_callback_completes() {
        let signal = Signal::default();
        assert_eq!(signal.arm(false, |_, _| {}), None);
        assert!(signal.set_visible(true));
        let generation = signal.arm(false, |_, _| {}).expect("visible request");
        assert_eq!(signal.arm(false, |_, _| {}), None);
        assert!(signal.is_current(generation));
        assert!(signal.complete(generation));
        assert!(!signal.complete(generation));
        let next = signal.arm(false, |_, _| {}).expect("next request");
        assert_ne!(next, generation);
        assert!(!signal.complete(generation));
        assert!(signal.complete(next));
    }

    #[test]
    fn feedback_does_not_gate_frame_callbacks_or_survive_visibility_epoch() {
        let signal = Signal::default();
        signal.set_visible(true);
        let first = signal.arm(true, |_, feedback| assert!(feedback)).unwrap();
        assert!(signal.complete(first));
        let next = signal.arm(true, |_, feedback| assert!(!feedback)).unwrap();
        assert!(signal.complete(next));
        signal.set_visible(false);
        signal.set_visible(true);
        assert!(!signal.complete_feedback(first, None));
        let latest = signal.arm(true, |_, feedback| assert!(feedback)).unwrap();
        assert!(!signal.complete_feedback(first, None));
        assert!(signal.complete_feedback(latest, None));
    }

    #[test]
    fn feedback_mailbox_is_cleared_on_hide_and_ignores_stale_feedback() {
        use std::sync::Arc;
        use parking_lot::Mutex;
        let mailbox = Arc::new(Mutex::new(None));
        let signal = Signal { feedback_mailbox: Some(mailbox.clone()), ..Signal::default() };
        let sample = gfx_core::PresentationFeedback::Wayland {
            clock_id: 1, actual_present_ns: 100, refresh_duration_ns: None, sequence: 0,
        };
        signal.set_visible(true);
        let first = signal.arm(true, |_, _| {}).unwrap();
        assert!(signal.complete_feedback(first, Some(sample)));
        assert_eq!(*mailbox.lock(), Some(sample));
        assert!(signal.complete(first));
        let second = signal.arm(true, |_, _| {}).unwrap();
        signal.set_visible(false);
        assert_eq!(*mailbox.lock(), None);
        signal.set_visible(true);
        assert!(!signal.complete_feedback(second, Some(sample)));
        assert_eq!(*mailbox.lock(), None);
    }

    #[test]
    fn restored_window_rejects_callback_from_before_hiding() {
        let signal = Signal::default();
        signal.set_visible(true);
        let old = signal.arm(false, |_, _| {}).expect("initial request");
        signal.set_visible(false);
        assert!(!signal.is_current(old));
        assert_eq!(signal.arm(false, |_, _| {}), None);
        signal.set_visible(true);
        let current = signal.arm(false, |_, _| {}).expect("restored request");
        assert_ne!(old, current);
        assert!(!signal.complete(old));
        assert_eq!(signal.arm(false, |_, _| {}), None);
        assert!(signal.complete(current));
    }
}

fn run(
    connection: Connection,
    receiver: channel::Channel<Command>,
    ready: mpsc::SyncSender<Result<QueueHandle<State>>>,
) -> Result<()> {
    let initialized = (|| {
        let mut event_loop = EventLoop::<State>::try_new()?;
        let queue = connection.new_event_queue::<State>();
        let qh = queue.handle();
        // InsertError owns a WaylandSource (and raw wl_display pointers), so it is
        // not Send + Sync and cannot be converted directly into anyhow::Error.
        // Preserve the diagnostic without moving the Wayland source across threads.
        WaylandSource::new(connection, queue)
            .insert(event_loop.handle())
            .map_err(|error| anyhow!("failed to register Wayland presentation source: {error}"))?;
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
                presentation_clock_id: None,
                closing: false,
            },
        ))
    })();
    let (mut event_loop, mut state) = match initialized {
        Ok(initialized) => {
            ready
                .send(Ok(initialized.1.qh.clone()))
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
