use std::{
    any::{Any, TypeId},
    collections::HashMap,
    sync::{Arc, RwLock},
};

use thiserror::Error;

type SharedService = Arc<dyn Any + Send + Sync>;

/// Errors produced by [`ServiceRegistry`].
#[derive(Debug, Error)]
pub enum ServiceRegistryError {
    /// The registry lock was poisoned by a panic while it was held.
    #[error("service registry lock is poisoned")]
    Poisoned,
    /// The registry's `TypeId` invariant was violated.
    #[error("service registry contained an unexpected concrete type")]
    TypeMismatch,
}

/// A type-indexed registry for application-wide services.
#[derive(Debug, Default)]
pub struct ServiceRegistry {
    services: RwLock<HashMap<TypeId, SharedService>>,
}

impl ServiceRegistry {
    /// Creates an empty service registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a service and returns the previous service of the same type.
    ///
    /// # Errors
    ///
    /// Returns an error when the registry lock is poisoned or an internal type
    /// invariant has been violated.
    pub fn register<T>(&self, service: T) -> Result<Option<Arc<T>>, ServiceRegistryError>
    where
        T: Send + Sync + 'static,
    {
        let previous = self
            .services
            .write()
            .map_err(|_| ServiceRegistryError::Poisoned)?
            .insert(TypeId::of::<T>(), Arc::new(service));

        downcast_optional(previous)
    }

    /// Returns a shared handle to a registered service.
    ///
    /// # Errors
    ///
    /// Returns an error when the registry lock is poisoned or an internal type
    /// invariant has been violated.
    pub fn get<T>(&self) -> Result<Option<Arc<T>>, ServiceRegistryError>
    where
        T: Send + Sync + 'static,
    {
        let service = self
            .services
            .read()
            .map_err(|_| ServiceRegistryError::Poisoned)?
            .get(&TypeId::of::<T>())
            .cloned();

        downcast_optional(service)
    }

    /// Removes and returns a service.
    ///
    /// # Errors
    ///
    /// Returns an error when the registry lock is poisoned or an internal type
    /// invariant has been violated.
    pub fn remove<T>(&self) -> Result<Option<Arc<T>>, ServiceRegistryError>
    where
        T: Send + Sync + 'static,
    {
        let service = self
            .services
            .write()
            .map_err(|_| ServiceRegistryError::Poisoned)?
            .remove(&TypeId::of::<T>());

        downcast_optional(service)
    }

    /// Returns whether a service of `T` is registered.
    ///
    /// # Errors
    ///
    /// Returns an error when the registry lock is poisoned.
    pub fn contains<T>(&self) -> Result<bool, ServiceRegistryError>
    where
        T: Send + Sync + 'static,
    {
        Ok(self
            .services
            .read()
            .map_err(|_| ServiceRegistryError::Poisoned)?
            .contains_key(&TypeId::of::<T>()))
    }
}

fn downcast_optional<T>(
    service: Option<SharedService>,
) -> Result<Option<Arc<T>>, ServiceRegistryError>
where
    T: Send + Sync + 'static,
{
    service
        .map(|service| Arc::downcast::<T>(service).map_err(|_| ServiceRegistryError::TypeMismatch))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::ServiceRegistry;

    #[test]
    fn registers_replaces_and_removes_services_by_type() {
        let registry = ServiceRegistry::new();

        assert!(
            registry
                .register(String::from("first"))
                .expect("register")
                .is_none()
        );
        assert_eq!(
            registry
                .get::<String>()
                .expect("get")
                .as_deref()
                .map(String::as_str),
            Some("first")
        );

        let previous = registry
            .register(String::from("second"))
            .expect("replace")
            .expect("previous service");
        assert_eq!(previous.as_str(), "first");
        assert!(registry.contains::<String>().expect("contains"));

        let removed = registry
            .remove::<String>()
            .expect("remove")
            .expect("removed service");
        assert_eq!(removed.as_str(), "second");
        assert!(!registry.contains::<String>().expect("contains"));
    }
}
