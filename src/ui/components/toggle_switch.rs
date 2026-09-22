use crate::ui::animation::ease_out_cubic_motion;
use crate::ui::theme::colors::ThemeColors;
use gpui::AnimationExt as _;
use gpui::*;
use std::rc::Rc;
use std::time::{Duration, Instant};

const TRACK_WIDTH: f32 = 44.0;
const TRACK_HEIGHT: f32 = 26.0;
const KNOB_SIZE: f32 = 22.0;
const KNOB_INSET_X: f32 = 2.0;
const KNOB_INSET_Y: f32 = (TRACK_HEIGHT - KNOB_SIZE) * 0.5;
const KNOB_TRAVEL: f32 = TRACK_WIDTH - KNOB_SIZE - 2.0 * KNOB_INSET_X;
const ANIMATION_DURATION: Duration = Duration::from_millis(160);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TogglePhase {
    Stable,
    Opening { at: Instant },
    Closing { at: Instant },
}

struct ToggleSwitchView {
    colors: ThemeColors,
    enabled: bool,
    phase: TogglePhase,
    on_toggle: Rc<dyn Fn(&mut App)>,
}

impl ToggleSwitchView {
    fn new(colors: ThemeColors, enabled: bool, on_toggle: Rc<dyn Fn(&mut App)>) -> Self {
        Self {
            colors,
            enabled,
            phase: TogglePhase::Stable,
            on_toggle,
        }
    }

    fn sync(
        &mut self,
        colors: ThemeColors,
        enabled: bool,
        on_toggle: Rc<dyn Fn(&mut App)>,
        now: Instant,
    ) -> bool {
        self.colors = colors;
        self.on_toggle = on_toggle;
        if self.enabled == enabled {
            return false;
        }

        self.enabled = enabled;
        self.phase = if enabled {
            TogglePhase::Opening { at: now }
        } else {
            TogglePhase::Closing { at: now }
        };
        true
    }

    fn settle_completed_phase(&mut self, now: Instant) -> bool {
        let started_at = match self.phase {
            TogglePhase::Stable => return false,
            TogglePhase::Opening { at } | TogglePhase::Closing { at } => at,
        };
        if now.saturating_duration_since(started_at) < ANIMATION_DURATION {
            return false;
        }

        self.phase = TogglePhase::Stable;
        true
    }

    fn phase_deadline(&self) -> Option<Instant> {
        match self.phase {
            TogglePhase::Stable => None,
            TogglePhase::Opening { at } | TogglePhase::Closing { at } => {
                Some(at + ANIMATION_DURATION)
            }
        }
    }

    fn render_track(&self) -> Div {
        let off_color = Hsla {
            a: 1.0,
            ..self.colors.border
        };
        let accent = div()
            .absolute()
            .inset_0()
            .rounded(px(crate::ui::theme::tokens::radius::FULL))
            .bg(Hsla {
                a: 1.0,
                ..self.colors.accent
            });
        let accent = match self.phase {
            TogglePhase::Opening { .. } => accent
                .with_animation(
                    "toggle-switch-accent",
                    ease_out_cubic_motion(ANIMATION_DURATION)
                        .with_property(AnimationProperty::opacity(0.0, 1.0)),
                    |this, _progress| this,
                )
                .into_any_element(),
            TogglePhase::Closing { .. } => accent
                .with_animation(
                    "toggle-switch-accent",
                    ease_out_cubic_motion(ANIMATION_DURATION)
                        .with_property(AnimationProperty::opacity(1.0, 0.0)),
                    |this, _progress| this,
                )
                .into_any_element(),
            TogglePhase::Stable => accent
                .opacity(if self.enabled { 1.0 } else { 0.0 })
                .into_any_element(),
        };

        // Layout is already at the destination position. The renderer owns only the temporary
        // visual offset, so toggling no longer puts the settings tree on a 60/120 Hz layout path.
        let target_left = KNOB_INSET_X + if self.enabled { KNOB_TRAVEL } else { 0.0 };
        let knob = div()
            .absolute()
            .top(px(KNOB_INSET_Y))
            .left(px(target_left))
            .w(px(KNOB_SIZE))
            .h(px(KNOB_SIZE))
            .rounded(px(crate::ui::theme::tokens::radius::FULL))
            .bg(self.colors.btn_primary_text)
            .shadow(knob_shadow());
        let knob = match self.phase {
            TogglePhase::Opening { .. } => knob
                .with_animation(
                    "toggle-switch-knob",
                    ease_out_cubic_motion(ANIMATION_DURATION).with_property(
                        AnimationProperty::translation(
                            point(px(-KNOB_TRAVEL), px(0.0)),
                            Point::default(),
                        ),
                    ),
                    |this, _progress| this,
                )
                .into_any_element(),
            TogglePhase::Closing { .. } => knob
                .with_animation(
                    "toggle-switch-knob",
                    ease_out_cubic_motion(ANIMATION_DURATION).with_property(
                        AnimationProperty::translation(
                            point(px(KNOB_TRAVEL), px(0.0)),
                            Point::default(),
                        ),
                    ),
                    |this, _progress| this,
                )
                .into_any_element(),
            TogglePhase::Stable => knob.into_any_element(),
        };

        let on_toggle = self.on_toggle.clone();
        div()
            .relative()
            .w(px(TRACK_WIDTH))
            .h(px(TRACK_HEIGHT))
            .rounded(px(crate::ui::theme::tokens::radius::FULL))
            .bg(off_color)
            .cursor_pointer()
            .shadow(track_shadow())
            .child(accent)
            .child(knob)
            .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                (on_toggle)(cx);
            })
    }
}

