use crate::i18n::Locale;
use crate::plugins::runtime::{PluginMemoryReport, PluginMemorySnapshot};
use crate::ui::components::button::Button;
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::components::toast;
use crate::ui::state::i18n::I18n;
use crate::ui::state::theme::ThemeState;
use crate::ui::state::update::UpdateState;
use crate::ui::window::debug::devtools;
use crate::ui::window::debug::state::{DebugRuntimeSnapshot, DebugState, snapshot_runtime_metrics};
use crate::utils::file_ops;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use std::io::{Read, Seek, SeekFrom};
use std::time::{Duration, Instant};

const GLOBAL_IMAGE_ASSET_SAMPLE_INTERVAL: Duration = Duration::from_secs(5);
const DEBUG_INITIAL_REFRESH_DELAY: Duration = Duration::from_millis(500);
const DEBUG_REFRESH_INTERVAL: Duration = Duration::from_millis(1500);
const DEBUG_CONSOLE_RENDER_LINE_LIMIT: usize = 96;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DebugTab {
    Overview,
    Elements,
    Performance,
    Console,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConsoleFilter {
    All,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConsoleSource {
    LatestLog,
    StallWatch,
}

pub struct DebugView {
    _refresh_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
    log_tail: SharedString,
    log_tail_last_updated: Option<Instant>,
    log_tail_error: Option<SharedString>,
    stall_log_tail: SharedString,
    stall_log_last_updated: Option<Instant>,
    stall_log_error: Option<SharedString>,
    runtime: DebugRuntimeSnapshot,
    tab: DebugTab,
    console_filter: ConsoleFilter,
    console_source: ConsoleSource,
}

impl DebugView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let log_path = file_ops::logs_dir().join("latest.log");
        let stall_log_path = file_ops::logs_dir().join("ui_foreground_stall.log");
        record_debug_window_metrics(window);
        let subscriptions = vec![cx.observe_window_bounds(window, |_, window, _cx| {
            record_debug_window_metrics(window);
        })];
        cx.on_next_frame(window, |_, window, _cx| {
            crate::ui::window::debug::state::record_debug_gpu_specs(window.gpu_specs());
        });

        let refresh_task = cx.spawn(async move |handle, cx| {
            let mut refresh_delay = DEBUG_INITIAL_REFRESH_DELAY;
            loop {
                Timer::after(refresh_delay).await;
                refresh_delay = DEBUG_REFRESH_INTERVAL;

                let log_path = log_path.clone();
                let stall_log_path = stall_log_path.clone();
                let read_task = cx.background_spawn(async move {
                    (
                        read_text_file_tail(&log_path, 64 * 1024),
                        read_text_file_tail(&stall_log_path, 48 * 1024),
                        snapshot_runtime_metrics(),
                    )
                });

                let (latest_log, stall_log, runtime) = read_task.await;
                let _ = handle.update(cx, |this, cx| {
                    let now = Instant::now();
                    let mut runtime = runtime;
                    if this
                        .runtime
                        .gpui_global_image_assets_sampled_at
                        .is_none_or(|sampled_at| {
                            now.saturating_duration_since(sampled_at)
                                >= GLOBAL_IMAGE_ASSET_SAMPLE_INTERVAL
                        })
                    {
                        let global_image_assets = cx.global_image_asset_cache_snapshot();
                        let gpui_memory = cx.gpui_memory_snapshot();
                        runtime.gpui_image_asset_cache_entries =
                            gpui_memory.gpui_image_asset_cache_entries;
                        runtime.gpui_image_asset_retained_compressed_bytes =
                            gpui_memory.gpui_image_asset_retained_compressed_bytes;
                        runtime.gpui_image_asset_total_retained_decoded_bytes =
                            gpui_memory.gpui_image_asset_retained_decoded_bytes;
                        runtime.gpui_render_image_cpu_bytes =
                            gpui_memory.gpui_render_image_cpu_bytes;
                        runtime.gpui_render_image_gpu_texture_bytes =
                            gpui_memory.gpui_render_image_gpu_texture_bytes;
                        runtime.gpui_icon_cache_entries = gpui_memory.gpui_icon_cache_entries;
                        runtime.gpui_icon_cache_decoded_bytes =
                            gpui_memory.gpui_icon_cache_decoded_bytes;
                        runtime.gpui_atlas_monochrome_bytes =
                            gpui_memory.gpui_atlas_monochrome_bytes;
                        runtime.gpui_atlas_polychrome_bytes =
                            gpui_memory.gpui_atlas_polychrome_bytes;
                        runtime.gpui_atlas_live_keys = gpui_memory.gpui_atlas_live_keys;
                        runtime.gpui_atlas_unused_bytes = gpui_memory.gpui_atlas_unused_bytes;
                        runtime.gpui_gpu_surface_texture_bytes =
                            gpui_memory.gpui_gpu_surface_texture_bytes;
                        runtime.gpui_gpu_estimated_total_retained_bytes =
                            gpui_memory.gpui_gpu_estimated_total_retained_bytes;
                        runtime.gpui_global_image_resource_decoded_bytes =
                            global_image_assets.resource_decoded_bytes;
                        runtime.gpui_global_image_resource_count =
                            global_image_assets.resource_count;
                        runtime.gpui_global_image_inline_decoded_bytes =
                            global_image_assets.inline_decoded_bytes;
                        runtime.gpui_global_image_inline_count = global_image_assets.inline_count;
                        runtime.gpui_global_image_compressed_bytes =
                            global_image_assets.compressed_bytes;
                        runtime.gpui_global_image_compressed_count =
                            global_image_assets.compressed_count;
                        runtime.gpui_global_image_target_decoded_bytes =
                            global_image_assets.target_decoded_bytes;
                        runtime.gpui_global_image_target_count = global_image_assets.target_count;
                        runtime.gpui_global_image_assets_sampled_at = Some(now);
                    } else {
                        runtime.gpui_global_image_resource_decoded_bytes =
                            this.runtime.gpui_global_image_resource_decoded_bytes;
                        runtime.gpui_global_image_resource_count =
                            this.runtime.gpui_global_image_resource_count;
                        runtime.gpui_global_image_inline_decoded_bytes =
                            this.runtime.gpui_global_image_inline_decoded_bytes;
                        runtime.gpui_global_image_inline_count =
                            this.runtime.gpui_global_image_inline_count;
                        runtime.gpui_global_image_compressed_bytes =
                            this.runtime.gpui_global_image_compressed_bytes;
                        runtime.gpui_global_image_compressed_count =
                            this.runtime.gpui_global_image_compressed_count;
                        runtime.gpui_global_image_target_decoded_bytes =
                            this.runtime.gpui_global_image_target_decoded_bytes;
                        runtime.gpui_global_image_target_count =
                            this.runtime.gpui_global_image_target_count;
                        runtime.gpui_global_image_assets_sampled_at =
                            this.runtime.gpui_global_image_assets_sampled_at;
                    }
                    runtime.plugin_memory = cx
                        .global::<crate::plugins::runtime::PluginRegistry>()
                        .memory_report();

                    let runtime_changed = this.runtime != runtime;
                    if runtime_changed {
                        this.runtime = runtime;
                    }

                    let latest_log_changed = apply_log_tail_refresh(
                        &mut this.log_tail,
                        &mut this.log_tail_last_updated,
                        &mut this.log_tail_error,
                        latest_log,
                        now,
                        "read latest.log failed",
                    );
                    let stall_log_changed = apply_log_tail_refresh(
                        &mut this.stall_log_tail,
                        &mut this.stall_log_last_updated,
                        &mut this.stall_log_error,
                        stall_log,
                        now,
                        "read ui_foreground_stall.log failed",
                    );

                    if latest_log_changed
                        || stall_log_changed
                        || (runtime_changed && this.tab != DebugTab::Console)
                    {
                        cx.notify();
                    }
                });
            }
        });

        Self {
            _refresh_task: Some(refresh_task),
            _subscriptions: subscriptions,
            log_tail: SharedString::from(""),
            log_tail_last_updated: None,
            log_tail_error: None,
            stall_log_tail: SharedString::from(""),
            stall_log_last_updated: None,
            stall_log_error: None,
            runtime: DebugRuntimeSnapshot::default(),
            tab: DebugTab::Overview,
            console_filter: ConsoleFilter::All,
            console_source: ConsoleSource::LatestLog,
        }
    }
}

fn record_debug_window_metrics(window: &Window) {
    let width_px = window.bounds().size.width / px(1.);
    let height_px = window.bounds().size.height / px(1.);
    crate::ui::window::debug::state::record_debug_window_frame(width_px, height_px);
}

fn apply_log_tail_refresh(
    text: &mut SharedString,
    last_updated: &mut Option<Instant>,
    error: &mut Option<SharedString>,
    result: std::io::Result<String>,
    now: Instant,
    error_context: &'static str,
) -> bool {
    let mut changed = false;
    match result {
        Ok(tail) => {
            if text.as_ref() != tail {
                *text = SharedString::from(tail);
                *last_updated = Some(now);
                changed = true;
            }
            if error.is_some() {
                *error = None;
                changed = true;
            }
        }
        Err(read_error) => {
            let message = format!("{error_context}: {read_error}");
            if error
                .as_ref()
                .is_none_or(|current| current.as_ref() != message)
            {
                *error = Some(SharedString::from(message));
                changed = true;
            }
        }
    }
    changed
}

fn read_text_file_tail(path: &std::path::Path, max_bytes: u64) -> std::io::Result<String> {
    if !path.exists() {
        return Ok(String::new());
    }

    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start))?;

    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).to_string())
}

fn copy_text_to_clipboard(
    text: impl Into<String>,
    success_message: impl Into<SharedString>,
    cx: &mut App,
) {
    cx.write_to_clipboard(ClipboardItem::new_string(text.into()));
    toast::success(cx, success_message.into());
}

fn open_path_in_background(
    path: impl Into<std::path::PathBuf>,
    success_message: impl Into<SharedString>,
    cx: &mut Context<DebugView>,
) {
    let path = path.into();
    let success_message = success_message.into();
    cx.spawn(async move |_this, cx| {
        match crate::utils::open_path::open_path(path.to_string_lossy().to_string()).await {
            Ok(()) => {
                let _ = toast::push_async(cx, toast::ToastKind::Success, success_message);
            }
            Err(error) => {
                let _ = toast::push_async(cx, toast::ToastKind::Error, SharedString::from(error));
            }
        }
        Ok::<(), anyhow::Error>(())
    })
    .detach();
}

fn run_aggressive_memory_cleanup(cx: &mut Context<DebugView>) {
    cx.spawn(async move |_this, cx| {
        let cleanup =
            cx.background_spawn(
                async move { crate::utils::memory::force_memory_cleanup_aggressive() },
            );
        let stats = cleanup.await;
        let message = SharedString::from(format!(
            "强清理完成：Working Set {} / Private {}",
            bytes_to_human(stats.working_set_kb.saturating_mul(1024)),
            bytes_to_human(stats.private_kb.saturating_mul(1024))
        ));
        let _ = toast::push_async(cx, toast::ToastKind::Success, message);
        Ok::<(), anyhow::Error>(())
    })
    .detach();
}

fn bytes_to_human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut unit = 0usize;
    let mut value = bytes as f64;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

