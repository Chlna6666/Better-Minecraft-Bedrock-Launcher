//! 动作分发：每个 action id 演示一组宿主能力。

use crate::state::{self, DEFAULT_MODE, TASK_TOTAL};
use crate::{EVENT_PING, PAGE_MAIN, PAGE_MODAL, PAGE_WINDOW, PLUGIN, RESOURCE_NOTICE};
use bmcbl_plugin_api::prelude::*;

plugin_actions! {
    pub enum ShowcaseAction {
        Increment = "increment",
        Reset = "reset",
        ToggleAdvanced = "toggle-advanced",
        SetMode = "set-mode",
        SaveConfig = "save-config",
        ReloadConfig = "reload-config",
        LoadNotes = "load-notes",
        SaveNotes = "save-notes",
        FetchSite = "fetch-site",
        CopySummary = "copy-summary",
        PasteClipboard = "paste-clipboard",
        ReadNotice = "read-notice",
        StartTask = "start-task",
        StepTask = "step-task",
        FinishTask = "finish-task",
        OpenWindow = "open-window",
        OpenModal = "open-modal",
        OpenDocs = "open-docs",
        GoSettings = "go-settings",
        EmitPing = "emit-ping",
        InvalidateAll = "invalidate-all",
    }
}

/// 动作处理器签名：可选的动作值来自按钮的 `action_value` 或下拉框选项。
type ActionHandler = fn(Option<&str>) -> PluginResult<()>;

/// 动作表：新增动作只需在这里加一行，并实现对应的处理器。
const ACTIONS: &[(&str, ActionHandler)] = &[
    (ShowcaseAction::Increment.as_str(), increment),
    (ShowcaseAction::Reset.as_str(), reset),
    (ShowcaseAction::ToggleAdvanced.as_str(), toggle_advanced),
    (ShowcaseAction::SetMode.as_str(), set_mode),
    (ShowcaseAction::SaveConfig.as_str(), save_config),
    (ShowcaseAction::ReloadConfig.as_str(), reload_config),
    (ShowcaseAction::LoadNotes.as_str(), load_notes),
    (ShowcaseAction::SaveNotes.as_str(), save_notes),
    (ShowcaseAction::FetchSite.as_str(), fetch_site),
    (ShowcaseAction::CopySummary.as_str(), copy_summary),
    (ShowcaseAction::PasteClipboard.as_str(), paste_clipboard),
    (ShowcaseAction::ReadNotice.as_str(), read_notice),
    (ShowcaseAction::StartTask.as_str(), start_task),
    (ShowcaseAction::StepTask.as_str(), step_task),
    (ShowcaseAction::FinishTask.as_str(), finish_task),
    (ShowcaseAction::OpenWindow.as_str(), open_window_page),
    (ShowcaseAction::OpenModal.as_str(), open_modal_page),
    (ShowcaseAction::OpenDocs.as_str(), open_docs),
    (ShowcaseAction::GoSettings.as_str(), go_settings),
    (ShowcaseAction::EmitPing.as_str(), emit_ping),
    (ShowcaseAction::InvalidateAll.as_str(), invalidate_everything),
];

/// 处理宿主事件：全局事件、路由变化与动作。
pub fn handle(event: HostEvent) -> PluginResult<()> {
    if let Some((name, payload)) = event.global_event() {
        return handle_global(name, payload);
    }
    if let Some(path) = event.route_path() {
        state::set_route(path)?;
        // 路由变化会影响侧边栏注入，主动失效它的缓存。
        return invalidate!(injection InjectionSlot::HomeSidebar, page = "/");
    }
    let action_id = event.action_id().unwrap_or_default().to_string();
    let action_value = match &event.kind {
        HostEventKind::Action(action) => action.value.clone(),
        HostEventKind::RouteChanged(_) | HostEventKind::Global(_) => None,
    };

    for &(action, handler) in ACTIONS {
        if action_id == action {
            return handler(action_value.as_deref());
        }
    }
    log_warn!("unknown action {action_id}");
    Ok(())
}

