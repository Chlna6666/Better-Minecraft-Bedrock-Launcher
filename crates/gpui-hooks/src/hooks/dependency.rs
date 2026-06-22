use smallvec::SmallVec;
use std::any::Any;

pub(crate) type Dependencies = SmallVec<[Box<dyn Dependency>; 4]>;

pub trait Dependency: Any {
    fn as_any(&self) -> &dyn Any;
    fn clone_boxed(&self) -> Box<dyn Dependency>;
    fn equals(&self, other: &dyn Dependency) -> bool;
}

impl<T> Dependency for T
where
    T: Any + Clone + PartialEq,
{
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clone_boxed(&self) -> Box<dyn Dependency> {
        Box::new(self.clone())
    }

    fn equals(&self, other: &dyn Dependency) -> bool {
        other.as_any().downcast_ref::<T>() == Some(self)
    }
}

impl Clone for Box<dyn Dependency> {
    fn clone(&self) -> Self {
        self.clone_boxed()
    }
}

pub(crate) fn to_dependencies<D>(deps: D) -> Dependencies
where
    D: IntoIterator,
    D::Item: Dependency + Clone + 'static,
{
    deps.into_iter()
        .map(|dependency| Box::new(dependency) as Box<dyn Dependency>)
        .collect()
}

pub(crate) fn dependencies_changed(
    previous: &[Box<dyn Dependency>],
    next: &[Box<dyn Dependency>],
) -> bool {
    previous.len() != next.len()
        || previous
            .iter()
            .zip(next.iter())
            .any(|(previous, next)| !previous.equals(next.as_ref()))
}
