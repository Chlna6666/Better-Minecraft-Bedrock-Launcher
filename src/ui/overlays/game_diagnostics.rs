//! Minecraft game diagnostics UI. Launcher/Sentry crashes have a separate overlay.
use crate::core::minecraft::diagnostics::{self, CrashExportOptions, ExitKind, GameSessionReport, CrashFinding, CrashReason, Confidence, EvidenceSource};
use crate::ui::state::game_diagnostics::GameDiagnosticsState;
use crate::ui::components::modal;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::components::scroll::ScrollableElement as _;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use std::path::PathBuf;

/// Crash details must remain legible even above an image or animated theme.
fn opaque(color: Hsla) -> Hsla {
    Hsla { a: 1.0, ..color }
}

#[derive(Clone, Copy)]
enum ExportField {
    Launcher,
    Game,
    Extensions,
    Windows,
    Dumps,
    Redact,
}

impl ExportField {
    fn checked(self, options: CrashExportOptions) -> bool {
        match self {
            Self::Launcher => options.include_launcher,
            Self::Game => options.include_game,
            Self::Extensions => options.include_extensions,
            Self::Windows => options.include_windows,
            Self::Dumps => options.include_dumps,
            Self::Redact => options.redact_text,
        }
    }

    fn toggle(self, options: &mut CrashExportOptions) {
        match self {
            Self::Launcher => options.include_launcher = !options.include_launcher,
            Self::Game => options.include_game = !options.include_game,
            Self::Extensions => options.include_extensions = !options.include_extensions,
            Self::Windows => options.include_windows = !options.include_windows,
            Self::Dumps => options.include_dumps = !options.include_dumps,
            Self::Redact => options.redact_text = !options.redact_text,
        }
    }
}

fn reason_title(reason: CrashReason) -> SharedString {
    match reason {
        CrashReason::MissingDependency => t!("GameCrash.reason.MissingDependency.title"),
        CrashReason::UnsupportedWineApi => t!("GameCrash.reason.UnsupportedWineApi.title"),
        CrashReason::WineGuestException => t!("GameCrash.reason.WineGuestException.title"),
        CrashReason::WineRuntime => t!("GameCrash.reason.WineRuntime.title"),
        CrashReason::VulkanUnavailable => t!("GameCrash.reason.VulkanUnavailable.title"),
        CrashReason::GraphicsDeviceLost => t!("GameCrash.reason.GraphicsDeviceLost.title"),
        CrashReason::InsufficientMemory => t!("GameCrash.reason.InsufficientMemory.title"),
        CrashReason::DiskFull => t!("GameCrash.reason.DiskFull.title"),
        CrashReason::PermissionDenied => t!("GameCrash.reason.PermissionDenied.title"),
        CrashReason::ModLoadFailure => t!("GameCrash.reason.ModLoadFailure.title"),
        CrashReason::LoaderFailure => t!("GameCrash.reason.LoaderFailure.title"),
        CrashReason::AccessViolation => t!("GameCrash.reason.AccessViolation.title"),
        CrashReason::IllegalInstruction => t!("GameCrash.reason.IllegalInstruction.title"),
        CrashReason::StackOverflow => t!("GameCrash.reason.StackOverflow.title"),
        CrashReason::FastFail => t!("GameCrash.reason.FastFail.title"),
        CrashReason::RunnerExited => t!("GameCrash.reason.RunnerExited.title"),
        CrashReason::Unclassified => t!("GameCrash.reason.Unclassified.title"),
    }
}

fn reason_detail(reason: CrashReason) -> SharedString {
    match reason {
        CrashReason::MissingDependency => t!("GameCrash.reason.MissingDependency.detail"),
        CrashReason::UnsupportedWineApi => t!("GameCrash.reason.UnsupportedWineApi.detail"),
        CrashReason::WineGuestException => t!("GameCrash.reason.WineGuestException.detail"),
        CrashReason::WineRuntime => t!("GameCrash.reason.WineRuntime.detail"),
        CrashReason::VulkanUnavailable => t!("GameCrash.reason.VulkanUnavailable.detail"),
        CrashReason::GraphicsDeviceLost => t!("GameCrash.reason.GraphicsDeviceLost.detail"),
        CrashReason::InsufficientMemory => t!("GameCrash.reason.InsufficientMemory.detail"),
        CrashReason::DiskFull => t!("GameCrash.reason.DiskFull.detail"),
        CrashReason::PermissionDenied => t!("GameCrash.reason.PermissionDenied.detail"),
        CrashReason::ModLoadFailure => t!("GameCrash.reason.ModLoadFailure.detail"),
        CrashReason::LoaderFailure => t!("GameCrash.reason.LoaderFailure.detail"),
        CrashReason::AccessViolation => t!("GameCrash.reason.AccessViolation.detail"),
        CrashReason::IllegalInstruction => t!("GameCrash.reason.IllegalInstruction.detail"),
        CrashReason::StackOverflow => t!("GameCrash.reason.StackOverflow.detail"),
        CrashReason::FastFail => t!("GameCrash.reason.FastFail.detail"),
        CrashReason::RunnerExited => t!("GameCrash.reason.RunnerExited.detail"),
        CrashReason::Unclassified => t!("GameCrash.reason.Unclassified.detail"),
    }
}

