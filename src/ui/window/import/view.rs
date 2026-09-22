use crate::core::minecraft::assets::{
    CheckImportRequest, ImportAssetsRequest, check_import_conflict, inspect_import_file,
    start_import_assets_task,
};
use crate::core::minecraft::import::{
    ImportCheckResult, PackagePreview, PreviewIconData, PreviewImageFormat, WorldPackReference,
};
use crate::core::minecraft::paths::{BuildType, Edition, GamePathOptions, get_game_root};
use crate::core::version::launch_versions::version_folder_matches;
use crate::launch::ImportLaunchContext;
use crate::ui::components::dropdown::{self, Dropdown, DropdownOption};
use crate::ui::components::minecraft_text::MinecraftFormattedText;
use crate::ui::components::scroll::ScrollableElement;
use crate::ui::components::toast;
use crate::ui::hooks::use_launcher::{
    LaunchVersionDescriptor, read_launcher_snapshot, start_launcher, sync_launcher_state,
};
use crate::ui::hooks::use_local_versions::{
    ensure_local_versions_loaded, read_local_versions_snapshot,
};
use crate::ui::state::i18n::I18n;
#[cfg(target_os = "windows")]
use crate::ui::state::launch_prereq::LaunchPrereqState;
use crate::ui::state::launcher::LauncherState;
use crate::ui::state::local_versions::LocalVersionsState;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};
use crate::ui::window::import::ImportWindowTarget;
use futures_util::future::join_all;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui::{InteractiveElement, ParentElement, Styled};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

