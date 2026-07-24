use super::*;
use crate::{StyleRefinement, TestAppContext, performance_metrics_snapshot};

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

        let before = performance_metrics_snapshot().style_refine_count;
        let _ = interactivity.compute_style_internal(None, None, window, cx);
        let after_first = performance_metrics_snapshot().style_refine_count;
        let _ = interactivity.compute_style_internal(None, None, window, cx);
        let after_second = performance_metrics_snapshot().style_refine_count;

        assert!(after_first > before);
        assert_eq!(after_second, after_first);
    });
}