fn reason_action(reason: CrashReason) -> SharedString {
    match reason {
        CrashReason::MissingDependency => t!("GameCrash.reason.MissingDependency.action"),
        CrashReason::UnsupportedWineApi => t!("GameCrash.reason.UnsupportedWineApi.action"),
        CrashReason::WineGuestException => t!("GameCrash.reason.WineGuestException.action"),
        CrashReason::WineRuntime => t!("GameCrash.reason.WineRuntime.action"),
        CrashReason::VulkanUnavailable => t!("GameCrash.reason.VulkanUnavailable.action"),
        CrashReason::GraphicsDeviceLost => t!("GameCrash.reason.GraphicsDeviceLost.action"),
        CrashReason::InsufficientMemory => t!("GameCrash.reason.InsufficientMemory.action"),
        CrashReason::DiskFull => t!("GameCrash.reason.DiskFull.action"),
        CrashReason::PermissionDenied => t!("GameCrash.reason.PermissionDenied.action"),
        CrashReason::ModLoadFailure => t!("GameCrash.reason.ModLoadFailure.action"),
        CrashReason::LoaderFailure => t!("GameCrash.reason.LoaderFailure.action"),
        CrashReason::AccessViolation => t!("GameCrash.reason.AccessViolation.action"),
        CrashReason::IllegalInstruction => t!("GameCrash.reason.IllegalInstruction.action"),
        CrashReason::StackOverflow => t!("GameCrash.reason.StackOverflow.action"),
        CrashReason::FastFail => t!("GameCrash.reason.FastFail.action"),
        CrashReason::RunnerExited => t!("GameCrash.reason.RunnerExited.action"),
        CrashReason::Unclassified => t!("GameCrash.reason.Unclassified.action"),
    }
}

fn confidence_label(value: Confidence) -> SharedString {
    match value {
        Confidence::High => t!("GameCrash.ui.confidence.high"),
        Confidence::Medium => t!("GameCrash.ui.confidence.medium"),
        Confidence::Low => t!("GameCrash.ui.confidence.low"),
    }
}
fn evidence_label(value: EvidenceSource) -> SharedString {
    match value {
        EvidenceSource::GameLog => t!("GameCrash.ui.source.game"),
        EvidenceSource::ModLog => t!("GameCrash.ui.source.mod"),
        EvidenceSource::RunnerLog => t!("GameCrash.ui.source.runner"),
        EvidenceSource::LauncherLog => t!("GameCrash.ui.source.launcher"),
        EvidenceSource::WindowsWer => t!("GameCrash.ui.source.wer"),
        EvidenceSource::CrashDump => t!("GameCrash.ui.source.dump"),
        EvidenceSource::ExitStatus => t!("GameCrash.ui.source.exit"),
    }
}

/// One actionable diagnosis, never a claim that a particular mod is at fault.
fn finding_card(finding: CrashFinding, colors: &ThemeColors) -> impl IntoElement {
    div()
        .min_w(px(0.))
        .rounded(px(10.))
        .border_1()
        .border_color(colors.border)
        .bg(colors.surface)
        .p(px(12.))
        .flex().flex_col().gap(px(7.))
        .child(
            div().min_w(px(0.)).flex().flex_wrap().items_center().gap(px(8.))
                .child(
                    div().min_w(px(0.)).text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .whitespace_normal()
                        .child(reason_title(finding.reason))
                )
                .child(
                    div().text_size(px(10.)).text_color(colors.text_secondary)
                        .rounded(px(6.)).border_1().border_color(colors.border)
                        .px(px(6.)).py(px(3.))
                        .child(confidence_label(finding.confidence))
                )
        )
        .child(
            div().min_w(px(0.)).text_size(px(12.)).line_height(px(18.))
                .whitespace_normal().text_color(colors.text_secondary)
                .child(reason_detail(finding.reason))
        )
        .child(
            div().min_w(px(0.)).text_size(px(11.)).whitespace_normal()
                .text_color(colors.text_secondary)
                .child(format!(
                    "{} · {}",
                    t!("GameCrash.ui.evidence"), evidence_label(finding.source),
                ))
        )
        .child(
            div().min_w(px(0.)).rounded(px(7.))
                .bg(opaque(colors.settings_field_bg)).p(px(10.))
                .flex().flex_col().gap(px(4.))
                .child(
                    div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(t!("GameCrash.ui.next"))
                )
                .child(
                    div().text_size(px(12.)).line_height(px(18.))
                        .whitespace_normal().text_color(colors.text_primary)
                        .child(reason_action(finding.reason))
                )
        )
}

