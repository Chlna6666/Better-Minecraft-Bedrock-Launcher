use crate::{
    GpuiMemoryTrimLevel, Pixels, PlatformTextSystem, SharedString, WrappedLineLayout, px,
    record_text_layout_cache_metrics,
};
use collections::FxHashMap;
use parking_lot::{Condvar, Mutex};
use smallvec::SmallVec;
use std::{
    cell::{Cell, RefCell},
    hash::Hash,
    mem,
    ops::Range,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::{
        Arc, LazyLock,
        atomic::{AtomicU64, Ordering},
    },
};

use super::key::{AsCacheKeyRef, CacheKey, CacheKeyRef};
use super::{FontRun, LineLayout};

pub(crate) struct LineLayoutCache {
    frame: RefCell<FrameCache>,
    platform_text_system: Arc<dyn PlatformTextSystem>,
    font_generation: Arc<AtomicU64>,
    cached_font_generation: AtomicU64,
    frame_metrics: LineLayoutCacheMetrics,
    row_access_depth: Cell<usize>,
    number_shaping: RefCell<crate::fast::number_shaping::NumberShaping>,
    recent_shapes: Arc<Mutex<RecentShapes>>,
}

const LINE_LAYOUT_CACHE_MIN_RETAINED_CAPACITY: usize = 64;
const LINE_LAYOUT_CACHE_TRIM_WATERMARK_MULTIPLIER: usize = 4;
static LINE_LAYOUT_SINGLEFLIGHT: LazyLock<LineLayoutSingleflight> =
    LazyLock::new(LineLayoutSingleflight::default);

#[derive(Clone, Hash, Eq, PartialEq)]
struct LineLayoutFlightKey {
    platform_identity: usize,
    font_generation: u64,
    cache_key: Arc<CacheKey>,
}

#[derive(Default)]
struct LineLayoutSingleflight {
    flights: Mutex<FxHashMap<LineLayoutFlightKey, Arc<LineLayoutFlight>>>,
}

struct LineLayoutFlight {
    state: Mutex<LineLayoutFlightState>,
    ready: Condvar,
    #[cfg(test)]
    followers: std::sync::atomic::AtomicUsize,
}

impl Default for LineLayoutFlight {
    fn default() -> Self {
        Self {
            state: Mutex::new(LineLayoutFlightState::Pending),
            ready: Condvar::new(),
            #[cfg(test)]
            followers: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

enum LineLayoutFlightState {
    Pending,
    Ready(Arc<LineLayout>),
    Cancelled,
}

impl LineLayoutSingleflight {
    fn shape<F>(
        &self,
        platform_identity: usize,
        font_generation: u64,
        cache_key: Arc<CacheKey>,
        shape: F,
    ) -> Arc<LineLayout>
    where
        F: FnOnce() -> LineLayout,
    {
        let flight_key = LineLayoutFlightKey {
            platform_identity,
            font_generation,
            cache_key,
        };
        let mut shape = Some(shape);

        loop {
            let (flight, is_leader) = {
                let mut flights = self.flights.lock();
                if let Some(flight) = flights.get(&flight_key) {
                    (flight.clone(), false)
                } else {
                    let flight = Arc::new(LineLayoutFlight::default());
                    flights.insert(flight_key.clone(), flight.clone());
                    (flight, true)
                }
            };

            if is_leader {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    (shape
                        .take()
                        .expect("line layout singleflight leader missing shaper"))(
                    )
                }));

                match result {
                    Ok(layout) => {
                        let layout = Arc::new(layout);
                        {
                            let mut state = flight.state.lock();
                            *state = LineLayoutFlightState::Ready(layout.clone());
                            flight.ready.notify_all();
                        }
                        self.remove_if_current(&flight_key, &flight);
                        return layout;
                    }
                    Err(payload) => {
                        {
                            let mut state = flight.state.lock();
                            *state = LineLayoutFlightState::Cancelled;
                            flight.ready.notify_all();
                        }
                        self.remove_if_current(&flight_key, &flight);
                        resume_unwind(payload);
                    }
                }
            }

            #[cfg(test)]
            flight
                .followers
                .fetch_add(1, std::sync::atomic::Ordering::Release);

            let mut state = flight.state.lock();
            loop {
                if let LineLayoutFlightState::Ready(layout) = &*state {
                    return layout.clone();
                }
                if matches!(&*state, LineLayoutFlightState::Cancelled) {
                    break;
                }
                let _work = crate::window::element_work::begin_named(
                    "text_singleflight_wait",
                    crate::window::element_work::ElementWorkPhase::Layout,
                );
                flight.ready.wait(&mut state);
            }
            drop(state);
            // The leader panicked. Its flight has been removed and this caller still owns its
            // unused shaping closure, so retry registration and allow one follower to take over.
        }
    }

    fn remove_if_current(&self, key: &LineLayoutFlightKey, flight: &Arc<LineLayoutFlight>) {
        let mut flights = self.flights.lock();
        if flights
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, flight))
        {
            flights.remove(key);
        }
    }

    #[cfg(test)]
    fn follower_count(
        &self,
        platform_identity: usize,
        font_generation: u64,
        cache_key: &Arc<CacheKey>,
    ) -> usize {
        let key = LineLayoutFlightKey {
            platform_identity,
            font_generation,
            cache_key: cache_key.clone(),
        };
        self.flights
            .lock()
            .get(&key)
            .map(|flight| flight.followers.load(std::sync::atomic::Ordering::Acquire))
            .unwrap_or_default()
    }
}

fn platform_text_system_identity(platform: &Arc<dyn PlatformTextSystem>) -> usize {
    Arc::as_ptr(platform) as *const () as usize
}

/// Persistent shaping keys and integer frame access logs. Retained ranges
/// carry stable slot identities without hashing text or moving map entries.
/// Flat append storage for normal frames, immutable page slices for
/// detached boundaries. Freeze only when the first boundary is attempted.
#[derive(Clone, Debug)]
enum AccessLog {
    Flat(Vec<usize>),
    Paged { pages: Vec<AccessPage>, len: usize },
}

#[derive(Clone, Debug)]
struct AccessPage {
    ids: Arc<[usize]>,
    range: Range<usize>,
    end: usize,
}

impl Default for AccessLog {
    fn default() -> Self { Self::Flat(Vec::new()) }
}

impl AccessLog {
    const PAGE_SIZE: usize = 256;

    fn len(&self) -> usize {
        match self { Self::Flat(ids) => ids.len(), Self::Paged { len, .. } => *len }
    }

    fn is_empty(&self) -> bool { self.len() == 0 }

    fn visit(&self, range: Range<usize>, mut f: impl FnMut(&[usize])) {
        debug_assert!(range.start <= range.end && range.end <= self.len());
        match self {
            Self::Flat(ids) => f(&ids[range]),
            Self::Paged { pages, .. } => {
                let first = pages.partition_point(|page| page.end <= range.start);
                for page in &pages[first..] {
                    let base = page.end - page.range.len();
                    if base >= range.end { break; }
                    let start = range.start.saturating_sub(base);
                    let end = (range.end - base).min(page.range.len());
                    if start < end {
                        f(&page.ids[page.range.start + start..page.range.start + end]);
                    }
                }
            }
        }
    }

    fn to_vec(&self) -> Vec<usize> {
        let mut ids = Vec::with_capacity(self.len());
        self.visit(0..self.len(), |slice| ids.extend_from_slice(slice));
        ids
    }

    fn flat_mut(&mut self) -> &mut Vec<usize> {
        if matches!(self, Self::Paged { .. }) {
            *self = Self::Flat(self.to_vec());
        }
        match self { Self::Flat(ids) => ids, _ => unreachable!() }
    }

    fn push(&mut self, id: usize) { self.flat_mut().push(id); }
    fn truncate(&mut self, len: usize) { self.flat_mut().truncate(len); }

    fn clear(&mut self) {
        match self {
            Self::Flat(ids) => ids.clear(),
            Self::Paged { .. } => *self = Self::default(),
        }
    }

    fn trim(&mut self, floor: usize, force: bool) {
        match self {
            Self::Flat(ids) => {
                if force { ids.shrink_to(floor); }
                else { trim_vec_capacity(ids, floor, LINE_LAYOUT_CACHE_TRIM_WATERMARK_MULTIPLIER); }
            }
            Self::Paged { pages, .. } => {
                if force { pages.shrink_to_fit(); }
                else { trim_vec_capacity(pages, floor.div_ceil(Self::PAGE_SIZE), LINE_LAYOUT_CACHE_TRIM_WATERMARK_MULTIPLIER); }
            }
        }
    }

    fn freeze(&mut self) {
        let Self::Flat(ids) = self else { return; };
        let len = ids.len();
        let mut pages = Vec::with_capacity(len.div_ceil(Self::PAGE_SIZE));
        let mut end = 0;
        for chunk in ids.chunks(Self::PAGE_SIZE) {
            end += chunk.len();
            pages.push(AccessPage { ids: Arc::from(chunk), range: 0..chunk.len(), end });
        }
        *self = Self::Paged { pages, len };
    }

    fn append_shared(&self, range: Range<usize>, target: &mut Vec<AccessPage>, len: &mut usize) {
        let Self::Paged { pages, .. } = self else { unreachable!("frozen text access log"); };
        let first = pages.partition_point(|page| page.end <= range.start);
        for page in &pages[first..] {
            let base = page.end - page.range.len();
            if base >= range.end { break; }
            let start = range.start.saturating_sub(base);
            let end = (range.end - base).min(page.range.len());
            if start >= end { continue; }
            let slice = page.range.start + start..page.range.start + end;
            *len += slice.len();
            if let Some(last) = target.last_mut()
                && Arc::ptr_eq(&last.ids, &page.ids) && last.range.end == slice.start
            {
                last.range.end = slice.end;
                last.end = *len;
            } else {
                target.push(AccessPage { ids: page.ids.clone(), range: slice, end: *len });
            }
        }
    }
}

#[cfg(test)]
impl PartialEq<Vec<usize>> for AccessLog {
    fn eq(&self, other: &Vec<usize>) -> bool { self.to_vec().as_slice() == other.as_slice() }
}

#[cfg(test)]
impl std::ops::Index<usize> for AccessLog {
    type Output = usize;
    fn index(&self, index: usize) -> &usize {
        match self {
            Self::Flat(ids) => &ids[index],
            Self::Paged { pages, .. } => {
                let page = &pages[pages.partition_point(|page| page.end <= index)];
                &page.ids[page.range.start + index - (page.end - page.range.len())]
            }
        }
    }
}

