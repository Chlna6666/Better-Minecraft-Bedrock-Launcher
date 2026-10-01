//! Portable map-record bundle export from a completed image preview.

use std::path::PathBuf;

use bedrock_world::{
    item::set_item_display,
    map_item::{MapBundle, SavedData, new_filled_map_item},
    nbt::NbtTag,
    surface::CancelFlag,
};
use indexmap::IndexMap;

use super::map_image::{MapPreviewResult, get_map_preview};
use crate::tasks::{runtime, task_manager};

/// Exports exact preview tiles and labeled filled-map references as a BMCBL map bundle.
///
/// The resulting NBT file contains `map_<id>` record bytes and an item list. It is an
/// application envelope for later ID remapping, not a directly placeable Minecraft chest.
/// This writes only the selected file and does not modify any world or player.
///
/// # Errors
/// Returns an expired preview or runtime submission error. Encoding and file errors
/// are reported on the returned visible task.
pub fn start_map_bundle_export(
    preview_task_id: &str,
    output: PathBuf,
    title: String,
) -> Result<String, String> {
    let preview = get_map_preview(preview_task_id)
        .ok_or_else(|| "地图预览已释放或过期，请重新转换".to_owned())?;
    let task_id = task_manager::create_task_with_details(
        None,
        "导出可移植地图包",
        output
            .file_name()
            .map(|name| name.to_string_lossy().into_owned()),
        "生成地图记录",
        Some(preview.tiles.len() as u64),
        false,
    );
    task_manager::register_task_cooperative_cancel(task_id.clone());
    let cancel = CancelFlag::new();
    task_manager::register_task_cancel_hook(task_id.clone(), {
        let cancel = cancel.clone();
        move || cancel.cancel()
    });
    let worker_id = task_id.clone();
    if let Err(error) = runtime::spawn_io(async move {
        let result = export(preview, output, title, &worker_id, &cancel).await;
        let status = match &result {
            Ok(_) => "completed",
            Err(message) if message.contains("已取消") => "cancelled",
            Err(_) => "error",
        };
        task_manager::finish_task(
            &worker_id,
            status,
            Some(match result {
                Ok(message) | Err(message) => message,
            }),
        );
    }) {
        task_manager::finish_task(&task_id, "error", Some(error.clone()));
        return Err(error);
    }
    Ok(task_id)
}

async fn export(
    preview: std::sync::Arc<MapPreviewResult>,
    output: PathBuf,
    title: String,
    task_id: &str,
    cancel: &CancelFlag,
) -> Result<String, String> {
    let worker_id = task_id.to_owned();
    let worker_cancel = cancel.clone();
    let bytes = runtime::run_cpu(move || {
        let total = preview.tiles.len() as u64;
        let bundle = build_bundle(&preview, &title, &worker_cancel, |_done| {
            task_manager::update_progress(&worker_id, 1, Some(total), Some("生成地图记录"));
        })?;
        bundle.to_nbt_bytes().map_err(|error| error.to_string())
    })
    .await??;
    if cancel.is_cancelled() {
        return Err("地图包导出已取消".to_owned());
    }
    task_manager::reset_progress(task_id, Some(1), Some("写入地图包"));
    let written = output.clone();
    runtime::run_io_blocking(move || std::fs::write(&written, bytes))
        .await?
        .map_err(|error| error.to_string())?;
    task_manager::update_progress(task_id, 1, Some(1), Some("写入地图包"));
    Ok(format!("已导出可移植地图包：{}", output.display()))
}

