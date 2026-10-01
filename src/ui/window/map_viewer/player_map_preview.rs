//! Selected filled-map preview in the player item inspector.

use super::model::MapViewerWindowView;
use super::prelude::*;
use ::bedrock_world::map_item::{MapItemId, filled_map_id};

pub(super) struct PlayerMapPreview {
    pub(super) id: MapItemId,
    pub(super) status: PlayerMapPreviewStatus,
}

pub(super) enum PlayerMapPreviewStatus {
    Loading,
    Ready(Arc<RenderImage>),
    Missing,
    Invalid,
    Error(SharedString),
}

enum MapPixelsLookup {
    Ready(::bedrock_world::map_item::Pixels),
    Missing,
    Invalid,
}

impl MapViewerWindowView {
    pub(super) fn clear_player_map_preview(&mut self, cx: &mut Context<Self>) {
        self.player_workspace.map_preview_generation = self
            .player_workspace
            .map_preview_generation
            .saturating_add(1);
        if let Some(PlayerMapPreview {
            status: PlayerMapPreviewStatus::Ready(image),
            ..
        }) = self.player_workspace.map_preview.take()
        {
            cx.drop_image(image, None);
        }
    }

    pub(super) fn sync_selected_player_map_preview(&mut self, cx: &mut Context<Self>) {
        let requested = self
            .selected_workspace_entry()
            .and_then(|entry| filled_map_id(&entry.item.nbt));
        if self
            .player_workspace
            .map_preview
            .as_ref()
            .map(|preview| &preview.id)
            == requested.as_ref()
        {
            return;
        }
        self.clear_player_map_preview(cx);
        let Some(id) = requested else {
            return;
        };
        let generation = self.player_workspace.map_preview_generation;
        self.player_workspace.map_preview = Some(PlayerMapPreview {
            id: id.clone(),
            status: PlayerMapPreviewStatus::Loading,
        });
        let world_path = self.world_path.clone();
        let query_budget = self.map_query_budget.clone();
        cx.spawn(async move |handle, cx| {
            let _query_permit = query_budget.acquire().await;
            let result = cx
                .background_spawn(async move {
                    let world = World::open(&world_path, ::bedrock_world::OpenOptions::default())
                        .map_err(|error| error.to_string())?;
                    let Some(map) = world.map_item(&id).map_err(|error| error.to_string())? else {
                        return Ok::<_, String>(MapPixelsLookup::Missing);
                    };
                    Ok(match map.pixels {
                        Some(pixels) => MapPixelsLookup::Ready(pixels),
                        None => MapPixelsLookup::Invalid,
                    })
                })
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if this.player_workspace.map_preview_generation != generation {
                    return;
                }
                let status = match result {
                    Ok(MapPixelsLookup::Ready(pixels)) => RenderImage::from_raw_pixels(
                        pixels.width,
                        pixels.height,
                        ImagePixelFormat::Rgba8,
                        pixels.rgba,
                    )
                    .map(|image| PlayerMapPreviewStatus::Ready(Arc::new(image)))
                    .unwrap_or_else(|error| {
                        PlayerMapPreviewStatus::Error(SharedString::from(error.to_string()))
                    }),
                    Ok(MapPixelsLookup::Missing) => PlayerMapPreviewStatus::Missing,
                    Ok(MapPixelsLookup::Invalid) => PlayerMapPreviewStatus::Invalid,
                    Err(error) => PlayerMapPreviewStatus::Error(SharedString::from(error)),
                };
                if let Some(preview) = this.player_workspace.map_preview.as_mut() {
                    preview.status = status;
                    cx.notify();
                }
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn render_player_map_preview(&self, colors: &ThemeColors) -> Option<AnyElement> {
        let preview = self.player_workspace.map_preview.as_ref()?;
        let content = match &preview.status {
            PlayerMapPreviewStatus::Loading => div()
                .text_color(colors.text_muted)
                .child(t!("MapViewer.map_preview_loading"))
                .into_any_element(),
            PlayerMapPreviewStatus::Ready(image) => div()
                .w_full()
                .flex()
                .justify_center()
                .child(img(image.clone()).w(px(128.0)).h(px(128.0)))
                .into_any_element(),
            PlayerMapPreviewStatus::Missing => div()
                .text_color(colors.text_muted)
                .child(t!("MapViewer.map_preview_missing"))
                .into_any_element(),
            PlayerMapPreviewStatus::Invalid => div()
                .text_color(colors.text_muted)
                .child(t!("MapViewer.map_preview_invalid"))
                .into_any_element(),
            PlayerMapPreviewStatus::Error(error) => div()
                .text_color(colors.text_muted)
                .child(t!("MapViewer.map_preview_error", error = error))
                .into_any_element(),
        };
        Some(
            div()
                .w_full()
                .p(px(9.0))
                .rounded(px(4.0))
                .border_1()
                .border_color(colors.border)
                .flex()
                .flex_col()
                .gap(px(7.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(format!("{} #{}", t!("MapViewer.map"), preview.id)),
                )
                .child(content)
                .into_any_element(),
        )
    }
}
