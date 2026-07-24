//! Generic durable-workflow recovery and handler dispatch.

use std::{
    collections::BTreeMap,
    error::Error,
    sync::{Arc, RwLock},
};

use futures_util::future::BoxFuture;
use thiserror::Error;

use crate::{
    AppTask, DurableTaskId, DurableTaskPhase, DurableTaskRecord, DurableTaskStore,
    DurableTaskStoreError, RuntimeError, TaskCancellation, TaskError, TaskOutcome, TaskScope,
};

type HandlerError = Box<dyn Error + Send + Sync>;

/// Output persisted after a durable handler reaches a successful terminal
/// state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableTaskCompletion {
    /// Opaque handler-owned checkpoint or result metadata.
    pub checkpoint: Vec<u8>,
}

/// Application-owned implementation for one durable task kind.
///
/// The handler must keep all domain semantics outside egpui. It receives the
/// recovered record and a task-scoped cancellation signal, and returns only
/// owned data that the coordinator can persist.
pub trait DurableTaskHandler: Send + Sync + 'static {
    /// Stable handler key matching [`DurableTaskRecord::kind`].
    fn kind(&self) -> &str;

    /// Resumes one recovered record.
    fn resume(
        &self,
        record: DurableTaskRecord,
        cancellation: TaskCancellation,
    ) -> BoxFuture<'static, Result<DurableTaskCompletion, HandlerError>>;
}

