//! Unified control entry for task lifecycle commands.
//!
//! Every interactive surface (task panel, plugin bridge, future CLI) routes
//! pause/resume/cancel through [`apply`] so ownership, capability and state
//! transition rules are checked in one place. Worker-side completion
//! ([`crate::tasks::task_manager::finish_task`]) is not a control command and
//! keeps its own typed entry.

use std::sync::Arc;

use crate::tasks::registry::{self, TaskCapabilities};
use crate::tasks::scheduler::{self, TaskPool, TaskPriority};
use crate::tasks::task_manager::{
    TaskOwner, TaskState, cancel_task, pause_task, resume_task, snapshot_arc, task_owner_of,
};

/// Control operations a caller can request on an existing task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskCommand {
    Pause,
    Resume,
    Cancel,
    /// Raises the dispatch priority of a queued task. Running tasks ignore it.
    Prioritize(TaskPriority),
}

impl TaskCommand {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Cancel => "cancel",
            Self::Prioritize(_) => "prioritize",
        }
    }
}

/// Why a control command was rejected. Each variant maps to a distinct caller
/// mistake so UI and plugins can react precisely instead of showing one generic
/// "operation failed".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskCommandError {
    /// No task with this id (already removed, or wrong id).
    TaskNotFound,
    /// The actor does not own the task. Plugins may only control tasks they
    /// created; core callers always pass [`TaskOwner::Core`].
    NotOwned,
    /// The task does not support this operation (e.g. pause on a non-pausable
    /// task).
    CapabilityDenied,
    /// The current state does not allow the requested operation (e.g. pause on
    /// a finished task).
    InvalidTransition { from: TaskState },
}

impl std::fmt::Display for TaskCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TaskNotFound => write!(f, "任务不存在"),
            Self::NotOwned => write!(f, "任务不属于当前调用方"),
            Self::CapabilityDenied => write!(f, "任务不支持该操作"),
            Self::InvalidTransition { from } => {
                write!(f, "当前状态不允许该操作：{}", from.as_str())
            }
        }
    }
}

/// Applies a control command on behalf of `actor`.
///
/// Validation order: existence → ownership → capability → state transition.
/// Capability checks consult the task's registered [`TaskDefinition`] when it
/// declared one, falling back to the per-task `supports_pause` flag. The
/// underlying manager functions re-check the transition under the task lock, so
/// a race between this check and the write degrades to a no-op rather than an
/// illegal state.
pub fn apply(
    actor: &TaskOwner,
    task_id: &str,
    command: TaskCommand,
) -> Result<(), TaskCommandError> {
    let snapshot = snapshot_arc(task_id).ok_or(TaskCommandError::TaskNotFound)?;
    let owner = task_owner_of(task_id).ok_or(TaskCommandError::TaskNotFound)?;
    if !actor_can_control(actor, &owner) {
        return Err(TaskCommandError::NotOwned);
    }

    let state = TaskState::from_status_str(snapshot.status.as_ref())
        .ok_or(TaskCommandError::TaskNotFound)?;
    let capabilities = task_capabilities(task_id, snapshot.supports_pause);

    match command {
        TaskCommand::Pause => {
            if !capabilities.pausable {
                return Err(TaskCommandError::CapabilityDenied);
            }
            match state {
                TaskState::Running | TaskState::Pausing | TaskState::Paused => {}
                _ => return Err(TaskCommandError::InvalidTransition { from: state }),
            }
            // Idempotent: pausing/paused tasks accept the request as success.
            let _ = pause_task(task_id);
            Ok(())
        }
        TaskCommand::Resume => {
            if !capabilities.pausable {
                return Err(TaskCommandError::CapabilityDenied);
            }
            match state {
                TaskState::Running | TaskState::Pausing | TaskState::Paused => {}
                _ => return Err(TaskCommandError::InvalidTransition { from: state }),
            }
            let _ = resume_task(task_id);
            Ok(())
        }
        TaskCommand::Cancel => {
            if !capabilities.cancellable {
                return Err(TaskCommandError::CapabilityDenied);
            }
            if state.is_terminal() {
                return Err(TaskCommandError::InvalidTransition { from: state });
            }
            cancel_task(task_id);
            Ok(())
        }
        TaskCommand::Prioritize(priority) => {
            if !capabilities.prioritizable {
                return Err(TaskCommandError::CapabilityDenied);
            }
            // Only queued tasks can be reordered; running work already holds a
            // slot and finished work cannot come back.
            if state != TaskState::Queued {
                return Err(TaskCommandError::InvalidTransition { from: state });
            }
            if !scheduler::prioritize(task_id, priority) {
                return Err(TaskCommandError::TaskNotFound);
            }
            Ok(())
        }
    }
}

/// Resolves the capabilities a task type allows. Tasks that declared a
/// registered type id use the definition's declarations; tasks without one keep
/// the historical behaviour (cancellable + prioritizable, pause per the
/// per-task flag).
fn task_capabilities(task_id: &str, supports_pause: bool) -> TaskCapabilities {
    if let Some(type_id) = crate::tasks::task_manager::task_definition_id(task_id)
        && let Some(definition) = registry::definition(type_id.as_ref())
    {
        return definition.capabilities;
    }
    TaskCapabilities {
        cancellable: true,
        pausable: supports_pause,
        prioritizable: true,
        retryable: false,
    }
}

