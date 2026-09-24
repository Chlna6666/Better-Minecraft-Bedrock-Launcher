//! Portable touch recognition and platform-tunable fling physics.

use std::{
    collections::VecDeque,
    mem,
    sync::LazyLock,
    time::{Duration, Instant},
};

use smallvec::SmallVec;

use crate::{
    Axis, LongPressEvent, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Pixels, Point,
    ScrollDelta, ScrollWheelEvent, TouchDragEvent, TouchEvent, TouchId, TouchPhase, point, px,
};

const SCROLL_EVENT_SEPARATION: Duration = Duration::from_millis(28);
const VELOCITY_WINDOW: Duration = Duration::from_millis(100);
const VELOCITY_ASSUME_STOPPED_GAP: Duration = Duration::from_millis(40);
const VELOCITY_MAX_SAMPLES: usize = 20;
const MAX_FLING_VELOCITY: f32 = 8000.0;
const MOMENTUM_STOP_VELOCITY: f32 = 10.0;

fn dominant_axis(delta: Point<Pixels>) -> Axis {
    if delta.x.abs() <= delta.y.abs() {
        Axis::Vertical
    } else {
        Axis::Horizontal
    }
}

fn lock_delta_to_axis(delta: &mut Point<Pixels>, axis: Axis) {
    match axis {
        Axis::Vertical => delta.x = px(0.0),
        Axis::Horizontal => delta.y = px(0.0),
    }
}

fn movements_oppose(left: Point<Pixels>, right: Point<Pixels>) -> bool {
    f32::from(left.x) * f32::from(right.x) + f32::from(left.y) * f32::from(right.y) < 0.0
}


/// Tracks the dominant axis across one precise scroll gesture.
///
/// Touchpads and touch-derived wheel streams often contain small motion on the orthogonal axis.
/// Locking the gesture rather than each individual event prevents alternating X/Y jitter while
/// still allowing a deliberate strong direction change to unlock.
#[derive(Clone, Copy, Debug, Default)]
pub struct OngoingScroll {
    last_event: Option<Instant>,
    axis: Option<Axis>,
}

impl OngoingScroll {
    /// Filters a precise-scroll delta to the dominant axis of the current gesture.
    pub fn filter(&mut self, delta: &mut Point<Pixels>, touch_phase: TouchPhase) {
        const UNLOCK_PERCENT: f32 = 1.9;
        const UNLOCK_LOWER_BOUND: Pixels = Pixels(6.0);

        if matches!(touch_phase, TouchPhase::Ended | TouchPhase::Cancelled) {
            self.last_event = None;
            self.axis = None;
            return;
        }

        let x = delta.x.abs();
        let y = delta.y.abs();
        if x == Pixels::ZERO && y == Pixels::ZERO {
            if touch_phase == TouchPhase::Started {
                self.last_event = None;
                self.axis = None;
            }
            return;
        }

        let now = Instant::now();
        let starts_new_gesture = touch_phase == TouchPhase::Started
            || self
                .last_event
                .map(|last| now.saturating_duration_since(last) >= SCROLL_EVENT_SEPARATION)
                .unwrap_or(true);

        let mut axis = self.axis;
        if starts_new_gesture {
            axis = Some(dominant_axis(*delta));
        } else if x.max(y) >= UNLOCK_LOWER_BOUND {
            match axis {
                Some(Axis::Vertical) if x > y && x >= y * UNLOCK_PERCENT => axis = None,
                Some(Axis::Horizontal) if y > x && y >= x * UNLOCK_PERCENT => axis = None,
                _ => {}
            }
        }

        self.last_event = Some(now);
        self.axis = axis;
        if let Some(axis) = axis {
            lock_delta_to_axis(delta, axis);
        }
    }
}

/// Feel constants consumed by the portable touch recognizer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GestureTuning {
    /// Distance a touch may travel before it ceases to be a potential tap.
    pub touch_slop: Pixels,
    /// Maximum interval between taps that contribute to a multi-tap count.
    pub multi_tap_interval: Duration,
    /// Maximum distance between taps that contribute to a multi-tap count.
    pub multi_tap_slop: Pixels,
    /// Hold duration before long press is offered.
    pub long_press_duration: Duration,
    /// Momentum model used after a pan is released.
    pub scroll_physics: ScrollPhysics,
    /// Minimum release speed in logical pixels per second that launches momentum.
    pub min_fling_velocity: f32,
}

impl Default for GestureTuning {
    fn default() -> Self {
        Self {
            touch_slop: px(8.0),
            multi_tap_interval: Duration::from_millis(400),
            multi_tap_slop: px(16.0),
            long_press_duration: Duration::from_millis(500),
            scroll_physics: ScrollPhysics::ios(),
            min_fling_velocity: 50.0,
        }
    }
}

