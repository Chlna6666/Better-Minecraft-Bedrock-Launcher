#[cfg(target_os = "linux")]
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(target_os = "windows")]
pub fn bmcbl_dir() -> PathBuf {
    exe_dir().join("BMCBL")
}

#[cfg(target_os = "linux")]
pub fn bmcbl_dir() -> PathBuf {
    linux_xdg_app_dir("XDG_DATA_HOME", &[".local", "share"], Path::new(""))
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn bmcbl_dir() -> PathBuf {
    exe_dir().join("BMCBL")
}

pub fn bmcbl_subdir<P: AsRef<Path>>(rel: P) -> PathBuf {
    bmcbl_dir().join(rel)
}

#[cfg(target_os = "linux")]
pub fn config_dir() -> PathBuf {
    linux_xdg_app_dir("XDG_CONFIG_HOME", &[".config"], Path::new("config"))
}

#[cfg(not(target_os = "linux"))]
pub fn config_dir() -> PathBuf {
    bmcbl_subdir("config")
}

#[cfg(target_os = "linux")]
pub fn cache_dir() -> PathBuf {
    linux_xdg_app_dir("XDG_CACHE_HOME", &[".cache"], Path::new("cache"))
}

#[cfg(not(target_os = "linux"))]
pub fn cache_dir() -> PathBuf {
    bmcbl_subdir("cache")
}

pub fn cache_subdir<P: AsRef<Path>>(rel: P) -> PathBuf {
    cache_dir().join(rel)
}

#[cfg(target_os = "linux")]
pub fn state_dir() -> PathBuf {
    linux_xdg_app_dir("XDG_STATE_HOME", &[".local", "state"], Path::new("state"))
}

#[cfg(not(target_os = "linux"))]
pub fn state_dir() -> PathBuf {
    bmcbl_dir()
}

pub fn state_subdir<P: AsRef<Path>>(rel: P) -> PathBuf {
    state_dir().join(rel)
}

pub fn logs_dir() -> PathBuf {
    state_subdir("logs")
}

#[cfg(target_os = "linux")]
pub fn downloads_dir() -> PathBuf {
    cache_subdir("downloads")
}

#[cfg(not(target_os = "linux"))]
pub fn downloads_dir() -> PathBuf {
    bmcbl_subdir("downloads")
}

pub fn runners_dir() -> PathBuf {
    bmcbl_subdir("runners")
}

pub fn prefixes_dir() -> PathBuf {
    bmcbl_subdir("prefixes")
}

pub fn create_initial_directories() {
    let root = bmcbl_dir();
    let dirs = vec![
        root.clone(),
        config_dir(),
        cache_dir(),
        logs_dir(),
        downloads_dir(),
        bmcbl_subdir("plugins"),
        bmcbl_subdir("music"),
        bmcbl_subdir("versions"),
        cache_subdir("data"),
        cache_subdir("api"),
    ];

    #[cfg(target_os = "linux")]
    let dirs = {
        let mut dirs = dirs;
        dirs.extend([state_dir(), runners_dir(), prefixes_dir()]);
        dirs
    };

    for dir in dirs {
        if let Err(e) = fs::create_dir_all(&dir) {
            eprintln!("Failed to create directory '{}': {}", dir.display(), e);
        }
    }

    #[cfg(target_os = "linux")]
    migrate_legacy_linux_config();
}

#[cfg(target_os = "linux")]
fn linux_xdg_app_dir(variable: &str, home_fallback: &[&str], portable_relative: &Path) -> PathBuf {
    linux_xdg_app_dir_from(
        std::env::var_os(variable).as_deref(),
        std::env::var_os("HOME").as_deref(),
        home_fallback,
        &exe_dir().join(".bmcbl"),
        portable_relative,
    )
}

#[cfg(target_os = "linux")]
fn linux_xdg_app_dir_from(
    configured_base: Option<&OsStr>,
    home: Option<&OsStr>,
    home_fallback: &[&str],
    portable_root: &Path,
    portable_relative: &Path,
) -> PathBuf {
    configured_base
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            home.map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|home| {
                    home_fallback
                        .iter()
                        .fold(home, |path, component| path.join(component))
                })
        })
        .map_or_else(
            || portable_root.join(portable_relative),
            |base| base.join("bmcbl"),
        )
}

#[cfg(target_os = "linux")]
fn migrate_legacy_linux_config() {
    let legacy_config = bmcbl_subdir("config").join("settings.toml");
    let current_config = config_dir().join("settings.toml");
    if current_config.exists() || !legacy_config.is_file() {
        return;
    }

    if let Err(error) = fs::copy(&legacy_config, &current_config) {
        eprintln!(
            "Failed to migrate Linux config '{}' to '{}': {}",
            legacy_config.display(),
            current_config.display(),
            error
        );
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::linux_xdg_app_dir_from;
    use std::ffi::OsStr;
    use std::path::Path;

    #[test]
    fn linux_xdg_app_dir_prefers_absolute_configured_base() {
        let path = linux_xdg_app_dir_from(
            Some(OsStr::new("/tmp/xdg-data")),
            Some(OsStr::new("/home/tester")),
            &[".local", "share"],
            Path::new("/opt/bmcbl/.bmcbl"),
            Path::new(""),
        );

        assert_eq!(path, Path::new("/tmp/xdg-data/bmcbl"));
    }

    #[test]
    fn linux_xdg_app_dir_ignores_relative_configured_base() {
        let path = linux_xdg_app_dir_from(
            Some(OsStr::new("relative/data")),
            Some(OsStr::new("/home/tester")),
            &[".local", "share"],
            Path::new("/opt/bmcbl/.bmcbl"),
            Path::new(""),
        );

        assert_eq!(path, Path::new("/home/tester/.local/share/bmcbl"));
    }

    #[test]
    fn linux_xdg_app_dir_uses_portable_fallback_without_home() {
        let path = linux_xdg_app_dir_from(
            None,
            None,
            &[".config"],
            Path::new("/opt/bmcbl/.bmcbl"),
            Path::new("config"),
        );

        assert_eq!(path, Path::new("/opt/bmcbl/.bmcbl/config"));
    }
}
