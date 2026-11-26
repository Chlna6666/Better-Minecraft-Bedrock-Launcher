//! Producer-owned compressed generated draw streams. StructuralDrawDelta is
//! expressed in canonical logical slots; preserve opaque runs directly and
//! rebuild only edited ranges and the camera-sorted transparent suffix.
use crate::prepare::StructuralDrawDelta;
use gfx_core::{BindGroupList, DrawIndirectCommand, DrawRunPlan, RenderCommand, RenderPipelineId};
use std::{collections::HashMap, ops::Range, sync::Arc};

const STRIDE: u64 = 16;
const BUILD_CHUNK: usize = 1024;

/// Explicit compatibility escape hatch; ordinary native multi-draw remains enabled.
pub(super) fn compact_limit(device: &dyn gpui::ExtensionDevice) -> u32 {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var_os("GPUI_3D_COMPACT_RUNS").as_deref() != Some(std::ffi::OsStr::new("0"))) {
        device.compact_draw_run_limit()
    } else { 0 }
}

#[derive(Clone)]
struct Run {
    draw: DrawIndirectCommand,
    len: u32,
}

impl Run {
    fn start(&self) -> usize { (self.draw.offset / STRIDE) as usize }
    fn end(&self) -> usize { self.start() + self.len as usize }
}

#[derive(Clone)]
pub(super) struct PublishedRuns {
    pub commands: Arc<[RenderCommand]>,
    pub plan: Arc<DrawRunPlan>,
}

#[derive(Default)]
pub(super) struct GeneratedRunCache {
    revision: Option<u64>,
    count: usize,
    opaque_count: usize,
    limit: u32,
    opaque: Vec<Run>,
    published: Option<PublishedRuns>,
}

fn compatible(a: &DrawIndirectCommand, b: &DrawIndirectCommand) -> bool {
    a.pipeline == b.pipeline && a.bind_groups == b.bind_groups
        && a.scissor == b.scissor && a.arguments == b.arguments
}

/// Append at run granularity, splitting only at the actual native limit.
fn append(out: &mut Vec<Run>, mut run: Run, limit: u32) {
    while run.len != 0 {
        if let Some(last) = out.last_mut()
            && last.len < limit && compatible(&last.draw, &run.draw)
            && last.draw.offset.checked_add(u64::from(last.len) * STRIDE) == Some(run.draw.offset)
        {
            let count = run.len.min(limit - last.len);
            last.len += count;
            run.len -= count;
            if run.len == 0 { break; }
            run.draw.offset += u64::from(count) * STRIDE;
        } else {
            let count = run.len.min(limit);
            let offset = run.draw.offset;
            let mut draw = run.draw.clone();
            draw.offset = offset;
            out.push(Run { draw, len: count });
            run.len -= count;
            if run.len == 0 { break; }
            run.draw.offset += u64::from(count) * STRIDE;
        }
    }
}

fn build_range(
    range: Range<usize>, canonical: bool, limit: u32, out: &mut Vec<Run>,
    scratch: &mut Vec<RenderCommand>,
    build: &mut impl FnMut(Range<usize>, &mut Vec<RenderCommand>) -> gpui::Result<()>,
) -> gpui::Result<bool> {
    for begin in (range.start..range.end).step_by(BUILD_CHUNK) {
        let end = begin.saturating_add(BUILD_CHUNK).min(range.end);
        scratch.clear();
        build(begin..end, scratch)?;
        // Restrict the compact producer to the canonical singleton ABI.
        // A noncanonical offset, skipped draw or authored scissor uses the
        // existing flat command path rather than guessing logical identity.
        if scratch.len() != end - begin { return Ok(false); }
        for (local, command) in scratch.drain(..).enumerate() {
            let RenderCommand::DrawIndirect(draw) = command else { return Ok(false); };
            let logical = draw.offset / STRIDE;
            if draw.scissor.is_some() || draw.offset % STRIDE != 0
                || (canonical && logical != (begin + local) as u64)
                || (!canonical && (logical < range.start as u64 || logical >= range.end as u64))
                || draw.offset.checked_add(STRIDE).is_none()
            { return Ok(false); }
            append(out, Run { draw, len: 1 }, limit);
        }
    }
    Ok(true)
}

