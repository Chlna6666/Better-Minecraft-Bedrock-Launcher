# Startup arguments

Startup arguments are the parameters passed to `BMCBL.exe` **when the program is started**.
Desktop shortcuts, file associations, the updater child process and internal helper processes use
them. They are a separate interface from the terminal CLI:

- Startup arguments: supplied at process creation for OS integration; this document.
- Terminal CLI: subcommands typed in a terminal; see [CLI](CLI.en.md).

The same program distinguishes them from the first argument: `launch`, `import`,
`list`, `info`, `help` or a help/version option use the terminal CLI; `--launch` and
the other startup parameters use this document.

## 1. Canonical arguments

```text
BMCBL.exe --launch <VERSION> [--silent | --gui] [--game-args <ARGS>]
BMCBL.exe --import <FILE>
BMCBL.exe --updater <SRC> <DST> [TIMEOUT]
BMCBL.exe --terminal-host <COMMAND> <RESPONSE>
BMCBL.exe
```

| Argument | Description |
| --- | --- |
| `--launch <VERSION>` | Start the given version. Without `--silent/--gui` it follows the instance “silent shortcut launch” setting. |
| `--silent` | Do not open the main window; start the game directly. |
| `--gui` | Open the main window and start the game automatically. Mutually exclusive with `--silent`; exits with a message when another instance is running, and waits for the agreement/onboarding gates on first run. |
| `--game-args <ARGS>` | Override the game launch arguments; requires `--launch`. Passed as a command-line string and split on whitespace/quotes by the platform; in editor mode it takes precedence over the generated deeplink. |
| `--import <FILE>` | Import a supported file and open the launcher. |
| `--updater <SRC> <DST> [TIMEOUT]` | Internal: updater child process. `TIMEOUT` defaults to 60 seconds. |
| `--terminal-host <COMMAND> <RESPONSE>` | Internal: Windows Terminal console host. |
| No arguments | Open the launcher main window. |

Version resolution and the `VERSION` value for `--launch` match the terminal CLI `launch`
command: an exact version folder is tried first, then display name or game version.

Examples:

```powershell
BMCBL.exe --launch "1.21.0 stable" --silent
BMCBL.exe --launch "1.21.0 stable" --gui
BMCBL.exe --import "C:\Downloads\pack.mcpack"
BMCBL.exe --updater "C:\temp\updater.exe" "C:\Games\BMCBL.exe" 60
```

## 2. Compatibility arguments (deprecated from 0.3.0)

The following legacy arguments remain available until 0.3.0 and are marked deprecated from that
release. New callers should use the canonical arguments.

| Legacy argument | Equivalent canonical argument | Notes |
| --- | --- | --- |
| `--launch-version <VERSION>` | `--launch <VERSION>` | Same semantics. |
| `--import-file <FILE>` | `--import <FILE>` | Unsupported types open the main window. |
| `--run-updater <SRC> <DST> <TIMEOUT>` | `--updater <SRC> <DST> <TIMEOUT>` | Timeout is required; a malformed value falls back to the 60-second default. |
| Bare `<FILE>` | `--import <FILE>` | OS “open with”; unsupported types open the main window. |

`--launch-version` cannot be combined with `--launch`, `--import-file` with
`--import`, or `--run-updater` with `--updater`. `--silent`, `--gui` and
`--game-args` require a launch mode.

The 0.2.x internal `run-updater` and `terminal-host` subcommands and the
`--bmcbl-terminal-host` flag are rewritten to `--updater` / `--terminal-host` and remain
available until 0.3.0.

## 3. OS integration

- Desktop shortcut: `BMCBL.exe --launch "<version folder>"`. The instance setting decides
  whether a window appears.
- File association: `BMCBL.exe --import "%1"` for supported extensions. Bumping the
  association version refreshes the registry entry on the next start.
- Updater: copies the current executable and starts it with
  `--updater <source> <destination> <timeout>`.
- Windows Terminal host: starts a lightweight helper with
  `--terminal-host <command> <response>`.

## 4. Differences from the terminal CLI

| Item | Startup arguments | Terminal CLI |
| --- | --- | --- |
| Trigger | Shortcut, file association, updater, helper process | Subcommand typed in a terminal |
| `--launch` default window | Follows the instance setting | Silent by default |
| JSON output | Not supported | `launch --json` |
| `list` / `info` | Not supported | Supported |
| Updater / terminal host | Internal arguments | Not exposed |

## 5. Platform note

On Linux, `--launch` and `--import` (GUI and silent) reject the root user;
`--updater` and `--terminal-host` are not covered by that check.

## 6. Exit codes

- Startup argument parsing error: `2`.
- Launch, import or update failure: `1`; updater child failure is always `2`.
- Normal start (GUI or completed silent launch): `0`.