const IMPORT_WINDOW_NARROW_WIDTH_PX: f32 = 520.0;
const IMPORT_WINDOW_COMPACT_WIDTH_PX: f32 = 420.0;
const IMPORT_WINDOW_TWO_COLUMN_WIDTH_PX: f32 = 900.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StatusKind {
    Error,
    Success,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImportPresentation {
    Window,
    Overlay,
}

#[derive(Clone)]
struct ImportFilePreview {
    path: PathBuf,
    preview: Option<PackagePreview>,
    error: Option<SharedString>,
}

fn extend_unique_import_paths(
    paths: &mut Vec<PathBuf>,
    incoming: impl IntoIterator<Item = PathBuf>,
) -> bool {
    let mut changed = false;
    for path in incoming {
        if !paths.iter().any(|existing| existing == &path) {
            paths.push(path);
            changed = true;
        }
    }
    changed
}

pub struct ImportWindowView {
    window_id: Option<u64>,
    presentation: ImportPresentation,
    import_context: ImportLaunchContext,
    file_paths: Vec<PathBuf>,
    batch_previews: Vec<ImportFilePreview>,
    target: ImportWindowTarget,
    preview: Option<PackagePreview>,
    selected_folder: Option<SharedString>,
    status: Option<(StatusKind, SharedString)>,
    conflict: Option<ImportCheckResult>,
    conflict_count: usize,
    conflict_requires_overwrite: bool,
    conflict_requires_shared_fallback: bool,
    show_conflict_dialog: bool,
    is_inspecting: bool,
    inspect_started_at: Instant,
    inspect_generation: u64,
    is_importing: bool,
    launch_after_import: bool,
    close_after_launch_completion: bool,
    launch_completion_close_scheduled: bool,
    titlebar_gesture: crate::ui::window::chrome::TitlebarGesture,
    _subscriptions: Vec<Subscription>,
}

impl ImportWindowView {
    pub fn new(
        import_context: ImportLaunchContext,
        target: ImportWindowTarget,
        presentation: ImportPresentation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let file_paths = vec![import_context.file_path.clone()];
        Self::new_with_paths(import_context, file_paths, target, presentation, window, cx)
    }

    pub fn new_batch(
        file_paths: Vec<PathBuf>,
        target: ImportWindowTarget,
        presentation: ImportPresentation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut unique_paths = Vec::with_capacity(file_paths.len());
        extend_unique_import_paths(&mut unique_paths, file_paths);
        let file_paths = unique_paths;
        let primary = file_paths
            .first()
            .cloned()
            .unwrap_or_else(|| PathBuf::from("unknown-import"));
        Self::new_with_paths(
            ImportLaunchContext { file_path: primary },
            file_paths,
            target,
            presentation,
            window,
            cx,
        )
    }

    fn new_with_paths(
        import_context: ImportLaunchContext,
        file_paths: Vec<PathBuf>,
        target: ImportWindowTarget,
        presentation: ImportPresentation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        ensure_local_versions_loaded(false, cx);
        let mut subscriptions = vec![
            cx.observe_global::<LocalVersionsState>(|this, cx| {
                this.sync_selected_folder(cx);
                cx.notify();
            }),
            cx.observe_global::<ThemeState>(|_, cx| {
                cx.notify();
            }),
            cx.observe_global::<LauncherState>(|_, cx| {
                cx.notify();
            }),
            cx.observe_global::<toast::ToastState>(|_, cx| {
                cx.notify();
            }),
            cx.observe_global::<dropdown::DropdownOverlayState>(|_, cx| {
                cx.notify();
            }),
        ];
        if presentation == ImportPresentation::Window {
            subscriptions.push(cx.observe_global_in::<I18n>(window, |_this, window, cx| {
                window.set_title(t!("Import.title").as_ref());
                cx.notify();
            }));
        } else {
            subscriptions.push(cx.observe_global::<I18n>(|_, cx| {
                cx.notify();
            }));
        }
        #[cfg(target_os = "windows")]
        subscriptions.push(cx.observe_global::<LaunchPrereqState>(|_, cx| {
            cx.notify();
        }));

        let mut this = Self {
            window_id: None,
            presentation,
            import_context,
            file_paths,
            batch_previews: Vec::new(),
            selected_folder: target.version_folder.clone(),
            target,
            preview: None,
            status: None,
            conflict: None,
            conflict_count: 0,
            conflict_requires_overwrite: false,
            conflict_requires_shared_fallback: false,
            show_conflict_dialog: false,
            is_inspecting: false,
            inspect_started_at: Instant::now(),
            inspect_generation: 0,
            is_importing: false,
            launch_after_import: false,
            close_after_launch_completion: false,
            launch_completion_close_scheduled: false,
            titlebar_gesture: crate::ui::window::chrome::TitlebarGesture::default(),
            _subscriptions: subscriptions,
        };
        this.sync_selected_folder(cx);
        this.inspect_file(cx);
        this
    }

    fn sync_selected_folder(&mut self, cx: &mut Context<Self>) {
        let snapshot = read_local_versions_snapshot(cx);
        if snapshot.versions.is_empty() {
            return;
        }

        if self.target.lock_version {
            if let Some(target) = self.target.version_folder.as_ref()
                && let Some(version) = snapshot.versions.iter().find(|version| {
                    version_folder_matches(version.folder.as_ref(), target.as_ref())
                })
            {
                self.selected_folder = Some(SharedString::from(version.folder.clone()));
            }
            return;
        }

        let exists = self.selected_folder.as_ref().is_some_and(|selected| {
            snapshot
                .versions
                .iter()
                .any(|version| version_folder_matches(version.folder.as_ref(), selected.as_ref()))
        });
        if !exists {
            self.selected_folder =
                Some(SharedString::from(snapshot.versions[0].folder.to_string()));
        }
    }

    fn inspect_file(&mut self, cx: &mut Context<Self>) {
        self.inspect_generation = self.inspect_generation.wrapping_add(1).max(1);
        let generation = self.inspect_generation;
        self.is_inspecting = true;
        self.inspect_started_at = Instant::now();
        self.preview = None;
        self.batch_previews.clear();
        self.status = None;

        let file_paths = self.file_paths.clone();
        let locale = cx.global::<I18n>().locale().code().to_string();
        debug!(
            "Import window inspect start: files={}, locale={}",
            file_paths.len(),
            locale
        );
        cx.spawn(async move |handle, cx| {
            let inspections = file_paths.into_iter().map(|path| {
                let locale = locale.clone();
                async move {
                    let path_text = path.display().to_string();
                    let result = inspect_import_file(path_text.clone(), Some(locale.clone())).await;
                    (path, path_text, result)
                }
            });
            let results = join_all(inspections).await;

            handle.update(cx, |this, cx| {
                if this.inspect_generation != generation {
                    debug!(
                        "Import window inspect stale result ignored: generation={}, current={}",
                        generation, this.inspect_generation
                    );
                    return;
                }
                this.is_inspecting = false;
                let total = results.len();
                let mut failed = 0usize;
                let mut first_preview = None;
                let mut entries = Vec::with_capacity(total);

                for (path, path_text, result) in results {
                    match result {
                        Ok(preview) => {
                            debug!(
                                "Import window inspect success: path={}, name={}, kind={}, valid={}",
                                path_text, preview.name, preview.kind, preview.valid
                            );
                            if first_preview.is_none() {
                                first_preview = Some(preview.clone());
                            }
                            entries.push(ImportFilePreview {
                                path,
                                preview: Some(preview),
                                error: None,
                            });
                        }
                        Err(error) => {
                            failed = failed.saturating_add(1);
                            warn!(
                                "Import window inspect failed: path={}, locale={}, error={}",
                                path_text, locale, error
                            );
                            entries.push(ImportFilePreview {
                                path,
                                preview: None,
                                error: Some(SharedString::from(error)),
                            });
                        }
                    }
                }

                this.preview = first_preview;
                this.batch_previews = entries;
                if failed > 0 {
                    let message = if total == 1 {
                        this.batch_previews
                            .first()
                            .and_then(|entry| entry.error.clone())
                            .unwrap_or_else(|| t!("Import.preview_unavailable"))
                    } else {
                        t!(
                            "Import.batch_inspect_failed",
                            failed = &failed.to_string(),
                            total = &total.to_string()
                        )
                    };
                    toast::error(cx, message.clone());
                    this.status = Some((StatusKind::Error, message));
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(crate) fn append_paths(
        &mut self,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if self.is_importing || paths.is_empty() {
            return;
        }

        if !extend_unique_import_paths(&mut self.file_paths, paths) {
            return;
        }

        if let Some(primary) = self.file_paths.first().cloned() {
            self.import_context.file_path = primary;
        }
        debug!(
            "Import window merged additional drop paths: files={}",
            self.file_paths.len()
        );
        self.inspect_file(cx);
        cx.notify();
    }

    fn has_invalid_or_failed_preview(&self) -> bool {
        self.is_inspecting
            || self.batch_previews.len() != self.file_paths.len()
            || self.batch_previews.iter().any(|entry| {
                entry.error.is_some()
                    || entry
                        .preview
                        .as_ref()
                        .is_none_or(|preview| !preview.valid)
            })
    }

    fn import_now(
        &mut self,
        overwrite: bool,
        allow_shared_fallback: bool,
        launch_after_import: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(selected_folder) = self.selected_folder.clone() else {
            self.status = Some((StatusKind::Error, t!("Import.errors.no_selected_version")));
            cx.notify();
            return;
        };
        if self.is_importing {
            return;
        }
        if self.has_invalid_or_failed_preview() {
            self.status = Some((
                StatusKind::Error,
                t!("Import.batch_invalid_or_unreadable"),
            ));
            cx.notify();
            return;
        }

        self.launch_after_import = launch_after_import;
        self.is_importing = true;
        self.status = None;
        self.show_conflict_dialog = false;

        let versions = read_local_versions_snapshot(cx);
        let Some(selected_version) = selected_launch_version(self, &versions) else {
            self.is_importing = false;
            self.status = Some((StatusKind::Error, t!("Import.errors.no_target_version")));
            cx.notify();
            return;
        };

        let build_type = version_build_type(selected_version);
        let edition = version_edition(selected_version);
        let enable_isolation = version_enable_isolation(selected_version);
        let user_id = self.target_user_id(&selected_folder);
        let launch_version = launch_version_descriptor(selected_version);
        let file_paths = self
            .file_paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();
        let file_count = file_paths.len();
        debug!(
            "Import window action start: files={}, version={}, build={:?}, edition={:?}, isolation={}, overwrite={}, shared_fallback={}, launch_after_import={}",
            file_count,
            selected_folder,
            build_type,
            edition,
            enable_isolation,
            overwrite,
            allow_shared_fallback,
            launch_after_import
        );

        let selected_folder_text = selected_folder.to_string();
        cx.spawn(async move |handle, cx| {
            let checks = file_paths.iter().cloned().map(|file_path| {
                let request = CheckImportRequest {
                    build_type: build_type.clone(),
                    edition: edition.clone(),
                    version_name: selected_folder_text.clone(),
                    enable_isolation,
                    user_id: user_id.clone(),
                    file_path: file_path.clone(),
                    allow_shared_fallback,
                };
                async move { (file_path, check_import_conflict(request).await) }
            });
            let check_results = join_all(checks).await;
            let mut conflicts = Vec::new();
            for (file_path, result) in check_results {
                match result {
                    Ok(conflict) => {
                        debug!(
                            "Import window conflict check result: path={}, has_conflict={}, target={}",
                            file_path, conflict.has_conflict, conflict.target_name
                        );
                        if conflict.has_conflict {
                            conflicts.push(conflict);
                        }
                    }
                    Err(error) => {
                        warn!(
                            "Import window conflict check failed: path={}, error={}",
                            file_path, error
                        );
                        handle.update(cx, |this, cx| {
                            this.is_importing = false;
                            this.status = Some((StatusKind::Error, SharedString::from(error)));
                            cx.notify();
                        })?;
                        return Ok::<(), anyhow::Error>(());
                    }
                }
            }

            let requires_shared_fallback = conflicts.iter().any(|conflict| {
                conflict.conflict_type.as_deref() == Some("shared_fallback")
            });
            let requires_overwrite = conflicts.iter().any(|conflict| {
                conflict.conflict_type.as_deref() != Some("shared_fallback")
            });
            if (requires_shared_fallback && !allow_shared_fallback)
                || (requires_overwrite && !overwrite)
            {
                let conflict_count = conflicts.len();
                let first_conflict = conflicts.into_iter().next();
                handle.update(cx, |this, cx| {
                    this.is_importing = false;
                    this.conflict = first_conflict;
                    this.conflict_count = conflict_count;
                    this.conflict_requires_overwrite = requires_overwrite;
                    this.conflict_requires_shared_fallback = requires_shared_fallback;
                    this.show_conflict_dialog = true;
                    cx.notify();
                })?;
                return Ok::<(), anyhow::Error>(());
            }

            let title = if file_count > 1 {
                format!("导入 Minecraft 内容（{file_count} 个文件）")
            } else {
                "导入 Minecraft 内容".to_string()
            };
            let task_id = match start_import_assets_task(
                ImportAssetsRequest {
                    build_type,
                    edition,
                    version_name: selected_folder_text,
                    enable_isolation,
                    user_id,
                    file_paths,
                    overwrite,
                    allow_shared_fallback,
                },
                title,
            ) {
                Ok(task_id) => task_id,
                Err(error) => {
                    handle.update(cx, |this, cx| {
                        this.finish_import_task(
                            Err(error),
                            launch_after_import.then_some(launch_version),
                            cx,
                        );
                    })?;
                    return Ok::<(), anyhow::Error>(());
                }
            };

            debug!(
                "Import window registered BMCBL task: files={}, task_id={}",
                file_count, task_id
            );
            let terminal = crate::tasks::task_manager::wait_for_task_terminal(&task_id).await;

            handle.update(cx, |this, cx| {
                if let Ok(snapshot) = &terminal {
                    debug!(
                        "Import window task terminal: task_id={}, status={}, message={:?}, launch_after_import={}",
                        snapshot.id,
                        snapshot.status,
                        snapshot.message,
                        launch_after_import
                    );
                } else if let Err(error) = &terminal {
                    warn!(
                        "Import window task wait failed: files={}, error={}",
                        file_count, error
                    );
                }
                this.finish_import_task(
                    terminal,
                    launch_after_import.then_some(launch_version),
                    cx,
                );
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn finish_import_task(
        &mut self,
        terminal: Result<std::sync::Arc<crate::tasks::task_manager::TaskSnapshot>, String>,
        launch_version: Option<LaunchVersionDescriptor>,
        cx: &mut Context<Self>,
    ) {
        self.is_importing = false;
        let should_launch_after_import = self.launch_after_import;
        self.launch_after_import = false;

        match terminal {
            Ok(snapshot) if snapshot.status.as_ref() == "completed" => {
                if let Some(version_folder) = self.selected_folder.clone() {
                    crate::ui::state::import::publish_import_completion(version_folder, cx);
                }
                let launch_started = should_launch_after_import
                    .then_some(())
                    .and(launch_version)
                    .and_then(|version| start_launcher(version, cx))
                    .is_some();
                let message = if launch_started {
                    t!("Import.importSuccessLaunching")
                } else {
                    t!("Import.auto_close", message = &t!("Import.importSuccess"))
                };
                toast::success(cx, message.clone());
                self.status = Some((StatusKind::Success, message));
                if launch_started {
                    self.close_after_launch_completion = true;
                    self.launch_completion_close_scheduled = false;
                } else {
                    self.schedule_auto_close(cx);
                }
            }
            Ok(snapshot) => {
                let error = snapshot
                    .message
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| snapshot.status.to_string());
                warn!(
                    "Import window finish failed: task_id={}, status={}, error={}",
                    snapshot.id,
                    snapshot.status,
                    error
                );
                self.close_after_launch_completion = false;
                self.launch_completion_close_scheduled = false;
                toast::error(cx, SharedString::from(error.clone()));
                self.status = Some((StatusKind::Error, SharedString::from(error)));
            }
            Err(error) => {
                warn!("Import window task feedback failed: error={}", error);
                self.close_after_launch_completion = false;
                self.launch_completion_close_scheduled = false;
                toast::error(cx, SharedString::from(error.clone()));
                self.status = Some((StatusKind::Error, SharedString::from(error)));
            }
        }
        cx.notify();
    }

    fn schedule_auto_close(&self, cx: &mut Context<Self>) {
        self.schedule_window_close_after(Duration::from_secs(5), cx);
    }

    fn schedule_window_close_after(&self, delay: Duration, cx: &mut Context<Self>) {
        if self.presentation == ImportPresentation::Overlay {
            self.schedule_overlay_close_after(delay, cx);
            return;
        }
        let Some(target_window_id) = self.window_id else {
            warn!("Import window close skipped: missing window id");
            return;
        };
        debug!(
            "Import window schedule close: window_id={}, delay_ms={}",
            target_window_id,
            delay.as_millis()
        );
        cx.spawn(async move |handle, cx| {
            cx.background_executor().timer(delay).await;
            let should_close = handle
                .read_with(cx, |this, _cx| {
                    this.status
                        .as_ref()
                        .is_some_and(|(kind, _)| *kind == StatusKind::Success)
                })
                .unwrap_or(false);
            if !should_close {
                debug!(
                    "Import window auto close cancelled: window_id={}",
                    target_window_id
                );
                return Ok::<(), anyhow::Error>(());
            }

            cx.update(|cx| {
                if let Some(window_handle) = cx
                    .windows()
                    .into_iter()
                    .find(|window| window.window_id().as_u64() == target_window_id)
                {
                    debug!("Import window closing: window_id={}", target_window_id);
                    let _ = window_handle.update(cx, |_root, window, _cx| {
                        window.remove_window();
                    });
                } else {
                    warn!(
                        "Import window close target missing: window_id={}",
                        target_window_id
                    );
                }
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn schedule_overlay_close_after(&self, delay: Duration, cx: &mut Context<Self>) {
        cx.spawn(async move |handle, cx| {
            cx.background_executor().timer(delay).await;
            let should_close = handle
                .read_with(cx, |this, _cx| {
                    this.status
                        .as_ref()
                        .is_some_and(|(kind, _)| *kind == StatusKind::Success)
                })
                .unwrap_or(false);
            if should_close {
                cx.update(crate::ui::state::import::dismiss_import_overlay)?;
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.presentation == ImportPresentation::Overlay {
            crate::ui::state::import::dismiss_import_overlay(cx);
        } else {
            window.remove_window();
        }
    }

    pub fn attach_window_id(&mut self, window_id: u64, cx: &mut Context<Self>) {
        self.window_id = Some(window_id);
        cx.notify();
    }

    fn theme_colors(&self, now: Instant, cx: &App) -> ThemeColors {
        let theme = cx.global::<ThemeState>();
        lerp_theme_colors(
            &LightColors::colors(),
            &DarkColors::colors(),
            theme.factor(now),
            theme.accent,
        )
    }

    fn kind_label(kind: &str, cx: &App) -> SharedString {
        if kind.starts_with("Import.") {
            cx.global::<I18n>()
                .lookup(kind)
                .unwrap_or_else(|| SharedString::from(kind.to_string()))
        } else {
            SharedString::from(kind.to_string())
        }
    }

    fn set_selected_folder(&mut self, folder: SharedString, cx: &mut Context<Self>) {
        if self.target.lock_version {
            return;
        }
        self.selected_folder = Some(folder);
        cx.notify();
    }

    fn target_user_id(&self, selected_folder: &SharedString) -> Option<String> {
        self.target
            .version_folder
            .as_ref()
            .is_some_and(|target| version_folder_matches(target.as_ref(), selected_folder.as_ref()))
            .then(|| self.target.user_id.as_ref().map(ToString::to_string))
            .flatten()
    }

    fn start_titlebar_drag(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _ = cx;
        self.titlebar_gesture
            .handle_mouse_down(event, window, Instant::now());
    }

    fn update_titlebar_drag(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _ = cx;
        self.titlebar_gesture.handle_mouse_move(event, window);
    }

    fn finish_titlebar_drag(&mut self, _cx: &mut Context<Self>) {
        self.titlebar_gesture.handle_mouse_up();
    }
}

impl Render for ImportWindowView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let frame_now = window.animation_time();
        sync_launcher_state(frame_now, cx);
        let window_width_px = if self.presentation == ImportPresentation::Overlay {
            ((window.bounds().size.width / px(1.0)) - 32.0).min(900.0)
        } else {
            window.bounds().size.width / px(1.0)
        };
        let use_two_column_layout = window_width_px >= IMPORT_WINDOW_TWO_COLUMN_WIDTH_PX;
        let colors = self.theme_colors(frame_now, cx);
        let is_overlay = self.presentation == ImportPresentation::Overlay;
        let content_padding = if is_overlay { 16.0 } else { 20.0 };
        let column_gap = if is_overlay { 16.0 } else { 24.0 };
        let sidebar_dropdown_width = px((window_width_px - 40.0).clamp(240.0, 520.0));
        let versions = read_local_versions_snapshot(cx);
        let launcher_snapshot = read_launcher_snapshot(frame_now, cx);
        let launcher_modal_animating = cx.global::<LauncherState>().is_modal_animating(frame_now);
        if self.close_after_launch_completion
            && !self.launch_completion_close_scheduled
            && launcher_snapshot
                .last_snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.status.as_ref() == "completed")
        {
            self.launch_completion_close_scheduled = true;
            self.close_after_launch_completion = false;
            self.schedule_window_close_after(Duration::from_millis(1200), cx);
        }
        let title = t!("Import.title");
        let processing_label = t!("Import.processing");
        let done_label = t!("Import.done");
        let start_import_label = t!("Import.startImport");
        let start_import_and_launch_label = t!("Import.startImportAndLaunch");

        let mut root = div()
            .relative()
            .size_full()
            .bg(if is_overlay {
                colors.settings_panel_bg
            } else {
                colors.bg
            })
            .when(!is_overlay, |this| this.child(background_layer(&colors)))
            .child(
                div()
                    .relative()
                    .size_full()
                    .flex()
                    .flex_col()
                    .min_h(px(0.))
                    .child(render_window_header(self, &colors, title, window, cx))
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h(px(0.))
                            .overflow_y_scrollbar()
                            .px(px(content_padding))
                            .py(px(if is_overlay { 14.0 } else { 18.0 }))
                            .flex()
                            .min_h(px(0.))
                            .justify_center()
                            .child(
                                div()
                                    .w_full()
                                    .max_w(px(1080.))
                                    .min_h(px(0.))
                                    .when(use_two_column_layout, |this| {
                                        this.child(
                                            div()
                                                .w_full()
                                                .flex()
                                                .items_start()
                                                .min_h(px(0.))
                                                .gap(px(column_gap))
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w(px(0.))
                                                        .min_h(px(0.))
                                                        .overflow_y_scrollbar()
                                                        .child(render_preview_card(
                                                            self, &colors, frame_now, cx,
                                                        )),
                                                )
                                                .child(
                                                    div()
                                                        .w(px(if is_overlay {
                                                            316.0
                                                        } else {
                                                            332.0
                                                        }))
                                                        .flex_none()
                                                        .flex()
                                                        .flex_col()
                                                        .gap(px(14.))
                                                        .child(render_versions_card(
                                                            self,
                                                            &colors,
                                                            cx,
                                                            &versions,
                                                            px(if is_overlay {
                                                                280.0
                                                            } else {
                                                                296.0
                                                            }),
                                                        ))
                                                        .when_some(
                                                            render_status_box(self, &colors),
                                                            |this, status| this.child(status),
                                                        ),
                                                ),
                                        )
                                    })
                                    .when(!use_two_column_layout, |this| {
                                        this.child(
                                            div()
                                                .w_full()
                                                .flex()
                                                .flex_col()
                                                .gap(px(14.))
                                                .child(render_preview_card(
                                                    self, &colors, frame_now, cx,
                                                ))
                                                .child(render_versions_card(
                                                    self,
                                                    &colors,
                                                    cx,
                                                    &versions,
                                                    sidebar_dropdown_width,
                                                ))
                                                .when_some(
                                                    render_status_box(self, &colors),
                                                    |this, status| this.child(status),
                                                ),
                                        )
                                    }),
                            ),
                    )
                    .child(render_footer(
                        self,
                        &colors,
                        if self.is_importing {
                            processing_label.clone()
                        } else if self
                            .status
                            .as_ref()
                            .is_some_and(|(kind, _)| *kind == StatusKind::Success)
                        {
                            done_label.clone()
                        } else {
                            start_import_label
                        },
                        if self.is_importing {
                            processing_label
                        } else {
                            start_import_and_launch_label
                        },
                        versions.versions.is_empty() || self.has_invalid_or_failed_preview(),
                        cx,
                    )),
            );

        if self.show_conflict_dialog {
            if let Some(conflict) = self.conflict.as_ref() {
                root = root.child(render_conflict_dialog(self, conflict, &colors, cx));
            }
        }

        if self.presentation == ImportPresentation::Window && launcher_snapshot.show_modal {
            root = root.child(
                crate::ui::overlays::launcher::render_launcher_overlay(
                    &launcher_snapshot,
                    window,
                    cx,
                )
                .with_layout_animation_target(launcher_modal_animating),
            );
        }

        #[cfg(target_os = "windows")]
        {
            let launch_prereq_state = cx.global::<LaunchPrereqState>();
            if self.presentation == ImportPresentation::Window && launch_prereq_state.visible {
                root = root.child(
                    crate::ui::overlays::launch_prereq::render_launch_prereq_overlay(
                        launch_prereq_state,
                        window,
                        cx,
                    ),
                );
            }
        }

        let toast_state = cx.global::<toast::ToastState>();
        let window_id = window.window_handle().window_id();
        if self.presentation == ImportPresentation::Window
            && toast::has_visible_toasts(window_id, frame_now, toast_state)
        {
            root = root.child(toast::render_overlay(
                window,
                cx,
                &colors,
                frame_now,
                toast_state,
            ));
        }
        if self.presentation == ImportPresentation::Window
            && toast::has_visible_breadcrumb(window_id, frame_now, toast_state)
        {
            root = root.child(toast::render_breadcrumb_overlay(
                window,
                cx,
                &colors,
                frame_now,
                toast_state,
            ));
        }

        let dropdown_state = cx.global::<dropdown::DropdownOverlayState>();
        if self.presentation == ImportPresentation::Window
            && dropdown::has_visible_overlay(window, frame_now, dropdown_state)
        {
            root = root.child(dropdown::render_overlay(window, frame_now, dropdown_state));
        }

        root
    }
}

fn render_window_header(
    view: &ImportWindowView,
    colors: &ThemeColors,
    title: SharedString,
    window: &mut Window,
    cx: &mut Context<ImportWindowView>,
) -> AnyElement {
    let is_overlay = view.presentation == ImportPresentation::Overlay;
    let window_width = if is_overlay {
        900.0
    } else {
        window.bounds().size.width / px(1.0)
    };
    let compact_controls = window_width <= IMPORT_WINDOW_COMPACT_WIDTH_PX;
    let hide_file_path = window_width <= IMPORT_WINDOW_NARROW_WIDTH_PX;
    let controls_width = if compact_controls { px(56.) } else { px(68.) };

    let drag_surface = div()
        .w_full()
        .h(px(if is_overlay { 56.0 } else { 46.0 }))
        .px(px(if is_overlay { 16.0 } else { 12.0 }))
        .flex()
        .items_center()
        .justify_between()
        .bg(Hsla {
            a: 0.94,
            ..colors.settings_panel_bg
        })
        .border_b_1()
        .border_color(Hsla {
            a: 0.12,
            ..colors.border
        })
        .when(!is_overlay && !cfg!(windows), |this| {
            this.on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event, window, cx| {
                    this.start_titlebar_drag(event, window, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event, window, cx| {
                this.update_titlebar_drag(event, window, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| this.finish_titlebar_drag(cx)),
            )
        })
        .when(!is_overlay && cfg!(windows), |this| {
            this.window_control_area(WindowControlArea::Drag)
        })
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .items_center()
                .gap(px(10.))
                .child(
                    div()
                        .flex_none()
                        .w(px(28.))
                        .h(px(28.))
                        .rounded(px(crate::ui::theme::tokens::radius::SM))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(Hsla {
                            a: 0.10,
                            ..colors.accent
                        })
                        .child(icon(lucide_gpui::icon!(package_open), 15.0, colors.accent)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.text_primary)
                                .child(title),
                        )
                        .when(!hide_file_path, |this| {
                            this.child(
                                div()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_size(px(10.))
                                    .text_color(colors.text_secondary)
                                    .child(import_file_summary(view)),
                            )
                        }),
                ),
        )
        .child(if is_overlay {
            render_overlay_controls(colors, controls_width, cx)
        } else {
            render_window_controls(colors, window, compact_controls, controls_width)
        });

    drag_surface.into_any_element()
}

fn render_overlay_controls(
    colors: &ThemeColors,
    width: Pixels,
    cx: &mut Context<ImportWindowView>,
) -> Div {
    div()
        .flex_none()
        .w(width)
        .flex()
        .justify_end()
        .items_center()
        .child(
            titlebar_button(colors, lucide_gpui::icon!(x), true, false)
                .id("import-overlay-close")
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.request_close(window, cx);
                })),
        )
}

fn background_layer(colors: &ThemeColors) -> Div {
    div().absolute().inset_0().bg(linear_gradient(
        135.,
        linear_color_stop(
            Hsla {
                a: 0.12,
                ..colors.accent
            },
            0.0,
        ),
        linear_color_stop(
            Hsla {
                a: 0.08,
                ..colors.stat_green_text
            },
            1.0,
        ),
    ))
}

fn section_divider(colors: &ThemeColors) -> Div {
    div().w_full().h(px(1.)).bg(Hsla {
        a: 0.08,
        ..colors.border
    })
}

fn section_shell(colors: &ThemeColors) -> Div {
    div()
        .w_full()
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .px(px(18.))
        .py(px(18.))
        .bg(Hsla {
            a: 0.82,
            ..colors.settings_panel_bg
        })
        .border_1()
        .border_color(Hsla {
            a: 0.08,
            ..colors.border
        })
        .flex()
        .flex_col()
        .gap(px(14.))
}

fn overlay_panel(colors: &ThemeColors) -> Div {
    div()
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .bg(Hsla {
            a: 0.88,
            ..colors.settings_panel_bg
        })
        .border_1()
        .border_color(Hsla {
            a: 0.12,
            ..colors.border
        })
        .shadow(vec![BoxShadow {
            color: Hsla {
                a: 0.12,
                ..rgb(0x000000).into()
            },
            blur_radius: px(24.),
            spread_radius: px(-10.),
            offset: point(px(0.), px(12.)),
        }])
}

fn titlebar_button(
    colors: &ThemeColors,
    icon_path: &'static str,
    danger: bool,
    compact: bool,
) -> Div {
    let hover = if danger {
        Hsla {
            a: 0.14,
            ..colors.danger
        }
    } else {
        Hsla {
            a: 0.10,
            ..colors.surface_hover
        }
    };
    let tone = if danger {
        colors.danger
    } else {
        colors.text_secondary
    };

    div()
        .flex_none()
        .flex_shrink_0()
        .w(if compact { px(24.) } else { px(32.) })
        .h(px(32.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .child(icon(icon_path, if compact { 12.0 } else { 14.0 }, tone))
}

fn render_window_controls(
    colors: &ThemeColors,
    window: &mut Window,
    compact: bool,
    width: Pixels,
) -> Div {
    let _ = window;
    div()
        .flex_none()
        .flex_shrink_0()
        .w(width)
        .flex()
        .items_center()
        .gap(px(4.))
        .child(
            titlebar_button(colors, lucide_gpui::icon!(minus), false, compact)
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    cx.stop_propagation();
                    window.minimize_window();
                }),
        )
        .child(
            titlebar_button(colors, lucide_gpui::icon!(x), true, compact)
                .id("import-window-close")
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(|_, window, _| {
                    window.remove_window();
                }),
        )
}

fn icon(path: &'static str, size_px: f32, color: Hsla) -> Svg {
    svg()
        .path(path)
        .w(px(size_px))
        .h(px(size_px))
        .text_color(color)
}

fn preview_icon(
    icon_data: Option<&PreviewIconData>,
    size_px: f32,
    colors: &ThemeColors,
) -> AnyElement {
    let mut frame = div()
        .w(px(size_px))
        .h(px(size_px))
        .flex_none()
        .rounded(px((size_px / 4.0).round()))
        .overflow_hidden()
        .bg(Hsla {
            a: 0.48,
            ..colors.surface
        })
        .border_1()
        .border_color(Hsla {
            a: 0.08,
            ..colors.border
        });
    if let Some(source) = preview_icon_source(icon_data) {
        frame = frame.child(img(source).w_full().h_full());
    } else {
        frame = frame.child(
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(icon(
                    lucide_gpui::icon!(package),
                    (size_px / 2.4).max(14.0),
                    colors.text_muted,
                )),
        );
    }
    frame.into_any_element()
}

fn preview_icon_source(icon: Option<&PreviewIconData>) -> Option<EncodedImageBytes> {
    let icon = icon?;
    Some(EncodedImageBytes::new(
        image_format_for_preview_icon(icon.format.clone()),
        icon.bytes.clone(),
    ))
}

fn image_format_for_preview_icon(format: PreviewImageFormat) -> ImageFormat {
    match format {
        PreviewImageFormat::Png => ImageFormat::Png,
        PreviewImageFormat::Jpeg => ImageFormat::Jpeg,
        PreviewImageFormat::Webp => ImageFormat::Webp,
        PreviewImageFormat::Gif => ImageFormat::Gif,
        PreviewImageFormat::Bmp => ImageFormat::Bmp,
    }
}

fn spinning_icon(
    now: Instant,
    started_at: Instant,
    path: &'static str,
    size_px: f32,
    color: Hsla,
) -> Svg {
    let elapsed = now.saturating_duration_since(started_at).as_secs_f32();
    let rotation = (elapsed / 0.9).fract() * std::f32::consts::TAU;
    svg()
        .path(path)
        .w(px(size_px))
        .h(px(size_px))
        .text_color(color)
        .with_transformation(Transformation::rotate(radians(rotation)))
}

fn render_preview_card(
    this: &ImportWindowView,
    colors: &ThemeColors,
    now: Instant,
    cx: &App,
) -> AnyElement {
    let i18n = cx.global::<I18n>();
    if this.is_inspecting {
        return section_shell(colors)
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                spinning_icon(
                    now,
                    this.inspect_started_at,
                    lucide_gpui::icon!(loader_circle),
                    18.0,
                    colors.accent,
                )
                .with_layout_animation_target(true),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(colors.text_secondary)
                    .child(t!("Import.inspecting")),
            )
            .into_any_element();
    }

    if this.file_paths.len() > 1 {
        return render_batch_preview_cards(this, colors, cx);
    }

    if let Some(preview) = &this.preview {
        if is_world_preview(preview) {
            let map_name_label = t!("Import.mapName");
            let map_type_label = t!("Import.mapType");
            let file_size_label = t!("Import.fileSize");

            let mut card = section_shell(colors)
                .flex()
                .flex_col()
                .gap(px(16.))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(16.))
                        .child(preview_icon(preview.icon.as_ref(), 80.0, colors))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.text_secondary)
                                        .child(t!("Import.mapInfo")),
                                )
                                .child(
                                    MinecraftFormattedText::new(
                                        SharedString::from(preview.name.clone()),
                                        colors,
                                    )
                                    .text_size(px(17.))
                                    .line_height(relative(1.25))
                                    .color(colors.text_primary),
                                )
                                .when(!preview.description.is_empty(), |this| {
                                    this.child(
                                        MinecraftFormattedText::new(
                                            SharedString::from(preview.description.clone()),
                                            colors,
                                        )
                                        .text_size(px(12.))
                                        .line_height(relative(1.45))
                                        .color(colors.text_secondary),
                                    )
                                }),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .child(preview_info_row(
                            colors,
                            map_name_label,
                            SharedString::from(preview.name.clone()),
                            lucide_gpui::icon!(map),
                        ))
                        .child(preview_info_row(
                            colors,
                            map_type_label,
                            ImportWindowView::kind_label(&preview.kind, cx),
                            lucide_gpui::icon!(map_pinned),
                        ))
                        .child(preview_info_row(
                            colors,
                            file_size_label,
                            SharedString::from(format_size(preview.size)),
                            lucide_gpui::icon!(hard_drive_download),
                        )),
                );

            if let Some(sub_packs) = &preview.sub_packs {
                card = card.child(render_embedded_pack_list(
                    sub_packs,
                    this,
                    colors,
                    cx,
                    t!("Import.embeddedPacksCount", count = sub_packs.len()),
                ));
            }

            if let Some(references) = &preview.world_pack_references {
                card = card.child(render_world_pack_reference_list(
                    references,
                    preview.sub_packs.as_deref(),
                    colors,
                    cx,
                ));
            }

            if !preview.valid {
                let reason = preview
                    .invalid_reason
                    .clone()
                    .unwrap_or_else(|| t!("Import.errors.missingUuid").to_string());
                card = card.child(
                    div()
                        .rounded(px(crate::ui::theme::tokens::radius::SM))
                        .px(px(10.))
                        .py(px(8.))
                        .bg(Hsla {
                            a: 0.10,
                            ..colors.danger
                        })
                        .border_1()
                        .border_color(Hsla {
                            a: 0.18,
                            ..colors.danger
                        })
                        .text_size(px(11.))
                        .text_color(colors.danger)
                        .child(t!("Import.errors.invalidPack", reason = &reason)),
                );
            }

            return card.into_any_element();
        }

        if is_world_template_preview(preview) {
            let template_name_label = t!("Import.templateName");
            let template_type_label = t!("Import.templateType");
            let template_version_label = t!("Import.templateVersion");
            let file_size_label = t!("Import.fileSize");

            let mut card = section_shell(colors)
                .flex()
                .flex_col()
                .gap(px(16.))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(16.))
                        .child(preview_icon(preview.icon.as_ref(), 80.0, colors))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.text_secondary)
                                        .child(t!("Import.templateInfo")),
                                )
                                .child(
                                    MinecraftFormattedText::new(
                                        SharedString::from(preview.name.clone()),
                                        colors,
                                    )
                                    .text_size(px(17.))
                                    .line_height(relative(1.25))
                                    .color(colors.text_primary),
                                )
                                .child(
                                    MinecraftFormattedText::new(
                                        if preview.description.is_empty() {
                                            t!("Import.noDescription")
                                        } else {
                                            SharedString::from(preview.description.clone())
                                        },
                                        colors,
                                    )
                                    .text_size(px(12.))
                                    .line_height(relative(1.45))
                                    .color(colors.text_secondary),
                                ),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .child(preview_info_row(
                            colors,
                            template_name_label,
                            SharedString::from(preview.name.clone()),
                            lucide_gpui::icon!(package_open),
                        ))
                        .child(preview_info_row(
                            colors,
                            template_type_label,
                            ImportWindowView::kind_label(&preview.kind, cx),
                            lucide_gpui::icon!(layout_template),
                        ))
                        .when_some(preview.version.as_ref(), |this, version| {
                            this.child(preview_info_row(
                                colors,
                                template_version_label,
                                SharedString::from(version.clone()),
                                lucide_gpui::icon!(tag),
                            ))
                        })
                        .child(preview_info_row(
                            colors,
                            file_size_label,
                            SharedString::from(format_size(preview.size)),
                            lucide_gpui::icon!(hard_drive_download),
                        )),
                );

            if let Some(sub_packs) = &preview.sub_packs {
                card = card.child(render_embedded_pack_list(
                    sub_packs,
                    this,
                    colors,
                    cx,
                    t!("Import.templatePacksCount", count = sub_packs.len()),
                ));
            }

            if let Some(references) = &preview.world_pack_references {
                card = card.child(render_world_pack_reference_list(
                    references,
                    preview.sub_packs.as_deref(),
                    colors,
                    cx,
                ));
            }

            if !preview.valid {
                let reason = preview
                    .invalid_reason
                    .clone()
                    .unwrap_or_else(|| t!("Import.errors.missingUuid").to_string());
                card = card.child(
                    div()
                        .rounded(px(crate::ui::theme::tokens::radius::SM))
                        .px(px(10.))
                        .py(px(8.))
                        .bg(Hsla {
                            a: 0.10,
                            ..colors.danger
                        })
                        .border_1()
                        .border_color(Hsla {
                            a: 0.18,
                            ..colors.danger
                        })
                        .text_size(px(11.))
                        .text_color(colors.danger)
                        .child(t!("Import.errors.invalidPack", reason = &reason)),
                );
            }

            return card.into_any_element();
        }

        let mut card = section_shell(colors).flex().flex_col().gap(px(14.)).child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(16.))
                .items_start()
                .child(preview_icon(preview.icon.as_ref(), 64.0, colors))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            MinecraftFormattedText::new(
                                SharedString::from(preview.name.clone()),
                                colors,
                            )
                            .text_size(px(15.))
                            .line_height(relative(1.25))
                            .color(colors.text_primary),
                        )
                        .child(
                            MinecraftFormattedText::new(
                                if preview.description.is_empty() {
                                    t!("Import.noDescription")
                                } else {
                                    SharedString::from(preview.description.clone())
                                },
                                colors,
                            )
                            .text_size(px(12.))
                            .line_height(relative(1.45))
                            .color(colors.text_secondary),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(6.))
                                .flex_wrap()
                                .child(meta_pill(
                                    colors,
                                    ImportWindowView::kind_label(&preview.kind, cx),
                                    true,
                                ))
                                .when_some(preview.version.as_ref(), |this, version| {
                                    this.child(meta_pill(
                                        colors,
                                        SharedString::from(format!("v{version}")),
                                        false,
                                    ))
                                })
                                .child(meta_pill(
                                    colors,
                                    SharedString::from(format_size(preview.size)),
                                    false,
                                )),
                        ),
                ),
        );

        if !preview.valid {
            let reason = preview
                .invalid_reason
                .clone()
                .unwrap_or_else(|| t!("Import.errors.missingUuid").to_string());
            card = card.child(
                div()
                    .rounded(px(crate::ui::theme::tokens::radius::SM))
                    .px(px(10.))
                    .py(px(8.))
                    .bg(Hsla {
                        a: 0.10,
                        ..colors.danger
                    })
                    .border_1()
                    .border_color(Hsla {
                        a: 0.18,
                        ..colors.danger
                    })
                    .text_size(px(11.))
                    .text_color(colors.danger)
                    .child(t!("Import.errors.invalidPack", reason = &reason)),
            );
        }

        if let Some(sub_packs) = &preview.sub_packs {
            let sub_pack_count = sub_packs.len().to_string();
            let mut list = div().flex().flex_col().gap(px(8.)).child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(icon(
                        lucide_gpui::icon!(layers_2),
                        14.0,
                        colors.text_secondary,
                    ))
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text_secondary)
                            .child(t!("Import.subPacksCount", count = &sub_pack_count)),
                    ),
            );

            for (index, sub_pack) in sub_packs.iter().enumerate() {
                list = list.child(
                    div()
                        .id(("import-sub-pack", index))
                        .rounded(px(crate::ui::theme::tokens::radius::SM))
                        .px(px(10.))
                        .py(px(8.))
                        .bg(Hsla {
                            a: 0.36,
                            ..colors.settings_field_bg
                        })
                        .border_1()
                        .border_color(Hsla {
                            a: 0.06,
                            ..colors.border
                        })
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(preview_icon(sub_pack.icon.as_ref(), 24.0, colors))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .gap(px(3.))
                                .child(
                                    MinecraftFormattedText::new(
                                        SharedString::from(sub_pack.name.clone()),
                                        colors,
                                    )
                                    .text_size(px(12.))
                                    .line_height(relative(1.2))
                                    .color(colors.text_primary),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(colors.text_secondary)
                                        .child(ImportWindowView::kind_label(&sub_pack.kind, cx)),
                                ),
                        )
                        .when(!sub_pack.valid, |this| {
                            this.child(
                                div()
                                    .text_size(px(10.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(colors.danger)
                                    .child(t!("Import.invalidPackShort")),
                            )
                        }),
                );
            }
            card = card.child(list);
        }

        return card.into_any_element();
    }

    section_shell(colors)
        .min_h(px(96.))
        .flex()
        .items_center()
        .child(
            div()
                .flex()
                .gap(px(10.))
                .items_center()
                .child(icon(lucide_gpui::icon!(file_up), 24.0, colors.text_muted))
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(relative(1.45))
                        .text_color(colors.text_secondary)
                        .text_center()
                        .child(import_file_summary(this)),
                ),
        )
        .into_any_element()
}

