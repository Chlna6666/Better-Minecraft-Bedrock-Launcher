//! Geometry Pool v3: numeric IDs, exact binary topology, lazy payload decoding.
//! Disk captures use bounded random-access reads; embedded ZIP decompresses once.
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Arc, Mutex};

const NONE: u32 = u32::MAX;
const MAX_COUNT: usize = 2_000_000;
const MAX_POOL: u64 = 1024 * 1024 * 1024;
const MAX_ENTRY: usize = 64 * 1024 * 1024;

fn u32_at(bytes: &[u8], at: &mut usize) -> Option<u32> {
    let next = at.checked_add(4)?;
    let value = u32::from_le_bytes(bytes.get(*at..next)?.try_into().ok()?);
    *at = next;
    Some(value)
}
fn u64_at(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let next = at.checked_add(8)?;
    let value = u64::from_le_bytes(bytes.get(*at..next)?.try_into().ok()?);
    *at = next;
    Some(value)
}
fn f32_at(bytes: &[u8], at: &mut usize) -> Option<f32> {
    Some(f32::from_bits(u32_at(bytes, at)?))
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    offset: u64,
    len: u32,
}
#[derive(Debug)]
enum Source {
    File(Mutex<File>),
    Embedded(Arc<[u8]>),
}
#[derive(Debug)]
pub(super) struct PoolV3 {
    entries: [Vec<Entry>; 4],
    source: Source,
}
impl PoolV3 {
    pub(super) fn open(root: &Path, embedded: Option<Vec<u8>>) -> Result<Self, String> {
        let (source, len, header) = if let Some(bytes) = embedded {
            if bytes.len() as u64 > MAX_POOL {
                return Err("oversize v3 pool".into());
            }
            let header = bytes.get(..28).ok_or("truncated v3 pool header")?.to_vec();
            let len = bytes.len() as u64;
            (Source::Embedded(Arc::from(bytes)), len, header)
        } else {
            let mut file = File::open(root.join("geometry_pool.bin"))
                .map_err(|e| format!("geometry_pool.bin: {e}"))?;
            let len = file.metadata().map_err(|e| e.to_string())?.len();
            if len > MAX_POOL {
                return Err("oversize v3 pool".into());
            }
            let mut header = vec![0; 28];
            file.read_exact(&mut header).map_err(|e| e.to_string())?;
            (Source::File(Mutex::new(file)), len, header)
        };
        if &header[..8] != b"BSCPOOL3" {
            return Err("invalid v3 pool magic".into());
        }
        let mut at = 8;
        if u32_at(&header, &mut at) != Some(3) {
            return Err("invalid v3 pool version".into());
        }
        let mut counts = [0usize; 4];
        for count in &mut counts {
            *count = u32_at(&header, &mut at).ok_or("invalid pool count")? as usize;
        }
        let total = counts.iter().try_fold(0usize, |n, c| n.checked_add(*c))
            .ok_or("pool count overflow")?;
        if total > MAX_COUNT {
            return Err("pool count exceeds limit".into());
        }
        let dir_len = total.checked_mul(12).ok_or("pool directory overflow")?;
        let begin = 28usize.checked_add(dir_len).ok_or("pool directory overflow")?;
        if begin as u64 > len {
            return Err("truncated pool directory".into());
        }
        let mut directory = vec![0u8; dir_len];
        match &source {
            Source::File(file) => file.lock().map_err(|e| e.to_string())?
                .read_exact(&mut directory).map_err(|e| e.to_string())?,
            Source::Embedded(bytes) => directory.copy_from_slice(&bytes[28..begin]),
        }
        let mut entries: [Vec<Entry>; 4] = std::array::from_fn(|_| Vec::new());
        at = 0;
        let mut next = begin as u64;
        for (kind, &count) in counts.iter().enumerate() {
            entries[kind].reserve(count);
            for _ in 0..count {
                let offset = u64_at(&directory, &mut at).ok_or("missing pool offset")?;
                let entry_len = u32_at(&directory, &mut at).ok_or("missing pool length")?;
                if offset != next || entry_len as usize > MAX_ENTRY {
                    return Err("noncanonical pool offset/length".into());
                }
                next = offset.checked_add(u64::from(entry_len)).ok_or("pool offset overflow")?;
                if next > len {
                    return Err("pool entry outside file".into());
                }
                entries[kind].push(Entry { offset, len: entry_len });
            }
        }
        if next != len {
            return Err("trailing pool data".into());
        }
        Ok(Self { entries, source })
    }
    pub(super) fn count(&self, kind: usize) -> usize {
        self.entries.get(kind).map_or(0, Vec::len)
    }
    pub(super) fn read(&self, kind: usize, id: u32) -> Option<Vec<u8>> {
        let entry = *self.entries.get(kind)?.get(id as usize)?;
        let mut buf = vec![0; entry.len as usize];
        match &self.source {
            Source::File(file) => {
                let mut file = file.lock().ok()?;
                file.seek(SeekFrom::Start(entry.offset)).ok()?;
                file.read_exact(&mut buf).ok()?;
            }
            Source::Embedded(data) => {
                let start = usize::try_from(entry.offset).ok()?;
                let end = start.checked_add(buf.len())?;
                buf.copy_from_slice(data.get(start..end)?);
            }
        }
        Some(buf)
    }
}
#[derive(Debug)]
pub(super) struct Geometry {
    pub(super) vertices: Vec<[f32; 3]>,
    pub(super) triangles: Vec<[usize; 3]>,
}
impl Geometry {
    pub(super) fn parse(bytes: &[u8]) -> Option<Self> {
        let mut at = 0;
        let n = u32_at(bytes, &mut at)? as usize;
        let t = u32_at(bytes, &mut at)? as usize;
        if n > MAX_COUNT || t == 0 || t > MAX_COUNT ||
            8usize.checked_add(n.checked_mul(12)?)?
                .checked_add(t.checked_mul(12)?)? != bytes.len() {
            return None;
        }
        let mut vertices = Vec::with_capacity(n);
        for _ in 0..n {
            let point = [f32_at(bytes, &mut at)?, f32_at(bytes, &mut at)?,
                         f32_at(bytes, &mut at)?];
            if !point.iter().all(|v| v.is_finite() && v.abs() <= 4096.) {
                return None;
            }
            vertices.push(point);
        }
        let mut triangles = Vec::with_capacity(t);
        for _ in 0..t {
            let indices = [u32_at(bytes, &mut at)? as usize,
                           u32_at(bytes, &mut at)? as usize,
                           u32_at(bytes, &mut at)? as usize];
            if indices.iter().any(|&v| v >= n) {
                return None;
            }
            triangles.push(indices);
        }
        Some(Self { vertices, triangles })
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Corner {
    pub(super) normal: [f32; 3],
    pub(super) uv: [f32; 2],
    pub(super) abgr: u32,
}
#[derive(Debug)]
pub(super) struct Appearance {
    pub(super) geometry_id: u32,
    pub(super) source_kind: u32,
    pub(super) uv_complete: bool,
    pub(super) corners: Vec<Corner>,
    pub(super) slots: Vec<Option<usize>>,
    // BSC writes the exact (variant, animation frame) selected per triangle.
    // A native atlas rectangle cannot be reconstructed from the slot alone.
    pub(super) variant_frames: Vec<Option<(u32, u32)>>,
}
impl Appearance {
    pub(super) fn parse(bytes: &[u8]) -> Option<Self> {
        let mut at = 0;
        let geometry_id = u32_at(bytes, &mut at)?;
        let _layer = u32_at(bytes, &mut at)?;
        let source_kind = u32_at(bytes, &mut at)?;
        let flags = u32_at(bytes, &mut at)?;
        let t = u32_at(bytes, &mut at)? as usize;
        if source_kind > 2 || flags & !3 != 0 || t == 0 || t > MAX_COUNT ||
            20usize.checked_add(t.checked_mul(84)?)? != bytes.len() {
            return None;
        }
        let mut corners = Vec::with_capacity(t * 3);
        for _ in 0..t * 3 {
            let normal = [f32_at(bytes, &mut at)?, f32_at(bytes, &mut at)?,
                          f32_at(bytes, &mut at)?];
            let uv = [f32_at(bytes, &mut at)?, f32_at(bytes, &mut at)?];
            let abgr = u32_at(bytes, &mut at)?;
            if !normal.iter().chain(uv.iter()).all(|v| v.is_finite()) {
                return None;
            }
            corners.push(Corner { normal, uv, abgr });
        }
        let mut slots = Vec::with_capacity(t);
        let mut variant_frames = Vec::with_capacity(t);
        for _ in 0..t {
            let slot = u32_at(bytes, &mut at)?;
            let variant = u32_at(bytes, &mut at)?;
            let frame = u32_at(bytes, &mut at)?;
            if (variant == NONE) != (frame == NONE) {
                return None;
            }
            slots.push((slot != NONE).then_some(slot as usize));
            variant_frames.push((variant != NONE).then_some((variant, frame)));
        }
        let uv_complete = flags & 1 != 0;
        if !uv_complete && slots.iter().any(Option::is_some) {
            return None;
        }
        Some(Self { geometry_id, source_kind, uv_complete, corners, slots, variant_frames })
    }
}
