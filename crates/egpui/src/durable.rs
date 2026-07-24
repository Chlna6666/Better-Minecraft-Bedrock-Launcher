use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Stable identifier for a task that may outlive a UI view or process.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DurableTaskId(String);

impl DurableTaskId {
    /// Creates a portable task identifier.
    ///
    /// # Errors
    ///
    /// Returns an error for empty, control-character, or path-like values.
    pub fn new(value: impl Into<String>) -> Result<Self, DurableTaskIdError> {
        let value = value.into();
        if value.trim().is_empty()
            || value.chars().any(char::is_control)
            || value.contains(['\\', '/', ':'])
            || value == "."
            || value == ".."
        {
            return Err(DurableTaskIdError(value));
        }
        Ok(Self(value))
    }

    /// Returns the stable identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DurableTaskId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Invalid durable task identifier.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("durable task identifier `{0}` is not portable")]
pub struct DurableTaskIdError(String);

/// Explicit lifecycle state persisted for a durable workflow.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableTaskPhase {
    /// Waiting for a handler to start or resume it.
    Queued,
    /// Handler is actively performing work.
    Running,
    /// Handler deliberately paused at a checkpoint.
    Paused,
    /// Handler completed and output validation succeeded.
    Completed,
    /// User or application cancellation reached a terminal point.
    Cancelled,
    /// Handler failed and published a useful error.
    Failed,
}

impl DurableTaskPhase {
    /// Returns whether this phase is terminal.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Failed)
    }

    /// Returns whether this phase should be resumed after a process restart.
    #[must_use]
    pub const fn is_recoverable(self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Paused)
    }
}

/// Policy selected by an application-specific durable task handler.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableTaskRecovery {
    /// Resume from the latest valid checkpoint.
    #[default]
    ResumeCheckpoint,
    /// Start again while retaining the old checkpoint for diagnostics.
    Restart,
    /// Surface the task to the user instead of starting automatically.
    RequireUser,
}

/// JSON-serializable task record independent of any BMCBL domain type.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DurableTaskRecord {
    /// Stable task identity.
    pub id: DurableTaskId,
    /// Application-owned handler key, such as `download` or `archive`.
    pub kind: String,
    /// Handler schema version for checkpoint migration.
    pub schema_version: u32,
    /// Current lifecycle phase.
    pub phase: DurableTaskPhase,
    /// Recovery decision selected by the handler or user.
    pub recovery: DurableTaskRecovery,
    /// Opaque handler-owned checkpoint bytes.
    #[serde(default)]
    pub checkpoint: Vec<u8>,
    /// Last useful failure detail, if any.
    #[serde(default)]
    pub error: Option<String>,
    /// Number of recovery or retry attempts.
    pub retry_count: u32,
    /// Unix timestamp in milliseconds.
    pub created_at_millis: u64,
    /// Unix timestamp in milliseconds.
    pub updated_at_millis: u64,
}

impl DurableTaskRecord {
    /// Creates a queued record with an opaque checkpoint.
    ///
    /// # Errors
    ///
    /// Returns an invalid identifier error.
    pub fn new(
        id: DurableTaskId,
        kind: impl Into<String>,
        schema_version: u32,
        checkpoint: Vec<u8>,
    ) -> Self {
        let now = unix_time_millis();
        Self {
            id,
            kind: kind.into(),
            schema_version,
            phase: DurableTaskPhase::Queued,
            recovery: DurableTaskRecovery::default(),
            checkpoint,
            error: None,
            retry_count: 0,
            created_at_millis: now,
            updated_at_millis: now,
        }
    }

    /// Updates the phase and timestamp.
    pub fn set_phase(&mut self, phase: DurableTaskPhase) {
        self.phase = phase;
        self.updated_at_millis = unix_time_millis();
    }

    /// Replaces the opaque checkpoint without changing lifecycle state.
    pub fn set_checkpoint(&mut self, checkpoint: Vec<u8>) {
        self.checkpoint = checkpoint;
        self.updated_at_millis = unix_time_millis();
    }

    /// Publishes a failure as an explicit terminal phase.
    pub fn fail(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
        self.set_phase(DurableTaskPhase::Failed);
    }
}

