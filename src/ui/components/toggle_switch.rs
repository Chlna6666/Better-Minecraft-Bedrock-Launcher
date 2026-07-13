use crate::ui::animation::{ease_out_cubic, raw_progress, request_animation_frame_if};
use crate::ui::theme::colors::ThemeColors;
use gpui::*;
use std::rc::Rc;
use std::time::{Duration, Instant};

const TRACK_WIDTH: f32 = 44.0;
const TRACK_HEIGHT: f32 = 26.0;
const KNOB_SIZE: f32 = 22.0;
const KNOB_INSET_X: f32 = 2.0;
const KNOB_INSET_Y: f32 = 2.0;
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

    fn sync(&mut self, colors: ThemeColors, enabled: bool, on_toggle: Rc<dyn Fn(&mut App)>) {
        self.colors = colors;
        self.on_toggle = on_toggle;
        if self.enabled == enabled {
            return;
        }

        self.enabled = enabled;
        let at = Instant::now();
        self.phase = if enabled {
            TogglePhase::Opening { at }
        } else {
            TogglePhase::Closing { at }
        };
    }

    fn animation_progress(&mut self, now: Instant) -> (f32, bool) {
        let (started_at, opening) = match self.phase {
            TogglePhase::Stable => return (f32::from(self.enabled), false),
            TogglePhase::Opening { at } => (at, true),
            TogglePhase::Closing { at } => (at, false),
        };
        let raw = raw_progress(now, started_at, ANIMATION_DURATION);
        if raw >= 1.0 {
            self.phase = TogglePhase::Stable;
            return (f32::from(self.enabled), false);
        }

        let eased = ease_out_cubic(raw);
        (if opening { eased } else { 1.0 - eased }, true)
    }

    fn render_track(&self, progress: f32) -> Div {
        let track_color = lerp_hsla(
            Hsla {
                a: 1.0,
                ..self.colors.border
            },
            Hsla {
                a: 1.0,
                ..self.colors.accent
            },
            progress,
        );
        let on_toggle = self.on_toggle.clone();

        div()
            .w(px(TRACK_WIDTH))
            .h(px(TRACK_HEIGHT))
            .rounded(px(999.0))
            .bg(track_color)
            .relative()
            .cursor_pointer()
            .shadow(track_shadow())
            .child(
                div()
                    .absolute()
                    .top(px(KNOB_INSET_Y))
                    .left(px(KNOB_INSET_X + KNOB_TRAVEL * progress))
                    .w(px(KNOB_SIZE))
                    .h(px(KNOB_SIZE))
                    .rounded(px(999.0))
                    .bg(rgb(0xffffff))
                    .shadow(knob_shadow()),
            )
            .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                (on_toggle)(cx);
            })
    }
}

impl Render for ToggleSwitchView {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let (progress, animating) = self.animation_progress(Instant::now());
        request_animation_frame_if(window, animating);
        self.render_track(progress)
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
        view.update(cx, |view, _cx| {
            view.sync(self.colors, self.enabled, self.on_toggle);
        });
        AnyView::from(view)
    }
}

fn lerp(start: f32, end: f32, progress: f32) -> f32 {
    start + (end - start) * progress
}

fn lerp_hsla(start: Hsla, end: Hsla, progress: f32) -> Hsla {
    Hsla {
        h: lerp(start.h, end.h, progress),
        s: lerp(start.s, end.s, progress),
        l: lerp(start.l, end.l, progress),
        a: lerp(start.a, end.a, progress),
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
            a: 0.16,
            ..rgb(0x000000).into()
        },
        blur_radius: px(8.0),
        spread_radius: px(0.0),
        offset: point(px(0.0), px(2.0)),
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
    fn sync_starts_animation_only_when_value_changes() {
        let mut view = test_view(false);
        view.sync(LightColors::colors(), false, Rc::new(|_| {}));
        assert_eq!(view.phase, TogglePhase::Stable);

        view.sync(LightColors::colors(), true, Rc::new(|_| {}));
        assert!(matches!(view.phase, TogglePhase::Opening { .. }));
    }

    #[test]
    fn completed_animation_settles_at_target() {
        let mut view = test_view(true);
        let started_at = Instant::now();
        view.phase = TogglePhase::Opening { at: started_at };

        let (progress, animating) =
            view.animation_progress(started_at + ANIMATION_DURATION + Duration::from_millis(1));

        assert_eq!(progress, 1.0);
        assert!(!animating);
        assert_eq!(view.phase, TogglePhase::Stable);
    }
}