fn import_file_summary(view: &ImportWindowView) -> SharedString {
    if view.file_paths.len() <= 1 {
        return SharedString::from(view.import_context.file_path.display().to_string());
    }

    t!(
        "Import.batch_files",
        count = &view.file_paths.len().to_string()
    )
}

fn render_batch_preview_cards(
    view: &ImportWindowView,
    colors: &ThemeColors,
    cx: &App,
) -> AnyElement {
    let mut list = section_shell(colors)
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child(t!(
                            "Import.batch_preview_title",
                            count = &view.file_paths.len().to_string()
                        )),
                )
                .child(meta_pill(
                    colors,
                    t!(
                        "Import.batch_preview_ready",
                        count = &view
                            .batch_previews
                            .iter()
                            .filter(|entry| entry.preview.is_some() && entry.error.is_none())
                            .count()
                            .to_string()
                    ),
                    !view.has_invalid_or_failed_preview(),
                )),
        );

    for (index, entry) in view.batch_previews.iter().enumerate() {
        let file_name = entry
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| entry.path.display().to_string());
        let row = div()
            .id(("import-batch-preview", index))
            .w_full()
            .rounded(px(crate::ui::theme::tokens::radius::SM))
            .px(px(12.))
            .py(px(10.))
            .bg(Hsla {
                a: 0.36,
                ..colors.settings_field_bg
            })
            .border_1()
            .border_color(Hsla {
                a: 0.08,
                ..colors.border
            })
            .flex()
            .items_center()
            .gap(px(12.));

        let row = if let Some(preview) = entry.preview.as_ref() {
            row.child(preview_icon(preview.icon.as_ref(), 42.0, colors))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            MinecraftFormattedText::new(
                                SharedString::from(preview.name.clone()),
                                colors,
                            )
                            .text_size(px(13.))
                            .line_height(relative(1.2))
                            .color(colors.text_primary),
                        )
                        .child(
                            div()
                                .overflow_hidden()
                                .text_ellipsis()
                                .text_size(px(10.))
                                .text_color(colors.text_muted)
                                .child(SharedString::from(file_name)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap(px(6.))
                                .child(meta_pill(
                                    colors,
                                    ImportWindowView::kind_label(&preview.kind, cx),
                                    preview.valid,
                                ))
                                .when_some(preview.version.as_ref(), |this, version| {
                                    this.child(meta_pill(
                                        colors,
                                        SharedString::from(format!("v{version}")),
                                        false,
                                    ))
                                })
                                .child(meta_pill(
                                    colors,
                                    SharedString::from(format_size(preview.size)),
                                    false,
                                )),
                        ),
                )
                .when(!preview.valid, |this| {
                    this.child(
                        div()
                            .text_size(px(10.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(colors.danger)
                            .child(t!("Import.invalidPackShort")),
                    )
                })
        } else {
            row.child(preview_icon(None, 42.0, colors)).child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text_primary)
                            .child(SharedString::from(file_name)),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(colors.danger)
                            .child(
                                entry
                                    .error
                                    .clone()
                                    .unwrap_or_else(|| t!("Import.preview_unavailable")),
                            ),
                    ),
            )
        };
        list = list.child(row);
    }

    list.into_any_element()
}

