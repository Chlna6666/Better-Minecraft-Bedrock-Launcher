use super::*;
use crate::{AnimationSpec, SceneAnimationId, TransitionProperty};

impl Window {
    /// Start an engine-owned sequence timeline and schedule its first frame.
    pub fn start_animation_sequence(&self, sequence: AnimationSequence) -> AnimationGroupId {
        let (group_id, driver) = {
            let mut engine = self.animation_engine.borrow_mut();
            let group_id = engine.start_sequence(sequence, self.animation_time());
            let driver = engine
                .group_driver(group_id)
                .unwrap_or(AnimationDriver::Auto);
            (group_id, driver)
        };
        self.request_animation_engine_frame(driver);
        group_id
    }

    /// Start an engine-owned parallel timeline and schedule its first frame.
    pub fn start_animation_parallel(&self, parallel: AnimationParallel) -> AnimationGroupId {
        let (group_id, driver) = {
            let mut engine = self.animation_engine.borrow_mut();
            let group_id = engine.start_parallel(parallel, self.animation_time());
            let driver = engine
                .group_driver(group_id)
                .unwrap_or(AnimationDriver::Auto);
            (group_id, driver)
        };
        self.request_animation_engine_frame(driver);
        group_id
    }
    /// Start an engine-owned stagger timeline and schedule its first frame.
    pub fn start_animation_stagger(&self, stagger: AnimationStagger) -> AnimationGroupId {
        let (group_id, driver) = {
            let mut engine = self.animation_engine.borrow_mut();
            let group_id = engine.start_stagger(stagger, self.animation_time());
            let driver = engine
                .group_driver(group_id)
                .unwrap_or(AnimationDriver::Auto);
            (group_id, driver)
        };
        self.request_animation_engine_frame(driver);
        group_id
    }

    /// Sample an engine-owned animation group at the current window animation time.
    pub fn sample_animation_group(
        &self,
        group_id: AnimationGroupId,
    ) -> Option<AnimationGroupSample> {
        self.animation_engine
            .borrow()
            .sample_group(group_id, self.animation_time())
    }

    /// Cancel an engine-owned animation group.
    pub fn cancel_animation_group(&self, group_id: AnimationGroupId) -> bool {
        self.animation_engine.borrow_mut().cancel_group(group_id)
    }

    /// Associate dirty visual bounds with an engine-owned animation group.
    pub fn set_animation_group_bounds(
        &self,
        group_id: AnimationGroupId,
        bounds: Bounds<Pixels>,
    ) -> bool {
        self.animation_engine
            .borrow_mut()
            .set_group_bounds(group_id, bounds)
    }

    /// Start a renderer-owned scene animation. Translation endpoints are already resolved to
    /// device pixels by `AnimationProperty::resolved_values`; do not scale them a second time here.
    pub(crate) fn start_scene_animation(
        &self,
        element_id: &GlobalElementId,
        property: TransitionProperty,
        spec: AnimationSpec,
        bounds: Bounds<Pixels>,
        from: [f32; 4],
        to: [f32; 4],
    ) -> SceneAnimationId {
        let animation_id = SceneAnimationId(self.next_scene_animation_id.get());
        self.next_scene_animation_id
            .set(self.next_scene_animation_id.get().wrapping_add(1));
        self.start_scene_animation_with_id(
            element_id,
            property,
            spec,
            bounds,
            from,
            to,
            animation_id,
            false,
        );
        animation_id
    }

    pub(crate) fn start_grouped_scene_animation(
        &self,
        element_id: &GlobalElementId,
        property: TransitionProperty,
        spec: AnimationSpec,
        bounds: Bounds<Pixels>,
        from: [f32; 4],
        to: [f32; 4],
        animation_id: Option<SceneAnimationId>,
    ) -> SceneAnimationId {
        let animation_id = animation_id.unwrap_or_else(|| {
            let animation_id = SceneAnimationId(self.next_scene_animation_id.get());
            self.next_scene_animation_id
                .set(self.next_scene_animation_id.get().wrapping_add(1));
            animation_id
        });
        self.start_scene_animation_with_id(
            element_id,
            property,
            spec,
            bounds,
            from,
            to,
            animation_id,
            true,
        );
        animation_id
    }

