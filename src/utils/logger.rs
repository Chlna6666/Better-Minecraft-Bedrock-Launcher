use crate::utils::diagnostics;
use crate::utils::file_ops;
use chrono::Local;
use once_cell::sync::{Lazy, OnceCell};
use std::collections::HashMap;
use std::fs::{self, OpenOptions, create_dir_all};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber, debug, error, info, warn};
use tracing_log::LogTracer;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::layer::Context;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

// 程序启动时间
static START_TIME: Lazy<Instant> = Lazy::new(Instant::now);

const LOG_THROTTLE_WINDOW: Duration = Duration::from_secs(3);
const LOG_THROTTLE_RETENTION: Duration = Duration::from_secs(120);
const LOG_THROTTLE_CLEANUP_INTERVAL: Duration = Duration::from_secs(15);
const LOG_THROTTLE_MAX_TRACKED: usize = 4096;
const PREVIOUS_LOG_FILE: &str = "previous.log";
const DEBUG_LOG_FILTER: &str = "bmcbl=debug,bmcbl::ui::window::map_viewer=trace,bedrock_leveldb=trace,bedrock_world=trace,bedrock_render=trace,gpui=debug,reqwest=info,hyper=warn,hyper_util=warn,h2=warn,rustls=warn,info";

static LOG_THROTTLE_STATE: Lazy<Mutex<LogThrottleState>> =
    Lazy::new(|| Mutex::new(LogThrottleState::default()));
static LATEST_LOG_PATH: OnceCell<PathBuf> = OnceCell::new();

#[derive(Default)]
struct LogThrottleState {
    entries: HashMap<LogThrottleKey, LogThrottleEntry>,
    last_cleanup: Option<Instant>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct LogThrottleKey {
    level: Level,
    target: String,
    fields: String,
}

#[derive(Clone, Debug)]
struct LogThrottleEntry {
    last_emitted: Instant,
    last_seen: Instant,
    suppressed: u32,
}

impl LogThrottleState {
    fn cleanup_if_needed(&mut self, now: Instant) {
        let should_cleanup = match self.last_cleanup {
            Some(last_cleanup) => now.duration_since(last_cleanup) >= LOG_THROTTLE_CLEANUP_INTERVAL,
            None => true,
        } || self.entries.len() > LOG_THROTTLE_MAX_TRACKED;

        if !should_cleanup {
            return;
        }

        self.entries
            .retain(|_, entry| now.duration_since(entry.last_seen) <= LOG_THROTTLE_RETENTION);
        self.last_cleanup = Some(now);

        if self.entries.len() <= LOG_THROTTLE_MAX_TRACKED {
            return;
        }

        let mut by_last_seen: Vec<_> = self
            .entries
            .iter()
            .map(|(key, entry)| (key.clone(), entry.last_seen))
            .collect();
        by_last_seen.sort_by_key(|(_, last_seen)| *last_seen);

        let overflow = by_last_seen.len().saturating_sub(LOG_THROTTLE_MAX_TRACKED);
        for (key, _) in by_last_seen.into_iter().take(overflow) {
            self.entries.remove(&key);
        }
    }
}

#[derive(Default)]
struct LogThrottleVisitor {
    fields: Vec<String>,
}

impl LogThrottleVisitor {
    fn push_value(&mut self, field: &Field, value: String) {
        self.fields.push(format!("{}={}", field.name(), value));
    }
}

impl Visit for LogThrottleVisitor {
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.push_value(field, value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.push_value(field, value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.push_value(field, value.to_string());
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.push_value(field, value.to_owned());
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.push_value(field, value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.push_value(field, format!("{:?}", value));
    }
}

fn should_throttle_level(level: &Level) -> bool {
    matches!(*level, Level::ERROR | Level::WARN)
}

fn build_log_throttle_key(event: &Event<'_>) -> LogThrottleKey {
    let metadata = event.metadata();
    let mut visitor = LogThrottleVisitor::default();
    event.record(&mut visitor);

    LogThrottleKey {
        level: *metadata.level(),
        target: metadata.target().to_owned(),
        fields: visitor.fields.join(" | "),
    }
}

#[derive(Clone, Debug)]
struct LogThrottleLayer {
    window: Duration,
}

impl Default for LogThrottleLayer {
    fn default() -> Self {
        Self {
            window: LOG_THROTTLE_WINDOW,
        }
    }
}

impl<S> Layer<S> for LogThrottleLayer
where
    S: Subscriber,
{
    fn event_enabled(&self, event: &Event<'_>, _ctx: Context<'_, S>) -> bool {
        let level = event.metadata().level();
        if !should_throttle_level(level) {
            return true;
        }

        let now = Instant::now();
        let key = build_log_throttle_key(event);
        let mut state = LOG_THROTTLE_STATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        state.cleanup_if_needed(now);

        match state.entries.get_mut(&key) {
            Some(entry) => {
                entry.last_seen = now;
                if now.duration_since(entry.last_emitted) < self.window {
                    entry.suppressed = entry.suppressed.saturating_add(1);
                    return false;
                }

                entry.last_emitted = now;
                entry.suppressed = 0;
                true
            }
            None => {
                state.entries.insert(
                    key,
                    LogThrottleEntry {
                        last_emitted: now,
                        last_seen: now,
                        suppressed: 0,
                    },
                );
                true
            }
        }
    }
}

// 自定义启动时间计时器
struct UptimeTimer;

impl FormatTime for UptimeTimer {
    fn format_time(&self, w: &mut Writer<'_>) -> Result<(), std::fmt::Error> {
        write!(w, "{}", elapsed_time()) // 直接返回 `write!` 的结果
    }
}

// 返回程序启动后的运行时间
fn elapsed_time() -> String {
    let elapsed = START_TIME.elapsed();
    let millis = elapsed.as_millis();
    let seconds = millis / 1000;
    let minutes = seconds / 60;
    let hours = minutes / 60;

    let millis = millis % 1000;
    let seconds = seconds % 60;
    let minutes = minutes % 60;
    let hours = hours % 24;

    format!("{:02}:{:02}:{:02}.{:03}", hours, minutes, seconds, millis)
}

fn append_crash_line(message: &str) {
    let Some(path) = LATEST_LOG_PATH.get() else {
        eprintln!("{message}");
        return;
    };

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{message}");
        let _ = file.flush();
    } else {
        eprintln!("{message}");
    }
}

fn backup_previous_latest_log(logs_dir: &std::path::Path, latest_log_file: &std::path::Path) {
    let previous_log_file = logs_dir.join(PREVIOUS_LOG_FILE);
    if !latest_log_file.exists() {
        return;
    }

    if let Err(error) = fs::copy(latest_log_file, &previous_log_file) {
        eprintln!(
            "Failed to back up previous latest.log to {}: {}",
            previous_log_file.display(),
            error
        );
    }
}

fn default_log_filter(debug_enabled: bool) -> String {
    let log_level = if debug_enabled {
        // Keep project diagnostics rich, but avoid frame-level network logs from dependencies.
        DEBUG_LOG_FILTER
    } else {
        "info"
    };
    format!("{log_level},blade_graphics=warn")
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        let location = panic_info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()))
            .unwrap_or_else(|| "unknown".to_string());
        let payload = if let Some(payload) = panic_info.payload().downcast_ref::<&str>() {
            (*payload).to_string()
        } else if let Some(payload) = panic_info.payload().downcast_ref::<String>() {
            payload.clone()
        } else {
            "non-string panic payload".to_string()
        };
        let backtrace = std::backtrace::Backtrace::force_capture();
        append_crash_line(&format!(
            "{} ERROR BMCBL::utils::logger: panic captured: location={} payload={} backtrace={backtrace}",
            elapsed_time(),
            location,
            payload
        ));
        if let Err(error) =
            diagnostics::record_panic_signal(Some(location), payload, Some(backtrace.to_string()))
        {
            eprintln!("Failed to persist panic diagnostics: {error:#}");
        }
    }));
}

