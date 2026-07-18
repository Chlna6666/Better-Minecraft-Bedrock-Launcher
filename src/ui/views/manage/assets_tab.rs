use super::*;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct AssetListSignature {
    pub(super) assets_ptr: usize,
    pub(super) assets_len: usize,
    pub(super) tab: ManageTab,
    pub(super) pack_subtype: ManagePackSubtype,
    pub(super) selected_gdk_user: Option<SharedString>,
    pub(super) query: SharedString,
    pub(super) sort_key: ManageAssetSortKey,
    pub(super) sort_desc: bool,
}

#[derive(Default)]
pub(super) struct AssetListRenderCache {
    pub(super) signature: Option<AssetListSignature>,
    pub(super) filtered_indices: Vec<usize>,
}

pub(super) struct AssetSortEntry {
    pub(super) index: usize,
    pub(super) display_name_lower: String,
    pub(super) folder_name_lower: String,
}

impl ManagePageView {
    pub(super) fn open_skin_preview_asset(&mut self, asset: ManageAssetEntry, cx: &mut App) {
        let skins = asset
            .skin_previews
            .as_ref()
            .map(|skins| {
                skins
                    .iter()
                    .map(|skin| crate::ui::window::skin_pack::SkinPreviewWindowSkin {
                        display_name: skin.display_name.clone(),
                        texture_path: skin.full_texture_path.clone(),
                        model_label: Some(skin.model_label.clone()),
                        preview_path: skin.preview_path.clone(),
                        geometry_path: skin.geometry_path.clone(),
                        geometry_identifier: skin.geometry_identifier.clone(),
                    })
                    .collect::<Vec<_>>()
            })
            .filter(|skins| !skins.is_empty())
            .or_else(|| {
                asset
                    .first_skin_full_texture_path
                    .clone()
                    .map(|texture_path| {
                        vec![crate::ui::window::skin_pack::SkinPreviewWindowSkin {
                            display_name: asset.display_name.clone(),
                            texture_path,
                            model_label: asset.first_skin_model_label.clone(),
                            preview_path: asset.icon_path.clone(),
                            geometry_path: None,
                            geometry_identifier: None,
                        }]
                    })
            });
        let Some(skins) = skins else {
            toast::error(cx, SharedString::from("这个皮肤包没有可预览的皮肤贴图"));
            return;
        };

        crate::ui::window::skin_pack::open_skin_preview_window(
            crate::ui::window::skin_pack::SkinPreviewWindowInit {
                title: asset.display_name,
                skins: Arc::from(skins.into_boxed_slice()),
                selected_index: 0,
            },
            cx,
        );
    }

    pub(super) fn set_pack_subtype(
        &mut self,
        pack_subtype: ManagePackSubtype,
        cx: &mut Context<Self>,
    ) {
        cx.update_global(|state: &mut ManagePageState, _cx| {
            state.pack_subtype = pack_subtype;
            state.selected_asset_keys.clear();
        });
        self.last_assets_signature = None;
        self.reset_asset_list_view();
        cx.notify();
    }

    pub(super) fn set_asset_sort(&mut self, key: ManageAssetSortKey, cx: &mut Context<Self>) {
        cx.update_global(|state: &mut ManagePageState, _cx| {
            if state.asset_sort_key == key {
                state.asset_sort_desc = !state.asset_sort_desc;
            } else {
                state.asset_sort_key = key;
                state.asset_sort_desc = !matches!(key, ManageAssetSortKey::Name);
            }
        });
        self.reset_asset_list_view();
        cx.notify();
    }

    pub(super) fn toggle_asset_selection(&mut self, key: SharedString, cx: &mut Context<Self>) {
        cx.update_global(|state: &mut ManagePageState, _cx| {
            if let Some(index) = state
                .selected_asset_keys
                .iter()
                .position(|item| *item == key)
            {
                state.selected_asset_keys.remove(index);
            } else {
                state.selected_asset_keys.push(key);
            }
        });
        cx.notify();
    }

