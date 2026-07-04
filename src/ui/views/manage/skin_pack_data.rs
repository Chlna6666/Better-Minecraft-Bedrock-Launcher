use super::*;

pub(super) fn render_skin_pack_management(
    colors: &ThemeColors,
    version: &ManagedVersionEntry,
    state: &ManagePageState,
    filtered_asset_indices: &[usize],
    asset_scroll_handle: &ScrollHandle,
    _window: &mut Window,
    cx: &mut Context<ManagePageView>,
) -> AnyElement {
    render_asset_list(
        colors,
        version,
        state,
        filtered_asset_indices,
        asset_scroll_handle,
        cx,
    )
    .into_any_element()
}
