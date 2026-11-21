#![expect(unsafe_code, reason = "Loading explicitly approved native libraries and validating C ABI tables requires unsafe FFI")]

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use bmcbl_plugin_api::native::abi::{self, NativeModuleApi};
use bmcbl_plugin_runtime::PluginError;
use sha2::{Digest, Sha256};

const MAX_MODULE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_DOWNLOAD_TEXT: usize = 256 * 1024;
static LOADED: Mutex<BTreeMap<String, usize>> = Mutex::new(BTreeMap::new());

pub(super) struct Module {
    library: Option<libloading::Library>,
    plugin: Option<String>,
    pub(super) api: NativeModuleApi,
}

impl Module {
    pub(super) fn load(plugin: &str, sha256: &str) -> Result<Self, PluginError> {
        validate_digest(sha256)?;
        let mut loaded = LOADED.lock().unwrap_or_else(|error| error.into_inner());
        let root = root(plugin);
        if removal_marker(plugin).is_file() {
            return Err(PluginError::denied(
                "native component was removed with its plugin",
            ));
        }
        if !root.join("approved").join(sha256).is_file() {
            return Err(PluginError::denied(
                "native component must be approved by the user",
            ));
        }
        let path = installed_path(plugin, sha256)?;
        verify_digest(&path, sha256)?;
        // SAFETY: this platform binary was installed and approved by its exact digest.
        // It executes with the BMCBL process's authority. Only its fixed ABI is resolved.
        let mut module = unsafe { Self::resolve(&path) }?;
        module.plugin = Some(plugin.to_owned());
        *loaded.entry(plugin.to_owned()).or_default() += 1;
        Ok(module)
    }

    unsafe fn resolve(path: &Path) -> Result<Self, PluginError> {
        // SAFETY: the caller validated and approved this specific absolute module path.
        let library =
            unsafe { open_library(path) }.map_err(|error| PluginError::app(error.to_string()))?;
        // SAFETY: the approved library promises this signature for its fixed symbol.
        let entry = unsafe { library.get::<abi::Entry>(abi::ENTRY) }
            .map_err(|error| PluginError::app(error.to_string()))?;
        // SAFETY: approved native code returns a static, readable header or null.
        let header = unsafe { entry() };
        if header.is_null() || !header.is_aligned() {
            return Err(PluginError::app(
                "native ABI header is missing or misaligned",
            ));
        }
        // SAFETY: header pointer is non-null and aligned; we initially read only the fixed prefix.
        let metadata = unsafe { *header };
        validate_header(metadata)?;
        let table = header.cast::<NativeModuleApi>();
        if !table.is_aligned() {
            return Err(PluginError::app("native ABI table is misaligned"));
        }
        // SAFETY: the validated size covers the current table prefix.
        let api = unsafe { *table };
        if api.create_instance.is_none()
            || api.submit_command.is_none()
            || api.read_events.is_none()
            || api.cancel_command.is_none()
            || api.destroy_instance.is_none()
        {
            return Err(PluginError::app(
                "native ABI table is missing a required operation",
            ));
        }
        Ok(Self {
            library: Some(library),
            plugin: None,
            api,
        })
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        drop(self.library.take());
        let Some(plugin) = self.plugin.take() else {
            return;
        };
        let mut loaded = LOADED.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(count) = loaded.get_mut(&plugin) {
            *count -= 1;
            if *count == 0 {
                loaded.remove(&plugin);
                clean_removed(&plugin);
            }
        }
    }
}

fn validate_header(header: abi::AbiHeader) -> Result<(), PluginError> {
    if header.abi_major != abi::MAJOR
        || header.abi_minor < abi::MINOR
        || header.struct_size < NativeModuleApi::HEADER.struct_size
        || header.reserved != 0
    {
        return Err(PluginError::app(format!(
            "incompatible native ABI {}.{} (table size {})",
            header.abi_major, header.abi_minor, header.struct_size
        )));
    }
    Ok(())
}

