use super::*;
use crate::ui::animation::request_animation_frame_if_active;
use crate::ui::state::music::{MusicDragTarget, MusicState};
use crate::ui::state::navigation::NavState;
use tokio::sync::{Semaphore, mpsc};
use tracing::instrument;
#[cfg(windows)]
use windows::Win32::{
    System::Threading::GetCurrentProcessId,
    UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId},
};

const DISABLE_CHROME_MUSIC_REFRESH_TASK: bool = false;
const DISABLE_CHROME_COVER_DECODE_TASK: bool = false;
const DISABLE_CHROME_UI_STALL_WATCH_TASK: bool = true;

/// 封面解码并发限制：只允许 1 个封面解码任务，避免狂点下一首时 CPU 占满
static COVER_DECODE_LIMITER: Semaphore = Semaphore::const_new(1);
const COVER_DECODE_TIMEOUT: Duration = Duration::from_secs(8);
const MUSIC_REFRESH_ACTIVE_INTERVAL: Duration = Duration::from_millis(500);
const MUSIC_REFRESH_ACTIVE_INTERVAL_NON_HOME: Duration = Duration::from_millis(750);
const MUSIC_REFRESH_IDLE_INTERVAL: Duration = Duration::from_millis(750);
const MUSIC_REFRESH_IDLE_INTERVAL_NON_HOME: Duration = Duration::from_secs(2);
const MUSIC_REFRESH_BACKGROUND_INTERVAL: Duration = Duration::from_secs(10);
const MUSIC_REFRESH_EMPTY_INTERVAL: Duration = Duration::from_secs(3);
const UI_STALL_WATCH_INTERVAL: Duration = Duration::from_millis(250);

type MusicRenderSignature = (
    (
        bool,
        SharedString,
        SharedString,
        u64,
        u64,
        bool,
        bool,
        bool,
        crate::music::MusicPlaybackMode,
        bool,
        Option<MusicDragTarget>,
    ),
    (u16, u16),
);

type ChromeRenderSignature = (bool, bool);
type ChromeAnimationFlags = (bool, bool, bool, bool);
type NavRenderSignature = (usize, Option<usize>, usize, usize, bool);
type PluginNavigationPages = Vec<crate::plugins::runtime::PluginPage>;
type PluginNavigationSignature = Vec<PluginNavigationSignatureEntry>;

#[derive(Clone, Debug, Eq, PartialEq)]
struct PluginNavigationSignatureEntry {
    plugin_id: String,
    page_id: String,
    title: SharedString,
    navigation_label: Option<String>,
    navigation_order: Option<i32>,
    icon_path: Option<std::path::PathBuf>,
}

fn ratio_bucket(value: f32) -> u16 {
    (value.clamp(0.0, 1.0) * 1000.0).round() as u16
}

fn build_music_render_signature(cx: &App) -> MusicRenderSignature {
    let route = crate::ui::navigation::current_route(cx);
    let now = Instant::now();

    cx.read_global(|music: &MusicState, _cx| {
        let include_live_progress = route == crate::ui::navigation::AppRoute::Home
            || music.snapshot.expanded
            || music.drag_target().is_some();

        let progress_bucket = if include_live_progress {
            ratio_bucket(music.displayed_progress_ratio())
        } else {
            0
        };

        let volume_bucket = if include_live_progress {
            ratio_bucket(music.displayed_volume_ratio())
        } else {
            ratio_bucket(music.snapshot.volume)
        };

        (
            (
                music.snapshot.available,
                music.snapshot.title.clone(),
                music.snapshot.artist.clone(),
                music.snapshot.generation,
                music.snapshot.cover_generation,
                music.snapshot.expanded,
                music.snapshot.is_playing,
                music.snapshot.muted,
                music.snapshot.mode,
                music.popup_animating(now),
                music.drag_target(),
            ),
            (progress_bucket, volume_bucket),
        )
    })
}

fn build_chrome_render_signature(cx: &App) -> ChromeRenderSignature {
    let now = Instant::now();
    cx.read_global(
        |topbar: &crate::ui::main_window::chrome::AppChromeState, _cx| {
            (
                topbar.music_inline_target_expanded(),
                topbar.music_inline_animating(now),
            )
        },
    )
}

