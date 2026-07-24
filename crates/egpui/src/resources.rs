use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{self, Read},
    path::{Component, Path, PathBuf},
    str::FromStr,
    sync::{Arc, RwLock},
};

use anyhow::anyhow;
use gpui::{AssetSource, SharedString};
use thiserror::Error;

/// A normalized namespaced resource identifier such as `app:icons/save.svg`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ResourceId {
    namespace: Arc<str>,
    path: Arc<str>,
}

impl ResourceId {
    /// Parses and normalizes a resource identifier.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid namespace, empty path, platform
    /// separators, or traversal components.
    pub fn new(value: impl AsRef<str>) -> Result<Self, ResourceIdError> {
        let value = value.as_ref();
        let Some((namespace, path)) = value.split_once(':') else {
            return Err(ResourceIdError::MissingNamespace);
        };
        if namespace.is_empty()
            || !namespace
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(ResourceIdError::InvalidNamespace(namespace.to_owned()));
        }
        if path.is_empty()
            || path.contains('\\')
            || path.starts_with('/')
            || path
                .split('/')
                .any(|component| component.is_empty() || component == "." || component == "..")
        {
            return Err(ResourceIdError::InvalidPath(path.to_owned()));
        }
        Ok(Self {
            namespace: Arc::from(namespace),
            path: Arc::from(path),
        })
    }

    /// Returns the namespace.
    #[must_use]
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Returns the normalized path within the namespace.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }
}

impl std::fmt::Display for ResourceId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}:{}", self.namespace, self.path)
    }
}

impl FromStr for ResourceId {
    type Err = ResourceIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

/// Invalid resource identifier.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ResourceIdError {
    /// The `namespace:path` separator was absent.
    #[error("resource identifier must contain a namespace followed by `:`")]
    MissingNamespace,
    /// The namespace was not portable.
    #[error("resource namespace `{0}` must use lowercase ASCII letters, digits, or hyphens")]
    InvalidNamespace(String),
    /// The logical path was not portable or attempted traversal.
    #[error("resource path `{0}` must be relative, normalized, and use `/` separators")]
    InvalidPath(String),
}

/// Optional metadata associated with a resolved resource.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResourceMetadata {
    /// MIME content type when known.
    pub content_type: Option<Arc<str>>,
    /// Exact uncompressed byte length when known.
    pub byte_length: Option<u64>,
    /// Lowercase SHA-256 digest when known.
    pub sha256: Option<Arc<str>>,
    /// Integer image scale, for example `2` for `@2x`.
    pub scale: Option<u16>,
}

/// Physical source of resource bytes.
#[derive(Clone, Debug)]
pub enum ResourceSource {
    /// Immutable bytes already resident in memory.
    Memory(Arc<[u8]>),
    /// A file validated by the owning resource pack.
    File(PathBuf),
}

/// A resolved resource with lazy byte access.
#[derive(Clone, Debug)]
pub struct ResourceHandle {
    id: ResourceId,
    source: ResourceSource,
    metadata: ResourceMetadata,
}

impl ResourceHandle {
    /// Creates a resource backed by immutable memory.
    #[must_use]
    pub fn from_bytes(
        id: ResourceId,
        bytes: impl Into<Arc<[u8]>>,
        metadata: ResourceMetadata,
    ) -> Self {
        Self {
            id,
            source: ResourceSource::Memory(bytes.into()),
            metadata,
        }
    }

    /// Creates a resource backed by a validated filesystem path.
    #[must_use]
    pub fn from_file(id: ResourceId, path: PathBuf, metadata: ResourceMetadata) -> Self {
        Self {
            id,
            source: ResourceSource::File(path),
            metadata,
        }
    }

    /// Returns the logical identifier.
    #[must_use]
    pub fn id(&self) -> &ResourceId {
        &self.id
    }

    /// Returns the resource metadata.
    #[must_use]
    pub const fn metadata(&self) -> &ResourceMetadata {
        &self.metadata
    }