/// Intersect old runs with the unchanged logical interval. Only the first
/// record's byte address changes; no survivor RenderCommand is materialized.
fn copy_survivors(old: &[Run], range: Range<usize>, next_start: usize, out: &mut Vec<Run>, limit: u32) -> bool {
    if range.is_empty() { return true; }
    let mut position = old.partition_point(|run| run.end() <= range.start);
    let mut covered = range.start;
    while let Some(run) = old.get(position) {
        let start = run.start().max(range.start);
        let end = run.end().min(range.end);
        if start >= range.end { break; }
        if start != covered || end <= start { return false; }
        let Some(offset) = next_start.checked_add(start - range.start)
            .and_then(|slot| u64::try_from(slot).ok()).and_then(|slot| slot.checked_mul(STRIDE))
        else { return false; };
        let mut draw = run.draw.clone();
        draw.offset = offset;
        let len = (end - start) as u32;
        if offset.checked_add(u64::from(len) * STRIDE).is_none() { return false; }
        append(out, Run { draw, len }, limit);
        covered = end;
        if covered == range.end { break; }
        position += 1;
    }
    covered == range.end
}

fn splice(
    previous: &[Run], old_count: usize, old_opaque: usize,
    count: usize, opaque: usize, delta: &StructuralDrawDelta, limit: u32,
    scratch: &mut Vec<RenderCommand>,
    build: &mut impl FnMut(Range<usize>, &mut Vec<RenderCommand>) -> gpui::Result<()>,
) -> gpui::Result<Option<Vec<Run>>> {
    if delta.previous_count != old_count { return Ok(None); }
    let (mut old, mut next) = (0usize, 0usize);
    let mut opaque_edits = 0;
    for edit in &delta.edits {
        if edit.old.start < old || edit.new.start < next
            || edit.old.start > edit.old.end || edit.new.start > edit.new.end
            || edit.old.end > old_count || edit.new.end > count
            || edit.old.start - old != edit.new.start - next
        { return Ok(None); }
        if edit.old.end <= old_opaque && edit.new.end <= opaque {
            opaque_edits += 1;
        } else if edit.old.start < old_opaque || edit.new.start < opaque {
            return Ok(None);
        }
        old = edit.old.end;
        next = edit.new.end;
    }
    if old_count - old != count - next { return Ok(None); }
    let mut runs = Vec::with_capacity(previous.len());
    old = 0;
    next = 0;
    for edit in &delta.edits[..opaque_edits] {
        if !copy_survivors(previous, old..edit.old.start, next, &mut runs, limit)
            || !build_range(edit.new.clone(), true, limit, &mut runs, scratch, build)?
        { return Ok(None); }
        old = edit.old.end;
        next = edit.new.end;
    }
    if old_opaque - old != opaque - next
        || !copy_survivors(previous, old..old_opaque, next, &mut runs, limit)
    { return Ok(None); }
    Ok(Some(runs))
}

