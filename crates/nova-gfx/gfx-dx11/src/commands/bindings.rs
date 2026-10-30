//! Immediate-context binding state, scoped to one render pass.
use crate::resources::Bound;
use gfx_core::ShaderStages;
use windows::core::Interface;

#[derive(Clone, Copy)]
enum Key {
    Uniform(usize, u32, u32),
    Srv(usize),
    Sampler(usize),
}

struct Stage {
    uniforms: [Option<(usize, u32, u32)>; 13],
    srvs: [usize; 128],
    samplers: [usize; 16],
}

impl Default for Stage {
    fn default() -> Self {
        Self {
            uniforms: [None; 13],
            srvs: [0; 128],
            samplers: [0; 16],
        }
    }
}

impl Stage {
    fn update(&mut self, slot: usize, key: Key) -> bool {
        match key {
            Key::Uniform(buffer, first, count) => {
                let value = Some((buffer, first, count));
                let changed = self.uniforms[slot] != value;
                self.uniforms[slot] = value;
                changed
            }
            Key::Srv(view) => {
                let changed = self.srvs[slot] != view;
                self.srvs[slot] = view;
                changed
            }
            Key::Sampler(sampler) => {
                let changed = self.samplers[slot] != sampler;
                self.samplers[slot] = sampler;
                changed
            }
        }
    }
}

#[derive(Default)]
pub(super) struct Bindings {
    vertex: Stage,
    fragment: Stage,
}

impl Bindings {
    pub(super) fn changed_stages(
        &mut self,
        slot: u32,
        stages: ShaderStages,
        bound: &Bound,
    ) -> ShaderStages {
        let key = match bound {
            Bound::Uniform {
                buffer,
                first,
                count,
            } => Key::Uniform(buffer.as_raw() as usize, *first, *count),
            Bound::Srv(view) => Key::Srv(view.as_raw() as usize),
            Bound::Sampler(sampler) => Key::Sampler(sampler.as_raw() as usize),
        };
        self.update(slot as usize, stages, key)
    }

    fn update(&mut self, slot: usize, stages: ShaderStages, key: Key) -> ShaderStages {
        let mut changed = ShaderStages::empty();
        if stages.contains(ShaderStages::VERTEX) && self.vertex.update(slot, key) {
            changed |= ShaderStages::VERTEX;
        }
        if stages.contains(ShaderStages::FRAGMENT) && self.fragment.update(slot, key) {
            changed |= ShaderStages::FRAGMENT;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_bindings_skip_calls_but_stages_ranges_and_kinds_stay_independent() {
        let mut state = Bindings::default();
        let both = ShaderStages::VERTEX | ShaderStages::FRAGMENT;
        assert_eq!(state.update(12, both, Key::Uniform(1, 0, 16)), both);
        assert!(state.update(12, both, Key::Uniform(1, 0, 16)).is_empty());
        assert_eq!(
            state.update(12, ShaderStages::VERTEX, Key::Uniform(1, 16, 16)),
            ShaderStages::VERTEX
        );
        assert_eq!(
            state.update(12, both, Key::Uniform(1, 16, 16)),
            ShaderStages::FRAGMENT
        );
        assert_eq!(state.update(12, both, Key::Uniform(1, 16, 32)), both);
        assert_eq!(state.update(127, both, Key::Srv(2)), both);
        assert!(state.update(127, both, Key::Srv(2)).is_empty());
        assert_eq!(
            state.update(127, ShaderStages::FRAGMENT, Key::Srv(3)),
            ShaderStages::FRAGMENT
        );
        assert_eq!(state.update(12, both, Key::Srv(1)), both);
        assert_eq!(state.update(15, both, Key::Sampler(4)), both);
        assert!(state.update(15, both, Key::Sampler(4)).is_empty());
        assert_eq!(Bindings::default().update(127, both, Key::Srv(2)), both);
    }
}