pub(super) fn build_bundle(
    preview: &MapPreviewResult,
    title: &str,
    cancel: &CancelFlag,
    mut progress: impl FnMut(usize),
) -> Result<MapBundle, String> {
    let mut records = Vec::with_capacity(preview.tiles.len());
    let mut items = Vec::with_capacity(preview.tiles.len());
    for (index, tile) in preview.tiles.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err("地图包导出已取消".to_owned());
        }
        let id = index as i64;
        records.push(
            SavedData::new_locked_pixels(id, tile.pixels.clone(), 0, 0, 0, 0)
                .map_err(|error| error.to_string())?,
        );
        // Slot numbers belong to an eventual chest/shulker target, not this portable list.
        let mut item = new_filled_map_item(id, 0).map_err(|error| error.to_string())?;
        let label = format!("{title} · ({},{})", tile.column + 1, tile.row + 1);
        let lore = vec![format!(
            "BMCBL · 北向上 · 列 {}/{} · 行 {}/{}",
            tile.column + 1,
            preview.columns,
            tile.row + 1,
            preview.rows
        )];
        set_item_display(&mut item, &label, &lore).map_err(|error| error.to_string())?;
        items.push(item);
        progress(index + 1);
    }
    let container = NbtTag::Compound(IndexMap::from([
        ("Name".to_owned(), NbtTag::String(title.to_owned())),
        ("Items".to_owned(), NbtTag::List(items)),
    ]));
    MapBundle::new(records, container)
        .and_then(|bundle| bundle.with_grid(preview.columns, preview.rows))
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::minecraft::map::image::{MapImageCrop, MapImageTile};
    use bedrock_world::map_item::{Pixels, filled_map_id};

    #[test]
    fn two_tiles_export_exact_pixels_and_coordinate_labels() {
        let preview = MapPreviewResult {
            source_width: 256,
            source_height: 128,
            crop: MapImageCrop {
                x: 0,
                y: 0,
                width: 256,
                height: 128,
            },
            columns: 2,
            rows: 1,
            preview_width: 257,
            preview_height: 128,
            preview_rgba: Vec::new(),
            tiles: (0..2)
                .map(|column| MapImageTile {
                    column,
                    row: 0,
                    pixels: Pixels {
                        width: 128,
                        height: 128,
                        rgba: vec![column as u8; 128 * 128 * 4],
                    },
                })
                .collect(),
        };
        let bundle = build_bundle(&preview, "demo", &CancelFlag::new(), |_| {}).expect("bundle");
        let encoded = bundle.to_nbt_bytes().expect("encode");
        let decoded = MapBundle::from_nbt_bytes(&encoded).expect("decode");
        assert_eq!(
            decoded.records()[1].pixels.as_ref().expect("pixels").rgba[0],
            1
        );
        let NbtTag::Compound(root) = decoded.container() else {
            panic!("root")
        };
        let Some(NbtTag::List(items)) = root.get("Items") else {
            panic!("items")
        };
        assert_eq!(filled_map_id(&items[0]).expect("map id").as_str(), "0");
        assert_eq!(filled_map_id(&items[1]).expect("map id").as_str(), "1");
        let NbtTag::Compound(item) = &items[1] else {
            panic!("item")
        };
        let Some(NbtTag::Compound(tag)) = item.get("tag") else {
            panic!("tag")
        };
        let Some(NbtTag::Compound(display)) = tag.get("display") else {
            panic!("display")
        };
        assert_eq!(
            display.get("Name"),
            Some(&NbtTag::String("demo · (2,1)".to_owned()))
        );
    }

    #[test]
    fn cancelled_bundle_builds_no_record() {
        let preview = MapPreviewResult {
            source_width: 128,
            source_height: 128,
            crop: MapImageCrop {
                x: 0,
                y: 0,
                width: 128,
                height: 128,
            },
            columns: 1,
            rows: 1,
            preview_width: 128,
            preview_height: 128,
            preview_rgba: Vec::new(),
            tiles: vec![MapImageTile {
                column: 0,
                row: 0,
                pixels: Pixels {
                    width: 128,
                    height: 128,
                    rgba: vec![0; 128 * 128 * 4],
                },
            }],
        };
        let cancel = CancelFlag::new();
        cancel.cancel();
        assert!(build_bundle(&preview, "demo", &cancel, |_| {}).is_err());
    }
}
