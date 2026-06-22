# BMCBL Project Spec (GPUI Single-EXE)

## Non-Goals (For Now)

- A 1:1 feature port of the upstream Tauri UI in one pass.
- Depending on WebView/Web frontends for core screens.

## Deliverable

- Windows: one `BMCBL.exe` that starts and can run without shipping extra folders next to it.

## Layout Rules

- `src/ui/views/`: app-level GPUI views (routes/screens).
- `src/ui/components/`: reusable UI components (pure GPUI rendering).
- `src/ui/theme/`: theme tokens/helpers.
- `src/i18n/`: localization implementation owned by this app (runtime language switching).
- `src/assets/`: embedded assets helpers (fonts, dll extraction, etc).
- `assets/`: build-time input assets (fonts/icons/locales/manifest/native payloads).
- `assets/locales/`: JSON translations (embedded into the binary at compile time).
- `assets/fonts/`: fonts embedded into the binary at compile time.
- `assets/bin/`: native payloads embedded into the binary (e.g. `BLoader.dll`).
- `src/core/`, `src/config/`, `src/utils/`, ...: backend modules (Tauri-free by default).

## Asset Embedding Policy

- If the app needs a file at runtime, embed it using `include_bytes!` / `include_str!`.
- If Windows expects a resource (manifest/icon), embed via `build.rs` + `embed-resource`.
- If a payload must exist on disk (e.g. a DLL loaded by the OS), extract it at runtime to:
  - `%LOCALAPPDATA%\\BMCBL\\runtime\\...` (preferred)
  - fallback: `%TEMP%\\BMCBL\\runtime\\...`

## Localization (Runtime Switching)

- Do not use `external component crates` localization for runtime switching.
- Use `I18n` as a GPUI `Global` (`src/i18n/mod.rs`).
- Switching language must refresh UI in-process (no restart).

## Backend Migration

- Keep backend logic Tauri-free; compile legacy Tauri `command` wrappers only behind `--features tauri-api`.
- Replace `tauri::command`-style boundaries with explicit Rust APIs and a GPUI-native event model.
- Keep platform-specific logic behind `cfg(windows)` where needed.