/// Match the flat renderer's stable first-seen pipeline/binding ordering,
/// but visit only run representatives. The blend suffix is never reordered.
fn grouped_runs(opaque: &[Run], blend: Vec<Run>, limit: u32) -> Vec<Run> {
    let mut pipeline_slots = HashMap::<RenderPipelineId, usize>::new();
    let mut pipelines = Vec::<Vec<usize>>::new();
    let mut group_slots = HashMap::<(RenderPipelineId, BindGroupList), usize>::new();
    let mut groups = Vec::<Vec<Run>>::new();
    for run in opaque {
        let pipeline = run.draw.pipeline;
        let pi = *pipeline_slots.entry(pipeline).or_insert_with(|| {
            let index = pipelines.len();
            pipelines.push(Vec::new());
            index
        });
        let key = (pipeline, run.draw.bind_groups.clone());
        let gi = *group_slots.entry(key).or_insert_with(|| {
            let index = groups.len();
            groups.push(Vec::new());
            pipelines[pi].push(index);
            index
        });
        append(&mut groups[gi], run.clone(), limit);
    }
    // Match the uncompressed path when grouping provides no draw reuse.
    if groups.len() == opaque.len() && opaque.iter().all(|run| run.len == 1) {
        let mut result = opaque.to_vec();
        result.extend(blend);
        return result;
    }
    let mut result = Vec::with_capacity(opaque.len() + blend.len());
    for pipeline in pipelines {
        for group in pipeline {
            result.append(&mut groups[group]);
        }
    }
    // Do not merge across the opaque/blended boundary even if native state
    // happens to match. Blend ordering is already camera-authored.
    result.extend(blend);
    result
}

impl GeneratedRunCache {
    pub(super) fn refresh(
        &mut self, revision: u64, count: usize, first_blend: Option<usize>,
        delta: Option<&StructuralDrawDelta>, blend_only: bool, limit: u32,
        mut build: impl FnMut(Range<usize>, &mut Vec<RenderCommand>) -> gpui::Result<()>,
    ) -> gpui::Result<Option<PublishedRuns>> {
        let opaque_count = first_blend.unwrap_or(count);
        if limit < 2 || opaque_count > count { return Ok(None); }
        if self.revision == Some(revision) && self.limit == limit
            && self.count == count && self.opaque_count == opaque_count
        { return Ok(self.published.clone()); }
        let mut scratch = Vec::with_capacity(BUILD_CHUNK.min(count));
        let sequential = self.revision.is_some_and(|old| old.wrapping_add(1) == revision)
            && self.limit == limit;
        let mut opaque = if sequential && blend_only && self.count == count
            && self.opaque_count == opaque_count
        { Some(self.opaque.clone()) }
        else if sequential && let Some(delta) = delta {
            splice(&self.opaque, self.count, self.opaque_count, count, opaque_count,
                delta, limit, &mut scratch, &mut build)?
        } else { None };
        if opaque.is_none() {
            let mut rebuilt = Vec::new();
            if !build_range(0..opaque_count, true, limit, &mut rebuilt, &mut scratch, &mut build)? {
                *self = Self::default();
                return Ok(None);
            }
            opaque = Some(rebuilt);
        }
        let opaque = opaque.expect("validated canonical runs");
        let mut blend = Vec::new();
        if !build_range(opaque_count..count, false, limit, &mut blend, &mut scratch, &mut build)? {
            *self = Self::default();
            return Ok(None);
        }
        let runs = grouped_runs(&opaque, blend, limit);
        let mut commands = Vec::with_capacity(runs.len());
        let mut lengths = Vec::with_capacity(runs.len());
        for run in runs {
            lengths.push(run.len);
            commands.push(RenderCommand::DrawIndirect(run.draw));
        }
        let commands: Arc<[RenderCommand]> = commands.into();
        let plan = Arc::new(DrawRunPlan::from_representatives(&commands, lengths, limit)?);
        let published = PublishedRuns { commands, plan };
        self.revision = Some(revision);
        self.count = count;
        self.opaque_count = opaque_count;
        self.limit = limit;
        self.opaque = opaque;
        self.published = Some(published.clone());
        Ok(Some(published))
    }
}

/// Adapter for immutable producer streams that do not yet expose logical
/// StructuralDrawDelta (e.g. a CSM light-pass permutation).
#[derive(Default)]
pub(super) struct FlatRunCache {
    source: Option<std::sync::Weak<[RenderCommand]>>,
    limit: u32,
    published: Option<PublishedRuns>,
}

