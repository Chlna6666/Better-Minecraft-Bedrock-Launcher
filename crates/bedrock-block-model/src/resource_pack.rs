use std::path::{Path, PathBuf};

use serde_json::Value;

/// Finds vanilla resource-pack roots under one installed game package.
#[must_use]
pub fn vanilla_roots(package_path: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    append_unique_path(&mut roots, package_path.to_path_buf());
    if path_file_name_eq(package_path, "textures")
        && let Some(parent) = package_path.parent()
    {
        append_unique_path(&mut roots, parent.to_path_buf());
    }
    append_vanilla_roots(
        &mut roots,
        &package_path.join("data").join("resource_packs"),
    );
    append_vanilla_roots(&mut roots, &package_path.join("data").join("resourcepacks"));
    roots
}

/// Finds vanilla resource-pack roots across installed game packages.
#[must_use]
pub fn vanilla_roots_for_packages<I, P>(package_roots: I) -> Vec<PathBuf>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut roots = Vec::new();
    for package_root in package_roots {
        for root in vanilla_roots(package_root.as_ref()) {
            append_unique_path(&mut roots, root);
        }
    }
    roots
}

/// Resolves the enabled resource-pack directories recorded by a world.
#[must_use]
pub fn paths_for_world(world_path: &Path) -> Vec<PathBuf> {
    let pack_ids = ids_for_world(world_path);
    if pack_ids.is_empty() {
        return Vec::new();
    }

    let roots = roots_for_world(world_path);
    let mut package_paths = Vec::new();
    for pack_id in pack_ids {
        for root in &roots {
            if let Some(package_path) = find_by_id(root, &pack_id) {
                append_unique_path(&mut package_paths, package_path);
                break;
            }
        }
    }
    package_paths
}

/// Reads the normalized resource-pack IDs enabled by a world.
#[must_use]
pub fn ids_for_world(world_path: &Path) -> Vec<String> {
    let Some(value) = read_optional_json(&world_path.join("world_resource_packs.json")) else {
        return Vec::new();
    };
    let Some(items) = value.as_array() else {
        return Vec::new();
    };

    let mut pack_ids = Vec::new();
    for item in items {
        let Some(pack_id) = item
            .get("pack_id")
            .and_then(Value::as_str)
            .or_else(|| item.get("uuid").and_then(Value::as_str))
        else {
            continue;
        };
        let normalized = normalize_uuid(pack_id);
        if !normalized.is_empty() && !pack_ids.iter().any(|existing| existing == &normalized) {
            pack_ids.push(normalized);
        }
    }
    pack_ids
}

/// Finds resource-pack search roots for a world and its Bedrock installation.
#[must_use]
pub fn roots_for_world(world_path: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    append_unique_path(&mut roots, world_path.join("resource_packs"));
    for ancestor in world_path.ancestors() {
        if !path_file_name_eq(ancestor, "minecraftWorlds") {
            continue;
        }
        let Some(com_mojang_dir) = ancestor.parent() else {
            continue;
        };
        if !path_file_name_eq(com_mojang_dir, "com.mojang") {
            continue;
        }
        append_unique_path(&mut roots, com_mojang_dir.join("resource_packs"));

        let Some(games_dir) = com_mojang_dir.parent() else {
            continue;
        };
        if !path_file_name_eq(games_dir, "games") {
            continue;
        }
        let Some(user_dir) = games_dir.parent() else {
            continue;
        };
        let Some(users_dir) = user_dir
            .parent()
            .filter(|path| path_file_name_eq(path, "Users"))
        else {
            continue;
        };
        append_unique_path(
            &mut roots,
            users_dir
                .join("Shared")
                .join("games")
                .join("com.mojang")
                .join("resource_packs"),
        );
    }
    roots
}

