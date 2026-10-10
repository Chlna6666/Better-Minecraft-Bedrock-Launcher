use crate::{
    BackdropBlurDamagePlan, Bounds, ScaledPixels, Scene, SceneAnimationCompletion,
    SceneAnimationId, SceneAnimationTimeline, SceneAnimationValue, TransitionProperty,
};
use collections::{FxHashMap, FxHashSet};
use futures::{StreamExt, channel::mpsc, stream::Stream};
use parking_lot::Mutex;
use smallvec::SmallVec;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

const MAX_DIRTY_RECTS: usize = 128;

#[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
pub(crate) enum UiCommitRequest {
    #[default]
    None,
    /// Run UI-owned timelines, rebuilding the scene only if they invalidate it.
    AnimationTick,
    Required,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
pub(crate) enum PresentationRequest {
    #[default]
    None,
    Required,
}

/// One coalesced platform wake-up containing two independent work domains.
///
/// UI commit and presentation are intentionally represented as separate request types rather than
/// two rendering-policy booleans. The platform may transport them together today, but neither
/// domain is defined in terms of the other.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
pub(crate) struct PlatformFrameRequest {
    ui_commit: UiCommitRequest,
    presentation: PresentationRequest,
}

impl PlatformFrameRequest {
    /// Wake the UI animation owner without forcing render/layout of a clean scene.
    /// Native compositor continuations use `presentation()` instead.
    pub(crate) const fn animation_tick() -> Self {
        Self {
            ui_commit: UiCommitRequest::AnimationTick,
            presentation: PresentationRequest::Required,
        }
    }

    pub(crate) const fn ui_commit() -> Self {
        Self {
            ui_commit: UiCommitRequest::Required,
            presentation: PresentationRequest::None,
        }
    }

    pub(crate) const fn presentation() -> Self {
        Self {
            ui_commit: UiCommitRequest::None,
            presentation: PresentationRequest::Required,
        }
    }

    pub(crate) const fn ui_commit_and_presentation() -> Self {
        Self {
            ui_commit: UiCommitRequest::Required,
            presentation: PresentationRequest::Required,
        }
    }

    /// Whether the UI owner must process this wake before native presentation continues.
    /// Only `needs_ui_rebuild()` forces rebuilding an otherwise clean scene.
    pub(crate) const fn needs_ui_commit(self) -> bool {
        !matches!(self.ui_commit, UiCommitRequest::None)
    }

    /// Whether the caller explicitly requires a scene rebuild, rather than just a UI tick.
    pub(crate) const fn needs_ui_rebuild(self) -> bool {
        matches!(self.ui_commit, UiCommitRequest::Required)
    }

    pub(crate) const fn needs_presentation(self) -> bool {
        matches!(self.presentation, PresentationRequest::Required)
    }

    pub(crate) const fn is_presentation_only(self) -> bool {
        self.needs_presentation() && !self.needs_ui_commit()
    }

    pub(crate) const fn requires_frame(self) -> bool {
        self.needs_ui_commit() || self.needs_presentation()
    }

