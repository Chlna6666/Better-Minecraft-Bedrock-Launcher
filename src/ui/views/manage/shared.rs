use super::*;

pub(super) const LEVEL_DAT_EDITOR_ROUTE_PATH: &str = "/manage/level-dat-editor";
pub(super) const MANAGE_ASSET_ROW_HEIGHT_PX: f32 = 58.0;
pub(super) const MANAGE_ASSET_ROW_GAP_PX: f32 = 10.0;
pub(super) const MANAGE_ASSET_ROW_PITCH_PX: f32 =
    MANAGE_ASSET_ROW_HEIGHT_PX + MANAGE_ASSET_ROW_GAP_PX;
pub(super) const MANAGE_ASSET_ROW_OVERSCAN: usize = 8;
pub(super) const MANAGE_ASSET_HEAVY_BUDGET: usize = 24;
pub(super) const MANAGE_VERSION_ROW_HEIGHT_PX: f32 = 66.0;
pub(super) const MANAGE_VERSION_ROW_GAP_PX: f32 = 4.0;
pub(super) const MANAGE_VERSION_ROW_PITCH_PX: f32 =
    MANAGE_VERSION_ROW_HEIGHT_PX + MANAGE_VERSION_ROW_GAP_PX;
pub(super) const MANAGE_VERSION_ROW_OVERSCAN: usize = 6;

