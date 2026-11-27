//! A list element that can be used to render a large number of differently sized elements
//! efficiently. Clients of this API need to ensure that elements outside of the scrolled
//! area do not change their height for this element to function correctly. If your elements
//! do change height, invalidate the affected indices via [`ListState::invalidate_items`],
//! or use [`ListState::splice`] / [`ListState::reset`] when the row sequence changes.
//! In order to minimize re-renders, this element's state is stored intrusively
//! on your own views, so that your code can coordinate directly with the list element's cached state.
//!
//! If all of your elements are the same height, see [`crate::UniformList`] for a simpler API

mod configuration;
mod component;
mod layout;
mod state;
#[cfg(test)]
mod tests;
mod tree;

pub use configuration::{
    ListAlignment, ListHorizontalSizingBehavior, ListMeasuringBehavior, ListOffset,
    ListScrollEvent, ListSizingBehavior,
};
pub use component::{List, list};
pub use state::ListState;
pub(crate) use state::ListPlacementProof;
