use super::*;
use std::time::Instant;

pub(super) fn render_curseforge_content(
    window: &mut Window,
    cx: &mut App,
    colors: &ThemeColors,
    curseforge_results_list: &Entity<super::CurseForgeResultsListView>,
    now: Instant,
) -> Div {
    super::render_curseforge_content(window, cx, colors, curseforge_results_list, now)
}
