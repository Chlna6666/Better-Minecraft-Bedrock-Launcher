pub(super) use super::tile_render_impl::*;

use super::model::TileRenderEvent;
use super::tile_render_impl::{
    TileBatchRequest, render_tile_batch_stream as render_tile_batch_stream_impl,
};
use futures::channel::mpsc::{UnboundedSender, unbounded};
use futures_util::StreamExt as _;
use std::time::Duration;

const TILE_PRESENT_INTERVAL: Duration = Duration::from_millis(16);

pub(super) fn render_tile_batch_stream(
    request: TileBatchRequest,
    event_sender: UnboundedSender<TileRenderEvent>,
) -> Result<(), String> {
    let (batch_sender, mut batch_receiver) = unbounded::<TileRenderEvent>();
    let forward_thread = std::thread::Builder::new()
        .name("map-tile-present".to_string())
        .spawn(move || {
            futures::executor::block_on(async move {
                while let Some(event) = batch_receiver.next().await {
                    match event {
                        TileRenderEvent::ReadyBatch { tiles } => {
                            for tile in tiles {
                                event_sender
                                    .unbounded_send(TileRenderEvent::ReadyBatch {
                                        tiles: vec![tile],
                                    })
                                    .map_err(|_| "地图瓦片显示事件接收端已关闭".to_string())?;
                                std::thread::sleep(TILE_PRESENT_INTERVAL);
                            }
                        }
                        event => {
                            event_sender
                                .unbounded_send(event)
                                .map_err(|_| "地图瓦片显示事件接收端已关闭".to_string())?;
                        }
                    }
                }
                Ok::<(), String>(())
            })
        })
        .map_err(|error| format!("创建地图瓦片显示线程失败: {error}"))?;

    let render_result = render_tile_batch_stream_impl(request, batch_sender);
    let forward_result = forward_thread
        .join()
        .map_err(|_| "地图瓦片显示线程崩溃".to_string())?;

    render_result?;
    forward_result
}