struct LayoutStore<V> {
    keys: FxHashMap<Arc<CacheKey>, usize>,
    slots: Vec<Option<LayoutEntry<V>>>,
    free: Vec<usize>,
    previous: AccessLog,
    current: AccessLog,
    // Unique slots touched this epoch, not the length of the replay log.
    used_count: usize,
    // Allocated only after the first independent text boundary.
    boundary_counts: Option<FxHashMap<usize, usize>>,
    // Lazily built reverse key ownership; no extra Arc clones on ordinary inserts.
    boundary_keys: Option<FxHashMap<usize, Arc<CacheKey>>>,
    boundary_inactive_limit: usize,
}

struct PreparedTextAccesses {
    log: AccessLog,
    changes: Vec<(usize, usize)>,
    retired: Vec<usize>,
}

struct LayoutEntry<V> {
    layout: Arc<V>,
    last_used: u64,
}

impl<V> Default for LayoutStore<V> {
    fn default() -> Self {
        Self {
            keys: FxHashMap::default(),
            slots: Vec::new(),
            free: Vec::new(),
            previous: AccessLog::default(),
            current: AccessLog::default(),
            used_count: 0,
            boundary_counts: None,
            boundary_keys: None,
            boundary_inactive_limit: 0,
        }
    }
}

impl<V> LayoutStore<V> {
    #[cfg(test)]
    fn lookup(&mut self, key: &dyn AsCacheKeyRef, epoch: u64) -> Option<(Arc<V>, bool)> {
        self.lookup_recorded(key, epoch, false)
    }

    fn lookup_recorded(&mut self, key: &dyn AsCacheKeyRef, epoch: u64, row_access: bool) -> Option<(Arc<V>, bool)> {
        let id = *self.keys.get(key)?;
        let entry = self.slots[id].as_mut().expect("live text layout slot");
        let reused = entry.last_used != epoch;
        if reused {
            entry.last_used = epoch;
            self.used_count += 1;
        }
        // Rows can retire independently. Record repeated accesses inside
        // a row even if another owner already used the same immutable slot.
        if reused || row_access { self.current.push(id); }
        Some((Arc::clone(&entry.layout), reused))
    }

    fn insert(&mut self, key: Arc<CacheKey>, layout: Arc<V>, epoch: u64) {
        let entry = LayoutEntry { layout, last_used: epoch };
        let id = if let Some(&id) = self.keys.get(&key) {
            if self.slots[id].as_ref().expect("live text layout slot").last_used != epoch {
                self.used_count += 1;
            }
            self.slots[id] = Some(entry);
            id
        } else {
            let id = if let Some(id) = self.free.pop() {
                debug_assert!(self.slots[id].is_none());
                self.slots[id] = Some(entry);
                id
            } else {
                let id = self.slots.len();
                self.slots.push(Some(entry));
                id
            };
            if let Some(keys) = self.boundary_keys.as_mut() {
                keys.insert(id, Arc::clone(&key));
            }
            self.keys.insert(key, id);
            self.used_count += 1;
            id
        };
        self.current.push(id);
    }

    fn replay(&mut self, range: Range<usize>, epoch: u64) -> usize {
        let mut reused = 0;
        let slots = &mut self.slots;
        let current = self.current.flat_mut();
        self.previous.visit(range, |ids| {
            for &id in ids {
                let entry = slots[id].as_mut().expect("retained text layout slot");
                if entry.last_used != epoch {
                    entry.last_used = epoch;
                    reused += 1;
                }
            }
            current.extend_from_slice(ids);
        });
        self.used_count += reused;
        reused
    }

    /// An abandoned independent boundary may have touched cache slots in
    /// the current epoch. Restore their eligibility for the ordinary draw;
    /// truncating the log alone would suppress the next lookup's record.
    fn cancel_current_accesses(&mut self, epoch: u64) {
        if self.used_count == 0 && self.current.is_empty() { return; }
        // A nested prepaint rollback can have truncated the current
        // access log already. Scan live slots on this exceptional path,
        // including slots whose original access records were removed.
        for entry in self.slots.iter_mut().flatten() {
            if entry.last_used == epoch {
                entry.last_used = epoch.wrapping_sub(1);
            }
        }
        self.current.clear();
        self.used_count = 0;
    }

    /// Prepare the next access log from the committed and independently
    /// materialized streams. The slot table is authoritative for both.
    /// Duplicate IDs are deliberate and no layout Arc is cloned.
    fn splice_pages(&mut self, plan: &crate::window::RecordSplicePlan) -> Option<AccessLog> {
        use crate::window::SpliceSource;
        if !plan.matches_sources(self.previous.len(), self.current.len()) { return None; }
        self.previous.freeze();
        self.current.freeze();
        let mut pages = Vec::new();
        let mut len = 0;
        plan.visit_ranges(|source, range| {
            let source = match source {
                SpliceSource::Retained => &self.previous,
                SpliceSource::Replacement => &self.current,
            };
            source.append_shared(range, &mut pages, &mut len);
        });
        let mut accesses = AccessLog::Paged { pages, len };
        // Successive one-row edits can fragment descriptor storage. Only
        // repack when the page count is disproportionate to the live log.
        if let AccessLog::Paged { pages, .. } = &accesses
            && pages.len() > len.div_ceil(AccessLog::PAGE_SIZE).max(8).saturating_mul(4)
        {
            accesses = AccessLog::Flat(accesses.to_vec());
            accesses.freeze();
        }
        (len == plan.len()).then_some(accesses)
    }

    #[cfg(test)]
    fn spliced_accesses(&mut self, plan: &crate::window::RecordSplicePlan) -> Option<AccessLog> {
        let accesses = self.splice_pages(plan)?;
        let mut valid = true;
        accesses.visit(0..accesses.len(), |ids| {
            valid &= ids.iter().all(|&id| self.slots.get(id).is_some_and(Option::is_some));
        });
        valid.then_some(accesses)
    }

    /// Validate retained IDs once per full traversal. Subsequent
    /// boundaries visit only removed and inserted access ranges.
    fn prepare_boundary_accesses(
        &mut self, plan: &crate::window::RecordSplicePlan,
    ) -> Option<PreparedTextAccesses> {
        if !plan.matches_sources(self.previous.len(), self.current.len()) { return None; }
        if self.boundary_counts.is_none() {
            let mut counts = FxHashMap::<usize, usize>::default();
            let mut valid = true;
            self.previous.visit(0..self.previous.len(), |ids| {
                for &id in ids {
                    if !self.slots.get(id).is_some_and(Option::is_some) { valid = false; break; }
                    let count = counts.entry(id).or_default();
                    if let Some(next) = count.checked_add(1) { *count = next; }
                    else { valid = false; break; }
                }
            });
            if !valid { return None; }
            // Unreferenced layouts may still be shared by another cache
            // or unlogged non-row lookup. Defer their physical retirement
            // to the ordinary ownership-aware sweep, with bounded growth.
            self.boundary_inactive_limit = self.keys.len().checked_sub(counts.len())?
                .saturating_add(256).max(512);
            self.boundary_keys = Some(self.keys.iter().map(|(key, &id)| (id, Arc::clone(key))).collect());
            self.boundary_counts = Some(counts);
        }
        let counts = self.boundary_counts.as_ref()?;
        let mut changes = FxHashMap::<usize, usize>::default();
        let mut valid = true;
        for (replacement, log) in [(false, &self.previous), (true, &self.current)] {
            for range in plan.edited_ranges(replacement) {
                log.visit(range, |ids| {
                    for &id in ids {
                        if !self.slots.get(id).is_some_and(Option::is_some) { valid = false; break; }
                        let count = changes.entry(id).or_insert_with(|| counts.get(&id).copied().unwrap_or(0));
                        let next = if replacement { count.checked_add(1) } else { count.checked_sub(1) };
                        if let Some(next) = next { *count = next; }
                        else { valid = false; break; }
                    }
                });
                if !valid { return None; }
            }
        }
        let mut active = counts.len();
        let mut additions = 0usize;
        for (&id, &count) in &changes {
            match (counts.contains_key(&id), count != 0) {
                (true, false) => active = active.checked_sub(1)?,
                (false, true) => { active = active.checked_add(1)?; additions += 1; }
                _ => {}
            }
        }
        if self.keys.len().checked_sub(active)? > self.boundary_inactive_limit { return None; }
        self.boundary_counts.as_mut()?.try_reserve(additions).ok()?;
        let log = self.splice_pages(plan)?;
        let retired: Vec<_> = changes.iter().filter_map(|(&id, &count)|
            (count == 0).then_some(id)).collect();
        if !retired.iter().all(|id| self.boundary_keys.as_ref().is_some_and(|keys| keys.contains_key(id))) {
            return None;
        }
        self.free.try_reserve(retired.len()).ok()?;
        Some(PreparedTextAccesses { log, changes: changes.into_iter().collect(), retired })
    }

    fn commit_boundary_accesses(&mut self, prepared: PreparedTextAccesses) -> Vec<usize> {
        let counts = self.boundary_counts.as_mut().expect("prepared text counts");
        for (id, count) in prepared.changes {
            if count == 0 { counts.remove(&id); }
            else { counts.insert(id, count); }
        }
        self.previous = prepared.log;
        self.current.clear();
        self.used_count = 0;
        prepared.retired
    }

    /// Retire only zero-reference IDs from the edited ranges. External
    /// layout owners keep the slot live; the normal sweep handles them
    /// once their last external reference goes away.
    fn retire_boundary_slots(
        &mut self, retired: Vec<usize>, mut retire: impl FnMut(&Arc<CacheKey>, Arc<V>),
    ) {
        let counts = self.boundary_counts.as_ref().expect("committed text counts");
        let keys = self.boundary_keys.as_mut().expect("committed reverse keys");
        for id in retired {
            debug_assert!(!counts.contains_key(&id));
            let Some(entry) = self.slots.get(id).and_then(Option::as_ref) else { continue; };
            if Arc::strong_count(&entry.layout) > 1 { continue; }
            let key = keys.remove(&id).expect("validated retired text key");
            debug_assert_eq!(self.keys.get(&key), Some(&id));
            self.keys.remove(&key);
            let entry = self.slots[id].take().expect("validated retired slot");
            self.free.push(id);
            retire(&key, entry.layout);
        }
    }

    /// Adopt only after both text streams have been prepared and their
    /// font generation is still current. Retained slots must be marked
    /// live before the ordinary frame finish retires unused entries.
    #[cfg(test)]
    fn adopt_spliced_accesses(&mut self, accesses: AccessLog, epoch: u64) -> usize {
        let mut reused = 0;
        let slots = &mut self.slots;
        accesses.visit(0..accesses.len(), |ids| {
            for &id in ids {
                let entry = slots[id].as_mut().expect("validated text layout slot");
                if entry.last_used != epoch {
                    entry.last_used = epoch;
                    reused += 1;
                }
            }
        });
        self.used_count += reused;
        self.current = accesses;
        reused
    }