    pub(super) fn select_asset_only(&mut self, key: SharedString, cx: &mut Context<Self>) {
        cx.update_global(|state: &mut ManagePageState, _cx| {
            state.selected_asset_keys.clear();
            state.selected_asset_keys.push(key);
        });
        cx.notify();
    }
    pub(super) fn import_assets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (version, config, tab, pack_subtype, selected_gdk_user) = {
            let state = cx.global::<ManagePageState>();
            let Some(version) = self.selected_version(state).cloned() else {
                return;
            };
            (
                version,
                state.version_config.clone(),
                state.tab,
                state.pack_subtype,
                state.selected_gdk_user.clone(),
            )
        };

        let (filter_name, extensions): (&str, &[&str]) = match tab {
            ManageTab::Mod => ("DLL", &["dll"]),
            ManageTab::ResourcePack => ("Packs", &["mcpack", "mcaddon", "mctemplate", "zip"]),
            ManageTab::SkinPack => ("Skin Packs", &["mcpack", "mcaddon", "zip"]),
            ManageTab::Map => ("Maps", &["mcworld", "mctemplate", "zip"]),
            ManageTab::Screenshot | ManageTab::Server => return,
        };

        window.defer(cx, move |_window, cx| {
            cx.spawn(async move |cx| {
                let files = cx
                    .background_spawn_blocking(move || {
                        pick_file_paths_with_filter(filter_name, extensions)
                    })
                    .await;

                if files.is_empty() {
                    return Ok::<(), anyhow::Error>(());
                }

                let result = match tab {
                    ManageTab::Mod => data::import_mod_files(
                        version.folder.as_ref(),
                        &files.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    )
                    .await
                    .map(|_| format!("已导入 {} 个 Mod", files.len())),
                    ManageTab::ResourcePack | ManageTab::SkinPack | ManageTab::Map => {
                        data::import_non_mod_files(
                            &version,
                            &config,
                            tab,
                            pack_subtype,
                            selected_gdk_user.as_ref().map(SharedString::as_ref),
                            files,
                            false,
                            false,
                        )
                        .await
                        .map(|summary| {
                            format!(
                                "导入完成：成功 {} 个，失败 {} 个",
                                summary.imported_count, summary.failed_count
                            )
                        })
                    }
                    ManageTab::Screenshot | ManageTab::Server => Ok(String::new()),
                };

                cx.update(|cx| match result {
                    Ok(message) => {
                        toast::success(cx, SharedString::from(message));
                        cx.update_global(|state: &mut ManagePageState, _cx| {
                            state.selected_asset_keys.clear();
                            state.assets_loaded = false;
                            state.assets_loading = false;
                            state.assets_error = None;
                        });
                    }
                    Err(error) => {
                        toast::error(cx, SharedString::from(error));
                    }
                })?;
                Ok::<(), anyhow::Error>(())
            })
            .detach();
        });
    }

    pub(super) fn request_delete_selected_assets(&mut self, cx: &mut Context<Self>) {
        let state = cx.global::<ManagePageState>();
        let Some(version) = self.selected_version(state).cloned() else {
            return;
        };
        let folder_names = selected_asset_folder_names(state);
        if folder_names.is_empty() {
            return;
        }
        self.confirm_dialog = Some(ConfirmDialogState {
            title: SharedString::from("删除资源"),
            description: SharedString::from(format!(
                "确定删除选中的 {} 个资源吗？",
                folder_names.len()
            )),
            confirm_label: SharedString::from("删除所选"),
            danger: true,
            pending: false,
            action: ConfirmAction::DeleteAssets {
                version,
                config: state.version_config.clone(),
                tab: state.tab,
                pack_subtype: state.pack_subtype,
                selected_gdk_user: state.selected_gdk_user.clone(),
                folder_names,
            },
        });
        cx.notify();
    }

    pub(super) fn open_asset_folder(&mut self, path: SharedString, cx: &mut Context<Self>) {
        cx.spawn(async move |_handle, cx| {
            if let Err(error) = crate::utils::open_path::open_path(path.to_string()).await {
                let _ = cx.update(|cx| {
                    toast::error(cx, SharedString::from(error));
                });
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }
}

fn append_skin_pack_asset_actions(
    actions: Div,
    colors: &ThemeColors,
    asset: &ManageAssetEntry,
    action_key: &SharedString,
    is_default: bool,
    cx: &mut Context<ManagePageView>,
) -> Div {
    let actions = if is_default {
        actions.child(
            skin_default_action_button(
                colors,
                SharedString::from(format!("manage-clear-default-skin-{}", asset.key)),
                true,
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.clear_vanilla_skin_pack_redirect(cx);
                }),
            ),
        )
    } else {
        actions.child(
            skin_default_action_button(
                colors,
                SharedString::from(format!("manage-set-default-skin-{}", asset.key)),
                false,
            )
            .on_mouse_down(MouseButton::Left, {
                let key = action_key.clone();
                cx.listener(move |this, _, _, cx| {
                    this.set_skin_pack_as_default(key.clone(), cx);
                })
            }),
        )
    };

    actions.child(
        compact_icon_button(
            colors,
            SharedString::from(format!("manage-skin-preview-{}", asset.key)),
            lucide_icons::icon_box(),
        )
        .on_mouse_down(MouseButton::Left, {
            let key = action_key.clone();
            cx.listener(move |this, _, _, cx| {
                let asset = resolve_asset_by_key(cx.global::<ManagePageState>(), &key);
                if let Some(asset) = asset {
                    this.open_skin_preview_asset(asset, cx);
                }
            })
        }),
    )
}

