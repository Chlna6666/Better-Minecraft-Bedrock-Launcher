# Terminal command-line interface

BMCBL exposes two independent entry points:

- **Terminal CLI**: this document. Users type `BMCBL launch`, `BMCBL list` and similar
  subcommands in a terminal.
- **Startup arguments**: parameters passed when the program is started by a shortcut, file
  association or updater. See [STARTUP_ARGUMENTS](STARTUP_ARGUMENTS.en.md).

The same `BMCBL.exe` chooses between them from the first argument: `launch`, `import`,
`list`, `info`, `help` or a help/version option selects the terminal CLI; anything else is
parsed as startup arguments. No arguments opens the main window.

## 1. Commands

```text
BMCBL launch [VERSION] [OPTIONS]
BMCBL import <FILE>
BMCBL list [--json]
BMCBL info [--json]
BMCBL help
```

`BMCBL --help` prints terminal command help and `BMCBL --version` prints the build version.

### 1.1 launch

A terminal launch is **silent by default**: no main window, the game starts directly. Use
`--gui` explicitly to show the launcher.

| Argument | Description |
| --- | --- |
| `VERSION` | Optional. Version folder, display name, or game version. When omitted, the first version after sorting is used (session count first, then game version). |
| `--silent` | Silent launch; identical to the default and kept for explicitness. |
| `--gui` | Open the main window and run the normal UI launch flow (prerequisite checks, dialogs, error reporting). Exits with a message when another instance is running; on first run it continues automatically after the agreement and onboarding gates are complete. |
| `--game-args <ARGS>` | Override the game launch arguments. Passed as a command-line string and split on whitespace/quotes by the platform. In editor mode it takes precedence over the generated deeplink. |
| `--json` | Print the launch result as JSON; cannot be combined with `--gui`. |

`--silent` and `--gui` are mutually exclusive. The terminal CLI does not use the instance
“silent shortcut launch” setting; that setting only affects startup arguments.

Version resolution order:

1. Exact version folder match (case-insensitive on Windows, after path normalization).
2. Display name or game version match (case-insensitive).
3. Multiple matches are reported as an error that asks for the version folder.

Examples:

```powershell
BMCBL launch "1.21.0 stable"
BMCBL launch --gui
BMCBL launch --game-args "-server 127.0.0.1" --json
```

A silent launch command returns after the launch task reaches a terminal state (the game PID is
available, or the task failed). The game process keeps running after the command exits. Scripts
that must wait for game exit should track the reported PID. `--gui` keeps the window event loop
alive until the main window closes.

### 1.2 import

Imports a resource pack, world, add-on or package and opens the import window.

Supported: `.mcpack`, `.mcworld`, `.mcaddon`, `.mctemplate`, `.zip`, `.appx`,
`.msixvc`. Other extensions fail with a usage error.

```powershell
BMCBL import pack.mcpack
```

### 1.3 list

Lists local versions. Each line contains the version folder, display name, game version and kind.
Ordering matches the main window.

```powershell
BMCBL list
BMCBL list --json
```

JSON output is an array whose entries contain `folder`, `name`, `version`,
`kind`, `path` and `sessions`.

### 1.4 info

Prints build information, executable location, data directory and version directory.

```powershell
BMCBL info
BMCBL info --json
```

## 2. Output and exit codes

- `0`: success.
- `1`: runtime failure, such as an unknown version, unreadable configuration, or a failed
  launch task.
- `2`: command-line usage error (clap), or an updater child process failure.

Successful `launch --json` output:

```json
{"ok":true,"taskId":"...","folder":"...","name":"...","version":"...","pid":1234}
```

`pid` is `null` when no game PID is available. Failures go to stderr and never pollute
`--json` stdout.

## 3. Console and logging

Release builds use the Windows GUI subsystem. Terminal CLI modes attach to the parent console and
take over stdout/stderr. When started by double-clicking in Explorer there is no parent console,
and behavior matches a normal GUI start.

`launch`, `import` and `list` write `latest.log` and archived logs like the GUI; `info` runs
before logger initialization and writes no log. Silent launch, `list` and `info` do not emit
tracing logs to the console; `--json` output is kept clean.

## 4. Platform note

On Linux, `list` and `info` may run as root; `launch` and `import` reject the root user.

## 5. Difference from startup arguments

| Scenario | Interface |
| --- | --- |
| Manual terminal command | Terminal CLI: `BMCBL launch`, `BMCBL list`, ... |
| Desktop shortcut | Startup argument: `BMCBL.exe --launch "<version folder>"` |
| File association | Startup argument: `BMCBL.exe --import "%1"` |
| Updater child | Startup argument: `BMCBL.exe --updater <source> <destination> <timeout>` |

Startup arguments follow the instance “silent shortcut launch” setting and provide no JSON output;
the terminal CLI is silent by default. See [STARTUP_ARGUMENTS](STARTUP_ARGUMENTS.en.md).