    fn finish(&mut self, epoch: u64, mut retire: impl FnMut(&Arc<CacheKey>, Arc<V>)) {
        self.boundary_counts = None;
        self.boundary_keys = None;
        self.boundary_inactive_limit = 0;
        debug_assert!(self.used_count <= self.keys.len());
        // Every key was used: no unused slot can be retired. Unlike an
        // access-log length check this also works for duplicate replays
        // and truncated speculative access ranges.
        if self.used_count != self.keys.len() {
            let slots = &mut self.slots;
            let free = &mut self.free;
            self.keys.retain(|key, id| {
                let id = *id;
                let entry = slots[id].as_ref().expect("live text layout slot");
                if entry.last_used == epoch || Arc::strong_count(&entry.layout) > 1 {
                    return true;
                }
                let entry = slots[id].take().expect("retiring text layout slot");
                retire(key, entry.layout);
                free.push(id);
                false
            });
        }
        mem::swap(&mut self.previous, &mut self.current);
        self.current.clear();
        self.used_count = 0;
    }

    fn clear(&mut self) {
        self.boundary_counts = None;
        self.boundary_keys = None;
        self.boundary_inactive_limit = 0;
        self.keys.clear();
        self.slots.clear();
        self.free.clear();
        self.previous.clear();
        self.current.clear();
        self.used_count = 0;
    }

    /// Slot IDs are private to this table and its two logs. Repack only on
    /// explicit memory trimming (or a small, heavily fragmented arena),
    /// rewriting every live index before exposing the compacted table.
    fn compact_slots(&mut self) {
        if self.free.is_empty() { return; }
        self.boundary_counts = None;
        self.boundary_keys = None;
        self.boundary_inactive_limit = 0;
        let mut relocated = vec![usize::MAX; self.slots.len()];
        let mut write = 0;
        for read in 0..self.slots.len() {
            if self.slots[read].is_some() {
                relocated[read] = write;
                if read != write {
                    self.slots[write] = self.slots[read].take();
                }
                write += 1;
            }
        }
        self.slots.truncate(write);
        for id in self.keys.values_mut().chain(self.previous.flat_mut().iter_mut()).chain(self.current.flat_mut().iter_mut()) {
            *id = relocated[*id];
            debug_assert!(*id < write);
        }
        self.free.clear();
    }

    fn trim(&mut self, floor: usize, force: bool) {
        // Compact only if enough memory will actually be reclaimed.
        // Bound automatic compaction work so ordinary frames do not pay
        // an unbounded relocation spike after a large page transition.
        let holes = self.free.len();
        if holes >= 64 && holes >= self.slots.len() / 2
            && (force || (self.slots.len() <= 4096 && holes >= self.slots.len() * 3 / 4))
        {
            self.compact_slots();
        }
        // Trim trailing holes without changing any live ID.
        while self.slots.last().is_some_and(Option::is_none) {
            self.slots.pop();
        }
        let len = self.slots.len();
        self.free.retain(|&id| id < len);
        if force {
            self.keys.shrink_to(floor);
            self.slots.shrink_to(floor);
            self.free.shrink_to(floor);
            self.previous.trim(floor, true);
            self.current.trim(floor, true);
        } else {
            trim_map_capacity(&mut self.keys, floor, LINE_LAYOUT_CACHE_TRIM_WATERMARK_MULTIPLIER);
            trim_vec_capacity(&mut self.slots, floor, LINE_LAYOUT_CACHE_TRIM_WATERMARK_MULTIPLIER);
            trim_vec_capacity(&mut self.free, floor, LINE_LAYOUT_CACHE_TRIM_WATERMARK_MULTIPLIER);
            self.previous.trim(floor, false);
            self.current.trim(floor, false);
        }
    }
}

struct FrameCache {
    epoch: u64,
    lines: LayoutStore<LineLayout>,
    wrapped_lines: LayoutStore<WrappedLineLayout>,
    prepared_boundary: Option<(PreparedTextAccesses, PreparedTextAccesses)>,
}

impl Default for FrameCache {
    fn default() -> Self {
        Self {
            epoch: 1,
            lines: LayoutStore::default(),
            wrapped_lines: LayoutStore::default(),
            prepared_boundary: None,
        }
    }
}

// Fast RecentShapes uses two generations instead of an LRU. Adapted from
// longbridge/gpui-fast fast/text.rs, revision 1100a214532bdeac0282acd1c5c6d2062d8ab1a7.
// Copyright 2022-2025 Zed Industries, Inc.; Copyright 2026 Longbridge.
// SPDX-License-Identifier: Apache-2.0
// Only retired, short unwrapped lines enter this pool. A live frame never
// shares a second cache entry with this pool, and glyphs are never copied.
const RECENT_SHAPE_UNITS: usize = 16 * 1024;
const MAX_RECENT_TEXT_BYTES: usize = 256;
const MAX_RECENT_RUNS: usize = 16;

#[derive(Default)]
pub(crate) struct RecentShapes {
    current: FxHashMap<Arc<CacheKey>, Arc<LineLayout>>,
    old: FxHashMap<Arc<CacheKey>, Arc<LineLayout>>,
    current_units: usize,
    generation: u64,
}

impl RecentShapes {
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn trim(&mut self, level: GpuiMemoryTrimLevel) {
        match level {
            GpuiMemoryTrimLevel::Light => self.old = FxHashMap::default(),
            GpuiMemoryTrimLevel::Moderate | GpuiMemoryTrimLevel::Aggressive => self.clear(),
        }
    }

    fn sync_generation(&mut self, generation: u64) {
        if self.generation != generation {
            self.clear();
            self.generation = generation;
        }
    }

    fn units(key: &CacheKey, layout: &LineLayout) -> Option<usize> {
        if key.text.len() > MAX_RECENT_TEXT_BYTES
            || key.runs.len() > MAX_RECENT_RUNS
            || layout.runs.len() > MAX_RECENT_RUNS
            || key.force_width.is_some()
        {
            return None;
        }
        let glyphs = layout.runs.iter().map(|run| run.glyphs.capacity()).sum::<usize>();
        let units = key.text.len().max(glyphs).max(8);
        (units <= RECENT_SHAPE_UNITS).then_some(units)
    }

    fn insert(&mut self, key: Arc<CacheKey>, layout: Arc<LineLayout>, units: usize) {
        if self.current_units + units > RECENT_SHAPE_UNITS {
            self.old = mem::take(&mut self.current);
            self.current_units = 0;
        }
        self.old.remove(key.as_ref());
        self.current_units += units;
        self.current.insert(key, layout);
    }