fn option_row(
    id: &'static str,
    title: SharedString,
    description: SharedString,
    field: ExportField,
    selected: bool,
    width: Pixels,
    enabled: bool,
    colors: &ThemeColors,
) -> impl IntoElement {
    div()
        .id(id)
        .w(width)
        .min_w(px(0.))
        .flex()
        .items_start()
        .gap(px(9.))
        .p(px(10.))
        .rounded(px(9.))
        .border_1()
        .border_color(if selected { colors.accent } else { colors.border })
        .bg(if selected { colors.surface_hover } else { colors.settings_field_bg })
        .cursor_pointer()
        .hover(|this| this.bg(colors.surface_hover))
        .child(
            div()
                .flex_none()
                .w(px(19.))
                .h(px(19.))
                .rounded(px(5.))
                .border_1()
                .border_color(if selected { colors.accent } else { colors.border })
                .bg(if selected { colors.accent } else { colors.surface })
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.))
                .text_color(colors.btn_primary_text)
                .child(if selected { "✓" } else { "" }),
        )
        .child(
            div()
                .min_w(px(0.))
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div().text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .whitespace_normal()
                        .child(title),
                )
                .child(
                    div().text_size(px(11.))
                        .line_height(px(16.))
                        .text_color(colors.text_secondary)
                        .whitespace_normal()
                        .child(description),
                ),
        )
        .on_click(move |_, _, cx| {
            cx.update_global(|state: &mut GameDiagnosticsState, _| {
                if !enabled || state.exporting {
                    return;
                }
                field.toggle(&mut state.options);
                state.awaiting_sensitive_confirmation = false;
                state.export_status = None;
                state.export_location = None;
            });
        })
}

fn action(
    id: &'static str,
    label: SharedString,
    primary: bool,
    enabled: bool,
    colors: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .rounded(px(8.))
        .border_1()
        .border_color(if primary { colors.accent } else { colors.border })
        .bg(if primary { colors.accent } else { colors.surface })
        .px(px(13.))
        .py(px(9.))
        .min_w(px(0.))
        .cursor_pointer()
        .hover(|this| this.bg(if primary { colors.accent_hover } else { colors.surface_hover }))
        .text_size(px(12.))
        .text_color(if !enabled {
            colors.text_muted
        } else if primary {
            colors.btn_primary_text
        } else {
            colors.text_primary
        })
        .child(label)
}

/// File picker and compression both run off the GPUI main thread.
/// Export is tracked by report identity and a monotonically increasing ticket.
fn start_export(cx: &mut App, id: String, options: CrashExportOptions) {
    let ticket = cx.update_global(|state: &mut GameDiagnosticsState, _| {
        if state.exporting || !options.includes_any()
            || state.pending.as_ref().is_none_or(|r| r.id != id)
        {
            return None;
        }
        state.exporting = true;
        state.export_generation = state.export_generation.wrapping_add(1);
        state.awaiting_sensitive_confirmation = false;
        state.export_status = Some(t!("GameCrash.ui.pickPath").to_string());
        state.export_location = None;
        Some(state.export_generation)
    });
    let Some(ticket) = ticket else { return };

    cx.spawn(async move |cx| {
        let identity = id.clone();
        let filename = format!("BMCBL-Game-Crash-{id}.zip");
        let outcome = crate::tasks::runtime::run_io_blocking(move || {
            let chosen = crate::utils::file_picker::pick_save_path_with_filter(
                &t!("GameCrash.ui.zipFilter"), &["zip"], &filename,
            );
            let Some(chosen) = chosen else {
                return Ok::<Option<String>, String>(None);
            };
            let path = PathBuf::from(chosen);
            diagnostics::export_zip_with_options(&identity, &path, options)?;
            Ok(Some(path.to_string_lossy().into_owned()))
        }).await;

        let _ = cx.update_global(|state: &mut GameDiagnosticsState, _| {
            if state.export_generation != ticket {
                return;
            }
            state.exporting = false;
            let selected = state.pending.as_ref().is_some_and(|r| r.id == id);
            if !selected {
                return;
            }
            match outcome {
                Ok(Ok(Some(path))) => {
                    state.export_status = Some(t!("GameCrash.ui.exportDone").to_string());
                    state.export_location = Some(path);
                }
                Ok(Ok(None)) => {
                    state.export_status = Some(t!("GameCrash.ui.exportCancel").to_string());
                }
                Ok(Err(error)) | Err(error) => {
                    state.export_status = Some(t!("GameCrash.ui.exportFail", error = error).to_string());
                }
            }
        });
    }).detach();
}