fn handle_global(name: &str, payload: &str) -> PluginResult<()> {
    if name != EVENT_PING {
        return Ok(());
    }
    let count = state::bump_ping()?;
    log_debug!("ping #{count} payload={payload}");
    refresh_main()
}

fn refresh_main() -> PluginResult<()> {
    invalidate!(page PAGE_MAIN)
}

fn increment(_value: Option<&str>) -> PluginResult<()> {
    state::set_counter(state::counter().saturating_add(1))?;
    state::persist()?;
    toast!(success, tr!("showcase.toast.incremented"))?;
    refresh_main()
}

fn reset(_value: Option<&str>) -> PluginResult<()> {
    state::set_counter(0)?;
    storage_delete(state::KEY_COUNTER)?;
    toast!(info, tr!("showcase.toast.reset"))?;
    refresh_main()
}

fn toggle_advanced(_value: Option<&str>) -> PluginResult<()> {
    state::toggle_advanced()?;
    refresh_main()
}

fn set_mode(value: Option<&str>) -> PluginResult<()> {
    state::set_mode(value.unwrap_or(DEFAULT_MODE))?;
    refresh_main()
}

/// 写回插件配置。键名与 `config/schema.toml` 一致，宿主也用同一份 schema 生成设置表单。
fn save_config(_value: Option<&str>) -> PluginResult<()> {
    let text = format!(
        concat!(
            "# 由 BMCBL Showcase 写回\n",
            "show_overlay_badge = {}\n",
            "display_mode = \"{}\"\n",
            "max_status_items = {}\n",
            "custom_status = \"{}\"\n"
        ),
        state::advanced(),
        state::mode(),
        TASK_TOTAL,
        state::route()
    );
    config_write(&text)?;
    state::set_config_text(&text)?;
    toast!(success, tr!("showcase.toast.config_saved"))?;
    refresh_main()
}

fn reload_config(_value: Option<&str>) -> PluginResult<()> {
    let text = read_config()?;
    state::set_config_text(&text)?;
    toast!(info, tr!("showcase.toast.config_loaded"))?;
    refresh_main()
}

fn load_notes(_value: Option<&str>) -> PluginResult<()> {
    // storage_* 读取的是宿主内存里的快照，不会在渲染期间触发磁盘 IO。
    let keys = storage_list(None::<&str>)?;
    let notes = storage_get(state::KEY_NOTES)?.unwrap_or_default();
    state::set_notes(&notes)?;
    log_info!("storage keys: {}", keys.join(", "));
    toast!(info, tr!("showcase.toast.notes_loaded"))?;
    refresh_main()
}

fn save_notes(value: Option<&str>) -> PluginResult<()> {
    let text = value.unwrap_or_default();
    state::set_notes(text)?;
    storage_set(state::KEY_NOTES, text)?;
    toast!(success, tr!("showcase.toast.notes_saved"))?;
    refresh_main()
}

fn fetch_site(_value: Option<&str>) -> PluginResult<()> {
    // HTTP 走宿主的异步缓存：首次调用通常拿到 loading，结果就绪后宿主会失效页面。
    let response = http_get_text("https://bmcbl.com/", 600, 65536)?;
    let summary = match response.state {
        HttpCacheState::Fresh | HttpCacheState::Stale => {
            let body = response.body.unwrap_or_default();
            format!("{} bytes | {}", body.len(), first_line(&body))
        }
        HttpCacheState::Loading => tr!("showcase.remote.loading"),
        HttpCacheState::Error => {
            let error = response.error.unwrap_or_default();
            format!("{} | {error}", tr!("showcase.remote.error"))
        }
    };
    state::set_remote(&summary)?;
    refresh_main()
}

fn first_line(body: &str) -> String {
    body.lines().next().unwrap_or_default().trim().to_string()
}

fn copy_summary(_value: Option<&str>) -> PluginResult<()> {
    write_clipboard_text(state::summary())?;
    toast!(success, tr!("showcase.toast.copied"))?;
    Ok(())
}