/// Closed-form momentum model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScrollPhysics {
    /// iOS-style exponential velocity decay.
    Exponential {
        /// Per-millisecond velocity multiplier.
        decay_per_ms: f32,
    },
    /// Android OverScroller-style friction spline.
    FrictionSpline {
        /// Scroll friction coefficient.
        friction: f32,
        /// Pixels per inch in the coordinate space used by the fling.
        pixels_per_inch: f32,
    },
}

impl ScrollPhysics {
    /// Returns the normal iOS deceleration model.
    pub const fn ios() -> Self {
        Self::Exponential {
            decay_per_ms: 0.998,
        }
    }

    /// Returns Android OverScroller defaults in logical pixels.
    pub const fn android() -> Self {
        Self::FrictionSpline {
            friction: 0.015,
            pixels_per_inch: 160.0,
        }
    }

    fn fling_duration(self, speed: f32) -> Duration {
        match self {
            Self::Exponential { decay_per_ms } => {
                if speed <= MOMENTUM_STOP_VELOCITY {
                    return Duration::ZERO;
                }
                let milliseconds =
                    (MOMENTUM_STOP_VELOCITY / speed).ln() / decay_per_ms.ln();
                Duration::from_secs_f32((milliseconds / 1000.0).max(0.0))
            }
            Self::FrictionSpline {
                friction,
                pixels_per_inch,
            } => {
                if speed <= 0.0 {
                    return Duration::ZERO;
                }
                let deceleration =
                    friction_spline::deceleration(speed, friction, pixels_per_inch);
                let seconds =
                    (deceleration / (friction_spline::deceleration_rate() - 1.0)).exp();
                Duration::from_secs_f64(seconds.max(0.0))
            }
        }
    }

    fn fling_distance(self, speed: f32, elapsed: Duration) -> f32 {
        let duration = self.fling_duration(speed);
        if duration.is_zero() {
            return 0.0;
        }
        let elapsed = elapsed.min(duration);
        match self {
            Self::Exponential { decay_per_ms } => {
                let milliseconds = elapsed.as_secs_f32() * 1000.0;
                (speed / 1000.0)
                    * (decay_per_ms.powf(milliseconds) - 1.0)
                    / decay_per_ms.ln()
            }
            Self::FrictionSpline {
                friction,
                pixels_per_inch,
            } => {
                let deceleration =
                    friction_spline::deceleration(speed, friction, pixels_per_inch);
                let rate = friction_spline::deceleration_rate();
                let total_distance = friction as f64
                    * friction_spline::physical_coefficient(pixels_per_inch)
                    * (rate / (rate - 1.0) * deceleration).exp();
                let progress = elapsed.as_secs_f64() / duration.as_secs_f64();
                total_distance as f32
                    * friction_spline::distance_coefficient(progress as f32)
            }
        }
    }
}

mod friction_spline {
    use super::LazyLock;

    const SAMPLE_COUNT: usize = 100;
    const INFLEXION: f32 = 0.35;
    const START_TENSION: f32 = 0.5;
    const END_TENSION: f32 = 1.0;
    const P1: f32 = START_TENSION * INFLEXION;
    const P2: f32 = 1.0 - END_TENSION * (1.0 - INFLEXION);

    pub(super) fn deceleration_rate() -> f64 {
        0.78f64.ln() / 0.9f64.ln()
    }

    static SPLINE_POSITION: LazyLock<[f32; SAMPLE_COUNT + 1]> = LazyLock::new(|| {
        let mut values = [0.0; SAMPLE_COUNT + 1];
        let mut x_min = 0.0;
        for (index, output) in values.iter_mut().take(SAMPLE_COUNT).enumerate() {
            let alpha = index as f32 / SAMPLE_COUNT as f32;
            let mut x_max = 1.0;
            let (x, coefficient) = loop {
                let x = x_min + (x_max - x_min) / 2.0;
                let coefficient = 3.0 * x * (1.0 - x);
                let time = coefficient * ((1.0 - x) * P1 + x * P2) + x * x * x;
                if (time - alpha).abs() < 1e-5 {
                    break (x, coefficient);
                }
                if time > alpha {
                    x_max = x;
                } else {
                    x_min = x;
                }
            };
            *output =
                coefficient * ((1.0 - x) * START_TENSION + x) + x * x * x;
        }
        values[SAMPLE_COUNT] = 1.0;
        values
    });