/// Finds a resource-pack directory by its manifest UUID or conventional directory name.
#[must_use]
pub fn find_by_id(root: &Path, pack_id: &str) -> Option<PathBuf> {
    let pack_id = normalize_uuid(pack_id);
    if pack_id.is_empty() || !root.is_dir() {
        return None;
    }

    for candidate_name in [
        format!("pack_{pack_id}"),
        pack_id.clone(),
        format!("pack_{}", pack_id.to_ascii_lowercase()),
        pack_id.to_ascii_lowercase(),
    ] {
        let candidate = root.join(candidate_name);
        if candidate_matches(&candidate, &pack_id, true) {
            return Some(candidate);
        }
    }

    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if candidate_matches(&path, &pack_id, false) {
            return Some(path);
        }
    }
    None
}

/// Reads and normalizes the resource-pack UUID from a manifest.
#[must_use]
pub fn manifest_uuid(path: &Path) -> Option<String> {
    let value = read_optional_json(&path.join("manifest.json"))?;
    value
        .get("header")
        .and_then(|header| header.get("uuid"))
        .and_then(Value::as_str)
        .map(normalize_uuid)
}

/// Removes braces and normalizes a resource-pack UUID for comparisons.
#[must_use]
pub fn normalize_uuid(value: &str) -> String {
    value
        .trim()
        .trim_matches('{')
        .trim_matches('}')
        .to_ascii_lowercase()
}

/// Adds a nonempty path once, preserving insertion order.
pub fn append_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if path.as_os_str().is_empty() {
        return;
    }
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn append_vanilla_roots(roots: &mut Vec<PathBuf>, resource_packs_dir: &Path) {
    // Keep overlay roots before the base pack so the base pack wins when the resolver
    // searches in reverse order, while overlays can still supply missing files.
    let vanilla = resource_packs_dir.join("vanilla");

    let Ok(entries) = std::fs::read_dir(resource_packs_dir) else {
        append_unique_path(roots, vanilla.join("client"));
        append_unique_path(roots, vanilla);
        return;
    };
    let mut overlays = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| overlay_version(name).is_some())
        })
        .collect::<Vec<_>>();
    overlays.sort_by(|left, right| {
        let left_version = left
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(overlay_version);
        let right_version = right
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(overlay_version);
        left_version
            .cmp(&right_version)
            .then_with(|| left.cmp(right))
    });

    let ignore_overlays = std::env::var("BMCBL_VANILLA_OVERLAYS")
        .is_ok_and(|value| matches!(value.as_str(), "ignore" | "none" | "0" | "false"));
    if !ignore_overlays {
        for root in overlays {
            append_unique_path(roots, root.join("client"));
            append_unique_path(roots, root);
        }
    }

    append_unique_path(roots, vanilla.join("client"));
    append_unique_path(roots, vanilla);
}

fn candidate_matches(path: &Path, pack_id: &str, allow_name_fallback: bool) -> bool {
    if !path.join("textures").is_dir()
        && !path.join("__brarchive/textures.brarchive").is_file()
        && !path.join("__brarchive/textures").is_dir()
    {
        return false;
    }
    if let Some(manifest_uuid) = manifest_uuid(path) {
        return manifest_uuid == pack_id;
    }
    allow_name_fallback
}

fn path_file_name_eq(path: &Path, expected: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(expected))
}

fn overlay_version(name: &str) -> Option<Vec<u32>> {
    let version = name.strip_prefix("vanilla_").or_else(|| {
        name.get(..8)
            .filter(|prefix| prefix.eq_ignore_ascii_case("vanilla_"))
            .and_then(|_| name.get(8..))
    })?;
    let mut parts = Vec::new();
    for part in version.split('.') {
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        parts.push(part.parse().ok()?);
    }
    (!parts.is_empty()).then_some(parts)
}

