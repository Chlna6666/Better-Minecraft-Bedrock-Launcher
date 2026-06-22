use super::*;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct ScreenshotListSignature {
    pub(super) screenshots_ptr: usize,
    pub(super) screenshots_len: usize,
    pub(super) selected_gdk_user: Option<SharedString>,
    pub(super) query: SharedString,
}

#[derive(Default)]
pub(super) struct ScreenshotListRenderCache {
    pub(super) signature: Option<ScreenshotListSignature>,
    pub(super) filtered_indices: Vec<usize>,
}

impl ManagePageView {
    pub(super) fn refresh_screenshots(&mut self, cx: &mut Context<Self>) {
        self.last_screenshots_signature = None;
        self.reset_screenshot_list_view();
        cx.update_global(|state: &mut ManagePageState, _cx| {
            state.screenshots_loaded = false;
            state.screenshots_loading = false;
            state.screenshots_error = None;
        });
        cx.notify();
    }
    pub(super) fn request_delete_screenshot(
        &mut self,
        entry: ManageScreenshotEntry,
        cx: &mut Context<Self>,
    ) {
        self.confirm_dialog = Some(ConfirmDialogState {
            title: SharedString::from("删除截图"),
            description: SharedString::from(format!(
                "确定删除截图 {} 吗？同名 .json/.mc 文件也会一起删除。",
                entry.file_name
            )),
            confirm_label: SharedString::from("删除截图"),
            danger: true,
            pending: false,
            action: ConfirmAction::DeleteScreenshot { entry },
        });
        cx.notify();
    }
}

impl ScreenshotListSignature {
    pub(super) fn from_state(state: &ManagePageState) -> Self {
        Self {
            screenshots_ptr: state.screenshots.as_ref().as_ptr() as usize,
            screenshots_len: state.screenshots.len(),
            selected_gdk_user: state.selected_gdk_user.clone(),
            query: SharedString::from(state.screenshot_search_query.trim().to_string()),
        }
    }
}

impl ScreenshotListRenderCache {
    pub(super) fn clear(&mut self) {
        self.signature = None;
        self.filtered_indices.clear();
    }

    pub(super) fn refresh(&mut self, state: &ManagePageState) -> bool {
        let signature = ScreenshotListSignature::from_state(state);
        if self.signature.as_ref() == Some(&signature) {
            return false;
        }
        self.filtered_indices = build_filtered_screenshot_indices(state, &signature);
        self.signature = Some(signature);
        true
    }

    pub(super) fn filtered_indices(&self) -> &[usize] {
        &self.filtered_indices
    }
}