    pub(super) fn physical_coefficient(pixels_per_inch: f32) -> f64 {
        9.80665 * 39.37 * pixels_per_inch as f64 * 0.84
    }

    pub(super) fn deceleration(speed: f32, friction: f32, pixels_per_inch: f32) -> f64 {
        (INFLEXION as f64 * speed as f64
            / (friction as f64 * physical_coefficient(pixels_per_inch)))
        .ln()
    }

    pub(super) fn distance_coefficient(time: f32) -> f32 {
        if time >= 1.0 {
            return 1.0;
        }
        let index =
            ((SAMPLE_COUNT as f32 * time) as usize).min(SAMPLE_COUNT - 1);
        let time_lower = index as f32 / SAMPLE_COUNT as f32;
        let time_upper = (index + 1) as f32 / SAMPLE_COUNT as f32;
        let distance_lower = SPLINE_POSITION[index];
        let distance_upper = SPLINE_POSITION[index + 1];
        distance_lower
            + (time - time_lower)
                * ((distance_upper - distance_lower) / (time_upper - time_lower))
    }
}

pub(crate) struct TouchGestureRecognizer {
    tuning: GestureTuning,
    state: TouchGestureState,
    momentum: Option<Momentum>,
    last_tap: Option<CompletedTap>,
}

#[derive(Debug)]
pub(crate) enum RecognizedTouchGesture {
    Scroll(ScrollWheelEvent),
    Tap {
        down: MouseDownEvent,
        up: MouseUpEvent,
    },
    TouchDrag(TouchDragEvent),
    LongPress(LongPressEvent),
}

enum TouchGestureState {
    Idle,
    Pending {
        touch: ActiveTouch,
        deadline: Instant,
        long_press_offered: bool,
        touch_drag_offered: bool,
    },
    Panning {
        touch: ActiveTouch,
        axis: Axis,
    },
    LongPressing(ActiveTouch),
    TouchDragging(ActiveTouch),
}

struct ActiveTouch {
    id: TouchId,
    start_position: Point<Pixels>,
    raw_position: Point<Pixels>,
    emitted_position: Point<Pixels>,
    /// Last non-zero raw movement, retained across stationary samples so prediction corrections
    /// cannot reverse a pan while the finger is still advancing.
    last_movement: Point<Pixels>,
    velocity_tracker: VelocityTracker,
}

struct CompletedTap {
    position: Point<Pixels>,
    time: Instant,
    count: usize,
}

struct Momentum {
    position: Point<Pixels>,
    direction: Point<f32>,
    speed: f32,
    axis: Axis,
    started_at: Instant,
    duration: Duration,
    emitted_distance: f32,
    physics: ScrollPhysics,
}

impl TouchGestureRecognizer {
    pub(crate) fn new(tuning: GestureTuning) -> Self {
        Self {
            tuning,
            state: TouchGestureState::Idle,
            momentum: None,
            last_tap: None,
        }
    }

    pub(crate) fn handle_event(
        &mut self,
        event: &TouchEvent,
    ) -> SmallVec<[RecognizedTouchGesture; 2]> {
        self.handle_event_at(event, Instant::now())
    }

