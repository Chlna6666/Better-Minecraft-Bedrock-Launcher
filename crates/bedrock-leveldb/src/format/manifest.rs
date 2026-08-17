use crate::coding::{
    get_length_prefixed_slice, get_varint32, get_varint64, put_length_prefixed_slice, put_varint32,
    put_varint64,
};
use crate::error::{LevelDbError, Result};
use crate::wal;
use std::collections::BTreeSet;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

const MANIFEST_MAGIC: &[u8; 9] = b"BWLDBMAN1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TableFileMeta {
    pub(crate) number: u64,
    pub(crate) file_size: u64,
    pub(crate) smallest_key: Option<Vec<u8>>,
    pub(crate) largest_key: Option<Vec<u8>>,
    pub(crate) smallest_internal_key: Option<Vec<u8>>,
    pub(crate) largest_internal_key: Option<Vec<u8>>,
}

impl TableFileMeta {
    #[must_use]
    pub(crate) const fn without_range(number: u64) -> Self {
        Self {
            number,
            file_size: 0,
            smallest_key: None,
            largest_key: None,
            smallest_internal_key: None,
            largest_internal_key: None,
        }
    }

    #[must_use]
    pub(crate) fn native(
        number: u64,
        file_size: u64,
        smallest_internal_key: Vec<u8>,
        largest_internal_key: Vec<u8>,
    ) -> Self {
        Self {
            number,
            file_size,
            smallest_key: internal_user_key(&smallest_internal_key).map(<[u8]>::to_vec),
            largest_key: internal_user_key(&largest_internal_key).map(<[u8]>::to_vec),
            smallest_internal_key: Some(smallest_internal_key),
            largest_internal_key: Some(largest_internal_key),
        }
    }

    #[must_use]
    pub(crate) fn may_contain_user_key(&self, key: &[u8]) -> bool {
        if let Some(smallest_key) = &self.smallest_key {
            if key < smallest_key.as_slice() {
                return false;
            }
        }
        if let Some(largest_key) = &self.largest_key {
            if key > largest_key.as_slice() {
                return false;
            }
        }
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Manifest {
    pub(crate) next_file_number: u64,
    pub(crate) log_number: u64,
    pub(crate) table_numbers: Vec<u64>,
    pub(crate) table_files: Vec<TableFileMeta>,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            next_file_number: 2,
            log_number: 1,
            table_numbers: Vec::new(),
            table_files: Vec::new(),
        }
    }
}