    /// Opens a new byte reader.
    ///
    /// # Errors
    ///
    /// Returns an IO error when a file-backed resource cannot be opened.
    pub fn open(&self) -> io::Result<Box<dyn Read + Send>> {
        match &self.source {
            ResourceSource::Memory(bytes) => Ok(Box::new(io::Cursor::new(bytes.clone()))),
            ResourceSource::File(path) => Ok(Box::new(File::open(path)?)),
        }
    }

    /// Reads the resource with an explicit maximum allocation.
    ///
    /// # Errors
    ///
    /// Returns an IO error or [`io::ErrorKind::FileTooLarge`] when the limit is
    /// exceeded.
    pub fn read_to_end(&self, maximum_bytes: usize) -> io::Result<Vec<u8>> {
        if self
            .metadata
            .byte_length
            .is_some_and(|length| length > maximum_bytes as u64)
        {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                format!("resource {} exceeds {maximum_bytes} bytes", self.id),
            ));
        }
        let limit = u64::try_from(maximum_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let mut reader = self.open()?.take(limit);
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        if bytes.len() > maximum_bytes {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                format!("resource {} exceeds {maximum_bytes} bytes", self.id),
            ));
        }
        Ok(bytes)
    }
}

/// One ordered source of resources.
pub trait ResourcePack: Send + Sync + 'static {
    /// Stable pack name used for diagnostics and duplicate detection.
    fn name(&self) -> &str;
    /// Higher priorities override lower priorities.
    fn priority(&self) -> i32;
    /// Resolves one resource, returning `None` when the pack does not contain it.
    fn load(&self, id: &ResourceId) -> Result<Option<ResourceHandle>, ResourceResolverError>;
    /// Lists resource identifiers beneath a namespace and path prefix.
    fn list(
        &self,
        namespace: &str,
        path_prefix: &str,
    ) -> Result<Vec<ResourceId>, ResourceResolverError>;
}

/// An immutable in-memory resource pack.
#[derive(Clone, Debug)]
pub struct MemoryResourcePack {
    name: Arc<str>,
    priority: i32,
    resources: BTreeMap<ResourceId, ResourceHandle>,
}

impl MemoryResourcePack {
    /// Creates an empty memory pack.
    #[must_use]
    pub fn new(name: impl Into<Arc<str>>, priority: i32) -> Self {
        Self {
            name: name.into(),
            priority,
            resources: BTreeMap::new(),
        }
    }

