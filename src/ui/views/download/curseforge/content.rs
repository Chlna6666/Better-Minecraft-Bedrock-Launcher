use super::*;
use std::time::Instant;

pub(super) fn render_curseforge_content(
    colors: &ThemeColors,
    state: &DownloadPageState,
    curseforge_results_list: &Entity<super::CurseForgeResultsListView>,
    now: Instant,
) -> Div {
    super::render_curseforge_content(colors, state, curseforge_results_list, now)
}