fn render_versions_card(
    this: &ImportWindowView,
    colors: &ThemeColors,
    cx: &mut Context<ImportWindowView>,
    versions: &crate::ui::hooks::use_local_versions::LocalVersionsSnapshot,
    dropdown_width: Pixels,
) -> AnyElement {
    let i18n = cx.global::<I18n>();
    let mut card = section_shell(colors).flex().flex_col().gap(px(10.)).child(
        div()
            .text_size(px(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(colors.text_secondary)
            .child(t!("Import.targetVersion")),
    );

    if versions.versions.is_empty() {
        return card
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(colors.text_secondary)
                    .child(t!("Import.noVersions")),
            )
            .into_any_element();
    }

    let selected_index = selected_version_index(this, versions);
    let selected_version = selected_index.and_then(|index| versions.versions.get(index));
    let dropdown_label = selected_version
        .map(version_primary_label)
        .or_else(|| this.target.instance_name.clone())
        .unwrap_or_else(|| t!("Import.noVersions"));
    let dropdown_options = versions
        .versions
        .iter()
        .map(|version| DropdownOption {
            label: version_dropdown_label(version, i18n),
        })
        .collect::<Vec<_>>();
    let view_handle = cx.entity().downgrade();
    let folder_values = versions
        .versions
        .iter()
        .map(|version| SharedString::from(version.folder.clone()))
        .collect::<Vec<_>>();
    let selected_meta = selected_version
        .map(version_detail_label)
        .or_else(|| this.target.game_version.clone())
        .unwrap_or_else(|| t!("Import.noVersions"));
    let target_locked = this.target.lock_version;
    let trigger_meta = if target_locked {
        t!("Import.locked_instance", meta = &selected_meta)
    } else {
        selected_meta.clone()
    };

    card = card
        .child(Dropdown::with_trigger(
            SharedString::from("import-target-version-dropdown"),
            colors,
            dropdown_width,
            px(58.),
            dropdown_label,
            dropdown_options,
            selected_index.unwrap_or(0),
            !target_locked,
            move |colors, _width, _trigger_height, enabled, open_k, label| {
                let chevron = svg()
                    .path(lucide_gpui::icon!(chevron_down))
                    .w(px(18.))
                    .h(px(18.))
                    .text_color(colors.text_secondary)
                    .opacity(if enabled { 0.78 } else { 0.36 })
                    .with_transformation(Transformation::rotate(radians(
                        open_k * std::f32::consts::PI,
                    )));

                div()
                    .size_full()
                    .px(px(14.))
                    .py(px(11.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .min_w(px(0.))
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(icon(
                                lucide_gpui::icon!(folder_open),
                                16.0,
                                if enabled {
                                    colors.accent
                                } else {
                                    colors.text_muted
                                },
                            ))
                            .child(
                                div()
                                    .min_w(px(0.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(3.))
                                    .child(
                                        div()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(colors.text_primary)
                                            .child(label.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(colors.text_secondary)
                                            .child(trigger_meta.clone()),
                                    ),
                            ),
                    )
                    .child(chevron)
                    .into_any_element()
            },
            move |index, _window, cx| {
                if let Some(folder) = folder_values.get(index).cloned() {
                    if let Err(error) = view_handle.update(cx, |this, cx| {
                        this.set_selected_folder(folder, cx);
                    }) {
                        tracing::debug!("import version dropdown selection skipped: {error:?}");
                    }
                }
            },
        ))
        .child(info_summary_panel(
            colors,
            i18n,
            selected_version,
            selected_meta,
            t!("Import.unknown"),
        ));
    card.into_any_element()
}

fn is_world_preview(preview: &PackagePreview) -> bool {
    preview.kind == "Import.minecraftWorlds"
}

fn is_world_template_preview(preview: &PackagePreview) -> bool {
    preview.kind == "Import.worldTemplates"
}

fn preview_info_row(
    colors: &ThemeColors,
    label: SharedString,
    value: SharedString,
    icon_path: &'static str,
) -> AnyElement {
    div()
        .w_full()
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .px(px(12.))
        .py(px(10.))
        .bg(Hsla {
            a: 0.34,
            ..colors.surface
        })
        .border_1()
        .border_color(Hsla {
            a: 0.06,
            ..colors.border
        })
        .flex()
        .items_start()
        .gap(px(10.))
        .child(icon(icon_path, 14.0, colors.text_muted))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(10.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_secondary)
                        .child(label),
                )
                .child(
                    div()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(value),
                ),
        )
        .into_any_element()
}

fn render_embedded_pack_list(
    sub_packs: &[PackagePreview],
    view: &ImportWindowView,
    colors: &ThemeColors,
    cx: &App,
    title: SharedString,
) -> AnyElement {
    let i18n = cx.global::<I18n>();
    let mut list = div().w_full().flex().flex_col().gap(px(8.)).child(
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(icon(
                lucide_gpui::icon!(package_plus),
                14.0,
                colors.text_secondary,
            ))
            .child(
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.text_secondary)
                    .child(title.clone()),
            ),
    );

    for (index, sub_pack) in sub_packs.iter().enumerate() {
        list = list.child(
            div()
                .id(("import-embedded-pack", index))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .px(px(10.))
                .py(px(8.))
                .bg(Hsla {
                    a: 0.36,
                    ..colors.settings_field_bg
                })
                .border_1()
                .border_color(Hsla {
                    a: 0.06,
                    ..colors.border
                })
                .flex()
                .items_center()
                .gap(px(10.))
                .child(preview_icon(sub_pack.icon.as_ref(), 24.0, colors))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(
                            MinecraftFormattedText::new(
                                SharedString::from(sub_pack.name.clone()),
                                colors,
                            )
                            .text_size(px(12.))
                            .line_height(relative(1.2))
                            .color(colors.text_primary),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(colors.text_secondary)
                                .child(ImportWindowView::kind_label(&sub_pack.kind, cx)),
                        ),
                )
                .when(!sub_pack.valid, |this| {
                    this.child(
                        div()
                            .text_size(px(10.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(colors.danger)
                            .child(t!("Import.invalidPackShort")),
                    )
                }),
        );
    }

    list.into_any_element()
}

fn render_world_pack_reference_list(
    references: &[WorldPackReference],
    sub_packs: Option<&[PackagePreview]>,
    colors: &ThemeColors,
    cx: &App,
) -> AnyElement {
    let i18n = cx.global::<I18n>();
    let count = references.len().to_string();
    let matched_count = references
        .iter()
        .filter(|reference| matched_pack_name_for_reference(reference, sub_packs).is_some())
        .count();
    let unmatched_count = references.len().saturating_sub(matched_count);
    let mut list = section_shell(colors)
        .gap(px(10.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(icon(
                    lucide_gpui::icon!(list_ordered),
                    14.0,
                    colors.text_secondary,
                ))
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_secondary)
                        .child(t!("Import.worldPackRefsCount", count = &count)),
                ),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .child(meta_pill(
                    colors,
                    t!("Import.worldPackRefsMatchedCount", count = matched_count),
                    true,
                ))
                .child(meta_pill(
                    colors,
                    t!(
                        "Import.worldPackRefsUnmatchedCount",
                        count = unmatched_count
                    ),
                    unmatched_count == 0,
                )),
        );

    for (index, reference) in references.iter().enumerate() {
        let order_label = t!("Import.packOrder", order = reference.order);
        let matched_pack = matched_pack_name_for_reference(reference, sub_packs);
        let matched = matched_pack.is_some();
        list = list.child(
            div()
                .id(("world-pack-reference", index))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .px(px(12.))
                .py(px(10.))
                .bg(Hsla {
                    a: 0.34,
                    ..colors.surface
                })
                .border_1()
                .border_color(Hsla {
                    a: 0.06,
                    ..colors.border
                })
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.))
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text_primary)
                                .child(ImportWindowView::kind_label(&reference.pack_type, cx)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(meta_pill(colors, order_label, true))
                                .child(meta_pill(
                                    colors,
                                    if matched {
                                        t!("Import.referenceMatched")
                                    } else {
                                        t!("Import.referenceUnmatched")
                                    },
                                    matched,
                                )),
                        ),
                )
                .child(preview_info_row(
                    colors,
                    t!("Import.dependencyUuid"),
                    SharedString::from(reference.uuid.clone()),
                    lucide_gpui::icon!(tag),
                ))
                .when_some(matched_pack, |this, pack_name| {
                    this.child(preview_info_row(
                        colors,
                        t!("Import.matchedPack"),
                        pack_name,
                        lucide_gpui::icon!(package_open),
                    ))
                })
                .when(!matched, |this| {
                    this.child(
                        div()
                            .rounded(px(crate::ui::theme::tokens::radius::SM))
                            .px(px(10.))
                            .py(px(8.))
                            .bg(Hsla {
                                a: 0.10,
                                ..colors.danger
                            })
                            .border_1()
                            .border_color(Hsla {
                                a: 0.18,
                                ..colors.danger
                            })
                            .text_size(px(11.))
                            .text_color(colors.danger)
                            .child(t!("Import.referenceMissingHint")),
                    )
                })
                .when_some(reference.version.as_ref(), |this, version| {
                    this.child(preview_info_row(
                        colors,
                        t!("Import.packVersion"),
                        SharedString::from(version.clone()),
                        lucide_gpui::icon!(tag),
                    ))
                })
                .when_some(reference.subpack.as_ref(), |this, subpack| {
                    this.child(preview_info_row(
                        colors,
                        t!("Import.subpackName"),
                        SharedString::from(subpack.clone()),
                        lucide_gpui::icon!(boxes),
                    ))
                }),
        );
    }

    list.into_any_element()
}