fn skin_default_action_button(
    colors: &ThemeColors,
    id: impl Into<ElementId>,
    active: bool,
) -> Stateful<Div> {
    let icon_color = if active {
        colors.accent
    } else {
        colors.text_secondary
    };

    div()
        .id(id)
        .w(px(28.))
        .h(px(28.))
        .rounded(px(8.))
        .flex()
        .items_center()
        .justify_center()
        .bg(if active {
            Hsla {
                a: 0.12,
                ..colors.accent
            }
        } else {
            colors.surface
        })
        .border_1()
        .border_color(if active {
            Hsla {
                a: 0.20,
                ..colors.accent
            }
        } else {
            colors.border
        })
        .cursor_pointer()
        .child(
            svg()
                .path(lucide_icons::icon_star())
                .w(px(13.))
                .h(px(13.))
                .text_color(icon_color),
        )
}

fn is_default_skin_pack_asset(state: &ManagePageState, asset: &ManageAssetEntry) -> bool {
    state
        .version_config
        .vanilla_skin_pack_redirect
        .as_ref()
        .is_some_and(|target| asset_path_matches(&asset.open_path, target))
}

fn asset_path_matches(left: &SharedString, right: &SharedString) -> bool {
    normalize_asset_path(left.as_ref()) == normalize_asset_path(right.as_ref())
}

