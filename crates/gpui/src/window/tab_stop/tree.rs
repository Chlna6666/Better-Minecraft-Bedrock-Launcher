use sum_tree::SeekTarget;

use super::node::{TabStopNode, TabStopPath};

#[derive(Clone, Debug)]
pub struct TabStopOrderNodeSummary {
    max_index: usize,
    max_path: TabStopPath,
    pub tab_stops: usize,
}

pub type TabStopCount = usize;

impl sum_tree::ContextLessSummary for TabStopOrderNodeSummary {
    fn zero() -> Self {
        TabStopOrderNodeSummary {
            max_index: 0,
            max_path: TabStopPath::default(),
            tab_stops: 0,
        }
    }

    fn add_summary(&mut self, summary: &Self) {
        self.max_index = summary.max_index;
        self.max_path = summary.max_path.clone();
        self.tab_stops += summary.tab_stops;
    }
}

impl sum_tree::KeyedItem for TabStopNode {
    type Key = Self;

    fn key(&self) -> Self::Key {
        self.clone()
    }
}

impl sum_tree::Item for TabStopNode {
    type Summary = TabStopOrderNodeSummary;

    fn summary(&self, _cx: <Self::Summary as sum_tree::Summary>::Context<'_>) -> Self::Summary {
        TabStopOrderNodeSummary {
            max_index: self.node_insertion_index,
            max_path: self.path.clone(),
            tab_stops: if self.tab_stop { 1 } else { 0 },
        }
    }
}

impl<'a> sum_tree::Dimension<'a, TabStopOrderNodeSummary> for TabStopCount {
    fn zero(_: <TabStopOrderNodeSummary as sum_tree::Summary>::Context<'_>) -> Self {
        0
    }

    fn add_summary(
        &mut self,
        summary: &'a TabStopOrderNodeSummary,
        _: <TabStopOrderNodeSummary as sum_tree::Summary>::Context<'_>,
    ) {
        *self += summary.tab_stops;
    }
}

impl<'a> sum_tree::Dimension<'a, TabStopOrderNodeSummary> for TabStopNode {
    fn zero(_: <TabStopOrderNodeSummary as sum_tree::Summary>::Context<'_>) -> Self {
        TabStopNode::default()
    }

    fn add_summary(
        &mut self,
        summary: &'a TabStopOrderNodeSummary,
        _: <TabStopOrderNodeSummary as sum_tree::Summary>::Context<'_>,
    ) {
        self.node_insertion_index = summary.max_index;
        self.path = summary.max_path.clone();
    }
}

impl<'a, 'b> SeekTarget<'a, TabStopOrderNodeSummary, TabStopNode> for &'b TabStopNode {
    fn cmp(
        &self,
        cursor_location: &TabStopNode,
        _: <TabStopOrderNodeSummary as sum_tree::Summary>::Context<'_>,
    ) -> std::cmp::Ordering {
        Iterator::cmp(self.path.0.iter(), cursor_location.path.0.iter()).then(<usize as Ord>::cmp(
            &self.node_insertion_index,
            &cursor_location.node_insertion_index,
        ))
    }
}
