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
    fn child(mut self, child: impl IntoElement) -> Self
    where
        Self: Sized,
    {
        self.extend(std::iter::once(child.into_element().into_any()));
        self
    }

    /// Add multiple child elements to this element.
    fn children(mut self, children: impl IntoIterator<Item = impl IntoElement>) -> Self
    where
        Self: Sized,
    {
        self.extend(children.into_iter().map(|child| child.into_any_element()));
        self
    }

    /// Conditionally add a child element.
    fn child_if<E, F>(mut self, condition: bool, build_child: F) -> Self
    where
        Self: Sized,
        E: IntoElement,
        F: FnOnce() -> E,
    {
        if condition {
            self.extend(std::iter::once(build_child().into_any_element()));
        }
        self
    }

    /// Conditionally add a child element from an option.
    fn child_some<T, E, F>(mut self, option: Option<T>, build_child: F) -> Self
    where
        Self: Sized,
        E: IntoElement,
        F: FnOnce(T) -> E,
    {
        if let Some(value) = option {
            self.extend(std::iter::once(build_child(value).into_any_element()));
        }
        self
    }

    /// Add children from a fixed-size array.
    fn children_array<const N: usize>(mut self, children: [AnyElement; N]) -> Self
    where
        Self: Sized,
    {
        self.extend(children);
        self
    }
}