    fn take(&mut self, key: &dyn AsCacheKeyRef) -> Option<(Arc<CacheKey>, Arc<LineLayout>)> {
        self.current.remove_entry(key).or_else(|| self.old.remove_entry(key))
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct LineLayoutFrameMetrics {
    pub(crate) hits: usize,
    pub(crate) reuses: usize,
    pub(crate) misses: usize,
}

#[derive(Default)]
struct LineLayoutCacheMetrics {
    hits: Cell<usize>,
    reuses: Cell<usize>,
    misses: Cell<usize>,
}

impl LineLayoutCacheMetrics {
    fn hit(&self) {
        self.hits.set(self.hits.get().saturating_add(1));
    }

    fn reuse(&self) {
        self.reuse_many(1);
    }

    fn reuse_many(&self, count: usize) {
        self.reuses.set(self.reuses.get().saturating_add(count));
    }

    fn miss(&self) {
        self.misses.set(self.misses.get().saturating_add(1));
    }

    fn frame_metrics(&self) -> LineLayoutFrameMetrics {
        LineLayoutFrameMetrics {
            hits: self.hits.get(),
            reuses: self.reuses.get(),
            misses: self.misses.get(),
        }
    }

    fn finish_frame(&self) -> LineLayoutFrameMetrics {
        let metrics = self.frame_metrics();
        record_text_layout_cache_metrics(metrics.hits, metrics.reuses, metrics.misses);
        self.hits.set(0);
        self.reuses.set(0);
        self.misses.set(0);
        metrics
    }
}

#[derive(Clone, Default, PartialEq)]
pub(crate) struct LineLayoutIndex {
    font_generation: u64,
    lines_index: usize,
    wrapped_lines_index: usize,
}

impl LineLayoutIndex {
    pub(crate) fn components(&self) -> (u64, usize, usize) {
        (self.font_generation, self.lines_index, self.wrapped_lines_index)
    }

    pub(crate) fn from_components(font_generation: u64, lines_index: usize, wrapped_lines_index: usize) -> Self {
        Self { font_generation, lines_index, wrapped_lines_index }
    }

    /// Project a retained text range through independent line and wrapped
    /// line access edits. Never infer the ownership of ambiguous checkpoints.
    #[allow(dead_code)]
    pub(crate) fn retained_splice(
        range: &Range<Self>,
        generation: u64,
        lines: &crate::window::RecordSplicePlan,
        wrapped: &crate::window::RecordSplicePlan,
    ) -> Option<Range<Self>> {
        Self::project_splice(range, generation, lines, wrapped, false)
    }

    pub(crate) fn project_splice(
        range: &Range<Self>,
        generation: u64,
        lines: &crate::window::RecordSplicePlan,
        wrapped: &crate::window::RecordSplicePlan,
        enclosing: bool,
    ) -> Option<Range<Self>> {
        if range.start.font_generation != generation || range.end.font_generation != generation {
            return None;
        }
        let map = |plan: &crate::window::RecordSplicePlan, range| {
            if enclosing { plan.enclosing_span(range) } else { plan.retained_span(range) }
        };
        let l = map(lines, range.start.lines_index..range.end.lines_index)?;
        let w = map(wrapped, range.start.wrapped_lines_index..range.end.wrapped_lines_index)?;
        Some(Self { font_generation: generation, lines_index: l.start, wrapped_lines_index: w.start }
            ..Self { font_generation: generation, lines_index: l.end, wrapped_lines_index: w.end })
    }

    /// Rebase this frame-local index from one retained subtree origin to another.
    ///
    /// The operation is checked because a malformed retained range must degrade the frame instead
    /// of wrapping an index and replaying unrelated text layouts.
    pub(crate) fn rebased_from(&self, source: &Self, target: &Self) -> Option<Self> {
        if self.font_generation != source.font_generation
            || source.font_generation != target.font_generation
        {
            return None;
        }

        Some(Self {
            font_generation: target.font_generation,
            lines_index: target
                .lines_index
                .checked_add(self.lines_index.checked_sub(source.lines_index)?)?,
            wrapped_lines_index: target.wrapped_lines_index.checked_add(
                self.wrapped_lines_index
                    .checked_sub(source.wrapped_lines_index)?,
            )?,
        })
    }
}

impl LineLayoutCache {
    #[cfg(test)]
    pub fn new(
        platform_text_system: Arc<dyn PlatformTextSystem>,
        font_generation: Arc<AtomicU64>,
    ) -> Self {
        Self::with_recent_shapes(platform_text_system, font_generation, Arc::default())
    }

    pub(crate) fn with_recent_shapes(
        platform_text_system: Arc<dyn PlatformTextSystem>,
        font_generation: Arc<AtomicU64>,
        recent_shapes: Arc<Mutex<RecentShapes>>,
    ) -> Self {
        let cached_font_generation = font_generation.load(Ordering::Acquire);
        Self {
            frame: RefCell::default(),
            platform_text_system,
            font_generation,
            cached_font_generation: AtomicU64::new(cached_font_generation),
            frame_metrics: LineLayoutCacheMetrics::default(),
            row_access_depth: Cell::new(0),
            number_shaping: RefCell::default(),
            recent_shapes,
        }
    }

    fn clear_if_font_generation_changed(&self) -> u64 {
        let generation = self.font_generation.load(Ordering::Acquire);
        if self.cached_font_generation.load(Ordering::Acquire) == generation {
            return generation;
        }

        // Font discovery may advance the shared generation from another thread.
        // Clear the persistent keys and both frame logs before publishing
        // the new font generation.
        let mut frame = self.frame.borrow_mut();
        let generation = self.font_generation.load(Ordering::Acquire);
        if self.cached_font_generation.load(Ordering::Acquire) != generation {
            frame.clear();
            self.number_shaping.borrow_mut().clear();
            self.cached_font_generation
                .store(generation, Ordering::Release);
        }
        generation
    }

    /// Scope repeated access records to independently retireable rows.
    pub(crate) fn record_row_accesses(&self) -> impl Drop + '_ {
        struct Recording<'a>(&'a Cell<usize>);
        impl Drop for Recording<'_> {
            fn drop(&mut self) { self.0.set(self.0.get() - 1); }
        }
        self.row_access_depth.set(self.row_access_depth.get() + 1);
        Recording(&self.row_access_depth)
    }

    pub(crate) fn committed_layout_index(&self) -> Option<LineLayoutIndex> {
        let generation = self.font_generation.load(Ordering::Acquire);
        if generation != self.cached_font_generation.load(Ordering::Acquire) { return None; }
        let frame = self.frame.borrow();
        Some(LineLayoutIndex::from_components(generation, frame.lines.previous.len(), frame.wrapped_lines.previous.len()))
    }

    pub fn layout_index(&self) -> LineLayoutIndex {
        let font_generation = self.clear_if_font_generation_changed();
        let frame = self.frame.borrow();
        LineLayoutIndex {
            font_generation,
            lines_index: frame.lines.current.len(),
            wrapped_lines_index: frame.wrapped_lines.current.len(),
        }
    }

    pub fn can_reuse_layouts(&self, range: Range<LineLayoutIndex>) -> bool {
        let font_generation = self.clear_if_font_generation_changed();
        if range.start.font_generation != font_generation
            || range.end.font_generation != font_generation
        {
            return false;
        }

        let frame = self.frame.borrow();
        self.font_generation.load(Ordering::Acquire) == font_generation
            && line_layout_range_is_valid(
                range.start.lines_index,
                range.end.lines_index,
                frame.lines.previous.len(),
            )
            && line_layout_range_is_valid(
                range.start.wrapped_lines_index,
                range.end.wrapped_lines_index,
                frame.wrapped_lines.previous.len(),
            )
    }

    pub fn reuse_layouts(&self, range: Range<LineLayoutIndex>) {
        let font_generation = self.clear_if_font_generation_changed();
        if range.start.font_generation != font_generation
            || range.end.font_generation != font_generation
        {
            return;
        }

        let mut frame = self.frame.borrow_mut();
        if self.font_generation.load(Ordering::Acquire) != font_generation {
            return;
        }
        let epoch = frame.epoch;
        let reused = frame.lines.replay(range.start.lines_index..range.end.lines_index, epoch)
            + frame.wrapped_lines.replay(
                range.start.wrapped_lines_index..range.end.wrapped_lines_index, epoch);
        self.frame_metrics.reuse_many(reused);
    }

    /// Adopt the two text access streams together. The current log must
    /// contain only independently materialized replacement accesses. This
    /// is called at the final boundary-commit stage, immediately before
    /// normal finish_frame; no further fallible Frame adoption may follow.
    /// On rejection neither log is changed (including font invalidation).
    #[cfg(test)]
    pub(crate) fn splice_layouts(
        &self,
        generation: u64,
        lines: &crate::window::RecordSplicePlan,
        wrapped: &crate::window::RecordSplicePlan,
    ) -> Option<()> {
        if self.cached_font_generation.load(Ordering::Acquire) != generation
            || self.font_generation.load(Ordering::Acquire) != generation
        {
            return None;
        }
        let mut frame = self.frame.borrow_mut();
        let next_lines = frame.lines.spliced_accesses(lines)?;
        let next_wrapped = frame.wrapped_lines.spliced_accesses(wrapped)?;
        if self.font_generation.load(Ordering::Acquire) != generation { return None; }
        let epoch = frame.epoch;
        let reused = frame.lines.adopt_spliced_accesses(next_lines, epoch)
            + frame.wrapped_lines.adopt_spliced_accesses(next_wrapped, epoch);
        self.frame_metrics.reuse_many(reused);
        Some(())
    }

    /// Stage both streams without changing their committed access logs or
    /// active counts. The final placement lease may still reject.
    pub(crate) fn prepare_boundary_layouts(
        &self, generation: u64,
        lines: &crate::window::RecordSplicePlan,
        wrapped: &crate::window::RecordSplicePlan,
    ) -> Option<()> {
        if self.cached_font_generation.load(Ordering::Acquire) != generation
            || self.font_generation.load(Ordering::Acquire) != generation { return None; }
        let mut frame = self.frame.borrow_mut();
        if frame.prepared_boundary.is_some() || frame.epoch == u64::MAX { return None; }
        let next_lines = frame.lines.prepare_boundary_accesses(lines)?;
        let next_wrapped = frame.wrapped_lines.prepare_boundary_accesses(wrapped)?;
        if self.font_generation.load(Ordering::Acquire) != generation { return None; }
        frame.prepared_boundary = Some((next_lines, next_wrapped));
        Some(())
    }

    /// Consume the staged logs after the window placement lease. Epoch
    /// advancement makes the next ordinary lookup/replay record fresh
    /// access; no retained slot's last_used stamp needs to be rewritten.
    pub(crate) fn commit_boundary_layouts(&self) -> LineLayoutFrameMetrics {
        let mut frame = self.frame.borrow_mut();
        let (lines, wrapped) = frame.prepared_boundary.take().expect("prepared text boundary");
        let retired_wrapped = frame.wrapped_lines.commit_boundary_accesses(wrapped);
        let retired_lines = frame.lines.commit_boundary_accesses(lines);
        // Wrappers can own plain line layouts. Drop eligible wrappers
        // before inspecting the retired plain slots, as in full finish().
        frame.wrapped_lines.retire_boundary_slots(retired_wrapped, |_, _| {});
        let generation = self.cached_font_generation.load(Ordering::Acquire);
        let mut recent = None;
        frame.lines.retire_boundary_slots(retired_lines, |key, layout| {
            if let Some(units) = RecentShapes::units(key, &layout) {
                let recent = recent.get_or_insert_with(|| self.recent_shapes.lock());
                if self.font_generation.load(Ordering::Acquire) == generation {
                    recent.sync_generation(generation);
                    recent.insert(Arc::clone(key), layout, units);
                }
            }
        });
        frame.epoch = frame.epoch.checked_add(1).expect("validated text epoch");
        self.frame_metrics.finish_frame()
    }

    /// Only valid for a detached candidate starting with empty current logs.
    /// Do not finish the text frame: committed access offsets must survive.
    #[allow(dead_code)]
    pub(crate) fn cancel_boundary_layouts(&self) {
        let mut frame = self.frame.borrow_mut();
        frame.prepared_boundary = None;
        let epoch = frame.epoch;
        frame.lines.cancel_current_accesses(epoch);
        frame.wrapped_lines.cancel_current_accesses(epoch);
    }

    pub fn truncate_layouts(&self, index: LineLayoutIndex) {
        let font_generation = self.clear_if_font_generation_changed();
        if index.font_generation != font_generation {
            return;
        }

        let mut frame = self.frame.borrow_mut();
        if self.font_generation.load(Ordering::Acquire) != font_generation {
            return;
        }
        frame.lines.current.truncate(index.lines_index);
        frame.wrapped_lines.current.truncate(index.wrapped_lines_index);
    }

    pub fn clear(&self) {
        self.frame.borrow_mut().clear();
        self.number_shaping.borrow_mut().clear();
        self.recent_shapes.lock().clear();
    }

    pub fn trim_retained_capacity_for_level(&self, level: GpuiMemoryTrimLevel) {
        let _font_generation = self.clear_if_font_generation_changed();
        self.frame.borrow_mut().trim_retained_capacity_for_level(level);
        self.recent_shapes.lock().trim(level);
        if matches!(level, GpuiMemoryTrimLevel::Moderate | GpuiMemoryTrimLevel::Aggressive) {
            self.number_shaping.borrow_mut().clear();
        }
    }

    pub fn finish_frame(&self) -> LineLayoutFrameMetrics {
        let generation = self.clear_if_font_generation_changed();
        self.frame.borrow_mut().finish(&self.recent_shapes, generation, &self.font_generation);
        self.frame_metrics.finish_frame()
    }

    pub fn layout_wrapped_line<Text>(
        &self,
        text: Text,
        font_size: Pixels,
        runs: &[FontRun],
        wrap_width: Option<Pixels>,
        max_lines: Option<usize>,
    ) -> Arc<WrappedLineLayout>
    where
        Text: AsRef<str>,
        SharedString: From<Text>,
    {
        let font_generation = self.clear_if_font_generation_changed();
        let key = &CacheKeyRef {
            text: text.as_ref(),
            font_size,
            runs,
            wrap_width,
            max_lines,
            force_width: None,
        } as &dyn AsCacheKeyRef;

        let mut frame = self.frame.borrow_mut();
        let epoch = frame.epoch;
        if let Some((layout, reused)) = frame.wrapped_lines.lookup_recorded(key, epoch, self.row_access_depth.get() != 0) {
            if reused { self.frame_metrics.reuse(); } else { self.frame_metrics.hit(); }
            drop(frame);
            if self.font_generation.load(Ordering::Acquire) == font_generation {
                return layout;
            }
            return self.layout_wrapped_line(text, font_size, runs, wrap_width, max_lines);
        }
        self.frame_metrics.miss();
        drop(frame);
        let text = SharedString::from(text);
        let unwrapped_layout = self.layout_line::<&SharedString>(&text, font_size, runs, None);
        let wrap_boundaries = if let Some(wrap_width) = wrap_width {
            unwrapped_layout.compute_wrap_boundaries(text.as_ref(), wrap_width, max_lines)
        } else {
            SmallVec::new()
        };
        let layout = Arc::new(WrappedLineLayout {
            unwrapped_layout,
            wrap_boundaries,
            wrap_width,
        });
        let key = Arc::new(CacheKey {
            text,
            font_size,
            runs: SmallVec::from(runs),
            wrap_width,
            max_lines,
            force_width: None,
        });

        if self.font_generation.load(Ordering::Acquire) != font_generation {
            return self.layout_wrapped_line::<&SharedString>(
                &key.text, font_size, runs, wrap_width, max_lines,
            );
        }

        let mut frame = self.frame.borrow_mut();
        if self.font_generation.load(Ordering::Acquire) != font_generation {
            drop(frame);
            return self.layout_wrapped_line::<&SharedString>(
                &key.text, font_size, runs, wrap_width, max_lines,
            );
        }
        let epoch = frame.epoch;
        frame.wrapped_lines.insert(key, layout.clone(), epoch);

        layout
    }

    pub fn layout_line<Text>(
        &self,
        text: Text,
        font_size: Pixels,
        runs: &[FontRun],
        force_width: Option<Pixels>,
    ) -> Arc<LineLayout>
    where
        Text: AsRef<str>,
        SharedString: From<Text>,
    {
        let font_generation = self.clear_if_font_generation_changed();
        let key = &CacheKeyRef {
            text: text.as_ref(),
            font_size,
            runs,
            wrap_width: None,
            max_lines: None,
            force_width,
        } as &dyn AsCacheKeyRef;

        let mut frame = self.frame.borrow_mut();
        let epoch = frame.epoch;
        if let Some((layout, reused)) = frame.lines.lookup_recorded(key, epoch, self.row_access_depth.get() != 0) {
            if reused { self.frame_metrics.reuse(); } else { self.frame_metrics.hit(); }
            drop(frame);
            if self.font_generation.load(Ordering::Acquire) == font_generation {
                return layout;
            }
            return self.layout_line(text, font_size, runs, force_width);
        }

        let recent_entry = if force_width.is_none() && text.as_ref().len() <= MAX_RECENT_TEXT_BYTES {
            let mut recent = self.recent_shapes.lock();
            if self.font_generation.load(Ordering::Acquire) == font_generation {
                recent.sync_generation(font_generation);
                recent.take(key)
            } else { None }
        } else { None };
        if let Some((key, entry)) = recent_entry {
            let layout = entry.clone();
            frame.lines.insert(key, entry, epoch);
            self.frame_metrics.reuse();
            drop(frame);
            if self.font_generation.load(Ordering::Acquire) == font_generation {
                return layout;
            }
            return self.layout_line(text, font_size, runs, force_width);
        }
        self.frame_metrics.miss();
        drop(frame);
        let text = SharedString::from(text);
        let key = Arc::new(CacheKey {
            text: text.clone(),
            font_size,
            runs: SmallVec::from(runs),
            wrap_width: None,
            max_lines: None,
            force_width,
        });
        let platform_identity = platform_text_system_identity(&self.platform_text_system);
        let layout =
            LINE_LAYOUT_SINGLEFLIGHT.shape(platform_identity, font_generation, key.clone(), || {
                let mut layout = if force_width.is_none() {
                    self.number_shaping.borrow_mut().shape(
                        self.platform_text_system.as_ref(), &text, font_size, runs, font_generation,
                    )
                } else {
                    None
                }.unwrap_or_else(|| self.platform_text_system.layout_line(&text, font_size, runs));

                if let Some(force_width) = force_width {
                    apply_force_width_to_layout(&mut layout, force_width);
                }
                layout
            });

        if self.font_generation.load(Ordering::Acquire) != font_generation {
            return self.layout_line::<&SharedString>(&text, font_size, runs, force_width);
        }

        let mut frame = self.frame.borrow_mut();
        if self.font_generation.load(Ordering::Acquire) != font_generation {
            drop(frame);
            return self.layout_line::<&SharedString>(&text, font_size, runs, force_width);
        }
        let epoch = frame.epoch;
        frame.lines.insert(key, layout.clone(), epoch);
        layout
    }
}