fn matched_pack_name_for_reference(
    reference: &WorldPackReference,
    sub_packs: Option<&[PackagePreview]>,
) -> Option<SharedString> {
    let sub_packs = sub_packs?;
    sub_packs.iter().find_map(|preview| {
        let header = preview.manifest.as_ref()?.header.as_ref()?;
        let uuid = header.uuid.as_ref()?;
        if uuid.eq_ignore_ascii_case(&reference.uuid) {
            Some(SharedString::from(preview.name.clone()))
        } else {
            None
        }
    })
}

fn info_summary_panel(
    colors: &ThemeColors,
    i18n: &I18n,
    selected_version: Option<&crate::core::version::launch_versions::LaunchVersionEntry>,
    selected_meta: SharedString,
    unknown_label: SharedString,
) -> AnyElement {
    let primary = selected_version
        .map(version_primary_label)
        .unwrap_or(unknown_label.clone());
    let target_path = selected_version
        .and_then(version_target_root_path)
        .unwrap_or_else(|| SharedString::from("-"));
    let isolation = selected_version
        .map(|version| version_isolation_label(version, i18n))
        .unwrap_or_else(|| SharedString::from("-"));
    let version_type = selected_version
        .map(|version| version_type_summary_label(version, i18n))
        .unwrap_or_else(|| SharedString::from("-"));

    div()
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .px(px(12.))
        .py(px(12.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .bg(Hsla {
            a: 0.32,
            ..colors.settings_field_bg
        })
        .border_1()
        .border_color(Hsla {
            a: 0.08,
            ..colors.border
        })
        .child(
            div()
                .flex()
                .gap(px(10.))
                .items_center()
                .child(icon(
                    lucide_gpui::icon!(badge_info),
                    16.0,
                    colors.text_muted,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(
                            div()
                                .overflow_hidden()
                                .text_ellipsis()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text_primary)
                                .child(primary),
                        )
                        .child(
                            div()
                                .overflow_hidden()
                                .text_ellipsis()
                                .text_size(px(11.))
                                .text_color(colors.text_secondary)
                                .child(selected_meta),
                        ),
                ),
        )
        .child(info_summary_row(
            colors,
            t!("Import.targetPath"),
            target_path,
            lucide_gpui::icon!(folder_tree),
        ))
        .child(info_summary_row(
            colors,
            t!("Import.isolationStatus"),
            isolation,
            lucide_gpui::icon!(hard_drive),
        ))
        .child(info_summary_row(
            colors,
            t!("Import.versionType"),
            version_type,
            lucide_gpui::icon!(boxes),
        ))
        .into_any_element()
}

fn info_summary_row(
    colors: &ThemeColors,
    label: SharedString,
    value: SharedString,
    icon_path: &'static str,
) -> AnyElement {
    div()
        .flex()
        .gap(px(10.))
        .items_start()
        .child(icon(icon_path, 14.0, colors.text_muted))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(colors.text_muted)
                        .child(label),
                )
                .child(
                    div()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_size(px(11.))
                        .line_height(relative(1.35))
                        .text_color(colors.text_secondary)
                        .child(value),
                ),
        )
        .into_any_element()
}

