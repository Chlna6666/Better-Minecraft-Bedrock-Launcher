use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Mutex;

use crate::{Acceleration, Error, Index};

/// Indexed, read-only file access. The file stays open for this object's lifetime.
///
/// Concurrent reads serialize access to this file's cursor. Streaming also
/// holds that lock while writing to the output. No global lock or executor is
/// created. External mutation is not prevented; this
/// is not an immutable snapshot and callers must avoid modifying active packs.
#[derive(Debug)]
pub struct Archive {
    index: Index,
    file: Mutex<File>,
}

impl Archive {
    /// Opens a container and validates its index without loading any payload.
    /// `acceleration` controls CPU metadata validation, not payload decoding.
    ///
    /// # Errors
    /// Returns file access errors, unsupported versions, malformed entries,
    /// invalid ranges or the index limit described by [`Index::read`].
    pub fn open(path: &Path, acceleration: Acceleration) -> Result<Self, Error> {
        let mut file = File::open(path)?;
        let index = Index::read(&mut BufReader::new(&mut file), acceleration)?;
        Ok(Self {
            index,
            file: Mutex::new(file),
        })
    }

    /// Validated metadata shared by all reads from this file.
    #[must_use]
    pub fn index(&self) -> &Index {
        &self.index
    }

    /// Reads one raw payload into a reusable caller buffer. A missing entry
    /// returns `false` and clears the buffer; a stub returns `true` with no bytes.
    /// Allocation is limited to 256 MiB per entry. Use [`Self::copy_to`] for
    /// larger payloads. No content decoding or game resource mutation occurs.
    ///
    /// # Errors
    /// Returns I/O errors (including truncation after open), lock poisoning,
    /// allocation failure or a payload exceeding 256 MiB.
    pub fn read_into(&self, name: &str, buffer: &mut Vec<u8>) -> Result<bool, Error> {
        buffer.clear();
        let Some(entry) = self.index.get(name) else {
            return Ok(false);
        };
        if entry.len() > 256 * 1024 * 1024 {
            return Err(Error::LimitExceeded("payload allocation exceeds 256 MiB"));
        }
        let length = usize::try_from(entry.len())
            .map_err(|_| Error::LimitExceeded("payload exceeds address space"))?;
        buffer
            .try_reserve(length)
            .map_err(|_| Error::LimitExceeded("payload allocation failed"))?;
        buffer.resize(length, 0);
        let mut file = self
            .file
            .lock()
            .map_err(|_| Error::Invalid("file lock poisoned"))?;
        file.seek(SeekFrom::Start(entry.offset()))?;
        if let Err(error) = file.read_exact(buffer) {
            buffer.clear();
            return Err(error.into());
        }
        Ok(true)
    }

    /// Streams one payload to a caller-owned output with bounded temporary memory.
    /// Returns `None` for a missing entry and `Some(0)` for a stub. Output may
    /// already contain partial bytes on error; the caller owns rollback policy.
    /// The output must not re-enter this archive while its cursor lock is held.
    ///
    /// # Errors
    /// Returns seek/read/write errors, a poisoned file lock or truncated content.
    pub fn copy_to(&self, name: &str, output: &mut impl Write) -> Result<Option<u64>, Error> {
        let Some(entry) = self.index.get(name) else {
            return Ok(None);
        };
        let mut file = self
            .file
            .lock()
            .map_err(|_| Error::Invalid("file lock poisoned"))?;
        file.seek(SeekFrom::Start(entry.offset()))?;
        let copied = std::io::copy(&mut (&mut *file).take(u64::from(entry.len())), output)?;
        if copied != u64::from(entry.len()) {
            return Err(Error::Invalid("truncated payload after open"));
        }
        Ok(Some(copied))
    }
}
