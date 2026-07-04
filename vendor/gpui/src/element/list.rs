//! A list element that can be used to render a large number of differently sized elements
//! efficiently. Clients of this API need to ensure that elements outside of the scrolled
//! area do not change their height for this element to function correctly. If your elements
//! do change height, notify the list element via [`ListState::splice`] or [`ListState::reset`].
//! In order to minimize re-renders, this element's state is stored intrusively
//! on your own views, so that your code can coordinate directly with the list element's cached state.
//!
//! If all of your elements are the same height, see [`crate::UniformList`] for a simpler API

mod element;
mod layout;
mod state;
#[cfg(test)]
mod tests;
mod tree;
mod types;

pub use element::{List, list};
pub use state::ListState;
pub use types::{
    ListAlignment, ListHorizontalSizingBehavior, ListMeasuringBehavior, ListOffset,
    ListScrollEvent, ListSizingBehavior,
};