fn request_export(cx: &mut App, id: String) {
    let selection = cx.update_global(|state: &mut GameDiagnosticsState, _| {
        if state.exporting || state.pending.as_ref().is_none_or(|r| r.id != id) {
            return None;
        }
        if !state.options.includes_any() {
            state.export_status = Some(t!("GameCrash.ui.selectEvidence").to_string());
            return None;
        }
        if state.options.needs_confirmation() {
            state.awaiting_sensitive_confirmation = true;
            state.export_status = None;
            return None;
        }
        Some(state.options)
    });
    if let Some(options) = selection {
        start_export(cx, id, options);
    }
}

/// Show precise, sanitized forensic signals without attaching raw stack traces
/// or pretending that shared logs establish a culprit or game PID.
fn forensic_lines(report: &GameSessionReport) -> Vec<String> {
    let evidence = &report.forensics;
    let mut lines = Vec::new();
    if !evidence.failed_mods.is_empty() {
        let mods = evidence.failed_mods.iter().map(|m| m.identifier.as_str())
            .collect::<Vec<_>>().join(", ");
        lines.push(t!("GameCrash.forensics.mods", mods = mods).to_string());
        if evidence.failed_mods.iter().any(|m| !m.session_private) {
            lines.push(t!("GameCrash.forensics.shared").to_string());
        } else {
            lines.push(t!("GameCrash.forensics.private").to_string());
        }
    }
    if let Some(stack) = &evidence.wine_stack {
        let module = stack.module.as_deref().map(str::to_owned)
            .unwrap_or_else(|| t!("GameCrash.forensics.unknown").to_string());
        lines.push(t!(
            "GameCrash.forensics.wine", module = module, frames = stack.frames
        ).to_string());
    }
    if !evidence.minidumps.is_empty() {
        let code = evidence.minidumps.iter().find_map(|dump| dump.exception_code)
            .map(|value| format!("0x{value:08X}"))
            .unwrap_or_else(|| t!("GameCrash.forensics.unknown").to_string());
        lines.push(t!(
            "GameCrash.forensics.dump", count = evidence.minidumps.len(), code = code
        ).to_string());
        if let Some(module) = evidence.minidumps.iter()
            .find_map(|dump| dump.fault_module.as_deref())
        {
            lines.push(t!("GameCrash.forensics.module", module = module).to_string());
        }
        lines.push(t!("GameCrash.forensics.dumpWarning").to_string());
    }
    lines
}

