//! View controls stay separate from mesh preparation and shader policy.
use super::super::rendering::LightingMode;
use super::*;
use gpui_3d::VoxelDisplay;

impl World3dEditorView {
    pub(super) fn render_sky_color(&self) -> Hsla {
        let mut color: Rgba = sky_color(self.dimension).into();
        if self.dimension == Dimension::Overworld && self.render_settings.lighting.is_lit() {
            let brightness = 0.1 + 0.9 * self.render_settings.shading().sky_brightness;
            color.r *= brightness;
            color.g *= brightness;
            color.b *= brightness;
        }
        color.into()
    }
    fn set_lighting_mode(&mut self, lighting: LightingMode, cx: &mut Context<Self>) {
        if self.render_settings.lighting == lighting {
            return;
        }
        self.render_settings.lighting = lighting;
        self.chunk_lights.clear();
        self.scene_dirty = true;
        self.render_settings.fog = lighting.is_lit();
        self.render_settings.shadows = lighting == LightingMode::VibrantPreview;
        if !lighting.is_lit() {
            self.render_settings.cycle = false;
        }
        if let Some(service) = &self.service {
            service.send(super::super::types::ChunkServiceCommand::SetLighting(
                lighting,
            ));
        }
        self.status = "正在后台更新渲染模式…".into();
        cx.notify();
    }

    pub(super) fn render_rendering_controls(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut panel = div().flex().flex_col().gap(px(6.));
        panel = panel.child(section_title(colors, "渲染模式"));
        let mut modes = div().flex().flex_wrap().gap(px(4.));
        for mode in [
            LightingMode::NightVision,
            LightingMode::Vanilla,
            LightingMode::VibrantPreview,
        ] {
            modes = modes.child(
                toolbar_button(colors, mode.label(), self.render_settings.lighting == mode)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.set_lighting_mode(mode, cx)),
                    ),
            );
        }
        panel = panel.child(modes);
        let mut displays = div().flex().flex_wrap().gap(px(4.));
        for (mode, label) in [
            (VoxelDisplay::Textured, "贴图"),
            (VoxelDisplay::VertexColor, "顶点色"),
            (VoxelDisplay::Wireframe, "方块线框"),
        ] {
            displays = displays.child(
                toolbar_button(colors, label, self.render_settings.display == mode).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.render_settings.display = mode;
                        cx.notify();
                    }),
                ),
            );
        }
        panel = panel.child(displays).child(
            toolbar_button(colors, "雾", self.render_settings.fog).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.render_settings.fog = !this.render_settings.fog;
                    cx.notify();
                }),
            ),
        );
        if self.render_settings.lighting == LightingMode::VibrantPreview {
            let mut effects = div().flex().flex_wrap().gap(px(4.));
            for (index, label, active) in [
                (0, "云层", self.render_settings.clouds),
                (1, "反射", self.render_settings.reflections),
            ] {
                effects = effects.child(toolbar_button(colors, label, active).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        if index == 0 {
                            this.render_settings.clouds = !this.render_settings.clouds;
                        } else {
                            this.render_settings.reflections = !this.render_settings.reflections;
                        }
                        cx.notify();
                    }),
                ));
            }
            panel = panel.child(effects);
            let mut quality = div().flex().flex_wrap().gap(px(4.));
            for (scale, label) in [(0.5, "性能"), (0.75, "均衡"), (1., "清晰")] {
                quality = quality.child(
                    toolbar_button(
                        colors,
                        label,
                        self.render_settings.resolution_scale == scale,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.render_settings.resolution_scale = scale;
                            cx.notify();
                        }),
                    ),
                );
            }
            panel = panel.child(quality);
            panel = panel.child(
                toolbar_button(colors, "太阳阴影", self.render_settings.shadows).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.render_settings.shadows = !this.render_settings.shadows;
                        cx.notify();
                    }),
                ),
            );
            panel = panel.child(
                div()
                    .text_size(px(10.))
                    .text_color(colors.text_muted)
                    .child("实验画质模式：反射、透光与原版效果仍有差异。"),
            );
        }
        if self.render_settings.lighting.is_lit() {
            let view = cx.entity().downgrade();
            panel = panel.child(info_row(
                colors,
                "日夜时间",
                format!(
                    "{:02}:00",
                    ((self.render_settings.time * 24. + 6.) % 24.) as u32
                )
                .into(),
            ));
            panel = panel.child(
                Slider::new(
                    "world3d-time",
                    colors,
                    0.,
                    1.,
                    self.render_settings.time,
                    move |value, app| {
                        let _ = view.update(app, |this, cx| {
                            this.render_settings.time = value;
                            cx.notify();
                        });
                    },
                )
                .width(px(270.)),
            );
            panel = panel.child(
                toolbar_button(colors, "昼夜循环", self.render_settings.cycle).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.render_settings.cycle = !this.render_settings.cycle;
                        this.last_light_tick = None;
                        cx.notify();
                    }),
                ),
            );
        }
        panel
    }

    pub(super) fn tick_lighting(&mut self, now: Instant, window: &mut Window) {
        if !self.render_settings.cycle || !self.render_settings.lighting.is_lit() {
            self.last_light_tick = None;
            return;
        }
        if let Some(previous) = self.last_light_tick {
            self.render_settings.time = (self.render_settings.time
                + now.saturating_duration_since(previous).as_secs_f32() / 1200.)
                % 1.;
        }
        self.last_light_tick = Some(now);
        window.request_animation_frame();
    }
}