    /// Creates a pack from a generated `include_bytes!` table.
    ///
    /// # Errors
    ///
    /// Returns the first invalid generated identifier.
    pub fn from_static(
        name: impl Into<Arc<str>>,
        namespace: &str,
        priority: i32,
        resources: &[(&str, &'static [u8])],
    ) -> Result<Self, ResourceIdError> {
        let mut pack = Self::new(name, priority);
        for (path, bytes) in resources {
            let id = ResourceId::new(format!("{namespace}:{path}"))?;
            pack.insert(
                id,
                Arc::<[u8]>::from(*bytes),
                ResourceMetadata {
                    byte_length: Some(bytes.len() as u64),
                    ..ResourceMetadata::default()
                },
            );
        }
        Ok(pack)
    }

    /// Inserts immutable bytes and returns the replaced handle, if any.
    pub fn insert(
        &mut self,
        id: ResourceId,
        bytes: impl Into<Arc<[u8]>>,
        metadata: ResourceMetadata,
    ) -> Option<ResourceHandle> {
        self.resources
            .insert(id.clone(), ResourceHandle::from_bytes(id, bytes, metadata))
    }
}

impl ResourcePack for MemoryResourcePack {
    fn name(&self) -> &str {
        &self.name
    }

    fn priority(&self) -> i32 {
        self.priority
    }

    fn load(&self, id: &ResourceId) -> Result<Option<ResourceHandle>, ResourceResolverError> {
        Ok(self.resources.get(id).cloned())
    }

    fn list(
        &self,
        namespace: &str,
        path_prefix: &str,
    ) -> Result<Vec<ResourceId>, ResourceResolverError> {
        Ok(self
            .resources
            .keys()
            .filter(|id| id.namespace() == namespace && id.path().starts_with(path_prefix))
            .cloned()
            .collect())
    }
}

/// A development-only directory overlay constrained to one canonical root.
#[derive(Clone, Debug)]
pub struct DirectoryResourcePack {
    name: Arc<str>,
    namespace: Arc<str>,
    root: Arc<PathBuf>,
    priority: i32,
}

impl DirectoryResourcePack {
    /// Creates a directory pack after canonicalizing its root.
    ///
    /// # Errors
    ///
    /// Returns an error when the namespace is invalid or the directory cannot
    /// be canonicalized.
    pub fn new(
        name: impl Into<Arc<str>>,
        namespace: impl Into<Arc<str>>,
        root: impl AsRef<Path>,
        priority: i32,
    ) -> Result<Self, ResourceResolverError> {
        let namespace = namespace.into();
        ResourceId::new(format!("{namespace}:probe"))
            .map_err(ResourceResolverError::InvalidIdentifier)?;
        let root = root
            .as_ref()
            .canonicalize()
            .map_err(|source| ResourceResolverError::Io {
                path: root.as_ref().to_owned(),
                source,
            })?;
        if !root.is_dir() {
            return Err(ResourceResolverError::InvalidDirectory(root));
        }
        Ok(Self {
            name: name.into(),
            namespace,
            root: Arc::new(root),
            priority,
        })
    }

    fn resolve_path(&self, id: &ResourceId) -> Result<Option<PathBuf>, ResourceResolverError> {
        if id.namespace() != self.namespace.as_ref() {
            return Ok(None);
        }
        let candidate = id
            .path()
            .split('/')
            .fold(self.root.as_path().to_owned(), |path, component| {
                path.join(component)
            });
        if !candidate.exists() {
            return Ok(None);
        }
        let canonical = candidate
            .canonicalize()
            .map_err(|source| ResourceResolverError::Io {
                path: candidate,
                source,
            })?;
        if !canonical.starts_with(self.root.as_path()) || !canonical.is_file() {
            return Err(ResourceResolverError::EscapedDirectory(id.clone()));
        }
        Ok(Some(canonical))
    }
}

impl ResourcePack for DirectoryResourcePack {
    fn name(&self) -> &str {
        &self.name
    }

    fn priority(&self) -> i32 {
        self.priority
    }

    fn load(&self, id: &ResourceId) -> Result<Option<ResourceHandle>, ResourceResolverError> {
        let Some(path) = self.resolve_path(id)? else {
            return Ok(None);
        };
        let metadata = std::fs::metadata(&path).map_err(|source| ResourceResolverError::Io {
            path: path.clone(),
            source,
        })?;
        Ok(Some(ResourceHandle::from_file(
            id.clone(),
            path,
            ResourceMetadata {
                byte_length: Some(metadata.len()),
                ..ResourceMetadata::default()
            },
        )))
    }

    fn list(
        &self,
        namespace: &str,
        path_prefix: &str,
    ) -> Result<Vec<ResourceId>, ResourceResolverError> {
        if namespace != self.namespace.as_ref() {
            return Ok(Vec::new());
        }
        let mut identifiers = Vec::new();
        visit_directory(self.root.as_path(), self.root.as_path(), &mut identifiers)?;
        identifiers
            .into_iter()
            .filter(|path| path.starts_with(path_prefix))
            .map(|path| {
                ResourceId::new(format!("{namespace}:{path}"))
                    .map_err(ResourceResolverError::InvalidIdentifier)
            })
            .collect()
    }
}

/// Resource resolution and pack registration failure.
#[derive(Debug, Error)]
pub enum ResourceResolverError {
    /// A pack name was registered more than once.
    #[error("resource pack `{0}` is already registered")]
    DuplicatePack(String),
    /// A directory pack root is invalid.
    #[error("resource directory `{0}` is not a directory")]
    InvalidDirectory(PathBuf),
    /// A symlink or filesystem path escaped its declared overlay root.
    #[error("resource `{0}` escaped its directory pack root")]
    EscapedDirectory(ResourceId),
    /// A resource identifier is invalid.
    #[error(transparent)]
    InvalidIdentifier(#[from] ResourceIdError),
    /// Filesystem access failed.
    #[error("resource filesystem operation failed for `{path}`: {source}")]
    Io {
        /// Filesystem path involved in the operation.
        path: PathBuf,
        /// Underlying filesystem error.
        source: io::Error,
    },
    /// Resource resolver coordination state was poisoned by a panic.
    #[error("resource resolver coordination state is poisoned")]
    Poisoned,
}

/// Ordered, thread-safe resource resolution service.
#[derive(Clone, Default)]
pub struct ResourceResolver {
    packs: Arc<RwLock<Vec<Arc<dyn ResourcePack>>>>,
}

impl ResourceResolver {
    /// Creates an empty resolver.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a pack and reorders packs by priority and stable name.
    ///
    /// # Errors
    ///
    /// Returns an error when the name is already registered or the lock is
    /// poisoned.
    pub fn register(&self, pack: impl ResourcePack) -> Result<(), ResourceResolverError> {
        self.register_shared(Arc::new(pack))
    }

    /// Registers a dynamically dispatched pack.
    ///
    /// # Errors
    ///
    /// Returns an error when the name is already registered or the lock is
    /// poisoned.
    pub fn register_shared(
        &self,
        pack: Arc<dyn ResourcePack>,
    ) -> Result<(), ResourceResolverError> {
        let mut packs = self
            .packs
            .write()
            .map_err(|_| ResourceResolverError::Poisoned)?;
        if packs.iter().any(|current| current.name() == pack.name()) {
            return Err(ResourceResolverError::DuplicatePack(pack.name().to_owned()));
        }
        packs.push(pack);
        packs.sort_by(|left, right| {
            right
                .priority()
                .cmp(&left.priority())
                .then_with(|| left.name().cmp(right.name()))
        });
        Ok(())
    }

    /// Resolves a resource from the highest-priority pack that contains it.
    ///
    /// # Errors
    ///
    /// Returns a pack or coordination failure.
    pub fn resolve(
        &self,
        id: &ResourceId,
    ) -> Result<Option<ResourceHandle>, ResourceResolverError> {
        let packs = self
            .packs
            .read()
            .map_err(|_| ResourceResolverError::Poisoned)?
            .clone();
        for pack in packs {
            if let Some(handle) = pack.load(id)? {
                return Ok(Some(handle));
            }
        }
        Ok(None)
    }

    /// Lists the de-duplicated logical view beneath a prefix.
    ///
    /// # Errors
    ///
    /// Returns a pack or coordination failure.
    pub fn list(
        &self,
        namespace: &str,
        path_prefix: &str,
    ) -> Result<Vec<ResourceId>, ResourceResolverError> {
        let packs = self
            .packs
            .read()
            .map_err(|_| ResourceResolverError::Poisoned)?
            .clone();
        let mut identifiers = BTreeSet::new();
        for pack in packs {
            identifiers.extend(pack.list(namespace, path_prefix)?);
        }
        Ok(identifiers.into_iter().collect())
    }
}

/// GPUI's legacy asset interface backed by a namespaced resource resolver.
#[derive(Clone)]
pub struct ResolverAssetSource {
    resolver: ResourceResolver,
    namespace: Arc<str>,
    maximum_asset_bytes: usize,
}

impl ResolverAssetSource {
    /// Creates a GPUI adapter with an explicit per-asset allocation limit.
    #[must_use]
    pub fn new(
        resolver: ResourceResolver,
        namespace: impl Into<Arc<str>>,
        maximum_asset_bytes: usize,
    ) -> Self {
        Self {
            resolver,
            namespace: namespace.into(),
            maximum_asset_bytes,
        }
    }
}

impl AssetSource for ResolverAssetSource {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let id = ResourceId::new(format!("{}:{path}", self.namespace))?;
        let Some(handle) = self.resolver.resolve(&id)? else {
            return Ok(None);
        };
        let bytes = handle
            .read_to_end(self.maximum_asset_bytes)
            .map_err(|error| anyhow!("failed to load GPUI asset `{id}`: {error}"))?;
        Ok(Some(Cow::Owned(bytes)))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(self
            .resolver
            .list(&self.namespace, path)?
            .into_iter()
            .map(|id| SharedString::from(id.path().to_owned()))
            .collect())
    }
}

fn visit_directory(
    root: &Path,
    current: &Path,
    identifiers: &mut Vec<String>,
) -> Result<(), ResourceResolverError> {
    let entries = std::fs::read_dir(current).map_err(|source| ResourceResolverError::Io {
        path: current.to_owned(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| ResourceResolverError::Io {
            path: current.to_owned(),
            source,
        })?;
        let file_type = entry
            .file_type()
            .map_err(|source| ResourceResolverError::Io {
                path: entry.path(),
                source,
            })?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            visit_directory(root, &path, identifiers)?;
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| ResourceResolverError::Io {
                    path: path.clone(),
                    source: io::Error::new(io::ErrorKind::InvalidData, "path escaped root"),
                })?;
            identifiers.push(normalize_relative_path(relative)?);
        }
    }
    Ok(())
}

fn normalize_relative_path(path: &Path) -> Result<String, ResourceResolverError> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                let Some(value) = value.to_str() else {
                    return Err(ResourceResolverError::Io {
                        path: path.to_owned(),
                        source: io::Error::new(
                            io::ErrorKind::InvalidData,
                            "resource path is not valid UTF-8",
                        ),
                    });
                };
                components.push(value);
            }
            _ => {
                return Err(ResourceResolverError::Io {
                    path: path.to_owned(),
                    source: io::Error::new(
                        io::ErrorKind::InvalidData,
                        "resource path is not relative",
                    ),
                });
            }
        }
    }
    Ok(components.join("/"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{
        MemoryResourcePack, ResourceId, ResourceMetadata, ResourceResolver, ResourceResolverError,
    };

    #[test]
    fn rejects_traversal_and_platform_separators() {
        assert!(ResourceId::new("app:../secret").is_err());
        assert!(ResourceId::new("app:icons\\save.svg").is_err());
        assert!(ResourceId::new("App:icons/save.svg").is_err());
    }

    #[test]
    fn higher_priority_pack_overrides_lower_priority_pack() {
        let id = ResourceId::new("app:data/value.txt").expect("id");
        let mut lower = MemoryResourcePack::new("lower", 0);
        lower.insert(
            id.clone(),
            Arc::<[u8]>::from(b"lower".as_slice()),
            ResourceMetadata::default(),
        );
        let mut higher = MemoryResourcePack::new("higher", 10);
        higher.insert(
            id.clone(),
            Arc::<[u8]>::from(b"higher".as_slice()),
            ResourceMetadata::default(),
        );
        let resolver = ResourceResolver::new();
        resolver.register(lower).expect("lower");
        resolver.register(higher).expect("higher");

        let bytes = resolver
            .resolve(&id)
            .expect("resolve")
            .expect("resource")
            .read_to_end(1024)
            .expect("read");
        assert_eq!(bytes, b"higher");
    }

    #[test]
    fn rejects_duplicate_pack_names() {
        let resolver = ResourceResolver::new();
        resolver
            .register(MemoryResourcePack::new("same", 0))
            .expect("first pack");
        let error = resolver
            .register(MemoryResourcePack::new("same", 1))
            .expect_err("duplicate");
        assert!(matches!(error, ResourceResolverError::DuplicatePack(_)));
    }
}
