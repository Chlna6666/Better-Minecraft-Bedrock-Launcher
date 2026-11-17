//! Task type registry: declarations of task capabilities and metadata.
//!
//! A [`TaskDefinition`] describes what a *kind* of task can do — cancellable,
//! pausable, prioritizable, retryable, which dispatch pool it uses and which
//! phases it reports. The registry maps namespaced type ids (`core.install.zip`,
//! `example.world.convert`) to definitions. Core registers its own types at
//! startup; plugins register through the plugin bridge and are confined to their
//! own namespace.
//!
//! The registry is declarative only: it never creates or runs tasks. Runtime
//! control (pause/resume/cancel) still goes through the command layer, which
//! consults the definition for capability checks.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use once_cell::sync::Lazy;

use super::scheduler::TaskPool;

/// What a task type supports. These are *type-level* declarations; an
/// individual task may still be unable to pause at a specific phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskCapabilities {
    pub cancellable: bool,
    pub pausable: bool,
    pub prioritizable: bool,
    pub retryable: bool,
}

impl TaskCapabilities {
    pub const NONE: Self = Self {
        cancellable: false,
        pausable: false,
        prioritizable: false,
        retryable: false,
    };

    /// The common case: cancellable, prioritizable, no pause support.
    pub const CANCELLABLE: Self = Self {
        cancellable: true,
        pausable: false,
        prioritizable: true,
        retryable: false,
    };

    /// Cancellable + checkpoint pause + priority (downloads, conversions).
    pub const PAUSABLE: Self = Self {
        cancellable: true,
        pausable: true,
        prioritizable: true,
        retryable: false,
    };
}

/// One reported phase of a task type. `code` is the stable machine code
/// (matching `TaskStage` codes in practice); `title_key` is an i18n key that
/// rendering owns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskPhase {
    pub code: String,
    pub title_key: Option<String>,
}

/// Declarative description of a task type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskDefinition {
    /// Namespaced type id, e.g. `core.install.zip` or `example.world.convert`.
    pub type_id: String,
    /// i18n key for the human-readable title.
    pub title_key: Option<String>,
    pub capabilities: TaskCapabilities,
    /// Dispatch pool this type uses. `None` means the task does not consume a
    /// slot-limited pool slot (UI-local or CPU-only work).
    pub pool: Option<TaskPool>,
    /// Phases this type may report, in order. Metadata only for now.
    pub phases: Vec<TaskPhase>,
}

impl TaskDefinition {
    pub fn new(type_id: impl Into<String>) -> Self {
        Self {
            type_id: type_id.into(),
            title_key: None,
            capabilities: TaskCapabilities::CANCELLABLE,
            pool: None,
            phases: Vec::new(),
        }
    }

    pub fn title_key(mut self, key: impl Into<String>) -> Self {
        self.title_key = Some(key.into());
        self
    }

    pub fn capabilities(mut self, capabilities: TaskCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    pub fn pool(mut self, pool: TaskPool) -> Self {
        self.pool = Some(pool);
        self
    }

    pub fn phase(mut self, code: impl Into<String>, title_key: Option<String>) -> Self {
        self.phases.push(TaskPhase {
            code: code.into(),
            title_key,
        });
        self
    }
}

/// Why a definition could not be registered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryError {
    /// The type id is not namespaced (`core.` / `<plugin_id>.`).
    InvalidTypeId,
    /// A definition with this id already exists.
    AlreadyRegistered,
    /// A plugin tried to register outside its own namespace.
    NamespaceMismatch,
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidTypeId => write!(f, "任务类型 ID 必须带命名空间（如 core.install.zip）"),
            Self::AlreadyRegistered => write!(f, "任务类型已注册"),
            Self::NamespaceMismatch => write!(f, "插件只能注册自己命名空间下的任务类型"),
        }
    }
}

struct Registry {
    definitions: HashMap<String, Arc<TaskDefinition>>,
}

static REGISTRY: Lazy<RwLock<Registry>> = Lazy::new(|| {
    RwLock::new(Registry {
        definitions: core_definitions(),
    })
});

