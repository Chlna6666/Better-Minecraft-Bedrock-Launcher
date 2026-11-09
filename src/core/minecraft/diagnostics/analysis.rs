//! Bounded evidence-based crash classification, independent of UI locale.
use super::{ExitKind, GameSessionReport, CompatibilityFault};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrashReason {
    MissingDependency, UnsupportedWineApi, WineGuestException, WineRuntime,
    VulkanUnavailable, GraphicsDeviceLost, InsufficientMemory, DiskFull,
    PermissionDenied, ModLoadFailure, LoaderFailure, AccessViolation,
    IllegalInstruction, StackOverflow, FastFail, RunnerExited, Unclassified,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence { High, Medium, Low }

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSource {
    GameLog, ModLog, RunnerLog, LauncherLog, WindowsWer, ExitStatus,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CrashFinding {
    pub reason: CrashReason,
    // Confidence of an observed *symptom*, never proof of component blame.
    pub confidence: Confidence,
    pub source: EvidenceSource,
}

fn add(out: &mut Vec<CrashFinding>, reason: CrashReason, confidence: Confidence, source: EvidenceSource) {
    if !out.iter().any(|f| f.reason == reason) {
        out.push(CrashFinding { reason, confidence, source });
    }
}
fn has(text: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|pattern| text.contains(pattern))
}
fn origin(name: &str) -> EvidenceSource {
    if name.contains("launch-task") { EvidenceSource::LauncherLog }
    else if name.contains("game.log") { EvidenceSource::GameLog }
    else if name.contains("runner") || name.contains("proton") { EvidenceSource::RunnerLog }
    else { EvidenceSource::ModLog }
}

/// Only immutable per-session, bounded snapshots may be supplied as sources.
/// Stores stable identifiers without sensitive log lines or user file paths.
pub fn analyze(report: &GameSessionReport, sources: &[(String, String)]) -> Vec<CrashFinding> {
    if matches!(report.kind, ExitKind::Normal | ExitKind::Running) {
        return Vec::new();
    }
    let mut findings = Vec::new();
    for (name, contents) in sources.iter().take(16) {
        let text = contents.to_ascii_lowercase();
        let source = origin(name);
        if has(&text, &["no space left on device", "enospc", "disk is full"]) {
            add(&mut findings, CrashReason::DiskFull, Confidence::High, source);
        }
        if has(&text, &["permission denied", "eacces", "access denied while"]) {
            add(&mut findings, CrashReason::PermissionDenied, Confidence::High, source);
        }
        if has(&text, &["vk_error_incompatible_driver", "failed to create vulkan instance",
            "vkcreateinstance failed", "no suitable physical device",
            "no vulkan devices found", "failed to create dxvk device"]) {
            add(&mut findings, CrashReason::VulkanUnavailable, Confidence::High, source);
        }
        if has(&text, &["dxgi_error_device_removed", "dxgi_error_device_hung",
            "vk_error_device_lost", "gpu device lost"]) {
            add(&mut findings, CrashReason::GraphicsDeviceLost, Confidence::High, source);
        }
        if has(&text, &["out of memory", "std::bad_alloc", "cannot allocate memory",
            "virtual memory exhausted"]) {
            add(&mut findings, CrashReason::InsufficientMemory, Confidence::Medium, source);
        }
        if has(&text, &["unimplemented function", "unimplemented api"]) {
            add(&mut findings, CrashReason::UnsupportedWineApi, Confidence::High, source);
        }
        if has(&text, &["err:module:import_dll", "dll not found", "failed to load dll",
            "library not found", "module not found", "libvulkan.so.1: cannot open",
            "ld-linux.so.2: could not open"]) {
            add(&mut findings, CrashReason::MissingDependency, Confidence::Medium, source);
        }
        if has(&text, &["unhandled page fault", "wine: unhandled exception",
            "unhandled exception in"]) {
            add(&mut findings, CrashReason::WineGuestException, Confidence::Medium, source);
        }
        if has(&text, &["pressure-vessel-wrap: error", "wineserver: failed",
            "failed to initialize wine", "proton: fatal", "wineboot failed"]) {
            add(&mut findings, CrashReason::WineRuntime, Confidence::High, source);
        }
        if has(&text, &["failed to load mod", "error loading mod", "exception in mod",
            "mod initialization failed", "failed to load plugin", "mod loader crashed"]) {
            add(&mut findings, CrashReason::ModLoadFailure, Confidence::Medium, source);
        }
        if has(&text, &["failed to inject bloader", "bloader injection failed",
            "unable to load bloader.dll", "failed to initialize levilamina"]) {
            add(&mut findings, CrashReason::LoaderFailure, Confidence::Medium, source);
        }
    }
    if let Some(wer) = &report.wer_fault {
        let module = wer.module.to_ascii_lowercase();
        if has(&module, &["nvwgf2umx", "nvlddmkm", "atidxx", "amdxx",
            "igd10", "igc64", "d3d12", "dxgi", "vulkan-1"]) {
            add(&mut findings, CrashReason::GraphicsDeviceLost, Confidence::Medium, EvidenceSource::WindowsWer);
        }
        if has(&module, &["bloader", "levilamina", "modloader", "liteloader"]) {
            add(&mut findings, CrashReason::LoaderFailure, Confidence::Medium, EvidenceSource::WindowsWer);
        }
    }
    if report.compatibility_prefix.is_some() {
        match report.compatibility_fault {
            Some(CompatibilityFault::WineUnhandledException) =>
                add(&mut findings, CrashReason::WineGuestException, Confidence::Medium, EvidenceSource::RunnerLog),
            Some(CompatibilityFault::WineDependencyFailure) =>
                add(&mut findings, CrashReason::MissingDependency, Confidence::Low, EvidenceSource::RunnerLog),
            Some(CompatibilityFault::RuntimeFailure) =>
                add(&mut findings, CrashReason::WineRuntime, Confidence::Medium, EvidenceSource::RunnerLog),
            None => {}
        }
        if report.kind == ExitKind::RunnerFailure {
            add(&mut findings, CrashReason::RunnerExited, Confidence::Low, EvidenceSource::ExitStatus);
        }
    } else if let Some(code) = report.exit_code {
        let candidate = match code {
            0xC0000135 => Some((CrashReason::MissingDependency, Confidence::High)),
            0xC0000005 => Some((CrashReason::AccessViolation, Confidence::Low)),
            0xC000001D => Some((CrashReason::IllegalInstruction, Confidence::Medium)),
            0xC00000FD => Some((CrashReason::StackOverflow, Confidence::Medium)),
            0xC0000409 => Some((CrashReason::FastFail, Confidence::Low)),
            _ => None,
        };
        if let Some((reason, confidence)) = candidate {
            add(&mut findings, reason, confidence, EvidenceSource::ExitStatus);
        }
    }
    if findings.is_empty() {
        add(&mut findings, CrashReason::Unclassified, Confidence::Low, EvidenceSource::ExitStatus);
    }
    findings.sort_by_key(|finding| {
        let confidence = match finding.confidence {
            Confidence::High => 0, Confidence::Medium => 1, Confidence::Low => 2,
        };
        let source = match finding.source {
            EvidenceSource::GameLog | EvidenceSource::ModLog | EvidenceSource::RunnerLog => 0,
            EvidenceSource::WindowsWer => 1, EvidenceSource::LauncherLog => 2,
            EvidenceSource::ExitStatus => 3,
        };
        (confidence, source)
    });
    findings.truncate(3);
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::path::PathBuf;

    fn report(platform: &str, code: Option<u32>) -> GameSessionReport {
        GameSessionReport {
            schema_version: super::super::GAME_DIAGNOSTICS_SCHEMA,
            id: "00000000-0000-4000-8000-000000000001".into(),
            task_id: "task".into(), version: "test".into(),
            game_directory: PathBuf::new(), platform: platform.into(), pid: 1,
            process_created_at: None,
            compatibility_prefix: (platform == "wine").then(|| PathBuf::from("/tmp/pfx")),
            compatibility_logs: None, compatibility_fault: None, early_exit: false,
            started_at: Utc::now(), ended_at: Some(Utc::now()), exit_code: code,
            exit_signal: None,
            kind: if platform == "wine" { ExitKind::RunnerFailure } else { ExitKind::SuspectedCrash },
            summary: String::new(), acknowledged: false, wer_fault: None,
            findings: Vec::new(),
        }
    }

    #[test]
    fn access_violation_without_logs_does_not_blame_mods() {
        let found = analyze(&report("gdk", Some(0xC0000005)), &[]);
        assert_eq!(found[0].reason, CrashReason::AccessViolation);
        assert_eq!(found[0].confidence, Confidence::Low);
    }
    #[test]
    fn wine_vulkan_failure_outranks_generic_runner_exit() {
        let logs = vec![("logs/compatibility-runner.log".into(),
            "DXVK vkCreateInstance failed VK_ERROR_INCOMPATIBLE_DRIVER".into())];
        let found = analyze(&report("wine", Some(1)), &logs);
        assert_eq!(found[0].reason, CrashReason::VulkanUnavailable);
    }
    #[test]
    fn mod_presence_is_not_a_mod_crash() {
        let logs = vec![("logs/bloader.log".into(), "BLoader discovered 12 mods".into())];
        let found = analyze(&report("gdk", Some(0xC0000005)), &logs);
        assert!(!found.iter().any(|x| x.reason == CrashReason::ModLoadFailure));
        let logs = vec![("logs/bloader.log".into(), "Failed to load mod: example".into())];
        let found = analyze(&report("gdk", Some(0xC0000005)), &logs);
        assert_eq!(found[0].reason, CrashReason::ModLoadFailure);
    }
    #[test]
    fn does_not_store_raw_log_evidence() {
        let logs = vec![("logs/game.log".into(),
            "permission denied TOKEN=private-secret".into())];
        let found = analyze(&report("gdk", Some(7)), &logs);
        assert!(!serde_json::to_string(&found).unwrap().contains("private-secret"));
    }
}