impl FlatRunCache {
    pub(super) fn refresh(&mut self, source: &Arc<[RenderCommand]>, limit: u32)
        -> gfx_core::Result<Option<PublishedRuns>>
    {
        if limit < 2 { return Ok(None); }
        let identity = Arc::downgrade(source);
        if self.limit == limit && self.source.as_ref().is_some_and(|old| old.ptr_eq(&identity)) {
            return Ok(self.published.clone());
        }
        let plan = DrawRunPlan::new(source, limit);
        let published = if plan.runs().len() == source.len() {
            None
        } else {
            let mut cursor = 0;
            let commands: Arc<[RenderCommand]> = plan.runs().iter().map(|&count| {
                let command = source[cursor].clone();
                cursor += count as usize;
                command
            }).collect();
            let compact = DrawRunPlan::from_representatives(&commands, plan.runs().to_vec(), limit)?;
            Some(PublishedRuns { commands, plan: Arc::new(compact) })
        };
        self.limit = limit;
        self.source = Some(identity);
        self.published = published.clone();
        Ok(published)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prepare::StructuralDrawEdit;
    use gfx_core::{BufferId, BindGroupId, bind_group_list};

    fn draw(slot: usize, group: u32) -> RenderCommand {
        RenderCommand::DrawIndirect(DrawIndirectCommand {
            pipeline: RenderPipelineId::from_parts(1, 1),
            bind_groups: bind_group_list([BindGroupId::from_parts(group, 1)]),
            arguments: BufferId::from_parts(1, 1),
            offset: slot as u64 * STRIDE, scissor: None,
        })
    }

    fn expand(published: &PublishedRuns) -> Vec<(usize, BindGroupId)> {
        published.commands.iter().zip(published.plan.runs()).flat_map(|(command, &count)| {
            let RenderCommand::DrawIndirect(draw) = command else { unreachable!() };
            (0..count).map(move |local| ((draw.offset / STRIDE) as usize + local as usize, draw.bind_groups[0]))
        }).collect()
    }

    #[test]
    fn structural_insert_reuses_survivor_runs_and_builds_only_edited_draws() {
        let mut cache = GeneratedRunCache::default();
        let old = cache.refresh(1, 10000, None, None, false, 1024,
            |range, output| { output.extend(range.map(|slot| draw(slot, 1))); Ok(()) })
            .unwrap().unwrap();
        assert_eq!(old.commands.len(), 10);
        let delta = StructuralDrawDelta { previous_count: 10000,
            edits: vec![StructuralDrawEdit { old: 5000..5000, new: 5000..5003 }] };
        let mut built = 0;
        let next = cache.refresh(2, 10003, None, Some(&delta), false, 1024,
            |range, output| { built += range.len(); output.extend(range.map(|slot| draw(slot, 2))); Ok(()) })
            .unwrap().unwrap();
        assert_eq!(built, 3);
        let mut actual = expand(&next);
        actual.sort_unstable_by_key(|entry| entry.0);
        assert_eq!(actual.len(), 10003);
        for (index, (slot, group)) in actual.into_iter().enumerate() {
            assert_eq!(index, slot);
            assert_eq!(group, BindGroupId::from_parts(if (5000..5003).contains(&index) { 2 } else { 1 }, 1));
        }
    }

    #[test]
    fn blend_only_preserves_camera_order_and_does_not_visit_opaque_draws() {
        let mut cache = GeneratedRunCache::default();
        cache.refresh(1, 104, Some(100), None, false, 64, |range, output| {
            output.extend(range.map(|slot| draw(slot, 1))); Ok(())
        }).unwrap();
        let next = cache.refresh(2, 104, Some(100), None, true, 64, |range, output| {
            assert_eq!(range, 100..104);
            output.extend([103, 101, 102, 100].map(|slot| draw(slot, 2))); Ok(())
        }).unwrap().unwrap();
        let actual = expand(&next);
        assert_eq!(actual[100..].iter().map(|entry| entry.0).collect::<Vec<_>>(), [103, 101, 102, 100]);
    }
}