    pub(crate) const fn merge(self, request: Self) -> Self {
        Self {
            ui_commit: if self.needs_ui_rebuild() || request.needs_ui_rebuild() {
                UiCommitRequest::Required
            } else if self.needs_ui_commit() || request.needs_ui_commit() {
                UiCommitRequest::AnimationTick
            } else {
                UiCommitRequest::None
            },
            presentation: if self.needs_presentation() || request.needs_presentation() {
                PresentationRequest::Required
            } else {
                PresentationRequest::None
            },
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PlatformFrameResult {
    /// The platform submitted this packet to its renderer before returning.
    Submitted,
    /// The compositor accepted the packet for later presentation.
    Queued,
    Deferred,
}

impl PlatformFrameResult {
    pub(crate) const fn is_accepted(self) -> bool {
        matches!(self, Self::Submitted | Self::Queued)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DirtyRect {
    pub(crate) bounds: Bounds<ScaledPixels>,
}

impl DirtyRect {
    pub(crate) fn new(bounds: Bounds<ScaledPixels>) -> Option<Self> {
        (!bounds.is_empty()).then_some(Self { bounds })
    }

    pub(crate) fn area(&self) -> f32 {
        f64::from(self.bounds.size.width) as f32 * f64::from(self.bounds.size.height) as f32
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct DirtyRegion {
    rects: SmallVec<[DirtyRect; 8]>,
    full: bool,
}

impl DirtyRegion {
    pub(crate) fn empty() -> Self {
        Self::default()
    }

    #[allow(dead_code)]
    pub(crate) fn full(bounds: Bounds<ScaledPixels>) -> Self {
        let mut region = Self {
            rects: SmallVec::new(),
            full: true,
        };
        region.push(bounds);
        region
    }

    pub(crate) fn push(&mut self, bounds: Bounds<ScaledPixels>) {
        let Some(rect) = DirtyRect::new(bounds) else {
            return;
        };
        if self.full && !self.rects.is_empty() {
            let combined = self.union_bounds().unwrap().union(&rect.bounds);
            self.rects.clear();
            self.rects.push(DirtyRect { bounds: combined });
            return;
        }

        // Keep a disjoint rect union. A direct bounding-box merge turns a titlebar
        // strip crossing a narrow dropdown into a window-sized dirty rectangle.
        let mut pending = SmallVec::<[DirtyRect; 8]>::new();
        pending.push(rect);
        for existing in &self.rects {
            let mut next = SmallVec::<[DirtyRect; 8]>::new();
            for candidate in pending.drain(..) {
                let overlap = candidate.bounds.intersect(&existing.bounds);
                if overlap.is_empty() {
                    next.push(candidate);
                    continue;
                }
                let b = candidate.bounds;
                let x0 = b.origin.x.0;
                let y0 = b.origin.y.0;
                let x1 = x0 + b.size.width.0;
                let y1 = y0 + b.size.height.0;
                let ox0 = overlap.origin.x.0;
                let oy0 = overlap.origin.y.0;
                let ox1 = ox0 + overlap.size.width.0;
                let oy1 = oy0 + overlap.size.height.0;
                // Horizontal strips above/below the overlap, then left/right
                // strips within its vertical interval. Together these partition
                // candidate minus existing exactly, without double counting.
                for (left, top, right, bottom) in [
                    (x0, y0, x1, oy0),
                    (x0, oy1, x1, y1),
                    (x0, oy0, ox0, oy1),
                    (ox1, oy0, x1, oy1),
                ] {
                    if right > left && bottom > top {
                        next.push(DirtyRect {
                            bounds: Bounds::new(
                                crate::Point {
                                    x: ScaledPixels(left),
                                    y: ScaledPixels(top),
                                },
                                crate::Size {
                                    width: ScaledPixels(right - left),
                                    height: ScaledPixels(bottom - top),
                                },
                            ),
                        });
                    }
                }
            }
            pending = next;
            if pending.is_empty() {
                break;
            }
        }
        self.rects.extend(pending);

        // Coalesce only when the union stays rectangular. Never expand an L
        // shape to its bounding box merely because two rectangles overlap.
        let mut index = 0;
        while index < self.rects.len() {
            let a = self.rects[index].bounds;
            let mut next_index = index + 1;
            let mut merged = false;
            while next_index < self.rects.len() {
                let b = self.rects[next_index].bounds;
                let horizontal = a.origin.y == b.origin.y
                    && a.size.height == b.size.height
                    && (a.origin.x + a.size.width == b.origin.x
                        || b.origin.x + b.size.width == a.origin.x);
                let vertical = a.origin.x == b.origin.x
                    && a.size.width == b.size.width
                    && (a.origin.y + a.size.height == b.origin.y
                        || b.origin.y + b.size.height == a.origin.y);
                if horizontal || vertical {
                    self.rects[index].bounds = a.union(&b);
                    self.rects.swap_remove(next_index);
                    merged = true;
                    break;
                }
                next_index += 1;
            }
            if merged {
                index = 0;
            } else {
                index += 1;
            }
        }
        if self.rects.len() > MAX_DIRTY_RECTS
            && let Some(bounds) = self.union_bounds()
        {
            // The bounded fallback is conservative but never omits changed pixels.
            self.rects.clear();
            self.rects.push(DirtyRect { bounds });
        }
    }

    /// Accumulates damage from a presentation packet that has not reached the surface yet.
    pub(crate) fn merge_from(&mut self, previous: &Self) {
        self.full |= previous.full;
        for rect in &previous.rects {
            self.push(rect.bounds);
        }
    }

    pub(crate) fn mark_full(&mut self, bounds: Bounds<ScaledPixels>) {
        self.full = true;
        self.rects.clear();
        self.push(bounds);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    pub(crate) fn is_full(&self) -> bool {
        self.full
    }

    pub(crate) fn rects(&self) -> &[DirtyRect] {
        &self.rects
    }

    pub(crate) fn rect_count(&self) -> usize {
        self.rects.len()
    }

    pub(crate) fn union_bounds(&self) -> Option<Bounds<ScaledPixels>> {
        self.rects
            .iter()
            .map(|rect| rect.bounds)
            .reduce(|bounds, rect| bounds.union(&rect))
    }

    pub(crate) fn area(&self) -> f32 {
        self.rects.iter().map(DirtyRect::area).sum()
    }

    pub(crate) fn coalesce_if_large(
        &mut self,
        viewport: Bounds<ScaledPixels>,
        max_partial_area_ratio: f32,
    ) {
        if self.full || self.rects.is_empty() {
            return;
        }

        let viewport_area =
            f64::from(viewport.size.width) as f32 * f64::from(viewport.size.height) as f32;
        if viewport_area <= 0.0 || self.area() <= viewport_area * max_partial_area_ratio {
            return;
        }

        self.mark_full(viewport);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AnimationDriver, AnimationEngine, AnimationSpec, Easing, ElementId, GlobalElementId, Point,
        TransitionProperty, bounds, size,
    };
    use std::time::Duration;

    fn rect(x: f32, width: f32) -> Bounds<ScaledPixels> {
        bounds(
            Point {
                x: ScaledPixels(x),
                y: ScaledPixels(0.0),
            },
            size(ScaledPixels(width), ScaledPixels(10.0)),
        )
    }

    #[test]
    fn merges_transitively_connected_damage() {
        let mut region = DirtyRegion::empty();
        region.push(rect(0.0, 10.0));
        region.push(rect(20.0, 10.0));
        region.push(rect(5.0, 20.0));

        assert_eq!(region.rect_count(), 1);
        assert_eq!(region.union_bounds(), Some(rect(0.0, 30.0)));
    }

    #[test]
    fn overlapping_cross_damage_does_not_invalidate_bounding_box_corners() {
        let mut region = DirtyRegion::empty();
        region.push(bounds(
            Point { x: ScaledPixels(0.0), y: ScaledPixels(40.0) },
            size(ScaledPixels(100.0), ScaledPixels(20.0)),
        ));
        region.push(bounds(
            Point { x: ScaledPixels(40.0), y: ScaledPixels(0.0) },
            size(ScaledPixels(20.0), ScaledPixels(100.0)),
        ));

        assert_eq!(region.area(), 3600.0);
        assert_eq!(region.rect_count(), 3);
        assert_eq!(region.union_bounds(), Some(bounds(
            Point { x: ScaledPixels(0.0), y: ScaledPixels(0.0) },
            size(ScaledPixels(100.0), ScaledPixels(100.0)),
        )));
    }

    #[test]
    fn repeated_identical_damage_is_not_double_counted() {
        let mut region = DirtyRegion::empty();
        region.push(rect(12.0, 16.0));
        region.push(rect(12.0, 16.0));
        assert_eq!(region.area(), 160.0);
        assert_eq!(region.rect_count(), 1);
    }

    #[test]
    fn merging_queued_packet_damage_preserves_both_scenes() {
        let frame_time = Instant::now();
        let mut previous_damage = DirtyRegion::empty();
        previous_damage.push(rect(0.0, 10.0));
        let mut latest_damage = DirtyRegion::empty();
        latest_damage.push(rect(20.0, 10.0));
        let previous = PresentationPacket::new(
            Arc::new(Scene::default()),
            [],
            [],
            frame_time,
            1.0,
            previous_damage,
            BackdropBlurDamagePlan::default(),
            PartialPresentMode::FullRedraw,
        );
        let mut latest = PresentationPacket::new(
            Arc::new(Scene::default()),
            [],
            [],
            frame_time,
            1.0,
            latest_damage,
            BackdropBlurDamagePlan::default(),
            PartialPresentMode::FullRedraw,
        );

        latest.merge_pending_damage_from(&previous);

        assert_eq!(latest.dirty_region.union_bounds(), Some(rect(0.0, 30.0)));
    }

    #[test]
    fn presentation_packet_deadline_tracks_successful_submission_and_frame_limit() {
        let now = Instant::now();
        let interval = Duration::from_millis(20);
        let mut packet = PresentationPacket::new(
            Arc::new(Scene::default()),
            [],
            [],
            now,
            1.0,
            DirtyRegion::empty(),
            BackdropBlurDamagePlan::default(),
            PartialPresentMode::FullRedraw,
        );
        packet.set_frame_interval(Some(interval));
        assert_eq!(packet.presentation_deadline(), None);
        assert!(packet.presentation_is_due(now));
        packet.record_presentation(now);
        assert_eq!(packet.presentation_deadline(), Some(now + interval));
        assert!(!packet.presentation_is_due(now + interval - Duration::from_nanos(1)));
        assert!(packet.presentation_is_due(now + interval));
        packet.set_frame_interval(None);
        assert_eq!(packet.presentation_deadline(), None);
        assert!(packet.presentation_is_due(now));
    }

    #[test]
    fn bounds_fragmented_damage_metadata() {
        let mut region = DirtyRegion::empty();
        for index in 0..=MAX_DIRTY_RECTS {
            region.push(rect(index as f32 * 20.0, 5.0));
        }

        assert_eq!(region.rect_count(), 1);
        assert_eq!(
            region.union_bounds(),
            Some(rect(0.0, MAX_DIRTY_RECTS as f32 * 20.0 + 5.0))
        );
    }

    #[test]
    fn animation_tick_keeps_ui_ownership_without_forcing_rebuild() {
        let tick = PlatformFrameRequest::animation_tick();
        assert!(tick.needs_ui_commit());
        assert!(!tick.needs_ui_rebuild());
        assert!(!tick.is_presentation_only());
        assert_eq!(tick.merge(PlatformFrameRequest::presentation()), tick);
        assert_eq!(PlatformFrameRequest::presentation().merge(tick), tick);
        assert_eq!(
            tick.merge(PlatformFrameRequest::ui_commit()),
            PlatformFrameRequest::ui_commit_and_presentation(),
        );
        assert_eq!(
            PlatformFrameRequest::ui_commit().merge(tick),
            PlatformFrameRequest::ui_commit_and_presentation(),
        );
    }

    #[test]
    fn pending_frame_requests_coalesce_until_ui_owner_consumes_them() {
        let (sender, receiver) = PlatformFrameRequestSender::channel();
        let mut requests = Box::pin(receiver.into_stream());

        assert!(sender.request(PlatformFrameRequest::ui_commit()));
        assert!(sender.request(PlatformFrameRequest::presentation()));

        let request = futures::executor::block_on(requests.next())
            .expect("a submitted frame request should wake the UI owner");
        assert!(request.needs_ui_commit());
        assert!(request.needs_presentation());
    }

    #[test]
    fn presentation_packet_samples_visual_timeline_without_ui_state() {
        let started_at = Instant::now();
        let element_id = GlobalElementId::from_path(&[ElementId::from("packet-animation")]);
        let mut engine = AnimationEngine::new();
        engine.start_transition(
            &element_id,
            TransitionProperty::Opacity,
            AnimationSpec::new(Duration::from_secs(1))
                .ease(Easing::Linear)
                .driver(AnimationDriver::Gpu),
            started_at,
        );
        assert!(engine.bind_scene_animation(
            &element_id,
            TransitionProperty::Opacity,
            SceneAnimationId(103),
            [0.0; 4],
            [1.0, 0.0, 0.0, 0.0],
        ));
        assert!(engine.set_transition_bounds(
            &element_id,
            TransitionProperty::Opacity,
            bounds(Point::default(), size(crate::px(20.0), crate::px(20.0))),
        ));

        let mut packet = PresentationPacket::new(
            Arc::new(Scene::default()),
            [],
            engine.presentation_timelines(),
            started_at,
            1.0,
            DirtyRegion::empty(),
            BackdropBlurDamagePlan::default(),
            PartialPresentMode::FullRedraw,
        );

        packet.sample_animations(started_at + Duration::from_millis(250));
        assert!(packet.sampled_active_presentation_animation);
        let first_progress = packet.presentation_animation_values[0].progress;
        packet.sample_animations(started_at + Duration::from_millis(750));
        let later_progress = packet.presentation_animation_values[0].progress;

        assert!((first_progress - 0.25).abs() < 0.001);
        assert!((later_progress - 0.75).abs() < 0.001);
        assert_eq!(engine.active_count(), 1);

        packet.consume_submitted_damage();
        let completions = packet.sample_animations(started_at + Duration::from_secs(2));
        assert_eq!(completions.len(), 1);
        assert!(packet.presentation_animation_timelines.is_empty());
        // A deferred final draw must remain schedulable even after its timeline settles.
        assert!(packet.has_pending_presentation());
        packet.sample_animations(started_at + Duration::from_secs(3));
        assert!(packet.has_pending_presentation());
        packet.consume_submitted_damage();
        assert!(!packet.has_pending_presentation());
    }

    #[test]
    fn submitted_animation_damage_does_not_accumulate_across_frames() {
        let started_at = Instant::now();
        let element_id = GlobalElementId::from_path(&[ElementId::from("blur-damage")]);
        let animation_id = SceneAnimationId(1 << 31);
        let mut engine = AnimationEngine::new();
        engine.start_transition(
            &element_id,
            TransitionProperty::Translation,
            AnimationSpec::new(Duration::from_secs(1))
                .ease(Easing::Linear)
                .driver(AnimationDriver::Gpu),
            started_at,
        );
        assert!(engine.bind_scene_animation(
            &element_id,
            TransitionProperty::Translation,
            animation_id,
            [0.0; 4],
            [100.0, 0.0, 0.0, 0.0]
        ));
        let viewport = rect(0.0, 500.0);
        let mut scene = Scene::default();
        scene.insert_animated_primitive(
            crate::Quad {
                bounds: rect(0.0, 10.0),
                content_mask: crate::ContentMask::new(viewport),
                ..Default::default()
            },
            animation_id,
        );
        scene.insert_primitive(crate::PaintBackdropBlur {
            order: 1,
            animation_id: None,
            bounds: viewport,
            content_mask: crate::ContentMask::new(viewport),
            corner_radii: Default::default(),
            radius: ScaledPixels(8.0),
            downsample: 2,
            levels: 3,
            recompute_overlap: false,
            saturation: 1.0,
            opacity: 1.0,
            tint: None,
        });
        let mut packet = PresentationPacket::new(
            Arc::new(scene),
            [],
            engine.presentation_timelines(),
            started_at,
            1.0,
            DirtyRegion::empty(),
            BackdropBlurDamagePlan::default(),
            PartialPresentMode::FullRedraw,
        );
        for index in 1..=40 {
            packet.sample_animations(started_at + Duration::from_millis(index * 10));
            assert!(packet.backdrop_blur_damage_plan.refresh_required());
            let (full, damage) = packet
                .backdrop_blur_damage_plan
                .source_damage_for_orders(0, u32::MAX);
            assert!(
                !full,
                "successfully submitted damage must not become sticky full refresh"
            );
            assert!(damage.count() <= 2);
            packet.consume_submitted_damage();
            assert!(packet.dirty_region.is_empty());
            assert!(packet.backdrop_blur_damage_plan.is_empty());
        }
        // A deferred frame does not consume damage. Keep every swept region until submission.
        for index in 41..=60 {
            packet.sample_animations(started_at + Duration::from_millis(index * 10));
        }
        assert!(
            packet
                .backdrop_blur_damage_plan
                .source_damage_for_orders(0, u32::MAX)
                .0
        );
        packet.consume_submitted_damage();
        packet.sample_animations(started_at + Duration::from_millis(610));
        assert!(packet.backdrop_blur_damage_plan.refresh_required());
        assert!(
            !packet
                .backdrop_blur_damage_plan
                .source_damage_for_orders(0, u32::MAX)
                .0
        );
    }

    #[test]
    fn presentation_packet_reports_completion_at_first_sample_after_endpoint() {
        let started_at = Instant::now();
        let element_id = GlobalElementId::from_path(&[ElementId::from("packet-completion")]);
        let mut engine = AnimationEngine::new();
        engine.start_transition(
            &element_id,
            TransitionProperty::Opacity,
            AnimationSpec::new(Duration::from_millis(1))
                .ease(Easing::Linear)
                .driver(AnimationDriver::Gpu),
            started_at,
        );
        assert!(engine.bind_scene_animation(
            &element_id,
            TransitionProperty::Opacity,
            SceneAnimationId(104),
            [0.0; 4],
            [1.0, 0.0, 0.0, 0.0],
        ));

        let mut packet = PresentationPacket::new(
            Arc::new(Scene::default()),
            [],
            engine.presentation_timelines(),
            started_at,
            1.0,
            DirtyRegion::empty(),
            BackdropBlurDamagePlan::default(),
            PartialPresentMode::FullRedraw,
        );

        let completions = packet.sample_animations(started_at + Duration::from_millis(2));

        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].animation_id, SceneAnimationId(104));
        assert!(packet.presentation_animation_timelines.is_empty());
        assert!(packet.sampled_active_presentation_animation);
        assert_eq!(packet.presentation_animation_values[0].progress, 1.0);
        assert_eq!(engine.active_count(), 1);
    }

    #[test]
    fn presentation_packet_keeps_two_horizontal_springs_in_motion() {
        let now = Instant::now();
        let element_id = GlobalElementId::from_path(&[ElementId::from("pill-springs")]);
        let animation_id = SceneAnimationId(120);
        let mut engine = AnimationEngine::new();
        for (property, stiffness, damping) in [
            (TransitionProperty::HorizontalEdgeFirst, 341.5, 22.2),
            (TransitionProperty::HorizontalEdgeSecond, 223.8, 23.9),
        ] {
            engine.start_transition(
                &element_id,
                property,
                AnimationSpec::new(Duration::ZERO).driver(AnimationDriver::Paint),
                now,
            );
            assert!(engine.bind_scene_animation(
                &element_id,
                property,
                animation_id,
                [-205.0, 0.0, 0.0, 0.0],
                [0.0; 4]
            ));
            engine.set_transition_spring(
                &element_id,
                property,
                crate::Spring {
                    physics: crate::SpringPhysics {
                        stiffness,
                        damping,
                        mass: 1.0,
                    },
                    settle_position: 0.001,
                    settle_velocity: 0.001,
                },
            );
            assert!(engine.set_grouped_visual_scene_animation(
                &element_id,
                property,
                animation_id,
                true
            ));
        }
        let mut packet = PresentationPacket::new(
            Arc::new(Scene::default()),
            engine.scene_values(now),
            engine.presentation_timelines(),
            now,
            1.0,
            DirtyRegion::empty(),
            BackdropBlurDamagePlan::default(),
            PartialPresentMode::FullRedraw,
        );
        packet.sample_animations(now + Duration::from_millis(90));
        assert_eq!(packet.presentation_animation_values.len(), 1);
        let early = packet.presentation_animation_values[0];
        assert_eq!(early.property, TransitionProperty::HorizontalEdges);
        assert!(early.from[0] < -1.0 && early.from[1] < -1.0);
        assert!((early.from[0] - early.from[1]).abs() > 1.0);
        packet.sample_animations(now + Duration::from_millis(180));
        let later = packet.presentation_animation_values[0];
        assert_ne!(early.from, later.from);
        assert!(!packet.presentation_animation_timelines.is_empty());
    }

    #[test]
    fn presentation_packet_packs_parallel_tracks_and_keeps_independent_progress() {
        let started_at = Instant::now();
        let element_id = GlobalElementId::from_path(&[ElementId::from("parallel-packet")]);
        let animation_id = SceneAnimationId(105);
        let mut engine = AnimationEngine::new();
        let tracks = [
            (
                TransitionProperty::Opacity,
                AnimationSpec::new(Duration::from_millis(200))
                    .ease(Easing::Linear)
                    .driver(AnimationDriver::Gpu),
                [0.0; 4],
                [1.0, 0.0, 0.0, 0.0],
            ),
            (
                TransitionProperty::Translation,
                AnimationSpec::new(Duration::from_millis(800))
                    .ease(Easing::Linear)
                    .driver(AnimationDriver::Gpu),
                [0.0; 4],
                [80.0, 40.0, 0.0, 0.0],
            ),
            (
                TransitionProperty::Scale,
                AnimationSpec::new(Duration::from_millis(1200))
                    .ease(Easing::Linear)
                    .driver(AnimationDriver::Gpu),
                [0.5, 0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0, 0.0],
            ),
        ];

        for (property, spec, from, to) in tracks {
            engine.start_transition(&element_id, property, spec, started_at);
            assert!(engine.bind_scene_animation(&element_id, property, animation_id, from, to));
            assert!(engine.set_grouped_visual_scene_animation(
                &element_id,
                property,
                animation_id,
                true,
            ));
        }

        let mut packet = PresentationPacket::new(
            Arc::new(Scene::default()),
            engine.scene_values(started_at),
            engine.presentation_timelines(),
            started_at,
            1.0,
            DirtyRegion::empty(),
            BackdropBlurDamagePlan::default(),
            PartialPresentMode::FullRedraw,
        );

        packet.sample_animations(started_at + Duration::from_millis(100));
        assert_eq!(packet.presentation_animation_values.len(), 1);
        let initial = packet.presentation_animation_values[0];
        assert_eq!(initial.property, TransitionProperty::VisualState);
        assert!((initial.from[0] - 10.0).abs() < 0.001);
        assert!((initial.from[1] - 5.0).abs() < 0.001);
        assert!((initial.from[2] - (0.5 + 0.5 * (100.0 / 1200.0))).abs() < 0.001);
        assert!((initial.from[3] - 0.5).abs() < 0.001);

        let completions = packet.sample_animations(started_at + Duration::from_millis(500));
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].property, TransitionProperty::Opacity);
        engine
            .complete_scene_animation(&completions[0])
            .expect("the opacity completion must match its own track");
        assert!(
            !engine.scene_animation_track_is_active(animation_id, TransitionProperty::Opacity,)
        );
        assert!(engine.scene_animation_track_is_bound(animation_id, TransitionProperty::Opacity,));
        assert!(
            engine.scene_animation_track_is_active(animation_id, TransitionProperty::Translation,)
        );
        assert!(engine.scene_animation_track_is_active(animation_id, TransitionProperty::Scale,));
        assert_eq!(packet.presentation_animation_values.len(), 1);
        let later = packet.presentation_animation_values[0];
        assert_eq!(later.property, TransitionProperty::VisualState);
        assert_eq!(later.from[3], 1.0);
        assert!((later.from[0] - 50.0).abs() < 0.001);
        assert!((later.from[1] - 25.0).abs() < 0.001);
        assert!((later.from[2] - (0.5 + 0.5 * (500.0 / 1200.0))).abs() < 0.001);
    }

    #[test]
    fn completed_parallel_tracks_keep_one_packed_renderer_value() {
        let started_at = Instant::now();
        let element_id = GlobalElementId::from_path(&[ElementId::from("completed-parallel")]);
        let animation_id = SceneAnimationId(106);
        let mut engine = AnimationEngine::new();
        for (property, from, to) in [
            (
                TransitionProperty::Opacity,
                [1.0, 0.0, 0.0, 0.0],
                [0.25, 0.0, 0.0, 0.0],
            ),
            (
                TransitionProperty::Translation,
                [0.0; 4],
                [24.0, 12.0, 0.0, 0.0],
            ),
        ] {
            engine.start_transition(
                &element_id,
                property,
                AnimationSpec::new(Duration::from_millis(100))
                    .ease(Easing::Linear)
                    .driver(AnimationDriver::Gpu),
                started_at,
            );
            assert!(engine.bind_scene_animation(&element_id, property, animation_id, from, to));
            assert!(engine.set_grouped_visual_scene_animation(
                &element_id,
                property,
                animation_id,
                true,
            ));
        }

        let mut packet = PresentationPacket::new(
            Arc::new(Scene::default()),
            [],
            engine.presentation_timelines(),
            started_at,
            1.0,
            DirtyRegion::empty(),
            BackdropBlurDamagePlan::default(),
            PartialPresentMode::FullRedraw,
        );
        let completions = packet.sample_animations(started_at + Duration::from_millis(150));
        assert_eq!(completions.len(), 2);
        for completion in &completions {
            engine
                .complete_scene_animation(completion)
                .expect("the matching compositor track should complete");
            assert!(
                engine
                    .scene_animation_track_is_bound(completion.animation_id, completion.property,)
            );
        }

        let completed_packet = PresentationPacket::new(
            Arc::new(Scene::default()),
            engine.scene_values(started_at + Duration::from_millis(150)),
            engine.presentation_timelines(),
            started_at + Duration::from_millis(150),
            1.0,
            DirtyRegion::empty(),
            BackdropBlurDamagePlan::default(),
            PartialPresentMode::FullRedraw,
        );
        assert_eq!(completed_packet.presentation_animation_values.len(), 1);
        let completed = completed_packet.presentation_animation_values[0];
        assert_eq!(completed.property, TransitionProperty::VisualState);
        assert_eq!(completed.from, [24.0, 12.0, 1.0, 0.25]);
    }

    #[test]
    fn repeating_spring_restarts_from_its_settled_sample_on_presentation_owner() {
        let started_at = Instant::now();
        let element_id = GlobalElementId::from_path(&[ElementId::from("repeating-spring")]);
        let mut engine = AnimationEngine::new();
        engine.start_transition(
            &element_id,
            TransitionProperty::Opacity,
            AnimationSpec::new(Duration::ZERO)
                .repeat(crate::RepeatMode::Forever)
                .driver(AnimationDriver::Gpu),
            started_at,
        );
        engine.set_transition_spring(
            &element_id,
            TransitionProperty::Opacity,
            crate::Spring::default(),
        );
        assert!(engine.bind_scene_animation(
            &element_id,
            TransitionProperty::Opacity,
            SceneAnimationId(107),
            [0.0; 4],
            [1.0, 0.0, 0.0, 0.0],
        ));

        let mut packet = PresentationPacket::new(
            Arc::new(Scene::default()),
            [],
            engine.presentation_timelines(),
            started_at,
            1.0,
            DirtyRegion::empty(),
            BackdropBlurDamagePlan::default(),
            PartialPresentMode::FullRedraw,
        );
        let settled_at = started_at + Duration::from_secs(20);
        let completions = packet.sample_animations(settled_at);
        assert!(completions.is_empty());
        assert_eq!(packet.presentation_animation_timelines.len(), 1);
        assert_eq!(packet.presentation_animation_values[0].progress, 1.0);

        packet.sample_animations(settled_at + Duration::from_millis(16));
        assert_eq!(packet.presentation_animation_timelines.len(), 1);
        assert_ne!(packet.presentation_animation_values[0].progress, 1.0);
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PartialPresentMode {
    #[default]
    FullRedraw,
    Partial,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum RetainedResourceTrimPolicy {
    #[default]
    None,
    Light,
    Strong,
}

/// Lifetime-free payload submitted from the UI commit domain to presentation.
///
/// The retained Scene is shared through Arc; all frame-varying metadata is snapshotted by value.
/// The platform/renderer boundary therefore owns everything it needs to enqueue this packet
/// without borrowing Window state.
pub(crate) struct PresentationPacket {
    pub(crate) window_id: u64,
    pub(crate) scene: Arc<Scene>,
    pub(crate) presentation_animation_values: SmallVec<[SceneAnimationValue; 4]>,
    pub(crate) sampled_active_presentation_animation: bool,
    previous_presentation_animation_values: SmallVec<[SceneAnimationValue; 4]>,
    presentation_animation_tracks: SmallVec<[SceneAnimationValue; 4]>,
    grouped_visual_animation_ids: FxHashSet<SceneAnimationId>,
    timeline_track_keys: FxHashSet<(SceneAnimationId, TransitionProperty)>,
    packed_visual_samples: FxHashMap<SceneAnimationId, PackedVisualSample>,
    pub(crate) presentation_animation_timelines: SmallVec<[SceneAnimationTimeline; 4]>,
    pub(crate) frame_time: Instant,
    pub(crate) scale_factor: f32,
    pub(crate) dirty_region: DirtyRegion,
    pub(crate) backdrop_blur_damage_plan: BackdropBlurDamagePlan,
    pub(crate) partial_present_mode: PartialPresentMode,
    pub(crate) force_full_backdrop_blur_refresh: bool,
    frame_interval: Option<Duration>,
    last_presented_at: Option<Instant>,
}

#[derive(Clone, Copy)]
struct PackedVisualSample {
    value: [f32; 4],
    seen_properties: u8,
    supported: bool,
    emitted: bool,
}

impl PresentationPacket {
    /// Keep the final animation sample scheduled until its damage reaches the surface.
    pub(crate) fn has_pending_presentation(&self) -> bool {
        !self.presentation_animation_timelines.is_empty() || !self.dirty_region.is_empty()
    }

    /// Consume damage only after the renderer has successfully submitted this sample.
    /// Deferred submissions keep accumulating damage until it reaches the surface.
    pub(crate) fn consume_submitted_damage(&mut self) {
        self.dirty_region = DirtyRegion::empty();
        self.backdrop_blur_damage_plan = BackdropBlurDamagePlan::default();
        self.force_full_backdrop_blur_refresh = false;
    }

    /// Preserves unsubmitted damage when a newer scene supersedes this packet in the owner queue.
    pub(crate) fn merge_pending_damage_from(&mut self, previous: &Self) {
        self.dirty_region.merge_from(&previous.dirty_region);
        self.backdrop_blur_damage_plan
            .merge_from(&previous.backdrop_blur_damage_plan);
        self.force_full_backdrop_blur_refresh |= previous.force_full_backdrop_blur_refresh;
    }

    pub(crate) fn new(
        scene: Arc<Scene>,
        presentation_animation_values: impl IntoIterator<Item = SceneAnimationValue>,
        presentation_animation_timelines: impl IntoIterator<Item = SceneAnimationTimeline>,
        frame_time: Instant,
        scale_factor: f32,
        dirty_region: DirtyRegion,
        backdrop_blur_damage_plan: BackdropBlurDamagePlan,
        partial_present_mode: PartialPresentMode,
    ) -> Self {
        let presentation_animation_tracks: SmallVec<[SceneAnimationValue; 4]> =
            presentation_animation_values.into_iter().collect();
        let presentation_animation_timelines = presentation_animation_timelines
            .into_iter()
            .collect::<SmallVec<[SceneAnimationTimeline; 4]>>();
        let mut grouped_visual_animation_ids: FxHashSet<SceneAnimationId> =
            presentation_animation_timelines
                .iter()
                .filter(|timeline| timeline.is_grouped_visual())
                .map(SceneAnimationTimeline::animation_id)
                .collect();
        let mut completed_track_properties = FxHashMap::<SceneAnimationId, u8>::default();
        for track in &presentation_animation_tracks {
            let property_bit = visual_property_bit(track.property);
            if property_bit == 0 {
                continue;
            }
            let seen_properties = completed_track_properties
                .entry(track.animation_id)
                .or_default();
            if *seen_properties != 0 && *seen_properties & property_bit == 0 {
                grouped_visual_animation_ids.insert(track.animation_id);
            }
            *seen_properties |= property_bit;
        }
        let mut presentation_animation_values = SmallVec::new();
        let mut packed_visual_samples = FxHashMap::default();
        coalesce_visual_animation_tracks(
            &presentation_animation_tracks,
            &grouped_visual_animation_ids,
            &mut packed_visual_samples,
            &mut presentation_animation_values,
        );
        Self {
            window_id: 0,
            scene,
            presentation_animation_values,
            sampled_active_presentation_animation: false,
            previous_presentation_animation_values: SmallVec::new(),
            presentation_animation_tracks,
            grouped_visual_animation_ids,
            timeline_track_keys: FxHashSet::default(),
            packed_visual_samples,
            presentation_animation_timelines,
            frame_time,
            scale_factor,
            dirty_region,
            backdrop_blur_damage_plan,
            partial_present_mode,
            force_full_backdrop_blur_refresh: false,
            frame_interval: None,
            last_presented_at: None,
        }
    }

    pub(crate) fn set_frame_interval(&mut self, frame_interval: Option<Duration>) {
        self.frame_interval = frame_interval;
    }

    pub(crate) fn presentation_is_due(&self, now: Instant) -> bool {
        self.frame_interval
            .zip(self.last_presented_at)
            .is_none_or(|(interval, last_presented_at)| now >= last_presented_at + interval)
    }

    pub(crate) fn presentation_deadline(&self) -> Option<Instant> {
        self.frame_interval
            .zip(self.last_presented_at)
            .map(|(interval, last)| last + interval)
    }

    pub(crate) fn record_presentation(&mut self, presented_at: Instant) {
        self.last_presented_at = Some(presented_at);
    }

    /// Sample scene-owned visual timelines without consulting mutable UI state.
    pub(crate) fn sample_animations(
        &mut self,
        now: Instant,
    ) -> SmallVec<[SceneAnimationCompletion; 4]> {
        self.frame_time = now;
        self.sampled_active_presentation_animation =
            !self.presentation_animation_timelines.is_empty();
        if self.presentation_animation_timelines.is_empty() {
            return SmallVec::new();
        }

        std::mem::swap(
            &mut self.presentation_animation_values,
            &mut self.previous_presentation_animation_values,
        );
        self.presentation_animation_values.clear();

        self.timeline_track_keys.clear();
        self.timeline_track_keys.extend(
            self.presentation_animation_timelines
                .iter()
                .map(SceneAnimationTimeline::track_key),
        );
        let timeline_track_keys = &self.timeline_track_keys;
        self.presentation_animation_tracks
            .retain(|value| !timeline_track_keys.contains(&(value.animation_id, value.property)));
        let mut completion_events = SmallVec::new();
        let scale_factor = self.scale_factor;
        let timelines = &mut self.presentation_animation_timelines;
        let tracks = &mut self.presentation_animation_tracks;
        let dirty_region = &mut self.dirty_region;
        timelines.retain_mut(|timeline| {
            let sample = timeline.sample_at(now);
            if let Some(value) = sample.value {
                tracks.push(value);
            }
            if let Some(completion) = sample.completion {
                completion_events.push(completion);
            }
            if let Some(bounds) = timeline.bounds() {
                dirty_region.push(bounds.scale(scale_factor));
            }
            !sample.done
        });

        coalesce_visual_animation_tracks(
            &self.presentation_animation_tracks,
            &self.grouped_visual_animation_ids,
            &mut self.packed_visual_samples,
            &mut self.presentation_animation_values,
        );
        let animation_damage = self.scene.backdrop_blur_animation_damage_plan(
            &self.previous_presentation_animation_values,
            &self.presentation_animation_values,
        );
        self.backdrop_blur_damage_plan.merge_from(&animation_damage);
        for bounds in self
            .scene
            .backdrop_blur_output_damage(&self.backdrop_blur_damage_plan)
        {
            self.dirty_region.push(bounds);
        }

        completion_events
    }
}

fn coalesce_visual_animation_tracks(
    tracks: &[SceneAnimationValue],
    grouped_ids: &FxHashSet<SceneAnimationId>,
    packed: &mut FxHashMap<SceneAnimationId, PackedVisualSample>,
    values: &mut SmallVec<[SceneAnimationValue; 4]>,
) {
    values.clear();
    packed.clear();
    if grouped_ids.is_empty() {
        values.extend_from_slice(tracks);
        return;
    }

    for track in tracks
        .iter()
        .filter(|track| grouped_ids.contains(&track.animation_id))
    {
        let entry = packed
            .entry(track.animation_id)
            .or_insert(PackedVisualSample {
                value: [0.0, 0.0, 1.0, 1.0],
                seen_properties: 0,
                supported: true,
                emitted: false,
            });
        let property_bit = visual_property_bit(track.property);
        if property_bit == 0 || entry.seen_properties & property_bit != 0 {
            entry.supported = false;
            continue;
        }
        entry.seen_properties |= property_bit;
        let progress = if track.progress.is_finite() {
            track.progress
        } else {
            0.0
        };
        let sampled = std::array::from_fn::<_, 4, _>(|index| {
            track.from[index] + (track.to[index] - track.from[index]) * progress
        });
        match track.property {
            TransitionProperty::HorizontalEdgeFirst => entry.value[0] = finite_or(sampled[0], 0.0),
            TransitionProperty::HorizontalEdgeSecond => entry.value[1] = finite_or(sampled[0], 0.0),
            TransitionProperty::Opacity => {
                entry.value[3] = finite_or(sampled[0], 1.0).clamp(0.0, 1.0);
            }
            TransitionProperty::Translation if track.from[3] != 1.0 && track.to[3] != 1.0 => {
                entry.value[0] = finite_or(sampled[0], 0.0);
                entry.value[1] = finite_or(sampled[1], 0.0);
            }
            TransitionProperty::Scale => entry.value[2] = finite_or(sampled[0], 1.0).max(0.0),
            _ => entry.supported = false,
        }
    }

    for track in tracks {
        let Some(sample) = packed.get_mut(&track.animation_id) else {
            values.push(*track);
            continue;
        };
        if !sample.supported {
            values.push(*track);
            continue;
        }
        if sample.emitted {
            continue;
        }
        sample.emitted = true;
        // A group with one currently applicable property is still composed. This matters while
        // sibling tracks are delayed or have a non-forwards fill mode.
        values.push(SceneAnimationValue {
            animation_id: track.animation_id,
            property: if sample.seen_properties & 24 != 0 {
                TransitionProperty::HorizontalEdges
            } else {
                TransitionProperty::VisualState
            },
            progress: 1.0,
            from: sample.value,
            to: sample.value,
        });
    }
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

fn visual_property_bit(property: TransitionProperty) -> u8 {
    match property {
        TransitionProperty::Opacity => 1,
        TransitionProperty::Translation => 2,
        TransitionProperty::Scale => 4,
        TransitionProperty::HorizontalEdgeFirst => 8,
        TransitionProperty::HorizontalEdgeSecond => 16,
        _ => 0,
    }
}

/// Coalesces native frame wakeups before they cross into the UI owner.
#[derive(Clone)]
pub(crate) struct PlatformFrameRequestSender {
    pending: Arc<Mutex<Option<PlatformFrameRequest>>>,
    wakeups: mpsc::UnboundedSender<()>,
}

pub(crate) struct PlatformFrameRequestReceiver {
    pending: Arc<Mutex<Option<PlatformFrameRequest>>>,
    wakeups: mpsc::UnboundedReceiver<()>,
}

impl PlatformFrameRequestSender {
    pub(crate) fn channel() -> (Self, PlatformFrameRequestReceiver) {
        let pending = Arc::new(Mutex::new(None));
        let (wakeups, wakeup_receiver) = mpsc::unbounded();
        (
            Self {
                pending: pending.clone(),
                wakeups,
            },
            PlatformFrameRequestReceiver {
                pending,
                wakeups: wakeup_receiver,
            },
        )
    }

    /// Merge requests while the UI owner is busy and wake it only once per pending batch.
    pub(crate) fn request(&self, request: PlatformFrameRequest) -> bool {
        let should_wake = {
            let mut pending = self.pending.lock();
            match *pending {
                Some(previous) => {
                    *pending = Some(previous.merge(request));
                    false
                }
                None => {
                    *pending = Some(request);
                    true
                }
            }
        };

        !should_wake || self.wakeups.unbounded_send(()).is_ok()
    }
}

impl PlatformFrameRequestReceiver {
    pub(crate) fn into_stream(self) -> impl Stream<Item = PlatformFrameRequest> + 'static {
        futures::stream::unfold(self, |mut receiver| async move {
            receiver.wakeups.next().await?;
            let request = receiver.pending.lock().take()?;
            Some((request, receiver))
        })
    }
}

pub(crate) struct ActivePresentationFrame {
    pub(crate) continues: bool,
    pub(crate) completed_animations: SmallVec<[SceneAnimationCompletion; 4]>,
}

/// Scheduling context for an active presentation initiated by a paced platform event.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ActivePresentationTiming {
    pub(crate) frame_pacing_wait: Duration,
    pub(crate) vsync_event_queue_delay: Duration,
    pub(crate) window_dispatch_delay: Duration,
    pub(crate) frame_started_at: Instant,
    pub(crate) renderer_scene_prepare: Duration,
    pub(crate) submission_prepare: Duration,
    pub(crate) retained_resource_prepare: Duration,
    pub(crate) frame_prepare_upload: Duration,
    pub(crate) draw_step_prepare: Duration,
    pub(crate) buffer_upload: Duration,
    pub(crate) atlas_upload: Duration,
    pub(crate) offscreen_render: Duration,
    pub(crate) backend_present: Duration,
    pub(crate) renderer_post_present: Duration,
}

pub(crate) type SceneAnimationCompletionSender =
    futures::channel::mpsc::UnboundedSender<SceneAnimationCompletion>;

// On Windows/Linux/FreeBSD the entire presentation payload must be movable to a renderer thread.
// macOS Scene still contains CoreVideo surface attachments, so that backend is intentionally not
// included in this contract until those platform-local attachments are split from Scene.
#[cfg(not(target_os = "macos"))]
#[allow(dead_code)]
fn assert_presentation_packet_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    fn assert_send<T: Send>() {}
    assert_send_sync::<Scene>();
    assert_send_sync::<PresentationPacket>();
    assert_send_sync::<PlatformFrameRequestSender>();
    assert_send::<SceneAnimationCompletion>();
    assert_send::<SceneAnimationCompletionSender>();
    assert_send::<crate::PlatformInput>();
    assert_send::<crate::WindowParams>();
    assert_send::<crate::RendererOptions>();
    assert_send::<crate::ImagePipelineConfig>();
    assert_send::<crate::DefaultFontConfig>();
    assert_send::<crate::WindowIconSource>();
    assert_send::<crate::AnimationEngine>();
    assert_send::<crate::AnimationTick>();
}