/// Failure while registering or running a durable workflow.
#[derive(Debug, Error)]
pub enum DurableWorkflowError {
    /// A handler kind was registered more than once.
    #[error("duplicate durable handler kind `{0}`")]
    DuplicateHandler(String),
    /// No application handler was registered for the record kind.
    #[error("no durable handler registered for `{0}`")]
    MissingHandler(String),
    /// The record explicitly requires a user decision before starting.
    #[error("durable task `{0}` requires user recovery")]
    UserRecovery(DurableTaskId),
    /// The record is already terminal and cannot be resumed.
    #[error("durable task `{0}` is already terminal")]
    AlreadyTerminal(DurableTaskId),
    /// Handler registry coordination failed.
    #[error("durable handler registry is poisoned")]
    RegistryPoisoned,
    /// Durable record persistence failed.
    #[error(transparent)]
    Store(#[from] DurableTaskStoreError),
    /// A persistence operation could not be scheduled or joined.
    #[error("durable persistence worker failed: {0}")]
    PersistenceWorker(#[source] TaskError),
    /// The owning application scope was cancelled.
    #[error("durable workflow was cancelled")]
    Cancelled,
    /// The handler returned an application error.
    #[error("durable handler `{kind}` failed: {source}")]
    Handler {
        /// Handler key.
        kind: String,
        /// Handler failure.
        #[source]
        source: HandlerError,
    },
    /// A handler failed and persisting its failure also failed.
    #[error("durable handler `{kind}` failed and failure persistence also failed")]
    HandlerPersistence {
        /// Handler key.
        kind: String,
        /// Original handler failure.
        #[source]
        source: HandlerError,
        /// Persistence failure.
        persistence: Box<Self>,
    },
    /// Runtime scheduling failed before a task could start.
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
}

/// Coordinates generic durable records without owning any product workflow.
pub struct DurableWorkflowCoordinator {
    scope: TaskScope,
    store: Arc<dyn DurableTaskStore>,
    handlers: Arc<RwLock<BTreeMap<Arc<str>, Arc<dyn DurableTaskHandler>>>>,
}

impl DurableWorkflowCoordinator {
    /// Creates a coordinator for one application scope and durable store.
    #[must_use]
    pub fn new(scope: TaskScope, store: Arc<dyn DurableTaskStore>) -> Self {
        Self {
            scope,
            store,
            handlers: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    /// Registers one application-owned handler.
    ///
    /// # Errors
    ///
    /// Returns an error for duplicate keys or poisoned coordination state.
    pub fn register_handler(
        &self,
        handler: Arc<dyn DurableTaskHandler>,
    ) -> Result<(), DurableWorkflowError> {
        let kind = Arc::<str>::from(handler.kind());
        let mut handlers = self
            .handlers
            .write()
            .map_err(|_| DurableWorkflowError::RegistryPoisoned)?;
        if handlers.insert(kind.clone(), handler).is_some() {
            return Err(DurableWorkflowError::DuplicateHandler(kind.to_string()));
        }
        Ok(())
    }

    /// Loads records and converts interrupted Running work into Queued work.
    ///
    /// Filesystem persistence is executed on the runtime's blocking domain.
    ///
    /// # Errors
    ///
    /// Returns a scheduling error if the application is shutting down.
    pub fn recover(&self) -> Result<AppTask<Vec<DurableTaskRecord>>, DurableWorkflowError> {
        let store = self.store.clone();
        self.scope
            .spawn_blocking(move || crate::recover_interrupted_tasks(store.as_ref()))
            .map_err(DurableWorkflowError::Runtime)
    }

    /// Starts one recovered record with its registered application handler.
    ///
    /// The returned task persists `Running` before invoking the handler, then
    /// persists `Completed`, `Cancelled`, or `Failed` after the handler exits.
    /// If the task is dropped while the handler is running, its last durable
    /// state remains recoverable as `Running` for the next process start.
    ///
    /// # Errors
    ///
    /// Returns a handler, policy, registry, or scheduling error.
    pub fn resume(
        &self,
        record: DurableTaskRecord,
    ) -> Result<AppTask<DurableTaskRecord>, DurableWorkflowError> {
        if record.phase.is_terminal() {
            return Err(DurableWorkflowError::AlreadyTerminal(record.id));
        }
        if matches!(record.recovery, crate::DurableTaskRecovery::RequireUser) {
            return Err(DurableWorkflowError::UserRecovery(record.id));
        }
        let handler = self.handler_for(&record.kind)?;
        let kind = handler.kind().to_owned();
        let store = self.store.clone();
        let execution_scope = self.scope.clone();
        self.scope
            .spawn_io_with_cancellation(move |cancellation| async move {
                let mut running = record.clone();
                running.set_phase(DurableTaskPhase::Running);
                persist_record(&execution_scope, store.clone(), running.clone()).await?;

                match handler.resume(record, cancellation.clone()).await {
                    Ok(_completion) if cancellation.is_cancelled() => {
                        running.set_phase(DurableTaskPhase::Cancelled);
                        persist_record(&execution_scope, store, running).await?;
                        Err(DurableWorkflowError::Cancelled)
                    }
                    Ok(completion) => {
                        running.checkpoint = completion.checkpoint;
                        running.error = None;
                        running.set_phase(DurableTaskPhase::Completed);
                        persist_record(&execution_scope, store, running.clone()).await?;
                        Ok(running)
                    }
                    Err(error) => {
                        let handler_error = DurableWorkflowError::Handler {
                            kind,
                            source: error,
                        };
                        running.fail(handler_error.to_string());
                        match persist_record(&execution_scope, store, running).await {
                            Ok(()) => Err(handler_error),
                            Err(persistence) => Err(DurableWorkflowError::HandlerPersistence {
                                kind: handler_error_kind(&handler_error),
                                source: handler_error_source(handler_error),
                                persistence: Box::new(persistence),
                            }),
                        }
                    }
                }
            })
            .map_err(DurableWorkflowError::Runtime)
    }

    fn handler_for(&self, kind: &str) -> Result<Arc<dyn DurableTaskHandler>, DurableWorkflowError> {
        let handlers = self
            .handlers
            .read()
            .map_err(|_| DurableWorkflowError::RegistryPoisoned)?;
        handlers
            .get(kind)
            .cloned()
            .ok_or_else(|| DurableWorkflowError::MissingHandler(kind.to_owned()))
    }
}

async fn persist_record(
    scope: &TaskScope,
    store: Arc<dyn DurableTaskStore>,
    record: DurableTaskRecord,
) -> Result<(), DurableWorkflowError> {
    let task = scope
        .spawn_blocking(move || store.upsert(record))
        .map_err(DurableWorkflowError::Runtime)?;
    match task.await {
        TaskOutcome::Completed(()) => Ok(()),
        TaskOutcome::Cancelled => Err(DurableWorkflowError::Cancelled),
        TaskOutcome::Failed(TaskError::Operation(error)) => {
            match error.downcast::<DurableTaskStoreError>() {
                Ok(error) => Err(DurableWorkflowError::Store(*error)),
                Err(error) => Err(DurableWorkflowError::PersistenceWorker(
                    TaskError::Operation(error),
                )),
            }
        }
        TaskOutcome::Failed(error) => Err(DurableWorkflowError::PersistenceWorker(error)),
    }
}

fn handler_error_kind(error: &DurableWorkflowError) -> String {
    match error {
        DurableWorkflowError::Handler { kind, .. } => kind.clone(),
        _ => String::from("unknown"),
    }
}

fn handler_error_source(error: DurableWorkflowError) -> HandlerError {
    match error {
        DurableWorkflowError::Handler { source, .. } => source,
        _ => Box::new(std::io::Error::other("handler failure lost")),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        io,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use futures_util::future::BoxFuture;

    use super::{
        DurableTaskCompletion, DurableTaskHandler, DurableWorkflowCoordinator, DurableWorkflowError,
    };
    use crate::{
        ApplicationRuntime, DurableTaskId, DurableTaskRecord, DurableTaskStore,
        DurableTaskStoreError, RuntimeConfig, TaskCancellation, TaskOutcome,
    };

    #[derive(Default)]
    struct MemoryStore {
        records: Mutex<BTreeMap<DurableTaskId, DurableTaskRecord>>,
    }

    impl DurableTaskStore for MemoryStore {
        fn load_all(&self) -> Result<Vec<DurableTaskRecord>, DurableTaskStoreError> {
            Ok(self
                .records
                .lock()
                .expect("store lock")
                .values()
                .cloned()
                .collect())
        }

        fn upsert(&self, record: DurableTaskRecord) -> Result<(), DurableTaskStoreError> {
            self.records
                .lock()
                .expect("store lock")
                .insert(record.id.clone(), record);
            Ok(())
        }

        fn remove(&self, id: &DurableTaskId) -> Result<(), DurableTaskStoreError> {
            self.records.lock().expect("store lock").remove(id);
            Ok(())
        }
    }

    struct EchoHandler;

    impl DurableTaskHandler for EchoHandler {
        fn kind(&self) -> &str {
            "echo"
        }

        fn resume(
            &self,
            record: DurableTaskRecord,
            _cancellation: TaskCancellation,
        ) -> BoxFuture<
            'static,
            Result<DurableTaskCompletion, Box<dyn std::error::Error + Send + Sync>>,
        > {
            Box::pin(async move {
                Ok(DurableTaskCompletion {
                    checkpoint: record.checkpoint,
                })
            })
        }
    }

    struct FailingHandler;

    impl DurableTaskHandler for FailingHandler {
        fn kind(&self) -> &str {
            "failing"
        }

        fn resume(
            &self,
            _record: DurableTaskRecord,
            _cancellation: TaskCancellation,
        ) -> BoxFuture<
            'static,
            Result<DurableTaskCompletion, Box<dyn std::error::Error + Send + Sync>>,
        > {
            Box::pin(async {
                Err(Box::new(io::Error::other("backend unavailable"))
                    as Box<dyn std::error::Error + Send + Sync>)
            })
        }
    }

    fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("test runtime")
            .block_on(future)
    }

    #[test]
    fn recovery_requeues_running_and_resumes_registered_handler() {
        let runtime = ApplicationRuntime::new(RuntimeConfig::default()).expect("runtime");
        let scope = runtime.application_scope();
        let store = Arc::new(MemoryStore::default());
        let id = DurableTaskId::new("echo-1").expect("id");
        let mut record = DurableTaskRecord::new(id, "echo", 1, vec![1, 2, 3]);
        record.set_phase(crate::DurableTaskPhase::Running);
        store.upsert(record).expect("seed");

        let coordinator = DurableWorkflowCoordinator::new(scope, store.clone());
        coordinator
            .register_handler(Arc::new(EchoHandler))
            .expect("handler");
        let recovered = block_on(coordinator.recover().expect("recover"));
        let recovered = match recovered {
            TaskOutcome::Completed(records) => records,
            outcome => panic!("unexpected recovery outcome: {outcome:?}"),
        };
        assert_eq!(recovered.len(), 1);
        let completed = block_on(coordinator.resume(recovered[0].clone()).expect("resume"));
        let completed = match completed {
            TaskOutcome::Completed(record) => record,
            outcome => panic!("unexpected resume outcome: {outcome:?}"),
        };
        assert_eq!(completed.phase, crate::DurableTaskPhase::Completed);
        assert_eq!(completed.checkpoint, vec![1, 2, 3]);
        assert_eq!(
            store.load_all().expect("load")[0].phase,
            crate::DurableTaskPhase::Completed
        );
        assert!(
            runtime
                .shutdown(Duration::from_secs(1))
                .expect("shutdown")
                .drained()
        );
    }

    #[test]
    fn missing_handler_is_reported_before_scheduling() {
        let runtime = ApplicationRuntime::new(RuntimeConfig::default()).expect("runtime");
        let coordinator = DurableWorkflowCoordinator::new(
            runtime.application_scope(),
            Arc::new(MemoryStore::default()),
        );
        let record = DurableTaskRecord::new(
            DurableTaskId::new("missing-1").expect("id"),
            "missing",
            1,
            Vec::new(),
        );
        assert!(matches!(
            coordinator.resume(record),
            Err(DurableWorkflowError::MissingHandler(_))
        ));
        runtime.shutdown(Duration::from_secs(1)).expect("shutdown");
    }

    #[test]
    fn handler_failure_is_persisted_as_terminal_record() {
        let runtime = ApplicationRuntime::new(RuntimeConfig::default()).expect("runtime");
        let store = Arc::new(MemoryStore::default());
        let record = DurableTaskRecord::new(
            DurableTaskId::new("failing-1").expect("id"),
            "failing",
            1,
            Vec::new(),
        );
        store.upsert(record.clone()).expect("seed");
        let coordinator =
            DurableWorkflowCoordinator::new(runtime.application_scope(), store.clone());
        coordinator
            .register_handler(Arc::new(FailingHandler))
            .expect("handler");

        let outcome = block_on(coordinator.resume(record).expect("resume"));
        assert!(matches!(outcome, TaskOutcome::Failed(_)));
        let persisted = store.load_all().expect("load");
        assert_eq!(persisted[0].phase, crate::DurableTaskPhase::Failed);
        assert_eq!(
            persisted[0].error.as_deref(),
            Some("durable handler `failing` failed: backend unavailable")
        );
        runtime.shutdown(Duration::from_secs(1)).expect("shutdown");
    }

    #[allow(dead_code)]
    fn _error_type_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<io::Error>();
    }
}
