from pathlib import Path

helpers = Path("src/ui/window/map_viewer/helpers.rs")
lifecycle = Path("src/ui/window/map_viewer/lifecycle.rs")

helpers_text = helpers.read_text(encoding="utf-8")
old_helper = '''pub(super) fn spawn_block_center(world_path: &std::path::Path) -> Option<(i32, i32)> {
    let document = bedrock_world::read_level_dat_document(&world_path.join("level.dat")).ok()?;
    let root = match &document.root {
        NbtTag::Compound(root) => root,
        _ => return None,
    };
    let spawn_x = nbt_i32(root.get("SpawnX")?)?;
    let spawn_z = nbt_i32(root.get("SpawnZ")?)?;
    Some((spawn_x, spawn_z))
}
'''
new_helper = '''pub(super) fn spawn_block_center(world_path: &std::path::Path) -> Option<(i32, i32)> {
    let level_dat_path = if world_path
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("db"))
    {
        world_path.parent()?.join("level.dat")
    } else {
        world_path.join("level.dat")
    };

    let document = match bedrock_world::read_level_dat_document(&level_dat_path) {
        Ok(document) => document,
        Err(error) => {
            tracing::warn!(
                world = %world_path.display(),
                level_dat = %level_dat_path.display(),
                %error,
                "map_viewer failed_to_read_spawn_from_level_dat"
            );
            return None;
        }
    };
    let root = match &document.root {
        NbtTag::Compound(root) => root,
        other => {
            tracing::warn!(
                level_dat = %level_dat_path.display(),
                root_type = ?other,
                "map_viewer level_dat_root_is_not_compound"
            );
            return None;
        }
    };
    let spawn_root = match root.get("Data") {
        Some(NbtTag::Compound(data)) => data,
        _ => root,
    };
    let spawn_x = spawn_root.get("SpawnX").and_then(nbt_i32);
    let spawn_z = spawn_root.get("SpawnZ").and_then(nbt_i32);
    match (spawn_x, spawn_z) {
        (Some(spawn_x), Some(spawn_z)) => {
            tracing::info!(
                world = %world_path.display(),
                level_dat = %level_dat_path.display(),
                spawn_x,
                spawn_z,
                "map_viewer level_dat_spawn_loaded"
            );
            Some((spawn_x, spawn_z))
        }
        _ => {
            tracing::warn!(
                world = %world_path.display(),
                level_dat = %level_dat_path.display(),
                has_spawn_x = spawn_x.is_some(),
                has_spawn_z = spawn_z.is_some(),
                "map_viewer level_dat_spawn_fields_missing"
            );
            None
        }
    }
}
'''
if old_helper not in helpers_text:
    raise SystemExit("spawn_block_center implementation not found")
helpers.write_text(helpers_text.replace(old_helper, new_helper, 1), encoding="utf-8")

lifecycle_text = lifecycle.read_text(encoding="utf-8")
old_init = '''        let mut viewport = MapViewport::new(window_size);
        if let Some((spawn_x, spawn_z)) = spawn_block_center(&world_path) {
            viewport.center_on_block(spawn_x, spawn_z, web_relief_render_layout());
        }
'''
new_init = '''        let mut viewport = MapViewport::new(window_size);
        let spawn_center = spawn_block_center(&world_path);
        if let Some((spawn_x, spawn_z)) = spawn_center {
            viewport.center_on_block(spawn_x, spawn_z, web_relief_render_layout());
        } else {
            tracing::warn!(
                world = %world_path.display(),
                "map_viewer spawn_unavailable_using_occupancy_fallback"
            );
        }
'''
if old_init not in lifecycle_text:
    raise SystemExit("map viewer initial viewport block not found")
lifecycle_text = lifecycle_text.replace(old_init, new_init, 1)
old_flag = "            recenter_on_next_metadata: true,\n"
new_flag = "            recenter_on_next_metadata: spawn_center.is_none(),\n"
if old_flag not in lifecycle_text:
    raise SystemExit("recenter_on_next_metadata initializer not found")
lifecycle.write_text(lifecycle_text.replace(old_flag, new_flag, 1), encoding="utf-8")
