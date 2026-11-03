use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bedrock_brarchive::{Acceleration, Archive};
use walkdir::WalkDir;

use crate::{BlockModelError, Result};

/// Read-only source of one pack resource, either a loose file or indexed archive
/// payload. Clones share the open archive and do not copy its content. Logical
/// paths are diagnostic identifiers, not necessarily existing filesystem files.
/// External pack mutation is not synchronized and no version conversion occurs.
#[derive(Clone)]
pub struct ResourceFile {
    path: PathBuf,
    archive: Option<(PathBuf, Arc<Archive>, String)>,
}

impl ResourceFile {
    /// Creates a loose file source without opening or reading it.
    #[must_use]
    pub fn file(path: PathBuf) -> Self {
        Self {
            path,
            archive: None,
        }
    }

    /// Pack-root-relative logical location joined to the pack root. For archive
    /// entries this path must not be passed to filesystem read functions.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn is_archived(&self) -> bool {
        self.archive.is_some()
    }

    /// Reads the original raw bytes. PNG/TGA/JSON/MCB decoding belongs to the
    /// consumer. Archive reads allocate at most 256 MiB and only this payload.
    ///
    /// # Errors
    /// Returns file errors, archive read/size failures or a missing indexed entry.
    pub fn read(&self) -> Result<Vec<u8>> {
        match &self.archive {
            Some((path, archive, name)) => {
                let mut bytes = Vec::new();
                let found = archive.read_into(name, &mut bytes).map_err(|source| {
                    BlockModelError::Archive {
                        path: path.clone(),
                        source,
                    }
                })?;
                if !found {
                    return Err(BlockModelError::Message(format!(
                        "missing archive entry {name}"
                    )));
                }
                Ok(bytes)
            }
            None => std::fs::read(&self.path).map_err(|source| BlockModelError::Read {
                path: self.path.clone(),
                source,
            }),
        }
    }
}

impl fmt::Debug for ResourceFile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResourceFile")
            .field("path", &self.path)
            .field(
                "archive",
                &self.archive.as_ref().map(|(path, _, name)| (path, name)),
            )
            .finish()
    }
}

impl PartialEq for ResourceFile {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
            && self.archive.as_ref().map(|(path, _, name)| (path, name))
                == other.archive.as_ref().map(|(path, _, name)| (path, name))
    }
}

impl Eq for ResourceFile {}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PackResources {
    root: PathBuf,
    archived: BTreeMap<PathBuf, ResourceFile>,
    loose: Option<BTreeMap<PathBuf, ResourceFile>>,
}

impl PackResources {
    pub(crate) fn open(root: &Path) -> Result<Self> {
        let mut resources = Self {
            root: root.to_owned(),
            archived: BTreeMap::new(),
            loose: None,
        };
        // Texture lookups probe many candidates across version overlays. Index loose paths once.
        {
            let mut loose = BTreeMap::new();
            let mut complete = true;
            let entries = WalkDir::new(root)
                .into_iter()
                .filter_entry(|entry| entry.depth() != 1 || entry.file_name() != "__brarchive");
            for entry in entries {
                let Ok(entry) = entry else {
                    complete = false;
                    break;
                };
                if entry.file_type().is_file()
                    || (entry.file_type().is_symlink() && entry.path().is_file())
                {
                    let Ok(relative) = entry.path().strip_prefix(root) else {
                        complete = false;
                        break;
                    };
                    loose.insert(
                        loose_path_key(relative),
                        ResourceFile::file(entry.path().to_owned()),
                    );
                }
            }
            // Keep the filesystem lookup path if any directory could not be scanned.
            if complete {
                resources.loose = Some(loose);
            }
        }
        let archive_root = root.join("__brarchive");
        if !archive_root.is_dir() {
            return Ok(resources);
        }
        for entry in WalkDir::new(&archive_root).sort_by_file_name() {
            let entry = entry.map_err(|source| BlockModelError::Walk {
                path: archive_root.clone(),
                source,
            })?;
            let path = entry.path();
            if path.is_file() && path.extension().is_some_and(|ext| ext == "brarchive") {
                let relative = path
                    .strip_prefix(&archive_root)
                    .map_err(|error| BlockModelError::Message(error.to_string()))?;
                // This consumer only needs block models and textures; sounds,
                // particles and UI archives are independent format consumers.
                if relevant_archive(relative) {
                    resources.add_archive(path, relative)?;
                }
            }
        }
        Ok(resources)
    }

