//! Strict, bounded, privacy-conscious crash evidence parsing.
use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};
use std::{fs, io::{Read, Seek, SeekFrom}, path::PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForensicSummary {
    /// Number of bounded, session-frozen text log sources actually inspected.
    #[serde(default)]
    pub log_sources_analyzed: u16,
    pub failed_mods: Vec<ModIssue>,
    /// Structured loader-originated evidence; no raw messages or user paths.
    #[serde(default)]
    pub loader_issues: Vec<LoaderIssue>,
    #[serde(default)]
    pub missing_dependencies: Vec<DependencyIssue>,
    pub wine_stack: Option<WineStack>,
    pub minidumps: Vec<MiniDump>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModIssue {
    pub identifier: String,
    /// A shared game log does not prove this mod failure belongs to one instance.
    pub session_private: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoaderFamily {
    BLoader, PreLoader, LeviLamina, LiteLoader, Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoaderIssueKind {
    NativeException, LoaderLoadFailure, ModLoadFailure,
    DependencyFailure, SymbolFailure, HookFailure, RuntimeError,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeExceptionPhase { Seh, Veh }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoaderIssue {
    pub loader: LoaderFamily,
    pub kind: LoaderIssueKind,
    /// A sanitized DLL or mod identifier, not a filesystem path.
    pub module: Option<String>,
    pub exception_code: Option<u32>,
    pub win32_error: Option<u32>,
    #[serde(default)]
    pub phase: Option<NativeExceptionPhase>,
    /// Only a launch-scoped file/pipe can support elevated confidence.
    pub session_private: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyIssue {
    /// Sanitized DLL/shared-library basename only, never a source file path.
    pub identifier: String,
    pub session_private: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WineStack {
    pub module: Option<String>,
    pub frames: u8,
    pub exception_code: Option<u32>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MiniDump {
    pub streams: u32,
    pub exception_code: Option<u32>,
    /// Module containing the exception address, never a full file path.
    pub fault_module: Option<String>,
}

fn safe_name(input: &str) -> Option<String> {
    let value = input.trim_matches(|c: char| c.is_whitespace()
        || matches!(c, ':' | '"' | '\'' | ',' | ';' | ')' | '(' | '[' | ']' | '+'));
    if value.is_empty() || value.len() > 80 || value.contains("..")
        || value.contains('/') || value.contains('\\')
        || !value.bytes().all(|b| b.is_ascii_alphanumeric()
            || matches!(b, b'.' | b'_' | b'-'))
    { return None; }
    if ["mod", "plugin", "module", "failed", "error", "load", "loading",
        "exception", "unknown", "name", "id", "in", "at"]
        .contains(&value.to_ascii_lowercase().as_str())
    { return None; }
    Some(value.to_owned())
}

fn find_mod(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    for trigger in [
        "failed to load mod", "failed to load plugin", "error loading mod",
        "could not load mod", "could not load plugin",
        "mod initialization failed", "plugin initialization failed", "exception in mod",
        "fail to load plugin", "fail to load mod",
    ] {
        if let Some(index) = lower.find(trigger) {
            let tail = line[index + trigger.len()..]
                .trim_start_matches(|c: char| c.is_whitespace()
                    || matches!(c, ':' | '=' | '-' | '"' | '\''));
            if let Some(name) = safe_name(tail.split_whitespace().next().unwrap_or("")) {
                return Some(name);
            }
        }
    }
    // Loader-specific diagnostics: a failure message and a named mod context.
    // Many successful BLoader lines contain metrics such as failed=0.
    // Those must NEVER be interpreted as loader failures.
    if ![" failed to ", " initialization failed", " exception in ",
        " panic:", " fatal:", " error:", " error]"]
        .iter().any(|pattern| lower.contains(pattern))
    {
        return None;
    }
    for loader in ["[bloader]", "[levilamina]"] {
        if let Some(pos) = lower.find(loader) {
            let after = line[pos + loader.len()..].trim_start();
            if let Some(after) = after.strip_prefix('[')
                && let Some((mod_name, _)) = after.split_once(']')
            {
                return safe_name(mod_name);
            }
        }
    }
    None
}

/// Recover only an allowlisted library basename from explicit load failures.
/// Do not persist paths or arbitrary log contents in the report.
fn find_dependency(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    // "Can't load LeviLamina.dll" may be Win32 1114 (DLL init failed),
    // not a missing dependency; PreLoader's following code decides.
    // "Failed to load plugin" likewise proves a mod load failure only.
    if ["failed to load plugin", "failed to load mod", "fail to load plugin"]
        .iter().any(|keyword| lower.contains(keyword))
        || !["not found", "cannot open", "failed to load dll",
            "could not load dll", "import_dll", "unimplemented function"]
            .iter().any(|keyword| lower.contains(keyword))
    {
        return None;
    }
    for token in line.split_whitespace() {
        let candidate = token.trim_matches(|ch: char| {
            ch.is_whitespace() || matches!(ch,
                ':' | ',' | ';' | '(' | ')' | '[' | ']' | '"' | '\'' | '=')
        });
        let name = candidate.rsplit(['\\', '/']).next().unwrap_or("");
        let name = name.trim_matches(|ch: char| matches!(ch, ':' | ',' | ';' | ')' | '('));
        let lower = name.to_ascii_lowercase();
        if (lower.ends_with(".dll") || lower.ends_with(".so")
            || lower.contains(".so."))
            && let Some(name) = safe_name(name)
        {
            return Some(name);
        }
    }
    None
}


fn loader_family(line: &str, source: &str) -> Option<LoaderFamily> {
    let lower = line.to_ascii_lowercase();
    if lower.contains("[preloader]") {
        Some(LoaderFamily::PreLoader)
    } else if lower.contains("[levilamina]") {
        Some(LoaderFamily::LeviLamina)
    } else if lower.contains("[bloader]") {
        Some(LoaderFamily::BLoader)
    } else if lower.contains("[liteloader]") || lower.contains("[liteloaderbds]") {
        Some(LoaderFamily::LiteLoader)
    } else if lower.contains("[modloader]") {
        Some(LoaderFamily::Other)
    } else if lower.contains("native-load result phase=")
        || lower.contains("preloader.priority.failed")
        || lower.contains("crash_report.captured phase=")
    {
        Some(LoaderFamily::BLoader)
    } else if source.contains("preloader") {
        Some(LoaderFamily::PreLoader)
    } else if source.contains("levilamina") {
        Some(LoaderFamily::LeviLamina)
    } else if source.contains("bloader")
        || source.contains("mod-owned-provider")
    {
        Some(LoaderFamily::BLoader)
    } else if lower.contains("raw seh exception, from <") {
        Some(LoaderFamily::Other)
    } else {
        None
    }
}

fn subject_from_loader_line(line: &str) -> Option<String> {
    // The module in "<...>" on LeviLamina's Raw SEH line is a faulting
    // module, not automatically the component responsible for the crash.
    if let Some(start) = line.find('<')
        && let Some(end) = line[start + 1..].find('>')
    {
        return safe_name(&line[start + 1..start + 1 + end]);
    }
    for token in line.split_whitespace() {
        let token = token.trim_matches(|ch: char| {
            ch.is_whitespace() || matches!(ch,
                ':' | ',' | ';' | '(' | ')' | '[' | ']' | '"' | '\'' | '!' | '=')
        });
        // BLoader native-load lines carry the DLL as "name=Foo.dll".
        // Keep only that value, never the original path or a key prefix.
        let token = token.strip_prefix("name=")
            .or_else(|| token.strip_prefix("module="))
            .unwrap_or(token);
        let name = token.rsplit(['\\', '/']).next().unwrap_or("");
        let name = name.trim_matches(|ch: char| {
            matches!(ch, ':' | ',' | ';' | '(' | ')' | '[' | ']' | '!' )
        });
        let lower = name.to_ascii_lowercase();
        if (lower.ends_with(".dll") || lower.ends_with(".so")
            || lower.contains(".so."))
            && let Some(name) = safe_name(name)
        {
            return Some(name);
        }
    }
    None
}

fn loader_line_issue(line: &str, source: &str, private: bool) -> Option<LoaderIssue> {
    let loader = loader_family(line, source)?;
    let lower = line.to_ascii_lowercase();
    let kind = if lower.contains("raw seh exception, from <")
        || lower.contains("unhandled seh exception")
        || lower.contains("unhandled exception 0x")
        || lower.contains("crash_report.captured phase=")
    {
        LoaderIssueKind::NativeException
    } else if source.contains("mod-owned-provider")
        && lower.contains("load failed:")
    {
        LoaderIssueKind::ModLoadFailure
    } else if source.contains("mod-owned-provider")
        && lower.contains("crashed:")
    {
        LoaderIssueKind::RuntimeError
    } else if source.contains("bloader")
        && lower.contains("load failed:")
    {
        LoaderIssueKind::ModLoadFailure
    } else if lower.contains("native-load result phase=")
        && lower.contains("state=failed")
    {
        LoaderIssueKind::LoaderLoadFailure
    } else if lower.contains("preloader.priority.failed") {
        LoaderIssueKind::LoaderLoadFailure
    } else if [
        "fail to load plugin", "failed to load plugin", "failed to load mod",
        "could not load mod", "failed to initialize mod",
        "mod initialization failed", "plugin initialization failed",
        "unrecognized mod type", "unrecognized plugin type",
    ].iter().any(|pattern| lower.contains(pattern)) {
        LoaderIssueKind::ModLoadFailure
    } else if [
        "dependency version mismatch", "unresolved dependency",
        "missing dependency", "missing required mod", "dependency cycle",
        "failed to resolve dependency", "failed to load dependency",
    ].iter().any(|pattern| lower.contains(pattern)) {
        LoaderIssueKind::DependencyFailure
    } else if [
        "failed to resolve symbol", "symbol not found", "failed to load pdb",
        "cannot find symbol", "failed to load symbols",
        "symbol lookup failed",
    ].iter().any(|pattern| lower.contains(pattern)) {
        LoaderIssueKind::SymbolFailure
    } else if [
        "failed to install hook", "failed to hook", "hook initialization failed",
        "hook registration failed", "failed to register hook",
        "hook failed", "hook install failed", "unable to install hook",
    ].iter().any(|pattern| lower.contains(pattern)) {
        LoaderIssueKind::HookFailure
    } else if [
        "can't load ", "cannot load ", "fail to load ", "failed to load ",
        "unable to load ", "failed to initialize levilamina",
        "failed to initialize bloader", "failed to inject bloader",
        "bloader injection failed",
    ].iter().any(|pattern| lower.contains(pattern)) {
        LoaderIssueKind::LoaderLoadFailure
    } else if (
        lower.contains(" error ") || lower.contains("[error]")
        || lower.contains(" error]") || lower.contains(" fatal:")
    ) && [
        "initialization failed", "startup failed", "loader failed",
        "loading failed", "unable to initialize", "fatal error",
        "failed to bootstrap",
    ].iter().any(|pattern| lower.contains(pattern)) {
        LoaderIssueKind::RuntimeError
    } else {
        return None;
    };
    Some(LoaderIssue {
        loader, kind,
        module: subject_from_loader_line(line),
        exception_code: if kind == LoaderIssueKind::NativeException {
            code_from(line)
        } else {
            None
        },
        win32_error: if kind == LoaderIssueKind::NativeException {
            None
        } else {
            loader_win32_code(line)
        },
        phase: if kind == LoaderIssueKind::NativeException {
            if lower.contains("raw seh exception") || lower.contains("phase=seh") {
                Some(NativeExceptionPhase::Seh)
            } else if lower.contains("phase=veh") {
                Some(NativeExceptionPhase::Veh)
            } else {
                None
            }
        } else {
            None
        },
        session_private: private,
    })
}

fn loader_issue_priority(issue: &LoaderIssue) -> (u8, u8) {
    let severity = match issue.kind {
        LoaderIssueKind::NativeException => 0,
        LoaderIssueKind::LoaderLoadFailure => 1,
        LoaderIssueKind::SymbolFailure | LoaderIssueKind::HookFailure => 2,
        LoaderIssueKind::DependencyFailure => 3,
        LoaderIssueKind::ModLoadFailure => 4,
        LoaderIssueKind::RuntimeError => 5,
    };
    (severity, if issue.session_private { 0 } else { 1 })
}

fn loader_win32_code(line: &str) -> Option<u32> {
    // A PreLoader "Error code: 127" is Win32, not NTSTATUS.
    // BLoader's LoadLibraryW failures instead include an HRESULT such as
    // 0x8007007E (HRESULT_FROM_WIN32(ERROR_MOD_NOT_FOUND)).
    let lower = line.to_ascii_lowercase();
    if lower.contains("loadlibrary") && lower.contains(" failed code=0x")
        && let Some(index) = lower.find("failed code=0x")
    {
        let hex = lower[index + "failed code=0x".len()..].chars()
            .take_while(|ch| ch.is_ascii_hexdigit()).collect::<String>();
        if hex.len() == 8
            && let Ok(hr) = u32::from_str_radix(&hex, 16)
            && (hr & 0xFFFF_0000) == 0x8007_0000
        {
            return Some(hr & 0xFFFF);
        }
    }
    for key in ["error code:", "error: code[", "win32 error ", "windows error "] {
        if let Some(index) = lower.find(key) {
            let text = lower[index + key.len()..].trim_start();
            let digits = text.chars().take_while(|c| c.is_ascii_digit()).collect::<String>();
            if !digits.is_empty() {
                return digits.parse::<u32>().ok();
            }
        }
    }
    None
}

fn code_from(line: &str) -> Option<u32> {
    let input = line.to_ascii_lowercase();
    for word in input.split(|c: char| !(c.is_ascii_hexdigit() || c == 'x')) {
        if let Some(suffix) = word.strip_prefix("0x")
            && suffix.len() == 8 && suffix.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return u32::from_str_radix(suffix, 16).ok();
        }
        if word.len() == 8 && word.starts_with('c')
            && word.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return u32::from_str_radix(word, 16).ok();
        }
    }
    None
}
fn stack_module(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if !(trimmed.starts_with("=>")
        || trimmed.chars().next().is_some_and(|c| c.is_ascii_digit()))
    { return None; }
    let lower = trimmed.to_ascii_lowercase();
    let offset = lower.find(" in ")?;
    safe_name(trimmed[offset + 4..].split_whitespace().next()?)
}

/// Analyze only frozen bounded session logs. Never store stack addresses,
/// arbitrary text, real paths, account IDs, or environment variables.
pub fn inspect_text(sources: &[(String, String)]) -> ForensicSummary {
    let mut out = ForensicSummary {
        log_sources_analyzed: sources.len().min(32) as u16,
        ..ForensicSummary::default()
    };
    for (name, content) in sources.iter().take(32) {
        let is_mod = ["mod", "bloader", "levilamina", "launch-task"]
            .iter().any(|tag| name.contains(tag));
        let is_runner = name.contains("runner") || name.contains("proton");
        let private = name == "logs/launch-task.log"
            || name == "logs/compatibility-runner.log"
            || name.contains("runner-provider");
        let mut pending_loader_issue: Option<(usize, usize)> = None;
        let mut exception = false;
        let mut in_stack = false;
        let mut module = None;
        let mut frames: u8 = 0;
        let mut exception_code = None;
        for unbounded in content.lines().take(8192) {
            let mut end = unbounded.len().min(4096);
            while !unbounded.is_char_boundary(end) { end -= 1; }
            let line = &unbounded[..end];
            // PreLoader emits its Win32 error on the line *after* the failing
            // DLL name. LeviLamina likewise emits NTSTATUS after Raw SEH.
            if let Some((index, remaining)) = pending_loader_issue.take()
                && let Some(issue) = out.loader_issues.get_mut(index)
            {
                if issue.kind == LoaderIssueKind::NativeException
                    && issue.exception_code.is_none()
                    && line.to_ascii_lowercase().contains("ntstatus")
                {
                    issue.exception_code = code_from(line);
                } else if issue.win32_error.is_none() {
                    issue.win32_error = loader_win32_code(line);
                }
                if remaining > 1 && issue.exception_code.is_none()
                    && issue.win32_error.is_none()
                {
                    pending_loader_issue = Some((index, remaining - 1));
                }
            }
            if let Some(issue) = loader_line_issue(line, name, private) {
                let duplicate = out.loader_issues.iter().any(|previous|
                    previous.loader == issue.loader
                    && previous.kind == issue.kind
                    && previous.module == issue.module
                    && previous.exception_code == issue.exception_code
                    && previous.win32_error == issue.win32_error
                );
                if !duplicate {
                    let index = if out.loader_issues.len() < 12 {
                        out.loader_issues.push(issue);
                        Some(out.loader_issues.len() - 1)
                    } else {
                        // Keep late native crashes rather than allowing repeated
                        // non-fatal mod errors to exhaust the evidence budget.
                        let worst = out.loader_issues.iter().enumerate()
                            .max_by_key(|(_, item)| loader_issue_priority(item))
                            .map(|(index, item)| (index, loader_issue_priority(item)));
                        if let Some((index, rank)) = worst
                            && loader_issue_priority(&issue) < rank
                        {
                            out.loader_issues[index] = issue;
                            Some(index)
                        } else {
                            None
                        }
                    };
                    if let Some(index) = index {
                        pending_loader_issue = Some((index, 4));
                    }
                }
            }
            if is_mod && out.failed_mods.len() < 8 {
                if let Some(identifier) = find_mod(line)
                    && !out.failed_mods.iter()
                        .any(|m| m.identifier.eq_ignore_ascii_case(&identifier))
                {
                    out.failed_mods.push(ModIssue { identifier, session_private: private });
                }
            }
            if out.missing_dependencies.len() < 8
                && let Some(identifier) = find_dependency(line)
                && !out.missing_dependencies.iter().any(|existing|
                    existing.identifier.eq_ignore_ascii_case(&identifier))
            {
                out.missing_dependencies.push(DependencyIssue {
                    identifier,
                    session_private: private,
                });
            }
            if !is_runner { continue; }
            let lower = line.to_ascii_lowercase();
            if lower.contains("unhandled page fault") || lower.contains("unhandled exception") {
                exception = true;
                exception_code = exception_code.or_else(|| code_from(line));
            }
            if lower.contains("backtrace:") { in_stack = true; continue; }
            if in_stack {
                if lower.starts_with("modules:") || lower.starts_with("threads:")
                    || lower.starts_with("system information:")
                { in_stack = false; continue; }
                if let Some(found) = stack_module(line) {
                    frames = frames.saturating_add(1).min(32);
                    if module.is_none() { module = Some(found); }
                }
            }
        }
        if exception && (frames != 0 || exception_code.is_some())
            && out.wine_stack.is_none()
        {
            out.wine_stack = Some(WineStack { module, frames, exception_code });
        }
    }
    out
}


/// Read only BLoader's machine-readable crash metadata, never raw registers,
/// stack, account data, absolute paths or unvalidated mod attribution.
/// BLoader crash JSON includes process_id and an offset-aware timestamp.
/// Require both to match this native Minecraft session before accepting it.
pub fn inspect_bloader_crash_json(
    sources: &[(String, String)],
    pid: u32,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    native_game: bool,
) -> Vec<LoaderIssue> {
    if !native_game { return Vec::new(); }
    let mut out = Vec::new();
    for (name, content) in sources.iter().take(32) {
        if !name.ends_with(".json") || content.len() > 1024 * 1024 { continue; }
        let Ok(report) = serde_json::from_str::<serde_json::Value>(content) else {
            continue;
        };
        if !report.get("loader_version").and_then(|v| v.as_str())
            .is_some_and(|version| !version.is_empty())
            || !report.get("attribution").is_some_and(|value| value.is_object())
            || report.get("recent_mod_events").is_none()
        {
            continue;
        }
        if report.get("process_id").and_then(|v| v.as_u64()) != Some(u64::from(pid)) {
            continue;
        }
        let Some(timestamp) = report.get("timestamp")
            .and_then(|value| value.as_str())
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc))
        else { continue; };
        if timestamp < started_at - chrono::Duration::seconds(30)
            || ended_at.is_some_and(|end| timestamp > end + chrono::Duration::seconds(120))
        {
            continue;
        }
        let phase = match report.get("phase").and_then(|v| v.as_str()) {
            Some("seh") => Some(NativeExceptionPhase::Seh),
            Some("veh") => Some(NativeExceptionPhase::Veh),
            _ => continue,
        };
        let module = report.get("exception_module").and_then(|value| value.as_str())
            .and_then(|full_path| full_path.rsplit(['\\', '/']).next())
            .and_then(safe_name);
        let exception_code = report.get("exception_code").and_then(|v| v.as_str())
            .and_then(code_from);
        if exception_code.is_none() { continue; }
        out.push(LoaderIssue {
            loader: LoaderFamily::BLoader,
            kind: LoaderIssueKind::NativeException,
            module,
            exception_code,
            win32_error: None,
            phase,
            session_private: true,
        });
        if out.len() == 4 { break; }
    }
    out
}

fn le_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let chunk: [u8; 4] = bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_le_bytes(chunk))
}

/// Parse the 32-byte MINIDUMP_HEADER and up to 128 directories. Extract only
/// the numeric exception code from ExceptionStream (type 6). No memory payloads.
fn le_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let data: [u8; 8] = bytes.get(offset..offset.checked_add(8)?)?.try_into().ok()?;
    Some(u64::from_le_bytes(data))
}

fn module_name<R: Read + Seek>(
    reader: &mut R, file_len: u64, name_rva: u32,
) -> Option<String> {
    let offset = u64::from(name_rva);
    if offset.checked_add(4)? > file_len { return None; }
    reader.seek(SeekFrom::Start(offset)).ok()?;
    let mut length = [0u8; 4];
    reader.read_exact(&mut length).ok()?;
    let bytes = u64::from(u32::from_le_bytes(length));
    if bytes == 0 || bytes > 256 || bytes % 2 != 0
        || offset.checked_add(4)?.checked_add(bytes)? > file_len
    {
        return None;
    }
    let mut utf16_bytes = [0u8; 256];
    reader.read_exact(&mut utf16_bytes[..bytes as usize]).ok()?;
    let units = utf16_bytes[..bytes as usize].chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let full_path = String::from_utf16(&units).ok()?;
    let basename = full_path.rsplit(['\\', '/']).next()?;
    safe_name(basename)
}

fn module_at_address<R: Read + Seek>(
    reader: &mut R, file_len: u64, rva: u64, length: u64, address: u64,
) -> Option<String> {
    if length < 4 || rva.checked_add(length)? > file_len { return None; }
    reader.seek(SeekFrom::Start(rva)).ok()?;
    let mut count = [0u8; 4];
    reader.read_exact(&mut count).ok()?;
    let count = u32::from_le_bytes(count);
    // Each MINIDUMP_MODULE is 108 bytes, and the list begins with u32 count.
    if count > 1024 || u64::from(count).checked_mul(108)?.checked_add(4)? > length {
        return None;
    }
    for index in 0..count {
        let offset = rva.checked_add(4 + u64::from(index) * 108)?;
        reader.seek(SeekFrom::Start(offset)).ok()?;
        let mut fields = [0u8; 24];
        reader.read_exact(&mut fields).ok()?;
        let base = le_u64(&fields, 0)?;
        let size = u64::from(le_u32(&fields, 8)?);
        if address >= base && address < base.saturating_add(size) {
            return module_name(reader, file_len, le_u32(&fields, 20)?);
        }
    }
    None
}

/// Read only the header, a bounded stream directory and selected numeric
/// exception/module metadata. No stack, memory bytes or private file paths
/// are retained by the diagnostic report.
pub fn parse_minidump<R: Read + Seek>(reader: &mut R, file_len: u64) -> Option<MiniDump> {
    if file_len < 32 { return None; }
    reader.seek(SeekFrom::Start(0)).ok()?;
    let mut header = [0u8; 32];
    reader.read_exact(&mut header).ok()?;
    if &header[..4] != b"MDMP" || le_u32(&header, 4)? & 0xffff != 0xa793 {
        return None;
    }
    let count = le_u32(&header, 8)?;
    let directory_start = u64::from(le_u32(&header, 12)?);
    if count > 128 || directory_start.checked_add(u64::from(count) * 12)? > file_len {
        return None;
    }
    let mut exception_code = None;
    let mut exception_address = None;
    let mut module_list = None;
    for n in 0..count {
        reader.seek(SeekFrom::Start(directory_start + u64::from(n) * 12)).ok()?;
        let mut directory = [0u8; 12];
        reader.read_exact(&mut directory).ok()?;
        let kind = le_u32(&directory, 0)?;
        let bytes = u64::from(le_u32(&directory, 4)?);
        let rva = u64::from(le_u32(&directory, 8)?);
        if rva.checked_add(bytes)? > file_len { continue; }
        match kind {
            6 if bytes >= 168 => {
                // MINIDUMP_EXCEPTION_STREAM:
                // ThreadId (4), alignment (4), ExceptionCode at +8,
                // ExceptionAddress at +24.
                reader.seek(SeekFrom::Start(rva + 8)).ok()?;
                let mut code = [0u8; 4];
                reader.read_exact(&mut code).ok()?;
                exception_code = Some(u32::from_le_bytes(code));
                reader.seek(SeekFrom::Start(rva + 24)).ok()?;
                let mut ip = [0u8; 8];
                reader.read_exact(&mut ip).ok()?;
                exception_address = Some(u64::from_le_bytes(ip));
            }
            4 if bytes >= 4 => module_list = Some((rva, bytes)),
            _ => {}
        }
    }
    let fault_module = match (exception_address, module_list) {
        (Some(address), Some((rva, bytes))) =>
            module_at_address(reader, file_len, rva, bytes, address),
        _ => None,
    };
    Some(MiniDump { streams: count, exception_code, fault_module })
}

pub fn inspect_dumps(paths: &[PathBuf]) -> Vec<MiniDump> {
    let mut out = Vec::new();
    for path in paths.iter().take(4) {
        let Ok(meta) = fs::symlink_metadata(path) else { continue };
        if !meta.file_type().is_file() { continue; }
        let Ok(mut reader) = fs::File::open(path) else { continue };
        if let Some(dump) = parse_minidump(&mut reader, meta.len()) {
            out.push(dump);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    const WINE: &str = include_str!("fixtures/wine_unhandled.log");
    const PROTON: &str = include_str!("fixtures/proton_dxvk.log");
    const UMU: &str = include_str!("fixtures/umu_startup.log");
    const MOD: &str = include_str!("fixtures/levilamina_mod_failure.log");

    #[test]
    fn wine_stack_and_mod_name_are_bounded_and_sanitized() {
        let result = inspect_text(&[
            ("logs/compatibility-runner.log".into(), WINE.into()),
            ("logs/mod-provider-00.log".into(), MOD.into()),
        ]);
        let wine = result.wine_stack.unwrap();
        assert_eq!(wine.module.as_deref(), Some("mod_bad.dll"));
        assert_eq!(wine.exception_code, Some(0xC0000005));
        assert_eq!(wine.frames, 2);
        assert_eq!(result.failed_mods[0].identifier, "BadAddon.dll");
        assert!(!result.failed_mods[0].session_private);
    }
    #[test]
    fn proton_and_umu_samples_are_not_guest_stack_traces() {
        assert!(inspect_text(&[("logs/runner-provider-00.log".into(), PROTON.into())])
            .wine_stack.is_none());
        assert!(inspect_text(&[("logs/runner-provider-00.log".into(), UMU.into())])
            .failed_mods.is_empty());
    }
    #[test]
    fn missing_dependency_is_sanitized_and_requires_a_failure() {
        assert_eq!(find_dependency("err:module:import_dll Library C:\\private\\BLoader.dll not found"),
            Some("BLoader.dll".to_string()));
        assert_eq!(find_dependency("libvulkan.so.1: cannot open shared object file"),
            Some("libvulkan.so.1".to_string()));
        assert_eq!(find_dependency("BLoader.dll loaded successfully"), None);
        assert_eq!(find_dependency("Can't load LeviLamina.dll"), None);
        assert_eq!(find_dependency("Failed to load plugin BadAddon.dll"), None);
        let evidence = inspect_text(&[("logs/compatibility-runner.log".into(),
            "libvulkan.so.1: cannot open shared object file".into())]);
        assert_eq!(evidence.missing_dependencies[0].identifier, "libvulkan.so.1");
        assert_eq!(evidence.log_sources_analyzed, 1);
        assert!(evidence.missing_dependencies[0].session_private);
    }


    #[test]
    fn preloader_multiline_errors_keep_win32_status_and_plugin_identity() {
        let log = "11:51:17 ERROR [PreLoader] Can't load LeviLamina.dll\n\
11:51:17 ERROR [PreLoader] Error code: 1114\n\
11:51:18 ERROR [PreLoader] Fail to load plugin [BadAddon.dll]\n\
11:51:18 ERROR [PreLoader] Error: Code[127]\n";
        let evidence = inspect_text(&[("logs/preloader.log".into(), log.into())]);
        assert_eq!(evidence.loader_issues.len(), 2);
        assert_eq!(evidence.loader_issues[0].loader, LoaderFamily::PreLoader);
        assert_eq!(evidence.loader_issues[0].kind, LoaderIssueKind::LoaderLoadFailure);
        assert_eq!(evidence.loader_issues[0].module.as_deref(), Some("LeviLamina.dll"));
        assert_eq!(evidence.loader_issues[0].win32_error, Some(1114));
        assert_eq!(evidence.loader_issues[1].kind, LoaderIssueKind::ModLoadFailure);
        assert_eq!(evidence.loader_issues[1].module.as_deref(), Some("BadAddon.dll"));
        assert_eq!(evidence.loader_issues[1].win32_error, Some(127));
    }

    #[test]
    fn levilamina_raw_seh_from_bloader_is_not_named_a_mod_load_failure() {
        let log = "2026-11-10 19:33:16.095 [info]   |Raw Seh Exception, from <BLoader.dll>:\n\
2026-11-10 19:33:16.095 [info]   |[0xC0000409:ntstatus] fail fast\n\
2026-11-10 19:33:16.096 [info]   |Parameter 0: 0x7\n";
        let evidence = inspect_text(&[("logs/mod-provider-00.log".into(), log.into())]);
        assert_eq!(evidence.loader_issues.len(), 1);
        let issue = &evidence.loader_issues[0];
        assert_eq!(issue.kind, LoaderIssueKind::NativeException);
        assert_eq!(issue.module.as_deref(), Some("BLoader.dll"));
        assert_eq!(issue.exception_code, Some(0xC0000409));
        assert!(!issue.session_private);
        assert!(evidence.failed_mods.is_empty());
    }

    #[test]
    fn bloader_native_load_report_and_hresult_are_recognized() {
        let text = "[BOOT] native-load result phase=dllmain state=failed stage=load name=TestMod.dll required=true detail=LoadLibraryW failed code=0x8007007E message=missing\n";
        let details = inspect_text(&[("logs/bloader-runtime.log".into(), text.into())]);
        assert_eq!(details.loader_issues.len(), 1);
        let issue = &details.loader_issues[0];
        assert_eq!(issue.loader, LoaderFamily::BLoader);
        assert_eq!(issue.module.as_deref(), Some("TestMod.dll"));
        assert_eq!(issue.win32_error, Some(126));
        assert_eq!(issue.kind, LoaderIssueKind::LoaderLoadFailure);
    }

    #[test]
    fn bloader_crash_json_requires_native_pid_and_timestamp() {
        let start = Utc::now();
        let text = serde_json::json!({
            "loader_version": "0.2.99",
            "timestamp": start.to_rfc3339(),
            "process_id": 44184,
            "exception_code": "0xC0000409",
            "exception_module": "C:\\\\mods\\\\BLoader.dll",
            "phase": "seh",
            "attribution": {"owner_type": "loader"},
            "recent_mod_events": "anything"
        }).to_string();
        let sources = vec![("logs/mod-provider-00.json".into(), text)];
        let matched = inspect_bloader_crash_json(&sources, 44184, start, None, true);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].phase, Some(NativeExceptionPhase::Seh));
        assert_eq!(matched[0].exception_code, Some(0xC0000409));
        assert_eq!(matched[0].module.as_deref(), Some("BLoader.dll"));
        assert!(matched[0].session_private);
        assert!(inspect_bloader_crash_json(&sources, 44185, start, None, true).is_empty());
        assert!(inspect_bloader_crash_json(&sources, 44184, start, None, false).is_empty());
        assert!(inspect_bloader_crash_json(&sources, 44184,
            start + chrono::Duration::minutes(10), None, true).is_empty());
    }

    #[test]
    fn bloader_mod_scoped_failure_log_is_not_lost() {
        let sources = vec![("logs/mod-owned-provider-00.log".into(),
            "[2026-11-10] [tid=1] [stream=stderr] Load failed: missing runtime".into())];
        let issues = inspect_text(&sources).loader_issues;
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].kind, LoaderIssueKind::ModLoadFailure);
        assert!(!issues[0].session_private);
    }

    #[test]
    fn late_native_exception_survives_mod_error_budget() {
        let mut text = String::new();
        for i in 0..30 {
            text.push_str(&format!("[ERROR] [PreLoader] fail to load plugin [bad_{i}.dll]\n"));
        }
        text.push_str("Raw Seh Exception, from <BLoader.dll>:\n[0xC0000409:ntstatus]\n");
        let issues = inspect_text(&[("logs/mod-provider-00.log".into(), text)])
            .loader_issues;
        assert_eq!(issues.len(), 12);
        assert!(issues.iter().any(|issue|
            issue.kind == LoaderIssueKind::NativeException
                && issue.exception_code == Some(0xC0000409)));
    }

    #[test]
    fn normal_loader_metrics_are_not_diagnosed_as_loader_failure() {
        let log = "[INFO] [BLoader] loaded=12 failed=0\n\
[INFO] [PreLoader] Loading symbols from pdb...\n\
[INFO] [LeviLamina] Loaded 10 mods\n\
[INFO] [LiteLoader] all plugins enabled\n";
        assert!(inspect_text(&[("logs/bloader.log".into(), log.into())])
            .loader_issues.is_empty());
    }

    #[test]
    fn loader_dependency_hook_and_symbol_failures_are_distinct() {
        let text = "[ERROR] [LeviLamina] dependency version mismatch\n\
[ERROR] [BLoader] failed to install hook for x\n\
[ERROR] [PreLoader] failed to resolve symbol x\n\
[ERROR] [LiteLoader] unrecognized mod type\n";
        let issues = inspect_text(&[("logs/launch-task.log".into(), text.into())]).loader_issues;
        assert_eq!(issues.len(), 4);
        assert_eq!(issues[0].kind, LoaderIssueKind::DependencyFailure);
        assert_eq!(issues[1].kind, LoaderIssueKind::HookFailure);
        assert_eq!(issues[2].kind, LoaderIssueKind::SymbolFailure);
        assert_eq!(issues[3].kind, LoaderIssueKind::ModLoadFailure);
        assert!(issues.iter().all(|issue| issue.session_private));
    }

    #[test]
    fn never_claim_mod_failure_from_discovery_count() {
        assert!(inspect_text(&[
            ("logs/bloader.log".into(),
                "[19:03:26 INFO] [BLoader] [Cosmetics] cape scan discovered=6 loaded=6 failed=0 items=8".into())
        ]).failed_mods.is_empty());
        assert!(safe_name("C:\\Users\\secret\\mod.dll").is_none());
    }
    #[test]
    fn minidump_reads_numeric_code_only() {
        let mut bytes = vec![0u8; 216];
        bytes[..4].copy_from_slice(b"MDMP");
        bytes[4..8].copy_from_slice(&0x0000a793u32.to_le_bytes());
        bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&32u32.to_le_bytes());
        bytes[32..36].copy_from_slice(&6u32.to_le_bytes());
        bytes[36..40].copy_from_slice(&168u32.to_le_bytes());
        bytes[40..44].copy_from_slice(&48u32.to_le_bytes());
        bytes[56..60].copy_from_slice(&0xC0000005u32.to_le_bytes());
        assert_eq!(parse_minidump(&mut Cursor::new(bytes), 216).unwrap().exception_code,
            Some(0xC0000005));
    }
    #[test]
    fn dump_fault_module_is_derived_from_exception_ip_and_sanitized() {
        let mut bytes = vec![0u8; 512];
        bytes[..4].copy_from_slice(b"MDMP");
        bytes[4..8].copy_from_slice(&0x0000a793u32.to_le_bytes());
        bytes[8..12].copy_from_slice(&2u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&32u32.to_le_bytes());
        // Exception stream: 168 bytes beginning at RVA 56.
        bytes[32..36].copy_from_slice(&6u32.to_le_bytes());
        bytes[36..40].copy_from_slice(&168u32.to_le_bytes());
        bytes[40..44].copy_from_slice(&56u32.to_le_bytes());
        bytes[64..68].copy_from_slice(&0xC0000005u32.to_le_bytes());
        bytes[80..88].copy_from_slice(&0x100010u64.to_le_bytes());
        // ModuleList stream: count 1 + one 108-byte MINIDUMP_MODULE.
        bytes[44..48].copy_from_slice(&4u32.to_le_bytes());
        bytes[48..52].copy_from_slice(&112u32.to_le_bytes());
        bytes[52..56].copy_from_slice(&224u32.to_le_bytes());
        bytes[224..228].copy_from_slice(&1u32.to_le_bytes());
        bytes[228..236].copy_from_slice(&0x100000u64.to_le_bytes());
        bytes[236..240].copy_from_slice(&0x2000u32.to_le_bytes());
        bytes[248..252].copy_from_slice(&352u32.to_le_bytes());
        let name = "C:\\private\\bloader.dll";
        let utf16 = name.encode_utf16().collect::<Vec<_>>();
        bytes[352..356].copy_from_slice(&(utf16.len() as u32 * 2).to_le_bytes());
        for (i, unit) in utf16.iter().enumerate() {
            let offset = 356 + i * 2;
            bytes[offset..offset + 2].copy_from_slice(&unit.to_le_bytes());
        }
        let summary = parse_minidump(&mut Cursor::new(bytes), 512).unwrap();
        assert_eq!(summary.fault_module.as_deref(), Some("bloader.dll"));
        assert_eq!(summary.exception_code, Some(0xC0000005));
    }

    #[test]
    fn malformed_minidump_rejected() {
        let mut bytes = vec![0u8; 32];
        bytes[..4].copy_from_slice(b"MDMP");
        bytes[4..8].copy_from_slice(&0x0000a793u32.to_le_bytes());
        bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_minidump(&mut Cursor::new(bytes), 32).is_none());
    }
}
