use super::*;
use crate::window::scroll_rows::{RowListIdentity, RowPrepaint, RowRevision};

#[allow(clippy::too_many_arguments)]
pub(super) fn prepaint(
    render: &RenderItems,
    row_revision: Option<RowRevision>,
    item_count: usize,
    frame: &mut UniformListLayout,
    id: Option<&GlobalElementId>,
    range: Range<usize>,
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
        let next = reused[row - range.start..]
            .iter()
            .position(|&reused| reused)
            .map_or(range.end, |distance| row + distance);
        let read_start = cx.entities.dependency_read_index();
        let items = render(row..next, window, cx);
        let shared_reads = cx.entities.dependency_reads_since(read_start);
        window.with_content_mask(Some(content_mask), |window| {
            for (mut item, index) in items.into_iter().zip(row..next) {
                let origin = row_origin(index);
                let capture = plan.can_capture(&mut item);
                let before =
                    capture.then(|| (window.prepaint_index(), cx.entities.dependency_read_index()));
                window.with_list_item_layout_key(index, |window| {
                    item.layout_as_root(available_space, window, cx);
                    item.prepaint_at(origin, window, cx);
                });
                if let Some((start, read_start)) = before {
                    let extra = cx.entities.dependency_reads_since(read_start);
                    // The renderer takes a range, so its reads belong to
                    // every row in that batch. Layout/prepaint are per-row.
                    let mut reads = shared_reads.clone();
                    reads.merge_replayed(&extra);
                    let prepaint = RowPrepaint {
                        row_revision: plan.row_revision(index),
                        origin,
                        records: start..window.prepaint_index(),
                        reads,
                        validation: Default::default(),
                    };
                    plan.record_prepaint(index, prepaint);
                }
                frame.items.push((index, Some(item)));
            }
        });
        row = next;
    }
    frame.row_plan = Some(plan);
}