fn normalize_asset_path(path: &str) -> String {
    path.trim().replace('/', "\\").to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn test_asset(
        key: &str,
        display_name: &str,
        folder_name: &str,
        detail: Option<&str>,
        description: Option<&str>,
        source: Option<&str>,
        size_bytes: Option<u64>,
        modified_iso: Option<&str>,
    ) -> ManageAssetEntry {
        ManageAssetEntry {
            key: SharedString::from(key.to_string()),
            folder_name: SharedString::from(folder_name.to_string()),
            display_name: SharedString::from(display_name.to_string()),
            detail: detail.map(|value| SharedString::from(value.to_string())),
            description: description.map(|value| SharedString::from(value.to_string())),
            file_path: SharedString::from(folder_name.to_string()),
            open_path: SharedString::from(folder_name.to_string()),
            icon_path: None,
            modified_iso: modified_iso.map(|value| SharedString::from(value.to_string())),
            modified_label: None,
            size_bytes,
            size_label: None,
            source: source.map(|value| SharedString::from(value.to_string())),
            edition: None,
            gdk_user: None,
            enabled: None,
            mod_type: None,
            inject_delay_ms: None,
            resource_pack_count: None,
            behavior_pack_count: None,
            skin_count: None,
            first_skin_full_texture_path: None,
            first_skin_model_label: None,
            skin_previews: None,
            kind: state::ManageAssetKind::ResourcePack,
        }
    }

    pub(super) fn test_state(assets: Vec<ManageAssetEntry>) -> ManagePageState {
        ManagePageState {
            assets: Arc::from(assets),
            ..ManagePageState::default()
        }
    }

    #[::core::prelude::v1::test]
    pub(super) fn asset_filter_matches_metadata_fields() {
        let mut state = test_state(vec![
            test_asset(
                "asset:alpha",
                "Alpha",
                "alpha-folder",
                Some("graphics"),
                Some("Bright shader pack"),
                Some("Marketplace"),
                Some(32),
                Some("2024-02-01"),
            ),
            test_asset(
                "asset:beta",
                "Beta",
                "beta-folder",
                Some("world"),
                Some("Survival map"),
                None,
                Some(12),
                Some("2024-01-01"),
            ),
        ]);

        state.asset_search_query = SharedString::from("survival");
        let signature = AssetListSignature::from_state(&state);
        let indices = build_filtered_asset_indices(&state, &signature);
        assert_eq!(indices, vec![1]);

        state.asset_search_query = SharedString::from("market");
        let signature = AssetListSignature::from_state(&state);
        let indices = build_filtered_asset_indices(&state, &signature);
        assert_eq!(indices, vec![0]);
    }

    #[::core::prelude::v1::test]
    pub(super) fn asset_sort_cache_reuses_signature_and_sorts_size() {
        let mut state = test_state(vec![
            test_asset(
                "asset:large",
                "Large",
                "large-folder",
                None,
                None,
                None,
                Some(64),
                Some("2024-02-01"),
            ),
            test_asset(
                "asset:small",
                "Small",
                "small-folder",
                None,
                None,
                None,
                Some(8),
                Some("2024-01-01"),
            ),
        ]);
        state.asset_sort_key = ManageAssetSortKey::Size;
        state.asset_sort_desc = false;

        let mut cache = AssetListRenderCache::default();
        assert!(cache.refresh(&state));
        assert_eq!(cache.filtered_indices(), &[1, 0]);
        assert!(!cache.refresh(&state));

        state.asset_sort_desc = true;
        assert!(cache.refresh(&state));
        assert_eq!(cache.filtered_indices(), &[0, 1]);
    }

    #[::core::prelude::v1::test]
    pub(super) fn asset_virtual_list_keeps_render_window_small() {
        let plan = compute_virtual_list_plan(
            1_000,
            MANAGE_ASSET_ROW_PITCH_PX,
            px(-680.),
            px(340.),
            MANAGE_ASSET_ROW_OVERSCAN,
            MANAGE_ASSET_HEAVY_BUDGET,
        );

        assert!(plan.render_slice.visible_len() <= 24);
        assert!(plan.heavy_slice.len() <= MANAGE_ASSET_HEAVY_BUDGET);
        assert!(plan.render_slice.end_index < 40);
    }
}
impl AssetListSignature {
    pub(super) fn from_state(state: &ManagePageState) -> Self {
        Self {
            assets_ptr: state.assets.as_ref().as_ptr() as usize,
            assets_len: state.assets.len(),
            tab: state.tab,
            pack_subtype: state.pack_subtype,
            selected_gdk_user: state.selected_gdk_user.clone(),
            query: SharedString::from(state.asset_search_query.trim().to_string()),
            sort_key: state.asset_sort_key,
            sort_desc: state.asset_sort_desc,
        }
    }
}

impl AssetListRenderCache {
    pub(super) fn clear(&mut self) {
        self.signature = None;
        self.filtered_indices.clear();
    }

    pub(super) fn refresh(&mut self, state: &ManagePageState) -> bool {
        let signature = AssetListSignature::from_state(state);
        if self.signature.as_ref() == Some(&signature) {
            return false;
        }

        let started_at = Instant::now();
        self.filtered_indices = build_filtered_asset_indices(state, &signature);
        let elapsed_ms = started_at.elapsed().as_secs_f64() * 1000.0;
        if elapsed_ms >= 4.0 {
            tracing::debug!(
                "manage asset list cache refresh slow: elapsed_ms={elapsed_ms:.3} assets={} filtered={} query_len={} sort_key={:?} sort_desc={}",
                state.assets.len(),
                self.filtered_indices.len(),
                signature.query.as_ref().len(),
                signature.sort_key,
                signature.sort_desc
            );
        }

        self.signature = Some(signature);
        true
    }