impl Manifest {
    #[must_use]
    pub(crate) const fn current_name() -> &'static str {
        "CURRENT"
    }

    #[must_use]
    pub(crate) fn manifest_name(number: u64) -> String {
        format!("MANIFEST-{number:06}")
    }

    #[must_use]
    pub(crate) fn table_name(number: u64) -> String {
        format!("{number:06}.ldb")
    }

    #[must_use]
    pub(crate) fn log_name(number: u64) -> String {
        format!("{number:06}.log")
    }

    pub(crate) fn load(root: &Path) -> Result<Self> {
        let current_path = root.join(Self::current_name());
        log::trace!("loading manifest pointer {}", current_path.display());
        let manifest_name = fs::read_to_string(&current_path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                LevelDbError::not_found(current_path.clone())
            } else {
                LevelDbError::io_at("read CURRENT", current_path.clone(), error)
            }
        })?;
        let manifest_path = root.join(manifest_name.trim());
        Self::read_file(&manifest_path)
    }

    pub(crate) fn store(&self, root: &Path) -> Result<()> {
        fs::create_dir_all(root)
            .map_err(|error| LevelDbError::io_at("create manifest directory", root, error))?;
        let manifest_name = Self::manifest_name(1);
        let manifest_path = root.join(&manifest_name);
        self.write_file(&manifest_path)?;
        let current_path = root.join(Self::current_name());
        fs::write(&current_path, format!("{manifest_name}\n"))
            .map_err(|error| LevelDbError::io_at("write CURRENT", &current_path, error))?;
        log::trace!("stored manifest {}", manifest_path.display());
        Ok(())
    }

    fn read_file(path: &Path) -> Result<Self> {
        let bytes =
            fs::read(path).map_err(|error| LevelDbError::io_at("read manifest", path, error))?;
        if bytes.starts_with(MANIFEST_MAGIC) {
            return Self::read_bedrock_leveldb_manifest(&bytes, path);
        }
        Self::read_native_leveldb_manifest(path)
    }

    fn read_bedrock_leveldb_manifest(bytes: &[u8], path: &Path) -> Result<Self> {
        if bytes.len() < MANIFEST_MAGIC.len() + 16 {
            return Err(LevelDbError::corruption_at(
                path,
                format!("manifest {} is truncated", path.display()),
            ));
        }
        if &bytes[..MANIFEST_MAGIC.len()] != MANIFEST_MAGIC {
            return Err(LevelDbError::corruption_at(
                path,
                format!("manifest {} has unsupported magic", path.display()),
            ));
        }
        let mut cursor = MANIFEST_MAGIC.len();
        let next_file_number = read_u64(bytes, &mut cursor)?;
        let log_number = read_u64(bytes, &mut cursor)?;
        let table_count = usize::try_from(read_u64(bytes, &mut cursor)?)
            .map_err(|_| LevelDbError::corruption("table count overflow".to_string()))?;
        let mut table_numbers = Vec::with_capacity(table_count);
        for _ in 0..table_count {
            table_numbers.push(read_u64(bytes, &mut cursor)?);
        }
        let table_files = table_numbers
            .iter()
            .copied()
            .map(TableFileMeta::without_range)
            .collect();
        Ok(Self {
            next_file_number,
            log_number,
            table_numbers,
            table_files,
        })
    }

    fn read_native_leveldb_manifest(path: &Path) -> Result<Self> {
        log::trace!("reading native LevelDB manifest {}", path.display());
        let mut file =
            File::open(path).map_err(|error| LevelDbError::io_at("open manifest", path, error))?;
        let records = wal::read_records(&mut file, true)?;
        let mut manifest = Self::default();

        for record in records {
            parse_native_version_edit(&record, &mut manifest)?;
        }

        if manifest.log_number == 0 {
            manifest.log_number = manifest.next_file_number.saturating_sub(1).max(1);
        }
        Ok(manifest)
    }

    fn write_file(&self, path: &Path) -> Result<()> {
        let tmp_path = tmp_path(path);
        self.write_native_file(&tmp_path)?;
        if path.exists() {
            fs::remove_file(path)
                .map_err(|error| LevelDbError::io_at("replace manifest", path, error))?;
        }
        fs::rename(&tmp_path, path)
            .map_err(|error| LevelDbError::io_at("rename manifest temp file", path, error))?;
        Ok(())
    }

    fn write_native_file(&self, path: &Path) -> Result<()> {
        let mut edit = Vec::new();
        put_varint32(1, &mut edit);
        put_length_prefixed_slice(b"leveldb.BytewiseComparator", &mut edit)?;
        put_varint32(2, &mut edit);
        put_varint64(self.log_number, &mut edit);
        put_varint32(3, &mut edit);
        put_varint64(self.next_file_number, &mut edit);
        put_varint32(4, &mut edit);
        put_varint64(self.next_file_number.saturating_add(1024), &mut edit);
        let table_numbers = self
            .table_files
            .iter()
            .map(|table| table.number)
            .collect::<BTreeSet<_>>();
        for table_number in self
            .table_numbers
            .iter()
            .filter(|table_number| !table_numbers.contains(table_number))
        {
            put_varint32(7, &mut edit);
            put_varint32(0, &mut edit);
            put_varint64(*table_number, &mut edit);
            put_varint64(0, &mut edit);
            put_length_prefixed_slice(&[], &mut edit)?;
            put_length_prefixed_slice(&[], &mut edit)?;
        }
        for table in &self.table_files {
            put_varint32(7, &mut edit);
            put_varint32(0, &mut edit);
            put_varint64(table.number, &mut edit);
            put_varint64(table.file_size, &mut edit);
            put_length_prefixed_slice(
                table.smallest_internal_key.as_deref().unwrap_or(&[]),
                &mut edit,
            )?;
            put_length_prefixed_slice(
                table.largest_internal_key.as_deref().unwrap_or(&[]),
                &mut edit,
            )?;
        }

        let mut file = File::create(path)
            .map_err(|error| LevelDbError::io_at("create native manifest", path, error))?;
        wal::append_record(&mut file, &edit)?;
        file.flush()
            .map_err(|error| LevelDbError::io_at("flush native manifest", path, error))?;
        Ok(())
    }
}

