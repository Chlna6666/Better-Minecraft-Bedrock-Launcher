//! Copy-on-write node pages keep immutable snapshots cheap to publish.
use super::NodeSlot;
use std::{
    ops::{Index, IndexMut},
    sync::Arc,
};

const PAGE_SIZE: usize = 64;

#[derive(Clone, Debug, Default)]
pub(super) struct NodePages {
    pages: Vec<Arc<Vec<NodeSlot>>>,
    len: usize,
}

impl NodePages {
    pub(super) fn len(&self) -> usize {
        self.len
    }
    pub(super) fn get(&self, index: usize) -> Option<&NodeSlot> {
        self.pages.get(index / PAGE_SIZE)?.get(index % PAGE_SIZE)
    }
    pub(super) fn get_mut(&mut self, index: usize) -> Option<&mut NodeSlot> {
        Arc::make_mut(self.pages.get_mut(index / PAGE_SIZE)?).get_mut(index % PAGE_SIZE)
    }
    pub(super) fn push(&mut self, slot: NodeSlot) {
        if self.len % PAGE_SIZE == 0 {
            self.pages.push(Arc::new(Vec::with_capacity(PAGE_SIZE)));
        }
        Arc::make_mut(self.pages.last_mut().expect("node page exists")).push(slot);
        self.len += 1;
    }
}

impl Index<usize> for NodePages {
    type Output = NodeSlot;
    fn index(&self, index: usize) -> &NodeSlot {
        self.get(index).expect("live node index")
    }
}
impl IndexMut<usize> for NodePages {
    fn index_mut(&mut self, index: usize) -> &mut NodeSlot {
        self.get_mut(index).expect("live node index")
    }
}