#[cfg(windows)]
fn install_unhandled_exception_hook() {
    use windows::Win32::System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_EXECUTE_HANDLER,
        EXCEPTION_POINTERS, SetUnhandledExceptionFilter,
    };

    fn exception_summary(exception_info: *const EXCEPTION_POINTERS) -> (u32, usize) {
        if exception_info.is_null() {
            return (0_u32, 0_usize);
        }

        // SAFETY: The OS invokes this callback with a valid pointer for the current crash.
        let record = unsafe { (*exception_info).ExceptionRecord };
        if record.is_null() {
            return (0_u32, 0_usize);
        }

        // SAFETY: `record` comes from the OS exception callback and is valid for this call.
        let record = unsafe { &*record };
        (
            record.ExceptionCode.0 as u32,
            record.ExceptionAddress as usize,
        )
    }

    unsafe extern "system" fn handle_vectored_exception(
        exception_info: *mut EXCEPTION_POINTERS,
    ) -> i32 {
        let (code, address) = exception_summary(exception_info);
        append_crash_line(&format!(
            "{} ERROR BMCBL::utils::logger: vectored exception captured: code=0x{:08X} address=0x{:X}",
            elapsed_time(),
            code,
            address
        ));
        EXCEPTION_CONTINUE_SEARCH
    }

    unsafe extern "system" fn handle_unhandled_exception(
        exception_info: *const EXCEPTION_POINTERS,
    ) -> i32 {
        let (code, address) = exception_summary(exception_info);
        append_crash_line(&format!(
            "{} ERROR BMCBL::utils::logger: unhandled exception captured: code=0x{:08X} address=0x{:X}",
            elapsed_time(),
            code,
            address
        ));
        let _ = diagnostics::record_unhandled_exception_signal(code, address);

        EXCEPTION_EXECUTE_HANDLER
    }

    // SAFETY: Installing a single process-wide unhandled exception filter is intended Win32 usage.
    unsafe {
        let _ = AddVectoredExceptionHandler(1, Some(handle_vectored_exception));
        let _ = SetUnhandledExceptionFilter(Some(handle_unhandled_exception));
    }
}