fn parse_native_version_edit(mut input: &[u8], manifest: &mut Manifest) -> Result<()> {
    while !input.is_empty() {
        let tag = get_varint32(&mut input)?;
        match tag {
            1 => {
                let _comparator = get_length_prefixed_slice(&mut input)?;
            }
            2 => {
                manifest.log_number = get_varint64(&mut input)?;
            }
            3 => {
                manifest.next_file_number = get_varint64(&mut input)?;
            }
            4 | 9 => {
                let _ = get_varint64(&mut input)?;
            }
            5 => {
                let _level = get_varint32(&mut input)?;
                let _internal_key = get_length_prefixed_slice(&mut input)?;
            }
            6 => {
                let _level = get_varint32(&mut input)?;
                let file_number = get_varint64(&mut input)?;
                manifest
                    .table_numbers
                    .retain(|number| *number != file_number);
                manifest
                    .table_files
                    .retain(|table| table.number != file_number);
            }
            7 => {
                let _level = get_varint32(&mut input)?;
                let file_number = get_varint64(&mut input)?;
                let file_size = get_varint64(&mut input)?;
                let smallest = get_length_prefixed_slice(&mut input)?;
                let largest = get_length_prefixed_slice(&mut input)?;
                manifest.table_numbers.push(file_number);
                manifest.table_files.push(TableFileMeta {
                    number: file_number,
                    file_size,
                    smallest_key: internal_user_key(smallest).map(<[u8]>::to_vec),
                    largest_key: internal_user_key(largest).map(<[u8]>::to_vec),
                    smallest_internal_key: Some(smallest.to_vec()),
                    largest_internal_key: Some(largest.to_vec()),
                });
            }
            other => {
                return Err(LevelDbError::corruption(format!(
                    "unknown native manifest version edit tag {other}"
                )));
            }
        }
    }
    manifest.table_numbers.sort_unstable();
    manifest.table_numbers.dedup();
    manifest.table_files.sort_by_key(|table| table.number);
    manifest.table_files.dedup_by_key(|table| table.number);
    for table_number in &manifest.table_numbers {
        if !manifest
            .table_files
            .iter()
            .any(|table| table.number == *table_number)
        {
            manifest
                .table_files
                .push(TableFileMeta::without_range(*table_number));
        }
    }
    manifest.table_files.sort_by_key(|table| table.number);
    Ok(())
}

fn internal_user_key(internal_key: &[u8]) -> Option<&[u8]> {
    let user_key_len = internal_key.len().checked_sub(8)?;
    internal_key.get(..user_key_len)
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64> {
    let end = cursor.saturating_add(8);
    if end > bytes.len() {
        return Err(LevelDbError::corruption(
            "manifest u64 is truncated".to_string(),
        ));
    }
    let value = u64::from_le_bytes(
        bytes[*cursor..end]
            .try_into()
            .map_err(|_| LevelDbError::corruption("manifest u64 is invalid".to_string()))?,
    );
    *cursor = end;
    Ok(value)
}

fn tmp_path(path: &Path) -> PathBuf {
    path.with_extension("manifesttmp")
}