/// Errors returned by durable checkpoint stores.
#[derive(Debug, Error)]
pub enum DurableTaskStoreError {
    /// The store file could not be read.
    #[error("failed to read durable task store `{path}`: {source}")]
    Read {
        /// Store path.
        path: PathBuf,
        /// Underlying IO error.
        source: io::Error,
    },
    /// The store file contained invalid JSON or a mismatched format version.
    #[error("failed to decode durable task store `{path}`: {source}")]
    Decode {
        /// Store path.
        path: PathBuf,
        /// JSON decoding error.
        source: serde_json::Error,
    },
    /// The store could not be written atomically.
    #[error("failed to write durable task store `{path}`: {source}")]
    Write {
        /// Store path.
        path: PathBuf,
        /// Underlying IO error.
        source: io::Error,
    },
    /// Two records used the same identifier.
    #[error("durable task store contains duplicate task `{0}`")]
    Duplicate(DurableTaskId),
    /// Store coordination state was poisoned.
    #[error("durable task store coordination state is poisoned")]
    Poisoned,
}

/// Synchronous persistence contract.
///
/// Implementations may perform filesystem IO. Callers must invoke them through
/// the application's blocking execution domain and never from GPUI render or
/// input callbacks.
pub trait DurableTaskStore: Send + Sync + 'static {
    /// Loads all records in deterministic identifier order.
    fn load_all(&self) -> Result<Vec<DurableTaskRecord>, DurableTaskStoreError>;
    /// Inserts or replaces one record atomically.
    fn upsert(&self, record: DurableTaskRecord) -> Result<(), DurableTaskStoreError>;
    /// Removes one record after the application has no need to recover it.
    fn remove(&self, id: &DurableTaskId) -> Result<(), DurableTaskStoreError>;
}

#[derive(Serialize, Deserialize)]
struct DurableTaskFile {
    format_version: u32,
    records: Vec<DurableTaskRecord>,
}

const STORE_FORMAT_VERSION: u32 = 1;

/// JSON file-backed durable task store with atomic replacement.
pub struct FileDurableTaskStore {
    path: PathBuf,
    records: Mutex<Option<BTreeMap<DurableTaskId, DurableTaskRecord>>>,
}

impl FileDurableTaskStore {
    /// Creates a lazy file store. The file is read on the first operation.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            records: Mutex::new(None),
        }
    }

    /// Returns the configured file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl DurableTaskStore for FileDurableTaskStore {
    fn load_all(&self) -> Result<Vec<DurableTaskRecord>, DurableTaskStoreError> {
        let records = self.load_records()?;
        Ok(records.values().cloned().collect())
    }

    fn upsert(&self, record: DurableTaskRecord) -> Result<(), DurableTaskStoreError> {
        let mut guard = self
            .records
            .lock()
            .map_err(|_| DurableTaskStoreError::Poisoned)?;
        if guard.is_none() {
            *guard = Some(read_records(&self.path)?);
        }
        let records = guard.as_mut().ok_or(DurableTaskStoreError::Poisoned)?;
        records.insert(record.id.clone(), record);
        write_records(&self.path, records)
    }

    fn remove(&self, id: &DurableTaskId) -> Result<(), DurableTaskStoreError> {
        let mut guard = self
            .records
            .lock()
            .map_err(|_| DurableTaskStoreError::Poisoned)?;
        if guard.is_none() {
            *guard = Some(read_records(&self.path)?);
        }
        let records = guard.as_mut().ok_or(DurableTaskStoreError::Poisoned)?;
        records.remove(id);
        write_records(&self.path, records)
    }
}

impl FileDurableTaskStore {
    fn load_records(
        &self,
    ) -> Result<BTreeMap<DurableTaskId, DurableTaskRecord>, DurableTaskStoreError> {
        let mut guard = self
            .records
            .lock()
            .map_err(|_| DurableTaskStoreError::Poisoned)?;
        if guard.is_none() {
            *guard = Some(read_records(&self.path)?);
        }
        guard.clone().ok_or(DurableTaskStoreError::Poisoned)
    }
}

/// Converts interrupted running tasks into recoverable queued records.
///
/// This function deliberately does not start handlers: the application decides
/// whether a download, archive extraction, or another domain can resume from
/// its checkpoint and then schedules it through its own runtime facade.
pub fn recover_interrupted_tasks(
    store: &dyn DurableTaskStore,
) -> Result<Vec<DurableTaskRecord>, DurableTaskStoreError> {
    let mut records = store.load_all()?;
    let mut recoverable = Vec::new();
    for record in &mut records {
        if record.phase == DurableTaskPhase::Running {
            record.phase = DurableTaskPhase::Queued;
            record.retry_count = record.retry_count.saturating_add(1);
            record.updated_at_millis = unix_time_millis();
            store.upsert(record.clone())?;
        }
        if record.phase.is_recoverable() {
            recoverable.push(record.clone());
        }
    }
    Ok(recoverable)
}

