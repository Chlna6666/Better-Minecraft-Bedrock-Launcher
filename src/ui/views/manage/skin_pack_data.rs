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
    div()
        .size_full()
        .min_h(px(0.))
        .flex()
        .flex_col()
        .child(div().flex_1().min_h(px(0.)).child(render_asset_list(
            colors,
            version,
            state,
            filtered_asset_indices,
            asset_scroll_handle,
            cx,
        )))
        .into_any_element()
}

impl ManagePageView {
    pub(super) fn set_skin_pack_as_default(
        &mut self,
        asset_key: SharedString,
        cx: &mut Context<Self>,
    ) {
        let (version, config, asset) = {
            let state = cx.global::<ManagePageState>();
            let Some(version) = self.selected_version(state).cloned() else {
                return;
            };
            let Some(asset) = resolve_asset_by_key(state, &asset_key)
                .filter(|asset| asset.kind == state::ManageAssetKind::SkinPack)
            else {
                toast::error(cx, SharedString::from("皮肤包不存在"));
                return;
            };
            (version, state.version_config.clone(), asset)
        };

        cx.spawn(async move |handle, cx| {
            let result =
                data::set_vanilla_skin_pack_redirect(&version, &config, Some(&asset)).await;
            let _ = handle.update(cx, |_this, cx| {
                match result {
                    Ok(next_config) => {
                        cx.update_global(|state: &mut ManagePageState, _cx| {
                            state.version_config = next_config;
                            state.version_config_error = None;
                        });
                        toast::success(cx, SharedString::from("默认皮肤已更新"));
                    }
                    Err(error) => {
                        toast::error(cx, SharedString::from(error));
                    }
                }
                cx.notify();
            });
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn clear_vanilla_skin_pack_redirect(&mut self, cx: &mut Context<Self>) {
        let (version, config) = {
            let state = cx.global::<ManagePageState>();
            let Some(version) = self.selected_version(state).cloned() else {
                return;
            };
            (version, state.version_config.clone())
        };

        cx.spawn(async move |handle, cx| {
            let result = data::set_vanilla_skin_pack_redirect(&version, &config, None).await;
            let _ = handle.update(cx, |_this, cx| {
                match result {
                    Ok(next_config) => {
                        cx.update_global(|state: &mut ManagePageState, _cx| {
                            state.version_config = next_config;
                            state.version_config_error = None;
                        });
                        toast::success(cx, SharedString::from("默认皮肤已清除"));
                    }
                    Err(error) => {
                        toast::error(cx, SharedString::from(error));
                    }
                }
                cx.notify();
            });
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }
}
