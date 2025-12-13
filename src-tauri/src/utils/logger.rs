use crate::config::config::read_config;
use chrono::Local;
use once_cell::sync::Lazy;
use std::fs::{create_dir_all, OpenOptions};
use std::time::Instant;
use tracing::{debug, error, info, warn};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

// 程序启动时间
static START_TIME: Lazy<Instant> = Lazy::new(Instant::now);

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

// 日志命令接口，支持多级日志写入
#[tauri::command]
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
pub fn init_logging() {
    let logs_dir = "BMCBL/logs";
    let latest_log_file = format!("{}/latest.log", logs_dir);

    // 确保日志目录存在
    if let Err(e) = create_dir_all(logs_dir) {
        eprintln!("Failed to create logs directory: {}", e);
        return;
    }

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

    // 判断是否启用 debug 日志
    let debug_enabled = match read_config() {
        Ok(config) => config.launcher.debug,
        Err(err) => {
            eprintln!("Failed to read config, defaulting to info logging: {}", err);
            false
        }
    };

    // 设置日志级别
    let log_level = if debug_enabled { "debug" } else { "info" };

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
        .with_writer(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(format!(
                    "{}/{}.log",
                    logs_dir,
                    Local::now().format("%Y-%m-%d")
                ))
                .unwrap(),
        ); // 明确指定文件输出

    // 文件层 - latest.log
    let latest_log_layer = tracing_subscriber::fmt::layer()
        .with_timer(UptimeTimer)
        .with_ansi(false) // 文件无 ANSI 转义
        .with_target(true) // 显示目标模块
        .with_writer(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&latest_log_file)
                .unwrap(),
        ); // 明确指定文件输出

    // 初始化日志订阅器
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level)), // 根据配置设置日志级别
        )
        .with(console_layer) // 控制台日志层
        .with(file_layer) // 按日期日志文件层
        .with(latest_log_layer) // 最新日志文件层
        .init(); // 初始化日志系统
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_logging_init() {
        init_logging();
        info!("这是 info 测试日志");
        debug!("这是 debug 测试日志");
        warn!("这是 warn 测试日志");
        error!("这是 error 测试日志");
    }
}