fn read_records(
    path: &Path,
) -> Result<BTreeMap<DurableTaskId, DurableTaskRecord>, DurableTaskStoreError> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(source) => {
            return Err(DurableTaskStoreError::Read {
                path: path.to_owned(),
                source,
            });
        }
    };
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)
        .map_err(|source| DurableTaskStoreError::Read {
            path: path.to_owned(),
            source,
        })?;
    let document = serde_json::from_slice::<DurableTaskFile>(&contents).map_err(|source| {
        DurableTaskStoreError::Decode {
            path: path.to_owned(),
            source,
        }
    })?;
    if document.format_version != STORE_FORMAT_VERSION {
        return Err(DurableTaskStoreError::Decode {
            path: path.to_owned(),
            source: serde_json::Error::io(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported durable task store format version",
            )),
        });
    }
    let mut records = BTreeMap::new();
    for record in document.records {
        let identifier = record.id.clone();
        if records.insert(identifier.clone(), record).is_some() {
            return Err(DurableTaskStoreError::Duplicate(identifier));
        }
    }
    Ok(records)
}

fn write_records(
    path: &Path,
    records: &BTreeMap<DurableTaskId, DurableTaskRecord>,
) -> Result<(), DurableTaskStoreError> {
    let document = DurableTaskFile {
        format_version: STORE_FORMAT_VERSION,
        records: records.values().cloned().collect(),
    };
    let encoded =
        serde_json::to_vec_pretty(&document).map_err(|source| DurableTaskStoreError::Write {
            path: path.to_owned(),
            source: io::Error::new(io::ErrorKind::InvalidData, source),
        })?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| DurableTaskStoreError::Write {
            path: parent.to_owned(),
            source,
        })?;
    }
    let temporary_path = path.with_extension(format!(
        "{}.tmp-{}",
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("json"),
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary_path)
        .map_err(|source| DurableTaskStoreError::Write {
            path: temporary_path.clone(),
            source,
        })?;
    file.write_all(&encoded)
        .and_then(|_| file.sync_all())
        .map_err(|source| DurableTaskStoreError::Write {
            path: temporary_path.clone(),
            source,
        })?;
    drop(file);
    match fs::rename(&temporary_path, path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            // Unix replaces atomically above. Windows may require removing the
            // old file first because its rename API does not replace open files.
            fs::remove_file(path).map_err(|source| DurableTaskStoreError::Write {
                path: path.to_owned(),
                source,
            })?;
            fs::rename(&temporary_path, path).map_err(|source| DurableTaskStoreError::Write {
                path: path.to_owned(),
                source,
            })
        }
        Err(source) => Err(DurableTaskStoreError::Write {
            path: path.to_owned(),
            source,
        }),
    }
}

fn unix_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(u128::from(u64::MAX)) as u64
        })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        DurableTaskId, DurableTaskPhase, DurableTaskRecord, DurableTaskRecovery, DurableTaskStore,
        FileDurableTaskStore, recover_interrupted_tasks,
    };

    #[test]
    fn interrupted_running_task_is_requeued_with_checkpoint() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("egpui-task-store-{unique}.json"));
        let store = FileDurableTaskStore::new(&path);
        let id = DurableTaskId::new("download-1").expect("id");
        let mut record = DurableTaskRecord::new(id.clone(), "download", 1, vec![1, 2, 3]);
        record.set_phase(DurableTaskPhase::Running);
        record.recovery = DurableTaskRecovery::ResumeCheckpoint;
        store.upsert(record).expect("write");

        let recovered = recover_interrupted_tasks(&store).expect("recover");
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].phase, DurableTaskPhase::Queued);
        assert_eq!(recovered[0].checkpoint, vec![1, 2, 3]);
        assert_eq!(recovered[0].retry_count, 1);
        let loaded = store.load_all().expect("load");
        assert_eq!(loaded[0].phase, DurableTaskPhase::Queued);
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn terminal_phases_are_not_recovered() {
        let path =
            std::env::temp_dir().join(format!("egpui-terminal-task-{}.json", std::process::id()));
        let store = FileDurableTaskStore::new(&path);
        let id = DurableTaskId::new("done").expect("id");
        let mut record = DurableTaskRecord::new(id, "test", 1, Vec::new());
        record.set_phase(DurableTaskPhase::Completed);
        store.upsert(record).expect("write");
        assert!(
            recover_interrupted_tasks(&store)
                .expect("recover")
                .is_empty()
        );
        fs::remove_file(path).expect("cleanup");
    }
}