    fn handle_event_at(
        &mut self,
        event: &TouchEvent,
        now: Instant,
    ) -> SmallVec<[RecognizedTouchGesture; 2]> {
        let mut recognized = SmallVec::new();
        match event.phase {
            TouchPhase::Started => {
                let caught_fling = if let Some(momentum) = self.momentum.take() {
                    recognized.push(RecognizedTouchGesture::Scroll(scroll_event(
                        momentum.position,
                        Point::default(),
                        TouchPhase::Ended,
                    )));
                    Some(momentum.axis)
                } else {
                    None
                };
                if matches!(self.state, TouchGestureState::Idle) {
                    let mut velocity_tracker = VelocityTracker::default();
                    velocity_tracker.push(now, event.position);
                    let touch = ActiveTouch {
                        id: event.id,
                        start_position: event.position,
                        raw_position: event.position,
                        emitted_position: event.position,
                        last_movement: Point::default(),
                        velocity_tracker,
                    };
                    if let Some(axis) = caught_fling {
                        // Catching an active fling must be draggable immediately. Waiting for
                        // touch-slop would freeze the content and then jump once slop is exceeded.
                        recognized.push(RecognizedTouchGesture::Scroll(scroll_event(
                            touch.start_position,
                            Point::default(),
                            TouchPhase::Started,
                        )));
                        self.state = TouchGestureState::Panning { touch, axis };
                    } else {
                        self.state = TouchGestureState::Pending {
                            touch,
                            deadline: now + self.tuning.long_press_duration,
                            long_press_offered: false,
                            touch_drag_offered: false,
                        };
                    }
                }
            }
            TouchPhase::Moved => {
                match mem::replace(&mut self.state, TouchGestureState::Idle) {
                    TouchGestureState::Pending {
                        mut touch,
                        deadline,
                        long_press_offered,
                        touch_drag_offered,
                    } if touch.id == event.id => {
                        touch.velocity_tracker.push(now, event.position);
                        touch.raw_position = event.position;
                        let accumulated = event.position - touch.start_position;
                        if accumulated.magnitude() > f64::from(self.tuning.touch_slop) {
                            let axis = dominant_axis(accumulated);
                            let mut target =
                                event.predicted_position.unwrap_or(event.position);
                            let mut delta = target - touch.start_position;
                            lock_delta_to_axis(&mut delta, axis);
                            touch.last_movement = accumulated;
                            lock_delta_to_axis(&mut touch.last_movement, axis);
                            if movements_oppose(delta, touch.last_movement) {
                                target = event.position;
                                delta = accumulated;
                                lock_delta_to_axis(&mut delta, axis);
                            }
                            touch.emitted_position = target;
                            recognized.push(RecognizedTouchGesture::Scroll(scroll_event(
                                touch.start_position,
                                delta,
                                TouchPhase::Started,
                            )));
                            self.state =
                                TouchGestureState::Panning { touch, axis };
                        } else {
                            self.state = TouchGestureState::Pending {
                                touch,
                                deadline,
                                long_press_offered,
                                touch_drag_offered,
                            };
                        }
                    }
                    TouchGestureState::Panning {
                        mut touch,
                        axis,
                    } if touch.id == event.id => {
                        let mut raw_delta = event.position - touch.raw_position;
                        lock_delta_to_axis(&mut raw_delta, axis);
                        if raw_delta != Point::default() {
                            touch.last_movement = raw_delta;
                        }
                        touch.velocity_tracker.push(now, event.position);
                        touch.raw_position = event.position;
                        let mut target =
                            event.predicted_position.unwrap_or(event.position);
                        let mut delta = target - touch.emitted_position;
                        lock_delta_to_axis(&mut delta, axis);

                        // Prediction error must not move content backwards while raw input still
                        // advances. Fall back to raw coordinates; if even that is still behind the
                        // already-emitted prediction, hold position until the finger catches up.
                        if movements_oppose(delta, touch.last_movement) {
                            target = event.position;
                            delta = target - touch.emitted_position;
                            lock_delta_to_axis(&mut delta, axis);
                            if movements_oppose(delta, touch.last_movement) {
                                target = touch.emitted_position;
                                delta = Point::default();
                            }
                        }

                        touch.emitted_position = target;
                        recognized.push(RecognizedTouchGesture::Scroll(scroll_event(
                            touch.start_position,
                            delta,
                            TouchPhase::Moved,
                        )));
                        self.state =
                            TouchGestureState::Panning { touch, axis };
                    }
                    TouchGestureState::LongPressing(mut touch)
                        if touch.id == event.id =>
                    {
                        touch.raw_position = event.position;
                        recognized.push(RecognizedTouchGesture::LongPress(
                            LongPressEvent {
                                phase: TouchPhase::Moved,
                                start_position: touch.start_position,
                                position: event.position,
                            },
                        ));
                        self.state = TouchGestureState::LongPressing(touch);
                    }
                    TouchGestureState::TouchDragging(mut touch)
                        if touch.id == event.id =>
                    {
                        touch.raw_position = event.position;
                        recognized.push(RecognizedTouchGesture::TouchDrag(
                            TouchDragEvent {
                                phase: TouchPhase::Moved,
                                start_position: touch.start_position,
                                position: event.position,
                            },
                        ));
                        self.state = TouchGestureState::TouchDragging(touch);
                    }
                    other => self.state = other,
                }
            }
            TouchPhase::Ended => {
                match mem::replace(&mut self.state, TouchGestureState::Idle) {
                    TouchGestureState::Pending { touch, .. }
                        if touch.id == event.id =>
                    {
                        let count = match &self.last_tap {
                            Some(tap)
                                if now.duration_since(tap.time)
                                    <= self.tuning.multi_tap_interval
                                    && (event.position - tap.position).magnitude()
                                        <= f64::from(self.tuning.multi_tap_slop) =>
                            {
                                tap.count + 1
                            }
                            _ => 1,
                        };
                        self.last_tap = Some(CompletedTap {
                            position: event.position,
                            time: now,
                            count,
                        });
                        recognized.push(RecognizedTouchGesture::Tap {
                            down: MouseDownEvent {
                                button: MouseButton::Left,
                                position: event.position,
                                modifiers: Modifiers::default(),
                                click_count: count,
                                first_mouse: false,
                            },
                            up: MouseUpEvent {
                                button: MouseButton::Left,
                                position: event.position,
                                modifiers: Modifiers::default(),
                                click_count: count,
                            },
                        });
                    }
                    TouchGestureState::Panning { touch, axis }
                        if touch.id == event.id =>
                    {
                        let stopped = touch
                            .velocity_tracker
                            .latest_sample_time()
                            .map(|latest| {
                                now.duration_since(latest)
                                    > VELOCITY_ASSUME_STOPPED_GAP
                            })
                            .unwrap_or(true);
                        let mut velocity = if stopped {
                            Point::default()
                        } else {
                            touch.velocity_tracker.velocity()
                        };
                        match axis {
                            Axis::Vertical => velocity.x = 0.0,
                            Axis::Horizontal => velocity.y = 0.0,
                        }
                        let mut speed =
                            (velocity.x * velocity.x + velocity.y * velocity.y)
                                .sqrt();
                        if speed > MAX_FLING_VELOCITY {
                            let scale = MAX_FLING_VELOCITY / speed;
                            velocity.x *= scale;
                            velocity.y *= scale;
                            speed = MAX_FLING_VELOCITY;
                        }

                        let mut release_delta =
                            event.position - touch.emitted_position;
                        lock_delta_to_axis(&mut release_delta, axis);

                        if speed >= self.tuning.min_fling_velocity {
                            let direction = point(
                                velocity.x / speed,
                                velocity.y / speed,
                            );
                            let duration =
                                self.tuning.scroll_physics.fling_duration(speed);
                            if !duration.is_zero() {
                                let total_distance = self
                                    .tuning
                                    .scroll_physics
                                    .fling_distance(speed, duration);
                                // If prediction left the emitted content ahead of the raw finger,
                                // do not visibly snap backwards on release. Start the momentum curve
                                // already advanced by that overshoot instead.
                                let overshoot = -(f32::from(release_delta.x) * direction.x
                                    + f32::from(release_delta.y) * direction.y);
                                let emitted_distance =
                                    if overshoot > 0.0 && overshoot < total_distance {
                                        release_delta += point(
                                            px(direction.x * overshoot),
                                            px(direction.y * overshoot),
                                        );
                                        overshoot
                                    } else {
                                        0.0
                                    };
                                self.momentum = Some(Momentum {
                                    position: touch.start_position,
                                    direction,
                                    speed,
                                    axis,
                                    started_at: now,
                                    duration,
                                    emitted_distance,
                                    physics: self.tuning.scroll_physics,
                                });
                            }
                        }

                        recognized.push(RecognizedTouchGesture::Scroll(scroll_event(
                            touch.start_position,
                            release_delta,
                            TouchPhase::Ended,
                        )));
                    }
                    TouchGestureState::LongPressing(touch)
                        if touch.id == event.id =>
                    {
                        recognized.push(RecognizedTouchGesture::LongPress(
                            LongPressEvent {
                                phase: TouchPhase::Ended,
                                start_position: touch.start_position,
                                position: event.position,
                            },
                        ));
                    }
                    TouchGestureState::TouchDragging(touch)
                        if touch.id == event.id =>
                    {
                        recognized.push(RecognizedTouchGesture::TouchDrag(
                            TouchDragEvent {
                                phase: TouchPhase::Ended,
                                start_position: touch.start_position,
                                position: event.position,
                            },
                        ));
                    }
                    other => self.state = other,
                }
            }
            TouchPhase::Cancelled => {
                match mem::replace(&mut self.state, TouchGestureState::Idle) {
                    TouchGestureState::Panning { touch, .. }
                        if touch.id == event.id =>
                    {
                        recognized.push(RecognizedTouchGesture::Scroll(scroll_event(
                            touch.start_position,
                            Point::default(),
                            TouchPhase::Cancelled,
                        )));
                    }
                    TouchGestureState::LongPressing(touch)
                        if touch.id == event.id =>
                    {
                        recognized.push(RecognizedTouchGesture::LongPress(
                            LongPressEvent {
                                phase: TouchPhase::Cancelled,
                                start_position: touch.start_position,
                                position: event.position,
                            },
                        ));
                    }
                    TouchGestureState::TouchDragging(touch)
                        if touch.id == event.id =>
                    {
                        recognized.push(RecognizedTouchGesture::TouchDrag(
                            TouchDragEvent {
                                phase: TouchPhase::Cancelled,
                                start_position: touch.start_position,
                                position: event.position,
                            },
                        ));
                    }
                    TouchGestureState::Pending { touch, .. }
                        if touch.id == event.id => {}
                    other => self.state = other,
                }
            }
        }
        recognized
    }