fn selected_version_index(
    view: &ImportWindowView,
    versions: &crate::ui::hooks::use_local_versions::LocalVersionsSnapshot,
) -> Option<usize> {
    view.selected_folder.as_ref().and_then(|selected_folder| {
        versions.versions.iter().position(|version| {
            version_folder_matches(version.folder.as_ref(), selected_folder.as_ref())
        })
    })
}

fn selected_launch_version<'a>(
    view: &ImportWindowView,
    versions: &'a crate::ui::hooks::use_local_versions::LocalVersionsSnapshot,
) -> Option<&'a crate::core::version::launch_versions::LaunchVersionEntry> {
    if view.target.lock_version {
        let selected_folder = view.selected_folder.as_ref()?;
        return versions.versions.iter().find(|version| {
            version_folder_matches(version.folder.as_ref(), selected_folder.as_ref())
        });
    }

    versions
        .versions
        .get(selected_version_index(view, versions)?)
        .or_else(|| versions.versions.first())
}

fn launch_version_descriptor(
    version: &crate::core::version::launch_versions::LaunchVersionEntry,
) -> LaunchVersionDescriptor {
    LaunchVersionDescriptor {
        folder: SharedString::from(version.folder.clone()),
        name: SharedString::from(version.name.clone()),
        version: SharedString::from(version.version.clone()),
        kind: SharedString::from(version.kind.clone()),
        path: SharedString::from(version.path.clone()),
        launch_args: None,
    }
}