fn paste_clipboard(_value: Option<&str>) -> PluginResult<()> {
    let text = read_clipboard_text()?.unwrap_or_default();
    let preview: String = text.chars().take(120).collect();
    state::set_clipboard_preview(&preview)?;
    toast!(info, tr!("showcase.toast.pasted"))?;
    refresh_main()
}

fn read_notice(_value: Option<&str>) -> PluginResult<()> {
    match read_resource_text(RESOURCE_NOTICE) {
        Ok(text) => {
            state::set_notice(text.trim())?;
            toast!(success, tr!("showcase.toast.notice"))?;
        }
        // 首次读取资源时宿主会返回加载中，读取完成后自动失效页面。
        Err(error) if error.code == "resource-loading" => {
            state::set_notice(&tr!("showcase.notice.loading"))?;
        }
        Err(error) => return Err(error),
    }
    refresh_main()
}

fn start_task(_value: Option<&str>) -> PluginResult<()> {
    let task_id = create_task(TaskCreateRequest {
        task_id: None,
        title: tr!("showcase.task.title"),
        detail: Some(tr!("showcase.task.detail")),
        stage: tr!("showcase.task.starting"),
        total: Some(TASK_TOTAL),
        supports_pause: false,
    })?;
    state::set_task_id(Some(&task_id))?;
    state::set_task_done(0)?;
    toast!(info, tr!("showcase.toast.task_started"))?;
    refresh_main()
}

fn step_task(_value: Option<&str>) -> PluginResult<()> {
    let Some(task_id) = state::task_id() else {
        return start_task(None);
    };
    let done = state::task_done().saturating_add(1).min(TASK_TOTAL);
    update_task(TaskUpdateRequest {
        task_id,
        stage: Some(tr!("showcase.task.running")),
        total: Some(TASK_TOTAL),
        done_delta: 1,
        message: Some(tr!("showcase.task.step", "done" => done)),
    })?;
    state::set_task_done(done)?;
    if done >= TASK_TOTAL {
        return finish_task(None);
    }
    refresh_main()
}

fn finish_task(_value: Option<&str>) -> PluginResult<()> {
    let Some(task_id) = state::task_id() else {
        return Ok(());
    };
    // `finish_task` 是宿主 API；这里的同名动作处理器通过全路径调用避免歧义。
    bmcbl_plugin_api::finish_task(TaskFinishRequest {
        task_id,
        status: "completed".to_string(),
        message: Some(tr!("showcase.task.finished")),
    })?;
    state::set_task_id(None)?;
    toast!(success, tr!("showcase.toast.task_finished"))?;
    refresh_main()
}

fn open_window_page(_value: Option<&str>) -> PluginResult<()> {
    PLUGIN
        .window(PAGE_WINDOW)
        .title(tr!("showcase.window.title"))
        .size(720, 480)
        .resizable(true)
        .open(PLUGIN)?;
    toast!(info, tr!("showcase.toast.window"))?;
    Ok(())
}

fn open_modal_page(_value: Option<&str>) -> PluginResult<()> {
    PLUGIN
        .modal(PAGE_MODAL)
        .title(tr!("showcase.modal.title"))
        .size(520, 360)
        .open(PLUGIN)?;
    toast!(info, tr!("showcase.toast.modal"))?;
    Ok(())
}

fn open_docs(_value: Option<&str>) -> PluginResult<()> {
    open_external_url("https://bmcbl.com/")?;
    toast!(info, tr!("showcase.toast.docs"))?;
    Ok(())
}

fn go_settings(_value: Option<&str>) -> PluginResult<()> {
    navigate_path("/settings")?;
    toast!(info, tr!("showcase.toast.settings"))?;
    Ok(())
}

fn emit_ping(_value: Option<&str>) -> PluginResult<()> {
    emit_event(EVENT_PING, &state::summary())?;
    Ok(())
}

fn invalidate_everything(_value: Option<&str>) -> PluginResult<()> {
    invalidate_all()?;
    toast!(info, tr!("showcase.toast.invalidated"))?;
    Ok(())
}