#[cfg(not(windows))]
fn install_unhandled_exception_hook() {}

// 日志接口，支持多级日志写入
pub fn log(level: &str, message: &str) {
    match level {
        "info" => info!("{}", message),
        "warning" | "warn" => warn!("{}", message),
        "error" => error!("{}", message),
        "debug" => debug!("{}", message),
        _ => info!("{}", message), // 默认使用 info
    }
}

// 初始化日志系统
pub fn init_logging(debug_enabled: bool) {
    let logs_dir: PathBuf = file_ops::bmcbl_subdir("logs");
    let latest_log_file = logs_dir.join("latest.log");
    let daily_log_file = logs_dir.join(format!("{}.log", Local::now().format("%Y-%m-%d")));
    let _ = LATEST_LOG_PATH.set(latest_log_file.clone());

    // 确保日志目录存在
    if let Err(e) = create_dir_all(&logs_dir) {
        eprintln!("Failed to create logs directory: {}", e);
        return;
    }

    backup_previous_latest_log(&logs_dir, &latest_log_file);

    // 清空 `latest.log`
    if let Err(e) = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true) // 清空文件内容
        .open(&latest_log_file)
    {
        eprintln!("Failed to clear latest.log: {}", e);
        return;
    }

    let daily_log_writer = match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&daily_log_file)
    {
        Ok(file) => file,
        Err(error) => {
            eprintln!(
                "Failed to open daily log file {}: {}",
                daily_log_file.display(),
                error
            );
            return;
        }
    };

    let latest_log_writer = match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&latest_log_file)
    {
        Ok(file) => file,
        Err(error) => {
            eprintln!(
                "Failed to open latest log file {}: {}",
                latest_log_file.display(),
                error
            );
            return;
        }
    };

    // 控制台层
    let console_layer = tracing_subscriber::fmt::layer()
        .with_timer(UptimeTimer) // 使用启动时间计时器
        .with_ansi(true) // 控制台输出时使用 ANSI 转义字符
        .with_target(true); // 显示目标模块

    // 文件层 - 按日期记录日志
    let file_layer = tracing_subscriber::fmt::layer()
        .with_timer(UptimeTimer)
        .with_ansi(false) // 文件无 ANSI 转义
        .with_target(true) // 显示目标模块
        .with_writer(daily_log_writer); // 明确指定文件输出

    // 文件层 - latest.log
    let latest_log_layer = tracing_subscriber::fmt::layer()
        .with_timer(UptimeTimer)
        .with_ansi(false) // 文件无 ANSI 转义
        .with_target(true) // 显示目标模块
        .with_writer(latest_log_writer); // 明确指定文件输出

    // 初始化日志订阅器
    let _ = LogTracer::init();
    let default_filter = default_log_filter(debug_enabled);
    let filter = EnvFilter::new(default_filter);

    let registry = tracing_subscriber::registry()
        .with(filter) // 根据配置设置日志级别
        .with(LogThrottleLayer::default()) // 短时间内抑制重复 warn/error 日志风暴
        .with(console_layer) // 控制台日志层
        .with(file_layer) // 按日期日志文件层
        .with(latest_log_layer); // 最新日志文件层

    if let Err(error) = registry.try_init() {
        eprintln!("Failed to initialize tracing subscriber: {error}");
        return;
    }

    install_panic_hook();
    install_unhandled_exception_hook();

    info!("Logging initialized");
    info!("Debug logging enabled: {}", debug_enabled);
    info!(
        "Log files ready: latest={}, daily={}",
        latest_log_file.display(),
        daily_log_file.display()
    );
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_log_filter_uses_current_crate_target() {
        assert!(DEBUG_LOG_FILTER.contains("bmcbl=debug"));
        assert!(DEBUG_LOG_FILTER.contains("bmcbl::ui::window::map_viewer=trace"));
        assert!(!DEBUG_LOG_FILTER.contains("BMCBL=debug"));
        assert!(!DEBUG_LOG_FILTER.contains("BMCBL::"));
    }

    #[test]
    fn default_debug_filter_enables_bmcbl_debug_logs() {
        let filter = default_log_filter(true);
        assert!(filter.contains("bmcbl=debug"));
        assert!(filter.contains("gpui=debug"));
        assert!(filter.contains("blade_graphics=warn"));
    }

    #[test]
    fn default_non_debug_filter_stays_at_info() {
        assert_eq!(default_log_filter(false), "info,blade_graphics=warn");
    }

    #[test]
    fn test_logging_init() {
        init_logging(true);
        info!("这是 info 测试日志");
        debug!("这是 debug 测试日志");
        warn!("这是 warn 测试日志");
        error!("这是 error 测试日志");
    }
}
