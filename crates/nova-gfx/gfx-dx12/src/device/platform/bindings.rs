//! Graphics root state is valid only inside one command-list render pass.
use gfx_core::{BindingResource, RenderPipelineId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TableKey {
    Resource(BindingResource),
    Native(u64),
}

pub(super) struct State {
    pub pipeline: Option<RenderPipelineId>,
    pub scissor: Option<(i32, i32, i32, i32)>,
    root: Option<usize>,
    tables: [Option<TableKey>; 64],
}

impl Default for State {
    fn default() -> Self {
        Self {
            pipeline: None,
            scissor: None,
            root: None,
            tables: [None; 64],
        }
    }
}

impl State {
    pub fn set_root(&mut self, root: usize) -> bool {
        if self.root == Some(root) {
            return false;
        }
        self.root = Some(root);
        self.tables.fill(None);
        true
    }
    pub fn set_table(&mut self, slot: u32, key: TableKey) -> bool {
        let previous = &mut self.tables[slot as usize];
        let changed = *previous != Some(key);
        *previous = Some(key);
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfx_core::{BufferBinding, BufferId};
    #[test]
    fn resource_ranges_and_root_changes_invalidate_cached_tables() {
        let mut state = State::default();
        let range = BufferBinding {
            buffer: BufferId::from_parts(1, 1),
            offset: 0,
            size: 32,
            stride: None,
        };
        let key = TableKey::Resource(BindingResource::Buffer(range));
        assert!(state.set_root(1));
        assert!(state.set_table(0, key));
        assert!(!state.set_root(1));
        assert!(!state.set_table(0, key));
        assert!(state.set_table(
            0,
            TableKey::Resource(BindingResource::Buffer(BufferBinding {
                offset: 16,
                ..range
            }))
        ));
        assert!(state.set_table(63, TableKey::Native(7)));
        assert!(!state.set_table(63, TableKey::Native(7)));
        assert!(state.set_root(2));
        assert!(state.set_table(63, TableKey::Native(7)));
        assert!(State::default().set_table(0, key));
    }
}