fn details(report: &GameSessionReport) -> String {
    let compatibility = report.compatibility_prefix.is_some();
    let pid_label = if compatibility { t!("GameCrash.ui.runnerPid") } else { t!("GameCrash.ui.nativePid") };
    let exit_label = if compatibility { t!("GameCrash.ui.runnerExit") } else { t!("GameCrash.ui.nativeExit") };
    let unknown = t!("GameCrash.ui.statusNone").to_string();
    let mut result = format!(
        "{}: {}\n{}: {}\n{}: {}\n{}: {}\n{}: {}\n{}: {}\n{}: {}\n{}",
        t!("GameCrash.ui.session"), report.id,
        t!("GameCrash.ui.version"), report.version,
        t!("GameCrash.ui.platform"), report.platform,
        pid_label, report.pid,
        t!("GameCrash.ui.start"), report.started_at.format("%Y-%m-%d %H:%M:%S UTC"),
        t!("GameCrash.ui.end"), report.ended_at.map_or_else(
            || unknown.clone(), |date| date.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        ),
        exit_label, report.exit_code.map_or_else(
            || unknown.clone(), |code| format!("0x{code:08X}"),
        ),
        t!("GameCrash.ui.analysis"),
    );
    for finding in &report.findings {
        result.push_str(&format!("\n{}", t!("GameCrash.ui.detectedReason", reason = reason_title(finding.reason))));
    }
    if compatibility {
        result.push_str(&format!("\n{}", t!("GameCrash.ui.runnerNote")));
    }
    if report.early_exit {
        result.push_str(&format!("\n{}", t!("GameCrash.ui.earlyExit")));
    }
    if let Some(signal) = report.exit_signal {
        result.push_str(&format!("\n{}: {signal}", t!("GameCrash.ui.signal")));
    }
    if let Some(fault) = report.compatibility_fault {
        result.push_str(&format!("\n{}: {fault:?}", t!("GameCrash.ui.fault")));
    }
    if let Some(wer) = &report.wer_fault {
        result.push_str(&format!("\n{}: {} ({})", t!("GameCrash.ui.wer"), wer.module, wer.exception_code));
    }
    let evidence = forensic_lines(report);
    if !evidence.is_empty() {
        result.push_str(&format!("\n\n{}", t!("GameCrash.forensics.title")));
        for line in evidence {
            result.push_str("\n");
            result.push_str(&line);
        }
    }
    result
}

/// A useful community support payload contains the readable diagnosis and
/// actionable recommendations, not just a stack of opaque exit codes.
fn shareable_diagnosis(report: &GameSessionReport, findings: &[CrashFinding]) -> String {
    let mut output = details(report);
    output.push_str(&format!("\n\n{}", t!("GameCrash.ui.analysis")));
    for finding in findings {
        output.push_str(&format!(
            "\n\n{}\n{}\n{}: {} ({})\n{}",
            reason_title(finding.reason),
            reason_detail(finding.reason),
            t!("GameCrash.ui.evidence"),
            evidence_label(finding.source),
            confidence_label(finding.confidence),
            reason_action(finding.reason),
        ));
    }
    crate::utils::log_privacy::redact_log_text(&output).into_owned()
}

#[derive(Clone, Copy)]
struct DialogGeometry {
    card_width: Pixels,
    card_height: Pixels,
    compact: bool,
    inset: f32,
    available_width: Pixels,
    option_width: Pixels,
}

fn dialog_geometry(window_width: Pixels, window_height: Pixels) -> DialogGeometry {
    let card_width = (window_width - px(24.)).max(px(0.)).min(px(760.));
    let card_height = (window_height - px(24.)).max(px(0.)).min(px(690.));
    let compact = window_width < px(560.) || window_height < px(460.);
    let inset = if compact { 12. } else { 20. };
    let available_width = (card_width - px(inset * 2. + 8.)).max(px(0.));
    let option_width = if compact {
        available_width
    } else {
        (available_width - px(10.)).max(px(0.)) * 0.5
    };
    DialogGeometry {
        card_width, card_height, compact, inset, available_width, option_width,
    }
}

