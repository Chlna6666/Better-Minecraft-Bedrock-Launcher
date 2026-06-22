use super::*;

impl ManagePageView {
    pub(super) fn backup_map_asset(&mut self, asset: ManageAssetEntry, cx: &mut Context<Self>) {
        let map_name = asset.display_name.to_string();
        let folder_path = asset.file_path.to_string();
        cx.spawn(async move |_handle, cx| {
            let result =
                tokio::task::spawn_blocking(move || data::backup_map(&folder_path, &map_name))
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|result| result);

            let _ = cx.update(|cx| match result {
                Ok(path) => {
                    toast::success(cx, SharedString::from(format!("备份已创建: {path}")));
                }
                Err(error) => {
                    toast::error(cx, SharedString::from(error));
                }
            });
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn export_map_asset(
        &mut self,
        asset: ManageAssetEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.defer(cx, move |_window, cx| {
            cx.spawn(async move |cx| {
                let default_file_name = format!("{}.mcworld", asset.display_name);
                let target = tokio::task::spawn_blocking(move || {
                    pick_save_path_with_filter("Minecraft World", &["mcworld"], &default_file_name)
                })
                .await
                .ok()
                .flatten();

                let Some(target) = target else {
                    return Ok::<(), anyhow::Error>(());
                };
                let folder_path = asset.file_path.to_string();
                let result =
                    tokio::task::spawn_blocking(move || data::export_map(&folder_path, &target))
                        .await
                        .map_err(|error| error.to_string())
                        .and_then(|result| result);

                cx.update(|cx| match result {
                    Ok(()) => {
                        toast::success(cx, SharedString::from("地图已导出"));
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
}

pub(super) fn append_map_asset_actions(
    actions: Div,
    colors: &ThemeColors,
    version: &ManagedVersionEntry,
    asset: &ManageAssetEntry,
    action_key: &SharedString,
    cx: &mut Context<ManagePageView>,
) -> Div {
    actions
        .child(
            compact_icon_button(
                colors,
                SharedString::from(format!("manage-map-preview-{}", asset.key)),
                lucide_icons::icon_eye(),
            )
            .on_mouse_down(MouseButton::Left, {
                let version = version.clone();
                let key = action_key.clone();
                cx.listener(move |_this, _, _, cx| {
                    let asset = resolve_asset_by_key(cx.global::<ManagePageState>(), &key);
                    if let Some(asset) = asset {
                        crate::ui::window::map_viewer::open_map_viewer_window(
                            crate::ui::window::map_viewer::MapViewerWindowInit {
                                version: version.clone(),
                                world_path: asset.file_path.clone(),
                                asset,
                                initial_mode: bedrock_render::RenderMode::SurfaceBlocks,
                            },
                            cx,
                        );
                    }
                })
            }),
        )
        .child(
            compact_icon_button(
                colors,
                SharedString::from(format!("manage-map-edit-{}", asset.key)),
                lucide_icons::icon_file_pen_line(),
            )
            .on_mouse_down(MouseButton::Left, {
                let key = action_key.clone();
                cx.listener(move |this, _, window, cx| {
                    let asset = resolve_asset_by_key(cx.global::<ManagePageState>(), &key);
                    if let Some(asset) = asset {
                        this.open_level_dat_editor(asset, window, cx);
                    }
                })
            }),
        )
        .child(
            compact_icon_button(
                colors,
                SharedString::from(format!("manage-map-backup-{}", asset.key)),
                lucide_icons::icon_archive(),
            )
            .on_mouse_down(MouseButton::Left, {
                let key = action_key.clone();
                cx.listener(move |this, _, _, cx| {
                    let asset = resolve_asset_by_key(cx.global::<ManagePageState>(), &key);
                    if let Some(asset) = asset {
                        this.backup_map_asset(asset, cx);
                    }
                })
            }),
        )
        .child(
            compact_icon_button(
                colors,
                SharedString::from(format!("manage-map-export-{}", asset.key)),
                lucide_icons::icon_share_2(),
            )
            .on_mouse_down(MouseButton::Left, {
                let key = action_key.clone();
                cx.listener(move |this, _, window, cx| {
                    let asset = resolve_asset_by_key(cx.global::<ManagePageState>(), &key);
                    if let Some(asset) = asset {
                        this.export_map_asset(asset, window, cx);
                    }
                })
            }),
        )
        .child(
            compact_icon_button(
                colors,
                SharedString::from(format!("manage-map-launch-{}", asset.key)),
                lucide_icons::icon_play(),
            )
            .on_mouse_down(MouseButton::Left, {
                let version = version.clone();
                let key = action_key.clone();
                cx.listener(move |_this, _, _, cx| {
                    let asset = resolve_asset_by_key(cx.global::<ManagePageState>(), &key);
                    if let Some(asset) = asset {
                        launch_map_version(&version, &asset, cx);
                    }
                })
            }),
        )
}
