use crate::ui::theme::colors::ThemeColors;
use gpui::*;
use std::rc::Rc;
use std::time::Instant;

const TRACK_W: f32 = 44.0;
const TRACK_H: f32 = 26.0;

const KNOB_SIZE: f32 = 22.0;

// 真正对称的几何参数
const KNOB_INSET_X: f32 = 2.0;
const KNOB_INSET_Y: f32 = 2.0;
const KNOB_TRAVEL: f32 = 18.0; // 44 - 22 - 2*2 = 18

const ANIM_DURATION: f32 = 0.16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TogglePhase {
    Stable,
    Opening { at: Instant },
    Closing { at: Instant },
}

#[derive(Clone, Copy, Debug)]
struct ToggleAnimState {
    phase: TogglePhase,
    last_enabled: bool,
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn lerp_hsla(a: Hsla, b: Hsla, t: f32) -> Hsla {
    Hsla {
        h: lerp(a.h, b.h, t),
        s: lerp(a.s, b.s, t),
        l: lerp(a.l, b.l, t),
        a: lerp(a.a, b.a, t),
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
            colors: colors.clone(),
            enabled,
            on_toggle: Rc::new(on_toggle),
        }
    }
}

impl RenderOnce for ToggleSwitch {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| ToggleAnimState {
            phase: TogglePhase::Stable,
            last_enabled: self.enabled,
        });

        let enabled = self.enabled;
        let colors = self.colors;
        let on_toggle = self.on_toggle.clone();

        if state.read(cx).last_enabled != enabled {
            let now = Instant::now();
            state.update(cx, |s, _| {
                s.phase = if enabled {
                    TogglePhase::Opening { at: now }
                } else {
                    TogglePhase::Closing { at: now }
                };
                s.last_enabled = enabled;
            });
        }

        let phase = state.read(cx).phase;

        let k = match phase {
            TogglePhase::Stable => {
                if enabled {
                    1.0
                } else {
                    0.0
                }
            }
            TogglePhase::Opening { at } => {
                let t = (Instant::now().duration_since(at).as_secs_f32() / ANIM_DURATION)
                    .clamp(0.0, 1.0);
                let eased = ease_out_cubic(t);

                if t >= 1.0 {
                    state.update(cx, |s, _| s.phase = TogglePhase::Stable);
                } else {
                    window.request_animation_frame();
                }

                eased
            }
            TogglePhase::Closing { at } => {
                let t = (Instant::now().duration_since(at).as_secs_f32() / ANIM_DURATION)
                    .clamp(0.0, 1.0);
                let eased = ease_out_cubic(t);

                if t >= 1.0 {
                    state.update(cx, |s, _| s.phase = TogglePhase::Stable);
                } else {
                    window.request_animation_frame();
                }

                1.0 - eased
            }
        };

        let track_color = lerp_hsla(
            Hsla {
                a: 1.0,
                ..colors.border
            },
            Hsla {
                a: 1.0,
                ..colors.accent
            },
            k,
        );

        let knob_left = px(KNOB_INSET_X + KNOB_TRAVEL * k);
        let knob_top = px(KNOB_INSET_Y);

        div()
            .w(px(TRACK_W))
            .h(px(TRACK_H))
            .rounded(px(999.0))
            .bg(track_color)
            .relative()
            .cursor_pointer()
            .shadow(vec![BoxShadow {
                color: Hsla {
                    a: 0.10,
                    ..rgb(0x000000).into()
                },
                blur_radius: px(8.0),
                spread_radius: px(-3.0),
                offset: point(px(0.0), px(2.0)),
            }])
            .child(
                div()
                    .absolute()
                    .top(knob_top)
                    .left(knob_left)
                    .w(px(KNOB_SIZE))
                    .h(px(KNOB_SIZE))
                    .rounded(px(999.0))
                    .bg(rgb(0xffffff))
                    .shadow(vec![BoxShadow {
                        color: Hsla {
                            a: 0.16,
                            ..rgb(0x000000).into()
                        },
                        blur_radius: px(8.0),
                        spread_radius: px(0.0),
                        offset: point(px(0.0), px(2.0)),
                    }]),
            )
            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                (on_toggle)(cx);
            })
    }
}
