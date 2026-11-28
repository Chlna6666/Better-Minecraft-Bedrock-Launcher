use super::*;
use crate::window::scroll_rows::{RowListIdentity, RowPrepaint, RowRevision, UniformRowPlacement};

#[allow(clippy::too_many_arguments)]
pub(super) fn prepaint(
    render: &RenderItems,
    row_revision: Option<RowRevision>,
    item_count: usize,
    frame: &mut UniformListLayout,
    id: Option<&GlobalElementId>,
    range: Range<usize>,
    placement: Option<UniformRowPlacement<'_>>,
    row_origin: impl Fn(usize) -> crate::Point<Pixels>,
    available_space: Size<AvailableSpace>,
    content_mask: ContentMask<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    if !window.scroll_row_replay_is_enabled() {
        return;
    }
    let Some(id) = id else {
        return;
    };
    if range.is_empty() || frame.item_size.height.0 <= 0. || !frame.item_size.height.0.is_finite() {
        return;
    }
    let identity = RowListIdentity::Uniform(id.clone(), frame.item_size);
    let Some(mut plan) = window.with_content_mask(Some(content_mask), |window| {
        window.plan_scroll_rows(identity, item_count, available_space, row_revision, cx)
    }) else {
        return;
    };
    let visible = range;
    let range = match placement {
        Some(geometry) => window.bind_uniform_row_placement(&mut plan, geometry, visible.clone()),
        None => visible.clone(),
    };
    if let RenderItems::Each(renderer) = render {
        window.retain_uniform_row_builder(&mut plan, renderer, content_mask);
    }
    let reused: SmallVec<[bool; 64]> = range
        .clone()
        .map(|row| window.can_reuse_scroll_row(&plan, row, row_origin(row)))
        .collect();

    // GPUI Fast renders each contiguous missing run once. Kept rows never
    // call the row renderer, request layout, or prepaint their elements.
    let mut row = range.start;
    while row < range.end {
        if reused[row - range.start] {
            let prepaint = window.reuse_scroll_row(&plan, row, row_origin(row));
            plan.record_prepaint(row, prepaint);
            frame.items.push((row, None));
            row += 1;
            continue;
        }
        let next = if render.independent() {
            // The independent API has no cross-row build reads. Capture
            // each missing row separately without changing batch semantics.
            row + 1
        } else {
            reused[row - range.start..]
                .iter()
                .position(|&reused| reused)
                .map_or(range.end, |distance| row + distance)
        };
        let read_start = cx.entities.dependency_read_index();
        match render {
            RenderItems::Each(render_row) => {
                debug_assert_eq!(next, row + 1);
                // An independently owned row does not need the range
                // renderer's large inline SmallVec or batch iteration.
                let item = render_row(row, window, cx);
                let reads = cx.entities.row_reads_since(read_start);
                window.with_content_mask(Some(content_mask), |window| {
                    materialize_row(item, row, &visible, row_origin(row), available_space,
                        &reads, &mut plan, frame, window, cx);
                });
            }
            RenderItems::Batch(render_range) => {
                let items = render_range(row..next, window, cx);
                let reads = cx.entities.row_reads_since(read_start);
                window.with_content_mask(Some(content_mask), |window| {
                    for (item, index) in items.into_iter().zip(row..next) {
                        materialize_row(item, index, &visible, row_origin(index), available_space,
                            &reads, &mut plan, frame, window, cx);
                    }
                });
            }
        }
        row = next;
    }
    frame.row_plan = Some(plan);
}


/// Shared physical row materialization, with all required layout and
/// dependency context supplied by the caller. This is still executed
/// inside the ordinary frame transaction; it does not retain callbacks
/// or borrow elements from a previously destroyed arena.
#[allow(clippy::too_many_arguments)]
fn materialize_row(
    mut item: AnyElement,
    index: usize,
    visible: &Range<usize>,
    origin: crate::Point<Pixels>,
    available_space: Size<AvailableSpace>,
    build_reads: &crate::RenderDependencies,
    plan: &mut RowPlan,
    frame: &mut UniformListLayout,
    window: &mut Window,
    cx: &mut App,
) {
    let capture = plan.can_capture(&mut item);
    if !capture && !visible.contains(&index) {
        return;
    }
    let before = capture.then(|| (
        window.prepaint_index(), cx.entities.dependency_read_index(),
        window.next_frame.input_clip_epoch,
    ));
    window.with_list_item_layout_key(index, |window| {
        item.layout_as_root(available_space, window, cx);
        item.prepaint_at(origin, window, cx);
    });
    if let Some((start, read_start, clip_epoch)) = before {
        let extra = cx.entities.row_reads_since(read_start);
        // Build reads are shared only for a true range renderer; layout
        // and prepaint reads belong to this particular row.
        let mut reads = build_reads.clone();
        reads.merge_replayed(&extra);
        plan.record_prepaint(index, RowPrepaint {
            row_revision: plan.row_revision(index),
            fixed_input_clip: clip_epoch == window.next_frame.input_clip_epoch,
            origin,
            records: start..window.prepaint_index(),
            reads,
            validation: Default::default(),
        });
    }
    frame.items.push((index, Some(item)));
}
