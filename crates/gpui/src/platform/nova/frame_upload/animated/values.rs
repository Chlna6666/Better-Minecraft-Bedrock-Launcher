use super::*;
use crate::{SceneAnimationId, SceneAnimationValue, TransitionProperty};
use collections::FxHashMap;
use smallvec::SmallVec;

const ENGINE_ANIMATION_ID_BASE: u32 = 1 << 31;
const DENSE_LOOKUP_MAX_SPAN: usize = 4096;
const DENSE_LOOKUP_DENSITY_FACTOR: usize = 4;

#[derive(Clone, Copy)]
pub(super) struct ResolvedAnimationValue {
    pub(super) property: TransitionProperty,
    pub(super) sampled: [f32; 4],
}

pub(super) struct ResolvedAnimationValues {
    scene: SmallVec<[Option<ResolvedAnimationValue>; 16]>,
    engine_base: u32,
    engine: SmallVec<[Option<ResolvedAnimationValue>; 16]>,
    sparse: FxHashMap<SceneAnimationId, ResolvedAnimationValue>,
}

impl ResolvedAnimationValue {
    #[inline]
    pub(super) fn new(value: &SceneAnimationValue) -> Self {
        let progress = if value.progress.is_finite() {
            value.progress
        } else {
            0.0
        };
        let mut sampled = std::array::from_fn(|index| {
            value.from[index] + (value.to[index] - value.from[index]) * progress
        });
        if value.property == TransitionProperty::FilterBlur {
            // Blur capture reserves the largest endpoint's 3-sigma footprint once. Keep CPU
            // fallback sampling inside that same endpoint interval even when easing overshoots,
            // otherwise an out-of-range sigma could sample beyond the retained target.
            let from = if value.from[0].is_finite() {
                value.from[0].max(0.0)
            } else {
                0.0
            };
            let to = if value.to[0].is_finite() {
                value.to[0].max(0.0)
            } else {
                0.0
            };
            let lower = from.min(to);
            let upper = from.max(to);
            sampled[0] = if sampled[0].is_finite() {
                sampled[0].clamp(lower, upper)
            } else {
                lower
            };
        }
        Self {
            property: value.property,
            sampled,
        }
    }
}

impl ResolvedAnimationValues {
    fn new(values: &[SceneAnimationValue]) -> Self {
        let mut scene_count = 0usize;
        let mut scene_max = None::<u32>;
        let mut engine_count = 0usize;
        let mut engine_min = None::<u32>;
        let mut engine_max = None::<u32>;
        for value in values {
            let id = value.animation_id.0;
            if id < ENGINE_ANIMATION_ID_BASE {
                scene_count += 1;
                scene_max = Some(scene_max.map_or(id, |max| max.max(id)));
            } else {
                engine_count += 1;
                engine_min = Some(engine_min.map_or(id, |min| min.min(id)));
                engine_max = Some(engine_max.map_or(id, |max| max.max(id)));
            }
        }

        let scene_len = scene_max
            .and_then(|max| dense_span_len(scene_count, 0, max))
            .unwrap_or(0);
        let engine_base = engine_min.unwrap_or(ENGINE_ANIMATION_ID_BASE);
        let engine_len = engine_max
            .and_then(|max| dense_span_len(engine_count, engine_base, max))
            .unwrap_or(0);
        let sparse_count = if scene_len == 0 { scene_count } else { 0 }
            + if engine_len == 0 { engine_count } else { 0 };

        let mut scene = SmallVec::new();
        scene.resize(scene_len, None);
        let mut engine = SmallVec::new();
        engine.resize(engine_len, None);
        let mut sparse = FxHashMap::default();
        if sparse_count > 0 {
            sparse.reserve(sparse_count);
        }

        let mut resolved = Self {
            scene,
            engine_base,
            engine,
            sparse,
        };
        for value in values {
            resolved.insert_first(value.animation_id, ResolvedAnimationValue::new(value));
        }
        resolved
    }