    pub(crate) fn start_scene_animation_with_id(
        &self,
        element_id: &GlobalElementId,
        property: TransitionProperty,
        spec: AnimationSpec,
        bounds: Bounds<Pixels>,
        from: [f32; 4],
        to: [f32; 4],
        animation_id: SceneAnimationId,
        grouped_visual: bool,
    ) {
        let mut engine = self.animation_engine.borrow_mut();
        engine.start_transition(element_id, property, spec, self.animation_time());
        engine.set_transition_bounds(element_id, property, bounds);
        engine.bind_scene_animation(element_id, property, animation_id, from, to);
        engine.set_grouped_visual_scene_animation(
            element_id,
            property,
            animation_id,
            grouped_visual,
        );
        let driver = engine
            .transition_driver(element_id, property)
            .unwrap_or(crate::AnimationDriver::Paint);
        drop(engine);
        self.scene_animation_needs_commit.set(true);
        self.request_animation_engine_frame(driver);
    }

    pub(crate) fn translate_scene_animation_origin(
        &self,
        element_id: &GlobalElementId,
        property: TransitionProperty,
        delta: [f32; 2],
        dirty_bounds: Bounds<Pixels>,
    ) {
        let mut engine = self.animation_engine.borrow_mut();
        if engine.translate_scene_animation_origin(element_id, property, delta) {
            engine.set_transition_bounds(element_id, property, dirty_bounds);
            self.scene_animation_needs_commit.set(true);
        }
    }

    pub(crate) fn retarget_scene_animation(
        &self,
        element_id: &GlobalElementId,
        property: TransitionProperty,
        animation_id: SceneAnimationId,
        spec: AnimationSpec,
        spring: Option<crate::Spring>,
        old_bounds: Bounds<Pixels>,
        new_bounds: Bounds<Pixels>,
        dirty_bounds: Bounds<Pixels>,
        to: [f32; 4],
    ) -> bool {
        let scale_factor = self.scale_factor();
        let base_translation_delta = [
            (old_bounds.origin.x.0 - new_bounds.origin.x.0) * scale_factor,
            (old_bounds.origin.y.0 - new_bounds.origin.y.0) * scale_factor,
        ];
        let mut engine = self.animation_engine.borrow_mut();
        let retargeted = engine.retarget_scene_animation(
            element_id,
            property,
            animation_id,
            spec,
            spring,
            self.animation_time(),
            dirty_bounds,
            base_translation_delta,
            to,
        );
        let driver = retargeted
            .then(|| engine.transition_driver(element_id, property))
            .flatten();
        drop(engine);

        if retargeted {
            self.scene_animation_needs_commit.set(true);
        }
        if let Some(driver) = driver {
            self.request_animation_engine_frame(driver);
        }
        retargeted
    }

    pub(crate) fn set_scene_animation_spring(
        &self,
        element_id: &GlobalElementId,
        property: TransitionProperty,
        spring: crate::Spring,
    ) {
        self.animation_engine
            .borrow_mut()
            .set_transition_spring(element_id, property, spring);
    }

    pub(crate) fn scene_animation_track_is_active(
        &self,
        animation_id: SceneAnimationId,
        property: TransitionProperty,
    ) -> bool {
        self.animation_engine
            .borrow()
            .scene_animation_track_is_active(animation_id, property)
    }

    pub(crate) fn scene_animation_track_is_bound(
        &self,
        animation_id: SceneAnimationId,
        property: TransitionProperty,
    ) -> bool {
        self.animation_engine
            .borrow()
            .scene_animation_track_is_bound(animation_id, property)
    }

    pub(crate) fn set_grouped_visual_scene_animation(
        &self,
        element_id: &GlobalElementId,
        property: TransitionProperty,
        animation_id: SceneAnimationId,
        grouped: bool,
    ) {
        if self
            .animation_engine
            .borrow_mut()
            .set_grouped_visual_scene_animation(element_id, property, animation_id, grouped)
        {
            self.scene_animation_needs_commit.set(true);
        }
    }

    pub(crate) fn cancel_scene_animation_track(
        &self,
        element_id: &GlobalElementId,
        property: TransitionProperty,
        animation_id: SceneAnimationId,
    ) {
        if self
            .animation_engine
            .borrow_mut()
            .cancel_scene_animation_track(element_id, property, animation_id)
        {
            self.scene_animation_needs_commit.set(true);
        }
    }

    pub(crate) fn scene_animation_is_active(&self, animation_id: SceneAnimationId) -> bool {
        self.animation_engine
            .borrow()
            .scene_animation_is_active(animation_id)
    }

    pub(crate) fn scene_animation_is_bound(&self, animation_id: SceneAnimationId) -> bool {
        self.animation_engine
            .borrow()
            .scene_animation_is_bound(animation_id)
    }
}