    pub(super) fn filtered_indices(&self) -> &[usize] {
        &self.filtered_indices
    }
}

pub(super) fn build_filtered_asset_indices(
    state: &ManagePageState,
    signature: &AssetListSignature,
) -> Vec<usize> {
    let query = signature.query.as_ref().to_ascii_lowercase();
    let mut entries = Vec::with_capacity(state.assets.len());

    for (index, asset) in state.assets.iter().enumerate() {
        if !asset_matches_query(asset, &query) {
            continue;
        }

        entries.push(AssetSortEntry {
            index,
            display_name_lower: asset.display_name.as_ref().to_ascii_lowercase(),
            folder_name_lower: asset.folder_name.as_ref().to_ascii_lowercase(),
        });
    }

    entries.sort_by(|left, right| {
        let left_asset = &state.assets[left.index];
        let right_asset = &state.assets[right.index];
        let ordering = match signature.sort_key {
            ManageAssetSortKey::Name => left
                .display_name_lower
                .cmp(&right.display_name_lower)
                .then_with(|| left.folder_name_lower.cmp(&right.folder_name_lower)),
            ManageAssetSortKey::Date => left_asset
                .modified_iso
                .as_ref()
                .map(SharedString::as_ref)
                .unwrap_or("")
                .cmp(
                    &right_asset
                        .modified_iso
                        .as_ref()
                        .map(SharedString::as_ref)
                        .unwrap_or(""),
                )
                .then_with(|| left.display_name_lower.cmp(&right.display_name_lower)),
            ManageAssetSortKey::Size => left_asset
                .size_bytes
                .unwrap_or(0)
                .cmp(&right_asset.size_bytes.unwrap_or(0))
                .then_with(|| left.display_name_lower.cmp(&right.display_name_lower)),
        };

        if signature.sort_desc {
            ordering.reverse()
        } else {
            ordering
        }
    });

    entries.into_iter().map(|entry| entry.index).collect()
}

pub(super) fn asset_matches_query(asset: &ManageAssetEntry, query: &str) -> bool {
    query.is_empty()
        || text_contains_query(&asset.display_name, query)
        || text_contains_query(&asset.folder_name, query)
        || asset
            .detail
            .as_ref()
            .is_some_and(|detail| text_contains_query(detail, query))
        || asset
            .description
            .as_ref()
            .is_some_and(|description| text_contains_query(description, query))
        || asset
            .source
            .as_ref()
            .is_some_and(|source| text_contains_query(source, query))
}

