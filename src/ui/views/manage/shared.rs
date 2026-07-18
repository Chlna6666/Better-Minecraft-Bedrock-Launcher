use super::*;

pub(super) const LEVEL_DAT_EDITOR_ROUTE_PATH: &str = "/manage/level-dat-editor";
pub(super) const MANAGE_ASSET_ROW_HEIGHT_PX: f32 = 58.0;
pub(super) const MANAGE_ASSET_ROW_GAP_PX: f32 = 10.0;
pub(super) const MANAGE_ASSET_ROW_PITCH_PX: f32 =
    MANAGE_ASSET_ROW_HEIGHT_PX + MANAGE_ASSET_ROW_GAP_PX;
pub(super) const MANAGE_ASSET_ROW_OVERSCAN: usize = 8;
pub(super) const MANAGE_ASSET_HEAVY_BUDGET: usize = 24;
pub(super) fn create_text_input(
    window: &mut Window,
    cx: &mut Context<ManagePageView>,
    placeholder: &str,
    initial: &str,
) -> Option<Entity<InputState>> {
    Some(cx.new(|cx| {
        let mut input = InputState::new(window, cx);
        input.set_placeholder(SharedString::from(placeholder.to_string()), window, cx);
        if !initial.trim().is_empty() {
            input.set_value(SharedString::from(initial.to_string()), window, cx);
        }
        input
    }))
}

pub(super) fn watch_import_task(task_id: String, cx: &mut App) {
    let task_id: Arc<str> = Arc::from(task_id);

    if let Some(snapshot) = task_manager::get_snapshot_arc(task_id.as_ref()) {
        if matches!(
            snapshot.status.as_ref(),
            "completed" | "cancelled" | "error"
        ) {
            ensure_local_versions_loaded(true, cx);
            return;
        }
    }

    cx.spawn({
        let task_id = task_id.clone();
        async move |cx| {
            let mut updates = task_manager::subscribe_task_updates();
            loop {
                let snapshot = match updates.recv().await {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        warn!("manage import task watcher closed: {error}");
                        break;
                    }
                };
                if snapshot.id.as_ref() != task_id.as_ref() {
                    continue;
                }
                if !matches!(
                    snapshot.status.as_ref(),
                    "completed" | "cancelled" | "error"
                ) {
                    continue;
                }

                let snapshot_clone = snapshot.clone();
                let _ = cx.update(|cx| {
                    ensure_local_versions_loaded(true, cx);
                    match snapshot_clone.status.as_ref() {
                        "completed" => {
                            toast::success(cx, SharedString::from("导入任务已完成"));
                        }
                        "cancelled" => {
                            toast::push(cx, SharedString::from("导入任务已取消"));
                        }
                        "error" => {
                            let message = snapshot_clone
                                .message
                                .as_ref()
                                .map(|message| SharedString::from(message.to_string()))
                                .unwrap_or_else(|| SharedString::from("导入任务失败"));
                            toast::error(cx, message);
                        }
                        _ => {}
                    }
                });
                break;
            }

            Ok::<(), anyhow::Error>(())
        }
    })
    .detach();
}

pub(super) fn launch_map_version(
    version: &ManagedVersionEntry,
    asset: &ManageAssetEntry,
    cx: &mut Context<ManagePageView>,
) {
    let encoded_folder: String = byte_serialize(asset.folder_name.as_ref().as_bytes()).collect();
    let descriptor = LaunchVersionDescriptor {
        folder: version.folder.clone(),
        name: version.name.clone(),
        version: version.version.clone(),
        kind: version.kind.clone(),
        path: version.path.clone(),
        launch_args: Some(SharedString::from(format!(
            "minecraft://?load={encoded_folder}"
        ))),
    };
    let _ = start_launcher(descriptor, cx);
}
pub(super) fn selected_asset_folder_names(state: &ManagePageState) -> Vec<String> {
    let selected_keys = state
        .selected_asset_keys
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    state
        .assets
        .iter()
        .filter(|asset| selected_keys.contains(&asset.key))
        .map(|asset| asset.folder_name.to_string())
        .collect()
}

pub(super) fn filtered_versions(state: &ManagePageState) -> Vec<&ManagedVersionEntry> {
    let query = state.search_query.trim();
    if query.is_empty() {
        return state.versions.iter().collect();
    }

    let needle = query.to_ascii_lowercase();
    state
        .versions
        .iter()
        .filter(|version| {
            version
                .folder
                .as_ref()
                .to_ascii_lowercase()
                .contains(&needle)
                || version.name.as_ref().to_ascii_lowercase().contains(&needle)
                || version
                    .version
                    .as_ref()
                    .to_ascii_lowercase()
                    .contains(&needle)
                || version
                    .manifest_version
                    .as_ref()
                    .to_ascii_lowercase()
                    .contains(&needle)
        })
        .collect()
}
pub(super) fn resolve_asset_by_key(
    state: &ManagePageState,
    key: &SharedString,
) -> Option<ManageAssetEntry> {
    state.assets.iter().find(|asset| asset.key == *key).cloned()
}