fn verify_digest(path: &Path, expected: &str) -> Result<(), PluginError> {
    if digest_file(path)? != expected {
        return Err(PluginError::denied(
            "native component SHA-256 does not match approval",
        ));
    }
    Ok(())
}

fn digest_file(path: &Path) -> Result<String, PluginError> {
    let mut input = File::open(path)
        .map_err(io_error)?
        .take(MAX_MODULE_BYTES + 1);
    let mut hash = Sha256::new();
    let mut buffer = [0; 32 * 1024];
    let mut total = 0;
    loop {
        let read = input.read(&mut buffer).map_err(io_error)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        hash.update(&buffer[..read]);
    }
    if total == 0 || total > MAX_MODULE_BYTES {
        return Err(PluginError::denied("native component size is invalid"));
    }
    Ok(hex::encode(hash.finalize()))
}

fn io_error(error: std::io::Error) -> PluginError {
    PluginError::app(error.to_string())
}

fn validate_digest(value: &str) -> Result<(), PluginError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err(PluginError::invalid_input(
            "expected a lowercase SHA-256 digest",
        ))
    }
}

fn library_name() -> String {
    format!(
        "{}module{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    )
}

pub(super) fn root(plugin: &str) -> std::path::PathBuf {
    crate::utils::file_ops::state_subdir("plugin-native").join(plugin)
}

pub(super) fn stage_download(
    plugin: &str,
    manifest: &Path,
    path: &Path,
    sha: &str,
) -> Result<(), PluginError> {
    validate_digest(sha)?;
    let files = LOADED.lock().unwrap_or_else(|error| error.into_inner());
    if !manifest.is_file() {
        return Err(PluginError::denied(
            "native component was removed with its plugin",
        ));
    }
    let path = download_path(path)?;
    prepare_install(plugin, &files)?;
    install_file(plugin, &path, sha)
}

fn download_path(path: &Path) -> Result<PathBuf, PluginError> {
    let downloads = crate::utils::file_ops::mod_downloads_dir()
        .canonicalize()
        .map_err(io_error)?;
    let path = path.canonicalize().map_err(io_error)?;
    if !path.starts_with(downloads) || !path.is_file() {
        return Err(PluginError::denied(
            "native component must come from the download cache",
        ));
    }
    Ok(path)
}

pub(super) fn read_download_text(path: &Path) -> Result<String, PluginError> {
    let mut bytes = Vec::new();
    File::open(download_path(path)?)
        .map_err(io_error)?
        .take(MAX_DOWNLOAD_TEXT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > MAX_DOWNLOAD_TEXT {
        return Err(PluginError::invalid_input("download text exceeds 256 KiB"));
    }
    String::from_utf8(bytes).map_err(|error| PluginError::invalid_input(error.to_string()))
}

fn installed_path(plugin: &str, sha: &str) -> Result<PathBuf, PluginError> {
    let directory = root(plugin).canonicalize().map_err(io_error)?.join(sha);
    let path = directory.join(library_name()).canonicalize().map_err(io_error)?;
    if !path.starts_with(&directory) || !path.is_file() {
        return Err(PluginError::denied("invalid native component path"));
    }
    Ok(path)
}

pub(super) fn approve_staged(plugin: &str, manifest: &Path, sha: &str) -> Result<(), PluginError> {
    validate_digest(sha)?;
    let _files = LOADED.lock().unwrap_or_else(|error| error.into_inner());
    if !manifest.is_file() || removal_marker(plugin).is_file() {
        return Err(PluginError::denied("native component was removed with its plugin"));
    }
    verify_digest(&installed_path(plugin, sha)?, sha)?;
    write_approval(plugin, sha)
}

fn install_file(plugin: &str, path: &Path, sha: &str) -> Result<(), PluginError> {
    let destination = root(plugin).join(sha);
    fs::create_dir_all(&destination).map_err(io_error)?;
    let target = destination.join(library_name());
    if target.exists() {
        return verify_digest(&target, sha);
    }
    let mut staged = tempfile::NamedTempFile::new_in(destination).map_err(io_error)?;
    let mut input = File::open(&path)
        .map_err(io_error)?
        .take(MAX_MODULE_BYTES + 1);
    std::io::copy(&mut input, &mut staged).map_err(io_error)?;
    verify_digest(staged.path(), sha)?;
    staged.as_file().sync_all().map_err(io_error)?;
    match staged.persist_noclobber(&target) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            verify_digest(&target, sha)
        }
        Err(error) => Err(io_error(error.error)),
    }
}

