use super::{AppRoute, MainWindowRenderModel, MainWindowView};
use gpui::*;
use std::time::{Duration, Instant};

const SECRET: &[u8] = b"7355608";
const INPUT_TIMEOUT: Duration = Duration::from_secs(3);
const COUNTDOWN_DURATION: Duration = Duration::from_secs(10);
const EXPLOSION_DURATION: Duration = Duration::from_millis(1_400);
const SHAKE_DURATION: Duration = Duration::from_millis(900);

#[derive(Default)]
struct SecretMatcher {
    matched: usize,
    last_key_at: Option<Instant>,
}

impl SecretMatcher {
    fn reset(&mut self) {
        self.matched = 0;
        self.last_key_at = None;
    }

    fn push(&mut self, digit: u8, now: Instant) -> bool {
        if self
            .last_key_at
            .is_some_and(|last_key_at| now.saturating_duration_since(last_key_at) > INPUT_TIMEOUT)
        {
            self.reset();
        }

        let expected = SECRET.get(self.matched).copied();
        self.matched = if expected == Some(digit) {
            self.matched + 1
        } else if digit == SECRET[0] {
            1
        } else {
            0
        };
        self.last_key_at = (self.matched > 0).then_some(now);

        if self.matched == SECRET.len() {
            self.reset();
            true
        } else {
            false
        }
    }
}

enum EasterEggPhase {
    Idle,
    Countdown {
        started_at: Instant,
    },
    Exploding {
        started_at: Instant,
        original_origin: Option<Point<Pixels>>,
        native_positioning: bool,
        window_was_moved: bool,
    },
}

pub(super) struct EasterEggState {
    matcher: SecretMatcher,
    phase: EasterEggPhase,
    timeline_task: Option<Task<()>>,
}

impl Default for EasterEggState {
    fn default() -> Self {
        Self {
            matcher: SecretMatcher::default(),
            phase: EasterEggPhase::Idle,
            timeline_task: None,
        }
    }
}

impl EasterEggState {
    pub(super) fn is_active(&self) -> bool {
        !matches!(self.phase, EasterEggPhase::Idle)
    }

    fn is_counting_down(&self) -> bool {
        matches!(self.phase, EasterEggPhase::Countdown { .. })
    }
}