    pub(crate) fn pending_long_press(&self) -> Option<(TouchId, Duration)> {
        let TouchGestureState::Pending {
            touch,
            deadline,
            long_press_offered: false,
            ..
        } = &self.state
        else {
            return None;
        };
        Some((
            touch.id,
            deadline.saturating_duration_since(Instant::now()),
        ))
    }

    pub(crate) fn offer_long_press(
        &mut self,
        id: TouchId,
    ) -> Option<RecognizedTouchGesture> {
        let TouchGestureState::Pending {
            touch,
            long_press_offered,
            ..
        } = &mut self.state
        else {
            return None;
        };
        if touch.id != id || *long_press_offered {
            return None;
        }
        *long_press_offered = true;
        Some(RecognizedTouchGesture::LongPress(LongPressEvent {
            phase: TouchPhase::Started,
            start_position: touch.start_position,
            position: touch.raw_position,
        }))
    }

    pub(crate) fn resolve_long_press(&mut self, claimed: bool) {
        if !claimed {
            return;
        }
        let state = mem::replace(&mut self.state, TouchGestureState::Idle);
        self.state = match state {
            TouchGestureState::Pending {
                touch,
                long_press_offered: true,
                ..
            } => TouchGestureState::LongPressing(touch),
            other => other,
        };
    }

