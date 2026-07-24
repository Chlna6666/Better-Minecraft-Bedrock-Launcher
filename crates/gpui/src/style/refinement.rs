use crate::{GridLocation, Styled};

use super::model::StyleRefinement;

impl Styled for StyleRefinement {
    fn style(&mut self) -> &mut StyleRefinement {
        self
    }
}

impl StyleRefinement {
    /// The grid location of this element
    pub fn grid_location_mut(&mut self) -> &mut GridLocation {
        if self.grid_location.is_none() {
            self.grid_location = Some(GridLocation::default());
        }
        self.grid_location
            .as_mut()
            .expect("grid location should be initialized")
    }
}
