use super::*;
use crate as gpui;
use gpui::NoAction;

actions!(
    test_only,
    [ActionAlpha, ActionBeta, ActionGamma, ActionDelta,]
);

mod matching;
mod precedence;