pub(super) fn text_contains_query(text: &SharedString, query: &str) -> bool {
    text.as_ref().to_ascii_lowercase().contains(query)
}
pub(super) fn render_asset_list(
    colors: &ThemeColors,
    version: &ManagedVersionEntry,
    state: &ManagePageState,
    filtered_asset_indices: &[usize],
    asset_scroll_handle: &ScrollHandle,
    cx: &mut Context<ManagePageView>,
) -> AnyElement {
    let missing_gdk_user =
        version.is_gdk() && is_gdk_user_scoped_tab(state.tab) && state.selected_gdk_user.is_none();

    if state.gdk_users_loading
        && state.gdk_users.is_empty()
        && is_gdk_user_scoped_tab(state.tab)
        && version.is_gdk()
    {
        return empty_state(
            colors,
            "images/manage/empty.svg",
            "正在读取用户目录",
            "请稍候，BMCBL 正在扫描当前 GDK 实例的可用用户。",
        )
        .into_any_element();
    }

    if let Some(error) = state.gdk_users_error.clone() {
        if is_gdk_user_scoped_tab(state.tab) && version.is_gdk() {
            return div()
                .w_full()
                .h_full()
                .rounded(px(16.))
                .bg(Hsla {
                    a: 0.10,
                    ..colors.danger
                })
                .p(px(16.))
                .text_size(px(13.))
                .text_color(colors.danger)
                .child(error)
                .into_any_element();
        }
    }

    if missing_gdk_user {
        return empty_state(
            colors,
            "images/manage/empty.svg",
            "未找到可用用户目录",
            "当前 GDK 实例没有扫描到可读取地图的普通用户目录。",
        )
        .into_any_element();
    }

    if state.assets_loading && state.assets.is_empty() {
        return empty_state(
            colors,
            "images/manage/empty.svg",
            "正在加载资源",
            "当前实例的资源列表正在刷新。",
        )
        .into_any_element();
    }

    if let Some(error) = state.assets_error.clone() {
        return div()
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
            .into_any_element();
    }

    if filtered_asset_indices.is_empty() {
        let (title, description) = match state.tab {
            ManageTab::Mod => ("没有 Mod", "导入 DLL 后会显示在这里。"),
            ManageTab::ResourcePack => ("没有资源包", "支持导入 mcpack、mcaddon、zip。"),
            ManageTab::SkinPack => ("没有皮肤包", "支持导入 mcpack、mcaddon、zip。"),
            ManageTab::Map => ("没有地图", "支持导入 mcworld、mctemplate、zip。"),
            ManageTab::Screenshot => ("没有截图", "游戏截图会显示在这里。"),
            ManageTab::Server => ("没有服务器", "添加服务器后会显示在这里。"),
        };
        return empty_state(colors, "images/manage/empty.svg", title, description)
            .into_any_element();
    }

    let scroll_handle = asset_scroll_handle.clone();
    let virtual_list_plan = compute_virtual_list_plan(
        filtered_asset_indices.len(),
        MANAGE_ASSET_ROW_PITCH_PX,
        asset_scroll_handle.offset().y,
        asset_scroll_handle.bounds().size.height,
        MANAGE_ASSET_ROW_OVERSCAN,
        MANAGE_ASSET_HEAVY_BUDGET,
    );
    let selected_asset_keys = state
        .selected_asset_keys
        .iter()
        .cloned()
        .collect::<HashSet<_>>();

    let render_started_at = Instant::now();
    let mut rows = div().w_full().flex().flex_col().min_w(px(0.));
    if virtual_list_plan.render_slice.top_spacer > px(0.) {
        rows = rows.child(div().h(virtual_list_plan.render_slice.top_spacer));
    }

    for virtual_index in virtual_list_plan.render_slice.start_index
        ..virtual_list_plan
            .render_slice
            .end_index
            .min(filtered_asset_indices.len())
    {
        let Some(asset_index) = filtered_asset_indices.get(virtual_index).copied() else {
            continue;
        };
        let Some(asset) = state.assets.get(asset_index) else {
            continue;
        };
        rows = rows.child(
            div()
                .w_full()
                .h(px(MANAGE_ASSET_ROW_PITCH_PX))
                .pb(px(MANAGE_ASSET_ROW_GAP_PX))
                .flex_none()
                .child(render_asset_row(
                    colors,
                    state,
                    version,
                    asset,
                    selected_asset_keys.contains(&asset.key),
                    virtual_list_plan.heavy_slice.contains(virtual_index),
                    cx,
                )),
        );
    }

    if virtual_list_plan.render_slice.bottom_spacer > px(0.) {
        rows = rows.child(div().h(virtual_list_plan.render_slice.bottom_spacer));
    }

    let render_elapsed_ms = render_started_at.elapsed().as_secs_f64() * 1000.0;
    if render_elapsed_ms >= 6.0 {
        tracing::debug!(
            "manage asset rows render slow: elapsed_ms={render_elapsed_ms:.3} total={} render_start={} render_len={} visible_start={} visible_len={} heavy_start={} heavy_len={}",
            filtered_asset_indices.len(),
            virtual_list_plan.render_slice.start_index,
            virtual_list_plan.render_slice.visible_len(),
            virtual_list_plan.visible_slice.start_index,
            virtual_list_plan.visible_slice.len(),
            virtual_list_plan.heavy_slice.start_index,
            virtual_list_plan.heavy_slice.len()
        );
    }

    div()
        .id("manage-asset-list-scroll")
        .w_full()
        .h_full()
        .min_h(px(0.))
        .min_w(px(0.))
        .overflow_y_scroll()
        .track_scroll(asset_scroll_handle)
        .on_scroll_wheel(move |event, window, cx| {
            let offset = scroll_handle.offset();
            let max_offset = scroll_handle.max_offset();
            let delta_y = scroll_event_delta_y(event);
            let at_bottom = offset.y <= -max_offset.height;
            let at_top = offset.y >= px(0.);

            if (at_bottom && delta_y < Pixels::ZERO) || (at_top && delta_y > Pixels::ZERO) {
                scroll_handle
                    .set_offset(point(offset.x, offset.y.clamp(-max_offset.height, px(0.))));
                window.prevent_default();
                cx.stop_propagation();
            }
        })
        .child(rows)
        .into_any_element()
}
pub(super) fn render_asset_row(
    colors: &ThemeColors,
    state: &ManagePageState,
    version: &ManagedVersionEntry,
    asset: &ManageAssetEntry,
    is_selected: bool,
    render_heavy: bool,
    cx: &mut Context<ManagePageView>,
) -> Stateful<Div> {
    let selection_key = asset.key.clone();
    let select_only_key = asset.key.clone();
    let action_key = asset.key.clone();
    let asset_for_folder = asset.open_path.clone();
    let row_background = if is_selected {
        Hsla {
            a: 0.12,
            ..colors.accent
        }
    } else {
        colors.surface
    };
    let thumbnail_background = colors.surface.blend(row_background).alpha(1.0);

    let leading = if render_heavy {
        asset.icon_path.clone()
    } else {
        None
    }
    .map_or_else(
        || {
            let icon = match asset.kind {
                state::ManageAssetKind::Mod => lucide_icons::icon_layers(),
                state::ManageAssetKind::ResourcePack => lucide_icons::icon_package(),
                state::ManageAssetKind::SkinPack => lucide_icons::icon_user(),
                state::ManageAssetKind::Map => lucide_icons::icon_map(),
            };

            div()
                .w(px(32.))
                .h(px(32.))
                .rounded(px(MANAGE_LIST_THUMBNAIL_RADIUS_PX))
                .bg(Hsla {
                    a: 0.10,
                    ..colors.surface
                })
                .border_1()
                .border_color(colors.border)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path(icon)
                        .w(px(16.))
                        .h(px(16.))
                        .text_color(colors.text_secondary),
                )
                .into_any_element()
        },
        |icon_path| {
            let icon = match asset.kind {
                state::ManageAssetKind::Mod => lucide_icons::icon_layers(),
                state::ManageAssetKind::ResourcePack => lucide_icons::icon_package(),
                state::ManageAssetKind::SkinPack => lucide_icons::icon_user(),
                state::ManageAssetKind::Map => lucide_icons::icon_map(),
            };
            rounded_manage_thumbnail(colors, &icon_path, icon.into(), thumbnail_background)
        },
    );

    let title = if render_heavy {
        formatted_single_line(
            asset.display_name.clone(),
            colors,
            px(13.),
            colors.text_primary,
        )
    } else {
        div()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_size(px(13.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(colors.text_primary)
            .child(asset.display_name.clone())
            .into_any_element()
    };
    let is_default_skin_pack = matches!(asset.kind, state::ManageAssetKind::SkinPack)
        && is_default_skin_pack_asset(state, asset);

    let mut meta = div().flex().items_center().gap(px(8.)).overflow_hidden();
    if render_heavy && let Some(description) = asset.description.clone() {
        meta = meta.child(
            div().max_w(px(260.)).overflow_hidden().child(
                MinecraftFormattedText::new(description, colors)
                    .text_size(px(11.))
                    .line_height(relative(1.2))
                    .color(colors.text_muted)
                    .wrap(false),
            ),
        );
    }
    if let Some(detail) = asset.detail.clone() {
        meta = meta.child(
            div()
                .text_size(px(11.))
                .text_color(colors.text_secondary)
                .child(detail),
        );
    }
    if !matches!(asset.kind, state::ManageAssetKind::Map) {
        if let Some(size_label) = asset.size_label.clone() {
            meta = meta.child(
                div()
                    .text_size(px(11.))
                    .text_color(colors.text_secondary)
                    .child(size_label),
            );
        }
        if let Some(modified) = asset.modified_label.clone() {
            meta = meta.child(
                div()
                    .text_size(px(11.))
                    .text_color(colors.text_secondary)
                    .child(modified),
            );
        }
    }
    if let Some(resource_count) = asset.resource_pack_count {
        meta = meta.child(subtle_badge(colors, format!("资源包 {resource_count}")));
    }
    if let Some(behavior_count) = asset.behavior_pack_count {
        meta = meta.child(subtle_badge(colors, format!("行为包 {behavior_count}")));
    }
    if let Some(skin_count) = asset.skin_count {
        meta = meta.child(subtle_badge(colors, format!("皮肤 {skin_count}")));
    }
    if let Some(mod_type) = asset.mod_type.clone() {
        meta = meta.child(
            div()
                .text_size(px(11.))
                .text_color(colors.text_secondary)
                .child(mod_type_label(mod_type.as_ref())),
        );
    }
    if let Some(enabled) = asset.enabled {
        meta = meta.child(
            div()
                .text_size(px(11.))
                .text_color(if enabled {
                    colors.stat_green_text
                } else {
                    colors.danger
                })
                .child(if enabled { "已启用" } else { "已禁用" }),
        );
    }

    let mut actions = div().flex().items_center().justify_end().gap(px(6.)).child(
        compact_icon_button(
            colors,
            SharedString::from(format!("manage-open-asset-{}", asset.key)),
            lucide_icons::icon_folder_open(),
        )
        .on_mouse_down(MouseButton::Left, {
            let path = asset_for_folder.clone();
            cx.listener(move |this, _, _, cx| {
                this.open_asset_folder(path.clone(), cx);
            })
        }),
    );

    actions = match asset.kind {
        state::ManageAssetKind::Mod => {
            append_mod_asset_actions(actions, colors, asset, &action_key, cx)
        }
        state::ManageAssetKind::ResourcePack => actions,
        state::ManageAssetKind::SkinPack => append_skin_pack_asset_actions(
            actions,
            colors,
            asset,
            &action_key,
            is_default_skin_pack,
            cx,
        ),
        state::ManageAssetKind::Map => {
            append_map_asset_actions(actions, colors, version, asset, &action_key, cx)
        }
    };

    div()
        .id(SharedString::from(format!(
            "manage-asset-row-{}",
            asset.key
        )))
        .w_full()
        .h(px(MANAGE_ASSET_ROW_HEIGHT_PX))
        .flex_none()
        .rounded(px(10.))
        .border_1()
        .border_color(if is_selected {
            colors.accent
        } else {
            Hsla {
                a: 0.30,
                ..colors.border
            }
        })
        .bg(row_background)
        .px(px(10.))
        .py(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .gap(px(12.))
                        .items_center()
                        .child(
                            div()
                                .w(px(18.))
                                .h(px(18.))
                                .rounded(px(6.))
                                .border_1()
                                .border_color(if is_selected {
                                    colors.accent
                                } else {
                                    colors.border
                                })
                                .bg(if is_selected {
                                    Hsla {
                                        a: 0.16,
                                        ..colors.accent
                                    }
                                } else {
                                    colors.surface
                                })
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .when(is_selected, |this| {
                                    this.child(
                                        svg()
                                            .path(lucide_icons::icon_check())
                                            .w(px(11.))
                                            .h(px(11.))
                                            .text_color(colors.accent),
                                    )
                                })
                                .on_mouse_down(MouseButton::Left, {
                                    let key = selection_key.clone();
                                    cx.listener(move |this, _, _, cx| {
                                        this.toggle_asset_selection(key.clone(), cx);
                                    })
                                }),
                        )
                        .child(leading)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .gap(px(3.))
                                .cursor_pointer()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .min_w(px(0.))
                                        .child(div().flex_1().min_w(px(0.)).child(title))
                                        .when_some(asset.enabled, |this, enabled| {
                                            this.child(div().w(px(6.)).h(px(6.)).rounded_full().bg(
                                                if enabled {
                                                    colors.stat_green_text
                                                } else {
                                                    colors.danger
                                                },
                                            ))
                                        }),
                                )
                                .child(meta)
                                .on_mouse_down(MouseButton::Left, {
                                    let key = select_only_key.clone();
                                    cx.listener(move |this, _, _, cx| {
                                        this.select_asset_only(key.clone(), cx);
                                    })
                                }),
                        ),
                )
                .child(div().flex_shrink_0().child(actions)),
        )
}
