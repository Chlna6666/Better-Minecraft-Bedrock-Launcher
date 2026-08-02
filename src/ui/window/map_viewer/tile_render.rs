pub(super) use super::tile_render_legacy::*;

use super::model::{CHUNKS_PER_TILE, TileRenderEvent};
use super::tile_render_legacy::{
    TileBatchRequest, render_tile_batch_stream as render_tile_batch_stream_legacy,
};
use bedrock_render::RenderLayout;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use futures_util::StreamExt as _;
use std::time::Duration;

const TILE_PRESENT_INTERVAL: Duration = Duration::from_millis(8);

pub(super) const fn web_relief_render_layout() -> RenderLayout {
    RenderLayout {
        chunks_per_tile: CHUNKS_PER_TILE,
        blocks_per_pixel: 1,
        pixels_per_block: 1,
    }
}

pub(super) const fn tile_texture_render_layout(
    world_layout: RenderLayout,
    _viewport_scale: f32,
) -> RenderLayout {
    RenderLayout {
        chunks_per_tile: world_layout.chunks_per_tile,
        blocks_per_pixel: 1,
        pixels_per_block: 1,
    }
}

pub(super) fn render_tile_batch_stream(
    request: TileBatchRequest,
    event_sender: UnboundedSender<TileRenderEvent>,
) -> Result<(), String> {
    let (render_sender, mut render_receiver) = unbounded::<TileRenderEvent>();
    let render_thread = std::thread::Builder::new()
        .name("map-world-tile-render".to_string())
        .spawn(move || render_tile_batch_stream_legacy(request, render_sender))
        .map_err(|error| format!("创建地图瓦片渲染线程失败: {error}"))?;

    let forward_result = futures::executor::block_on(async move {
        while let Some(event) = render_receiver.next().await {
            match event {
                TileRenderEvent::ReadyBatch { tiles } => {
                    for (index, tile) in tiles.into_iter().enumerate() {
                        if index > 0 {
                            std::thread::sleep(TILE_PRESENT_INTERVAL);
                        }
                        event_sender
                            .unbounded_send(TileRenderEvent::ReadyBatch {
                                tiles: vec![tile],
                            })
                            .map_err(|_| "地图瓦片显示事件接收端已关闭".to_string())?;
                    }
                }
                event => {
                    let complete = matches!(&event, TileRenderEvent::Complete { .. });
                    event_sender
                        .unbounded_send(event)
                        .map_err(|_| "地图瓦片显示事件接收端已关闭".to_string())?;
                    if complete {
                        break;
                    }
                }
            }
        }
        Ok::<(), String>(())
    });

    let render_result = render_thread
        .join()
        .map_err(|_| "地图瓦片渲染线程崩溃".to_string())?;
    forward_result?;
    render_result
}
