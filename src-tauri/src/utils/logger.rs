use chrono::Local;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::time::Instant;

#[derive(Serialize, Deserialize)]
pub enum LogLevel {
    Info,
    Warning,
    Error,
    Debug,
}

// 记录程序启动时的时间
static START_TIME: Lazy<Instant> = Lazy::new(Instant::now);

pub fn clear_latest_log() {
    let logs_dir = "BMCBL/logs";
    let latest_log_file = format!("{}/latest.log", logs_dir);
    create_dir_all(logs_dir).expect("Unable to create logs directory");
    // 清空 latest.log 文件的内容
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true) // 清空文件内容
        .open(latest_log_file)
        .expect("Unable to open latest log file");
}

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

#[tauri::command]
pub fn log(level: String, message: &str) {
    let log_level = match level.as_str() {
        "Info" => LogLevel::Info,
        "Warning" => LogLevel::Warning,
        "Error" => LogLevel::Error,
        "Debug" => LogLevel::Debug,
        _ => LogLevel::Info, // 默认情况下使用 Info 级别
    };

    let timestamp = elapsed_time();

    let (log_level_str, log_level_str_no_color) = match log_level {
        LogLevel::Info => ("\x1b[32mINFO\x1b[0m", "INFO"),
        LogLevel::Warning => ("\x1b[33mWARNING\x1b[0m", "WARNING"),
        LogLevel::Error => ("\x1b[31mERROR\x1b[0m", "ERROR"),
        LogLevel::Debug => ("\x1b[34mDEBUG\x1b[0m", "DEBUG"),
    };

    // 终端输出带颜色的日志
    println!("[{}] {} {}", timestamp, log_level_str, message);

    // 创建 logs 目录
    let logs_dir = "BMCBL/logs";
    create_dir_all(logs_dir).expect("Unable to create logs directory");

    // 生成按年-月-日格式的日志文件名
    let log_file_name = format!("{}/{}.log", logs_dir, Local::now().format("%Y-%m-%d"));

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file_name)
        .expect("Unable to open log file");

    // 文件写入不带颜色的日志
    writeln!(
        &mut file,
        "[{}] [{}] {}",
        timestamp, log_level_str_no_color, message
    )
    .expect("Unable to write to log file");

    // 处理 latest.log 文件
    let latest_log_file = format!("{}/latest.log", logs_dir);
    let mut latest_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(latest_log_file)
        .expect("Unable to open latest log file");

    // 文件写入最新的日志
    writeln!(
        latest_file,
        "[{}] [{}] {}",
        timestamp, log_level_str_no_color, message
    )
    .expect("Unable to write to latest log file");
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        $crate::utils::logger::log("Info".to_string(), &format!($($arg)*));
    };
}

#[macro_export]
macro_rules! warning {
    ($($arg:tt)*) => {
        $crate::utils::logger::log("Warning".to_string(), &format!($($arg)*));
    };
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {
        $crate::utils::logger::log("Error".to_string(), &format!($($arg)*));
    };
}

#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
        $crate::utils::logger::log("Debug".to_string(), &format!($($arg)*));
    };
}
