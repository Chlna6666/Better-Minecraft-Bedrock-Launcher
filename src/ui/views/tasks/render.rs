use super::TasksPageView;
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};
use gpui::*;
use std::time::Instant;

#[path = "render/card.rs"]
mod card;
#[path = "render/overlay.rs"]
mod overlay;
#[path = "render/page.rs"]
mod page;
#[path = "render/progress.rs"]
mod progress;
#[path = "render/shell.rs"]
mod shell;

pub(crate) use card::render_task_card;
pub use overlay::render_tasks_overlay;
use page::render_tasks_page;
pub(crate) use progress::progress_panel;
pub(crate) use shell::{
    TaskVisualKind, page_shell, task_border_color, task_card_bg, task_card_hover_bg,
    task_icon_button, task_status_accent, task_text_main, task_text_secondary, task_text_tertiary,
    task_visual_accent, task_visual_icon, task_visual_kind, task_warning_color,
};

impl Render for TasksPageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let theme = cx.global::<ThemeState>();
        let colors = lerp_theme_colors(
            &LightColors::colors(),
            &DarkColors::colors(),
            theme.factor(now),
            theme.accent,
        );
        render_tasks_page(colors, self, window, cx)
    }
}
