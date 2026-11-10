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
    InitializationFailure, InPageError, ArithmeticFault, HeapCorruption,
    UnhandledCppException, LoaderInitializationFailure,
    ModDependencyFailure, LoaderSymbolFailure, LoaderHookFailure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence { High, Medium, Low }

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSource {
    GameLog, ModLog, RunnerLog, LauncherLog, LoaderLog, WindowsWer, CrashDump, ExitStatus,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CrashFinding {
    pub reason: CrashReason,
    // Confidence of an observed *symptom*, never proof of component blame.
    pub confidence: Confidence,
    pub source: EvidenceSource,
}

fn rank(confidence: Confidence) -> u8 {
    match confidence {
        Confidence::High => 0,
        Confidence::Medium => 1,
        Confidence::Low => 2,
    }
}

fn add(out: &mut Vec<CrashFinding>, reason: CrashReason, confidence: Confidence, source: EvidenceSource) {
    // A shared log can mention errors from another Minecraft instance.
    let confidence = if source == EvidenceSource::ModLog
        || (source == EvidenceSource::RunnerLog && reason == CrashReason::ModLoadFailure)
    {
        Confidence::Low
    } else {
        confidence
    };
    if let Some(existing) = out.iter_mut().find(|finding| finding.reason == reason) {
        // Evidence may arrive in stages: a low-confidence log hint must not
        // mask later definitive WER exception metadata for the same symptom.
        if rank(confidence) < rank(existing.confidence) {
            existing.confidence = confidence;
            existing.source = source;
        }
    } else {
        out.push(CrashFinding { reason, confidence, source });
    }
}

/// Interpret only the documented exception *type*, not a responsible component.
pub fn reason_from_windows_exception(code: u32) -> Option<CrashReason> {
    Some(match code {
        0xC000_0005 => CrashReason::AccessViolation,
        0xC000_0006 => CrashReason::InPageError,
        0xC000_0017 => CrashReason::InsufficientMemory,
        0xC000_001D | 0xC000_0096 => CrashReason::IllegalInstruction,
        0xC000_0094 => CrashReason::ArithmeticFault,
        0xC000_00FD => CrashReason::StackOverflow,
        0xC000_0374 => CrashReason::HeapCorruption,
        0xE06D_7363 => CrashReason::UnhandledCppException,
        0xC000_0135 => CrashReason::MissingDependency,
        0xC000_007B | 0xC000_0139 | 0xC000_0142 =>
            CrashReason::InitializationFailure,
        0xC000_0409 => CrashReason::FastFail,
        _ => return None,
    })
}

fn wer_exception_code(value: &str) -> Option<u32> {
    let value = value.trim();
    let value = value.strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    if value.len() != 8 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(value, 16).ok()
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
    // analyze_snapshot() captures up to 32 bounded per-session sources.
    // Limiting this pass to 16 silently discarded later provider errors.
    for (name, contents) in sources.iter().take(32) {
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
            // Optional Wine API probes also emit "unimplemented" in healthy
            // sessions. This is a symptom until tied to a fatal launcher error.
            add(&mut findings, CrashReason::UnsupportedWineApi, Confidence::Medium, source);
        }
        if has(&text, &["err:module:import_dll", "dll not found", "failed to load dll",
            "library not found", "module not found", "libvulkan.so.1: cannot open",
            "ld-linux.so.2: could not open"]) {
            add(&mut findings, CrashReason::MissingDependency, Confidence::Medium, source);
        }
        if has(&text, &["unhandled page fault", "wine: unhandled exception",
            "unhandled exception in"]) {
            // Wine logs do not identify whether the guest exception came from
            // Minecraft or a helper process. Avoid overconfident attribution.
            add(&mut findings, CrashReason::WineGuestException, Confidence::Low, source);
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
        // WER reports the actual exception code separately from the game's
        // exit status. It may differ, and is the stronger evidence.
        if let Some(reason) = wer_exception_code(&wer.exception_code)
            .and_then(reason_from_windows_exception)
        {
            add(&mut findings, reason, Confidence::High, EvidenceSource::WindowsWer);
        }
        // Merely naming d3d12.dll/dxgi.dll as the faulting module does NOT
        // prove a DXGI_ERROR_DEVICE_REMOVED or GPU device loss.
        let module = wer.module.to_ascii_lowercase();
        if has(&module, &["bloader", "levilamina", "modloader", "liteloader"]) {
            add(&mut findings, CrashReason::LoaderFailure, Confidence::Low, EvidenceSource::WindowsWer);
        }
    }
    if report.compatibility_prefix.is_some() {
        match report.compatibility_fault {
            Some(CompatibilityFault::WineUnhandledException) =>
                add(&mut findings, CrashReason::WineGuestException, Confidence::Low, EvidenceSource::RunnerLog),
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
        if let Some(reason) = reason_from_windows_exception(code) {
            // An exit status alone cannot prove an exception was unhandled.
            let confidence = if matches!(reason,
                CrashReason::AccessViolation | CrashReason::FastFail |
                CrashReason::InPageError | CrashReason::ArithmeticFault |
                CrashReason::HeapCorruption | CrashReason::UnhandledCppException
            ) {
                Confidence::Low
            } else {
                Confidence::Medium
            };
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
            EvidenceSource::GameLog | EvidenceSource::ModLog
            | EvidenceSource::RunnerLog | EvidenceSource::LoaderLog => 0,
            EvidenceSource::WindowsWer | EvidenceSource::CrashDump => 1, EvidenceSource::LauncherLog => 2,
            EvidenceSource::ExitStatus => 3,
        };
        (confidence, source)
    });
    findings.truncate(3);
    findings
}

/// Correlate structured evidence with the existing log classifier. A minidump
/// proves a numeric exception, not which mod caused the failure.
pub fn analyze_with_forensics(
    report: &GameSessionReport,
    sources: &[(String, String)],
    evidence: &super::forensics::ForensicSummary,
) -> Vec<CrashFinding> {
    let mut findings = analyze(report, sources);
    if matches!(report.kind, ExitKind::Normal | ExitKind::Running) {
        return Vec::new();
    }
    let mut add_or_promote = |reason: CrashReason, confidence: Confidence, source: EvidenceSource| {
        // Shared loader files can belong to another Minecraft instance.
        let confidence = if source == EvidenceSource::ModLog {
            Confidence::Low
        } else {
            confidence
        };
        if let Some(found) = findings.iter_mut().find(|f| f.reason == reason) {
            let rank = |c: Confidence| match c {
                Confidence::High => 0, Confidence::Medium => 1, Confidence::Low => 2,
            };
            if reason == CrashReason::ModLoadFailure
                && source == EvidenceSource::ModLog
                && confidence == Confidence::Low
                && found.source == EvidenceSource::ModLog
            {
                // A shared file can contain another instance's loader error.
                found.confidence = Confidence::Low;
            } else if rank(confidence) < rank(found.confidence) {
                found.confidence = confidence;
                found.source = source;
            }
        } else {
            findings.push(CrashFinding { reason, confidence, source });
        }
    };
    if !evidence.missing_dependencies.is_empty() {
        let private = evidence.missing_dependencies.iter().any(|item| item.session_private);
        add_or_promote(
            CrashReason::MissingDependency,
            if private { Confidence::Medium } else { Confidence::Low },
            if report.compatibility_prefix.is_some() {
                EvidenceSource::RunnerLog
            } else if private {
                EvidenceSource::LauncherLog
            } else {
                EvidenceSource::ModLog
            },
        );
    }

    // Keep loader symptom identity separate from the eventual game crash
    // cause. A logged SEH fault can originate from a helper or injected DLL;
    // it does not identify a responsible mod without stronger evidence.
    for issue in &evidence.loader_issues {
        use super::forensics::{LoaderIssueKind as Kind, NativeExceptionPhase};
        let source = if issue.session_private {
            EvidenceSource::LoaderLog
        } else {
            EvidenceSource::ModLog
        };
        let confidence = if issue.session_private {
            Confidence::Medium
        } else {
            Confidence::Low
        };
        match issue.kind {
            Kind::LoaderLoadFailure => {
                let reason = match issue.win32_error {
                    Some(126) => CrashReason::MissingDependency,
                    Some(127) => CrashReason::LoaderSymbolFailure,
                    Some(193 | 1114) => CrashReason::LoaderInitializationFailure,
                    _ => CrashReason::LoaderInitializationFailure,
                };
                add_or_promote(reason, confidence, source);
            }
            Kind::ModLoadFailure => {
                add_or_promote(CrashReason::ModLoadFailure, confidence, source);
            }
            Kind::DependencyFailure => {
                add_or_promote(CrashReason::ModDependencyFailure, confidence, source);
            }
            Kind::SymbolFailure => {
                add_or_promote(CrashReason::LoaderSymbolFailure, confidence, source);
            }
            Kind::HookFailure => {
                add_or_promote(CrashReason::LoaderHookFailure, confidence, source);
            }
            Kind::RuntimeError => {
                add_or_promote(CrashReason::LoaderFailure, Confidence::Low, source);
            }
            Kind::NativeException => {
                // VEH can capture first-chance exceptions subsequently handled.
                let exception_confidence = if issue.phase == Some(NativeExceptionPhase::Veh) {
                    Confidence::Low
                } else {
                    confidence
                };
                if let Some(code) = issue.exception_code
                    && let Some(reason) = reason_from_windows_exception(code)
                {
                    add_or_promote(reason, exception_confidence, source);
                }
                if issue.module.as_deref().is_some_and(|module| {
                    let module = module.to_ascii_lowercase();
                    ["bloader.dll", "preloader.dll", "levilamina.dll",
                        "liteloader.dll"].contains(&module.as_str())
                }) {
                    // At the exception site, not a causal determination.
                    add_or_promote(CrashReason::LoaderFailure, Confidence::Low, source);
                }
            }
        }
    }
    if !evidence.failed_mods.is_empty() {
        let own_log = evidence.failed_mods.iter().any(|item| item.session_private);
        let confidence = if own_log { Confidence::Medium } else { Confidence::Low };
        let source = if own_log { EvidenceSource::LauncherLog } else { EvidenceSource::ModLog };
        add_or_promote(CrashReason::ModLoadFailure, confidence, source);
    }
    if evidence.wine_stack.is_some() {
        // The Wine stack may originate from a helper, not Minecraft itself.
        add_or_promote(CrashReason::WineGuestException, Confidence::Low, EvidenceSource::RunnerLog);
    }
    for dump in &evidence.minidumps {
        // A Wine/Proton report directory belongs to a launcher session, not
        // necessarily to the Minecraft guest. Without the guest PID/executable
        // identity, auxiliary-process dumps must not establish the game's cause.
        if report.compatibility_prefix.is_some() {
            continue;
        }
        if dump.fault_module.as_deref().is_some_and(|module| {
            let name = module.to_ascii_lowercase();
            name.contains("bloader") || name.contains("levilamina")
        }) {
            add_or_promote(
                CrashReason::LoaderFailure,
                Confidence::Medium,
                EvidenceSource::CrashDump,
            );
        }
        if let Some(reason) = dump.exception_code.and_then(reason_from_windows_exception) {
            add_or_promote(reason, Confidence::High, EvidenceSource::CrashDump);
        }
    }
    if findings.len() > 1 {
        findings.retain(|f| f.reason != CrashReason::Unclassified);
    }
    findings.sort_by_key(|f| {
        let confidence = match f.confidence {
            Confidence::High => 0, Confidence::Medium => 1, Confidence::Low => 2,
        };
        let source = match f.source {
            EvidenceSource::GameLog | EvidenceSource::ModLog
            | EvidenceSource::RunnerLog | EvidenceSource::LoaderLog => 0,
            EvidenceSource::WindowsWer | EvidenceSource::CrashDump => 1,
            EvidenceSource::LauncherLog => 2, EvidenceSource::ExitStatus => 3,
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
            forensics: super::super::forensics::ForensicSummary::default(),
            log_offsets: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn wer_exception_code_outranks_weak_exit_or_mod_log() {
        let mut report = report("gdk", Some(0xC0000005));
        report.wer_fault = Some(super::super::WerFault {
            event_at: Utc::now(),
            module: "dxgi.dll".into(),
            exception_code: "c0000005".into(),
        });
        let findings = analyze(&report, &[
            ("logs/mod-provider-00.log".into(), "failed to load mod OldMod".into()),
        ]);
        let access = findings.iter().find(|finding|
            finding.reason == CrashReason::AccessViolation).unwrap();
        assert_eq!(access.confidence, Confidence::High);
        assert_eq!(access.source, EvidenceSource::WindowsWer);
        assert!(!findings.iter().any(|f| f.reason == CrashReason::GraphicsDeviceLost));
    }

    #[test]
    fn formerly_unclassified_native_exception_codes_have_specific_reasons() {
        let cases = [
            (0xC000_0006, CrashReason::InPageError),
            (0xC000_0017, CrashReason::InsufficientMemory),
            (0xC000_0094, CrashReason::ArithmeticFault),
            (0xC000_0142, CrashReason::InitializationFailure),
            (0xC000_0096, CrashReason::IllegalInstruction),
            (0xC000_0374, CrashReason::HeapCorruption),
            (0xE06D_7363, CrashReason::UnhandledCppException),
            (0xC000_007B, CrashReason::InitializationFailure),
            (0xC000_0139, CrashReason::InitializationFailure),
        ];
        for (code, reason) in cases {
            assert_eq!(analyze(&report("gdk", Some(code)), &[])[0].reason, reason);
        }
        assert_eq!(wer_exception_code("0xC0000142"), Some(0xC0000142));
        assert_eq!(wer_exception_code("xyz"), None);
    }

    #[test]
    fn matches_later_bounded_log_sources() {
        let mut files = (0..20).map(|index|
            (format!("logs/mod-provider-{index:02}.log"), "normal start".to_string())
        ).collect::<Vec<_>>();
        files.push(("logs/runner-provider-21.log".into(),
            "vkCreateInstance failed VK_ERROR_INCOMPATIBLE_DRIVER".into()));
        let findings = analyze(&report("wine", Some(1)), &files);
        assert!(findings.iter().any(|f| f.reason == CrashReason::VulkanUnavailable));
    }


    #[test]
    fn preloader_errors_result_in_specific_reasons_without_blame() {
        let report = report("gdk", Some(0xC0000142));
        let sources = vec![("logs/launch-task.log".into(),
            "[ERROR] [PreLoader] Can't load LeviLamina.dll\n\
[ERROR] [PreLoader] Error code: 1114\n".into())];
        let evidence = super::super::forensics::inspect_text(&sources);
        let findings = analyze_with_forensics(&report, &sources, &evidence);
        let finding = findings.iter().find(|finding|
            finding.reason == CrashReason::LoaderInitializationFailure).unwrap();
        assert_eq!(finding.confidence, Confidence::Medium);
        assert_eq!(finding.source, EvidenceSource::LoaderLog);
        let sources = vec![("logs/mod-provider-00.log".into(),
            "[ERROR] [PreLoader] Fail to load plugin [OldMod.dll]\n\
[ERROR] [PreLoader] Error: Code[127]".into())];
        let evidence = super::super::forensics::inspect_text(&sources);
        let findings = analyze_with_forensics(&report, &sources, &evidence);
        assert!(findings.iter().any(|f| f.reason == CrashReason::ModLoadFailure
            && f.confidence == Confidence::Low));
    }

    #[test]
    fn raw_seh_bloader_is_a_symptom_not_proof_of_mod_loading_failure() {
        let report = report("gdk", Some(0xC0000409));
        let sources = vec![("logs/mod-provider-00.log".into(),
            "Raw Seh Exception, from <BLoader.dll>:\n\
[0xC0000409:ntstatus]\n".into())];
        let evidence = super::super::forensics::inspect_text(&sources);
        let findings = analyze_with_forensics(&report, &sources, &evidence);
        assert!(findings.iter().any(|f| f.reason == CrashReason::FastFail));
        assert!(!findings.iter().any(|f| f.reason == CrashReason::ModLoadFailure));
        assert!(findings.iter().any(|f| f.reason == CrashReason::LoaderFailure
            && f.confidence == Confidence::Low));
    }

    #[test]
    fn loader_hook_symbol_and_dependency_errors_are_separate() {
        let report = report("gdk", None);
        let sources = vec![("logs/launch-task.log".into(),
            "[ERROR] [BLoader] failed to install hook\n\
[ERROR] [PreLoader] failed to resolve symbol\n\
[ERROR] [LeviLamina] dependency version mismatch".into())];
        let evidence = super::super::forensics::inspect_text(&sources);
        let findings = analyze_with_forensics(&report, &sources, &evidence);
        assert!(findings.iter().any(|f| f.reason == CrashReason::LoaderHookFailure));
        assert!(findings.iter().any(|f| f.reason == CrashReason::LoaderSymbolFailure));
        assert!(findings.iter().any(|f| f.reason == CrashReason::ModDependencyFailure));
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
    #[test]
    fn regression_wine_proton_umu_and_loader_logs() {
        const WINE: &str = include_str!("fixtures/wine_unhandled.log");
        const PROTON: &str = include_str!("fixtures/proton_dxvk.log");
        const UMU: &str = include_str!("fixtures/umu_startup.log");
        const MOD: &str = include_str!("fixtures/levilamina_mod_failure.log");
        let cases = [
            ("wine", WINE, CrashReason::WineGuestException),
            ("proton", PROTON, CrashReason::VulkanUnavailable),
            ("umu", UMU, CrashReason::WineRuntime),
        ];
        for (platform, output, expected) in cases {
            let report = report(platform, Some(1));
            let source = vec![("logs/compatibility-runner.log".to_string(), output.to_string())];
            let forensic = super::super::forensics::inspect_text(&source);
            let findings = analyze_with_forensics(&report, &source, &forensic);
            assert!(
                findings.iter().any(|finding| finding.reason == expected),
                "{platform} must identify {expected:?}: {findings:?}"
            );
        }
        let report = report("gdk", Some(0xC0000005));
        let mod_logs = vec![("logs/mod-provider-00.log".to_string(), MOD.to_string())];
        let forensic = super::super::forensics::inspect_text(&mod_logs);
        let findings = analyze_with_forensics(&report, &mod_logs, &forensic);
        assert!(findings.iter().any(|f| f.reason == CrashReason::ModLoadFailure));
        assert_eq!(forensic.failed_mods[0].identifier, "BadAddon.dll");
    }

    #[test]
    fn unowned_provider_logs_never_claim_high_confidence() {
        let logs = vec![("logs/mod-provider-00.log".into(),
            "Failed to load plugin: OldPlugin.dll".into())];
        let found = analyze(&report("gdk", Some(0xC0000005)), &logs);
        let issue = found.iter().find(|f| f.reason == CrashReason::ModLoadFailure).unwrap();
        assert_eq!(issue.confidence, Confidence::Low);
        let logs = vec![("logs/mod-provider-00.log".into(),
            "Failed to load dll: optional.dll".into())];
        let found = analyze(&report("gdk", Some(0xC0000005)), &logs);
        let issue = found.iter().find(|f| f.reason == CrashReason::MissingDependency).unwrap();
        assert_eq!(issue.confidence, Confidence::Low);
    }

    #[test]
    fn shared_mod_logs_are_only_low_confidence_for_multilaunch_sessions() {
        let report = report("gdk", Some(0xC0000005));
        let files = vec![("logs/mod-provider-00.log".to_string(),
            "Failed to load mod: OtherInstance.dll".to_string())];
        let evidence = super::super::forensics::inspect_text(&files);
        let findings = analyze_with_forensics(&report, &files, &evidence);
        let mod_finding = findings.iter()
            .find(|f| f.reason == CrashReason::ModLoadFailure).unwrap();
        assert_eq!(mod_finding.confidence, Confidence::Low);
    }

    #[test]
    fn wine_helper_dump_cannot_prove_minecraft_guest_crash() {
        let report = report("wine", Some(1));
        let evidence = super::super::forensics::ForensicSummary {
            failed_mods: vec![],
            loader_issues: vec![],
            missing_dependencies: vec![],
            log_sources_analyzed: 0,
            wine_stack: None,
            minidumps: vec![super::super::forensics::MiniDump {
                streams: 2,
                exception_code: Some(0xC0000005),
                fault_module: Some("bloader.dll".into()),
            }],
        };
        let findings = analyze_with_forensics(&report, &[], &evidence);
        assert!(!findings.iter().any(|finding|
            matches!(finding.reason, CrashReason::AccessViolation | CrashReason::LoaderFailure)));
    }

    #[test]
    fn minidump_module_only_suggests_the_loader_without_blame() {
        let report = report("gdk", Some(0xC0000005));
        let evidence = super::super::forensics::ForensicSummary {
            failed_mods: vec![],
            loader_issues: vec![],
            missing_dependencies: vec![],
            log_sources_analyzed: 0,
            wine_stack: None,
            minidumps: vec![super::super::forensics::MiniDump {
                streams: 2,
                exception_code: Some(0xC0000005),
                fault_module: Some("bloader.dll".to_string()),
            }],
        };
        let findings = analyze_with_forensics(&report, &[], &evidence);
        assert!(findings.iter().any(|finding|
            finding.reason == CrashReason::LoaderFailure
                && finding.confidence == Confidence::Medium
        ));
        assert!(!findings.iter().any(|f| f.reason == CrashReason::ModLoadFailure));
    }

    #[test]
    fn dump_metadata_improves_exception_confidence_without_attributing_a_mod() {
        let report = report("gdk", Some(0xC0000005));
        let evidence = super::super::forensics::ForensicSummary {
            failed_mods: vec![],
            loader_issues: vec![],
            missing_dependencies: vec![],
            log_sources_analyzed: 0,
            wine_stack: None,
            minidumps: vec![super::super::forensics::MiniDump {
                streams: 1,
                exception_code: Some(0xC0000005),
                fault_module: None,
            }],
        };
        let findings = analyze_with_forensics(&report, &[], &evidence);
        assert_eq!(findings[0].reason, CrashReason::AccessViolation);
        assert_eq!(findings[0].confidence, Confidence::High);
        assert_eq!(findings[0].source, EvidenceSource::CrashDump);
        assert!(!findings.iter().any(|f| f.reason == CrashReason::ModLoadFailure));
    }

}