pub(super) fn resolve_screenshot_by_key(
    state: &ManagePageState,
    key: &SharedString,
) -> Option<ManageScreenshotEntry> {
    state
        .screenshots
        .iter()
        .find(|entry| entry.key == *key)
        .cloned()
}

pub(super) fn resolve_server_by_key(
    state: &ManagePageState,
    key: &SharedString,
) -> Option<ManageServerEntry> {
    state
        .servers
        .iter()
        .find(|entry| entry.key == *key)
        .cloned()
}

pub(super) fn is_asset_tab(tab: ManageTab) -> bool {
    matches!(
        tab,
        ManageTab::Mod | ManageTab::ResourcePack | ManageTab::SkinPack | ManageTab::Map
    )
}

pub(super) fn is_gdk_user_scoped_tab(tab: ManageTab) -> bool {
    matches!(
        tab,
        ManageTab::Map | ManageTab::Screenshot | ManageTab::Server
    )
}

pub(super) fn mini_icon_button(
    colors: &ThemeColors,
    id: impl Into<ElementId>,
    icon_path: &'static str,
) -> Stateful<Div> {
    icon_action(colors, id, icon_path)
}

pub(super) fn sidebar_icon_button(
    id: impl Into<ElementId>,
    icon_path: &'static str,
    colors: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(18.))
        .h(px(18.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .child(
            svg()
                .path(icon_path)
                .w(px(15.))
                .h(px(15.))
                .text_color(colors.text_secondary),
        )
}

pub(super) fn toolbar_glyph_button(
    id: impl Into<ElementId>,
    icon_path: &'static str,
    colors: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(24.))
        .h(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .opacity(0.92)
        .hover(|style| style.opacity(1.0))
        .child(
            svg()
                .path(icon_path)
                .w(px(18.))
                .h(px(18.))
                .text_color(colors.text_secondary),
        )
}

pub(super) fn compact_icon_button(
    colors: &ThemeColors,
    id: impl Into<ElementId>,
    icon_path: &'static str,
) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(28.))
        .h(px(28.))
        .rounded(px(8.))
        .flex()
        .items_center()
        .justify_center()
        .bg(colors.surface)
        .border_1()
        .border_color(colors.border)
        .cursor_pointer()
        .child(
            svg()
                .path(icon_path)
                .w(px(13.))
                .h(px(13.))
                .text_color(colors.text_secondary),
        )
}

pub(super) fn icon_badge(colors: &ThemeColors, icon_path: &'static str) -> Div {
    div()
        .w(px(40.))
        .h(px(40.))
        .rounded(px(14.))
        .bg(Hsla {
            a: 0.12,
            ..colors.accent
        })
        .border_1()
        .border_color(Hsla {
            a: 0.14,
            ..colors.border
        })
        .flex()
        .items_center()
        .justify_center()
        .child(
            svg()
                .path(icon_path)
                .w(px(18.))
                .h(px(18.))
                .text_color(colors.accent),
        )
}

pub(super) fn formatted_single_line(
    text: impl Into<SharedString>,
    colors: &ThemeColors,
    size: Pixels,
    color: Hsla,
) -> AnyElement {
    div()
        .overflow_hidden()
        .child(
            MinecraftFormattedText::new(text.into(), colors)
                .text_size(size)
                .line_height(relative(1.2))
                .color(color)
                .wrap(false),
        )
        .into_any_element()
}

pub(super) fn error_panel(colors: &ThemeColors, error: SharedString) -> AnyElement {
    div()
        .w_full()
        .h_full()
        .rounded(px(16.))
        .bg(Hsla {
            a: 0.10,
            ..colors.danger
        })
        .p(px(16.))
        .text_size(px(13.))
        .line_height(relative(1.5))
        .text_color(colors.danger)
        .child(error)
        .into_any_element()
}

pub(super) fn clamp_scroll_at_edges(
    scroll_handle: &ScrollHandle,
    event: &ScrollWheelEvent,
    window: &mut Window,
    cx: &mut App,
) {
    let offset = scroll_handle.offset();
    let max_offset = scroll_handle.max_offset();
    let delta_y = scroll_event_delta_y(event);
    let at_bottom = offset.y <= -max_offset.height;
    let at_top = offset.y >= px(0.);

    if (at_bottom && delta_y < Pixels::ZERO) || (at_top && delta_y > Pixels::ZERO) {
        scroll_handle.set_offset(point(offset.x, offset.y.clamp(-max_offset.height, px(0.))));
        window.prevent_default();
        cx.stop_propagation();
    }
}

pub(super) fn scroll_event_delta_y(event: &ScrollWheelEvent) -> Pixels {
    match event.delta {
        ScrollDelta::Pixels(delta) => delta.y,
        ScrollDelta::Lines(delta) => px(delta.y * 20.0),
    }
}

pub(super) fn mod_type_label(raw: &str) -> SharedString {
    match raw.trim() {
        "preload-native" => SharedString::from("Preload Native"),
        "hot-inject" => SharedString::from("Hot Inject"),
        "native" => SharedString::from("Native"),
        "lse-quickjs" => SharedString::from("LSE QuickJS"),
        value if !value.is_empty() => SharedString::from(value.to_string()),
        _ => SharedString::from("Unknown"),
    }
}