// Combining marks such as Thai vowel signs and Arabic diacritics are commonly shaped at the same
// x position as their base glyph. Fixed-width positioning must not advance the cell counter for
// those zero/near-zero-advance glyphs, or the mark is displaced into the next cell. Keep the
// original within-cell offset while only advancing when shaping has moved far enough to indicate a
// new base glyph.
fn apply_force_width_to_layout(layout: &mut LineLayout, force_width: Pixels) {
    let mut glyph_pos = 0usize;
    let mut last_base_shaped_x = px(f32::NEG_INFINITY);
    let mut last_base_actual_x = px(0.);

    for run in layout.runs.iter_mut() {
        for glyph in run.glyphs.iter_mut() {
            let shaped_x = glyph.position.x;

            if shaped_x > last_base_shaped_x + force_width * 0.5 {
                let forced_x = glyph_pos * force_width;
                if (shaped_x - forced_x).abs() > px(1.) {
                    glyph.position.x = forced_x;
                }
                last_base_shaped_x = shaped_x;
                last_base_actual_x = glyph.position.x;
                glyph_pos += 1;
            } else {
                glyph.position.x = last_base_actual_x + (shaped_x - last_base_shaped_x);
            }
        }
    }
}

fn line_layout_range_is_valid(start: usize, end: usize, len: usize) -> bool {
    start <= end && end <= len
}

impl FrameCache {
    fn clear(&mut self) {
        self.prepared_boundary = None;
        self.lines.clear();
        self.wrapped_lines.clear();
        self.epoch = 1;
    }

    fn trim_retained_capacity_for_level(&mut self, level: GpuiMemoryTrimLevel) {
        let (floor, force) = match level {
            GpuiMemoryTrimLevel::Light => (LINE_LAYOUT_CACHE_MIN_RETAINED_CAPACITY, false),
            GpuiMemoryTrimLevel::Moderate => (LINE_LAYOUT_CACHE_MIN_RETAINED_CAPACITY, true),
            GpuiMemoryTrimLevel::Aggressive => {
                self.clear();
                (0, true)
            }
        };
        self.lines.trim(floor, force);
        self.wrapped_lines.trim(floor, force);
    }

    fn finish(
        &mut self,
        recent_shapes: &Mutex<RecentShapes>,
        generation: u64,
        font_generation: &AtomicU64,
    ) {
        debug_assert!(self.prepared_boundary.is_none());
        self.prepared_boundary = None;
        // Drop unused wrappers first, preserving Fast's ownership-based
        // retention of live unwrapped layouts and bounded recent shapes.
        self.wrapped_lines.finish(self.epoch, |_, _| {});
        let mut recent = None;
        self.lines.finish(self.epoch, |key, layout| {
            if let Some(units) = RecentShapes::units(key, &layout) {
                let recent = recent.get_or_insert_with(|| recent_shapes.lock());
                if font_generation.load(Ordering::Acquire) == generation {
                    recent.sync_generation(generation);
                    recent.insert(Arc::clone(key), layout, units);
                }
            }
        });
        self.epoch = self.epoch.checked_add(1).expect("text layout epoch exhausted");
    }
}

fn trim_map_capacity<K, V>(map: &mut FxHashMap<K, V>, floor: usize, multiplier: usize)
where
    K: Eq + Hash,
{
    let target = floor.max(map.len());
    if map.capacity() > target.saturating_mul(multiplier) {
        map.shrink_to(target);
    }
}