fn read_optional_json(path: &Path) -> Option<Value> {
    crate::json::read_json_file(path).ok()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use super::{ids_for_world, paths_for_world, vanilla_roots};

    struct TestPack(TempDir);

    impl TestPack {
        fn new() -> Self {
            Self(tempfile::tempdir().expect("create temporary pack directory"))
        }

        fn path(&self) -> &Path {
            self.0.path()
        }

        fn write(&self, relative_path: &str, contents: &str) {
            self.write_bytes(relative_path, contents.as_bytes());
        }

        fn write_bytes(&self, relative_path: &str, contents: &[u8]) {
            let path = self.path().join(relative_path);
            fs::create_dir_all(path.parent().expect("parent directory"))
                .expect("create parent directory");
            fs::write(path, contents).expect("write test file");
        }
    }

    #[test]
    fn vanilla_roots_include_base_and_sorted_overlays() {
        let pack = TestPack::new();
        pack.write_bytes("data/resource_packs/vanilla/textures/.keep", b"");
        pack.write_bytes("data/resource_packs/vanilla_1.21.40/textures/.keep", b"");
        pack.write_bytes("data/resource_packs/vanilla_1.20.0/textures/.keep", b"");

        let roots = vanilla_roots(pack.path());

        let expected = vec![
            pack.path().to_path_buf(),
            pack.path()
                .join("data")
                .join("resource_packs")
                .join("vanilla_1.20.0")
                .join("client"),
            pack.path()
                .join("data")
                .join("resource_packs")
                .join("vanilla_1.20.0"),
            pack.path()
                .join("data")
                .join("resource_packs")
                .join("vanilla_1.21.40")
                .join("client"),
            pack.path()
                .join("data")
                .join("resource_packs")
                .join("vanilla_1.21.40"),
            pack.path()
                .join("data")
                .join("resource_packs")
                .join("vanilla")
                .join("client"),
            pack.path()
                .join("data")
                .join("resource_packs")
                .join("vanilla"),
            pack.path()
                .join("data")
                .join("resourcepacks")
                .join("vanilla")
                .join("client"),
            pack.path()
                .join("data")
                .join("resourcepacks")
                .join("vanilla"),
        ];

        assert_eq!(roots, expected);
    }

    #[test]
    fn paths_for_world_follow_manifest_pack_order() {
        let root = TestPack::new();
        let world_path = root
            .path()
            .join("Users")
            .join("player")
            .join("games")
            .join("com.mojang")
            .join("minecraftWorlds")
            .join("world");
        let local_pack = world_path.join("resource_packs").join("pack-local");
        let global_pack = root
            .path()
            .join("Users")
            .join("player")
            .join("games")
            .join("com.mojang")
            .join("resource_packs")
            .join("global-pack");
        let shared_pack = root
            .path()
            .join("Users")
            .join("Shared")
            .join("games")
            .join("com.mojang")
            .join("resource_packs")
            .join("shared-pack");
        for pack_path in [&local_pack, &global_pack, &shared_pack] {
            fs::create_dir_all(pack_path.join("textures")).expect("create pack textures");
        }
        root.write(
            "Users/player/games/com.mojang/minecraftWorlds/world/resource_packs/pack-local/manifest.json",
            r#"{"header":{"uuid":"{11111111-1111-1111-1111-111111111111}"}}"#,
        );
        root.write(
            "Users/player/games/com.mojang/resource_packs/global-pack/manifest.json",
            r#"{"header":{"uuid":"22222222-2222-2222-2222-222222222222"}}"#,
        );
        root.write(
            "Users/Shared/games/com.mojang/resource_packs/shared-pack/manifest.json",
            r#"{"header":{"uuid":"33333333-3333-3333-3333-333333333333"}}"#,
        );
        root.write(
            "Users/player/games/com.mojang/minecraftWorlds/world/world_resource_packs.json",
            r#"[
                // comments and trailing commas are accepted
                {"pack_id":"22222222-2222-2222-2222-222222222222"},
                {"pack_id":"11111111-1111-1111-1111-111111111111"},
                {"uuid":"33333333-3333-3333-3333-333333333333"},
            ]"#,
        );

        assert_eq!(
            paths_for_world(&world_path),
            vec![global_pack, local_pack, shared_pack]
        );
    }

    #[test]
    fn ids_for_world_normalize_and_deduplicate() {
        let root = TestPack::new();
        let world_path = root.path().join("world");
        root.write(
            "world/world_resource_packs.json",
            r#"[
                {"pack_id":"{AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA}"},
                {"uuid":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"},
                {"pack_id":"BBBBBBBB-BBBB-BBBB-BBBB-BBBBBBBBBBBB"},
            ]"#,
        );

        assert_eq!(
            ids_for_world(&world_path),
            vec![
                "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa".to_owned(),
                "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb".to_owned(),
            ]
        );
    }
}
