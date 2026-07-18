#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuildChannel {
    Stable,
    Nightly,
}

impl BuildChannel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Nightly => "nightly",
        }
    }
}

const APP_VERSION: &str = env!("BMCBL_BUILD_VERSION");
const APP_BUILD_CHANNEL: &str = env!("BMCBL_BUILD_CHANNEL");
const APP_LICENSE: &str = env!("CARGO_PKG_LICENSE");
const GIT_COMMIT_HASH: &str = env!("GIT_COMMIT_HASH");
const BUILD_TIME: &str = env!("BUILD_TIME");
const PACKAGE_NAME: &str = env!("CARGO_PKG_NAME");

pub fn get_version() -> &'static str {
    APP_VERSION
}

pub fn build_channel() -> BuildChannel {
    match APP_BUILD_CHANNEL {
        "nightly" => BuildChannel::Nightly,
        _ => BuildChannel::Stable,
    }
}

pub fn get_license() -> &'static str {
    APP_LICENSE
}

pub fn get_build_info() -> String {
    format!(
        "App Version: {}\nBuild Channel: {}\nGit Commit: {}\nBuild Time: {}",
        APP_VERSION,
        build_channel().as_str(),
        GIT_COMMIT_HASH,
        BUILD_TIME
    )
}

pub fn runtime_app_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_stem()
                .map(|file_stem| file_stem.to_string_lossy().trim().to_string())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| PACKAGE_NAME.to_string())
}