pub(super) fn render_manage_loading_rows(colors: &ThemeColors, count: usize) -> Div {
    let row_bg = Hsla {
        a: 0.42,
        ..colors.surface
    };
    let block = Hsla {
        a: 0.10,
        ..colors.text_secondary
    };

    div()
        .size_full()
        .min_h(px(0.))
        .flex()
        .flex_col()
        .children((0..count.max(1)).map(|index| {
            let title_width = 138.0 - (index % 3) as f32 * 16.0;
            let detail_width = 92.0 + (index % 2) as f32 * 22.0;
            div()
                .w_full()
                .h(px(MANAGE_ASSET_ROW_PITCH_PX))
                .pb(px(MANAGE_ASSET_ROW_GAP_PX))
                .flex_none()
                .child(
                    div()
                        .w_full()
                        .h(px(MANAGE_ASSET_ROW_HEIGHT_PX))
                        .px(px(10.))
                        .rounded(px(crate::ui::theme::tokens::radius::SM))
                        .bg(row_bg)
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .child(
                            div()
                                .w(px(32.))
                                .h(px(32.))
                                .rounded(px(MANAGE_LIST_THUMBNAIL_RADIUS_PX))
                                .bg(block),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .gap(px(7.))
                                .child(
                                    div()
                                        .w(px(title_width))
                                        .h(px(9.))
                                        .rounded(px(4.5))
                                        .bg(block),
                                )
                                .child(
                                    div()
                                        .w(px(detail_width))
                                        .h(px(7.))
                                        .rounded(px(3.5))
                                        .bg(Hsla { a: 0.07, ..block }),
                                ),
                        )
                        .child(
                            div()
                                .w(px(56.))
                                .h(px(8.))
                                .rounded(px(4.))
                                .bg(Hsla { a: 0.06, ..block }),
                        ),
                )
        }))
}
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
    let mut updates = task_manager::subscribe_task_updates();

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
            loop {
                let snapshot = match updates.recv().await {
                    Ok(snapshot) => snapshot,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(skipped, "manage import task watcher lagged; resyncing");
                        let Some(snapshot) = task_manager::get_snapshot_arc(task_id.as_ref())
                        else {
                            continue;
                        };
                        snapshot
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        warn!("manage import task watcher closed");
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
                    let _i18n = cx.global::<I18n>();
                    match snapshot_clone.status.as_ref() {
                        "completed" => {
                            cx.update_global(|state: &mut ManagePageState, _cx| {
                                state.selected_asset_keys.clear();
                                state.assets_loaded = false;
                                state.assets_loading = false;
                                state.assets_error = None;
                            });
                            toast::success(cx, t!("ManagePage.import_completed"));
                        }
                        "cancelled" => {
                            toast::push(cx, t!("ManagePage.import_cancelled"));
                        }
                        "error" => {
                            let message = snapshot_clone
                                .message
                                .as_ref()
                                .map(|message| SharedString::from(message.to_string()))
                                .unwrap_or_else(|| t!("ManagePage.import_failed"));
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

#[derive(Clone)]
pub(super) enum VersionMutationUiCompletion {
    Deleted { folder: String },
    Renamed { new_name: String },
}

pub(super) fn watch_version_mutation_task(
    task_id: String,
    completion: VersionMutationUiCompletion,
    success_message: SharedString,
    view_handle: WeakEntity<ManagePageView>,
    cx: &mut App,
) {
    let task_id: Arc<str> = Arc::from(task_id);
    let mut updates = task_manager::subscribe_task_updates();

    cx.spawn({
        let task_id = task_id.clone();
        async move |cx| {
            let snapshot = loop {
                if let Some(snapshot) = task_manager::get_snapshot_arc(task_id.as_ref())
                    && snapshot.is_terminal()
                {
                    break snapshot;
                }

                match updates.recv().await {
                    Ok(snapshot)
                        if snapshot.id.as_ref() == task_id.as_ref() && snapshot.is_terminal() =>
                    {
                        break snapshot;
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(
                            skipped,
                            task_id = %task_id,
                            "version mutation watcher lagged; resyncing"
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        warn!(task_id = %task_id, "version mutation watcher closed");
                        return Ok::<(), anyhow::Error>(());
                    }
                }
            };

            let _ = cx.update(|cx| {
                let _i18n = cx.global::<I18n>();
                match snapshot.status.as_ref() {
                    "completed" => {
                        match &completion {
                            VersionMutationUiCompletion::Deleted { folder } => {
                                remove_local_version(folder, cx);
                            }
                            VersionMutationUiCompletion::Renamed { new_name } => {
                                cx.update_global(|state: &mut ManagePageState, _cx| {
                                    state.selected_folder =
                                        Some(SharedString::from(new_name.clone()));
                                });
                            }
                        }
                        let _ = view_handle.update(cx, |this, cx| {
                            this.invalidate_version_dependent_data(cx);
                            cx.notify();
                        });
                        ensure_local_versions_loaded(true, cx);
                        toast::success(cx, success_message);
                    }
                    "cancelled" => {
                        ensure_local_versions_loaded(true, cx);
                        toast::push(cx, t!("Tasks.status.cancelled"));
                    }
                    "error" => {
                        ensure_local_versions_loaded(true, cx);
                        let message = snapshot
                            .message
                            .as_ref()
                            .map(|message| SharedString::from(message.to_string()))
                            .unwrap_or_else(|| SharedString::from("版本操作失败"));
                        toast::error(cx, message);
                    }
                    _ => {}
                }
            });
            Ok::<(), anyhow::Error>(())
        }
    })
    .detach();
}

pub(super) fn watch_manage_asset_mutation_task(
    task_id: String,
    success_message: SharedString,
    cx: &mut App,
) {
    let task_id: Arc<str> = Arc::from(task_id);
    let mut updates = task_manager::subscribe_task_updates();

    cx.spawn({
        let task_id = task_id.clone();
        async move |cx| {
            let snapshot = loop {
                if let Some(snapshot) = task_manager::get_snapshot_arc(task_id.as_ref())
                    && snapshot.is_terminal()
                {
                    break snapshot;
                }

                match updates.recv().await {
                    Ok(snapshot) if snapshot.id.as_ref() == task_id.as_ref() && snapshot.is_terminal() => {
                        break snapshot;
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(skipped, task_id = %task_id, "manage mutation watcher lagged; resyncing");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        warn!(task_id = %task_id, "manage mutation watcher closed");
                        return Ok::<(), anyhow::Error>(());
                    }
                }
            };

            let _ = cx.update(|cx| {
                let _i18n = cx.global::<I18n>();
                cx.update_global(|state: &mut ManagePageState, _cx| {
                    state.selected_asset_keys.clear();
                    state.assets_loaded = false;
                    state.assets_loading = false;
                    state.assets_error = None;
                });
                match snapshot.status.as_ref() {
                    "completed" => {
                        toast::success(cx, success_message);
                    }
                    "cancelled" => {
                        toast::push(cx, t!("Tasks.status.cancelled"));
                    }
                    "error" => {
                        let message = snapshot
                            .message
                            .as_ref()
                            .map(|message| SharedString::from(message.to_string()))
                            .unwrap_or_else(|| SharedString::from("资源操作失败"));
                        toast::error(cx, message);
                    }
                    _ => {}
                }
            });
            Ok::<(), anyhow::Error>(())
        }
    })
    .detach();
}

pub(super) fn watch_screenshot_mutation_task(
    task_id: String,
    success_message: SharedString,
    view_handle: WeakEntity<ManagePageView>,
    cx: &mut App,
) {
    let task_id: Arc<str> = Arc::from(task_id);
    let mut updates = task_manager::subscribe_task_updates();

    cx.spawn({
        let task_id = task_id.clone();
        async move |cx| {
            let snapshot = loop {
                if let Some(snapshot) = task_manager::get_snapshot_arc(task_id.as_ref())
                    && snapshot.is_terminal()
                {
                    break snapshot;
                }

                match updates.recv().await {
                    Ok(snapshot)
                        if snapshot.id.as_ref() == task_id.as_ref() && snapshot.is_terminal() =>
                    {
                        break snapshot;
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(
                            skipped,
                            task_id = %task_id,
                            "screenshot mutation watcher lagged; resyncing"
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        warn!(task_id = %task_id, "screenshot mutation watcher closed");
                        return Ok::<(), anyhow::Error>(());
                    }
                }
            };

            let _ = cx.update(|cx| {
                let _i18n = cx.global::<I18n>();
                let _ = view_handle.update(cx, |this, cx| {
                    this.last_screenshots_signature = None;
                    cx.notify();
                });
                cx.update_global(|state: &mut ManagePageState, _cx| {
                    state.screenshots_loaded = false;
                    state.screenshots_loading = false;
                });

                match snapshot.status.as_ref() {
                    "completed" => {
                        toast::success(cx, success_message);
                    }
                    "cancelled" => {
                        toast::push(cx, t!("Tasks.status.cancelled"));
                    }
                    "error" => {
                        let message = snapshot
                            .message
                            .as_ref()
                            .map(|message| SharedString::from(message.to_string()))
                            .unwrap_or_else(|| SharedString::from("截图操作失败"));
                        toast::error(cx, message);
                    }
                    _ => {}
                }
            });
            Ok::<(), anyhow::Error>(())
        }
    })
    .detach();
}

pub(super) fn watch_server_mutation_task(
    task_id: String,
    success_message: SharedString,
    view_handle: WeakEntity<ManagePageView>,
    cx: &mut App,
) {
    let task_id: Arc<str> = Arc::from(task_id);
    let mut updates = task_manager::subscribe_task_updates();

    cx.spawn({
        let task_id = task_id.clone();
        async move |cx| {
            let snapshot = loop {
                if let Some(snapshot) = task_manager::get_snapshot_arc(task_id.as_ref())
                    && snapshot.is_terminal()
                {
                    break snapshot;
                }

                match updates.recv().await {
                    Ok(snapshot)
                        if snapshot.id.as_ref() == task_id.as_ref() && snapshot.is_terminal() =>
                    {
                        break snapshot;
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(
                            skipped,
                            task_id = %task_id,
                            "server mutation watcher lagged; resyncing"
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        warn!(task_id = %task_id, "server mutation watcher closed");
                        return Ok::<(), anyhow::Error>(());
                    }
                }
            };

            let _ = cx.update(|cx| {
                let _i18n = cx.global::<I18n>();
                let _ = view_handle.update(cx, |this, cx| {
                    this.last_servers_signature = None;
                    cx.notify();
                });
                cx.update_global(|state: &mut ManagePageState, _cx| {
                    state.servers_loaded = false;
                    state.servers_loading = false;
                    state.servers_error = None;
                    state.server_motd = Arc::new(HashMap::new());
                    state.server_motd_loading = false;
                    state.server_motd_request_id =
                        state.server_motd_request_id.wrapping_add(1);
                });

                match snapshot.status.as_ref() {
                    "completed" => {
                        toast::success(cx, success_message);
                    }
                    "cancelled" => {
                        toast::push(cx, t!("Tasks.status.cancelled"));
                    }
                    "error" => {
                        let message = snapshot
                            .message
                            .as_ref()
                            .map(|message| SharedString::from(message.to_string()))
                            .unwrap_or_else(|| SharedString::from("服务器操作失败"));
                        toast::error(cx, message);
                    }
                    _ => {}
                }
            });
            Ok::<(), anyhow::Error>(())
        }
    })
    .detach();
}

pub(super) fn watch_levilamina_install_task(task_id: String, cx: &mut App) {
    let wait_task = gpui_tokio::Tokio::spawn_result(cx, async move {
        task_manager::wait_for_task_terminal(&task_id)
            .await
            .map_err(anyhow::Error::msg)
    });
    cx.spawn(async move |cx| {
        if wait_task
            .await
            .is_ok_and(|snapshot| snapshot.status.as_ref() == "completed")
        {
            cx.update(|cx| ensure_local_versions_loaded(true, cx))?;
        }
        Ok::<(), anyhow::Error>(())
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct VersionListSignature {
    versions_revision: u64,
    versions_len: usize,
    query: SharedString,
}

impl VersionListSignature {
    pub(super) fn from_state(state: &ManagePageState) -> Self {
        Self {
            versions_revision: state.versions_revision,
            versions_len: state.versions.len(),
            query: SharedString::from(state.search_query.trim().to_string()),
        }
    }
}

#[derive(Default)]
pub(super) struct VersionListRenderCache {
    signature: Option<VersionListSignature>,
    filtered_indices: Vec<usize>,
}

pub(super) enum VersionListRefresh {
    Unchanged,
    ContentChanged,
    QueryChanged,
}

impl VersionListRenderCache {
    pub(super) fn refresh(&mut self, state: &ManagePageState) -> VersionListRefresh {
        let signature = VersionListSignature::from_state(state);
        if self.signature.as_ref() == Some(&signature) {
            return VersionListRefresh::Unchanged;
        }

        let query_changed = self
            .signature
            .as_ref()
            .is_some_and(|previous| previous.query != signature.query);
        self.filtered_indices = build_filtered_version_indices(state, &signature);
        self.signature = Some(signature);
        if query_changed {
            VersionListRefresh::QueryChanged
        } else {
            VersionListRefresh::ContentChanged
        }
    }

    pub(super) fn filtered_indices(&self) -> &[usize] {
        &self.filtered_indices
    }
}

pub(super) fn build_filtered_version_indices(
    state: &ManagePageState,
    signature: &VersionListSignature,
) -> Vec<usize> {
    let query = signature.query.as_ref();
    if query.is_empty() {
        return (0..state.versions.len()).collect();
    }

    let needle = query.to_ascii_lowercase();
    state
        .versions
        .iter()
        .enumerate()
        .filter(|(_, version)| {
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
        .map(|(index, _)| index)
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
        .opacity(0.96)
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
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .flex()
        .items_center()
        .justify_center()
        .bg(Hsla {
            a: 0.72,
            ..colors.surface
        })
        .border_1()
        .border_color(Hsla {
            a: 0.24,
            ..colors.border
        })
        .cursor_pointer()
        .active(|style| style.scale(0.94))
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
        .rounded(px(crate::ui::theme::tokens::radius::SM))
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
        .rounded(px(crate::ui::theme::tokens::radius::MD))
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

pub(super) fn mod_type_label(raw: &str, i18n: &crate::ui::state::i18n::I18n) -> SharedString {
    match raw.trim() {
        "preload-native" => i18n
            .lookup("AssetManager.mod_type_preload_native")
            .unwrap_or_else(|| SharedString::from("Native Load")),
        "hot-inject" => i18n
            .lookup("AssetManager.mod_type_hot_inject")
            .unwrap_or_else(|| SharedString::from("Delayed Load")),
        "native" => i18n
            .lookup("AssetManager.mod_type_native")
            .unwrap_or_else(|| SharedString::from("Native")),
        "lse-quickjs" => i18n
            .lookup("AssetManager.mod_type_lse_quickjs")
            .unwrap_or_else(|| SharedString::from("LSE QuickJS")),
        value if !value.is_empty() => SharedString::from(value.to_string()),
        _ => SharedString::from("Unknown"),
    }
}