fn trim_vec_capacity<T>(vec: &mut Vec<T>, floor: usize, multiplier: usize) {
    let target = floor.max(vec.len());
    if vec.capacity() > target.saturating_mul(multiplier) {
        vec.shrink_to(target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{ShapedGlyph, ShapedRun};
    use crate::{
        FontId, GlyphId, GpuiMemoryTrimLevel, NoopTextSystem, Point, snapshot,
        point, px,
    };

    #[test]
    fn line_layout_singleflight_deduplicates_concurrent_shape() {
        let singleflight = Arc::new(LineLayoutSingleflight::default());
        let key = Arc::new(CacheKey {
            text: "shared".into(),
            font_size: px(14.),
            runs: SmallVec::from([FontRun {
                len: 6,
                font_id: FontId(1),
            }]),
            wrap_width: None,
            max_lines: None,
            force_width: None,
        });
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(0);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);

        let leader_singleflight = singleflight.clone();
        let leader_key = key.clone();
        let leader = std::thread::spawn(move || {
            leader_singleflight.shape(7, 0, leader_key, || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                let mut layout = LineLayout::default();
                layout.width = px(42.);
                layout
            })
        });

        started_rx.recv().unwrap();
        let follower_singleflight = singleflight.clone();
        let follower_key = key.clone();
        let follower = std::thread::spawn(move || {
            follower_singleflight.shape(7, 0, follower_key, || {
                panic!("follower must reuse the in-flight shaped layout")
            })
        });

        while singleflight.follower_count(7, 0, &key) == 0 {
            std::thread::yield_now();
        }
        release_tx.send(()).unwrap();

        let first = leader.join().unwrap();
        let second = follower.join().unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first.width, px(42.));
        assert!(singleflight.flights.lock().is_empty());
    }

    #[test]
    fn line_layout_singleflight_cleans_up_after_panicking_leader() {
        let singleflight = LineLayoutSingleflight::default();
        let key = Arc::new(CacheKey {
            text: "panic".into(),
            font_size: px(14.),
            runs: SmallVec::from([FontRun {
                len: 5,
                font_id: FontId(1),
            }]),
            wrap_width: None,
            max_lines: None,
            force_width: None,
        });

        let result = catch_unwind(AssertUnwindSafe(|| {
            singleflight.shape(9, 0, key.clone(), || panic!("shape failed"))
        }));
        assert!(result.is_err());
        assert!(singleflight.flights.lock().is_empty());

        let layout = singleflight.shape(9, 0, key, LineLayout::default);
        assert_eq!(layout.width, px(0.));
        assert!(singleflight.flights.lock().is_empty());
    }

    #[test]
    fn line_layout_index_rebases_relative_offsets() {
        let source = LineLayoutIndex {
            font_generation: 7,
            lines_index: 10,
            wrapped_lines_index: 4,
        };
        let target = LineLayoutIndex {
            font_generation: 7,
            lines_index: 30,
            wrapped_lines_index: 20,
        };
        let index = LineLayoutIndex {
            font_generation: 7,
            lines_index: 17,
            wrapped_lines_index: 9,
        };
        let rebased = index.rebased_from(&source, &target).unwrap();
        assert_eq!(rebased.lines_index, 37);
        assert_eq!(rebased.wrapped_lines_index, 25);

        let different_generation = LineLayoutIndex {
            font_generation: 8,
            lines_index: 17,
            wrapped_lines_index: 9,
        };
        assert!(
            different_generation
                .rebased_from(&source, &target)
                .is_none(),
            "retained layout indices from another font generation must never be rebased"
        );
    }

    #[test]
    fn force_width_preserves_combining_mark_offset_across_runs() {
        fn glyph(id: u32, x: f32, index: usize) -> ShapedGlyph {
            ShapedGlyph {
                id: GlyphId(id),
                position: point(px(x), px(0.)),
                render_offset: Point::default(),
                font_size: px(14.),
                index,
                is_emoji: false,
                is_cjk: false,
            }
        }

        let mut layout = LineLayout {
            font_size: px(14.),
            width: px(18.),
            ascent: px(11.),
            descent: px(3.),
            runs: vec![
                ShapedRun {
                    font_id: FontId(1),
                    glyphs: vec![glyph(1, 0., 0)],
                },
                // Simulate a combining mark shaped by a fallback face. Its one-pixel relative
                // offset belongs to the first base glyph and must not consume another cell.
                ShapedRun {
                    font_id: FontId(2),
                    glyphs: vec![glyph(2, 1., 1)],
                },
                ShapedRun {
                    font_id: FontId(1),
                    glyphs: vec![glyph(3, 8., 2)],
                },
            ],
            len: 3,
        };

        apply_force_width_to_layout(&mut layout, px(10.));

        assert_eq!(layout.runs[0].glyphs[0].position.x, px(0.));
        assert_eq!(layout.runs[1].glyphs[0].position.x, px(1.));
        assert_eq!(layout.runs[2].glyphs[0].position.x, px(10.));
    }

    #[test]
    fn retired_lines_leave_frame_map_but_live_measurements_survive_idle_frames() {
        let cache =
            LineLayoutCache::new(Arc::new(NoopTextSystem::new()), Arc::new(AtomicU64::new(0)));
        let runs = [FontRun { len: 5, font_id: FontId(1) }];
        let live = cache.layout_line("hello", px(14.), &runs, None);
        let dead = cache.layout_line("other", px(14.), &runs, None);
        let weak = Arc::downgrade(&dead);
        drop(dead);
        cache.finish_frame();
        assert!(weak.upgrade().is_some());
        cache.finish_frame();
        assert!(weak.upgrade().is_some());
        assert_eq!(cache.frame.borrow().lines.keys.len(), 1);
        let retired = cache.layout_line("other", px(14.), &runs, None);
        assert!(Arc::ptr_eq(&weak.upgrade().unwrap(), &retired));
        assert!(cache.recent_shapes.lock().current.is_empty());
        assert!(cache.frame.borrow().lines.previous.is_empty());
        let reused = cache.layout_line("hello", px(14.), &runs, None);
        assert!(Arc::ptr_eq(&live, &reused));
    }

    #[test]
    fn dead_wrapped_layout_is_not_kept_in_recent_shapes() {
        let cache =
            LineLayoutCache::new(Arc::new(NoopTextSystem::new()), Arc::new(AtomicU64::new(0)));
        let runs = [FontRun { len: 5, font_id: FontId(1) }];
        let wrapped = cache.layout_wrapped_line("hello", px(14.), &runs, Some(px(80.)), None);
        let weak_wrapped = Arc::downgrade(&wrapped);
        let weak_line = Arc::downgrade(&wrapped.unwrapped_layout);
        drop(wrapped);
        cache.finish_frame();
        cache.finish_frame();
        assert!(weak_wrapped.upgrade().is_none());
        assert!(weak_line.upgrade().is_some());
        cache.recent_shapes.lock().clear();
        assert!(weak_line.upgrade().is_none());
        assert!(cache.frame.borrow().wrapped_lines.keys.is_empty());
        assert!(cache.frame.borrow().lines.keys.is_empty());
    }

    #[test]
    fn recent_shapes_move_between_windows_and_respect_font_generation() {
        let platform: Arc<dyn PlatformTextSystem> = Arc::new(NoopTextSystem::new());
        let generation = Arc::new(AtomicU64::new(0));
        let recent = Arc::new(Mutex::new(RecentShapes::default()));
        let first = LineLayoutCache::with_recent_shapes(platform.clone(), generation.clone(), recent.clone());
        let second = LineLayoutCache::with_recent_shapes(platform, generation.clone(), recent.clone());
        let runs = [FontRun { len: 5, font_id: FontId(1) }];
        let line = first.layout_line("hello", px(14.), &runs, None);
        let weak = Arc::downgrade(&line);
        drop(line);
        first.finish_frame();
        first.finish_frame();
        assert!(first.frame.borrow().lines.keys.is_empty());
        let reused = second.layout_line("hello", px(14.), &runs, None);
        assert!(Arc::ptr_eq(&weak.upgrade().unwrap(), &reused));
        assert!(recent.lock().current.is_empty());
        drop(reused);
        second.finish_frame();
        second.finish_frame();
        assert!(weak.upgrade().is_some());
        generation.fetch_add(1, Ordering::Release);
        let fresh = first.layout_line("hello", px(14.), &runs, None);
        assert!(weak.upgrade().is_none());
        assert_eq!(fresh.len, 5);
    }

    #[test]
    fn recent_shapes_are_bounded_and_skip_oversized_lines() {
        let cache = LineLayoutCache::new(Arc::new(NoopTextSystem::new()), Arc::default());
        let long = "x".repeat(MAX_RECENT_TEXT_BYTES + 1);
        let runs = [FontRun { len: long.len(), font_id: FontId(1) }];
        let oversized = cache.layout_line(long.as_str(), px(14.), &runs, None);
        let weak = Arc::downgrade(&oversized);
        drop(oversized);
        cache.finish_frame();
        cache.finish_frame();
        assert!(weak.upgrade().is_none());
        let mut recent = cache.recent_shapes.lock();
        for i in 0..3 {
            let key = Arc::new(CacheKey {
                text: format!("shape {i}").into(),
                font_size: px(14.),
                runs: SmallVec::new(),
                wrap_width: None, max_lines: None, force_width: None,
            });
            recent.insert(key, Arc::new(LineLayout::default()), RECENT_SHAPE_UNITS);
        }
        assert_eq!(recent.current.len(), 1);
        assert_eq!(recent.old.len(), 1);
        assert_eq!(recent.current_units, RECENT_SHAPE_UNITS);
    }

    #[test]
    fn text_boundary_splice_validates_both_streams_before_adoption() {
        use crate::window::RecordSplicePlan;
        let generation = Arc::new(AtomicU64::new(0));
        let cache = LineLayoutCache::new(Arc::new(NoopTextSystem::new()), generation.clone());
        let runs = [FontRun { len: 5, font_id: FontId(1) }];
        let old = cache.layout_wrapped_line("hello", px(14.), &runs, Some(px(80.)), None);
        cache.finish_frame();
        let new = cache.layout_wrapped_line("world", px(14.), &runs, Some(px(80.)), None);
        let (old_lines, old_wrapped, new_lines, new_wrapped) = {
            let frame = cache.frame.borrow();
            (frame.lines.previous.to_vec(), frame.wrapped_lines.previous.to_vec(),
                frame.lines.current.to_vec(), frame.wrapped_lines.current.to_vec())
        };
        let lines = RecordSplicePlan::new(old_lines.len(), new_lines.len(),
            &[(old_lines.len()..old_lines.len(), 0..new_lines.len())]).unwrap();
        let wrapped = RecordSplicePlan::new(old_wrapped.len(), new_wrapped.len(),
            &[(old_wrapped.len()..old_wrapped.len(), 0..new_wrapped.len())]).unwrap();
        let invalid = RecordSplicePlan::new(old_wrapped.len() + 1, new_wrapped.len(), &[]).unwrap();
        assert!(cache.splice_layouts(0, &lines, &invalid).is_none());
        {
            let frame = cache.frame.borrow();
            assert_eq!(frame.lines.current, new_lines);
            assert_eq!(frame.wrapped_lines.current, new_wrapped);
        }
        assert!(cache.splice_layouts(1, &lines, &wrapped).is_none());
        cache.splice_layouts(0, &lines, &wrapped).unwrap();
        cache.finish_frame();
        let frame = cache.frame.borrow();
        assert_eq!(frame.lines.previous, [old_lines, new_lines].concat());
        assert_eq!(frame.wrapped_lines.previous, [old_wrapped, new_wrapped].concat());
        assert_eq!(frame.wrapped_lines.previous.len(), 2);
        assert!(Arc::ptr_eq(&old, &frame.wrapped_lines.slots[frame.wrapped_lines.previous[0]].as_ref().unwrap().layout));
        assert!(Arc::ptr_eq(&new, &frame.wrapped_lines.slots[frame.wrapped_lines.previous[1]].as_ref().unwrap().layout));
    }

    #[test]
    fn boundary_text_retirement_preserves_external_owners_and_reuses_free_slots() {
        use crate::window::RecordSplicePlan;
        let key = |text: &'static str| Arc::new(CacheKey {
            text: SharedString::from(text), font_size: px(14.), runs: SmallVec::new(),
            wrap_width: None, max_lines: None, force_width: None,
        });
        let mut store = LayoutStore::<LineLayout>::default();
        let a = key("shared");
        let b = key("unshared");
        let external = Arc::new(LineLayout::default());
        store.insert(a.clone(), external.clone(), 1);
        store.insert(b.clone(), Arc::new(LineLayout::default()), 1);
        let a_id = store.keys[&a];
        let b_id = store.keys[&b];
        store.finish(1, |_, _| {});
        let plan = RecordSplicePlan::new(2, 0, &[(0..2, 0..0)]).unwrap();
        let prepared = store.prepare_boundary_accesses(&plan).unwrap();
        let retired = store.commit_boundary_accesses(prepared);
        store.retire_boundary_slots(retired, |key, _| assert_eq!(key, &b));
        assert!(store.slots[a_id].is_some());
        assert!(store.slots[b_id].is_none());
        assert_eq!(store.free, vec![b_id]);
        let c = key("new");
        store.insert(c.clone(), Arc::new(LineLayout::default()), 2);
        assert_eq!(store.keys[&c], b_id);
        let plan = RecordSplicePlan::new(0, 1, &[(0..0, 0..1)]).unwrap();
        let prepared = store.prepare_boundary_accesses(&plan).unwrap();
        let retired = store.commit_boundary_accesses(prepared);
        store.retire_boundary_slots(retired, |_, _| panic!("nothing retired"));
        let plan = RecordSplicePlan::new(1, 0, &[(0..1, 0..0)]).unwrap();
        let prepared = store.prepare_boundary_accesses(&plan).unwrap();
        let retired = store.commit_boundary_accesses(prepared);
        store.retire_boundary_slots(retired, |key, _| assert_eq!(key, &c));
        assert!(!store.keys.contains_key(&c));
        assert!(store.keys.contains_key(&a));
        drop(external);
        store.finish(4, |key, _| assert_eq!(key, &a));
        assert!(store.keys.is_empty());
    }

    #[test]
    fn boundary_text_count_delta_adopts_duplicate_replacement_ids() {
        use crate::window::RecordSplicePlan;
        let mut store = LayoutStore::<LineLayout>::default();
        let key = |text: &'static str| Arc::new(CacheKey {
            text: SharedString::from(text), font_size: px(14.), runs: SmallVec::new(),
            wrap_width: None, max_lines: None, force_width: None,
        });
        let a = key("old");
        let b = key("new");
        store.insert(a.clone(), Arc::new(LineLayout::default()), 1);
        store.lookup_recorded(a.as_ref(), 1, true).unwrap();
        let old_id = store.keys[&a];
        store.finish(1, |_, _| {});
        store.insert(b.clone(), Arc::new(LineLayout::default()), 2);
        let new_id = store.keys[&b];
        let plan = RecordSplicePlan::new(2, 1, &[(0..1, 0..1), (2..2, 0..1)]).unwrap();
        let prepared = store.prepare_boundary_accesses(&plan).unwrap();
        assert_eq!(store.previous, vec![old_id, old_id]);
        store.commit_boundary_accesses(prepared);
        assert_eq!(store.previous, vec![new_id, old_id, new_id]);
        let counts = store.boundary_counts.as_ref().unwrap();
        assert_eq!(counts[&old_id], 1);
        assert_eq!(counts[&new_id], 2);
        assert_eq!(store.slots[old_id].as_ref().unwrap().last_used, 1);
        assert_eq!(store.used_count, 0);
        let plan = RecordSplicePlan::new(3, 0, &[(1..2, 0..0)]).unwrap();
        let prepared = store.prepare_boundary_accesses(&plan).unwrap();
        store.commit_boundary_accesses(prepared);
        assert!(!store.boundary_counts.as_ref().unwrap().contains_key(&old_id));
        assert_eq!(store.boundary_counts.as_ref().unwrap()[&new_id], 2);
        assert_eq!(store.replay(0..2, 4), 1);
        store.finish(4, |key, _| assert_eq!(key, &a));
        assert_eq!(store.previous, vec![new_id, new_id]);
        assert!(!store.keys.contains_key(&a));
    }

    #[test]
    fn boundary_text_counts_update_only_changed_slots_and_survive_cancel() {
        use crate::window::RecordSplicePlan;
        let generation = Arc::new(AtomicU64::new(0));
        let cache = LineLayoutCache::new(Arc::new(NoopTextSystem::new()), generation);
        let runs = [FontRun { len: 5, font_id: FontId(1) }];
        {
            let _record = cache.record_row_accesses();
            cache.layout_wrapped_line("hello", px(14.), &runs, Some(px(80.)), None);
            cache.layout_wrapped_line("hello", px(14.), &runs, Some(px(80.)), None);
        }
        cache.finish_frame();
        let (old_lines, old_wrapped, old_id, old_stamp) = {
            let frame = cache.frame.borrow();
            let id = frame.wrapped_lines.previous[0];
            (frame.lines.previous.len(), frame.wrapped_lines.previous.len(),
                id, frame.wrapped_lines.slots[id].as_ref().unwrap().last_used)
        };
        assert_eq!(old_wrapped, 2);
        let lines = RecordSplicePlan::new(old_lines, 0, &[]).unwrap();
        let wrapped = RecordSplicePlan::new(old_wrapped, 0, &[(0..1, 0..0)]).unwrap();
        cache.prepare_boundary_layouts(0, &lines, &wrapped).unwrap();
        cache.cancel_boundary_layouts();
        {
            let frame = cache.frame.borrow();
            assert_eq!(frame.wrapped_lines.previous.len(), 2);
            assert_eq!(frame.wrapped_lines.boundary_counts.as_ref().unwrap()[&old_id], 2);
        }
        cache.prepare_boundary_layouts(0, &lines, &wrapped).unwrap();
        cache.commit_boundary_layouts();
        {
            let frame = cache.frame.borrow();
            assert_eq!(frame.wrapped_lines.previous, vec![old_id]);
            assert_eq!(frame.wrapped_lines.boundary_counts.as_ref().unwrap()[&old_id], 1);
            assert_eq!(frame.wrapped_lines.slots[old_id].as_ref().unwrap().last_used, old_stamp);
        }
        let wrapped = RecordSplicePlan::new(1, 0, &[(0..1, 0..0)]).unwrap();
        cache.prepare_boundary_layouts(0, &lines, &wrapped).unwrap();
        cache.commit_boundary_layouts();
        {
            let frame = cache.frame.borrow();
            assert!(frame.wrapped_lines.previous.is_empty());
            assert!(frame.wrapped_lines.boundary_counts.as_ref().unwrap().is_empty());
            assert!(frame.wrapped_lines.slots[old_id].is_none(), "unshared zero-reference wrapper retires locally");
        }
        // Normal frame traversal remains authoritative for unedited or externally shared slots.
        cache.finish_frame();
        let frame = cache.frame.borrow();
        assert!(frame.wrapped_lines.boundary_counts.is_none());
        assert!(frame.wrapped_lines.slots[old_id].is_none());
    }

    #[test]
    fn boundary_text_pages_share_physical_slot_ids_across_successive_edits() {
        use crate::window::RecordSplicePlan;
        let mut store = LayoutStore::<LineLayout>::default();
        let key = Arc::new(CacheKey {
            text: SharedString::from("shared"), font_size: px(14.), runs: SmallVec::new(),
            wrap_width: None, max_lines: None, force_width: None,
        });
        store.insert(key.clone(), Arc::new(LineLayout::default()), 1);
        for _ in 1..768 {
            store.lookup_recorded(key.as_ref(), 1, true).unwrap();
        }
        store.finish(1, |_, _| {});
        let plan = RecordSplicePlan::new(768, 0, &[(0..1, 0..0)]).unwrap();
        let next = store.spliced_accesses(&plan).unwrap();
        let AccessLog::Paged { pages, .. } = &store.previous else { panic!("frozen log") };
        let middle = pages[1].ids.clone();
        let AccessLog::Paged { pages, .. } = &next else { panic!("shared log") };
        assert!(Arc::ptr_eq(&middle, &pages[1].ids));
        store.adopt_spliced_accesses(next, 2);
        store.finish(2, |_, _| {});
        let plan = RecordSplicePlan::new(767, 0, &[(766..767, 0..0)]).unwrap();
        let next = store.spliced_accesses(&plan).unwrap();
        let AccessLog::Paged { pages, len } = &next else { panic!("shared log") };
        assert_eq!(*len, 766);
        assert!(Arc::ptr_eq(&middle, &pages[1].ids));
    }

    #[test]
    fn row_scoped_text_hits_keep_independent_access_records() {
        use crate::window::RecordSplicePlan;
        let mut store = LayoutStore::<LineLayout>::default();
        let key = Arc::new(CacheKey {
            text: SharedString::from("shared"), font_size: px(14.), runs: SmallVec::new(),
            wrap_width: None, max_lines: None, force_width: None,
        });
        store.insert(key.clone(), Arc::new(LineLayout::default()), 1);
        let id = store.current[0];
        assert!(!store.lookup_recorded(key.as_ref(), 1, false).unwrap().1);
        assert_eq!(store.current, vec![id]);
        assert!(!store.lookup_recorded(key.as_ref(), 1, true).unwrap().1);
        assert!(!store.lookup_recorded(key.as_ref(), 1, true).unwrap().1);
        assert_eq!(store.current, vec![id, id, id]);
        assert_eq!(store.used_count, 1);
        store.finish(1, |_, _| {});
        let plan = RecordSplicePlan::new(3, 0, &[(0..2, 0..0)]).unwrap();
        let accesses = store.spliced_accesses(&plan).unwrap();
        assert_eq!(accesses, vec![id]);
        assert_eq!(store.adopt_spliced_accesses(accesses, 2), 1);
        store.finish(2, |_, _| panic!("kept row must retain shared text layout"));
        assert_eq!(store.previous, vec![id]);
        assert_eq!(store.keys[&key], id);
    }

    #[test]
    fn cancelled_boundary_text_accesses_can_be_recorded_again() {
        let mut store = LayoutStore::<LineLayout>::default();
        let key = Arc::new(CacheKey {
            text: SharedString::from("a"), font_size: px(14.), runs: SmallVec::new(),
            wrap_width: None, max_lines: None, force_width: None,
        });
        store.insert(key.clone(), Arc::new(LineLayout::default()), 1);
        store.finish(1, |_, _| {});
        let id = store.previous[0];
        store.replay(0..1, 2);
        store.replay(0..1, 2);
        store.cancel_current_accesses(2);
        assert_eq!(store.previous, vec![id]);
        assert!(store.current.is_empty());
        assert_eq!(store.used_count, 0);
        assert_eq!(store.replay(0..1, 2), 1);
        assert_eq!(store.current, vec![id]);
    }

    #[test]
    fn text_access_splice_preserves_slots_duplicates_and_retained_liveness() {
        use crate::window::RecordSplicePlan;
        let mut store = LayoutStore::<LineLayout>::default();
        let key = |text: &'static str| Arc::new(CacheKey {
            text: SharedString::from(text), font_size: px(14.), runs: SmallVec::new(),
            wrap_width: None, max_lines: None, force_width: None,
        });
        let a = key("a");
        let b = key("b");
        let c = key("c");
        store.insert(a.clone(), Arc::new(LineLayout::default()), 1);
        store.insert(b.clone(), Arc::new(LineLayout::default()), 1);
        let a_id = store.keys[&a];
        let b_id = store.keys[&b];
        store.finish(1, |_, _| {});
        store.insert(c.clone(), Arc::new(LineLayout::default()), 2);
        let c_id = store.keys[&c];
        let plan = RecordSplicePlan::new(2, 1, &[(1..2, 0..1), (2..2, 0..1)]).unwrap();
        let accesses = store.spliced_accesses(&plan).unwrap();
        assert_eq!(accesses, vec![a_id, c_id, c_id]);
        assert_eq!(store.previous, vec![a_id, b_id]);
        assert_eq!(store.current, vec![c_id]);
        assert_eq!(store.adopt_spliced_accesses(accesses, 2), 1);
        assert_eq!(store.used_count, 2);
        store.finish(2, |key, _| assert_eq!(key, &b));
        assert_eq!(store.previous, vec![a_id, c_id, c_id]);
        assert_eq!(store.keys[&a], a_id);
        assert_eq!(store.keys[&c], c_id);
        assert!(!store.keys.contains_key(&b));
    }

    #[test]
    fn text_index_splice_rejects_wrong_font_generation_and_ambiguous_checkpoint() {
        use crate::window::RecordSplicePlan;
        let lines = RecordSplicePlan::new(6, 2, &[(1..2, 0..2)]).unwrap();
        let wrapped = RecordSplicePlan::new(5, 0, &[(1..3, 0..0)]).unwrap();
        let index = |l, w| LineLayoutIndex { font_generation: 7, lines_index: l, wrapped_lines_index: w };
        let span = index(3, 4)..index(5, 5);
        let mapped = LineLayoutIndex::retained_splice(&span, 7, &lines, &wrapped).unwrap();
        assert_eq!(mapped.start.lines_index..mapped.end.lines_index, 4..6);
        assert_eq!(mapped.start.wrapped_lines_index..mapped.end.wrapped_lines_index, 2..3);
        assert!(LineLayoutIndex::retained_splice(&span, 8, &lines, &wrapped).is_none());
        let insert = RecordSplicePlan::new(6, 1, &[(3..3, 0..1)]).unwrap();
        let empty = index(3, 4)..index(3, 4);
        assert!(LineLayoutIndex::retained_splice(&empty, 7, &insert, &wrapped).is_none());
    }

    #[test]
    fn layout_store_retires_unused_slots_without_aliasing_replay_indices() {
        let mut store = LayoutStore::<LineLayout>::default();
        let key = |text: &'static str| Arc::new(CacheKey {
            text: SharedString::from(text),
            font_size: px(14.),
            runs: SmallVec::new(),
            wrap_width: None,
            max_lines: None,
            force_width: None,
        });
        let a = key("a");
        let b = key("b");
        let c = key("c");
        store.insert(a.clone(), Arc::new(LineLayout::default()), 1);
        store.insert(b.clone(), Arc::new(LineLayout::default()), 1);
        let a_id = store.keys[&a];
        let b_id = store.keys[&b];
        assert_eq!(store.used_count, 2);
        store.finish(1, |_, _| panic!("used layout must survive"));
        assert_eq!(store.used_count, 0);
        assert_eq!(store.replay(0..1, 2), 1);
        assert_eq!(store.replay(0..1, 2), 0);
        assert_eq!(store.used_count, 1);
        let mut retired = 0;
        store.finish(2, |key, _| {
            assert_eq!(key, &b);
            retired += 1;
        });
        assert_eq!(retired, 1);
        assert_eq!(store.previous, vec![a_id, a_id]);
        assert!(!store.keys.contains_key(&b));
        store.insert(c.clone(), Arc::new(LineLayout::default()), 3);
        assert_eq!(store.keys[&c], b_id);
        assert_eq!(store.replay(0..2, 3), 1);
        assert_eq!(store.used_count, 2);
        // Speculative truncation removes records, not this epoch's cache
        // ownership, exactly like the original per-frame maps.
        store.current.truncate(1);
        store.finish(3, |_, _| panic!("all keys touched"));
        assert_eq!(store.previous, vec![b_id]);
        assert_eq!(store.keys.len(), 2);
        store.clear();
        assert!(store.previous.is_empty());
        assert_eq!(store.used_count, 0);
    }

    #[test]
    fn compacted_text_slots_rebase_both_access_logs_and_lookup_identity() {
        let mut store = LayoutStore::<LineLayout>::default();
        let keys: Vec<_> = (0..128).map(|i| Arc::new(CacheKey {
            text: format!("line {i}").into(),
            font_size: px(14.),
            runs: SmallVec::new(),
            wrap_width: None, max_lines: None, force_width: None,
        })).collect();
        for key in &keys {
            store.insert(key.clone(), Arc::new(LineLayout::default()), 1);
        }
        store.finish(1, |_, _| {});
        assert_eq!(store.replay(127..128, 2), 1);
        store.finish(2, |_, _| {});
        assert_eq!(store.free.len(), 127);
        assert_eq!(store.replay(0..1, 3), 1);
        store.trim(0, true);
        assert_eq!(store.slots.len(), 1);
        assert_eq!(store.previous, vec![0]);
        assert_eq!(store.current, vec![0]);
        assert_eq!(store.keys[&keys[127]], 0);
        assert_eq!(store.used_count, 1);
        store.finish(3, |_, _| panic!("live slot retired"));
    }

    #[test]
    fn retained_layout_replay_uses_stable_slots_and_preserves_duplicate_ranges() {
        let cache = LineLayoutCache::new(Arc::new(NoopTextSystem::new()), Arc::default());
        let runs = [FontRun { len: 5, font_id: FontId(1) }];
        let start = cache.layout_index();
        let line = cache.layout_line("hello", px(14.), &runs, None);
        let wrapped = cache.layout_wrapped_line("hello", px(14.), &runs, Some(px(80.)), None);
        let end = cache.layout_index();
        cache.finish_frame();
        let (line_id, wrapped_id) = {
            let frame = cache.frame.borrow();
            (frame.lines.previous[0], frame.wrapped_lines.previous[0])
        };
        assert!(cache.can_reuse_layouts(start.clone()..end.clone()));
        cache.reuse_layouts(start.clone()..end.clone());
        cache.reuse_layouts(start..end);
        {
            let frame = cache.frame.borrow();
            assert_eq!(frame.lines.current, vec![line_id, line_id]);
            assert_eq!(frame.wrapped_lines.current, vec![wrapped_id, wrapped_id]);
            assert!(Arc::ptr_eq(&line, &frame.lines.slots[line_id].as_ref().unwrap().layout));
            assert!(Arc::ptr_eq(&wrapped, &frame.wrapped_lines.slots[wrapped_id].as_ref().unwrap().layout));
        }
        assert_eq!(cache.frame_metrics.frame_metrics().reuses, 2);
        cache.finish_frame();
        assert_eq!(cache.frame.borrow().lines.previous, vec![line_id, line_id]);
    }

    #[test]
    fn layout_line_records_same_frame_hits() {
        let cache =
            LineLayoutCache::new(Arc::new(NoopTextSystem::new()), Arc::new(AtomicU64::new(0)));
        let runs = [FontRun {
            len: 5,
            font_id: FontId(1),
        }];

        let first = cache.layout_line("hello", px(14.), &runs, None);
        let second = cache.layout_line("hello", px(14.), &runs, None);
        cache.finish_frame();

        let metrics = snapshot();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(metrics.text_layout_hits, 1);
        assert_eq!(metrics.text_layout_reuses, 0);
        assert_eq!(metrics.text_layout_misses, 1);
    }

    #[test]
    fn layout_line_records_previous_frame_reuse() {
        let cache =
            LineLayoutCache::new(Arc::new(NoopTextSystem::new()), Arc::new(AtomicU64::new(0)));
        let runs = [FontRun {
            len: 5,
            font_id: FontId(1),
        }];

        let first = cache.layout_line("hello", px(14.), &runs, None);
        cache.finish_frame();
        let second = cache.layout_line("hello", px(14.), &runs, None);
        cache.finish_frame();

        let metrics = snapshot();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(metrics.text_layout_hits, 0);
        assert_eq!(metrics.text_layout_reuses, 1);
        assert_eq!(metrics.text_layout_misses, 0);
    }

    #[test]
    fn layout_line_reuses_live_entry_after_idle_frame() {
        let cache =
            LineLayoutCache::new(Arc::new(NoopTextSystem::new()), Arc::new(AtomicU64::new(0)));
        let runs = [FontRun {
            len: 5,
            font_id: FontId(1),
        }];

        let first = cache.layout_line("hello", px(14.), &runs, None);
        cache.finish_frame();
        cache.finish_frame();
        let second = cache.layout_line("hello", px(14.), &runs, None);

        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn font_generation_invalidates_live_layouts() {
        let generation = Arc::new(AtomicU64::new(0));
        let cache = LineLayoutCache::new(Arc::new(NoopTextSystem::new()), generation.clone());
        let runs = [FontRun {
            len: 5,
            font_id: FontId(1),
        }];

        let first = cache.layout_line("hello", px(14.), &runs, None);
        cache.finish_frame();
        cache.finish_frame();
        let stale_index = cache.layout_index();

        generation.fetch_add(1, Ordering::Release);

        assert!(!cache.can_reuse_layouts(stale_index.clone()..stale_index));
        let second = cache.layout_line("hello", px(14.), &runs, None);
        assert!(!Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn aggressive_trim_clears_layout_cache_entries() {
        let cache =
            LineLayoutCache::new(Arc::new(NoopTextSystem::new()), Arc::new(AtomicU64::new(0)));
        let runs = [FontRun {
            len: 5,
            font_id: FontId(1),
        }];

        let first = cache.layout_line("hello", px(14.), &runs, None);
        cache.finish_frame();
        cache.finish_frame();
        cache.trim_retained_capacity_for_level(GpuiMemoryTrimLevel::Aggressive);
        let second = cache.layout_line("hello", px(14.), &runs, None);

        assert!(!Arc::ptr_eq(&first, &second));
    }
}

#[cfg(test)]
mod clamp_cache_tests {
    use super::*;
    use crate::{FontId, NoopTextSystem, px};

    #[test]
    fn wrapped_cache_distinguishes_line_clamp() {
        let text = "one two three four five six";
        let runs = [FontRun { len: text.len(), font_id: FontId(1) }];
        let cache = LineLayoutCache::new(Arc::new(NoopTextSystem::new()), Arc::default());
        let full = cache.layout_wrapped_line(text, px(14.), &runs, Some(px(50.)), None);
        let clamped = cache.layout_wrapped_line(text, px(14.), &runs, Some(px(50.)), Some(1));
        assert!(!Arc::ptr_eq(&full, &clamped));
        assert!(full.wrap_boundaries.len() > clamped.wrap_boundaries.len());
        cache.finish_frame();
        let warm = cache.layout_wrapped_line(text, px(14.), &runs, Some(px(50.)), Some(1));
        assert!(Arc::ptr_eq(&clamped, &warm));
    }
}