/// Pauses or resumes dispatch of an entire pool queue. Running tasks are not
/// affected; only new slot grants stop.
pub fn set_queue_paused(pool: TaskPool, paused: bool) {
    scheduler::set_pool_paused(pool, paused);
}

/// Changes the concurrency limit of a pool. Raising it dispatches queued tasks
/// immediately; lowering it lets running tasks drain before new ones start.
pub fn set_queue_concurrency(pool: TaskPool, concurrency: usize) {
    scheduler::set_pool_concurrency(pool, concurrency);
}

fn actor_can_control(actor: &TaskOwner, owner: &TaskOwner) -> bool {
    match (actor, owner) {
        (TaskOwner::Core, _) => true,
        (TaskOwner::Plugin(actor_id), TaskOwner::Plugin(owner_id)) => {
            Arc::ptr_eq(actor_id, owner_id) || actor_id == owner_id
        }
        (TaskOwner::Plugin(_), TaskOwner::Core) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::stage::TaskStage;
    use crate::tasks::task_manager::{
        TaskFinishStatus, TaskOptions, create_task, create_task_owned, finish_task, remove_task,
    };
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

    fn unique_id(tag: &str) -> String {
        format!(
            "task-command-test-{tag}-{}",
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        )
    }

    #[test]
    fn core_can_pause_and_resume_pausable_task() {
        let task_id = unique_id("pause");
        create_task(
            TaskOptions::new("命令层暂停测试", TaskStage::RUNNING)
                .id(task_id.clone())
                .pausable(true),
        )
        .expect("unique id");
        assert!(apply(&TaskOwner::Core, &task_id, TaskCommand::Pause).is_ok());
        assert!(apply(&TaskOwner::Core, &task_id, TaskCommand::Resume).is_ok());
        finish_task(&task_id, TaskFinishStatus::Completed, None);
        assert!(remove_task(&task_id));
    }

    #[test]
    fn pause_on_non_pausable_task_is_capability_denied() {
        let task_id = unique_id("nonpausable");
        create_task(TaskOptions::new("不可暂停任务", TaskStage::RUNNING).id(task_id.clone()))
            .expect("unique id");
        assert_eq!(
            apply(&TaskOwner::Core, &task_id, TaskCommand::Pause),
            Err(TaskCommandError::CapabilityDenied)
        );
        finish_task(&task_id, TaskFinishStatus::Completed, None);
        assert!(remove_task(&task_id));
    }

    #[test]
    fn pause_on_finished_task_is_invalid_transition() {
        let task_id = unique_id("finished");
        create_task(
            TaskOptions::new("已完成任务", TaskStage::RUNNING)
                .id(task_id.clone())
                .pausable(true),
        )
        .expect("unique id");
        finish_task(&task_id, TaskFinishStatus::Completed, None);
        assert_eq!(
            apply(&TaskOwner::Core, &task_id, TaskCommand::Pause),
            Err(TaskCommandError::InvalidTransition {
                from: TaskState::Succeeded
            })
        );
        assert_eq!(
            apply(&TaskOwner::Core, &task_id, TaskCommand::Cancel),
            Err(TaskCommandError::InvalidTransition {
                from: TaskState::Succeeded
            })
        );
        assert!(remove_task(&task_id));
    }

    #[test]
    fn plugin_cannot_control_core_task() {
        let task_id = unique_id("owned");
        create_task(
            TaskOptions::new("宿主任务", TaskStage::RUNNING)
                .id(task_id.clone())
                .pausable(true),
        )
        .expect("unique id");
        let plugin = TaskOwner::Plugin(Arc::from("example.plugin"));
        assert_eq!(
            apply(&plugin, &task_id, TaskCommand::Pause),
            Err(TaskCommandError::NotOwned)
        );
        finish_task(&task_id, TaskFinishStatus::Completed, None);
        assert!(remove_task(&task_id));
    }

    #[test]
    fn plugin_can_control_own_task_only() {
        let task_id = unique_id("plugin-owned");
        create_task_owned(
            TaskOwner::Plugin(Arc::from("example.plugin")),
            TaskOptions::new("插件任务", TaskStage::RUNNING)
                .id(task_id.clone())
                .pausable(true),
        )
        .expect("unique id");
        let owner = TaskOwner::Plugin(Arc::from("example.plugin"));
        let other = TaskOwner::Plugin(Arc::from("other.plugin"));
        assert!(apply(&owner, &task_id, TaskCommand::Pause).is_ok());
        assert_eq!(
            apply(&other, &task_id, TaskCommand::Pause),
            Err(TaskCommandError::NotOwned)
        );
        assert!(apply(&owner, &task_id, TaskCommand::Resume).is_ok());
        assert!(apply(&owner, &task_id, TaskCommand::Cancel).is_ok());
        finish_task(&task_id, TaskFinishStatus::Cancelled, None);
        assert!(remove_task(&task_id));
    }

    #[test]
    fn missing_task_reports_not_found() {
        assert_eq!(
            apply(
                &TaskOwner::Core,
                "task-command-test-missing",
                TaskCommand::Cancel
            ),
            Err(TaskCommandError::TaskNotFound)
        );
    }
}