fn version_build_type(
    version: &crate::core::version::launch_versions::LaunchVersionEntry,
) -> BuildType {
    crate::ui::hooks::use_local_versions::version_build_type(version)
}

fn version_edition(version: &crate::core::version::launch_versions::LaunchVersionEntry) -> Edition {
    crate::ui::hooks::use_local_versions::version_edition(version)
}

fn version_enable_isolation(
    version: &crate::core::version::launch_versions::LaunchVersionEntry,
) -> bool {
    crate::ui::hooks::use_local_versions::version_enable_isolation(version)
}

fn version_target_root_path(
    version: &crate::core::version::launch_versions::LaunchVersionEntry,
) -> Option<SharedString> {
    crate::ui::hooks::use_local_versions::version_target_root_path(version)
}

fn version_isolation_label(
    version: &crate::core::version::launch_versions::LaunchVersionEntry,
    i18n: &I18n,
) -> SharedString {
    crate::ui::hooks::use_local_versions::version_isolation_label(version, i18n)
}

fn version_type_summary_label(
    version: &crate::core::version::launch_versions::LaunchVersionEntry,
    i18n: &I18n,
) -> SharedString {
    crate::ui::hooks::use_local_versions::version_type_summary_label(version, i18n)
}

fn version_dropdown_label(
    version: &crate::core::version::launch_versions::LaunchVersionEntry,
    i18n: &I18n,
) -> SharedString {
    let name = version_primary_label(version);
    let game_version = version_detail_label(version);
    let edition = crate::ui::hooks::use_local_versions::launch_version_display_type(version, i18n);
    SharedString::from(format!("{name} · {game_version} ({edition})"))
}

fn version_primary_label(
    version: &crate::core::version::launch_versions::LaunchVersionEntry,
) -> SharedString {
    crate::ui::hooks::use_local_versions::version_primary_label(version)
}

fn version_detail_label(
    version: &crate::core::version::launch_versions::LaunchVersionEntry,
) -> SharedString {
    crate::ui::hooks::use_local_versions::version_detail_label(version)
}

fn render_status_box(this: &ImportWindowView, colors: &ThemeColors) -> Option<AnyElement> {
    let (kind, message) = this.status.as_ref()?;
    let tone = match kind {
        StatusKind::Error => colors.danger,
        StatusKind::Success => colors.stat_green_text,
    };
    let icon_path = match kind {
        StatusKind::Error => lucide_gpui::icon!(circle_alert),
        StatusKind::Success => lucide_gpui::icon!(circle_check_big),
    };
    Some(
        div()
            .rounded(px(crate::ui::theme::tokens::radius::SM))
            .px(px(12.))
            .py(px(10.))
            .bg(Hsla { a: 0.10, ..tone })
            .border_1()
            .border_color(Hsla { a: 0.16, ..tone })
            .flex()
            .gap(px(10.))
            .items_center()
            .child(icon(icon_path, 18.0, tone))
            .child(
                div()
                    .flex_1()
                    .text_size(px(13.))
                    .line_height(relative(1.4))
                    .text_color(tone)
                    .child(message.clone()),
            )
            .into_any_element(),
    )
}

fn render_footer(
    this: &ImportWindowView,
    colors: &ThemeColors,
    import_label: SharedString,
    import_and_launch_label: SharedString,
    disabled_by_context: bool,
    cx: &mut Context<ImportWindowView>,
) -> AnyElement {
    let disabled = this.is_importing || disabled_by_context;
    let is_overlay = this.presentation == ImportPresentation::Overlay;
    div()
        .w_full()
        .px(px(if is_overlay { 16.0 } else { 20.0 }))
        .py(px(if is_overlay { 12.0 } else { 14.0 }))
        .bg(Hsla {
            a: 0.94,
            ..colors.settings_panel_bg
        })
        .border_t_1()
        .border_color(Hsla {
            a: 0.10,
            ..colors.border
        })
        .flex()
        .items_center()
        .justify_end()
        .gap(px(12.))
        .child(
            secondary_button(colors, import_label, disabled)
                .w(px(if is_overlay { 156.0 } else { 180.0 }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.import_now(false, false, false, cx);
                    }),
                ),
        )
        .child(
            primary_button(colors, import_and_launch_label, disabled)
                .w(px(if is_overlay { 188.0 } else { 220.0 }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.import_now(false, false, true, cx);
                    }),
                ),
        )
        .into_any_element()
}