impl Render for ToggleSwitchView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = window.animation_time();
        self.settle_completed_phase(now);

        // Renderer-owned animations invalidate their retained element at completion, but the
        // component also owns logical phase state. Arm one cheap deadline wake-up so Closing is
        // guaranteed to collapse to Stable even if the surrounding retained tree is otherwise
        // completely idle. This is one wake-up per transition, not a per-frame layout animation.
        if let Some(deadline) = self.phase_deadline() {
            window.request_invalidation_at(deadline + Duration::from_millis(1), cx);
        }

        self.render_track()
    }
}

#[derive(IntoElement)]
pub struct ToggleSwitch {
    id: ElementId,
    colors: ThemeColors,
    enabled: bool,
    on_toggle: Rc<dyn Fn(&mut App)>,
}

impl ToggleSwitch {
    pub fn new(
        id: impl Into<ElementId>,
        colors: &ThemeColors,
        enabled: bool,
        on_toggle: impl Fn(&mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            colors: *colors,
            enabled,
            on_toggle: Rc::new(on_toggle),
        }
    }
}

impl RenderOnce for ToggleSwitch {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let initial_on_toggle = self.on_toggle.clone();
        let view = window.use_keyed_state(self.id, cx, |_, _| {
            ToggleSwitchView::new(self.colors, self.enabled, initial_on_toggle)
        });
        let now = window.animation_time();
        view.update(cx, |view, cx| {
            // use_keyed_state keeps this view alive across parent renders. Entity::update does not
            // implicitly invalidate a view, so a real state transition must notify the retained
            // subtree and start a fresh logical phase at the same presentation timestamp.
            if view.sync(self.colors, self.enabled, self.on_toggle, now) {
                cx.notify();
            }
        });
        AnyView::from(view)
    }
}

fn track_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: Hsla {
            a: 0.10,
            ..rgb(0x000000).into()
        },
        blur_radius: px(8.0),
        spread_radius: px(-3.0),
        offset: point(px(0.0), px(2.0)),
    }]
}

fn knob_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: Hsla {
            a: 0.12,
            ..rgb(0x000000).into()
        },
        blur_radius: px(2.0),
        spread_radius: px(0.0),
        offset: point(px(0.0), px(0.0)),
    }]
}

#[cfg(test)]
mod tests {
    use super::{ANIMATION_DURATION, TogglePhase, ToggleSwitchView};
    use crate::ui::theme::colors::LightColors;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    fn test_view(enabled: bool) -> ToggleSwitchView {
        ToggleSwitchView::new(LightColors::colors(), enabled, Rc::new(|_| {}))
    }

    #[test]
    fn initial_state_does_not_animate() {
        let view = test_view(false);
        assert_eq!(view.phase, TogglePhase::Stable);
    }

    #[test]
    fn sync_records_only_real_state_transitions() {
        let now = Instant::now();
        let mut view = test_view(false);
        assert!(!view.sync(LightColors::colors(), false, Rc::new(|_| {}), now));
        assert_eq!(view.phase, TogglePhase::Stable);

        assert!(view.sync(LightColors::colors(), true, Rc::new(|_| {}), now));
        assert_eq!(view.phase, TogglePhase::Opening { at: now });

        let later = now + Duration::from_millis(20);
        assert!(view.sync(LightColors::colors(), false, Rc::new(|_| {}), later));
        assert_eq!(view.phase, TogglePhase::Closing { at: later });
    }

    #[test]
    fn completed_closing_phase_settles_to_static_off_state() {
        let started_at = Instant::now();
        let mut view = test_view(true);
        assert!(view.sync(
            LightColors::colors(),
            false,
            Rc::new(|_| {}),
            started_at,
        ));
        assert!(!view.settle_completed_phase(
            started_at + ANIMATION_DURATION - Duration::from_millis(1)
        ));
        assert!(matches!(view.phase, TogglePhase::Closing { .. }));

        assert!(view.settle_completed_phase(
            started_at + ANIMATION_DURATION + Duration::from_millis(1)
        ));
        assert_eq!(view.phase, TogglePhase::Stable);
        assert!(!view.enabled);
        assert_eq!(view.phase_deadline(), None);
    }
}