fn build_nav_render_signature(cx: &App) -> NavRenderSignature {
    cx.read_global(|nav: &NavState, _cx| {
        (
            nav.active_index,
            nav.pending_route_index,
            nav.pill_from_index,
            nav.pill_to_index,
            nav.labels_target_visible,
        )
    })
}

fn build_plugin_navigation_signature(
    pages: &[crate::plugins::runtime::PluginPage],
) -> PluginNavigationSignature {
    pages
        .iter()
        .map(|page| PluginNavigationSignatureEntry {
            plugin_id: page.plugin_id.clone(),
            page_id: page.page_id.clone(),
            title: page.title.clone(),
            navigation_label: page
                .navigation
                .as_ref()
                .map(|navigation| navigation.label.clone()),
            navigation_order: page.navigation.as_ref().map(|navigation| navigation.order),
            icon_path: page.icon_path.clone(),
        })
        .collect()
}

#[cfg(windows)]
fn is_main_window_foreground() -> bool {
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.0.is_null() {
            return false;
        }
        let mut foreground_process_id = 0;
        GetWindowThreadProcessId(foreground, Some(&mut foreground_process_id));
        foreground_process_id == GetCurrentProcessId()
    }
}

#[cfg(not(windows))]
fn is_main_window_foreground() -> bool {
    true
}

pub(super) struct AppChromeView {
    _subscriptions: Vec<Subscription>,
    _music_refresh_task: Option<Task<()>>,
    _cover_decode_task: Option<Task<()>>,
    _ui_stall_watch_task: Option<Task<()>>,
    last_window_width_px: f32,
    last_window_width_bucket: Option<i32>,
    last_music_available: bool,
    last_update_available: bool,
    last_music_render_signature: MusicRenderSignature,
    last_chrome_render_signature: ChromeRenderSignature,
    last_nav_render_signature: NavRenderSignature,
    last_route_target: crate::ui::navigation::RouteTarget,
    last_glass_effect_enabled: bool,
    plugin_navigation_pages: PluginNavigationPages,
    plugin_navigation_signature: PluginNavigationSignature,
    last_animation_flags: Option<ChromeAnimationFlags>,
}