fn render_conflict_dialog(
    view: &ImportWindowView,
    conflict: &ImportCheckResult,
    colors: &ThemeColors,
    cx: &mut Context<ImportWindowView>,
) -> AnyElement {
    let i18n = cx.global::<I18n>();
    let conflict_type = conflict.conflict_type.clone();
    let existing_preview = conflict.existing_pack_info.as_ref();
    let incoming_preview = view.preview.as_ref();
    let is_shared_fallback = conflict_type.as_deref() == Some("shared_fallback");
    let mixed_batch_conflict =
        view.conflict_requires_overwrite && view.conflict_requires_shared_fallback;
    let primary_label = if mixed_batch_conflict {
        t!("Import.conflict.confirmBatch")
    } else if view.conflict_requires_shared_fallback {
        t!("Import.conflict.importToShared")
    } else {
        t!("Import.conflict.overwriteImport")
    };
    div()
        .absolute()
        .inset_0()
        .bg(Hsla {
            a: 0.96,
            ..colors.bg
        })
        .child(background_layer(colors))
        .child(
            div()
                .size_full()
                .px(px(24.))
                .py(px(28.))
                .flex()
                .justify_center()
                .child(
                    div()
                        .w_full()
                        .max_w(px(760.))
                        .flex()
                        .flex_col()
                        .gap(px(18.))
                        .child({
                            let mut panel = overlay_panel(colors)
                                .p(px(24.))
                                .flex()
                                .flex_col()
                                .gap(px(18.))
                                .child(
                                    div()
                                        .flex()
                                        .items_start()
                                        .justify_between()
                                        .gap(px(16.))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(12.))
                                                .child(icon(
                                                    lucide_gpui::icon!(triangle_alert),
                                                    24.0,
                                                    colors.danger,
                                                ))
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_col()
                                                        .gap(px(6.))
                                                        .child(
                                                            div()
                                                                .text_size(px(19.))
                                                                .font_weight(FontWeight::BOLD)
                                                                .text_color(colors.text_primary)
                                                                .child(t!("Import.conflict.title")),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_size(px(12.))
                                                                .font_weight(FontWeight::MEDIUM)
                                                                .text_color(colors.text_muted)
                                                                .child(if view.conflict_count > 1 {
                                                                    t!(
                                                                        "Import.conflict.batchCount",
                                                                        count = &view.conflict_count.to_string()
                                                                    )
                                                                } else {
                                                                    SharedString::from(
                                                                        conflict.target_name.clone(),
                                                                    )
                                                                }),
                                                        ),
                                                ),
                                        )
                                        .child(meta_chip(
                                            colors,
                                            if is_shared_fallback {
                                                t!("Import.conflict.shared")
                                            } else {
                                                t!("Import.conflict.overwriteImport")
                                            },
                                            !is_shared_fallback,
                                        )),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .line_height(relative(1.45))
                                        .text_color(colors.text_secondary)
                                        .child(SharedString::from(conflict.message.clone())),
                                );

                            if is_shared_fallback {
                                panel = panel.child(
                                    div()
                                        .rounded(px(crate::ui::theme::tokens::radius::MD))
                                        .bg(Hsla {
                                            a: 0.42,
                                            ..colors.surface
                                        })
                                        .border_1()
                                        .border_color(Hsla {
                                            a: 0.08,
                                            ..colors.border
                                        })
                                        .p(px(16.))
                                        .flex()
                                        .gap(px(12.))
                                        .items_start()
                                        .child(icon(
                                            lucide_gpui::icon!(folder_git_2),
                                            18.0,
                                            colors.accent,
                                        ))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w(px(0.))
                                                .flex()
                                                .flex_col()
                                                .gap(px(6.))
                                                .child(
                                                    div()
                                                        .text_size(px(13.))
                                                        .font_weight(FontWeight::BOLD)
                                                        .text_color(colors.text_primary)
                                                        .child(t!(
                                                            "Import.conflict.importToShared"
                                                        )),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(12.))
                                                        .line_height(relative(1.45))
                                                        .text_color(colors.text_secondary)
                                                        .child(t!(
                                                            "Import.conflict.overwriteWarning"
                                                        )),
                                                ),
                                        ),
                                );
                            } else {
                                if view.conflict_count <= 1 {
                                    panel = panel.child(render_conflict_compare_panel(
                                        colors,
                                        existing_preview,
                                        incoming_preview,
                                        view,
                                        cx,
                                    ));
                                } else {
                                    panel = panel.child(
                                        div()
                                            .rounded(px(crate::ui::theme::tokens::radius::SM))
                                            .px(px(12.))
                                            .py(px(10.))
                                            .bg(Hsla {
                                                a: 0.08,
                                                ..colors.danger
                                            })
                                            .text_size(px(12.))
                                            .text_color(colors.text_secondary)
                                            .child(t!(
                                                "Import.conflict.batchSummary",
                                                count = &view.conflict_count.to_string()
                                            )),
                                    );
                                }
                                panel = panel.child(
                                        div()
                                            .text_size(px(12.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_center()
                                            .text_color(colors.danger)
                                            .child(t!("Import.conflict.overwriteWarning")),
                                    );
                            }

                            panel.child(
                                div()
                                    .flex()
                                    .gap(px(12.))
                                    .child(
                                        secondary_button(&colors, t!("common.cancel"), false)
                                            .on_mouse_up(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    this.show_conflict_dialog = false;
                                                    cx.notify();
                                                }),
                                            ),
                                    )
                                    .child(
                                        primary_button(&colors, primary_label, false).on_mouse_up(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                let overwrite = this.conflict_requires_overwrite;
                                                let allow_shared_fallback =
                                                    this.conflict_requires_shared_fallback;
                                                let launch_after_import = this.launch_after_import;
                                                this.import_now(
                                                    overwrite,
                                                    allow_shared_fallback,
                                                    launch_after_import,
                                                    cx,
                                                );
                                            }),
                                        ),
                                    ),
                            )
                        }),
                ),
        )
        .into_any_element()
}

fn render_conflict_compare_panel(
    colors: &ThemeColors,
    existing_preview: Option<&PackagePreview>,
    incoming_preview: Option<&PackagePreview>,
    view: &ImportWindowView,
    cx: &App,
) -> AnyElement {
    div()
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .bg(Hsla {
            a: 0.42,
            ..colors.surface
        })
        .border_1()
        .border_color(Hsla {
            a: 0.08,
            ..colors.border
        })
        .p(px(14.))
        .flex()
        .gap(px(12.))
        .child(render_conflict_pack_card(
            colors,
            t!("Import.conflict.current"),
            existing_preview,
            false,
            view,
            cx,
        ))
        .child(
            div()
                .w(px(40.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w(px(34.))
                        .h(px(34.))
                        .rounded_full()
                        .bg(Hsla {
                            a: 0.65,
                            ..colors.settings_field_bg
                        })
                        .border_1()
                        .border_color(Hsla {
                            a: 0.08,
                            ..colors.border
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(
                            lucide_gpui::icon!(arrow_right),
                            16.0,
                            colors.text_muted,
                        )),
                ),
        )
        .child(render_conflict_pack_card(
            colors,
            t!("Import.conflict.new"),
            incoming_preview,
            true,
            view,
            cx,
        ))
        .into_any_element()
}

fn render_conflict_pack_card(
    colors: &ThemeColors,
    title: SharedString,
    preview: Option<&PackagePreview>,
    emphasize: bool,
    view: &ImportWindowView,
    cx: &App,
) -> AnyElement {
    let version_text = preview
        .and_then(|preview| preview.version.as_ref())
        .map(|version| SharedString::from(format!("v{version}")))
        .unwrap_or_else(|| ImportWindowView::kind_label("Import.unknown", cx));
    let package_name = preview
        .map(|preview| SharedString::from(preview.name.clone()))
        .unwrap_or_else(|| ImportWindowView::kind_label("Import.unknown", cx));
    let description = preview
        .map(|preview| {
            if preview.description.is_empty() {
                t!("Import.noDescription")
            } else {
                SharedString::from(preview.description.clone())
            }
        })
        .unwrap_or_else(|| t!("Import.noDescription"));
    let size_text = preview.map(|preview| format_size(preview.size));
    let card_background = if emphasize {
        Hsla {
            a: 0.64,
            ..colors.settings_panel_bg
        }
    } else {
        Hsla {
            a: 0.46,
            ..colors.settings_field_bg
        }
    };
    let title_tone = if emphasize {
        colors.accent
    } else {
        colors.text_secondary
    };

    div()
        .flex_1()
        .min_w(px(0.))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .bg(card_background)
        .border_1()
        .border_color(Hsla {
            a: if emphasize { 0.16 } else { 0.08 },
            ..if emphasize {
                colors.accent
            } else {
                colors.border
            }
        })
        .p(px(14.))
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .gap(px(10.))
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(title_tone)
                        .child(title),
                )
                .child(meta_chip(
                    colors,
                    preview
                        .map(|preview| ImportWindowView::kind_label(&preview.kind, cx))
                        .unwrap_or_else(|| ImportWindowView::kind_label("Import.unknown", cx)),
                    emphasize,
                )),
        )
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(12.))
                .child(preview_icon(
                    preview.and_then(|preview| preview.icon.as_ref()),
                    52.0,
                    colors,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(
                            MinecraftFormattedText::new(package_name, colors)
                                .text_size(px(14.))
                                .line_height(relative(1.25))
                                .color(colors.text_primary),
                        )
                        .child(
                            MinecraftFormattedText::new(description, colors)
                                .text_size(px(12.))
                                .line_height(relative(1.45))
                                .color(colors.text_secondary),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(6.))
                                .flex_wrap()
                                .child(meta_chip(colors, version_text, false))
                                .when_some(size_text, |this, size_text| {
                                    this.child(meta_chip(
                                        colors,
                                        SharedString::from(size_text),
                                        false,
                                    ))
                                }),
                        ),
                ),
        )
        .into_any_element()
}

fn primary_button(colors: &ThemeColors, label: SharedString, disabled: bool) -> Div {
    div()
        .h(px(46.))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(14.))
        .font_weight(FontWeight::BOLD)
        .text_color(colors.btn_primary_text)
        .bg(colors.accent)
        .border_1()
        .border_color(Hsla {
            a: 0.0,
            ..colors.accent
        })
        .when(disabled, |this| this.opacity(0.45))
        .when(!disabled, |this| {
            this.cursor_pointer()
                .hover(|style| style.bg(colors.accent_hover))
        })
        .child(label)
}

fn secondary_button(colors: &ThemeColors, label: SharedString, disabled: bool) -> Div {
    div()
        .h(px(46.))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(14.))
        .font_weight(FontWeight::BOLD)
        .text_color(colors.text_primary)
        .bg(Hsla {
            a: 0.72,
            ..colors.surface
        })
        .border_1()
        .border_color(Hsla {
            a: 0.10,
            ..colors.border
        })
        .when(disabled, |this| this.opacity(0.45))
        .when(!disabled, |this| {
            this.cursor_pointer()
                .hover(|style| style.bg(colors.surface_hover))
        })
        .child(label)
}

fn meta_pill(colors: &ThemeColors, label: SharedString, accent: bool) -> AnyElement {
    let background = if accent {
        Hsla {
            a: 0.12,
            ..colors.accent
        }
    } else {
        Hsla {
            a: 0.52,
            ..colors.surface
        }
    };
    let foreground = if accent {
        colors.accent
    } else {
        colors.text_secondary
    };
    div()
        .px(px(7.))
        .py(px(4.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(background)
        .text_size(px(10.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(foreground)
        .child(label)
        .into_any_element()
}

fn meta_chip(colors: &ThemeColors, label: SharedString, accent: bool) -> AnyElement {
    meta_pill(colors, label, accent)
}

fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut value = bytes as f64;
    let mut unit_index = 0;
    while value >= 1024.0 && unit_index < UNITS.len() - 1 {
        value /= 1024.0;
        unit_index += 1;
    }
    format!("{value:.2} {}", UNITS[unit_index])
}

#[cfg(test)]
mod batch_path_tests {
    use super::extend_unique_import_paths;
    use std::path::PathBuf;

    #[test]
    fn import_batch_keeps_every_unique_path_across_split_drops() {
        let mut paths = Vec::new();
        assert!(extend_unique_import_paths(
            &mut paths,
            [
                PathBuf::from("a.mcpack"),
                PathBuf::from("b.mcpack"),
                PathBuf::from("a.mcpack"),
            ],
        ));
        assert!(extend_unique_import_paths(
            &mut paths,
            [PathBuf::from("c.mcpack"), PathBuf::from("b.mcpack")],
        ));

        assert_eq!(
            paths,
            vec![
                PathBuf::from("a.mcpack"),
                PathBuf::from("b.mcpack"),
                PathBuf::from("c.mcpack"),
            ]
        );
    }
}
