//! Conservative persistent Section-occupancy directory for the 3D editor.
//!
//! This is NOT a mesh cache. It stores only palette-certified 16³ solid masks
//! and the complete set of decoded section heights. The portal graph is rebuilt
//! in memory, and every unknown/malformed/stale entry fails open.
//!
//! Inspired by Sodium's independent Section visibility graph and map renderers'
//! source-revision invalidation. Bedrock has no per-column stable revision token:
//! an external DB change therefore invalidates this *entire* snapshot, whereas
//! editor writes can invalidate only their explicitly affected columns.

use super::super::types::World3dChunkProbe;
use super::super::visibility::SectionVisibility;
use bedrock_render::{ChunkPos, Dimension};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

const MAGIC: &[u8; 8] = b"BMCBSIDX";
const FORMAT: u32 = 1;
const MAX_COLUMNS: usize = 16_384;
const MAX_SECTIONS: usize = 48;
const MAX_FILE_SIZE: u64 = 128 * 1024 * 1024;
const FLUSH_INTERVAL: Duration = Duration::from_secs(8);
const VIEW_REVISION_CHECK_INTERVAL: Duration = Duration::from_secs(1);
const OFFSET: u64 = 0xcbf29ce484222325;
const PRIME: u64 = 0x100000001b3;

fn mix_bytes(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash = (hash ^ u64::from(byte)).wrapping_mul(PRIME);
    }
    hash
}

fn stable_path_hash(path: &Path) -> u64 {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut value = canonical.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        value.make_ascii_lowercase();
    }
    mix_bytes(OFFSET, value.as_bytes())
}

/// A closed-world fingerprint uses every LevelDB data/log/manifest file's
/// metadata, plus the Bedrock world-version header. No LevelDB values or
/// SubChunks are read. Failed metadata queries disable index-based skipping.
/// A live external writer can race filesystem metadata checks; the worker
/// also invalidates explicitly edited columns and rechecks before DB elision.
fn source_revision(world: &Path) -> Option<u64> {
    let mut hash = OFFSET;
    let level = world.join("level.dat");
    let db = world.join("db");
    let mut files = vec![level];
    let mut has_current = false;
    for file in fs::read_dir(db).ok()? {
        let path = file.ok()?.path();
        let name = path.file_name()?.to_string_lossy();
        let relevant = name == "CURRENT"
            || name.starts_with("MANIFEST-")
            || name == "IDENTITY"
            || name.starts_with("OPTIONS-")
            || matches!(path.extension().and_then(|ext| ext.to_str()), Some("ldb" | "sst"))
            || (name.chars().take_while(|ch| ch.is_ascii_digit()).count() > 0
                && name.ends_with(".log"));
        if relevant {
            has_current |= name == "CURRENT";
            files.push(path);
            if files.len() > 100_000 {
                return None;
            }
        }
    }
    if !has_current {
        return None;
    }
    files.sort_unstable();
    for path in files {
        let metadata = fs::metadata(&path).ok()?;
        if !metadata.is_file() {
            return None;
        }
        let changed = metadata.modified().ok()?
            .duration_since(UNIX_EPOCH).ok()?.as_nanos();
        hash = mix_bytes(hash, path.file_name()?.to_string_lossy().as_bytes());
        hash = mix_bytes(hash, &metadata.len().to_le_bytes());
        hash = mix_bytes(hash, &changed.to_le_bytes());
    }
    Some(hash)
}

struct IndexedColumn {
    masks: BTreeMap<i8, [u64; 64]>,
    // A persistent index may contain thousands of SubChunks but the camera
    // only needs portal graphs for the current view. Do not flood-fill every
    // cached mask at startup or after each newly decoded DB column.
    probe: OnceLock<Arc<World3dChunkProbe>>,
}

impl IndexedColumn {
    fn new(masks: BTreeMap<i8, [u64; 64]>) -> Self {
        Self { masks, probe: OnceLock::new() }
    }

    fn probe(&self, pos: ChunkPos) -> Arc<World3dChunkProbe> {
        Arc::clone(self.probe.get_or_init(|| column_probe(pos, &self.masks)))
    }
}