pub(super) fn build_filtered_screenshot_indices(
    state: &ManagePageState,
    signature: &ScreenshotListSignature,
) -> Vec<usize> {
    let query = signature.query.as_ref().to_ascii_lowercase();
    let mut indices = Vec::with_capacity(state.screenshots.len());
    for (index, screenshot) in state.screenshots.iter().enumerate() {
        if query.is_empty()
            || text_contains_query(&screenshot.file_name, &query)
            || screenshot
                .capture_time_label
                .as_ref()
                .is_some_and(|label| text_contains_query(label, &query))
            || screenshot
                .gdk_user
                .as_ref()
                .is_some_and(|user| text_contains_query(user, &query))
        {
            indices.push(index);
        }
    }
    indices.sort_by(|left, right| {
        let left = &state.screenshots[*left];
        let right = &state.screenshots[*right];
        right
            .capture_time_iso
            .as_ref()
            .or(right.modified_iso.as_ref())
            .map(SharedString::as_ref)
            .unwrap_or("")
            .cmp(
                left.capture_time_iso
                    .as_ref()
                    .or(left.modified_iso.as_ref())
                    .map(SharedString::as_ref)
                    .unwrap_or(""),
            )
            .then_with(|| right.file_name.as_ref().cmp(left.file_name.as_ref()))
    });
    indices
}
pub(super) fn render_screenshot_list(
    colors: &ThemeColors,
    version: &ManagedVersionEntry,
    state: &ManagePageState,
    filtered_indices: &[usize],
    scroll_handle: &ScrollHandle,
    cx: &mut Context<ManagePageView>,
) -> AnyElement {
    if state.gdk_users_loading && version.is_gdk() {
        return empty_state(
            colors,
            "images/manage/empty.svg",
            "正在读取用户目录",
            "请稍候，BMCBL 正在扫描当前 GDK 实例的可用用户。",
        )
        .into_any_element();
    }

    if version.is_gdk() && state.selected_gdk_user.is_none() {
        return empty_state(
            colors,
            "images/manage/empty.svg",
            "未找到可用用户目录",
            "当前 GDK 实例没有扫描到可读取截图的用户目录。",
        )
        .into_any_element();
    }

    if state.screenshots_loading {
        return empty_state(
            colors,
            "images/manage/empty.svg",
            "正在加载截图",
            "截图目录正在后台扫描。",
        )
        .into_any_element();
    }

    if let Some(error) = state.screenshots_error.clone() {
        return error_panel(colors, error);
    }

    if filtered_indices.is_empty() {
        return empty_state(
            colors,
            "images/manage/empty.svg",
            "没有截图",
            "游戏截图会显示在这里。",
        )
        .into_any_element();
    }

    let scroll_handle_for_event = scroll_handle.clone();
    let virtual_list_plan = compute_virtual_list_plan(
        filtered_indices.len(),
        MANAGE_ASSET_ROW_PITCH_PX,
        scroll_handle.offset().y,
        scroll_handle.bounds().size.height,
        MANAGE_ASSET_ROW_OVERSCAN,
        MANAGE_ASSET_HEAVY_BUDGET,
    );

    let mut rows = div().w_full().flex().flex_col().min_w(px(0.));
    if virtual_list_plan.render_slice.top_spacer > px(0.) {
        rows = rows.child(div().h(virtual_list_plan.render_slice.top_spacer));
    }

    for virtual_index in virtual_list_plan.render_slice.start_index
        ..virtual_list_plan
            .render_slice
            .end_index
            .min(filtered_indices.len())
    {
        let Some(index) = filtered_indices.get(virtual_index).copied() else {
            continue;
        };
        let Some(entry) = state.screenshots.get(index) else {
            continue;
        };
        rows = rows.child(
            div()
                .w_full()
                .h(px(MANAGE_ASSET_ROW_PITCH_PX))
                .pb(px(MANAGE_ASSET_ROW_GAP_PX))
                .flex_none()
                .child(render_screenshot_row(
                    colors,
                    entry,
                    virtual_list_plan.heavy_slice.contains(virtual_index),
                    cx,
                )),
        );
    }

    if virtual_list_plan.render_slice.bottom_spacer > px(0.) {
        rows = rows.child(div().h(virtual_list_plan.render_slice.bottom_spacer));
    }

    div()
        .id("manage-screenshot-list-scroll")
        .w_full()
        .h_full()
        .min_h(px(0.))
        .min_w(px(0.))
        .overflow_y_scroll()
        .track_scroll(scroll_handle)
        .on_scroll_wheel(move |event, window, cx| {
            clamp_scroll_at_edges(&scroll_handle_for_event, event, window, cx);
        })
        .child(rows)
        .into_any_element()
}
pub(super) fn render_screenshot_row(
    colors: &ThemeColors,
    entry: &ManageScreenshotEntry,
    render_heavy: bool,
    cx: &mut Context<ManagePageView>,
) -> Stateful<Div> {
    let key = entry.key.clone();
    let row_background = colors.surface;
    let thumbnail_background = colors.surface.blend(row_background).alpha(1.0);
    let leading = if render_heavy {
        rounded_asset_thumbnail(
            colors,
            &entry.image_path,
            lucide_icons::icon_image().into(),
            thumbnail_background,
        )
    } else {
        div()
            .w(px(32.))
            .h(px(32.))
            .rounded(px(MANAGE_ASSET_THUMBNAIL_RADIUS_PX))
            .bg(colors.surface_hover)
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .path(lucide_icons::icon_image())
                    .w(px(16.))
                    .h(px(16.))
                    .text_color(colors.text_secondary),
            )
            .into_any_element()
    };

    let mut meta = div().flex().items_center().gap(px(8.)).overflow_hidden();
    if let Some(label) = entry
        .capture_time_label
        .clone()
        .or(entry.modified_label.clone())
    {
        meta = meta.child(
            div()
                .text_size(px(11.))
                .text_color(colors.text_secondary)
                .child(label),
        );
    }
    if let Some(size) = entry.size_label.clone() {
        meta = meta.child(
            div()
                .text_size(px(11.))
                .text_color(colors.text_secondary)
                .child(size),
        );
    }
    if let Some(user) = entry.gdk_user.clone() {
        meta = meta.child(subtle_badge(colors, user));
    }

    let actions = div()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(6.))
        .child(
            compact_icon_button(
                colors,
                SharedString::from(format!("manage-screenshot-open-folder-{}", entry.key)),
                lucide_icons::icon_folder_open(),
            )
            .on_mouse_down(MouseButton::Left, {
                let folder = entry.folder_path.clone();
                cx.listener(move |this, _, _, cx| {
                    this.open_path_background(folder.clone(), cx);
                })
            }),
        )
        .child(
            compact_icon_button(
                colors,
                SharedString::from(format!("manage-screenshot-preview-{}", entry.key)),
                lucide_icons::icon_eye(),
            )
            .on_mouse_down(MouseButton::Left, {
                let image = entry.image_path.clone();
                cx.listener(move |this, _, _, cx| {
                    this.open_path_background(image.clone(), cx);
                })
            }),
        )
        .child(
            compact_icon_button(
                colors,
                SharedString::from(format!("manage-screenshot-delete-{}", entry.key)),
                lucide_icons::icon_trash_2(),
            )
            .on_mouse_down(MouseButton::Left, {
                cx.listener(move |this, _, _, cx| {
                    let entry = resolve_screenshot_by_key(cx.global::<ManagePageState>(), &key);
                    if let Some(entry) = entry {
                        this.request_delete_screenshot(entry, cx);
                    }
                })
            }),
        );

    div()
        .id(SharedString::from(format!(
            "manage-screenshot-row-{}",
            entry.key
        )))
        .w_full()
        .h(px(MANAGE_ASSET_ROW_HEIGHT_PX))
        .rounded(px(10.))
        .border_1()
        .border_color(colors.border)
        .bg(row_background)
        .px(px(10.))
        .flex()
        .items_center()
        .gap(px(12.))
        .child(leading)
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(entry.file_name.clone()),
                )
                .child(meta),
        )
        .child(actions)
}
