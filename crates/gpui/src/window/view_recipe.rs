use super::*;
use crate::{RetainedViewRecipe, window::frame::RetainedPaintContext};

#[derive(Clone)]
pub(super) struct CommittedViewRecipe {
    owner: EntityId,
    recipe: RetainedViewRecipe,
    context: RetainedPaintContext,
    viewport: Size<Pixels>,
}

impl Window {
    /// Admission happens before layout. Only explicit repeatable recipes are
    /// retained; a miss executes the original Render and normal lifecycle.
    pub(crate) fn reuse_view_recipe(&mut self, owner: EntityId) -> Option<RetainedViewRecipe> {
        if self.view_recipes.is_empty()
            || cfg!(gpui_hot_reload)
            || self.force_view_cache_refresh()
            || self.view_dirty_scope(owner).is_some()
            || !self.retained_replay_is_targeted()
        {
            return None;
        }
        let key = self.current_stable_layout_element_id()?;
        if self.invalidator.retained_route_path_is_dirty(&key, &|_| true)
            || self.invalidator.requires_reconciliation_for_route(&key, &|_| true)
        {
            return None;
        }
        let previous = self.view_recipes.get(&key)?;
        if previous.owner != owner
            || previous.viewport != self.viewport_size
            || previous.context != self.current_retained_paint_context()
            || self.rendered_frame.dispatch_tree.view_path_reversed(owner).next().is_none()
        {
            return None;
        }
        let previous = previous.clone();
        let recipe = previous.recipe.clone();
        self.next_view_recipes.insert(key, previous);
        // Render did not repeat its dependency reads. The existing committed
        // replay preservation path must retain those still-mounted edges.
        self.replayed_dependency_ranges = true;
        Some(recipe)
    }

    pub(crate) fn record_view_recipe(&mut self, owner: EntityId, recipe: RetainedViewRecipe) {
        let Some(key) = self.current_stable_layout_element_id() else { return; };
        let entry = CommittedViewRecipe {
            owner,
            recipe,
            context: self.current_retained_paint_context(),
            viewport: self.viewport_size,
        };
        self.next_view_recipes.insert(key, entry);
    }

    pub(super) fn commit_view_recipes(&mut self) {
        // Remove unmounted entries and entries with ambiguous/uncommitted
        // lifecycle metadata. The candidate table is bounded by this frame.
        self.next_view_recipes.retain(|key, entry| {
            self.rendered_frame.retained_element_ranges.get(key)
                .is_some_and(|range| range.identity_stable)
                && self.rendered_frame.dispatch_tree.view_path_reversed(entry.owner).next().is_some()
        });
        mem::swap(&mut self.view_recipes, &mut self.next_view_recipes);
        self.next_view_recipes.clear();
    }
}