impl AppChromeView {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let initial_music_render_signature = build_music_render_signature(cx);
        let initial_chrome_render_signature = build_chrome_render_signature(cx);
        let initial_nav_render_signature = build_nav_render_signature(cx);
        let initial_route_target = crate::ui::navigation::current_route_target(cx);
        let initial_glass_effect_enabled = cx
            .global::<crate::ui::views::settings::state::SettingsPageState>()
            .glass_effect_enabled;
        let initial_plugin_navigation_pages = crate::plugins::runtime::navigation_pages(cx);
        let initial_plugin_navigation_signature =
            build_plugin_navigation_signature(&initial_plugin_navigation_pages);
        let mut subscriptions = Vec::new();
        subscriptions.push(cx.observe_global::<MusicState>(|this, cx| {
            let music_available = cx.global::<MusicState>().snapshot.available;
            if this.last_music_available != music_available {
                this.sync_layout_targets(this.last_window_width_px, Instant::now(), cx);
            }

            let signature = build_music_render_signature(cx);
            if this.last_music_render_signature != signature {
                this.last_music_render_signature = signature;
                cx.notify();
            }
        }));
        subscriptions.push(
            cx.observe_global::<crate::ui::main_window::chrome::AppChromeState>(|this, cx| {
                let signature = build_chrome_render_signature(cx);
                if this.last_chrome_render_signature != signature {
                    this.last_chrome_render_signature = signature;
                    cx.notify();
                }
            }),
        );
        subscriptions.push(cx.observe_global::<gpui_router::RouterState>(|this, cx| {
            let route = crate::ui::navigation::current_route_target(cx);
            let route_changed = this.last_route_target != route;
            let layout_changed =
                this.sync_layout_targets(this.last_window_width_px, Instant::now(), cx);
            if route_changed {
                this.last_route_target = route;
            }
            if route_changed || layout_changed {
                cx.notify();
            }
        }));
        subscriptions.push(cx.observe_global::<NavState>(|this, cx| {
            let layout_changed =
                this.sync_layout_targets(this.last_window_width_px, Instant::now(), cx);
            let signature = build_nav_render_signature(cx);
            let nav_changed = this.last_nav_render_signature != signature;
            if nav_changed {
                this.last_nav_render_signature = signature;
            }
            if layout_changed || nav_changed {
                cx.notify();
            }
        }));
        subscriptions.push(cx.observe_global::<ThemeState>(|_, cx| {
            cx.notify();
        }));
        subscriptions.push(cx.observe_global::<UpdateState>(|this, cx| {
            let update_available = cx.global::<UpdateState>().available.is_some();
            if this.last_update_available != update_available {
                this.sync_layout_targets(this.last_window_width_px, Instant::now(), cx);
                cx.notify();
            }
        }));
        subscriptions.push(
            cx.observe_global::<crate::ui::views::settings::state::SettingsPageState>(
                |this, cx| {
                    let glass_effect_enabled = cx
                        .global::<crate::ui::views::settings::state::SettingsPageState>()
                        .glass_effect_enabled;
                    if this.last_glass_effect_enabled != glass_effect_enabled {
                        this.last_glass_effect_enabled = glass_effect_enabled;
                        cx.notify();
                    }
                },
            ),
        );
        subscriptions.push(
            cx.observe_global::<crate::plugins::runtime::PluginRegistry>(|this, cx| {
                let pages = crate::plugins::runtime::navigation_pages(cx);
                let signature = build_plugin_navigation_signature(&pages);
                if this.plugin_navigation_signature != signature {
                    this.plugin_navigation_signature = signature;
                    this.plugin_navigation_pages = pages;
                    cx.notify();
                }
            }),
        );
        subscriptions.push(cx.observe_window_bounds(window, |this, window, cx| {
            let window_width_px = window.bounds().size.width / px(1.0);
            if this.sync_layout_targets(window_width_px, Instant::now(), cx) {
                cx.notify();
            }
        }));
        // 创建封面解码请求 channel (使用 mpsc 保证可靠传递)
        let (cover_req_tx, mut cover_req_rx) =
            mpsc::unbounded_channel::<crate::music::CoverDecodeRequest>();

        let _cover_decode_task = if DISABLE_CHROME_COVER_DECODE_TASK {
            None
        } else {
            Some(cx.spawn(async move |_handle, cx| {
                tracing::info!("cover_decode_worker: starting");
                while let Some(request) = cover_req_rx.recv().await {
                    tracing::info!(
                        "cover_decode_worker: received request for {:?}",
                        request.track_path
                    );

                    // 获取解码许可（只允许 1 个并发）
                    let _permit = COVER_DECODE_LIMITER.acquire().await.ok();
                    let request_clone = request.clone();

                    let decoded_cover = match tokio::time::timeout(
                        COVER_DECODE_TIMEOUT,
                        tokio::task::spawn_blocking(move || {
                            crate::music::MusicController::decode_cover_thumbnail(&request_clone)
                        }),
                    )
                    .await
                    {
                        Ok(Ok(result)) => {
                            if result.is_some() {
                                tracing::info!(
                                    "cover_decode_worker: decoded successfully for {:?}",
                                    request.track_path
                                );
                            } else {
                                tracing::warn!(
                                    "cover_decode_worker: decode returned None for {:?}",
                                    request.track_path
                                );
                            }
                            result
                        }
                        Ok(Err(error)) => {
                            tracing::warn!(
                                "cover_decode_worker: decode task join failed for {:?}: {error}",
                                request.track_path
                            );
                            None
                        }
                        Err(_) => {
                            tracing::warn!(
                                "cover_decode_worker: decode timed out for {:?} after {:?}",
                                request.track_path,
                                COVER_DECODE_TIMEOUT
                            );
                            None
                        }
                    };

                    // 应用结果（代际校验会自动丢弃旧结果）
                    match cx.update_global(|music: &mut MusicState, _cx| {
                        music.apply_decoded_cover_if_current(
                            &request,
                            decoded_cover,
                            Instant::now(),
                        );
                    }) {
                        Ok(()) => {
                            tracing::debug!(
                                generation = request.generation,
                                "cover_decode_worker: applied cover result"
                            );
                        }
                        Err(error) => {
                            tracing::warn!(
                                generation = request.generation,
                                "cover_decode_worker: failed to apply cover result: {error:?}"
                            );
                        }
                    }
                }
                tracing::warn!("cover_decode_worker: channel closed, exiting");
            }))
        };