    pub(crate) fn hash_paths(&self, state: &mut impl std::hash::Hasher) -> Option<()> {
        use std::hash::Hash as _;
        let loose = self.loose.as_ref()?;
        // Hash logical paths, not archive offsets or image bytes: cached entries
        // restore sources against the current ordered resource catalogs.
        loose.len().hash(state);
        for path in loose.keys() {
            path.hash(state);
        }
        self.archived.len().hash(state);
        for path in self.archived.keys() {
            path.hash(state);
        }
        Some(())
    }

    fn add_archive(&mut self, path: &Path, relative: &Path) -> Result<()> {
        let archive = Arc::new(Archive::open(path, Acceleration::Auto).map_err(|source| {
            BlockModelError::Archive {
                path: path.to_owned(),
                source,
            }
        })?);
        let directory = relative.with_extension("");
        for entry in archive.index().entries().filter(|entry| !entry.is_empty()) {
            let relative_path = directory.join(entry.name());
            self.archived.insert(
                relative_path.clone(),
                ResourceFile {
                    path: self.root.join(&relative_path),
                    archive: Some((
                        path.to_owned(),
                        Arc::clone(&archive),
                        entry.name().to_owned(),
                    )),
                },
            );
        }
        Ok(())
    }

    pub(crate) fn get(&self, relative: &Path) -> Option<ResourceFile> {
        // A non-empty archived payload wins within a pack; manifest stubs were
        // excluded so they never hide a loose file or fabricate an empty image.
        self.archived.get(relative).cloned().or_else(|| {
            if let Some(loose) = &self.loose {
                return loose.get(&loose_path_key(relative)).cloned();
            }
            let path = self.root.join(relative);
            path.is_file().then(|| ResourceFile::file(path))
        })
    }

    pub(crate) fn json_files(&self, folder: &Path) -> Result<BTreeMap<PathBuf, ResourceFile>> {
        let mut files = BTreeMap::new();
        let directory = self.root.join(folder);
        if directory.is_dir() {
            for entry in WalkDir::new(&directory).sort_by_file_name() {
                let entry = entry.map_err(|source| BlockModelError::Walk {
                    path: directory.clone(),
                    source,
                })?;
                if entry.file_type().is_file() && is_json(entry.path()) {
                    let relative = entry
                        .path()
                        .strip_prefix(&self.root)
                        .map_err(|error| BlockModelError::Message(error.to_string()))?;
                    files.insert(
                        relative.to_owned(),
                        ResourceFile::file(entry.path().to_owned()),
                    );
                }
            }
        }
        for (path, file) in &self.archived {
            if path.starts_with(folder) && is_json(path) {
                files.insert(path.clone(), file.clone());
            }
        }
        Ok(files)
    }
}

fn loose_path_key(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(path.to_string_lossy().to_lowercase())
    }
    #[cfg(not(windows))]
    {
        path.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::PackResources;
    use std::{fs, path::Path};
    use tempfile::TempDir;

    #[test]
    fn indexed_loose_pack_finds_texture_and_root_metadata() {
        let root = TempDir::new().unwrap();
        fs::create_dir(root.path().join("textures")).unwrap();
        fs::write(root.path().join("textures/stone.png"), b"texture").unwrap();
        fs::write(root.path().join("textures/UPPER.PNG"), b"texture").unwrap();
        fs::write(root.path().join("blocks.json"), b"{}").unwrap();

        let pack = PackResources::open(root.path()).unwrap();

        assert!(pack.get(Path::new("textures/stone.png")).is_some());
        #[cfg(windows)]
        assert!(pack.get(Path::new("textures/upper.png")).is_some());
        assert!(pack.get(Path::new("blocks.json")).is_some());
        assert!(pack.get(Path::new("textures/missing.png")).is_none());
    }
}

fn relevant_archive(path: &Path) -> bool {
    [
        "textures",
        "models",
        "blocks",
        "definitions",
        "local_lighting",
    ]
    .iter()
    .any(|directory| {
        path.starts_with(directory) || path == Path::new(&format!("{directory}.brarchive"))
    })
}

fn is_json(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
}
