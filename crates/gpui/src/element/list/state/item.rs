use crate::{App, FocusHandle, Pixels, Size, Window};

#[derive(Clone)]
pub(in crate::element::list) enum ListItem {
    Unmeasured {
        focus_handle: Option<FocusHandle>,
    },
    Measured {
        size: Size<Pixels>,
        focus_handle: Option<FocusHandle>,
    },
}

impl ListItem {
    pub(super) fn size(&self) -> Option<Size<Pixels>> {
        if let ListItem::Measured { size, .. } = self {
            Some(*size)
        } else {
            None
        }
    }

    pub(in crate::element::list) fn focus_handle(&self) -> Option<FocusHandle> {
        match self {
            ListItem::Unmeasured { focus_handle } | ListItem::Measured { focus_handle, .. } => {
                focus_handle.clone()
            }
        }
    }

    pub(super) fn contains_focused(&self, window: &Window, cx: &App) -> bool {
        match self {
            ListItem::Unmeasured { focus_handle } | ListItem::Measured { focus_handle, .. } => {
                focus_handle
                    .as_ref()
                    .is_some_and(|handle| handle.contains_focused(window, cx))
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::element::list) struct ListItemSummary {
    pub(in crate::element::list) count: usize,
    pub(in crate::element::list) rendered_count: usize,
    pub(in crate::element::list) unrendered_count: usize,
    pub(in crate::element::list) height: Pixels,
    pub(in crate::element::list) has_focus_handles: bool,
}