        let music_refresh_task = if DISABLE_CHROME_MUSIC_REFRESH_TASK {
            None
        } else {
            Some(cx.spawn(async move |_handle, cx| {
                let mut last_generation = cx
                    .read_global(|music: &MusicState, _cx| music.snapshot.generation)
                    .unwrap_or(0);
                let mut last_cover_generation = cx
                    .read_global(|music: &MusicState, _cx| music.snapshot.cover_generation)
                    .unwrap_or(0);
                let mut last_requested_cover_token: Option<(u64, Option<u64>)> = None;

                loop {
                    let route = cx
                        .read_global(|router: &gpui_router::RouterState, _cx| {
                            crate::ui::navigation::AppRoute::from_pathname(
                                &router.location.pathname,
                            )
                        })
                        .unwrap_or(crate::ui::navigation::AppRoute::Home);
                    let window_foreground = is_main_window_foreground();
                    let poll_interval = cx
                        .read_global(|music: &MusicState, _cx| {
                            let active_interval = if route == crate::ui::navigation::AppRoute::Home
                            {
                                MUSIC_REFRESH_ACTIVE_INTERVAL
                            } else {
                                MUSIC_REFRESH_ACTIVE_INTERVAL_NON_HOME
                            };
                            let idle_interval = if route == crate::ui::navigation::AppRoute::Home {
                                MUSIC_REFRESH_IDLE_INTERVAL
                            } else {
                                MUSIC_REFRESH_IDLE_INTERVAL_NON_HOME
                            };
                            let background_quiet = !window_foreground
                                && route != crate::ui::navigation::AppRoute::Home
                                && !music.snapshot.expanded
                                && music.drag_target().is_none();

                            if music.snapshot.is_playing || music.drag_target().is_some() {
                                active_interval
                            } else if background_quiet {
                                MUSIC_REFRESH_BACKGROUND_INTERVAL
                            } else if music.snapshot.available {
                                idle_interval
                            } else {
                                MUSIC_REFRESH_EMPTY_INTERVAL
                            }
                        })
                        .unwrap_or(MUSIC_REFRESH_EMPTY_INTERVAL);

                    Timer::after(poll_interval).await;

                    let snapshot = cx.read_global(|music: &MusicState, _cx| {
                        (
                            music.snapshot.generation,
                            music.snapshot.cover_generation,
                            music.snapshot.is_playing,
                            music.snapshot.expanded,
                            music.drag_target().is_some(),
                        )
                    });

                    let Ok((
                        current_generation,
                        current_cover_generation,
                        is_playing,
                        is_expanded,
                        is_dragging,
                    )) = snapshot
                    else {
                        continue;
                    };

                    // 切歌或封面变化时触发渲染
                    let song_changed = current_generation != last_generation;
                    let cover_changed = current_cover_generation != last_cover_generation;

                    if song_changed {
                        tracing::info!(
                            "music_refresh: song_changed, generation={}",
                            current_generation
                        );
                        // 切歌首刷走快路径，避免同步封面解码
                        // 注意：由于 Topbar 已订阅 MusicState，不需要手动触发重绘
                        last_generation = current_generation;
                        if let Err(error) = cx.update_global(|music: &mut MusicState, cx| {
                            music.refresh_no_cover(Instant::now(), cx);
                        }) {
                            tracing::warn!(
                                generation = current_generation,
                                "music_refresh: failed to refresh after song change: {error:?}"
                            );
                        }
                    } else if cover_changed {
                        // 封面变化只更新计数，不触发额外重绘
                        // 因为 apply_decoded_cover_if_current 会触发重绘
                        last_cover_generation = current_cover_generation;
                        last_requested_cover_token = None;
                    } else if is_playing || is_dragging {
                        if !window_foreground
                            && route != crate::ui::navigation::AppRoute::Home
                            && !is_expanded
                            && !is_dragging
                        {
                            last_requested_cover_token = None;
                            continue;
                        }
                        // Foreground playback keeps the progress UI responsive.
                        // Background/non-home playback is intentionally decimated
                        // by the poll interval chosen above to avoid competing
                        // with normal rendering and image work.
                        if let Err(error) = cx.update_global(|music: &mut MusicState, cx| {
                            music.refresh_no_cover(Instant::now(), cx);
                        }) {
                            tracing::warn!(
                                generation = current_generation,
                                "music_refresh: failed to refresh playback progress: {error:?}"
                            );
                        }
                    }

                    if !window_foreground
                        && route != crate::ui::navigation::AppRoute::Home
                        && !is_expanded
                        && !is_dragging
                    {
                        last_requested_cover_token = None;
                        continue;
                    }

                    let pending_cover_request = cx
                        .read_global(|music: &MusicState, _cx| {
                            if music.snapshot.cover_render_image.is_some() {
                                return None;
                            }

                            music.current_cover_request().map(|request| {
                                ((request.generation, request.cover_cache_key), request)
                            })
                        })
                        .ok()
                        .flatten();

                    if let Some((request_token, request)) = pending_cover_request {
                        if last_requested_cover_token != Some(request_token) {
                            tracing::info!(
                                "music_refresh: sending cover decode request for {:?}",
                                request.track_path
                            );
                            if cover_req_tx.send(request).is_ok() {
                                last_requested_cover_token = Some(request_token);
                            } else {
                                tracing::warn!("music_refresh: cover decode worker unavailable");
                            }
                        }
                    } else {
                        last_requested_cover_token = None;
                    }
                }
            }))
        };

