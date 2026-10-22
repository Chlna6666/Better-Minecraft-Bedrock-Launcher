use std::path::{Path, PathBuf};

pub(crate) const DEFAULT_UPDATER_TIMEOUT_SECS: u64 = 60;

pub(crate) const IMPORT_EXTENSIONS: &[&str] = &[
    "mcpack",
    "mcworld",
    "mcaddon",
    "mctemplate",
    "zip",
    "appx",
    "msixvc",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LaunchUiMode {
    /// Startup arguments follow the instance "silent shortcut launch" setting.
    /// The terminal CLI never uses Auto: it defaults to Silent.
    Auto,
    Silent,
    Gui,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaunchOptions {
    pub(crate) version: Option<String>,
    pub(crate) ui_mode: LaunchUiMode,
    pub(crate) game_args: Option<String>,
    pub(crate) json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportLaunchContext {
    pub(crate) file_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaunchTarget {
    pub(crate) folder: String,
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) kind: String,
    pub(crate) package_folder: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectLaunchContext {
    pub(crate) target: LaunchTarget,
    /// Resolved before the GUI starts: true means launch without a window.
    pub(crate) silent: bool,
    pub(crate) game_args: Option<String>,
    pub(crate) json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpdaterLaunchContext {
    pub(crate) source_path: PathBuf,
    pub(crate) destination_path: PathBuf,
    pub(crate) timeout_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LaunchMode {
    Main,
    Import(ImportLaunchContext),
    Updater(UpdaterLaunchContext),
    DirectLaunch(DirectLaunchContext),
}

impl LaunchMode {
    pub(crate) fn is_main(&self) -> bool {
        matches!(self, Self::Main | Self::DirectLaunch(_))
    }

    pub(crate) fn direct_launch(&self) -> Option<&DirectLaunchContext> {
        match self {
            Self::DirectLaunch(context) => Some(context),
            Self::Main | Self::Import(_) | Self::Updater(_) => None,
        }
    }
}

pub(crate) fn is_import_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            IMPORT_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}
/// Splits a launch argument string into argv tokens. Whitespace separates
/// tokens, single/double quotes group text, and a backslash outside quotes
/// escapes the next character. Windows consumes the original string as a raw
/// command line; Linux uses this split so both platforms agree on likely intent.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn split_launch_args(raw: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;

    for character in raw.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            } else {
                current.push(character);
            }
            continue;
        }
        match character {
            '\\' => escaped = true,
            '"' | '\'' => quote = Some(character),
            character if character.is_whitespace() => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            character => current.push(character),
        }
    }
    if escaped {
        current.push('\\');
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::split_launch_args;

    #[test]
    fn splits_plain_arguments_on_whitespace() {
        assert_eq!(
            split_launch_args("-server 127.0.0.1 --name Steve"),
            ["-server", "127.0.0.1", "--name", "Steve"]
        );
    }

    #[test]
    fn preserves_quoted_paths_with_spaces() {
        assert_eq!(
            split_launch_args("\"C:\\Games\\Minecraft Bedrock\\game.exe\" --flag"),
            ["C:\\Games\\Minecraft Bedrock\\game.exe", "--flag"]
        );
        assert_eq!(
            split_launch_args("--name 'Steve Smith'"),
            ["--name", "Steve Smith"]
        );
    }

    #[test]
    fn handles_escaped_spaces_and_empty_input() {
        assert_eq!(
            split_launch_args("--name Steve\\ Smith"),
            ["--name", "Steve Smith"]
        );
        assert!(split_launch_args("   ").is_empty());
    }
}
