use super::*;
use crate::{StyleRefinement, TestAppContext};

#[gpui::test]
fn style_reads_leave_click_state_unallocated_until_input_registration(cx: &mut TestAppContext) {
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let mut interactivity = Interactivity::new();
        interactivity.active_style = Some(Box::new(StyleRefinement {
            opacity: Some(0.5),
            ..Default::default()
        }));
        let mut state = InteractiveElementState::default();

        let idle = interactivity.compute_style_internal(None, Some(&state), window, cx);
        assert_eq!(idle.opacity, None);
        assert!(state.clicked_state.is_none());

        state.ensure_clicked_state().borrow_mut().element = true;
        let active = interactivity.compute_style_internal(None, Some(&state), window, cx);
        assert_eq!(active.opacity, Some(0.5));
    });
}