        let ui_stall_watch_task = if DISABLE_CHROME_UI_STALL_WATCH_TASK {
            None
        } else {
            Some(cx.spawn(async move |_handle, cx| {
                loop {
                    Timer::after(UI_STALL_WATCH_INTERVAL).await;

                    let _music_snapshot = cx
                        .read_global(|music: &MusicState, _cx| {
                            (
                                music.snapshot.is_playing,
                                music.snapshot.generation,
                                music.snapshot.cover_generation,
                                music.snapshot.title.clone(),
                            )
                        })
                        .unwrap_or((false, 0, 0, SharedString::from("unknown")));
                    let _download_snapshot = cx
                        .read_global(
                            |download: &crate::ui::views::download::state::DownloadPageState,
                             _cx| {
                                (
                                    download.curseforge_results_loading,
                                    download.curseforge_page_index,
                                    download.curseforge_mods.len(),
                                    download.curseforge_disable_result_logos,
                                    download.curseforge_last_query_key.clone(),
                                )
                            },
                        )
                        .unwrap_or((false, 0, 0, false, SharedString::from("unknown")));
                }
            }))
        };

        let mut this = Self {
            _subscriptions: subscriptions,
            _music_refresh_task: music_refresh_task,
            _cover_decode_task: _cover_decode_task,
            _ui_stall_watch_task: ui_stall_watch_task,
            last_window_width_px: window.bounds().size.width / px(1.0),
            last_window_width_bucket: None,
            last_music_available: false,
            last_update_available: false,
            last_music_render_signature: initial_music_render_signature,
            last_chrome_render_signature: initial_chrome_render_signature,
            last_nav_render_signature: initial_nav_render_signature,
            last_route_target: initial_route_target,
            last_glass_effect_enabled: initial_glass_effect_enabled,
            plugin_navigation_pages: initial_plugin_navigation_pages,
            plugin_navigation_signature: initial_plugin_navigation_signature,
            last_animation_flags: None,
        };
        let initial_window_width_px = this.last_window_width_px;
        this.sync_layout_targets(initial_window_width_px, Instant::now(), cx);
        this.last_nav_render_signature = build_nav_render_signature(cx);
        this.last_chrome_render_signature = build_chrome_render_signature(cx);
        this
    }

    fn sync_layout_targets(
        &mut self,
        window_width_px: f32,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> bool {
        let music_available = cx.global::<MusicState>().snapshot.available;
        let update_available = cx.global::<UpdateState>().available.is_some();
        let (
            music_inline_factor,
            music_inline_target_expanded,
            labels_layout_factor,
            labels_opacity_factor,
            labels_target_visible,
        ) = {
            let topbar: &crate::ui::main_window::chrome::AppChromeState =
                cx.global::<crate::ui::main_window::chrome::AppChromeState>();
            let nav: &NavState = cx.global::<NavState>();
            (
                topbar.music_inline_factor(now),
                topbar.music_inline_target_expanded(),
                nav.labels_layout_factor(now),
                nav.labels_opacity_factor(now),
                nav.labels_target_visible,
            )
        };

        let current_width_bucket = Self::width_bucket(window_width_px);
        let window_width_changed = self.last_window_width_bucket != Some(current_width_bucket);
        let music_available_changed = self.last_music_available != music_available;
        let update_available_changed = self.last_update_available != update_available;

        self.last_window_width_px = window_width_px;
        if !window_width_changed && !music_available_changed && !update_available_changed {
            return false;
        }

        self.last_window_width_bucket = Some(current_width_bucket);
        self.last_music_available = music_available;
        self.last_update_available = update_available;

        let width_wants_labels = Self::compute_width_wants_labels(
            window_width_px,
            music_available,
            music_inline_factor,
            update_available,
            labels_target_visible,
        );
        let can_expand_music_inline_without_overflow =
            Self::can_expand_music_inline_without_overflow(
                window_width_px,
                music_available,
                update_available,
            );
        let nav_ready_for_music_inline = width_wants_labels
            && labels_target_visible
            && labels_layout_factor >= 0.88
            && labels_opacity_factor >= 0.82;
        let music_should_expand = if !nav_ready_for_music_inline {
            false
        } else if music_inline_target_expanded {
            window_width_px >= 1200.0 && can_expand_music_inline_without_overflow
        } else {
            window_width_px >= 1220.0 && can_expand_music_inline_without_overflow
        };
        let show_labels_target = width_wants_labels;

        let mut layout_target_changed = false;
        if labels_target_visible != show_labels_target {
            layout_target_changed = true;
            cx.update_global(|nav: &mut NavState, _cx| {
                nav.set_labels_target_immediate(show_labels_target);
            });
        }
        let music_inline_target = width_wants_labels && music_should_expand;
        if music_inline_target_expanded != music_inline_target {
            layout_target_changed = true;
            cx.update_global(
                |topbar: &mut crate::ui::main_window::chrome::AppChromeState, _cx| {
                    topbar.set_music_inline_expanded(music_inline_target, now);
                },
            );
        }
        if layout_target_changed {
            self.last_nav_render_signature = build_nav_render_signature(cx);
            self.last_chrome_render_signature = build_chrome_render_signature(cx);
        }
        layout_target_changed
    }

    fn prepare_render_state(
        &self,
        now: Instant,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> TopbarRenderState {
        let theme_k = cx.global::<ThemeState>().factor(now);
        let theme_accent = cx.global::<ThemeState>().accent;

        // 移除 render 路径中的 debug 状态更新：避免自驱动重绘循环
        // DebugState 的帧统计应在 debug 面板自己的 render 中处理

        let window_width = window.bounds().size.width;

        let (
            music_snapshot,
            music_expanded_factor,
            music_progress_ratio,
            music_volume_ratio,
            music_drag_target,
            music_popup_animating,
            music_inline_factor,
            music_inline_animating,
        ) = {
            let music: &MusicState = cx.global::<MusicState>();
            let topbar: &crate::ui::main_window::chrome::AppChromeState =
                cx.global::<crate::ui::main_window::chrome::AppChromeState>();
            (
                music.snapshot.clone(),
                music.expanded_factor(now),
                music.displayed_progress_ratio(),
                music.displayed_volume_ratio(),
                music.drag_target(),
                music.popup_animating(now),
                topbar.music_inline_factor(now),
                topbar.music_inline_animating(now),
            )
        };
        let update_available = cx.global::<UpdateState>().available.is_some();
        let glass_effect_enabled = cx
            .global::<crate::ui::views::settings::state::SettingsPageState>()
            .glass_effect_enabled;
        let plugin_navigation_pages = self.plugin_navigation_pages.clone();

        let (
            visual_active_index,
            pill_steps,
            pill_direction,
            pill_leading_progress,
            pill_trailing_progress,
            labels_layout_factor,
            labels_opacity_factor,
            nav_animating,
        ) = {
            let nav: &NavState = cx.global::<NavState>();
            (
                nav.visual_active_index(),
                nav.pill_steps(now),
                nav.pill_direction(),
                nav.pill_leading_progress(now),
                nav.pill_trailing_progress(now),
                nav.labels_layout_factor(now),
                nav.labels_opacity_factor(now),
                nav.is_animating(now),
            )
        };

        let (theme_target_dark, theme_animating) = {
            let theme: &ThemeState = cx.global::<ThemeState>();
            (theme.target_dark, theme.is_animating(now))
        };

        TopbarRenderState {
            theme_k,
            theme_target_dark,
            theme_animating,
            theme_accent,
            window_width,
            music_snapshot,
            music_expanded_factor,
            music_progress_ratio,
            music_volume_ratio,
            music_drag_target,
            music_popup_animating,
            music_inline_factor,
            music_inline_animating,
            update_available,
            visual_active_index,
            pill_steps,
            pill_direction,
            pill_leading_progress,
            pill_trailing_progress,
            labels_layout_factor,
            labels_opacity_factor,
            nav_animating,
            glass_effect_enabled,
            plugin_navigation_pages,
        }
    }

    fn compute_width_wants_labels(
        window_width_px: f32,
        music_available: bool,
        music_inline_factor: f32,
        update_available: bool,
        labels_target_visible: bool,
    ) -> bool {
        let inset_x_px = (window_width_px * 0.03).clamp(16.0, 28.0);
        let nav_pad_x_px = if window_width_px <= 1000.0 {
            16.0
        } else {
            24.0
        };
        let inner_w_px = (window_width_px - inset_x_px * 2.0 - nav_pad_x_px * 2.0).max(320.0);
        let music_w_px = crate::ui::main_window::music_player::mini_capsule_width_for_factor(
            music_available,
            music_inline_factor,
        ) / px(1.0);
        let right_controls_w_px: f32 = 40.0 * 3.0
            + (1.0 + 8.0 * 2.0)
            + 8.0 * 4.0
            + if music_available {
                12.0 + music_w_px
            } else {
                0.0
            };
        let nav_side_safety_px = 50.0;
        let left_content_w_px: f32 = if update_available { 168.0 } else { 124.0 };
        let left_slot_w_px = (left_content_w_px + nav_side_safety_px).max(180.0);
        let right_slot_w_px = (right_controls_w_px + nav_side_safety_px).max(180.0);
        let expanded_item_w_px = 16.0 * 2.0 + 18.0 + 8.0 + 33.0;
        let expanded_capsule_w_px = 6.0 * 2.0 + 3.0 * 5.0 + 6.0 * expanded_item_w_px;
        let required_inner_w_px = left_slot_w_px + right_slot_w_px + expanded_capsule_w_px;
        let hysteresis_px = if labels_target_visible { -10.0 } else { 20.0 };
        window_width_px >= 1200.0 && inner_w_px >= required_inner_w_px + hysteresis_px
    }

    fn can_expand_music_inline_without_overflow(
        window_width_px: f32,
        music_available: bool,
        update_available: bool,
    ) -> bool {
        let inset_x_px = (window_width_px * 0.03).clamp(16.0, 28.0);
        let nav_pad_x_px = if window_width_px <= 1000.0 {
            16.0
        } else {
            24.0
        };
        let inner_w_px = (window_width_px - inset_x_px * 2.0 - nav_pad_x_px * 2.0).max(320.0);
        let music_w_px = crate::ui::main_window::music_player::mini_capsule_width_for_factor(
            music_available,
            1.0,
        ) / px(1.0);
        let right_controls_w_px: f32 = 40.0 * 3.0
            + (1.0 + 8.0 * 2.0)
            + 8.0 * 4.0
            + if music_available {
                12.0 + music_w_px
            } else {
                0.0
            };
        let nav_side_safety_px = 50.0;
        let left_content_w_px: f32 = if update_available { 168.0 } else { 124.0 };
        let left_slot_w_px = (left_content_w_px + nav_side_safety_px).max(180.0);
        let right_slot_w_px = (right_controls_w_px + nav_side_safety_px).max(180.0);
        let expanded_item_w_px = 16.0 * 2.0 + 18.0 + 8.0 + 33.0;
        let expanded_capsule_w_px = 6.0 * 2.0 + 3.0 * 5.0 + 6.0 * expanded_item_w_px;
        let required_inner_w_px = left_slot_w_px + right_slot_w_px + expanded_capsule_w_px;
        inner_w_px >= required_inner_w_px + 8.0
    }

    fn width_bucket(width_px: f32) -> i32 {
        (width_px / 12.0).round() as i32
    }

    fn render_with_state(
        topbar_state: TopbarRenderState,
        show_modal: bool,
        route: crate::ui::navigation::RouteTarget,
    ) -> AnyElement {
        crate::ui::main_window::chrome::render_app_chrome(
            format!("v{}", env!("CARGO_PKG_VERSION")).into(),
            topbar_state.visual_active_index,
            topbar_state.pill_steps,
            topbar_state.pill_direction,
            topbar_state.pill_leading_progress,
            topbar_state.pill_trailing_progress,
            topbar_state.labels_layout_factor,
            topbar_state.labels_opacity_factor,
            topbar_state.music_snapshot,
            topbar_state.music_expanded_factor,
            topbar_state.music_progress_ratio,
            topbar_state.music_volume_ratio,
            topbar_state.music_drag_target,
            topbar_state.music_inline_factor,
            route,
            topbar_state.window_width,
            topbar_state.theme_k,
            topbar_state.theme_target_dark,
            topbar_state.update_available,
            show_modal,
            topbar_state.theme_accent,
            topbar_state.glass_effect_enabled,
            topbar_state.plugin_navigation_pages,
        )
        .into_any_element()
    }
}

impl Render for AppChromeView {
    #[instrument(name = "AppChromeView::render", skip_all)]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let route = crate::ui::navigation::current_route_target(cx);
        let topbar_state = self.prepare_render_state(now, window, cx);

        // 只在展开/收起动画进行时请求 RAF
        // 拖拽期间不需要持续 RAF，只在 pointer move 时更新
        let nav_animating = topbar_state.nav_animating;
        let theme_animating = topbar_state.theme_animating;
        let music_inline_animating = topbar_state.music_inline_animating;
        // music_popup_animating 现在是纯展开/收起动画，不包含拖拽
        let music_popup_animating = topbar_state.music_popup_animating;
        let animation_flags = (
            nav_animating,
            theme_animating,
            music_popup_animating,
            music_inline_animating,
        );
        if self.last_animation_flags != Some(animation_flags) {
            self.last_animation_flags = Some(animation_flags);
            tracing::trace!(
                nav_animating,
                theme_animating,
                music_popup_animating,
                music_inline_animating,
                "chrome animation flags"
            );
        }

        // 只在动画进行中或动画状态变化时请求 RAF
        request_animation_frame_if_active(window, nav_animating);
        request_animation_frame_if_active(window, theme_animating);
        request_animation_frame_if_active(window, music_popup_animating);
        request_animation_frame_if_active(window, music_inline_animating);

        Self::render_with_state(topbar_state, cx.global::<UpdateState>().show_modal, route)
    }
}