impl MainWindowView {
    pub(super) fn install_easter_egg_interceptor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity().downgrade();
        let window_id = window.window_handle().window_id();
        self._window_subscriptions
            .push(cx.intercept_keystrokes(move |event, window, cx| {
                if window.window_handle().window_id() != window_id {
                    return;
                }
                if let Err(error) = view.update(cx, |this, cx| {
                    this.handle_easter_egg_keystroke(event, window, cx);
                }) {
                    tracing::debug!(?error, "easter egg keystroke target was released");
                }
            }));
    }

    fn handle_easter_egg_keystroke(
        &mut self,
        event: &KeystrokeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.easter_egg.is_counting_down() && event.keystroke.key == "escape" {
            cx.stop_propagation();
            self.cancel_easter_egg(cx);
            return;
        }
        if self.easter_egg.is_active() {
            return;
        }
        if crate::ui::navigation::current_route(cx) != AppRoute::Settings
            || event.keystroke.modifiers.modified()
            || event
                .context_stack
                .iter()
                .any(|context| context.contains("Input") || context.contains("CodeEditor"))
        {
            self.easter_egg.matcher.reset();
            return;
        }

        let Some(digit) = easter_egg_digit(&event.keystroke) else {
            self.easter_egg.matcher.reset();
            return;
        };
        if self.easter_egg.matcher.push(digit, Instant::now()) {
            self.start_easter_egg(window, cx);
        }
    }

    fn start_easter_egg(&mut self, window: &Window, cx: &mut Context<Self>) {
        let started_at = Instant::now();
        self.easter_egg.timeline_task.take();
        self.easter_egg.phase = EasterEggPhase::Countdown { started_at };

        let window_handle = window.window_handle();
        self.easter_egg.timeline_task = Some(cx.spawn(async move |view, cx| {
            Timer::after(COUNTDOWN_DURATION).await;
            if cx
                .update_window(window_handle, |_, window, cx| {
                    view.update(cx, |this, cx| this.begin_easter_egg_explosion(window, cx))
                })
                .is_err()
            {
                return;
            }
            Timer::after(EXPLOSION_DURATION).await;
            if let Err(error) = cx.update_window(window_handle, |_, window, cx| {
                view.update(cx, |this, cx| this.finish_easter_egg(window, cx))
            }) {
                tracing::debug!(?error, "easter egg window closed before completion");
            }
        }));
        cx.notify();
    }

    fn begin_easter_egg_explosion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.easter_egg.is_counting_down() {
            return;
        }
        let original_origin = matches!(window.window_bounds(), WindowBounds::Windowed(_))
            .then(|| window.bounds().origin);
        let native_positioning =
            original_origin.is_some_and(|origin| window.set_window_origin(origin));
        self.easter_egg.phase = EasterEggPhase::Exploding {
            started_at: Instant::now(),
            original_origin,
            native_positioning,
            window_was_moved: native_positioning,
        };
        cx.notify();
    }

    fn cancel_easter_egg(&mut self, cx: &mut Context<Self>) {
        if !self.easter_egg.is_counting_down() {
            return;
        }
        self.easter_egg.timeline_task.take();
        self.easter_egg.phase = EasterEggPhase::Idle;
        cx.notify();
    }

    fn finish_easter_egg(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let EasterEggPhase::Exploding {
            original_origin: Some(origin),
            window_was_moved: true,
            ..
        } = self.easter_egg.phase
        {
            if !window.set_window_origin(origin) {
                tracing::warn!("easter egg: failed to restore native window origin");
            }
        }
        self.easter_egg.phase = EasterEggPhase::Idle;
        cx.notify();
    }

    pub(super) fn compose_easter_egg(
        &mut self,
        mut root: Div,
        model: &MainWindowRenderModel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        if matches!(model.builtin_route, route if route != AppRoute::Settings)
            && !self.easter_egg.is_active()
        {
            self.easter_egg.matcher.reset();
        }

        let now = model.now;
        let (overlay, content_offset) = match self.easter_egg.phase {
            EasterEggPhase::Idle => return root,
            EasterEggPhase::Countdown { started_at } => {
                let elapsed = now.saturating_duration_since(started_at);
                let next_second = elapsed.as_secs().saturating_add(1).min(10);
                window.request_invalidation_at(started_at + Duration::from_secs(next_second), cx);
                (render_countdown_overlay(elapsed), Point::default())
            }
            EasterEggPhase::Exploding {
                started_at,
                original_origin,
                native_positioning,
                ..
            } => {
                crate::ui::animation::request_layout_animation_frame_if(window, true);
                let elapsed = now.saturating_duration_since(started_at);
                let offset = shake_offset(elapsed);
                if native_positioning {
                    if let Some(origin) = original_origin {
                        if window.set_window_origin(origin + offset) {
                            (render_explosion_overlay(elapsed), Point::default())
                        } else {
                            if let EasterEggPhase::Exploding {
                                native_positioning, ..
                            } = &mut self.easter_egg.phase
                            {
                                *native_positioning = false;
                            }
                            tracing::warn!(
                                "easter egg: native window movement failed during explosion"
                            );
                            (render_explosion_overlay(elapsed), offset)
                        }
                    } else {
                        (render_explosion_overlay(elapsed), offset)
                    }
                } else {
                    (render_explosion_overlay(elapsed), offset)
                }
            }
        };

        if content_offset != Point::default() {
            root = root.left(content_offset.x).top(content_offset.y);
        }
        root.child(overlay)
    }
}

fn easter_egg_digit(keystroke: &Keystroke) -> Option<u8> {
    let key = keystroke.key.as_bytes();
    match key {
        [digit @ b'0'..=b'9'] => Some(*digit),
        [b'n', b'u', b'm', digit @ b'0'..=b'9'] => Some(*digit),
        _ => None,
    }
}

fn shake_offset(elapsed: Duration) -> Point<Pixels> {
    let progress = (elapsed.as_secs_f32() / SHAKE_DURATION.as_secs_f32()).clamp(0.0, 1.0);
    let amplitude = 18.0 * (1.0 - progress).powi(2);
    let time = elapsed.as_secs_f32();
    point(
        px((time * 93.0).sin() * amplitude),
        px((time * 127.0 + 1.2).sin() * amplitude * 0.76),
    )
}

fn blocking_overlay() -> Div {
    div()
        .absolute()
        .inset_0()
        .size_full()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
        .on_mouse_move(|_, _, cx| cx.stop_propagation())
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
}

fn render_countdown_overlay(elapsed: Duration) -> Div {
    let remaining = 10_u64.saturating_sub(elapsed.as_secs()).max(1);
    blocking_overlay()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(0x07100ee6))
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_3()
                .px_10()
                .py_8()
                .rounded_lg()
                .border_2()
                .border_color(rgb(0xe14b37))
                .bg(rgba(0x111814f2))
                .text_color(rgb(0xff6a4f))
                .child(div().text_xs().child("BMCBL // DEVICE ARMED"))
                .child(div().text_size(px(72.)).child(format!("{remaining:02}")))
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(0xd7c7a8))
                        .child("ESC  DISARM"),
                ),
        )
}