fn column_probe(pos: ChunkPos, masks: &BTreeMap<i8, [u64; 64]>) -> Arc<World3dChunkProbe> {
    let visibility = masks.iter().filter(|(_, solid)| solid.iter().any(|&s| s != 0))
        .map(|(&y, solid)| {
            (y, Arc::new(SectionVisibility::from_solid_bits(*solid)))
        }).collect();
    Arc::new(World3dChunkProbe {
        pos,
        min_y: 0,
        subchunks: BTreeMap::new(),
        visibility,
        state_version: None,
        storage_bytes: 0,
    })
}

fn take<const N: usize>(src: &mut &[u8]) -> Option<[u8; N]> {
    let (bytes, rest) = src.split_at_checked(N)?;
    *src = rest;
    bytes.try_into().ok()
}

fn parse(data: &[u8], expected_revision: u64) -> Option<BTreeMap<ChunkPos, IndexedColumn>> {
    let mut input = data;
    if &take::<8>(&mut input)? != MAGIC {
        return None;
    }
    if u32::from_le_bytes(take(&mut input)?) != FORMAT {
        return None;
    }
    if u64::from_le_bytes(take(&mut input)?) != expected_revision {
        return None;
    }
    let count = u32::from_le_bytes(take(&mut input)?) as usize;
    let expected_checksum = u64::from_le_bytes(take(&mut input)?);
    if count > MAX_COLUMNS || mix_bytes(OFFSET, input) != expected_checksum {
        return None;
    }
    let mut columns = BTreeMap::new();
    for _ in 0..count {
        let x = i32::from_le_bytes(take(&mut input)?);
        let z = i32::from_le_bytes(take(&mut input)?);
        let dimension = Dimension::from_id(i32::from_le_bytes(take(&mut input)?));
        let pos = ChunkPos { x, z, dimension };
        let count = usize::from(take::<1>(&mut input)?[0]);
        if count > MAX_SECTIONS || count == 0 || columns.contains_key(&pos) {
            return None;
        }
        let mut masks = BTreeMap::new();
        for _ in 0..count {
            let y = take::<1>(&mut input)?[0] as i8;
            let tag = take::<1>(&mut input)?[0];
            let solid = match tag {
                0 => [0; 64],
                1 => [u64::MAX; 64],
                2 => {
                    let mut words = [0; 64];
                    for word in &mut words {
                        *word = u64::from_le_bytes(take(&mut input)?);
                    }
                    words
                }
                _ => return None,
            };
            if masks.insert(y, solid).is_some() {
                return None;
            }
        }
        columns.insert(pos, IndexedColumn::new(masks));
    }
    input.is_empty().then_some(columns)
}

fn encode(entries: &BTreeMap<ChunkPos, IndexedColumn>, revision: u64) -> Vec<u8> {
    let mut body = Vec::new();
    for (&pos, entry) in entries {
        body.extend_from_slice(&pos.x.to_le_bytes());
        body.extend_from_slice(&pos.z.to_le_bytes());
        body.extend_from_slice(&pos.dimension.id().to_le_bytes());
        body.push(entry.masks.len() as u8);
        for (&y, solid) in &entry.masks {
            body.push(y as u8);
            if solid.iter().all(|&word| word == 0) {
                body.push(0);
            } else if solid.iter().all(|&word| word == u64::MAX) {
                body.push(1);
            } else {
                body.push(2);
                for word in solid {
                    body.extend_from_slice(&word.to_le_bytes());
                }
            }
        }
    }
    let mut output = Vec::with_capacity(32 + body.len());
    output.extend_from_slice(MAGIC);
    output.extend_from_slice(&FORMAT.to_le_bytes());
    output.extend_from_slice(&revision.to_le_bytes());
    output.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    output.extend_from_slice(&mix_bytes(OFFSET, &body).to_le_bytes());
    output.extend_from_slice(&body);
    output
}