fn core_definitions() -> HashMap<String, Arc<TaskDefinition>> {
    let mut map = HashMap::new();
    let mut register = |definition: TaskDefinition| {
        map.insert(definition.type_id.clone(), Arc::new(definition));
    };

    register(
        TaskDefinition::new("core.download.http")
            .title_key("TaskTitle.download")
            .capabilities(TaskCapabilities::PAUSABLE)
            .pool(TaskPool::Download),
    );
    register(
        TaskDefinition::new("core.install.zip")
            .title_key("TaskTitle.extract")
            .capabilities(TaskCapabilities::CANCELLABLE)
            .pool(TaskPool::Archive),
    );
    register(
        TaskDefinition::new("core.install.minecraft")
            .title_key("TaskTitle.install_minecraft")
            .capabilities(TaskCapabilities::CANCELLABLE)
            .pool(TaskPool::Archive),
    );
    register(
        TaskDefinition::new("core.install.levilamina")
            .title_key("TaskTitle.install_levilamina")
            .capabilities(TaskCapabilities::CANCELLABLE),
    );
    register(
        TaskDefinition::new("core.install.native_mod").capabilities(TaskCapabilities::CANCELLABLE),
    );
    register(TaskDefinition::new("core.map.convert").capabilities(TaskCapabilities::PAUSABLE));
    register(TaskDefinition::new("core.map.export").capabilities(TaskCapabilities::CANCELLABLE));
    register(TaskDefinition::new("core.cape.import").capabilities(TaskCapabilities::CANCELLABLE));
    register(
        TaskDefinition::new("core.plugin.install").capabilities(TaskCapabilities::CANCELLABLE),
    );
    register(
        TaskDefinition::new("core.version.mutate").capabilities(TaskCapabilities::CANCELLABLE),
    );
    map
}

/// Registers a definition on behalf of the host (core namespace or any type).
/// Core code may register any id; the plugin bridge uses
/// [`register_for_plugin`] which enforces namespace ownership.
pub fn register(definition: TaskDefinition) -> Result<(), RegistryError> {
    validate_namespaced(&definition.type_id)?;
    let mut registry = REGISTRY.write().unwrap();
    if registry.definitions.contains_key(&definition.type_id) {
        return Err(RegistryError::AlreadyRegistered);
    }
    registry
        .definitions
        .insert(definition.type_id.clone(), Arc::new(definition));
    Ok(())
}

/// Registers a definition on behalf of `plugin_id`. The type id must live under
/// the plugin's own namespace (`<plugin_id>.<something>`), so plugins can never
/// shadow core or other plugins' types.
pub fn register_for_plugin(
    plugin_id: &str,
    definition: TaskDefinition,
) -> Result<(), RegistryError> {
    validate_namespaced(&definition.type_id)?;
    let expected_prefix = format!("{plugin_id}.");
    if !definition.type_id.starts_with(&expected_prefix) {
        return Err(RegistryError::NamespaceMismatch);
    }
    register(definition)
}

/// Removes every definition owned by a plugin (called on plugin unload).
pub fn unregister_plugin_namespace(plugin_id: &str) -> usize {
    let prefix = format!("{plugin_id}.");
    let mut registry = REGISTRY.write().unwrap();
    let before = registry.definitions.len();
    registry
        .definitions
        .retain(|type_id, _| !type_id.starts_with(&prefix));
    before - registry.definitions.len()
}

pub fn definition(type_id: &str) -> Option<Arc<TaskDefinition>> {
    REGISTRY.read().unwrap().definitions.get(type_id).cloned()
}

pub fn all_definitions() -> Vec<Arc<TaskDefinition>> {
    let registry = REGISTRY.read().unwrap();
    let mut definitions: Vec<_> = registry.definitions.values().cloned().collect();
    definitions.sort_by(|left, right| left.type_id.cmp(&right.type_id));
    definitions
}

fn validate_namespaced(type_id: &str) -> Result<(), RegistryError> {
    // A namespace is required: `segment(.segment)+`.
    let valid = type_id
        .split('.')
        .filter(|segment| !segment.is_empty())
        .count()
        >= 2
        && type_id.chars().all(|ch| {
            ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '.' || ch == '_' || ch == '-'
        });
    if valid {
        Ok(())
    } else {
        Err(RegistryError::InvalidTypeId)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_definitions_are_registered() {
        let http = definition("core.download.http").expect("core download definition");
        assert!(http.capabilities.pausable);
        assert_eq!(http.pool, Some(TaskPool::Download));
        assert!(definition("core.install.zip").is_some());
    }

    #[test]
    fn plugin_registration_is_namespace_scoped() {
        let ok = register_for_plugin(
            "example.plugin",
            TaskDefinition::new("example.plugin.convert").title_key("Example.convert"),
        );
        assert_eq!(ok, Ok(()));
        assert!(definition("example.plugin.convert").is_some());

        let mismatch = register_for_plugin(
            "example.plugin",
            TaskDefinition::new("core.install.zip.hijack"),
        );
        assert_eq!(mismatch, Err(RegistryError::NamespaceMismatch));

        // Cleanup for repeated test runs.
        unregister_plugin_namespace("example.plugin");
        assert!(definition("example.plugin.convert").is_none());
    }

    #[test]
    fn unnamespaced_ids_are_rejected() {
        assert_eq!(
            register(TaskDefinition::new("convert")),
            Err(RegistryError::InvalidTypeId)
        );
        assert_eq!(
            register(TaskDefinition::new("Core.Convert")),
            Err(RegistryError::InvalidTypeId)
        );
    }
}
