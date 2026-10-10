//! 插件状态：会话内的即时值放在 session，需要跨启动保留的值放在 storage。
//!
//! `render_*` 只读取这里的会话值。持久化、配置和资源读取都发生在 `init`、`handle_event`
//! 与 `shutdown`，因为宿主会拒绝在渲染阶段写磁盘。

use bmcbl_plugin_api::prelude::*;

/// 持久化存储键：计数器。
pub const KEY_COUNTER: &str = "counter";
/// 持久化存储键：备注。
pub const KEY_NOTES: &str = "notes";
/// 持久化存储键：最近一次保存时间。
pub const KEY_SAVED_AT: &str = "saved_at";

const SESSION_COUNTER: &str = "showcase.counter";
const SESSION_ADVANCED: &str = "showcase.advanced";
const SESSION_MODE: &str = "showcase.mode";
const SESSION_ROUTE: &str = "showcase.route";
const SESSION_REMOTE: &str = "showcase.remote";
const SESSION_CLIPBOARD: &str = "showcase.clipboard";
const SESSION_NOTICE: &str = "showcase.notice";
const SESSION_NOTES: &str = "showcase.notes";
const SESSION_CONFIG: &str = "showcase.config";
const SESSION_TASK: &str = "showcase.task";
const SESSION_TASK_DONE: &str = "showcase.task_done";
const SESSION_PING: &str = "showcase.ping";

/// 默认模式，与 `config/default.toml` 中的 `display_mode` 保持一致。
pub const DEFAULT_MODE: &str = "balanced";
/// 示例任务的总步数。
pub const TASK_TOTAL: u64 = 5;

fn session_text(key: &str) -> String {
    session_get(key).ok().flatten().unwrap_or_default()
}

fn session_number(key: &str) -> u64 {
    session_text(key).parse().unwrap_or(0)
}

fn set_session_text(key: &str, value: &str) -> PluginResult<()> {
    session_set(key, Some(value))
}

fn set_session_number(key: &str, value: u64) -> PluginResult<()> {
    set_session_text(key, &value.to_string())
}

/// 从插件存储恢复持久值；在 `init` 中调用一次。
pub fn restore() -> PluginResult<()> {
    let counter = storage_get(KEY_COUNTER)?
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    set_session_number(SESSION_COUNTER, counter)?;
    let notes = storage_get(KEY_NOTES)?.unwrap_or_default();
    set_session_text(SESSION_NOTES, &notes)?;
    set_session_text(SESSION_MODE, DEFAULT_MODE)
}

/// 把会话值写回插件存储；保存动作与 `shutdown` 都会调用。
pub fn persist() -> PluginResult<()> {
    storage_set(KEY_COUNTER, counter().to_string())?;
    storage_set(KEY_NOTES, &notes())?;
    storage_set(
        KEY_SAVED_AT,
        current_unix_ms().unwrap_or_default().to_string(),
    )?;
    Ok(())
}

pub fn counter() -> u64 {
    session_number(SESSION_COUNTER)
}

pub fn set_counter(value: u64) -> PluginResult<()> {
    set_session_number(SESSION_COUNTER, value)
}

pub fn advanced() -> bool {
    session_text(SESSION_ADVANCED) == "true"
}

pub fn toggle_advanced() -> PluginResult<bool> {
    let next = !advanced();
    set_session_text(SESSION_ADVANCED, if next { "true" } else { "false" })?;
    Ok(next)
}

pub fn mode() -> String {
    let value = session_text(SESSION_MODE);
    if value.is_empty() {
        DEFAULT_MODE.to_string()
    } else {
        value
    }
}

pub fn set_mode(value: &str) -> PluginResult<()> {
    if value.is_empty() {
        return set_session_text(SESSION_MODE, DEFAULT_MODE);
    }
    set_session_text(SESSION_MODE, value)
}

pub fn route() -> String {
    let value = session_text(SESSION_ROUTE);
    if value.is_empty() {
        "/".to_string()
    } else {
        value
    }
}

pub fn set_route(value: &str) -> PluginResult<()> {
    set_session_text(SESSION_ROUTE, value)
}

pub fn remote() -> String {
    session_text(SESSION_REMOTE)
}

pub fn set_remote(value: &str) -> PluginResult<()> {
    set_session_text(SESSION_REMOTE, value)
}

pub fn clipboard_preview() -> String {
    session_text(SESSION_CLIPBOARD)
}

pub fn set_clipboard_preview(value: &str) -> PluginResult<()> {
    set_session_text(SESSION_CLIPBOARD, value)
}

pub fn notice() -> String {
    session_text(SESSION_NOTICE)
}

pub fn set_notice(value: &str) -> PluginResult<()> {
    set_session_text(SESSION_NOTICE, value)
}

pub fn notes() -> String {
    session_text(SESSION_NOTES)
}

pub fn set_notes(value: &str) -> PluginResult<()> {
    set_session_text(SESSION_NOTES, value)
}

pub fn config_text() -> String {
    session_text(SESSION_CONFIG)
}

pub fn set_config_text(value: &str) -> PluginResult<()> {
    set_session_text(SESSION_CONFIG, value)
}

pub fn task_id() -> Option<String> {
    let value = session_text(SESSION_TASK);
    (!value.is_empty()).then_some(value)
}

pub fn set_task_id(value: Option<&str>) -> PluginResult<()> {
    set_session_text(SESSION_TASK, value.unwrap_or_default())
}

pub fn task_done() -> u64 {
    session_number(SESSION_TASK_DONE)
}

pub fn set_task_done(value: u64) -> PluginResult<()> {
    set_session_number(SESSION_TASK_DONE, value)
}

pub fn ping_count() -> u64 {
    session_number(SESSION_PING)
}

/// 记录一次插件自定义事件，返回累计次数。
pub fn bump_ping() -> PluginResult<u64> {
    let next = ping_count().saturating_add(1);
    set_session_number(SESSION_PING, next)?;
    Ok(next)
}

/// 会话语义的两行摘要，用于剪贴板与注入视图。
pub fn summary() -> String {
    format!(
        "counter={} mode={} route={} ping={}",
        counter(),
        mode(),
        route(),
        ping_count()
    )
}
