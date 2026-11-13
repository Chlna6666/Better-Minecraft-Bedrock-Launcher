use super::TasksPageView;
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};
use gpui::*;

mod card;
mod overlay;
mod page;
mod progress;
mod shell;

pub use overlay::render_overlay;

pub(crate) use shell::{
    TaskVisualKind, page_shell, task_border_color, task_card_hover_bg, task_icon_button,
    task_status_accent, task_text_main, task_text_secondary, task_text_tertiary,
    task_visual_accent, task_visual_icon, task_visual_kind, task_warning_color,
};

impl Render for TasksPageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = window.animation_time();
        let theme = cx.global::<ThemeState>();
        let colors = lerp_theme_colors(
            &LightColors::theme(),
            &DarkColors::theme(),
            theme.factor(now),
            theme.accent,
        );
        page::render(colors, self, window, cx)
    }
}
