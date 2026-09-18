use super::*;
use crate::{StyleRefinement, TestAppContext, current_thread_style_refine_count};

#[gpui::test]
fn computed_style_cache_reuses_same_frame_style(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let mut interactivity = Interactivity::new();
        interactivity.base_style.opacity = Some(0.5);
        interactivity.hover_style = Some(Box::new(StyleRefinement {
            opacity: Some(0.75),
            ..Default::default()
        }));

        let before = current_thread_style_refine_count();
        let _ = interactivity.compute_style_internal(None, None, window, cx);
        let after_first = current_thread_style_refine_count();
        let _ = interactivity.compute_style_internal(None, None, window, cx);
        let after_second = current_thread_style_refine_count();

        assert!(after_first > before);
        assert_eq!(after_second, after_first);
    });
}

#[gpui::test]
fn persisted_hover_state_drives_layout_style_without_hitbox(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let mut interactivity = Interactivity::new();
        interactivity.base_style.opacity = Some(0.5);
        interactivity.hover_style = Some(Box::new(StyleRefinement {
            opacity: Some(0.75),
            ..Default::default()
        }));

        let mut element_state = InteractiveElementState::default();
        element_state.ensure_style_hover_state().borrow_mut().element = true;

        let style =
            interactivity.compute_style_internal(None, Some(&mut element_state), window, cx);
        assert_eq!(style.opacity, 0.75);
    });
}
