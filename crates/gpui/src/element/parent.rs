use super::{AnyElement, Element, IntoElement};

/// This is a helper trait to provide a uniform interface for constructing elements that
/// can accept any number of any kind of child elements
pub trait ParentElement {
    /// Extend this element's children with the given child elements.
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>);

    /// Extend this element's children with already-erased child elements.
    fn extend_any(&mut self, elements: impl IntoIterator<Item = AnyElement>)
    where
        Self: Sized,
    {
        self.extend(elements);
    }

    /// Add a single child element to this element.
    #[track_caller]
    fn child(mut self, child: impl IntoElement) -> Self
    where
        Self: Sized,
    {
        let retained_source = core::panic::Location::caller();
        self.extend(std::iter::once(
            child
                .into_any_element()
                .with_retained_mount(retained_source, 0),
        ));
        self
    }

    /// Add multiple child elements to this element.
    #[track_caller]
    fn children(mut self, children: impl IntoIterator<Item = impl IntoElement>) -> Self
    where
        Self: Sized,
    {
        let retained_source = core::panic::Location::caller();
        self.extend(children.into_iter().enumerate().map(|(index, child)| {
            child.into_any_element().with_retained_mount(
                retained_source,
                index.min(u32::MAX as usize) as u32,
            )
        }));
        self
    }

    /// Conditionally add a child element.
    #[track_caller]
    fn child_if<E, F>(mut self, condition: bool, build_child: F) -> Self
    where
        Self: Sized,
        E: IntoElement,
        F: FnOnce() -> E,
    {
        if condition {
            let retained_source = core::panic::Location::caller();
            self.extend(std::iter::once(
                build_child()
                    .into_any_element()
                    .with_retained_mount(retained_source, 0),
            ));
        }
        self
    }

    /// Conditionally add a child element from an option.
    #[track_caller]
    fn child_some<T, E, F>(mut self, option: Option<T>, build_child: F) -> Self
    where
        Self: Sized,
        E: IntoElement,
        F: FnOnce(T) -> E,
    {
        if let Some(value) = option {
            let retained_source = core::panic::Location::caller();
            self.extend(std::iter::once(
                build_child(value)
                    .into_any_element()
                    .with_retained_mount(retained_source, 0),
            ));
        }
        self
    }

    /// Add children from a fixed-size array.
    #[track_caller]
    fn children_array<const N: usize>(mut self, children: [AnyElement; N]) -> Self
    where
        Self: Sized,
    {
        let retained_source = core::panic::Location::caller();
        self.extend(children.into_iter().enumerate().map(|(index, child)| {
            child.with_retained_mount(retained_source, index.min(u32::MAX as usize) as u32)
        }));
        self
    }
}