fn render_explosion_overlay(elapsed: Duration) -> Div {
    let progress = (elapsed.as_secs_f32() / EXPLOSION_DURATION.as_secs_f32()).clamp(0.0, 1.0);
    blocking_overlay().bg(rgba(0x080605e8)).child(
        canvas(
            move |_, _, _| {},
            move |bounds, _, window, _| paint_explosion(bounds, progress, window),
        )
        .size_full(),
    )
}

#[allow(clippy::cast_precision_loss)]
fn paint_explosion(bounds: Bounds<Pixels>, progress: f32, window: &mut Window) {
    let center = bounds.center();
    let flash_alpha = (1.0 - progress * 4.5).clamp(0.0, 0.92);
    if flash_alpha > 0.0 {
        window.paint_quad(fill(bounds, white().alpha(flash_alpha)));
    }

    let largest_extent = (bounds.size.width / px(1.0)).max(bounds.size.height / px(1.0));
    let radius = px(24.0 + progress.sqrt() * largest_extent * 0.72);
    let thickness = px((8.0 * (1.0 - progress)).max(1.0));
    let ring_color = rgb(0xffb13b).alpha((1.0 - progress).powi(2));
    let ring = Bounds::from_corners(
        center - point(radius, radius),
        center + point(radius, radius),
    );
    window.paint_quad(fill(
        Bounds::new(ring.origin, size(ring.size.width, thickness)),
        ring_color,
    ));
    window.paint_quad(fill(
        Bounds::new(
            ring.bottom_left() - point(px(0.), thickness),
            size(ring.size.width, thickness),
        ),
        ring_color,
    ));
    window.paint_quad(fill(
        Bounds::new(ring.origin, size(thickness, ring.size.height)),
        ring_color,
    ));
    window.paint_quad(fill(
        Bounds::new(
            ring.top_right() - point(thickness, px(0.)),
            size(thickness, ring.size.height),
        ),
        ring_color,
    ));

    for index in 0..48_u32 {
        let seed = index.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let angle = (seed % 6_283) as f32 / 1_000.0;
        let speed = 90.0 + ((seed >> 12) % 260) as f32;
        let distance = speed * progress * (1.0 - progress * 0.28);
        let gravity = 150.0 * progress * progress;
        let particle_size = px(2.0 + ((seed >> 24) % 5) as f32);
        let origin = center
            + point(
                px(angle.cos() * distance),
                px(angle.sin() * distance + gravity),
            );
        let color = if index % 3 == 0 {
            rgb(0x6f6861).alpha((1.0 - progress) * 0.7)
        } else {
            rgb(0xff7a22).alpha((1.0 - progress).powi(2))
        };
        window.paint_quad(fill(
            Bounds::new(origin, size(particle_size, particle_size)),
            color,
        ));
    }
}

#[cfg(test)]
pub(super) fn verify_easter_egg_logic() {
    let mut matcher = SecretMatcher::default();
    let start = Instant::now();
    assert!(!matcher.push(b'7', start));
    assert!(!matcher.push(b'9', start + Duration::from_millis(10)));
    for (index, digit) in SECRET.iter().enumerate() {
        let triggered = matcher.push(*digit, start + Duration::from_millis(20 + index as u64));
        assert_eq!(triggered, index + 1 == SECRET.len());
    }

    assert!(!matcher.push(b'7', start + Duration::from_secs(1)));
    assert!(!matcher.push(
        b'3',
        start + Duration::from_secs(1) + INPUT_TIMEOUT + Duration::from_millis(1)
    ));
    assert_eq!(matcher.matched, 0);

    for key in ["7", "num7"] {
        assert_eq!(
            easter_egg_digit(&Keystroke {
                key: key.to_string(),
                ..Keystroke::default()
            }),
            Some(b'7')
        );
    }
    assert_eq!(
        easter_egg_digit(&Keystroke {
            key: "f7".to_string(),
            ..Keystroke::default()
        }),
        None
    );

    let peak = shake_offset(Duration::from_millis(10));
    assert!(peak.x != px(0.) || peak.y != px(0.));
    assert_eq!(shake_offset(SHAKE_DURATION), Point::default());
    assert_eq!(shake_offset(EXPLOSION_DURATION), Point::default());
}