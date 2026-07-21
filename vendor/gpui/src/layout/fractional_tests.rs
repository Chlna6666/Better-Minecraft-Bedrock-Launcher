use super::engine::{EXPECT_MESSAGE, TaffyLayoutEngine};
use crate::{
    AlignItems, Display, FlexDirection, JustifyContent, Style, px, relative, size,
};

#[test]
fn centered_child_keeps_equal_device_margins_at_fractional_scale() {
    let scale_factor = 1.25;
    let mut engine = TaffyLayoutEngine::new();

    let mut knob_style = Style::default();
    knob_style.display = Display::Flex;
    knob_style.size = size(px(22.).into(), px(22.).into());

    let knob = engine.request_layout(knob_style, px(16.), scale_factor, &[]);

    let mut track_style = Style::default();
    track_style.display = Display::Flex;
    track_style.size = size(px(44.).into(), px(26.).into());
    track_style.align_items = Some(AlignItems::Center);

    let track = engine.request_layout(track_style, px(16.), scale_factor, &[knob]);

    let mut root_style = Style::default();
    root_style.display = Display::Flex;
    root_style.size = size(px(100.).into(), px(101.).into());
    root_style.flex_direction = FlexDirection::Column;
    root_style.align_items = Some(AlignItems::Center);
    root_style.justify_content = Some(JustifyContent::Center);

    let root = engine.request_layout(root_style, px(16.), scale_factor, &[track]);
    engine
        .taffy
        .compute_layout(
            root.into(),
            taffy::geometry::Size {
                width: taffy::style::AvailableSpace::Definite(100. * scale_factor),
                height: taffy::style::AvailableSpace::Definite(101. * scale_factor),
            },
        )
        .expect(EXPECT_MESSAGE);

    let track_bounds = engine.layout_bounds(track, scale_factor);
    let knob_bounds = engine.layout_bounds(knob, scale_factor);
    let top_margin = knob_bounds.origin.y - track_bounds.origin.y;
    let bottom_margin = track_bounds.origin.y + track_bounds.size.height
        - (knob_bounds.origin.y + knob_bounds.size.height);

    assert!(
        (top_margin.0 - bottom_margin.0).abs() < 0.0001,
        "centered child margins diverged: top={top_margin:?}, bottom={bottom_margin:?}"
    );
}


#[test]
fn percentage_passthrough_keeps_relative_modal_centered() {
    let scale_factor = 1.0;
    let mut engine = TaffyLayoutEngine::new();

    let mut modal_style = Style::default();
    modal_style.display = Display::Flex;
    modal_style.size = size(relative(0.75).into(), px(240.).into());
    let modal = engine.request_layout(modal_style, px(16.), scale_factor, &[]);

    let mut animation_wrapper_style = Style::default();
    animation_wrapper_style.display = Display::Flex;
    animation_wrapper_style.percentage_passthrough = true;
    let animation_wrapper = engine.request_layout(
        animation_wrapper_style,
        px(16.),
        scale_factor,
        &[modal],
    );

    let mut root_style = Style::default();
    root_style.display = Display::Flex;
    root_style.size = size(px(800.).into(), px(600.).into());
    root_style.align_items = Some(AlignItems::Center);
    root_style.justify_content = Some(JustifyContent::Center);
    let root = engine.request_layout(
        root_style,
        px(16.),
        scale_factor,
        &[animation_wrapper],
    );

    engine
        .taffy
        .compute_layout(
            root.into(),
            taffy::geometry::Size {
                width: taffy::style::AvailableSpace::Definite(800.),
                height: taffy::style::AvailableSpace::Definite(600.),
            },
        )
        .expect(EXPECT_MESSAGE);

    let wrapper_bounds = engine.layout_bounds(animation_wrapper, scale_factor);
    let modal_bounds = engine.layout_bounds(modal, scale_factor);

    assert_eq!(wrapper_bounds.size.width, px(600.));
    assert_eq!(modal_bounds.size.width, px(600.));
    assert_eq!(wrapper_bounds.origin.x, px(100.));
    assert_eq!(modal_bounds.origin.x, px(100.));
}