pub fn render_game_diagnostics_overlay(
    colors: &ThemeColors,
    window_width: Pixels,
    window_height: Pixels,
    state: &GameDiagnosticsState,
) -> Option<AnyElement> {
    let report = state.pending.clone()?;
    // Constrain all layout dimensions to the current viewport, including tiny
    // floating windows and mid-animation resizes.
    let DialogGeometry {
        card_width, card_height, compact, inset, available_width, option_width,
    } = dialog_geometry(window_width, window_height);
    let selection = state.options;
    let exporting = state.exporting;
    let export_status = state.export_status.clone();
    let export_location = state.export_location.clone();
    let history = state.history.clone();
    let confirmation = state.awaiting_sensitive_confirmation;

    let id_for_export = report.id.clone();
    let id_for_confirm = report.id.clone();
    let id_for_acknowledge = report.id.clone();
    let title = match report.kind {
        ExitKind::RunnerFailure => t!("GameCrash.ui.title.runner"),
        ExitKind::Unknown => t!("GameCrash.ui.title.unknown"),
        ExitKind::ConfirmedCrash => t!("GameCrash.ui.title.confirmed"),
        _ => t!("GameCrash.ui.title.unexpected"),
    };
    let show_technical = state.show_technical;
    let findings = if report.findings.is_empty() {
        diagnostics::analysis::analyze(&report, &[])
    } else {
        report.findings.clone()
    };
    let copy_text = shareable_diagnosis(&report, &findings);
    let forensic_display = forensic_lines(&report);

    let export_warning = if !selection.redact_text && selection.include_dumps {
        t!("GameCrash.ui.riskBoth")
    } else if !selection.redact_text {
        t!("GameCrash.ui.riskRaw")
    } else {
        t!("GameCrash.ui.riskDump")
    };

    let body = modal::modal_surface(
        opaque(colors.settings_panel_bg),
        colors.border,
        card_width,
        card_height,
        px(14.),
    )
        .min_w(px(0.))
        .min_h(px(0.))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .flex_none()
                .px(px(inset))
                .py(px(if compact { 12. } else { 16. }))
                .border_b_1()
                .border_color(colors.border)
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div().min_w(px(0.))
                        .text_size(px(if compact { 17. } else { 20. }))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .whitespace_normal()
                        .child(title),
                )
                .child(
                    div().min_w(px(0.))
                        .text_size(px(11.))
                        .text_color(colors.text_secondary)
                        .whitespace_normal()
                        .child(format!(
                            "{} · {} · PID {}",
                            report.version, report.platform, report.pid,
                        )),
                ),
        )
        // The *whole* body including confirmation and actions is scrollable,
        // so even a very short window cannot clip the export confirmation.
        .child(
            div()
                .id("game-diagnostics-scroll")
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scrollbar()
                .child(
                    div()
                        .w_full()
                        .min_w(px(0.))
                        .p(px(inset))
                        .flex()
                        .flex_col()
                        .gap(px(14.))
                        .child(
                            div()
                                .min_w(px(0.))
                                .rounded(px(10.))
                                .bg(colors.surface)
                                .p(px(12.))
                                .flex()
                                .flex_col()
                                .gap(px(5.))
                                .child(
                                    div().min_w(px(0.))
                                        .text_size(px(13.))
                                        .line_height(px(19.))
                                        .whitespace_normal()
                                        .text_color(colors.text_primary)
                                        .child(t!("GameCrash.ui.analysisNote")),
                                )
                                .child(
                                    div().text_size(px(11.))
                                        .text_color(colors.text_secondary)
                                        .whitespace_normal()
                                        .child(t!("GameCrash.ui.intro")),
                                ),
                        )
                        .child(
                            div()
                                .min_w(px(0.)).flex().flex_col().gap(px(9.))
                                .child(
                                    div().text_size(px(15.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(colors.text_primary)
                                        .child(t!("GameCrash.ui.analysis"))
                                )
                                .children(findings.into_iter().map(|finding| {
                                    finding_card(finding, colors)
                                }))
                        )
                        .when(!forensic_display.is_empty(), |panel| panel.child(
                            div()
                                .min_w(px(0.))
                                .rounded(px(10.))
                                .border_1()
                                .border_color(colors.border)
                                .bg(colors.surface)
                                .p(px(12.))
                                .flex()
                                .flex_col()
                                .gap(px(7.))
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.text_primary)
                                        .child(t!("GameCrash.forensics.title"))
                                )
                                .children(forensic_display.into_iter().map(|line| {
                                    div()
                                        .min_w(px(0.))
                                        .text_size(px(12.))
                                        .line_height(px(18.))
                                        .whitespace_normal()
                                        .text_color(colors.text_secondary)
                                        .child(line)
                                }))
                        ))
                        .child(
                            action(
                                "game-diagnostics-technical-toggle",
                                if show_technical {
                                    t!("GameCrash.ui.technicalHide")
                                } else {
                                    t!("GameCrash.ui.technicalShow")
                                },
                                false, true, colors,
                            ).on_click(|_, _, cx| {
                                cx.update_global(|state: &mut GameDiagnosticsState, _| {
                                    state.show_technical = !state.show_technical;
                                });
                            })
                        )
                        .when(!history.is_empty(), |panel| panel.child(
                            div()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .gap(px(7.))
                                .child(
                                    div().text_size(px(12.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.text_primary)
                                        .child(t!("GameCrash.ui.history")),
                                )
                                .child(
                                    div().flex().flex_wrap().gap(px(6.))
                                        .children(history.into_iter().take(12).enumerate().map(
                                            |(index, previous)| {
                                                let text = format!(
                                                    "{} · {}",
                                                    previous.version,
                                                    previous.started_at.format("%m-%d %H:%M"),
                                                );
                                                div()
                                                    .id(("game-diagnostics-history", index))
                                                    .rounded(px(6.))
                                                    .border_1()
                                                    .border_color(colors.border)
                                                    .bg(colors.surface)
                                                    .px(px(9.))
                                                    .py(px(7.))
                                                    .cursor_pointer()
                                                    .hover(|this| this.bg(colors.surface_hover))
                                                    .text_size(px(11.))
                                                    .text_color(colors.text_secondary)
                                                    .child(text)
                                                    .on_click(move |_, _, cx| {
                                                        cx.update_global(
                                                            |state: &mut GameDiagnosticsState, _| {
                                                                state.select_report(previous.clone());
                                                            },
                                                        );
                                                    })
                                            },
                                        )),
                                ),
                        ))
                        .when(show_technical, |panel| panel.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(7.))
                                .child(
                                    div().text_size(px(12.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.text_primary)
                                        .child(t!("GameCrash.ui.technical")),
                                )
                                .child(
                                    div().min_w(px(0.))
                                        .rounded(px(8.))
                                        .border_1()
                                        .border_color(colors.border)
                                        .bg(opaque(colors.settings_field_bg))
                                        .p(px(12.))
                                        .overflow_hidden()
                                        .text_size(px(11.))
                                        .line_height(px(18.))
                                        .whitespace_normal()
                                        .font_family("Consolas")
                                        .text_color(colors.text_primary)
                                        .child(details(&report)),
                                ),
                        ))
                        .when(show_technical, |panel| panel.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(8.))
                                .child(
                                    div().text_size(px(13.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.text_primary)
                                        .child(t!("GameCrash.ui.exportTitle")),
                                )
                                .child(
                                    div().min_w(px(0.))
                                        .text_size(px(11.))
                                        .text_color(colors.text_secondary)
                                        .whitespace_normal()
                                        .child(t!("GameCrash.ui.exportDesc")),
                                )
                                .child(
                                    div().min_w(px(0.))
                                        .flex().flex_wrap().gap(px(10.))
                                        .child(option_row(
                                            "game-export-launcher", t!("GameCrash.ui.launcherTitle"),
                                            t!("GameCrash.ui.launcherDesc"),
                                            ExportField::Launcher, selection.include_launcher,
                                            option_width, !exporting, colors,
                                        ))
                                        .child(option_row(
                                            "game-export-game", t!("GameCrash.ui.gameTitle"),
                                            t!("GameCrash.ui.gameDesc"),
                                            ExportField::Game, selection.include_game,
                                            option_width, !exporting, colors,
                                        ))
                                        .child(option_row(
                                            "game-export-extensions", t!("GameCrash.ui.modsTitle"),
                                            t!("GameCrash.ui.modsDesc"),
                                            ExportField::Extensions, selection.include_extensions,
                                            option_width, !exporting, colors,
                                        ))
                                        .when(cfg!(target_os = "windows"), |panel| panel.child(option_row(
                                            "game-export-windows", t!("GameCrash.ui.werTitle"),
                                            t!("GameCrash.ui.werDesc"),
                                            ExportField::Windows, selection.include_windows,
                                            option_width, !exporting, colors,
                                        )))
                                        .child(option_row(
                                            "game-export-dumps", t!("GameCrash.ui.dumpTitle"),
                                            t!("GameCrash.ui.dumpDesc"),
                                            ExportField::Dumps, selection.include_dumps,
                                            option_width, !exporting, colors,
                                        )),
                                ),
                        ))
                        .when(show_technical, |panel| panel.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(7.))
                                .child(
                                    div().text_size(px(13.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.text_primary)
                                        .child(t!("GameCrash.ui.privacy")),
                                )
                                .child(option_row(
                                    "game-export-redact",
                                    t!("GameCrash.ui.redactTitle"),
                                    t!("GameCrash.ui.redactDesc"),
                                    ExportField::Redact,
                                    selection.redact_text,
                                    available_width,
                                    !exporting,
                                    colors,
                                ))
                                .when(!selection.redact_text, |panel| panel.child(
                                    div().text_size(px(11.))
                                        .text_color(colors.danger)
                                        .whitespace_normal()
                                        .child(t!("GameCrash.ui.redactWarning")),
                                )),
                        ))
                        .when(confirmation, |panel| panel.child(
                            div()
                                .rounded(px(10.))
                                .border_1()
                                .border_color(colors.danger)
                                .p(px(12.))
                                .flex()
                                .flex_col()
                                .gap(px(9.))
                                .child(
                                    div().text_size(px(13.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.danger)
                                        .child(t!("GameCrash.ui.riskTitle")),
                                )
                                .child(
                                    div().text_size(px(12.))
                                        .line_height(px(18.))
                                        .whitespace_normal()
                                        .text_color(colors.text_primary)
                                        .child(export_warning),
                                )
                                .child(
                                    div().flex().flex_wrap().gap(px(8.))
                                        .child(
                                            action("game-export-confirm", t!("GameCrash.ui.confirmExport"), true, !exporting, colors)
                                                .on_click(move |_, _, cx| {
                                                    let options = cx.global::<GameDiagnosticsState>().options;
                                                    if options.needs_confirmation() {
                                                        start_export(cx, id_for_confirm.clone(), options);
                                                    }
                                                }),
                                        )
                                        .child(
                                            action("game-export-back", t!("GameCrash.ui.back"), false, true, colors)
                                                .on_click(|_, _, cx| {
                                                    cx.update_global(
                                                        |state: &mut GameDiagnosticsState, _| {
                                                            state.awaiting_sensitive_confirmation = false;
                                                        },
                                                    );
                                                }),
                                        ),
                                ),
                        ))
                        .when_some(export_status, |panel, status| panel.child(
                            div()
                                .min_w(px(0.))
                                .rounded(px(8.))
                                .bg(colors.surface)
                                .p(px(10.))
                                .text_size(px(12.))
                                .whitespace_normal()
                                .text_color(colors.text_primary)
                                .child(status),
                        ))
                        .when_some(export_location, |panel, path| panel.child(
                            div()
                                .min_w(px(0.))
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(8.))
                                .child(
                                    div().min_w(px(0.)).flex_1()
                                        .text_size(px(11.))
                                        .truncate()
                                        .text_color(colors.text_secondary)
                                        .child(path.clone()),
                                )
                                .child(
                                    action("game-export-copy-path", t!("GameCrash.ui.copyPath"), false, true, colors)
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(path.clone()));
                                        }),
                                ),
                        ))
                        .child(
                            div()
                                .min_w(px(0.))
                                .flex()
                                .flex_wrap()
                                .justify_end()
                                .items_center()
                                .gap(px(8.))
                                .child(
                                    action("game-diagnostics-copy", t!("GameCrash.ui.copy"), false, true, colors)
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(
                                                ClipboardItem::new_string(copy_text.clone()),
                                            );
                                        }),
                                )
                                .child(
                                    action("game-diagnostics-export", t!("GameCrash.ui.exportButton"),
                                        true, !exporting && selection.includes_any(), colors,
                                    ).on_click(move |_, _, cx| {
                                        request_export(cx, id_for_export.clone());
                                    }),
                                )
                                .child(
                                    action("game-diagnostics-dismiss", t!("GameCrash.ui.close"), false, true, colors)
                                        .on_click(move |_, _, cx| {
                                            let id = id_for_acknowledge.clone();
                                            let dismissed = cx.update_global(
                                                |state: &mut GameDiagnosticsState, _| {
                                                    state.dismiss_pending_if_matches(&id)
                                                },
                                            );
                                            if dismissed {
                                                let _ = crate::tasks::runtime::spawn_io(async move {
                                                    let _ = crate::tasks::runtime::run_io_blocking(
                                                        move || diagnostics::acknowledge(&id),
                                                    ).await;
                                                });
                                            }
                                        }),
                                ),
                        ),
                ),
        );

    // Use the shared modal backdrop and surface instead of a custom transparent overlay.
    let backdrop = hsla(0., 0., 0., 0.60);
    if crate::core::ui_prefs::reduced_motion() {
        Some(modal::animated_modal_layer(body, backdrop, 1.0, true).into_any_element())
    } else {
        Some(modal::modal_layer(body, backdrop))
    }
}


#[cfg(test)]
mod tests {
    use super::dialog_geometry;
    use gpui::px;

    #[test]
    fn game_diagnostics_layout_never_exceeds_viewport() {
        for (width, height) in [
            (0.0, 0.0),
            (180.0, 160.0),
            (320.0, 240.0),
            (450.0, 350.0),
            (760.0, 500.0),
            (972.0, 600.0),
            (2560.0, 1440.0),
        ] {
            let metrics = dialog_geometry(px(width), px(height));
            assert!(metrics.card_width >= px(0.));
            assert!(metrics.card_height >= px(0.));
            assert!(metrics.card_width <= px(width));
            assert!(metrics.card_height <= px(height));
            assert!(metrics.option_width >= px(0.));
            assert!(metrics.option_width <= metrics.available_width);
            assert!(metrics.available_width <= metrics.card_width);
        }
    }
}