fn window_metrics_summary(
    metrics: &[crate::ui::window::debug::state::DebugWindowMetrics],
) -> String {
    if metrics.is_empty() {
        return "No active window metrics".to_string();
    }

    metrics
        .iter()
        .map(|window| {
            format!(
                "#{} redraw={} draw={} present={} skip={} skipped={} reconfig={} errors={} layout={} upload={}",
                window.window_id,
                window.request_redraw_count,
                window.draw_count,
                window.present_count,
                window.skip_count,
                window.skipped_frame_count,
                window.surface_reconfigure_count,
                window.present_error_count,
                window.layout_recompute_count,
                bytes_to_human(window.upload_bytes as u64),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn bool_label(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn plugin_memory_summary(report: &PluginMemoryReport) -> String {
    if report.plugins.is_empty() {
        return "No plugins loaded".to_string();
    }
    report
        .plugins
        .iter()
        .take(8)
        .map(plugin_memory_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn plugin_memory_line(plugin: &PluginMemorySnapshot) -> String {
    format!(
        "{} ({}) total={} wasm={} ({}/{} pages) render={} entries/{} http={} entries/{} body + {} errors logs={} entries/{} enabled={} loaded={}",
        plugin.name,
        plugin.plugin_id,
        bytes_to_human(plugin.total_estimated_bytes as u64),
        bytes_to_human(plugin.wasm_linear_bytes as u64),
        plugin.wasm_page_count,
        plugin.wasm_limit_bytes / (64 * 1024),
        plugin.render_cache_entries,
        bytes_to_human(plugin.render_cache_bytes as u64),
        plugin.http_cache_entries,
        bytes_to_human(plugin.http_cache_body_bytes as u64),
        bytes_to_human(plugin.http_cache_error_bytes as u64),
        plugin.log_entries,
        bytes_to_human(plugin.log_bytes as u64),
        bool_label(plugin.enabled),
        bool_label(plugin.loaded)
    )
}

fn memory_attribution_summary(runtime: &DebugRuntimeSnapshot) -> String {
    let mut components = vec![
        (
            "GPUI allocator reserved",
            runtime.gpui_allocator_reserved_bytes,
        ),
        ("GPUI GPU retained", runtime.gpui_gpu_retained_bytes),
        (
            "GPUI image asset retained",
            runtime.gpui_image_asset_total_retained_decoded_bytes,
        ),
        (
            "GPUI global image assets",
            runtime
                .gpui_global_image_resource_decoded_bytes
                .saturating_add(runtime.gpui_global_image_inline_decoded_bytes)
                .saturating_add(runtime.gpui_global_image_compressed_bytes)
                .saturating_add(runtime.gpui_global_image_target_decoded_bytes),
        ),
        ("GPUI atlas retained", runtime.gpui_atlas_retained_bytes),
        (
            "GPUI unified estimate",
            runtime.gpui_gpu_estimated_total_retained_bytes,
        ),
        (
            "BMCBL map viewer estimate",
            runtime.bmcbl_memory.total_estimated_bytes(),
        ),
        (
            "Plugins estimate",
            runtime.plugin_memory.total_estimated_bytes,
        ),
    ];
    components.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0)));
    components
        .into_iter()
        .map(|(label, bytes)| format!("{label}: {}", bytes_to_human(bytes as u64)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_color(a: impl Into<Hsla>, b: impl Into<Hsla>, t: f32) -> Hsla {
    let a: Hsla = a.into();
    let b: Hsla = b.into();
    Hsla {
        h: lerp_f32(a.h, b.h, t),
        s: lerp_f32(a.s, b.s, t),
        l: lerp_f32(a.l, b.l, t),
        a: lerp_f32(a.a, b.a, t),
    }
}

fn panel_card(
    card: Hsla,
    border: Hsla,
    title: impl Into<SharedString>,
    body: impl IntoElement,
) -> Div {
    let title = title.into();
    div()
        .rounded(px(12.))
        .bg(card)
        .border_1()
        .border_color(border)
        .p(px(10.))
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(body)
}

fn line(label: impl Into<SharedString>, value: impl Into<SharedString>, muted: Hsla) -> Div {
    let label = label.into();
    let value = value.into();
    div()
        .flex()
        .justify_between()
        .items_start()
        .gap(px(8.))
        .child(
            div()
                .min_w(px(92.))
                .text_size(px(10.))
                .text_color(muted)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_size(px(10.))
                .line_height(px(14.))
                .whitespace_normal()
                .child(value),
        )
}

fn metrics_list(
    entries: impl IntoIterator<Item = (SharedString, SharedString)>,
    muted: Hsla,
) -> Div {
    div().flex().flex_col().gap(px(4.)).children(
        entries
            .into_iter()
            .map(|(label, value)| line(label, value, muted).into_any_element()),
    )
}

fn metrics_panel(
    card: Hsla,
    border: Hsla,
    title: impl Into<SharedString>,
    entries: Vec<(SharedString, SharedString)>,
    muted: Hsla,
) -> Div {
    panel_card(card, border, title, metrics_list(entries, muted))
}

fn optional_text(value: &SharedString, fallback: &'static str) -> SharedString {
    if value.is_empty() {
        SharedString::from(fallback)
    } else {
        value.clone()
    }
}

fn sparkline(history: &std::collections::VecDeque<f32>, accent: Hsla) -> Div {
    let max_value = history.iter().copied().fold(1.0_f32, f32::max).max(1.0);
    div()
        .h(px(64.))
        .flex()
        .items_end()
        .gap(px(1.))
        .children(history.iter().map(|value| {
            let ratio = (*value / max_value).clamp(0.06, 1.0);
            div()
                .w(px(3.))
                .h(px(56.0 * ratio))
                .rounded(px(999.))
                .bg(Hsla { a: 0.88, ..accent })
                .into_any_element()
        }))
}

fn inspector_value(value: &SharedString, empty: &'static str) -> SharedString {
    if value.is_empty() {
        SharedString::from(empty)
    } else {
        value.clone()
    }
}

fn selected_element_summary(debug: &DebugState, empty: &'static str) -> String {
    let selected_label = inspector_value(&debug.inspector.selected_label, empty);
    let source_location = inspector_value(&debug.inspector.source_location, empty);
    let bounds_label = inspector_value(&debug.inspector.bounds_label, empty);
    let content_size_label = inspector_value(&debug.inspector.content_size_label, empty);
    let background_hex = inspector_value(&debug.inspector.background_hex, empty);
    let border_hex = inspector_value(&debug.inspector.border_hex, empty);
    let opacity = debug
        .inspector
        .opacity
        .map(|value| format!("{value:.2}"))
        .unwrap_or_else(|| "1.00".to_string());
    format!(
        "selected={selected_label}\nsource={source_location}\nbounds={bounds_label}\ncontent={content_size_label}\nbackground={background_hex}\nborder={border_hex}\nopacity={opacity}"
    )
}

fn render_window_metrics_panel(
    card: Hsla,
    border: Hsla,
    muted: Hsla,
    runtime: &DebugRuntimeSnapshot,
) -> Div {
    panel_card(
        card,
        border,
        "Per-window Metrics",
        div().flex().flex_col().gap(px(8.)).child(
            div()
                .text_size(px(11.))
                .line_height(px(17.))
                .whitespace_normal()
                .text_color(muted)
                .child(SharedString::from(window_metrics_summary(
                    &runtime.gpui_window_metrics,
                ))),
        ),
    )
}

#[derive(Clone, Copy)]
struct DebugCopy {
    devtools: &'static str,
    subtitle: &'static str,
    overview: &'static str,
    elements: &'static str,
    performance: &'static str,
    logs: &'static str,
    navigator: &'static str,
    explorer: &'static str,
    selected_node: &'static str,
    recent_nodes: &'static str,
    recent_nodes_hint: &'static str,
    no_history: &'static str,
    actions: &'static str,
    paths: &'static str,
    styles: &'static str,
    metrics: &'static str,
    log_source: &'static str,
    latest_log: &'static str,
    stall_log: &'static str,
    console_filters: &'static str,
    filter_all: &'static str,
    filter_errors: &'static str,
    filter_warnings: &'static str,
    filter_info: &'static str,
    filter_debug: &'static str,
    filter_trace: &'static str,
    runtime: &'static str,
    inspector: &'static str,
    enabled: &'static str,
    picking: &'static str,
    source: &'static str,
    window: &'static str,
    debug_window: &'static str,
    fps: &'static str,
    app_version: &'static str,
    executable: &'static str,
    exe_size: &'static str,
    updates: &'static str,
    element_viewer: &'static str,
    no_selection: &'static str,
    pick_hint: &'static str,
    layout: &'static str,
    appearance: &'static str,
    bounds: &'static str,
    content: &'static str,
    background: &'static str,
    border: &'static str,
    opacity: &'static str,
    tools: &'static str,
    toggle_inspector: &'static str,
    pick_element: &'static str,
    opacity_down: &'static str,
    opacity_up: &'static str,
    copy_selection: &'static str,
    copy_source: &'static str,
    reset_styles: &'static str,
    open_logs: &'static str,
    refresh_windows: &'static str,
    clear_history: &'static str,
    copy_path: &'static str,
    open_current_log: &'static str,
    copy_console: &'static str,
    live_style: &'static str,
    clear_background: &'static str,
    inspector_state: &'static str,
    selection: &'static str,
    selection_locked: &'static str,
    selection_empty: &'static str,
    frame_time: &'static str,
    render_time: &'static str,
    frame_ms: &'static str,
    render_ms: &'static str,
    render_avg_ms: &'static str,
    console: &'static str,
    console_updated: &'static str,
    console_not_updated: &'static str,
    console_empty: &'static str,
    stall_monitor: &'static str,
    no_stall_events: &'static str,
    source_unavailable: &'static str,
    copied: &'static str,
    opened: &'static str,
    refreshed: &'static str,
    history_cleared: &'static str,
    styles_reset: &'static str,
    unknown: &'static str,
    none: &'static str,
}

impl DebugCopy {
    fn from_locale(locale: Locale) -> Self {
        match locale {
            Locale::ZhCn | Locale::ZhTw => Self {
                devtools: "开发工具",
                subtitle: "元素查看、实时样式、性能分析与日志控制台",
                overview: "概览",
                elements: "元素",
                performance: "性能",
                logs: "控制台",
                navigator: "导航",
                explorer: "元素树",
                selected_node: "当前节点",
                recent_nodes: "最近选中",
                recent_nodes_hint: "点击历史项可重新同步当前检查目标。",
                no_history: "暂无历史记录",
                actions: "操作",
                paths: "路径",
                styles: "样式",
                metrics: "盒模型",
                log_source: "日志源",
                latest_log: "latest.log",
                stall_log: "ui_foreground_stall.log",
                console_filters: "级别过滤",
                filter_all: "全部",
                filter_errors: "错误",
                filter_warnings: "警告",
                filter_info: "信息",
                filter_debug: "调试",
                filter_trace: "追踪",
                runtime: "运行时",
                inspector: "检查器",
                enabled: "启用",
                picking: "拾取中",
                source: "来源",
                window: "主窗口",
                debug_window: "调试窗口",
                fps: "帧率",
                app_version: "应用版本",
                executable: "程序路径",
                exe_size: "程序大小",
                updates: "更新状态",
                element_viewer: "元素查看器",
                no_selection: "未选中元素",
                pick_hint: "点击“拾取元素”后，到主窗口选择一个组件。",
                layout: "布局",
                appearance: "外观",
                bounds: "边界",
                content: "内容区",
                background: "背景",
                border: "边框",
                opacity: "透明度",
                tools: "工具",
                toggle_inspector: "切换检查器",
                pick_element: "拾取元素",
                opacity_down: "降低透明度",
                opacity_up: "提高透明度",
                copy_selection: "复制选中信息",
                copy_source: "复制源码位置",
                reset_styles: "重置样式",
                open_logs: "打开日志目录",
                refresh_windows: "刷新窗口",
                clear_history: "清空历史",
                copy_path: "复制路径",
                open_current_log: "打开当前日志",
                copy_console: "复制日志内容",
                live_style: "实时样式",
                clear_background: "清空背景",
                inspector_state: "检查器状态",
                selection: "选中状态",
                selection_locked: "已锁定",
                selection_empty: "空",
                frame_time: "帧时间",
                render_time: "渲染时间",
                frame_ms: "帧耗时",
                render_ms: "渲染耗时",
                render_avg_ms: "平均渲染耗时",
                console: "日志控制台",
                console_updated: "最近更新于 {ms}ms 前",
                console_not_updated: "尚未更新",
                console_empty: "当前没有可显示的日志输出。",
                stall_monitor: "卡顿监控",
                no_stall_events: "当前没有记录到 UI 前台卡顿事件。",
                source_unavailable: "无法获取源码位置",
                copied: "已复制",
                opened: "已打开",
                refreshed: "已刷新窗口",
                history_cleared: "已清空历史记录",
                styles_reset: "已重置选中元素样式",
                unknown: "未知",
                none: "无",
            },
            _ => Self {
                devtools: "DevTools",
                subtitle: "Element viewer, live styles, performance and console",
                overview: "Overview",
                elements: "Elements",
                performance: "Performance",
                logs: "Console",
                navigator: "Navigation",
                explorer: "Explorer",
                selected_node: "Selected Node",
                recent_nodes: "Recent Picks",
                recent_nodes_hint: "Click a history item to sync the inspector target again.",
                no_history: "No recent inspected elements",
                actions: "Actions",
                paths: "Paths",
                styles: "Styles",
                metrics: "Box Model",
                log_source: "Log Source",
                latest_log: "latest.log",
                stall_log: "ui_foreground_stall.log",
                console_filters: "Level Filters",
                filter_all: "All",
                filter_errors: "Errors",
                filter_warnings: "Warnings",
                filter_info: "Info",
                filter_debug: "Debug",
                filter_trace: "Trace",
                runtime: "Runtime",
                inspector: "Inspector",
                enabled: "Enabled",
                picking: "Picking",
                source: "Source",
                window: "Main Window",
                debug_window: "Debug Window",
                fps: "FPS",
                app_version: "App Version",
                executable: "Executable",
                exe_size: "Executable Size",
                updates: "Updates",
                element_viewer: "Element Viewer",
                no_selection: "No element selected",
                pick_hint: "Click `Pick Element` and choose a widget in the main window.",
                layout: "Layout",
                appearance: "Appearance",
                bounds: "Bounds",
                content: "Content",
                background: "Background",
                border: "Border",
                opacity: "Opacity",
                tools: "Tools",
                toggle_inspector: "Toggle Inspector",
                pick_element: "Pick Element",
                opacity_down: "Opacity -",
                opacity_up: "Opacity +",
                copy_selection: "Copy Selection",
                copy_source: "Copy Source",
                reset_styles: "Reset Styles",
                open_logs: "Open Logs",
                refresh_windows: "Refresh Windows",
                clear_history: "Clear History",
                copy_path: "Copy Path",
                open_current_log: "Open Current Log",
                copy_console: "Copy Console",
                live_style: "Live Style",
                clear_background: "Clear Background",
                inspector_state: "Inspector State",
                selection: "Selection",
                selection_locked: "Locked",
                selection_empty: "Empty",
                frame_time: "Frame Time",
                render_time: "Render Time",
                frame_ms: "Frame ms",
                render_ms: "Render ms",
                render_avg_ms: "Avg render ms",
                console: "Console",
                console_updated: "Updated {ms}ms ago",
                console_not_updated: "Not updated",
                console_empty: "No log output available.",
                stall_monitor: "Stall Monitor",
                no_stall_events: "No UI foreground stall has been recorded.",
                source_unavailable: "Source unavailable",
                copied: "Copied",
                opened: "Opened",
                refreshed: "Windows refreshed",
                history_cleared: "History cleared",
                styles_reset: "Selected element styles reset",
                unknown: "Unknown",
                none: "None",
            },
        }
    }
}

#[derive(Clone, Copy)]
enum LogLevelTone {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
    Plain,
}

fn detect_log_tone(line: &str) -> LogLevelTone {
    let upper = line.to_ascii_uppercase();
    if upper.contains(" ERROR ") || upper.starts_with("ERROR") {
        LogLevelTone::Error
    } else if upper.contains(" WARN ") || upper.starts_with("WARN") {
        LogLevelTone::Warn
    } else if upper.contains(" INFO ") || upper.starts_with("INFO") {
        LogLevelTone::Info
    } else if upper.contains(" DEBUG ") || upper.starts_with("DEBUG") {
        LogLevelTone::Debug
    } else if upper.contains(" TRACE ") || upper.starts_with("TRACE") {
        LogLevelTone::Trace
    } else {
        LogLevelTone::Plain
    }
}

fn log_tone_colors(tone: LogLevelTone, theme_k: f32) -> (Hsla, Hsla, Hsla) {
    match tone {
        LogLevelTone::Error => (
            lerp_color(rgb(0xfef2f2), rgb(0x2a1113), theme_k),
            lerp_color(rgb(0xfecaca), rgb(0x7f1d1d), theme_k),
            lerp_color(rgb(0xb91c1c), rgb(0xfca5a5), theme_k),
        ),
        LogLevelTone::Warn => (
            lerp_color(rgb(0xfffbeb), rgb(0x2b2110), theme_k),
            lerp_color(rgb(0xfde68a), rgb(0x854d0e), theme_k),
            lerp_color(rgb(0xb45309), rgb(0xfcd34d), theme_k),
        ),
        LogLevelTone::Info => (
            lerp_color(rgb(0xeff6ff), rgb(0x101f33), theme_k),
            lerp_color(rgb(0xbfdbfe), rgb(0x1d4ed8), theme_k),
            lerp_color(rgb(0x1d4ed8), rgb(0x93c5fd), theme_k),
        ),
        LogLevelTone::Debug => (
            lerp_color(rgb(0xf0fdf4), rgb(0x0f2218), theme_k),
            lerp_color(rgb(0xbbf7d0), rgb(0x166534), theme_k),
            lerp_color(rgb(0x15803d), rgb(0x86efac), theme_k),
        ),
        LogLevelTone::Trace => (
            lerp_color(rgb(0xf5f3ff), rgb(0x1c1633), theme_k),
            lerp_color(rgb(0xddd6fe), rgb(0x6d28d9), theme_k),
            lerp_color(rgb(0x7c3aed), rgb(0xc4b5fd), theme_k),
        ),
        LogLevelTone::Plain => (
            lerp_color(rgb(0xf8fafc), rgb(0x0b1020), theme_k),
            lerp_color(rgb(0xe2e8f0), rgb(0x223044), theme_k),
            lerp_color(rgb(0x475569), rgb(0xcbd5e1), theme_k),
        ),
    }
}

fn log_badge_label(tone: LogLevelTone) -> &'static str {
    match tone {
        LogLevelTone::Error => "ERROR",
        LogLevelTone::Warn => "WARN",
        LogLevelTone::Info => "INFO",
        LogLevelTone::Debug => "DEBUG",
        LogLevelTone::Trace => "TRACE",
        LogLevelTone::Plain => "LOG",
    }
}

fn log_matches_filter(tone: LogLevelTone, filter: ConsoleFilter) -> bool {
    match filter {
        ConsoleFilter::All => true,
        ConsoleFilter::Error => matches!(tone, LogLevelTone::Error),
        ConsoleFilter::Warn => matches!(tone, LogLevelTone::Warn),
        ConsoleFilter::Info => matches!(tone, LogLevelTone::Info),
        ConsoleFilter::Debug => matches!(tone, LogLevelTone::Debug),
        ConsoleFilter::Trace => matches!(tone, LogLevelTone::Trace),
    }
}

fn render_log_console(
    log_tail: &SharedString,
    text: Hsla,
    border: Hsla,
    muted: Hsla,
    theme_k: f32,
    mono: &'static str,
    copy: DebugCopy,
    filter: ConsoleFilter,
    height: Pixels,
) -> AnyElement {
    let mut lines: Vec<&str> = log_tail
        .lines()
        .filter(|line| log_matches_filter(detect_log_tone(line), filter))
        .rev()
        .take(DEBUG_CONSOLE_RENDER_LINE_LIMIT)
        .collect();
    lines.reverse();
    div()
        .h(height)
        .min_h(px(220.))
        .rounded(px(10.))
        .bg(lerp_color(rgb(0xf8fafc), rgb(0x0b1020), theme_k))
        .border_1()
        .border_color(border)
        .p(px(10.))
        .overflow_hidden()
        .overflow_y_scrollbar()
        .min_w(px(0.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .children(if lines.is_empty() {
                    vec![
                        div()
                            .text_size(px(11.))
                            .text_color(muted)
                            .child(copy.console_empty)
                            .into_any_element(),
                    ]
                } else {
                    lines
                        .into_iter()
                        .map(|line| {
                            let tone = detect_log_tone(line);
                            let (row_bg, row_border, badge) = log_tone_colors(tone, theme_k);
                            div()
                                .w_full()
                                .rounded(px(8.))
                                .bg(row_bg)
                                .border_1()
                                .border_color(row_border)
                                .px(px(8.))
                                .py(px(6.))
                                .flex()
                                .gap(px(8.))
                                .items_start()
                                .overflow_hidden()
                                .child(
                                    div()
                                        .min_w(px(52.))
                                        .px(px(6.))
                                        .py(px(2.))
                                        .rounded(px(999.))
                                        .bg(Hsla { a: 0.18, ..badge })
                                        .text_size(px(10.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(badge)
                                        .child(log_badge_label(tone)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .text_size(px(11.))
                                        .line_height(px(16.))
                                        .text_color(text)
                                        .whitespace_nowrap()
                                        .overflow_x_scrollbar()
                                        .child(line.to_string()),
                                )
                                .into_any_element()
                        })
                        .collect()
                }),
        )
        .into_any_element()
}

impl Render for DebugView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let theme_k = cx.global::<ThemeState>().factor(now);
        let locale = cx.global::<I18n>().locale();
        let copy = DebugCopy::from_locale(locale);
        let debug = cx.global::<DebugState>().clone();
        let update = cx.global::<UpdateState>();
        let debug_window_width = window.bounds().size.width / px(1.);
        let debug_window_height = window.bounds().size.height / px(1.);
        let runtime = self.runtime.clone();
        let narrow_layout = debug_window_width < 1180.0;
        let compact_layout = debug_window_width < 1440.0;
        let stacked_layout = debug_window_width < 900.0;
        let short_layout = debug_window_height < 760.0;
        let history_height = if stacked_layout {
            px(220.)
        } else if short_layout {
            px(300.)
        } else {
            px(420.)
        };
        let console_height = if short_layout {
            px(280.)
        } else if narrow_layout {
            px(360.)
        } else {
            px(460.)
        };

        let bg = lerp_color(rgb(0xf6f7fb), rgb(0x0b1220), theme_k);
        let card = lerp_color(rgb(0xffffff), rgb(0x111827), theme_k);
        let border = lerp_color(rgb(0xe5e7eb), rgb(0x223044), theme_k);
        let text = lerp_color(rgb(0x0f172a), rgb(0xe5e7eb), theme_k);
        let muted = lerp_color(rgb(0x64748b), rgb(0x94a3b8), theme_k);
        let accent = lerp_color(rgb(0x0ea5e9), rgb(0x67e8f9), theme_k);
        let mono = "Consolas";

        let update_modal = format!(
            "show_modal={} modal_visible={} downloading={}",
            update.show_modal, update.modal_visible, update.downloading
        );
        let displayed_debug_window_width = if runtime.debug_window_width_px > 0.0 {
            runtime.debug_window_width_px
        } else {
            debug_window_width
        };
        let displayed_debug_window_height = if runtime.debug_window_height_px > 0.0 {
            runtime.debug_window_height_px
        } else {
            debug_window_height
        };

        let sidebar_button = |label: &'static str, active: bool| {
            div()
                .when(!narrow_layout, |this| this.w_full())
                .when(narrow_layout, |this| this.min_w(px(120.)))
                .px(px(10.))
                .py(px(8.))
                .rounded(px(10.))
                .bg(if active {
                    Hsla { a: 0.18, ..accent }
                } else {
                    Hsla { a: 0.0, ..accent }
                })
                .border_1()
                .border_color(if active {
                    Hsla { a: 0.34, ..accent }
                } else {
                    Hsla { a: 0.10, ..border }
                })
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text)
                        .child(label),
                )
        };

        let filter_button = |label: &'static str, active: bool| {
            div()
                .px(px(8.))
                .py(px(5.))
                .rounded(px(999.))
                .bg(if active {
                    Hsla { a: 0.18, ..accent }
                } else {
                    Hsla { a: 0.0, ..accent }
                })
                .border_1()
                .border_color(if active {
                    Hsla { a: 0.34, ..accent }
                } else {
                    Hsla { a: 0.12, ..border }
                })
                .child(div().text_size(px(11.)).text_color(text).child(label))
        };

        let action_button = |id: &'static str, label: &'static str| {
            Button::new(id)
                .label(label)
                .bg(lerp_color(rgb(0xf8fafc), rgb(0x0f172a), theme_k))
                .border_color(border)
                .text_color(text)
        };

        let path_block = |label: &'static str, value: SharedString| {
            div()
                .rounded(px(10.))
                .bg(lerp_color(rgb(0xf8fafc), rgb(0x0b1020), theme_k))
                .border_1()
                .border_color(border)
                .p(px(10.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(div().text_size(px(11.)).text_color(muted).child(label))
                .child(
                    div()
                        .text_size(px(11.))
                        .line_height(px(16.))
                        .whitespace_nowrap()
                        .overflow_x_scrollbar()
                        .child(value),
                )
        };

        let navigation = div()
            .w(if narrow_layout {
                relative(1.)
            } else {
                px(180.).into()
            })
            .min_w(px(0.))
            .rounded(px(12.))
            .bg(lerp_color(rgb(0xf8fafc), rgb(0x0f172a), theme_k))
            .border_1()
            .border_color(border)
            .p(px(10.))
            .flex()
            .when(narrow_layout, |this| this.flex_wrap().items_center())
            .when(!narrow_layout, |this| this.flex_col())
            .gap(px(8.))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(muted)
                    .child(copy.navigator),
            )
            .child(
                sidebar_button(copy.elements, self.tab == DebugTab::Elements)
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.tab = DebugTab::Elements;
                            tracing::debug!("debug_window_click: tab=elements");
                            cx.notify();
                        }),
                    ),
            )
            .child(
                sidebar_button(copy.logs, self.tab == DebugTab::Console)
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.tab = DebugTab::Console;
                            tracing::debug!("debug_window_click: tab=console");
                            cx.notify();
                        }),
                    ),
            )
            .child(
                sidebar_button(copy.performance, self.tab == DebugTab::Performance)
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.tab = DebugTab::Performance;
                            tracing::debug!("debug_window_click: tab=performance");
                            cx.notify();
                        }),
                    ),
            )
            .child(
                sidebar_button(copy.overview, self.tab == DebugTab::Overview)
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.tab = DebugTab::Overview;
                            tracing::debug!("debug_window_click: tab=overview");
                            cx.notify();
                        }),
                    ),
            );

        let logs_dir = file_ops::logs_dir();
        let latest_log_path = logs_dir.join("latest.log");
        let stall_log_path = logs_dir.join("ui_foreground_stall.log");
        let overview = (self.tab == DebugTab::Overview).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(
                    div()
                        .flex()
                        .when(compact_layout, |this| this.flex_col())
                        .gap(px(12.))
                        .children([
                            panel_card(
                                card,
                                border,
                                copy.runtime,
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.))
                                    .child(line(
                                        copy.enabled,
                                        SharedString::from(format!("{}", debug.enabled)),
                                        muted,
                                    ))
                                    .child(line(
                                        copy.window,
                                        SharedString::from(format!(
                                            "{:.0} x {:.0} px",
                                            runtime.main_window_width_px,
                                            runtime.main_window_height_px
                                        )),
                                        muted,
                                    ))
                                    .child(line(
                                        copy.debug_window,
                                        SharedString::from(format!(
                                            "{:.0} x {:.0} px",
                                            displayed_debug_window_width,
                                            displayed_debug_window_height
                                        )),
                                        muted,
                                    ))
                                    .child(line(
                                        copy.fps,
                                        SharedString::from(format!("{:.1}", runtime.main_fps)),
                                        muted,
                                    ))
                                    .child(line(
                                        copy.app_version,
                                        SharedString::from(crate::utils::app_info::get_version()),
                                        muted,
                                    ))
                                    .child(line(
                                        copy.exe_size,
                                        SharedString::from(bytes_to_human(debug.exe_size_bytes)),
                                        muted,
                                    ))
                                    .child(line(
                                        copy.updates,
                                        SharedString::from(update_modal.clone()),
                                        muted,
                                    )),
                            )
                            .flex_1()
                            .into_any_element(),
                            panel_card(
                                card,
                                border,
                                copy.inspector,
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.))
                                    .child(line(
                                        copy.enabled,
                                        SharedString::from(format!("{}", debug.inspector.enabled)),
                                        muted,
                                    ))
                                    .child(line(
                                        copy.picking,
                                        SharedString::from(format!("{}", debug.inspector.picking)),
                                        muted,
                                    ))
                                    .child(line(
                                        copy.selection,
                                        if debug.inspector.selected_label.is_empty() {
                                            SharedString::from(copy.selection_empty)
                                        } else {
                                            SharedString::from(copy.selection_locked)
                                        },
                                        muted,
                                    ))
                                    .child(line(
                                        copy.source,
                                        if debug.inspector.source_location.is_empty() {
                                            SharedString::from(copy.none)
                                        } else {
                                            debug.inspector.source_location.clone()
                                        },
                                        muted,
                                    )),
                            )
                            .flex_1()
                            .into_any_element(),
                        ]),
                )
                .child(
                    div()
                        .flex()
                        .when(compact_layout, |this| this.flex_col())
                        .gap(px(12.))
                        .children([
                            panel_card(
                                card,
                                border,
                                copy.paths,
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(8.))
                                    .child(path_block(copy.executable, debug.exe_path.clone()))
                                    .child(path_block(
                                        copy.latest_log,
                                        SharedString::from(
                                            latest_log_path.to_string_lossy().to_string(),
                                        ),
                                    ))
                                    .child(path_block(
                                        copy.stall_log,
                                        SharedString::from(
                                            stall_log_path.to_string_lossy().to_string(),
                                        ),
                                    )),
                            )
                            .flex_1()
                            .into_any_element(),
                            panel_card(
                                card,
                                border,
                                copy.actions,
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap(px(8.))
                                    .child(
                                        action_button("debug-copy-exe-path", copy.copy_path)
                                            .on_click(cx.listener(move |_, _, _, cx| {
                                                let exe_path =
                                                    cx.read_global(|debug: &DebugState, _cx| {
                                                        debug.exe_path.clone()
                                                    });
                                                copy_text_to_clipboard(
                                                    exe_path.to_string(),
                                                    SharedString::from(format!(
                                                        "{}: {}",
                                                        copy.copied, copy.executable
                                                    )),
                                                    cx,
                                                );
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        action_button("debug-open-log-dir", copy.open_logs)
                                            .on_click(cx.listener(move |_, _, _, cx| {
                                                open_path_in_background(
                                                    file_ops::logs_dir(),
                                                    SharedString::from(format!(
                                                        "{}: {}",
                                                        copy.opened, copy.open_logs
                                                    )),
                                                    cx,
                                                );
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        action_button(
                                            "debug-refresh-windows",
                                            copy.refresh_windows,
                                        )
                                        .on_click(
                                            cx.listener(move |_, _, _, cx| {
                                                tracing::debug!(
                                                    "debug_window_click: action=refresh_windows"
                                                );
                                                cx.refresh_windows();
                                                cx.notify();
                                                toast::success(
                                                    cx,
                                                    SharedString::from(copy.refreshed),
                                                );
                                            }),
                                        ),
                                    )
                                    .child(
                                        action_button("debug-clear-history", copy.clear_history)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                tracing::debug!(
                                                    "debug_window_click: action=clear_history"
                                                );
                                                let _ = cx.update_global(
                                                    |debug: &mut DebugState, _cx| {
                                                        debug.clear_inspector_history();
                                                    },
                                                );
                                                this.log_tail_last_updated = Some(Instant::now());
                                                cx.refresh_windows();
                                                cx.notify();
                                                toast::success(
                                                    cx,
                                                    SharedString::from(copy.history_cleared),
                                                );
                                            })),
                                    ),
                            )
                            .flex_1()
                            .into_any_element(),
                        ]),
                )
        });

        let inspector_controls = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .child(
                action_button("debug-toggle-inspector", copy.toggle_inspector).on_click(
                    cx.listener(|_, _, _, cx| {
                        tracing::debug!("debug_window_click: action=toggle_inspector");
                        devtools::toggle_main_window_inspector(cx);
                        cx.notify();
                    }),
                ),
            )
            .child(
                action_button("debug-pick-inspector", copy.pick_element).on_click(cx.listener(
                    |_, _, _, cx| {
                        tracing::debug!("debug_window_click: action=pick_inspector");
                        devtools::begin_main_window_pick(cx);
                        cx.notify();
                    },
                )),
            )
            .child(
                action_button("debug-opacity-down", copy.opacity_down).on_click(cx.listener(
                    |_, _, _, cx| {
                        tracing::debug!("debug_window_click: action=opacity_down");
                        let current = cx
                            .read_global(|debug: &DebugState, _cx| debug.inspector.opacity)
                            .unwrap_or(1.0);
                        devtools::set_selected_element_opacity(cx, current - 0.05);
                        cx.notify();
                    },
                )),
            )
            .child(
                action_button("debug-opacity-up", copy.opacity_up).on_click(cx.listener(
                    |_, _, _, cx| {
                        tracing::debug!("debug_window_click: action=opacity_up");
                        let current = cx
                            .read_global(|debug: &DebugState, _cx| debug.inspector.opacity)
                            .unwrap_or(1.0);
                        devtools::set_selected_element_opacity(cx, current + 0.05);
                        cx.notify();
                    },
                )),
            )
            .child(
                action_button("debug-copy-selection", copy.copy_selection).on_click(cx.listener(
                    move |_, _, _, cx| {
                        let debug = cx.read_global(|debug: &DebugState, _cx| debug.clone());
                        if debug.inspector.selected_label.is_empty() {
                            toast::error(cx, SharedString::from(copy.no_selection));
                            return;
                        }
                        copy_text_to_clipboard(
                            selected_element_summary(&debug, copy.none),
                            SharedString::from(format!("{}: {}", copy.copied, copy.copy_selection)),
                            cx,
                        );
                        cx.notify();
                    },
                )),
            )
            .child(
                action_button("debug-copy-selection-source", copy.copy_source).on_click(
                    cx.listener(move |_, _, _, cx| {
                        let source = cx.read_global(|debug: &DebugState, _cx| {
                            debug.inspector.source_location.clone()
                        });
                        if source.is_empty() {
                            toast::error(cx, SharedString::from(copy.source_unavailable));
                            return;
                        }
                        copy_text_to_clipboard(
                            source.to_string(),
                            SharedString::from(format!("{}: {}", copy.copied, copy.copy_source)),
                            cx,
                        );
                        cx.notify();
                    }),
                ),
            )
            .child(
                action_button("debug-reset-selection-style", copy.reset_styles).on_click(
                    cx.listener(move |_, _, _, cx| {
                        devtools::reset_selected_element_styles(cx);
                        cx.notify();
                        toast::success(cx, SharedString::from(copy.styles_reset));
                    }),
                ),
            );

        let color_presets = ["#0ea5e9", "#22c55e", "#f97316", "#ef4444", "#111827"];
        let has_selection = !debug.inspector.selected_label.is_empty();
        let elements = (self.tab == DebugTab::Elements).then(|| {
            div()
            .flex()
            .min_w(px(0.))
            .when(narrow_layout, |this| this.flex_col())
            .gap(px(12.))
            .items_start()
            .child(
                div()
                    .w(if narrow_layout {
                        relative(1.)
                    } else if compact_layout {
                        px(220.).into()
                    } else {
                        px(240.).into()
                    })
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(panel_card(
                        card,
                        border,
                        copy.explorer,
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .child(
                                div()
                                    .rounded(px(10.))
                                    .bg(lerp_color(rgb(0xf8fafc), rgb(0x0b1020), theme_k))
                                    .border_1()
                                    .border_color(border)
                                    .p(px(10.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(muted)
                                            .child(copy.selected_node),
                                    )
                                    .child(
                                        div()

                                            .text_size(px(12.))
                                            .whitespace_nowrap()
                                            .overflow_x_scrollbar()
                                            .child(if has_selection {
                                                format!("<div {}>", debug.inspector.selected_label)
                                            } else {
                                                format!("<div>{}</div>", copy.no_selection)
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(muted)
                                    .child(copy.recent_nodes),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(muted)
                                    .child(copy.recent_nodes_hint),
                            )
                            .child(
                                div()
                                    .h(history_height)
                                    .overflow_y_scrollbar()
                                    .flex()
                                    .flex_col()
                                    .gap(px(8.))
                                    .children(if debug.inspector_history.is_empty() {
                                        vec![
                                            div()
                                                .rounded(px(8.))
                                                .bg(lerp_color(
                                                    rgb(0xf8fafc),
                                                    rgb(0x0f172a),
                                                    theme_k,
                                                ))
                                                .border_1()
                                                .border_color(border)
                                                .p(px(10.))
                                                .text_size(px(11.))
                                                .text_color(muted)
                                                .child(copy.no_history)
                                                .into_any_element(),
                                        ]
                                    } else {
                                        debug
                                            .inspector_history
                                            .iter()
                                            .take(24)
                                            .enumerate()
                                            .map(|(index, entry)| {
                                                div()
                                                    .rounded(px(8.))
                                                    .bg(lerp_color(
                                                        rgb(0xf8fafc),
                                                        rgb(0x0f172a),
                                                        theme_k,
                                                    ))
                                                    .border_1()
                                                    .border_color(border)
                                                    .p(px(10.))
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(4.))
                                                    .cursor_pointer()
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(move |_, _, _, cx| {
                                                            devtools::select_inspector_history_entry(
                                                                index, cx,
                                                            );
                                                            cx.notify();
                                                        }),
                                                    )
                                                    .child(
                                                        div()

                                                            .text_size(px(11.))
                                                            .whitespace_nowrap()
                                                            .overflow_x_scrollbar()
                                                            .child(format!(
                                                                "<div {}>",
                                                                entry.selected_label
                                                            )),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(10.))
                                                            .text_color(muted)
                                                            .whitespace_nowrap()
                                                            .overflow_x_scrollbar()
                                                            .child(inspector_value(
                                                                &entry.source_location,
                                                                copy.source_unavailable,
                                                            )),
                                                    )
                                                    .into_any_element()
                                            })
                                            .collect()
                                    }),
                            ),
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .when(narrow_layout, |this| this.w_full())
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(panel_card(
                        card,
                        border,
                        copy.element_viewer,
                        div().flex().flex_col().gap(px(10.)).child(
                            div()
                                .rounded(px(10.))
                                .bg(lerp_color(rgb(0xf8fafc), rgb(0x0b1020), theme_k))
                                .border_1()
                                .border_color(border)
                                .p(px(12.))
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .child(
                                    div()

                                        .text_size(px(13.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .whitespace_nowrap()
                                        .overflow_x_scrollbar()
                                        .child(if has_selection {
                                            debug.inspector.selected_label.clone()
                                        } else {
                                            SharedString::from(copy.no_selection)
                                        }),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(muted)
                                        .whitespace_nowrap()
                                        .overflow_x_scrollbar()
                                        .child(if has_selection {
                                            inspector_value(
                                                &debug.inspector.source_location,
                                                copy.source_unavailable,
                                            )
                                        } else {
                                            SharedString::from(copy.pick_hint)
                                        }),
                                ),
                        ),
                    ))
                    .child(panel_card(
                        card,
                        border,
                        copy.styles,
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .child(
                                div()
                                    .flex()
                                    .when(compact_layout, |this| this.flex_col())
                                    .gap(px(12.))
                                    .children([
                                        panel_card(
                                            lerp_color(rgb(0xf8fafc), rgb(0x101826), theme_k),
                                            border,
                                            copy.layout,
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap(px(6.))
                                                .child(line(
                                                    copy.bounds,
                                                    inspector_value(
                                                        &debug.inspector.bounds_label,
                                                        copy.unknown,
                                                    ),
                                                    muted,
                                                ))
                                                .child(line(
                                                    copy.content,
                                                    inspector_value(
                                                        &debug.inspector.content_size_label,
                                                        copy.unknown,
                                                    ),
                                                    muted,
                                                )),
                                        )
                                        .flex_1()
                                        .into_any_element(),
                                        panel_card(
                                            lerp_color(rgb(0xf8fafc), rgb(0x101826), theme_k),
                                            border,
                                            copy.appearance,
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap(px(6.))
                                                .child(line(
                                                    copy.background,
                                                    inspector_value(
                                                        &debug.inspector.background_hex,
                                                        copy.none,
                                                    ),
                                                    muted,
                                                ))
                                                .child(line(
                                                    copy.border,
                                                    inspector_value(
                                                        &debug.inspector.border_hex,
                                                        copy.none,
                                                    ),
                                                    muted,
                                                ))
                                                .child(line(
                                                    copy.opacity,
                                                    SharedString::from(
                                                        debug
                                                            .inspector
                                                            .opacity
                                                            .map(|value| format!("{value:.2}"))
                                                            .unwrap_or_else(|| "1.00".to_string()),
                                                    ),
                                                    muted,
                                                )),
                                        )
                                        .flex_1()
                                        .into_any_element(),
                                    ]),
                            )
                            .child(panel_card(
                                lerp_color(rgb(0xf8fafc), rgb(0x101826), theme_k),
                                border,
                                copy.live_style,
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(8.))
                                    .child(div().flex().items_center().gap(px(8.)).children(
                                        color_presets.into_iter().map(|hex| {
                                            let color =
                                                crate::ui::theme::colors::parse_hex_color_to_hsla(
                                                    hex,
                                                )
                                                .unwrap_or_else(|| rgb(0x0ea5e9).into());
                                            div()
                                                .w(px(28.))
                                                .h(px(28.))
                                                .rounded(px(999.))
                                                .bg(color)
                                                .border_1()
                                                .border_color(Hsla { a: 0.24, ..text })
                                                .cursor_pointer()
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |_, _, _, cx| {
                                                        devtools::set_selected_element_background(
                                                            cx, hex,
                                                        );
                                                        cx.notify();
                                                    }),
                                                )
                                                .into_any_element()
                                        }),
                                    ))
                                    .child(
                                        action_button(
                                            "debug-clear-selected-background",
                                            copy.clear_background,
                                        )
                                        .on_click(
                                            cx.listener(|_, _, _, cx| {
                                                devtools::clear_selected_element_background(cx);
                                                cx.notify();
                                            }),
                                        ),
                                    ),
                            ))
                            .child(panel_card(
                                lerp_color(rgb(0xf8fafc), rgb(0x101826), theme_k),
                                border,
                                copy.tools,
                                inspector_controls,
                            )),
                    )),
            )
            .child(
                div()
                    .w(if narrow_layout {
                        relative(1.)
                    } else if compact_layout {
                        px(220.).into()
                    } else {
                        px(260.).into()
                    })
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(panel_card(
                        card,
                        border,
                        copy.metrics,
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(line(
                                copy.bounds,
                                inspector_value(&debug.inspector.bounds_label, copy.unknown),
                                muted,
                            ))
                            .child(line(
                                copy.content,
                                inspector_value(&debug.inspector.content_size_label, copy.unknown),
                                muted,
                            ))
                            .child(line(
                                copy.opacity,
                                SharedString::from(
                                    debug
                                        .inspector
                                        .opacity
                                        .map(|value| format!("{value:.2}"))
                                        .unwrap_or_else(|| "1.00".to_string()),
                                ),
                                muted,
                            )),
                    ))
                    .child(panel_card(
                        card,
                        border,
                        copy.inspector_state,
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(line(
                                copy.enabled,
                                SharedString::from(format!("{}", debug.inspector.enabled)),
                                muted,
                            ))
                            .child(line(
                                copy.picking,
                                SharedString::from(format!("{}", debug.inspector.picking)),
                                muted,
                            ))
                            .child(line(
                                copy.selection,
                                if has_selection {
                                    SharedString::from(copy.selection_locked)
                                } else {
                                    SharedString::from(copy.selection_empty)
                                },
                                muted,
                            ))
                            .child(line(
                                copy.source,
                                inspector_value(
                                    &debug.inspector.source_location,
                                    copy.source_unavailable,
                                ),
                                muted,
                            )),
                    )),
            )
        });

        let stall_preview = {
            let lines = self
                .stall_log_tail
                .lines()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(str::to_string)
                .collect::<Vec<_>>();
            if lines.is_empty() {
                SharedString::from(copy.no_stall_events)
            } else {
                SharedString::from(lines.join("\n"))
            }
        };

        let process_id = runtime
            .process_id
            .map(|pid| pid.to_string())
            .unwrap_or_else(|| copy.unknown.to_string());
        let process_tasks = runtime
            .process_task_count
            .map(|count| count.to_string())
            .unwrap_or_else(|| copy.none.to_string());
        let gpu_device = optional_text(&runtime.gpu_device_name, copy.unknown);
        let gpu_driver_name = optional_text(&runtime.gpu_driver_name, copy.none);
        let gpu_driver_info = optional_text(&runtime.gpu_driver_info, copy.none);
        let map_memory = runtime.bmcbl_memory.map_viewer.clone();
        let memory_attribution = SharedString::from(memory_attribution_summary(&runtime));
        let plugin_memory_details =
            SharedString::from(plugin_memory_summary(&runtime.plugin_memory));

        let performance = (self.tab == DebugTab::Performance).then(|| {
            div()
            .flex()
            .flex_col()
            .min_h(px(0.))
            .min_w(px(0.))
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .min_w(px(0.))
                    .when(compact_layout, |this| this.flex_col())
                    .gap(px(10.))
                    .children([
                        panel_card(
                            card,
                            border,
                            copy.frame_time,
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(8.))
                                .child(metrics_list(
                                    vec![
                                        (
                                            SharedString::from(copy.window),
                                            SharedString::from(format!(
                                                "{:.0} x {:.0} px",
                                                runtime.main_window_width_px,
                                                runtime.main_window_height_px
                                            )),
                                        ),
                                        (
                                            SharedString::from(copy.fps),
                                            SharedString::from(format!("{:.1}", runtime.main_fps)),
                                        ),
                                        (
                                            SharedString::from(copy.frame_ms),
                                            SharedString::from(format!(
                                                "{:.2}",
                                                runtime.main_frame_time_ms
                                            )),
                                        ),
                                    ],
                                    muted,
                                ))
                                .child(sparkline(&runtime.frame_time_history_ms, accent)),
                        )
                        .flex_1()
                        .into_any_element(),
                        panel_card(
                            card,
                            border,
                            copy.render_time,
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(8.))
                                .child(metrics_list(
                                    vec![
                                        (
                                            SharedString::from(copy.render_ms),
                                            SharedString::from(format!(
                                                "{:.2}",
                                                runtime.main_render_time_ms
                                            )),
                                        ),
                                        (
                                            SharedString::from(copy.render_avg_ms),
                                            SharedString::from(format!(
                                                "{:.2}",
                                                runtime.main_render_time_avg_ms
                                            )),
                                        ),
                                        (
                                            SharedString::from("GPUI draw ms"),
                                            SharedString::from(format!(
                                                "{:.2}",
                                                runtime.gpui_draw_time_ms
                                            )),
                                        ),
                                        (
                                            SharedString::from("Present FPS"),
                                            SharedString::from(format!(
                                                "{:.1}",
                                                runtime.gpui_present_fps
                                            )),
                                        ),
                                    ],
                                    muted,
                                ))
                                .child(sparkline(
                                    &runtime.render_time_history_ms,
                                    lerp_color(rgb(0xf97316), rgb(0xfbbf24), theme_k),
                                )),
                        )
                        .flex_1()
                        .into_any_element(),
                    ]),
            )
            .child(
                div()
                    .w_full()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .grid()
                    .gap(px(10.))
                    .children([
                        metrics_panel(
                            card,
                            border,
                            "系统 / 进程",
                            vec![
                                (SharedString::from("进程 ID"), SharedString::from(process_id)),
                                (SharedString::from("任务数"), SharedString::from(process_tasks)),
                                (
                                    SharedString::from("CPU 占用"),
                                    SharedString::from(format!(
                                        "总计 {:.1}% / 归一化 {:.1}%",
                                        runtime.process_cpu_percent,
                                        runtime.process_cpu_normalized_percent
                                    )),
                                ),
                                (
                                    SharedString::from("当前驻留"),
                                    SharedString::from(format!(
                                        "RSS {} / 工作集 {}",
                                        bytes_to_human(runtime.process_memory_bytes),
                                        bytes_to_human(runtime.process_working_set_kb * 1024)
                                    )),
                                ),
                                (
                                    SharedString::from("提交内存"),
                                    SharedString::from(format!(
                                        "Private {} / Virtual {}（Virtual 只观察，不作为泄漏判据）",
                                        bytes_to_human(runtime.process_private_kb * 1024),
                                        bytes_to_human(runtime.process_virtual_memory_bytes)
                                    )),
                                ),
                                (
                                    SharedString::from("内存峰值"),
                                    SharedString::from(format!(
                                        "峰值工作集 {}",
                                        bytes_to_human(runtime.process_peak_working_set_kb * 1024)
                                    )),
                                ),
                                (
                                    SharedString::from("系统内存"),
                                    SharedString::from(format!(
                                        "已用 {} / 总计 {} / 可用 {} ({:.1}%)",
                                        bytes_to_human(runtime.system_memory_used_bytes),
                                        bytes_to_human(runtime.system_memory_total_bytes),
                                        bytes_to_human(runtime.system_memory_available_bytes),
                                        runtime.system_memory_used_percent
                                    )),
                                ),
                            ],
                            muted,
                        )
                        .into_any_element(),
                        panel_card(
                            card,
                            border,
                            "内存验证动作",
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .line_height(px(15.))
                                        .text_color(muted)
                                        .whitespace_normal()
                                        .child(
                                            "482MB Virtual 常见来自 mimalloc 保留区、tokio/thread stack reservation、GPU driver 虚拟地址和 GPUI 基础 atlas/surface；只有 Private 或 GPU retained 持续增长才按泄漏处理。",
                                        ),
                                )
                                .child(
                                    action_button("debug-aggressive-memory-cleanup", "强清理内存")
                                        .on_click(cx.listener(|_, _, _, cx| {
                                            run_aggressive_memory_cleanup(cx);
                                        })),
                                ),
                        )
                        .into_any_element(),
                        metrics_panel(
                            card,
                            border,
                            "GPU / 渲染器",
                            vec![
                                (
                                    SharedString::from("渲染后端"),
                                    runtime.gpui_renderer_backend.clone(),
                                ),
                                (SharedString::from("GPU 设备"), gpu_device),
                                (SharedString::from("驱动"), gpu_driver_name),
                                (SharedString::from("驱动信息"), gpu_driver_info),
                                (
                                    SharedString::from("软件模拟"),
                                    SharedString::from(if runtime.gpu_software_emulated {
                                        "是"
                                    } else {
                                        "否"
                                    }),
                                ),
                                (
                                    SharedString::from("Surface"),
                                    SharedString::from(format!(
                                        "{} / {} / {}",
                                        optional_text(
                                            &runtime.gpui_gpu_surface_format,
                                            copy.unknown
                                        ),
                                        optional_text(
                                            &runtime.gpui_gpu_surface_alpha_mode,
                                            copy.unknown
                                        ),
                                        optional_text(
                                            &runtime.gpui_gpu_surface_present_mode,
                                            copy.unknown
                                        )
                                    )),
                                ),
                            ],
                            muted,
                        )
                        .into_any_element(),
                        metrics_panel(
                            card,
                            border,
                            "帧阶段",
                            vec![
                                (
                                    SharedString::from("当前帧"),
                                    SharedString::from(format!(
                                        "build {:.2} / layout {:.2} / prepaint {:.2} / paint {:.2} / scene {:.2} / backend {:.2} ms",
                                        runtime.gpui_frame_build_time_ms,
                                        runtime.gpui_frame_layout_time_ms,
                                        runtime.gpui_frame_prepaint_time_ms,
                                        runtime.gpui_frame_paint_time_ms,
                                        runtime.gpui_frame_scene_finish_time_ms,
                                        runtime.gpui_frame_backend_draw_time_ms
                                    )),
                                ),
                                (
                                    SharedString::from("首帧"),
                                    SharedString::from(format!(
                                        "build {:.2} / layout {:.2} / prepaint {:.2} / paint {:.2} / scene {:.2} / backend {:.2} ms",
                                        runtime.gpui_first_frame_build_time_ms,
                                        runtime.gpui_first_frame_layout_time_ms,
                                        runtime.gpui_first_frame_prepaint_time_ms,
                                        runtime.gpui_first_frame_paint_time_ms,
                                        runtime.gpui_first_frame_scene_finish_time_ms,
                                        runtime.gpui_first_frame_backend_draw_time_ms
                                    )),
                                ),
                                (
                                    SharedString::from("帧计数"),
                                    SharedString::from(format!(
                                        "request {} / draw {} / present {} / skip {} / retained {}",
                                        runtime.gpui_frame_request_count,
                                        runtime.gpui_draw_count,
                                        runtime.gpui_present_count,
                                        runtime.gpui_skip_count,
                                        runtime.gpui_retained_present_count
                                    )),
                                ),
                            ],
                            muted,
                        )
                        .into_any_element(),
                        metrics_panel(
                            card,
                            border,
                            "布局 / 文本缓存",
                            vec![
                                (
                                    SharedString::from("布局节点"),
                                    SharedString::from(format!(
                                        "{} 节点 / {} 测量 / {} roots",
                                        runtime.gpui_layout_nodes,
                                        runtime.gpui_measured_layout_nodes,
                                        runtime.gpui_layout_roots
                                    )),
                                ),
                                (
                                    SharedString::from("边界缓存"),
                                    SharedString::from(format!(
                                        "{} hits / {} misses",
                                        runtime.gpui_layout_bounds_cache_hits,
                                        runtime.gpui_layout_bounds_cache_misses
                                    )),
                                ),
                                (
                                    SharedString::from("保留布局"),
                                    SharedString::from(format!(
                                        "{} hits / {} misses / {} reused / {} saved",
                                        runtime.gpui_layout_cache_hits,
                                        runtime.gpui_layout_cache_misses,
                                        runtime.gpui_layout_cache_reused_roots,
                                        runtime.gpui_layout_cache_saved_roots
                                    )),
                                ),
                                (
                                    SharedString::from("文本布局"),
                                    SharedString::from(format!(
                                        "{} hits / {} reuses / {} misses",
                                        runtime.gpui_text_layout_hits,
                                        runtime.gpui_text_layout_reuses,
                                        runtime.gpui_text_layout_misses
                                    )),
                                ),
                            ],
                            muted,
                        )
                        .into_any_element(),
                        metrics_panel(
                            card,
                            border,
                            "场景 / 保留渲染",
                            vec![
                                (
                                    SharedString::from("场景"),
                                    SharedString::from(format!(
                                        "{} primitives / {} batches / {} segments",
                                        runtime.gpui_scene_primitives,
                                        runtime.gpui_scene_batches,
                                        runtime.gpui_scene_segments
                                    )),
                                ),
                                (
                                    SharedString::from("保留 segment"),
                                    SharedString::from(format!(
                                        "{} rebuild / {} reuse / {} replayed",
                                        runtime.gpui_scene_segment_rebuild_count,
                                        runtime.gpui_scene_segment_reuse_count,
                                        runtime.gpui_scene_replayed_primitives
                                    )),
                                ),
                                (
                                    SharedString::from("脏区域"),
                                    SharedString::from(format!(
                                        "{} rect / area {} / transform {}",
                                        runtime.gpui_dirty_rect_count,
                                        runtime.gpui_dirty_rect_area,
                                        runtime.gpui_dirty_transform_count
                                    )),
                                ),
                                (
                                    SharedString::from("容量"),
                                    SharedString::from(format!(
                                        "scene {} / frame {}",
                                        bytes_to_human(runtime.gpui_scene_retained_capacity as u64),
                                        bytes_to_human(runtime.gpui_frame_retained_capacity as u64)
                                    )),
                                ),
                                (
                                    SharedString::from("跳过统计"),
                                    SharedString::from(format!(
                                        "retained {} / pointer {} / inactive {}",
                                        runtime.gpui_retained_frame_skips,
                                        runtime.gpui_skipped_pointer_frame_count,
                                        runtime.gpui_inactive_present_skip_count
                                    )),
                                ),
                            ],
                            muted,
                        )
                        .into_any_element(),
                        metrics_panel(
                            card,
                            border,
                            "内存归因排行",
                            vec![(
                                SharedString::from("按估算总量"),
                                memory_attribution.clone(),
                            )],
                            muted,
                        )
                        .into_any_element(),
                        metrics_panel(
                            card,
                            border,
                            "BMCBL 内存归因",
                            vec![
                                (
                                    SharedString::from("Map Viewer 总计"),
                                    SharedString::from(bytes_to_human(
                                        runtime.bmcbl_memory.total_estimated_bytes() as u64,
                                    )),
                                ),
                                (
                                    SharedString::from("地图 tile"),
                                    SharedString::from(format!(
                                        "{} tiles / {}",
                                        map_memory.tile_count,
                                        bytes_to_human(map_memory.tile_bytes as u64)
                                    )),
                                ),
                                (
                                    SharedString::from("Canvas snapshot"),
                                    SharedString::from(format!(
                                        "{} tile refs / {}",
                                        map_memory.canvas_snapshot_tile_count,
                                        bytes_to_human(map_memory.canvas_snapshot_bytes as u64)
                                    )),
                                ),
                                (
                                    SharedString::from("Paste preview"),
                                    SharedString::from(format!(
                                        "{} images / {}",
                                        map_memory.paste_preview_count,
                                        bytes_to_human(map_memory.paste_preview_bytes as u64)
                                    )),
                                ),
                                (
                                    SharedString::from("Copied/import preview"),
                                    SharedString::from(format!(
                                        "{} images / {}",
                                        map_memory.copied_import_preview_count,
                                        bytes_to_human(
                                            map_memory.copied_import_preview_bytes as u64
                                        )
                                    )),
                                ),
                                (
                                    SharedString::from("3D mesh CPU"),
                                    SharedString::from(format!(
                                        "{} chunks / {} vertices / {}",
                                        map_memory.preview_3d_chunk_mesh_count,
                                        map_memory.preview_3d_vertex_count,
                                        bytes_to_human(map_memory.preview_3d_mesh_bytes as u64)
                                    )),
                                ),
                                (
                                    SharedString::from("3D surface GPU"),
                                    SharedString::from(format!(
                                        "{} / render_in_flight={}",
                                        bytes_to_human(map_memory.preview_3d_surface_bytes as u64),
                                        bool_label(map_memory.preview_3d_render_in_flight)
                                    )),
                                ),
                            ],
                            muted,
                        )
                        .into_any_element(),
                        metrics_panel(
                            card,
                            border,
                            "插件内存归因",
                            vec![
                                (
                                    SharedString::from("插件总计"),
                                    SharedString::from(bytes_to_human(
                                        runtime.plugin_memory.total_estimated_bytes as u64,
                                    )),
                                ),
                                (
                                    SharedString::from("Module cache"),
                                    SharedString::from(format!(
                                        "{} modules / {} shared",
                                        runtime.plugin_memory.module_cache_entries,
                                        bytes_to_human(
                                            runtime.plugin_memory.module_cache_estimated_bytes
                                                as u64
                                        )
                                    )),
                                ),
                                (
                                    SharedString::from("Top plugins"),
                                    plugin_memory_details.clone(),
                                ),
                            ],
                            muted,
                        )
                        .into_any_element(),
                        metrics_panel(
                            card,
                            border,
                            "Nova GPU 上传 / 资源",
                            vec![
                                (
                                    SharedString::from("上传"),
                                    SharedString::from(format!(
                                        "{} total / {} pod / {} prepared",
                                        bytes_to_human(runtime.gpui_upload_bytes as u64),
                                        bytes_to_human(runtime.gpui_pod_upload_bytes as u64),
                                        runtime.gpui_prepared_command_count
                                    )),
                                ),
                                (
                                    SharedString::from("Atlas"),
                                    SharedString::from(format!(
                                        "{} textures / upload {} / {} tiles / {:.2} ms / retained {}",
                                        runtime.gpui_atlas_textures,
                                        bytes_to_human(runtime.gpui_atlas_upload_bytes as u64),
                                        runtime.gpui_atlas_upload_tiles,
                                        runtime.gpui_atlas_upload_time_ms,
                                        bytes_to_human(runtime.gpui_atlas_retained_bytes as u64)
                                    )),
                                ),
                                (
                                    SharedString::from("图像缓存"),
                                    SharedString::from(format!(
                                        "unified {} entries {} decoded / cpu {} / gpu {} / bounded {} items {} / target metrics {} items {} largest {} / evict {} / drop {} / remove {}",
                                        runtime.gpui_image_asset_cache_entries,
                                        bytes_to_human(
                                            runtime
                                                .gpui_image_asset_total_retained_decoded_bytes
                                                as u64
                                        ),
                                        bytes_to_human(runtime.gpui_render_image_cpu_bytes as u64),
                                        bytes_to_human(
                                            runtime.gpui_render_image_gpu_texture_bytes as u64
                                        ),
                                        runtime.gpui_image_cache_items,
                                        bytes_to_human(runtime.gpui_image_cache_bytes as u64),
                                        runtime.gpui_image_asset_retained_count,
                                        bytes_to_human(
                                            runtime.gpui_image_asset_retained_decoded_bytes as u64
                                        ),
                                        bytes_to_human(
                                            runtime
                                                .gpui_image_asset_largest_retained_decoded_bytes
                                                as u64
                                        ),
                                        runtime.gpui_image_cache_evictions,
                                        runtime.gpui_image_drop_count,
                                        runtime.gpui_atlas_remove_count
                                    )),
                                ),
                                (
                                    SharedString::from("全局图像资产"),
                                    SharedString::from(format!(
                                        "resource {} items {} / inline {} items {} / compressed {} items {} / target {} items {}",
                                        runtime.gpui_global_image_resource_count,
                                        bytes_to_human(
                                            runtime.gpui_global_image_resource_decoded_bytes as u64
                                        ),
                                        runtime.gpui_global_image_inline_count,
                                        bytes_to_human(
                                            runtime.gpui_global_image_inline_decoded_bytes as u64
                                        ),
                                        runtime.gpui_global_image_compressed_count,
                                        bytes_to_human(
                                            runtime.gpui_global_image_compressed_bytes as u64
                                        ),
                                        runtime.gpui_global_image_target_count,
                                        bytes_to_human(
                                            runtime.gpui_global_image_target_decoded_bytes as u64
                                        ),
                                    )),
                                ),
                                (
                                    SharedString::from("图像解码"),
                                    SharedString::from(format!(
                                        "{} 次 / {} -> {} / {} 帧 / max {:.2} ms / slow {}",
                                        runtime.gpui_image_decode_count,
                                        bytes_to_human(
                                            runtime.gpui_image_decode_total_compressed_bytes as u64
                                        ),
                                        bytes_to_human(
                                            runtime.gpui_image_decode_total_decoded_bytes as u64
                                        ),
                                        runtime.gpui_image_decode_total_frames,
                                        runtime.gpui_image_decode_max_time_ms,
                                        runtime.gpui_image_decode_slow_count
                                    )),
                                ),
                                (
                                    SharedString::from("Pass"),
                                    SharedString::from(format!(
                                        "mask {} / main {} / composite {}",
                                        runtime.gpui_mask_pass_count,
                                        runtime.gpui_main_pass_count,
                                        runtime.gpui_composite_pass_count
                                    )),
                                ),
                                (
                                    SharedString::from("Bind group"),
                                    SharedString::from(format!(
                                        "{} create / {} hits / {} misses",
                                        runtime.gpui_bind_group_creations,
                                        runtime.gpui_bind_group_cache_hits,
                                        runtime.gpui_bind_group_cache_misses
                                    )),
                                ),
                                (
                                    SharedString::from("上传 arena"),
                                    SharedString::from(format!(
                                        "uniform {} / {} used, storage {} / {} used",
                                        bytes_to_human(
                                            runtime.gpui_upload_arena_uniform_used as u64
                                        ),
                                        bytes_to_human(
                                            runtime.gpui_upload_arena_uniform_capacity as u64
                                        ),
                                        bytes_to_human(
                                            runtime.gpui_upload_arena_storage_used as u64
                                        ),
                                        bytes_to_human(
                                            runtime.gpui_upload_arena_storage_capacity as u64
                                        )
                                    )),
                                ),
                                (
                                    SharedString::from("GPU 缓存"),
                                    SharedString::from(format!(
                                        "{} hits / {} misses / GPU retained {} / surface {} / atlas mono {} poly {} live {} unused {} / unified {}",
                                        runtime.gpui_gpu_cache_hits,
                                        runtime.gpui_gpu_cache_misses,
                                        bytes_to_human(runtime.gpui_gpu_retained_bytes as u64),
                                        bytes_to_human(
                                            runtime.gpui_gpu_surface_texture_bytes as u64
                                        ),
                                        bytes_to_human(runtime.gpui_atlas_monochrome_bytes as u64),
                                        bytes_to_human(runtime.gpui_atlas_polychrome_bytes as u64),
                                        runtime.gpui_atlas_live_keys,
                                        bytes_to_human(runtime.gpui_atlas_unused_bytes as u64),
                                        bytes_to_human(
                                            runtime.gpui_gpu_estimated_total_retained_bytes as u64
                                        )
                                    )),
                                ),
                                (
                                    SharedString::from("Allocator"),
                                    SharedString::from(format!(
                                        "allocated {} / reserved {} / {} blocks / {} allocations",
                                        bytes_to_human(
                                            runtime.gpui_allocator_allocated_bytes as u64
                                        ),
                                        bytes_to_human(
                                            runtime.gpui_allocator_reserved_bytes as u64
                                        ),
                                        runtime.gpui_allocator_block_count,
                                        runtime.gpui_allocator_allocation_count
                                    )),
                                ),
                                (
                                    SharedString::from("Allocator buckets"),
                                    SharedString::from(format!(
                                        "gpu-only {} / {} / {} blocks, cpu->gpu {} / {} / {} blocks, gpu->cpu {} / {} / {} blocks",
                                        bytes_to_human(
                                            runtime
                                                .gpui_allocator_gpu_only_allocated_bytes
                                                as u64
                                        ),
                                        bytes_to_human(
                                            runtime
                                                .gpui_allocator_gpu_only_reserved_bytes
                                                as u64
                                        ),
                                        runtime.gpui_allocator_gpu_only_block_count,
                                        bytes_to_human(
                                            runtime
                                                .gpui_allocator_cpu_to_gpu_allocated_bytes
                                                as u64
                                        ),
                                        bytes_to_human(
                                            runtime
                                                .gpui_allocator_cpu_to_gpu_reserved_bytes
                                                as u64
                                        ),
                                        runtime.gpui_allocator_cpu_to_gpu_block_count,
                                        bytes_to_human(
                                            runtime
                                                .gpui_allocator_gpu_to_cpu_allocated_bytes
                                                as u64
                                        ),
                                        bytes_to_human(
                                            runtime
                                                .gpui_allocator_gpu_to_cpu_reserved_bytes
                                                as u64
                                        ),
                                        runtime.gpui_allocator_gpu_to_cpu_block_count
                                    )),
                                ),
                                (
                                    SharedString::from("HAL memory"),
                                    SharedString::from(format!(
                                        "buffer {} / texture {} / accel {} / {} allocations",
                                        bytes_to_human(
                                            runtime.gpui_hal_buffer_memory_bytes as u64
                                        ),
                                        bytes_to_human(
                                            runtime.gpui_hal_texture_memory_bytes as u64
                                        ),
                                        bytes_to_human(
                                            runtime
                                                .gpui_hal_acceleration_structure_memory_bytes
                                                as u64
                                        ),
                                        runtime.gpui_hal_memory_allocation_count
                                    )),
                                ),
                                (
                                    SharedString::from("Staging buffer"),
                                    SharedString::from(format!(
                                        "live {} ({} buffers, peak {}) / pending {} ({} buffers, peak {}) / created {}",
                                        bytes_to_human(
                                            runtime.gpui_core_staging_buffer_live_bytes as u64
                                        ),
                                        runtime.gpui_core_staging_buffer_live_count,
                                        bytes_to_human(
                                            runtime
                                                .gpui_core_staging_buffer_peak_live_bytes
                                                as u64
                                        ),
                                        bytes_to_human(
                                            runtime.gpui_core_staging_buffer_pending_bytes
                                                as u64
                                        ),
                                        runtime.gpui_core_staging_buffer_pending_count,
                                        bytes_to_human(
                                            runtime
                                                .gpui_core_staging_buffer_peak_pending_bytes
                                                as u64
                                        ),
                                        bytes_to_human(
                                            runtime.gpui_core_staging_buffer_created_bytes as u64
                                        )
                                    )),
                                ),
                                (
                                    SharedString::from("Retained targets"),
                                    SharedString::from(format!(
                                        "frame={} path={} backdrop={} depth={} blur_groups={} mesh_buffers={}",
                                        bool_label(runtime.gpui_has_retained_frame_target),
                                        bool_label(runtime.gpui_has_path_textures),
                                        bool_label(runtime.gpui_has_backdrop_texture),
                                        bool_label(runtime.gpui_has_depth_texture),
                                        runtime.gpui_backdrop_blur_target_groups,
                                        runtime.gpui_gpu_mesh_buffers
                                    )),
                                ),
                                (
                                    SharedString::from("调度器"),
                                    SharedString::from(format!(
                                        "{} wakeup / {:.2} ms idle / refresh {} / effects {} / fallback {}",
                                        runtime.gpui_scheduler_wakeups,
                                        runtime.gpui_idle_sleep_time_ms,
                                        runtime.gpui_coalesced_refresh_count,
                                        runtime.gpui_coalesced_refresh_effect_count,
                                        runtime.gpui_full_redraw_fallback_count
                                    )),
                                ),
                                (
                                    SharedString::from("Surface 健康"),
                                    SharedString::from(format!(
                                        "{} reconfig / {} error / {} partial",
                                        runtime.gpui_gpu_surface_reconfigure_count,
                                        runtime.gpui_gpu_surface_error_count,
                                        runtime.gpui_partial_redraw_count
                                    )),
                                ),
                                (
                                    SharedString::from("Backdrop blur"),
                                    SharedString::from(format!(
                                        "{} primitives / {} target groups",
                                        runtime.gpui_backdrop_blur_primitives,
                                        runtime.gpui_backdrop_blur_target_groups
                                    )),
                                ),
                            ],
                            muted,
                        )
                        .into_any_element(),
                        render_window_metrics_panel(card, border, muted, &runtime).into_any_element(),
                    ]),
            )
            .child(panel_card(
                card,
                border,
                copy.stall_monitor,
                div()
                    .rounded(px(10.))
                    .bg(lerp_color(rgb(0xf8fafc), rgb(0x0b1020), theme_k))
                    .border_1()
                    .border_color(border)
                    .p(px(10.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .line_height(px(17.))
                            .whitespace_normal()
                            .child(stall_preview),
                    ),
            ))
        });

        let active_console_path = match self.console_source {
            ConsoleSource::LatestLog => latest_log_path.clone(),
            ConsoleSource::StallWatch => stall_log_path.clone(),
        };
        let active_console_label = match self.console_source {
            ConsoleSource::LatestLog => copy.latest_log,
            ConsoleSource::StallWatch => copy.stall_log,
        };
        let active_console_tail = match self.console_source {
            ConsoleSource::LatestLog => &self.log_tail,
            ConsoleSource::StallWatch => &self.stall_log_tail,
        };
        let active_console_error = match self.console_source {
            ConsoleSource::LatestLog => self.log_tail_error.clone(),
            ConsoleSource::StallWatch => self.stall_log_error.clone(),
        };
        let active_console_updated_at = match self.console_source {
            ConsoleSource::LatestLog => self.log_tail_last_updated,
            ConsoleSource::StallWatch => self.stall_log_last_updated,
        };

        let console = (self.tab == DebugTab::Console).then(|| {
            panel_card(
                card,
                border,
                copy.console,
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(muted)
                                    .child(copy.log_source),
                            )
                            .child(
                                filter_button(
                                    copy.latest_log,
                                    self.console_source == ConsoleSource::LatestLog,
                                )
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.console_source = ConsoleSource::LatestLog;
                                        cx.notify();
                                    }),
                                ),
                            )
                            .child(
                                filter_button(
                                    copy.stall_log,
                                    self.console_source == ConsoleSource::StallWatch,
                                )
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.console_source = ConsoleSource::StallWatch;
                                        this.console_filter = ConsoleFilter::All;
                                        cx.notify();
                                    }),
                                ),
                            )
                            .child(
                                action_button("debug-open-current-log", copy.open_current_log)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let path = match this.console_source {
                                            ConsoleSource::LatestLog => {
                                                file_ops::logs_dir().join("latest.log")
                                            }
                                            ConsoleSource::StallWatch => {
                                                file_ops::logs_dir().join("ui_foreground_stall.log")
                                            }
                                        };
                                        open_path_in_background(
                                            path,
                                            SharedString::from(format!(
                                                "{}: {}",
                                                copy.opened, copy.open_current_log
                                            )),
                                            cx,
                                        );
                                        cx.notify();
                                    })),
                            )
                            .child(
                                action_button("debug-copy-console", copy.copy_console).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        let text = match this.console_source {
                                            ConsoleSource::LatestLog => this.log_tail.to_string(),
                                            ConsoleSource::StallWatch => {
                                                this.stall_log_tail.to_string()
                                            }
                                        };
                                        if text.trim().is_empty() {
                                            toast::error(
                                                cx,
                                                SharedString::from(copy.console_empty),
                                            );
                                            return;
                                        }
                                        copy_text_to_clipboard(
                                            text,
                                            SharedString::from(format!(
                                                "{}: {}",
                                                copy.copied, copy.copy_console
                                            )),
                                            cx,
                                        );
                                        cx.notify();
                                    }),
                                ),
                            ),
                    )
                    .child(path_block(
                        active_console_label,
                        SharedString::from(active_console_path.to_string_lossy().to_string()),
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(muted)
                                    .child(copy.console_filters),
                            )
                            .child(
                                filter_button(
                                    copy.filter_all,
                                    self.console_filter == ConsoleFilter::All,
                                )
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.console_filter = ConsoleFilter::All;
                                        cx.notify();
                                    }),
                                ),
                            )
                            .child(
                                filter_button(
                                    copy.filter_errors,
                                    self.console_filter == ConsoleFilter::Error,
                                )
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.console_filter = ConsoleFilter::Error;
                                        cx.notify();
                                    }),
                                ),
                            )
                            .child(
                                filter_button(
                                    copy.filter_warnings,
                                    self.console_filter == ConsoleFilter::Warn,
                                )
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.console_filter = ConsoleFilter::Warn;
                                        cx.notify();
                                    }),
                                ),
                            )
                            .child(
                                filter_button(
                                    copy.filter_info,
                                    self.console_filter == ConsoleFilter::Info,
                                )
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.console_filter = ConsoleFilter::Info;
                                        cx.notify();
                                    }),
                                ),
                            )
                            .child(
                                filter_button(
                                    copy.filter_debug,
                                    self.console_filter == ConsoleFilter::Debug,
                                )
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.console_filter = ConsoleFilter::Debug;
                                        cx.notify();
                                    }),
                                ),
                            )
                            .child(
                                filter_button(
                                    copy.filter_trace,
                                    self.console_filter == ConsoleFilter::Trace,
                                )
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.console_filter = ConsoleFilter::Trace;
                                        cx.notify();
                                    }),
                                ),
                            ),
                    )
                    .children(active_console_error.map(|error| {
                        div()
                            .text_size(px(11.))
                            .text_color(lerp_color(rgb(0xb91c1c), rgb(0xfca5a5), theme_k))
                            .whitespace_normal()
                            .child(error)
                    }))
                    .child(
                        div().text_size(px(11.)).text_color(muted).child(
                            active_console_updated_at
                                .map(|updated_at| {
                                    copy.console_updated.replace(
                                        "{ms}",
                                        &now.saturating_duration_since(updated_at)
                                            .as_millis()
                                            .to_string(),
                                    )
                                })
                                .unwrap_or_else(|| copy.console_not_updated.to_string()),
                        ),
                    )
                    .child(render_log_console(
                        active_console_tail,
                        text,
                        border,
                        muted,
                        theme_k,
                        mono,
                        copy,
                        self.console_filter,
                        console_height,
                    )),
            )
        });

        let content = match self.tab {
            DebugTab::Overview => overview.map_or_else(
                || Empty {}.into_any_element(),
                |content| content.into_any_element(),
            ),
            DebugTab::Elements => elements.map_or_else(
                || Empty {}.into_any_element(),
                |content| content.into_any_element(),
            ),
            DebugTab::Performance => performance.map_or_else(
                || Empty {}.into_any_element(),
                |content| content.into_any_element(),
            ),
            DebugTab::Console => console.map_or_else(
                || Empty {}.into_any_element(),
                |content| content.into_any_element(),
            ),
        };

        div().size_full().bg(bg).text_color(text).p(px(14.)).child(
            div()
                .size_full()
                .min_h(px(0.))
                .min_w(px(0.))
                .rounded(px(12.))
                .bg(card)
                .border_1()
                .border_color(border)
                .p(px(14.))
                .flex()
                .flex_col()
                .gap(px(12.))
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .text_size(px(18.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(text)
                                        .child(copy.devtools),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(muted)
                                        .child(copy.subtitle),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(muted)
                                .child(crate::utils::app_info::get_version()),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h(px(0.))
                        .min_w(px(0.))
                        .flex()
                        .when(narrow_layout, |this| this.flex_col())
                        .gap(px(12.))
                        .items_start()
                        .child(navigation)
                        .child(
                            div()
                                .flex_1()
                                .min_h(px(0.))
                                .min_w(px(0.))
                                .overflow_hidden()
                                .overflow_y_scrollbar()
                                .child(content),
                        ),
                ),
        )
    }
}