    pub(crate) fn offer_touch_drag(
        &mut self,
        id: TouchId,
    ) -> Option<RecognizedTouchGesture> {
        let TouchGestureState::Pending {
            touch,
            touch_drag_offered,
            ..
        } = &mut self.state
        else {
            return None;
        };
        if touch.id != id || *touch_drag_offered {
            return None;
        }
        *touch_drag_offered = true;
        Some(RecognizedTouchGesture::TouchDrag(TouchDragEvent {
            phase: TouchPhase::Started,
            start_position: touch.start_position,
            position: touch.raw_position,
        }))
    }

    pub(crate) fn resolve_touch_drag(&mut self, claimed: bool) {
        if !claimed {
            return;
        }
        let state = mem::replace(&mut self.state, TouchGestureState::Idle);
        self.state = match state {
            TouchGestureState::Pending {
                touch,
                touch_drag_offered: true,
                ..
            } => TouchGestureState::TouchDragging(touch),
            other => other,
        };
    }

    pub(crate) fn has_momentum(&self) -> bool {
        self.momentum.is_some()
    }

    pub(crate) fn tick_momentum(&mut self) -> Option<RecognizedTouchGesture> {
        let momentum = self.momentum.as_mut()?;
        let elapsed = Instant::now()
            .duration_since(momentum.started_at)
            .min(momentum.duration);
        let distance =
            momentum.physics.fling_distance(momentum.speed, elapsed);
        let step = (distance - momentum.emitted_distance).max(0.0);
        momentum.emitted_distance = momentum.emitted_distance.max(distance);

        let mut delta = point(
            px(momentum.direction.x * step),
            px(momentum.direction.y * step),
        );
        lock_delta_to_axis(&mut delta, momentum.axis);

        let phase = if elapsed >= momentum.duration {
            TouchPhase::Ended
        } else {
            TouchPhase::Moved
        };
        let position = momentum.position;
        if phase == TouchPhase::Ended {
            self.momentum = None;
        }
        Some(RecognizedTouchGesture::Scroll(scroll_event(
            position, delta, phase,
        )))
    }
}

fn scroll_event(
    position: Point<Pixels>,
    delta: Point<Pixels>,
    touch_phase: TouchPhase,
) -> ScrollWheelEvent {
    ScrollWheelEvent {
        position,
        delta: ScrollDelta::Pixels(delta),
        modifiers: Modifiers::default(),
        touch_phase,
    }
}

#[derive(Default)]
struct VelocityTracker {
    samples: VecDeque<(Instant, Point<Pixels>)>,
}

impl VelocityTracker {
    fn push(&mut self, time: Instant, position: Point<Pixels>) {
        self.samples.push_back((time, position));
        while self.samples.len() > VELOCITY_MAX_SAMPLES {
            self.samples.pop_front();
        }
    }

    fn latest_sample_time(&self) -> Option<Instant> {
        self.samples.back().map(|(time, _)| *time)
    }