    #[inline]
    pub(super) fn get(&self, animation_id: &SceneAnimationId) -> Option<&ResolvedAnimationValue> {
        let id = animation_id.0;
        if id < ENGINE_ANIMATION_ID_BASE {
            if let Some(value) = self.scene.get(id as usize).and_then(Option::as_ref) {
                return Some(value);
            }
        } else if let Some(offset) = id.checked_sub(self.engine_base)
            && let Some(value) = self.engine.get(offset as usize).and_then(Option::as_ref)
        {
            return Some(value);
        }
        self.sparse.get(animation_id)
    }

    fn insert_first(&mut self, animation_id: SceneAnimationId, value: ResolvedAnimationValue) {
        let id = animation_id.0;
        if id < ENGINE_ANIMATION_ID_BASE {
            if let Some(slot) = self.scene.get_mut(id as usize) {
                if slot.is_none() {
                    *slot = Some(value);
                }
                return;
            }
        } else if let Some(offset) = id.checked_sub(self.engine_base)
            && let Some(slot) = self.engine.get_mut(offset as usize)
        {
            if slot.is_none() {
                *slot = Some(value);
            }
            return;
        }
        self.sparse.entry(animation_id).or_insert(value);
    }
}

fn dense_span_len(count: usize, min: u32, max: u32) -> Option<usize> {
    let span = max.checked_sub(min)?.checked_add(1)? as usize;
    let density_limit = count.saturating_mul(DENSE_LOOKUP_DENSITY_FACTOR).max(16);
    (span <= density_limit && span <= DENSE_LOOKUP_MAX_SPAN).then_some(span)
}

pub(super) fn resolve_animation_values(values: &[SceneAnimationValue]) -> ResolvedAnimationValues {
    ResolvedAnimationValues::new(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_animation_values_preserve_first_duplicate() {
        let id = crate::SceneAnimationId(5);
        let values = [
            SceneAnimationValue {
                animation_id: id,
                property: TransitionProperty::Translation,
                progress: 0.5,
                from: [0.0; 4],
                to: [10.0, 0.0, 0.0, 0.0],
            },
            SceneAnimationValue {
                animation_id: id,
                property: TransitionProperty::Translation,
                progress: 1.0,
                from: [0.0; 4],
                to: [99.0, 0.0, 0.0, 0.0],
            },
        ];
        let resolved = resolve_animation_values(&values);
        assert_eq!(resolved.get(&id).unwrap().sampled[0], 5.0);
    }

    #[test]
    fn resolved_animation_values_handle_dense_scene_and_engine_namespaces() {
        let scene_id = crate::SceneAnimationId(3);
        let engine_id = crate::SceneAnimationId(ENGINE_ANIMATION_ID_BASE + 7);
        let values = [
            SceneAnimationValue {
                animation_id: scene_id,
                property: TransitionProperty::Translation,
                progress: 1.0,
                from: [0.0; 4],
                to: [3.0, 0.0, 0.0, 0.0],
            },
            SceneAnimationValue {
                animation_id: engine_id,
                property: TransitionProperty::Opacity,
                progress: 0.5,
                from: [0.0; 4],
                to: [1.0, 0.0, 0.0, 0.0],
            },
        ];
        let resolved = resolve_animation_values(&values);
        assert_eq!(resolved.get(&scene_id).unwrap().sampled[0], 3.0);
        assert_eq!(resolved.get(&engine_id).unwrap().sampled[0], 0.5);
    }

    #[test]
    fn resolved_animation_values_fall_back_for_sparse_ids() {
        let first = crate::SceneAnimationId(1);
        let sparse = crate::SceneAnimationId(10_000);
        let values = [
            SceneAnimationValue {
                animation_id: first,
                property: TransitionProperty::Translation,
                progress: 1.0,
                from: [0.0; 4],
                to: [1.0, 0.0, 0.0, 0.0],
            },
            SceneAnimationValue {
                animation_id: sparse,
                property: TransitionProperty::Translation,
                progress: 1.0,
                from: [0.0; 4],
                to: [2.0, 0.0, 0.0, 0.0],
            },
        ];
        let resolved = resolve_animation_values(&values);
        assert_eq!(resolved.get(&first).unwrap().sampled[0], 1.0);
        assert_eq!(resolved.get(&sparse).unwrap().sampled[0], 2.0);
    }
}