fn save_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file); // Release the handle before rename, especially on Windows.
        // Windows cannot rename over an existing file. An absent cache during
        // replacement simply falls back to normal world decoding on restart.
        if cfg!(windows) && path.exists() {
            fs::remove_file(path)?;
        }
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// World-local metadata only. Neither the UI nor the GPU owner stores the
/// persistent index; the existing streaming worker owns its lifecycle.
pub(super) struct VisibilityIndex {
    world: PathBuf,
    file: PathBuf,
    revision: Option<u64>,
    entries: BTreeMap<ChunkPos, IndexedColumn>,
    epoch: u64,
    dirty: bool,
    last_flush: Instant,
    last_checked: Instant,
}

impl VisibilityIndex {
    pub(super) fn open(world: &Path) -> Self {
        let file = crate::utils::file_ops::cache_subdir("world3d/section_occlusion")
            .join(format!("{:016x}.idx", stable_path_hash(world)));
        let revision = source_revision(world);
        let entries = revision.and_then(|stamp| {
            let metadata = fs::metadata(&file).ok()?;
            if metadata.len() > MAX_FILE_SIZE {
                return None;
            }
            parse(&fs::read(&file).ok()?, stamp)
        }).unwrap_or_default();
        Self {
            world: world.to_path_buf(), file, revision, entries,
            epoch: 1, dirty: false, last_flush: Instant::now(),
            last_checked: Instant::now(),
        }
    }

    /// Re-scan LevelDB file *metadata*, not block values. Missing or changed
    /// files invalidate every old persistent proof. The caller must then
    /// reschedule any columns skipped using the previous index.
    pub(super) fn refresh_if_due(&mut self) -> bool {
        if self.last_checked.elapsed() < VIEW_REVISION_CHECK_INTERVAL {
            return false;
        }
        self.refresh()
    }

    pub(super) fn refresh(&mut self) -> bool {
        self.last_checked = Instant::now();
        let new_revision = source_revision(&self.world);
        if new_revision == self.revision {
            return false;
        }
        self.entries.clear();
        self.revision = new_revision;
        self.dirty = false;
        self.epoch = self.epoch.wrapping_add(1);
        true
    }

    pub(super) fn enabled(&self) -> bool { self.revision.is_some() }
    pub(super) fn epoch(&self) -> u64 { self.epoch }
    pub(super) fn len(&self) -> usize { self.entries.len() }

    /// Filter by the active circular view *before* reconstructing the
    /// expensive 16³ portal graphs from their persistent compact masks.
    /// Off-screen index entries remain only a few raw words on disk/in RAM.
    pub(super) fn probes_in_view(
        &self, dimension: Dimension, center_x: i32, center_z: i32, radius: i32,
    ) -> impl Iterator<Item = (ChunkPos, Arc<World3dChunkProbe>)> + '_ {
        self.entries.iter()
            .filter(move |(&pos, _)| {
                super::inside_view(pos, dimension, center_x, center_z, radius)
            })
            .map(|(&pos, value)| (pos, value.probe(pos)))
    }

    pub(super) fn candidate_bands(&self) -> impl Iterator<Item = (ChunkPos, i32)> + '_ {
        self.entries.iter().flat_map(|(&pos, value)| {
            value.masks.keys().map(move |&y| (pos, i32::from(y).div_euclid(4)))
        })
    }

    pub(super) fn upsert(&mut self, pos: ChunkPos, masks: BTreeMap<i8, [u64; 64]>) {
        if self.revision.is_none() || masks.is_empty() || masks.len() > MAX_SECTIONS {
            return;
        }
        if self.entries.get(&pos).is_some_and(|old| old.masks == masks) {
            return;
        }
        // Keep cache size bounded even if the editor crosses very large worlds.
        if self.entries.len() == MAX_COLUMNS && !self.entries.contains_key(&pos) {
            self.entries.pop_first();
        }
        self.entries.insert(pos, IndexedColumn::new(masks));
        self.epoch = self.epoch.wrapping_add(1);
        self.dirty = true;
    }

    pub(super) fn invalidate(&mut self, pos: ChunkPos) {
        if self.entries.remove(&pos).is_some() {
            self.epoch = self.epoch.wrapping_add(1);
            self.dirty = true;
        }
    }

    pub(super) fn flush_if_due(&mut self) {
        if self.dirty && self.last_flush.elapsed() >= FLUSH_INTERVAL {
            self.flush();
        }
    }

    pub(super) fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        // Never persist a snapshot from an older LevelDB revision.
        if self.refresh() {
            return;
        }
        let Some(revision) = self.revision else { return };
        let bytes = encode(&self.entries, revision);
        if bytes.len() as u64 <= MAX_FILE_SIZE && save_atomically(&self.file, &bytes).is_ok() {
            self.dirty = false;
            self.last_flush = Instant::now();
        }
    }
}