    fn velocity(&self) -> Point<f32> {
        let Some((newest_time, _)) = self.samples.back() else {
            return Point::default();
        };

        let mut times: SmallVec<[f64; VELOCITY_MAX_SAMPLES]> =
            SmallVec::new();
        let mut horizontal: SmallVec<[f64; VELOCITY_MAX_SAMPLES]> =
            SmallVec::new();
        let mut vertical: SmallVec<[f64; VELOCITY_MAX_SAMPLES]> =
            SmallVec::new();
        let mut previous_time = *newest_time;

        for (time, position) in self.samples.iter().rev() {
            let age = newest_time.duration_since(*time);
            if age > VELOCITY_WINDOW
                || previous_time.duration_since(*time)
                    > VELOCITY_ASSUME_STOPPED_GAP
            {
                break;
            }
            previous_time = *time;
            times.push(-age.as_secs_f64());
            horizontal.push(f64::from(f32::from(position.x)));
            vertical.push(f64::from(f32::from(position.y)));
        }

        let endpoint_estimate = |values: &[f64]| -> f32 {
            let elapsed = -times.last().copied().unwrap_or(0.0);
            if elapsed <= f64::EPSILON {
                return 0.0;
            }
            ((values.first().copied().unwrap_or(0.0)
                - values.last().copied().unwrap_or(0.0))
                / elapsed) as f32
        };

        if times.len() < 3 {
            return point(
                endpoint_estimate(&horizontal),
                endpoint_estimate(&vertical),
            );
        }

        point(
            quadratic_velocity_at_newest(&times, &horizontal)
                .map_or_else(|| endpoint_estimate(&horizontal), |v| v as f32),
            quadratic_velocity_at_newest(&times, &vertical)
                .map_or_else(|| endpoint_estimate(&vertical), |v| v as f32),
        )
    }
}

fn quadratic_velocity_at_newest(
    times: &[f64],
    values: &[f64],
) -> Option<f64> {
    let count = times.len() as f64;
    let (mut sum_t1, mut sum_t2, mut sum_t3, mut sum_t4) =
        (0.0, 0.0, 0.0, 0.0);
    let (mut sum_v, mut sum_vt, mut sum_vt2) =
        (0.0, 0.0, 0.0);

    for (&time, &value) in times.iter().zip(values) {
        let time_squared = time * time;
        sum_t1 += time;
        sum_t2 += time_squared;
        sum_t3 += time_squared * time;
        sum_t4 += time_squared * time_squared;
        sum_v += value;
        sum_vt += value * time;
        sum_vt2 += value * time_squared;
    }

    let determinant =
        count * (sum_t2 * sum_t4 - sum_t3 * sum_t3)
            - sum_t1 * (sum_t1 * sum_t4 - sum_t3 * sum_t2)
            + sum_t2 * (sum_t1 * sum_t3 - sum_t2 * sum_t2);
    if determinant.abs() < 1e-12 {
        return None;
    }

    let linear_determinant =
        count * (sum_vt * sum_t4 - sum_t3 * sum_vt2)
            - sum_v * (sum_t1 * sum_t4 - sum_t3 * sum_t2)
            + sum_t2 * (sum_t1 * sum_vt2 - sum_vt * sum_t2);
    Some(linear_determinant / determinant)
}


#[cfg(test)]
mod tests {
    use super::*;

    fn touch_event(
        id: TouchId,
        phase: TouchPhase,
        y: f32,
        predicted_y: Option<f32>,
    ) -> TouchEvent {
        TouchEvent {
            id,
            phase,
            position: point(px(100.0), px(y)),
            predicted_position: predicted_y.map(|predicted| point(px(100.0), px(predicted))),
            force: None,
        }
    }

    fn scroll_delta(gesture: &RecognizedTouchGesture) -> Point<Pixels> {
        let RecognizedTouchGesture::Scroll(scroll) = gesture else {
            panic!("expected scroll gesture");
        };
        scroll.delta.pixel_delta(px(16.0))
    }

    #[test]
    fn claimed_touch_drag_emits_phased_stream_without_pan_or_tap() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let now = Instant::now();
        let id = TouchId(11);

        recognizer.handle_event_at(
            &touch_event(id, TouchPhase::Started, 20.0, None),
            now,
        );
        let Some(RecognizedTouchGesture::TouchDrag(started)) =
            recognizer.offer_touch_drag(id)
        else {
            panic!("expected touch drag offer");
        };
        assert_eq!(started.phase, TouchPhase::Started);
        assert_eq!(started.position.y, px(20.0));
        recognizer.resolve_touch_drag(true);

        let moved = recognizer.handle_event_at(
            &touch_event(id, TouchPhase::Moved, 40.0, None),
            now + Duration::from_millis(10),
        );
        let [RecognizedTouchGesture::TouchDrag(moved)] = moved.as_slice() else {
            panic!("expected moved touch drag");
        };
        assert_eq!(moved.phase, TouchPhase::Moved);
        assert_eq!(moved.position.y, px(40.0));