pub(super) fn approve_file(plugin: &str, path: &Path) -> Result<String, PluginError> {
    let extension = std::env::consts::DLL_EXTENSION;
    if !path.is_file()
        || !path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case(extension))
    {
        return Err(PluginError::invalid_input(
            "choose a native library for this platform",
        ));
    }
    let sha = digest_file(path)?;
    let files = LOADED.lock().unwrap_or_else(|error| error.into_inner());
    prepare_install(plugin, &files)?;
    install_file(plugin, path, &sha)?;
    write_approval(plugin, &sha)?;
    Ok(sha)
}

fn write_approval(plugin: &str, sha: &str) -> Result<(), PluginError> {
    let approvals = root(plugin).join("approved");
    fs::create_dir_all(&approvals).map_err(io_error)?;
    fs::write(approvals.join(sha), []).map_err(io_error)
}

fn removal_marker(plugin: &str) -> std::path::PathBuf {
    crate::utils::file_ops::state_subdir("plugin-native").join(format!("{plugin}.removed"))
}

pub(super) fn mark_removed(plugin: &str) -> std::io::Result<()> {
    let loaded = LOADED.lock().unwrap_or_else(|error| error.into_inner());
    let path = root(plugin);
    if path.exists() {
        fs::write(removal_marker(plugin), [])?;
        if !loaded.contains_key(plugin) {
            clean_removed(plugin);
        }
    }
    Ok(())
}

fn clean_removed(plugin: &str) {
    if removal_marker(plugin).is_file()
        && let Err(error) = remove_component_dir(plugin)
    {
        tracing::debug!(%plugin, %error, "native component cleanup pending");
    }
}

fn prepare_install(plugin: &str, loaded: &BTreeMap<String, usize>) -> Result<(), PluginError> {
    let marker = removal_marker(plugin);
    if !marker.is_file() {
        return Ok(());
    }
    if loaded.contains_key(plugin) {
        return Err(PluginError::new(
            "busy",
            "previous native modules are still being released",
        ));
    }
    remove_component_dir(plugin).map_err(io_error)?;
    fs::remove_file(marker).map_err(io_error)
}

fn remove_component_dir(plugin: &str) -> std::io::Result<()> {
    match fs::remove_dir_all(root(plugin)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

unsafe fn open_library(path: &Path) -> Result<libloading::Library, libloading::Error> {
    #[cfg(windows)]
    {
        use libloading::os::windows::{
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, Library,
        };
        // SAFETY: the caller validated the binary. Resolve dependencies from the module and System32.
        unsafe {
            Library::load_with_flags(
                path,
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        }
        .map(Into::into)
    }
    #[cfg(not(windows))]
    {
        // SAFETY: the caller validated the approved absolute module path.
        unsafe { libloading::Library::new(path) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_incompatible_or_short_tables() {
        let valid = NativeModuleApi::HEADER;
        assert!(validate_header(valid).is_ok());
        assert!(
            validate_header(abi::AbiHeader {
                abi_major: valid.abi_major + 1,
                ..valid
            })
            .is_err()
        );
        assert!(
            validate_header(abi::AbiHeader {
                struct_size: valid.struct_size - 1,
                ..valid
            })
            .is_err()
        );
    }
}