impl Drop for VisibilityIndex {
    fn drop(&mut self) {
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(x: i32) -> ChunkPos {
        ChunkPos { x, z: -9, dimension: Dimension::Overworld }
    }

    fn fixture() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "bmcbl-occlusion-index-test-{}-{}",
            std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(path.join("db")).unwrap();
        fs::write(path.join("level.dat"), b"world-version-1").unwrap();
        fs::write(path.join("db/CURRENT"), b"MANIFEST-000001\n").unwrap();
        fs::write(path.join("db/MANIFEST-000001"), b"manifest").unwrap();
        path
    }

    #[test]
    fn source_revision_change_fails_open_without_accessing_subchunk_values() {
        let world = fixture();
        let old = source_revision(&world).unwrap();
        fs::write(world.join("db/000001.ldb"), b"some-new-chunk-data").unwrap();
        assert_ne!(old, source_revision(&world).unwrap());
        fs::remove_file(world.join("db/CURRENT")).unwrap();
        assert_eq!(source_revision(&world), None);
        fs::remove_dir_all(world).unwrap();
    }

    #[test]
    fn edit_invalidation_only_discards_affected_column_in_stable_revision() {
        let world = fixture();
        let path = world.join("index.bin");
        let revision = source_revision(&world);
        let mut index = VisibilityIndex {
            world: world.clone(), file: path.clone(), revision,
            entries: BTreeMap::new(), epoch: 1, dirty: false,
            last_flush: Instant::now(),
            last_checked: Instant::now(),
        };
        index.upsert(pos(1), BTreeMap::from([(0, [u64::MAX; 64])]));
        index.upsert(pos(2), BTreeMap::from([(0, [0; 64])]));
        index.flush();
        assert_eq!(index.len(), 2);
        index.invalidate(pos(1));
        index.flush();
        let entries = parse(&fs::read(&path).unwrap(), revision.unwrap()).unwrap();
        assert!(!entries.contains_key(&pos(1)));
        assert!(entries.contains_key(&pos(2)));
        // A foreign writer invalidates the complete previous certificate:
        // no stale wall may be used to skip a future DB lookup.
        fs::write(world.join("db/000002.log"), b"external edit").unwrap();
        assert!(index.refresh());
        assert_eq!(index.len(), 0);
        assert!(parse(&fs::read(&path).unwrap(), source_revision(&world).unwrap()).is_none());
        drop(index);
        fs::remove_dir_all(world).unwrap();
    }

    #[test]
    fn sparse_masks_roundtrip_without_treating_unknown_as_solid() {
        let mut masks = BTreeMap::new();
        masks.insert(-4, [u64::MAX; 64]);
        masks.insert(-3, [0; 64]);
        let mut partial = [0u64; 64];
        partial[0] = 1;
        masks.insert(-2, partial);
        let mut entries = BTreeMap::new();
        entries.insert(pos(5), IndexedColumn::new(masks.clone()));
        let bytes = encode(&entries, 123);
        let decoded = parse(&bytes, 123).unwrap();
        assert_eq!(decoded.get(&pos(5)).unwrap().masks, masks);
        assert!(parse(&bytes, 124).is_none());
        let mut corrupt = bytes.clone();
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(parse(&corrupt, 123).is_none());
        assert!(decoded.get(&pos(99)).is_none());
    }

    #[test]
    fn invalid_format_and_truncation_are_not_valid_occluders() {
        let mut entries = BTreeMap::new();
        let masks = BTreeMap::from([(0, [u64::MAX; 64])]);
        entries.insert(pos(1), IndexedColumn::new(masks));
        let bytes = encode(&entries, 4);
        for length in 0..bytes.len() {
            assert!(parse(&bytes[..length], 4).is_none());
        }
    }
}