        let ended = recognizer.handle_event_at(
            &touch_event(id, TouchPhase::Ended, 45.0, None),
            now + Duration::from_millis(20),
        );
        let [RecognizedTouchGesture::TouchDrag(ended)] = ended.as_slice() else {
            panic!("expected ended touch drag");
        };
        assert_eq!(ended.phase, TouchPhase::Ended);
        assert_eq!(ended.position.y, px(45.0));
        assert!(!recognizer.has_momentum());
    }

    #[test]
    fn unclaimed_touch_drag_stays_eligible_for_pan() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let now = Instant::now();
        let id = TouchId(12);

        recognizer.handle_event_at(
            &touch_event(id, TouchPhase::Started, 0.0, None),
            now,
        );
        assert!(recognizer.offer_touch_drag(id).is_some());
        recognizer.resolve_touch_drag(false);

        let moved = recognizer.handle_event_at(
            &touch_event(id, TouchPhase::Moved, 20.0, None),
            now + Duration::from_millis(10),
        );
        assert!(matches!(
            moved.as_slice(),
            [RecognizedTouchGesture::Scroll(ScrollWheelEvent {
                touch_phase: TouchPhase::Started,
                ..
            })]
        ));
    }

    #[test]
    fn predicted_positions_do_not_emit_false_reversals() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let now = Instant::now();
        let id = TouchId(1);

        recognizer.handle_event_at(
            &touch_event(id, TouchPhase::Started, 100.0, None),
            now,
        );

        let recognized = recognizer.handle_event_at(
            &touch_event(id, TouchPhase::Moved, 120.0, Some(130.0)),
            now + Duration::from_millis(16),
        );
        assert_eq!(scroll_delta(&recognized[0]), point(px(0.0), px(30.0)));

        let recognized = recognizer.handle_event_at(
            &touch_event(id, TouchPhase::Moved, 125.0, Some(127.0)),
            now + Duration::from_millis(32),
        );
        assert_eq!(scroll_delta(&recognized[0]), Point::default());

        let recognized = recognizer.handle_event_at(
            &touch_event(id, TouchPhase::Moved, 125.0, Some(126.0)),
            now + Duration::from_millis(40),
        );
        assert_eq!(scroll_delta(&recognized[0]), Point::default());

        let recognized = recognizer.handle_event_at(
            &touch_event(id, TouchPhase::Moved, 132.0, Some(136.0)),
            now + Duration::from_millis(48),
        );
        assert_eq!(scroll_delta(&recognized[0]), point(px(0.0), px(6.0)));

        let recognized = recognizer.handle_event_at(
            &touch_event(id, TouchPhase::Moved, 124.0, Some(140.0)),
            now + Duration::from_millis(64),
        );
        assert_eq!(scroll_delta(&recognized[0]), point(px(0.0), px(-12.0)));
    }

    #[test]
    fn prediction_overshoot_is_folded_into_fling_and_can_be_caught() {
        let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
        let now = Instant::now();
        let id = TouchId(7);

        recognizer.handle_event_at(
            &touch_event(id, TouchPhase::Started, 100.0, None),
            now,
        );
        recognizer.handle_event_at(
            &touch_event(id, TouchPhase::Moved, 120.0, Some(130.0)),
            now + Duration::from_millis(16),
        );
        recognizer.handle_event_at(
            &touch_event(id, TouchPhase::Moved, 130.0, Some(140.0)),
            now + Duration::from_millis(32),
        );

        let released = recognizer.handle_event_at(
            &touch_event(id, TouchPhase::Ended, 130.0, None),
            now + Duration::from_millis(33),
        );
        assert_eq!(released.len(), 1);
        assert_eq!(scroll_delta(&released[0]), Point::default());
        assert!(recognizer.has_momentum());

        let caught = recognizer.handle_event_at(
            &touch_event(TouchId(8), TouchPhase::Started, 130.0, None),
            now + Duration::from_millis(34),
        );
        assert_eq!(caught.len(), 2);
        let RecognizedTouchGesture::Scroll(end_fling) = &caught[0] else {
            panic!("expected fling end");
        };
        let RecognizedTouchGesture::Scroll(start_pan) = &caught[1] else {
            panic!("expected immediate pan start");
        };
        assert_eq!(end_fling.touch_phase, TouchPhase::Ended);
        assert_eq!(start_pan.touch_phase, TouchPhase::Started);
        assert!(!recognizer.has_momentum());
    }
}
