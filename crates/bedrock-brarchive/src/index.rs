use std::collections::BTreeMap;
use std::io::{Cursor, Read, Seek, SeekFrom};

use crate::{Acceleration, Error};

const MAGIC: [u8; 8] = [0x7d, 0x27, 0x25, 0xb1, 0xa0, 0x52, 0x70, 0x26];
const HEADER_SIZE: u64 = 16;
const RECORD_SIZE: u64 = 256;
const MAX_ENTRIES: u32 = 65_536;

/// A validated v1 entry descriptor. Payloads may overlap for deduplication.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    name: String,
    offset: u64,
    length: u32,
}

impl Entry {
    /// UTF-8 resource name relative to the archive's corresponding directory.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Absolute byte offset inside the container, after the entry table.
    #[must_use]
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Payload length. Zero-length records may be external-file manifest stubs.
    #[must_use]
    pub fn len(&self) -> u32 {
        self.length
    }

    /// Whether this descriptor has no embedded payload.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Borrows the payload from the original complete container without copying.
    /// No JSON, image or version conversion is performed.
    ///
    /// # Errors
    /// Returns an error if the supplied container does not cover this range.
    pub fn bytes<'a>(&self, container: &'a [u8]) -> Result<&'a [u8], Error> {
        let start = usize::try_from(self.offset)
            .map_err(|_| Error::Invalid("payload offset exceeds address space"))?;
        let length = usize::try_from(self.length)
            .map_err(|_| Error::Invalid("payload length exceeds address space"))?;
        let end = start
            .checked_add(length)
            .ok_or(Error::Invalid("payload range overflow"))?;
        container
            .get(start..end)
            .ok_or(Error::Invalid("truncated payload"))
    }
}

/// Validated metadata with case-sensitive keyed lookup, independent of payloads.
/// Duplicate names use the last record. Unsorted tables and shared ranges are valid.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Index {
    entries: BTreeMap<String, Entry>,
}

impl Index {
    /// Validates a complete container without copying its payloads.
    /// `acceleration` controls metadata validation only; payload bytes and all
    /// format/error rules remain identical for automatic and scalar execution.
    ///
    /// # Errors
    /// Fails for unsupported versions, malformed/truncated tables, invalid UTF-8,
    /// unsafe relative names, out-of-file ranges or more than 65,536 records.
    pub fn parse(container: &[u8], acceleration: Acceleration) -> Result<Self, Error> {
        Self::read(&mut Cursor::new(container), acceleration)
    }

    /// Reads only the v1 header and entry table from a seekable source.
    /// The source is positioned just after the table on success. Names must be
    /// portable relative paths; unknown payload bytes are preserved untouched.
    /// `acceleration` selects CPU metadata validation with a portable fallback.
    ///
    /// # Errors
    /// Returns I/O errors and the same validation failures as [`Self::parse`].
    pub fn read(
        source: &mut (impl Read + Seek),
        acceleration: Acceleration,
    ) -> Result<Self, Error> {
        match acceleration {
            Acceleration::Auto => Self::read_index::<true>(source),
            Acceleration::Scalar => Self::read_index::<false>(source),
        }
    }

    fn read_index<const SIMD: bool>(source: &mut (impl Read + Seek)) -> Result<Self, Error> {
        let file_length = source.seek(SeekFrom::End(0))?;
        source.seek(SeekFrom::Start(0))?;
        let mut header = [0; 16];
        source.read_exact(&mut header)?;
        if header[..8] != MAGIC {
            return Err(Error::Invalid("magic mismatch"));
        }
        let count = integer(&header[8..12]);
        let version = integer(&header[12..16]);
        if version != 1 {
            return Err(Error::UnsupportedVersion(version));
        }
        if count > MAX_ENTRIES {
            return Err(Error::LimitExceeded("more than 65,536 entries"));
        }
        let content_base = HEADER_SIZE + u64::from(count) * RECORD_SIZE;
        if content_base > file_length {
            return Err(Error::Invalid("entry table extends beyond file"));
        }
        // Keep small archives on exactly the scalar builder, amortizing the
        // first CPU detection and SIMD dispatch over a substantial entry table.
        if SIMD && count >= 32 {
            Self::read_entries::<true>(source, count, content_base, file_length)
        } else {
            Self::read_entries::<false>(source, count, content_base, file_length)
        }
    }

    fn read_entries<const SIMD: bool>(
        source: &mut impl Read,
        count: u32,
        content_base: u64,
        file_length: u64,
    ) -> Result<Self, Error> {
        let mut entries = BTreeMap::new();
        for _ in 0..count {
            let mut record = [0; 256];
            source.read_exact(&mut record)?;
            let entry = parse_entry::<SIMD>(&record, content_base, file_length)?;
            entries.insert(entry.name.clone(), entry);
        }
        Ok(Self { entries })
    }

    /// Looks up an exact relative name. A missing entry is distinct from a stub.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Entry> {
        self.entries.get(name)
    }

    /// Enumerates unique descriptors in name order without reading payloads.
    #[must_use]
    pub fn entries(&self) -> impl ExactSizeIterator<Item = &Entry> {
        self.entries.values()
    }
}

fn parse_entry<const SIMD: bool>(
    record: &[u8; 256],
    base: u64,
    file_length: u64,
) -> Result<Entry, Error> {
    let name_length = usize::from(record[0]);
    if name_length == 0 || name_length > 247 {
        return Err(Error::Invalid("entry name length must be 1..=247"));
    }
    let name = std::str::from_utf8(&record[1..=name_length])
        .map_err(|_| Error::Invalid("entry name is not UTF-8"))?;
    let forbidden = if SIMD {
        crate::name::contains_forbidden(name)
    } else {
        name.contains(['\0', ':', '\\'])
    };
    if forbidden || name.split('/').any(|part| matches!(part, "" | "." | "..")) {
        return Err(Error::Invalid("entry name is not a portable relative path"));
    }
    let offset = base + u64::from(integer(&record[248..252]));
    let length = integer(&record[252..256]);
    if offset + u64::from(length) > file_length {
        return Err(Error::Invalid("entry payload extends beyond file"));
    }
    Ok(Entry {
        name: name.to_owned(),
        offset,
        length,
    })
}

fn integer(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}
