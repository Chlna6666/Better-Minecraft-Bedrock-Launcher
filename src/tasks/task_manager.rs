use once_cell::sync::Lazy;
use std::future::Future;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio::sync::{Notify, broadcast};
use tokio::task::AbortHandle;
use tracing::{debug, error, info, warn};

const TASK_EMIT_INTERVAL_MS: u128 = 100;
const TASK_EVENT_BATCH_LIMIT: usize = 256;
const TASK_PROGRESS_MIN_UPDATE_INTERVAL: f64 = 0.10;
const TASK_PROGRESS_IDLE_RESET_SECONDS: f64 = 1.0;
const TASK_PROGRESS_EMA_ALPHA: f64 = 0.2;
const TASK_VISUALIZATION_ENABLED: bool = true;
const TASK_LOG_LIMIT: usize = 128;
type TaskCancelHook = Box<dyn Fn() + Send + Sync + 'static>;

static TASK_COUNTER: AtomicU64 = AtomicU64::new(1);
static TASKS: Lazy<Mutex<HashMap<String, Task>>> = Lazy::new(|| Mutex::new(HashMap::new()));
static TASK_SNAPSHOTS: Lazy<RwLock<HashMap<Arc<str>, Arc<TaskSnapshot>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));
static TASK_CONTROLS: Lazy<RwLock<HashMap<String, Arc<TaskControl>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));
static TASK_ABORT_HANDLES: Lazy<Mutex<HashMap<String, AbortHandle>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static TASK_CANCEL_HOOKS: Lazy<Mutex<HashMap<String, TaskCancelHook>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static TASK_COOPERATIVE_CANCEL: Lazy<Mutex<HashSet<String>>> =
    Lazy::new(|| Mutex::new(HashSet::new()));
struct TaskLogBuffer {
    entries: VecDeque<Arc<str>>,
    snapshot: Arc<[Arc<str>]>,
    version: u64,
}

impl Default for TaskLogBuffer {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            snapshot: Arc::from([]),
            version: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TaskLogSnapshot {
    pub logs: Arc<[Arc<str>]>,
    pub version: u64,
}

impl Default for TaskLogSnapshot {
    fn default() -> Self {
        Self {
            logs: Arc::from([]),
            version: 0,
        }
    }
}

static TASK_LOGS: Lazy<Mutex<HashMap<Arc<str>, TaskLogBuffer>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

static TASK_EVENTS: Lazy<broadcast::Sender<TaskEvent>> = Lazy::new(|| {
    // Best-effort: drop updates if there are no receivers, or buffer overflow happens.
    let (tx, _rx) = broadcast::channel(4096);
    tx
});

static TASK_LOG_EVENTS: Lazy<broadcast::Sender<Arc<str>>> = Lazy::new(|| {
    // Logs are bounded and only the latest immutable snapshot matters to UI consumers.
    let (tx, _rx) = broadcast::channel(1024);
    tx
});

#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ByteRangeVisualization {
    pub start: u64,
    pub end: u64,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadVisualization {
    pub index: u32,
    pub label: Option<String>,
    pub active: bool,
    /// Absolute byte offset in the file where this thread's current work unit starts.
    pub start: u64,
    pub done: u64,
    pub total: u64,
    pub current_item: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskVisualization {
    pub worker_total: Option<u32>,
    pub worker_active: Option<u32>,
    pub unit_label: Option<String>,
    pub unit_total: Option<u64>,
    pub unit_done: Option<u64>,
    pub current_item: Option<String>,
    #[serde(default)]
    pub downloaded_ranges: Option<Vec<ByteRangeVisualization>>,
    pub threads: Option<Vec<ThreadVisualization>>,
}

impl TaskVisualization {
    fn is_empty(&self) -> bool {
        self.worker_total.is_none()
            && self.worker_active.is_none()
            && self.unit_label.is_none()
            && self.unit_total.is_none()
            && self.unit_done.is_none()
            && self.current_item.is_none()
            && self.downloaded_ranges.is_none()
            && self
                .threads
                .as_ref()
                .is_none_or(|threads| threads.is_empty())
    }
}

#[derive(Debug)]
pub struct TaskControl {
    cancelled: AtomicBool,
    paused: AtomicBool,
    notify: Notify,
}

impl TaskControl {
    fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            notify: Notify::new(),
        }
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.paused.store(false, Ordering::Relaxed);
        self.notify.notify_waiters();
    }

    fn pause(&self) {
        self.paused.store(true, Ordering::Relaxed);
    }

    fn resume(&self) {
        self.paused.store(false, Ordering::Relaxed);
        self.notify.notify_waiters();
    }

    pub fn cancelled_requested(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub fn paused_requested(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// 挂起等待直到任务被取消（`cancel()` 会通过 `notify_waiters` 唤醒），
    /// 用于替代热路径上的 sleep 轮询。
    pub async fn wait_cancelled(&self) {
        loop {
            let mut notified = std::pin::pin!(self.notify.notified());
            // 先注册 waiter 再检查标志，避免在检查与等待之间丢失通知。
            notified.as_mut().enable();
            if self.cancelled.load(Ordering::Relaxed) {
                return;
            }
            notified.await;
        }
    }
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug, Default, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    #[default]
    Operation,
    Workflow,
    Transfer,
    Stage,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TaskSnapshot {
    pub id: Arc<str>,
    pub title: Arc<str>,
    pub detail: Option<Arc<str>>,
    pub stage: Arc<str>,
    pub total: Option<u64>,
    pub done: u64,
    pub speed_bytes_per_sec: f64,
    pub eta: Arc<str>,
    pub percent: Option<f64>,
    pub status: Arc<str>, // "running" | "paused" | "cancelling" | "completed" | "cancelled" | "error"
    pub cancel_requested: bool,
    pub message: Option<Arc<str>>,
    pub supports_pause: bool,
    /// 用 Arc 包裹使 snapshot 构建时的 clone 变为浅拷贝（threads/downloaded_ranges 可能较大）。
    pub visualization: Option<Arc<TaskVisualization>>,
    pub started_at_unix: u64,
    pub last_update_unix: u64,
    #[serde(default)]
    pub sequence: u64,
    #[serde(default)]
    pub visibility: TaskVisibility,
    #[serde(default)]
    pub parent_id: Option<Arc<str>>,
    #[serde(default)]
    pub task_type: TaskType,
}

impl TaskSnapshot {
    pub fn is_terminal(&self) -> bool {
        is_terminal_status(self.status.as_ref())
    }
}

#[derive(Clone, Debug)]
pub enum TaskEvent {
    Updated(Arc<TaskSnapshot>),
    Removed(Arc<str>),
}

#[derive(Clone, Debug)]
pub enum TaskEventDelivery {
    Event(TaskEvent),
    Batch(Vec<TaskEvent>),
    ResyncRequired,
}

pub struct TaskUpdateReceiver {
    events: broadcast::Receiver<TaskEvent>,
}

impl TaskUpdateReceiver {
    pub async fn recv(&mut self) -> Result<Arc<TaskSnapshot>, RecvError> {
        loop {
            match self.events.recv().await? {
                TaskEvent::Updated(snapshot) => return Ok(snapshot),
                TaskEvent::Removed(_) => {}
            }
        }
    }

    pub fn try_recv(
        &mut self,
    ) -> Result<Arc<TaskSnapshot>, tokio::sync::broadcast::error::TryRecvError> {
        loop {
            match self.events.try_recv()? {
                TaskEvent::Updated(snapshot) => return Ok(snapshot),
                TaskEvent::Removed(_) => {}
            }
        }
    }
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TaskVisibility {
    #[default]
    Visible,
    Hidden,
}

pub struct TaskRenderSnapshotLists {
    pub active: Vec<Arc<TaskSnapshot>>,
    pub finished: Vec<Arc<TaskSnapshot>>,
    pub active_total: usize,
    pub finished_total: usize,
}

struct Task {
    id: Arc<str>,
    title: Arc<str>,
    detail: Option<Arc<str>>,
    stage: Arc<str>,
    total: Option<u64>,
    done: u64,
    start_instant: Instant,
    last_instant: Instant,
    last_done: u64,
    speed_ema: f64,
    status: Arc<str>,
    cancel_requested: bool,
    message: Option<Arc<str>>,
    paused: bool,
    supports_pause: bool,
    visualization: Arc<TaskVisualization>,
    last_emit_instant: Instant,
    sequence: u64,
    visibility: TaskVisibility,
    parent_id: Option<Arc<str>>,
    task_type: TaskType,
}

impl Task {
    fn new(
        id: Arc<str>,
        title: Arc<str>,
        detail: Option<Arc<str>>,
        stage: &str,
        total: Option<u64>,
        supports_pause: bool,
        visibility: TaskVisibility,
    ) -> Self {
        let now = Instant::now();
        Self {
            id,
            title,
            detail,
            stage: Arc::from(stage),
            total,
            done: 0,
            start_instant: now,
            last_instant: now,
            last_done: 0,
            speed_ema: 0.0,
            status: Arc::<str>::from("running"),
            cancel_requested: false,
            message: None,
            paused: false,
            supports_pause,
            visualization: Arc::new(TaskVisualization::default()),
            last_emit_instant: now,
            sequence: 0,
            visibility,
            parent_id: None,
            task_type: TaskType::Operation,
        }
    }

    fn touch(&mut self) {
        self.sequence = self.sequence.saturating_add(1);
    }

    fn snapshot(&self) -> TaskSnapshot {
        let now_unix = unix_now_seconds();
        let (percent_opt, eta_str, done_clamped) = match self.total {
            Some(tot) => {
                let done_clamped = std::cmp::min(self.done, tot);
                let percent = if tot == 0 {
                    0.0
                } else {
                    (done_clamped as f64 / tot as f64) * 100.0
                };
                let eta = if self.speed_ema > 1e-6 && tot > done_clamped {
                    let remain = (tot - done_clamped) as f64;
                    Arc::<str>::from(format_duration_hms(Duration::from_secs_f64(
                        remain / self.speed_ema,
                    )))
                } else {
                    Arc::<str>::from("unknown")
                };
                (Some(percent), eta, done_clamped)
            }
            None => (None, Arc::<str>::from("unknown"), self.done),
        };

        TaskSnapshot {
            id: self.id.clone(),
            title: self.title.clone(),
            detail: self.detail.clone(),
            // Keep the semantic stage code in the domain snapshot. Rendering
            // owns the locale-specific label so switching language can
            // re-render existing tasks without recreating the task.
            stage: self.stage.clone(),
            total: self.total,
            done: done_clamped,
            speed_bytes_per_sec: self.speed_ema,
            eta: eta_str,
            percent: percent_opt,
            status: self.status.clone(),
            cancel_requested: self.cancel_requested,
            message: self.message.clone(),
            supports_pause: self.supports_pause,
            visualization: (!self.visualization.is_empty())
                .then(|| Arc::clone(&self.visualization)),
            started_at_unix: now_unix.saturating_sub(self.start_instant.elapsed().as_secs()),
            last_update_unix: now_unix,
            sequence: self.sequence,
            visibility: self.visibility,
            parent_id: self.parent_id.clone(),
            task_type: self.task_type,
        }
    }

    fn should_emit(&self, now: Instant) -> bool {
        now.duration_since(self.last_emit_instant).as_millis() as u128 >= TASK_EMIT_INTERVAL_MS
    }

    fn mark_emitted(&mut self, now: Instant) {
        self.last_emit_instant = now;
    }
}

fn default_task_title(_stage: &str) -> Arc<str> {
    Arc::<str>::from("background task")
}

pub fn is_terminal_status(status: &str) -> bool {
    matches!(status, "completed" | "cancelled" | "error")
}

pub fn subscribe_task_events() -> broadcast::Receiver<TaskEvent> {
    TASK_EVENTS.subscribe()
}

pub fn subscribe_task_log_updates() -> broadcast::Receiver<Arc<str>> {
    TASK_LOG_EVENTS.subscribe()
}

pub fn task_event_stream() -> impl futures::Stream<Item = TaskEventDelivery> {
    let receiver = subscribe_task_events();
    futures::stream::unfold(receiver, |mut receiver| async move {
        match receiver.recv().await {
            Ok(first_event) => {
                let mut events = vec![first_event];
                loop {
                    match receiver.try_recv() {
                        Ok(event) => events.push(event),
                        Err(TryRecvError::Empty | TryRecvError::Closed) => break,
                        Err(TryRecvError::Lagged(_)) => {
                            return Some((TaskEventDelivery::ResyncRequired, receiver));
                        }
                    }
                }

                if events.len() == 1 {
                    let Some(event) = events.pop() else {
                        return None;
                    };
                    Some((TaskEventDelivery::Event(event), receiver))
                } else {
                    Some((
                        TaskEventDelivery::Batch(coalesce_task_events(events)),
                        receiver,
                    ))
                }
            }
            Err(RecvError::Lagged(_)) => Some((TaskEventDelivery::ResyncRequired, receiver)),
            Err(RecvError::Closed) => None,
        }
    })
}

fn coalesce_task_events(events: Vec<TaskEvent>) -> Vec<TaskEvent> {
    let mut latest_by_task = HashMap::with_capacity(events.len());
    for event in events {
        let task_id = match &event {
            TaskEvent::Updated(snapshot) => snapshot.id.clone(),
            TaskEvent::Removed(task_id) => task_id.clone(),
        };
        let should_replace = match latest_by_task.get(&task_id) {
            None => true,
            Some(TaskEvent::Updated(current)) => match &event {
                TaskEvent::Updated(next) => current.sequence <= next.sequence,
                TaskEvent::Removed(_) => true,
            },
            Some(TaskEvent::Removed(_)) => matches!(event, TaskEvent::Updated(_)),
        };
        if should_replace {
            latest_by_task.insert(task_id, event);
        }
    }
    latest_by_task.into_values().collect()
}

pub fn subscribe_task_updates() -> TaskUpdateReceiver {
    TaskUpdateReceiver {
        events: subscribe_task_events(),
    }
}

pub async fn wait_for_task_terminal(task_id: &str) -> Result<Arc<TaskSnapshot>, String> {
    let mut events = subscribe_task_events();
    if let Some(snapshot) = get_snapshot_arc(task_id)
        && snapshot.is_terminal()
    {
        return Ok(snapshot);
    }

    loop {
        match events.recv().await {
            Ok(TaskEvent::Updated(snapshot))
                if snapshot.id.as_ref() == task_id && snapshot.is_terminal() =>
            {
                return Ok(snapshot);
            }
            Ok(TaskEvent::Removed(removed_task_id)) if removed_task_id.as_ref() == task_id => {
                return Err("任务在到达终态前已移除".to_string());
            }
            Ok(_) => {}
            Err(RecvError::Lagged(_)) => {
                if let Some(snapshot) = get_snapshot_arc(task_id)
                    && snapshot.is_terminal()
                {
                    return Ok(snapshot);
                }
            }
            Err(RecvError::Closed) => {
                return get_snapshot_arc(task_id)
                    .filter(|snapshot| snapshot.is_terminal())
                    .ok_or_else(|| "任务事件通道已关闭".to_string());
            }
        }
    }
}

pub fn task_visualization_enabled() -> bool {
    TASK_VISUALIZATION_ENABLED
}

pub fn register_task_abort_handle(task_id: impl Into<String>, abort_handle: AbortHandle) {
    TASK_ABORT_HANDLES
        .lock()
        .unwrap()
        .insert(task_id.into(), abort_handle);
}

pub fn register_task_cancel_hook(
    task_id: impl Into<String>,
    cancel_hook: impl Fn() + Send + Sync + 'static,
) {
    TASK_CANCEL_HOOKS
        .lock()
        .unwrap()
        .insert(task_id.into(), Box::new(cancel_hook));
}

/// Register a task whose owner must remain alive until cleanup or rollback completes.
pub fn register_task_cooperative_cancel(task_id: impl Into<String>) {
    TASK_COOPERATIVE_CANCEL
        .lock()
        .unwrap()
        .insert(task_id.into());
}

fn task_uses_cooperative_cancel(task_id: &str) -> bool {
    TASK_COOPERATIVE_CANCEL.lock().unwrap().contains(task_id)
}

fn clear_task_cooperative_cancel(task_id: &str) {
    TASK_COOPERATIVE_CANCEL.lock().unwrap().remove(task_id);
}

fn register_task_control(task_id: impl Into<String>, control: Arc<TaskControl>) {
    if let Ok(mut map) = TASK_CONTROLS.write() {
        map.insert(task_id.into(), control);
    }
}

pub fn task_control(task_id: &str) -> Option<Arc<TaskControl>> {
    TASK_CONTROLS
        .read()
        .ok()
        .and_then(|map| map.get(task_id).cloned())
}

pub fn abort_task(task_id: &str) -> bool {
    let abort_handle = TASK_ABORT_HANDLES.lock().unwrap().remove(task_id);
    if let Some(abort_handle) = abort_handle {
        abort_handle.abort();
        true
    } else {
        false
    }
}

fn clear_task_abort_handle(task_id: &str) {
    TASK_ABORT_HANDLES.lock().unwrap().remove(task_id);
}

fn run_task_cancel_hook(task_id: &str) {
    if let Some(cancel_hook) = TASK_CANCEL_HOOKS.lock().unwrap().remove(task_id) {
        cancel_hook();
    }
}

fn clear_task_cancel_hook(task_id: &str) {
    TASK_CANCEL_HOOKS.lock().unwrap().remove(task_id);
}

/// A bounded, privacy-preserving operation label. Task titles/details can contain
/// user-defined instance names, file names and paths; do not serialize them.
fn operation_category(title: &str, stage: &str) -> &'static str {
    if stage.starts_with("map_")
        || title.contains("区块")
        || title.contains("区域包")
        || title.contains("结构")
        || title.contains("地图")
        || title.contains("世界")
        || title.contains("模型")
    {
        if stage.contains("export") || title.contains("导出") {
            "地图/模型导出"
        } else if stage.contains("import") || title.contains("导入") {
            "地图/结构导入"
        } else if stage.contains("copy") || title.contains("复制") {
            "地图区块复制"
        } else {
            "地图编辑"
        }
    } else if title.contains("资源包") || title.contains("材质包") || title.contains("附加包") {
        if title.contains("导入") || title.contains("添加") {
            "资源包导入"
        } else if title.contains("导出") {
            "资源包导出"
        } else {
            "资源包管理"
        }
    } else if title.contains("导入") || title.contains("Import") {
        "文件/版本导入"
    } else if title.contains("导出") || title.contains("Export") {
        "数据导出"
    } else if title.contains("更新") || title.contains("Update") {
        "程序/组件更新"
    } else if title.contains("下载") || title.contains("Download") || stage == "downloading" {
        "下载"
    } else if title.contains("安装") || title.contains("Install") || stage == "installing" {
        "安装"
    } else if title.contains("启动") || title.contains("Launch") || stage == "launching" {
        "启动"
    } else if title.contains("添加") || title.contains("Add") {
        "添加"
    } else if title.contains("删除") || title.contains("移除") {
        "删除"
    } else {
        "后台任务"
    }
}

fn safe_operation_field(input: &str) -> &str {
    if input.len() <= 80
        && !input.is_empty()
        && input.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
    {
        input
    } else {
        "[已隐藏]"
    }
}

fn task_chunk_count(detail: Option<&str>) -> Option<u64> {
    let mut parts = detail?.split_whitespace();
    let count = parts.next()?.parse::<u64>().ok()?;
    if parts.next()? == "个" && parts.next()? == "chunk" {
        Some(count)
    } else {
        None
    }
}

fn record_operation_event(snapshot: &TaskSnapshot, event: &str) {
    if snapshot.visibility == TaskVisibility::Hidden {
        return;
    }
    let category = operation_category(&snapshot.title, &snapshot.stage);
    let task_id = safe_operation_field(&snapshot.id);
    let stage = safe_operation_field(&snapshot.stage);
    // The editor's "N 个 chunk · filename" detail is never logged verbatim:
    // only its numeric chunk count is allowed through this narrow parser.
    let chunks = task_chunk_count(snapshot.detail.as_deref());
    let total_units = snapshot.total;
    let safe_reason = if event == "error" {
        let raw = snapshot.message.as_deref().unwrap_or("未提供错误详情");
        Some(
            crate::utils::log_privacy::redact_log_text(raw)
                .chars()
                .take(1000)
                .collect::<String>(),
        )
    } else {
        None
    };
    match event {
        "error" => error!(
            target: "bmcbl::operation",
            operation = category, task_id, stage, event,
            reason = safe_reason.as_deref().unwrap_or("未知错误"),
            "操作失败；详情已保留在任务面板"
        ),
        "cancelled" => warn!(
            target: "bmcbl::operation",
            operation = category, task_id, stage, event,
            "操作已取消"
        ),
        "cancelling" => info!(
            target: "bmcbl::operation",
            operation = category, task_id, stage, event,
            "正在取消操作并清理资源"
        ),
        "completed" => info!(
            target: "bmcbl::operation",
            operation = category, task_id, stage, event,
            "操作已完成"
        ),
        "stage" => info!(
            target: "bmcbl::operation",
            operation = category, task_id, stage, event,
            "操作进入新阶段"
        ),
        _ => info!(
            target: "bmcbl::operation",
            operation = category, task_id, stage, event,
            ?chunks, ?total_units,
            "操作已开始"
        ),
    }
}

/// Data-only task configuration, matching a JS/TS-style createTask(options) API.
/// A task is a root by default. Parent assignment is opt-in and immutable at creation.
#[derive(Clone, Debug)]
pub struct TaskOptions {
    pub id: Option<String>,
    pub title: String,
    pub detail: Option<String>,
    pub stage: String,
    pub total: Option<u64>,
    pub supports_pause: bool,
    pub visibility: TaskVisibility,
    pub kind: TaskType,
    pub parent_id: Option<String>,
}

impl TaskOptions {
    pub fn new(title: impl Into<String>, stage: impl Into<String>) -> Self {
        Self {
            id: None,
            title: title.into(),
            detail: None,
            stage: stage.into(),
            total: None,
            supports_pause: false,
            visibility: TaskVisibility::Visible,
            kind: TaskType::Operation,
            parent_id: None,
        }
    }

    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn total(mut self, total: u64) -> Self {
        self.total = Some(total);
        self
    }

    pub fn pausable(mut self, supports_pause: bool) -> Self {
        self.supports_pause = supports_pause;
        self
    }

    pub fn visibility(mut self, visibility: TaskVisibility) -> Self {
        self.visibility = visibility;
        self
    }

    pub fn kind(mut self, kind: TaskType) -> Self {
        self.kind = kind;
        self
    }

    pub fn parent(mut self, id: impl Into<String>) -> Self {
        self.parent_id = Some(id.into());
        self
    }

    /// Explicitly inherit only the currently entered scope, never unrelated tasks.
    /// Tokio task-local context survives awaits but does not leak into spawned jobs.
    pub fn in_current_scope(mut self) -> Self {
        // An explicit parent always wins over the surrounding async context.
        if self.parent_id.is_none()
            && let Ok(parent_id) = ACTIVE_TASK_SCOPE.try_with(Arc::clone)
        {
            self.parent_id = Some(parent_id.to_string());
        }
        self
    }
}

tokio::task_local! {
    static ACTIVE_TASK_SCOPE: Arc<str>;
}

/// An explicit parent context. The scope is active only while `run` is awaited.
#[derive(Clone, Debug)]
pub struct TaskScope {
    parent_id: Arc<str>,
}

impl TaskScope {
    pub fn new(parent_id: &str) -> Result<Self, String> {
        let parent = get_snapshot_arc(parent_id)
            .ok_or_else(|| format!("父任务不存在: {parent_id}"))?;
        if parent.cancel_requested || parent.is_terminal() {
            return Err(format!("父任务已结束或正在取消: {parent_id}"));
        }
        Ok(Self { parent_id: parent.id.clone() })
    }

    pub fn create_task(&self, options: TaskOptions) -> Result<String, String> {
        if options.parent_id.is_some() {
            return Err("TaskScope 不接受已指定 parent_id 的任务选项".to_owned());
        }
        create_task(options.parent(self.parent_id.to_string()))
    }

    pub async fn run<F: Future>(&self, operation: F) -> F::Output {
        ACTIVE_TASK_SCOPE.scope(self.parent_id.clone(), operation).await
    }

    /// Spawn on the shared IO runtime while explicitly carrying parent context.
    /// Plain Tokio spawns do not inherit task-local state across task boundaries.
    pub fn spawn_io<F>(&self, operation: F) -> Result<tokio::task::JoinHandle<F::Output>, String>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let scope = self.clone();
        crate::tasks::runtime::spawn_io(async move { scope.run(operation).await })
    }
}

/// Single allocation path: the first observable snapshot already has the final kind,
/// visibility and parent relationship. Duplicate IDs and invalid parents are errors.
pub fn create_task(options: TaskOptions) -> Result<String, String> {
    let TaskOptions {
        id, title, detail, stage, total, supports_pause,
        visibility, kind, parent_id,
    } = options;
    let id = id.unwrap_or_else(|| {
        let number = TASK_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("task-{number}-{}", unix_now_seconds())
    });
    let mut task = Task::new(
        Arc::from(id.as_str()), Arc::from(title),
        detail.map(Arc::<str>::from), &stage, total, supports_pause, visibility,
    );
    task.task_type = kind;
    let snapshot = {
        let mut tasks = TASKS.lock().unwrap();
        if tasks.contains_key(&id) {
            return Err(format!("任务 ID 已存在: {id}"));
        }
        if let Some(parent_id) = parent_id.as_deref() {
            if parent_id == id {
                return Err("任务不能成为自身的子任务".to_owned());
            }
            let parent = tasks.get(parent_id)
                .ok_or_else(|| format!("父任务不存在: {parent_id}"))?;
            if parent.cancel_requested || is_terminal_status(parent.status.as_ref()) {
                return Err(format!("父任务已结束或正在取消: {parent_id}"));
            }
            task.parent_id = Some(Arc::from(parent_id));
        }
        task.touch();
        let snapshot = task.snapshot();
        tasks.insert(id.clone(), task);
        register_task_control(id.clone(), Arc::new(TaskControl::new()));
        snapshot
    };

    debug!(task_id = %id, ?kind, parent_id, "task_manager: created task");
    record_operation_event(&snapshot, "started");
    emit_task_update(snapshot);
    Ok(id)
}

/// Simple root task. Existing entry points delegate to the common options path.
pub fn create_task_with_details(
    id_opt: Option<String>,
    title: impl Into<String>,
    detail: Option<String>,
    initial_stage: &str,
    total: Option<u64>,
    supports_pause: bool,
) -> String {
    create_task_with_details_and_visibility(
        id_opt, title, detail, initial_stage, total, supports_pause, TaskVisibility::Visible,
    )
}

pub fn create_task_with_details_and_visibility(
    id_opt: Option<String>,
    title: impl Into<String>,
    detail: Option<String>,
    initial_stage: &str,
    total: Option<u64>,
    supports_pause: bool,
    visibility: TaskVisibility,
) -> String {
    let mut options = TaskOptions::new(title, initial_stage)
        .pausable(supports_pause)
        .visibility(visibility);
    if let Some(id) = id_opt { options.id = Some(id); }
    options.detail = detail;
    options.total = total;
    create_task(options).expect("legacy root task creation must use a unique task ID")
}

pub fn create_task_with_options(
    id_opt: Option<String>,
    initial_stage: &str,
    total: Option<u64>,
    supports_pause: bool,
) -> String {
    create_task_with_details(
        id_opt,
        default_task_title(initial_stage).to_string(),
        None,
        initial_stage,
        total,
        supports_pause,
    )
}

pub fn set_task_labels(task_id: &str, title: impl Into<String>, detail: Option<String>) -> bool {
    let mut snapshot_to_emit: Option<TaskSnapshot> = None;
    let mut changed = false;
    {
        let mut map = TASKS.lock().unwrap();
        if let Some(task) = map.get_mut(task_id) {
            task.title = Arc::from(title.into());
            task.detail = detail.map(Arc::<str>::from);
            task.touch();
            snapshot_to_emit = Some(task.snapshot());
            changed = true;
        }
    }

    if let Some(snapshot) = snapshot_to_emit {
        emit_task_update(snapshot);
    }

    changed
}

pub fn set_task_message(task_id: &str, message: Option<String>) -> bool {
    let mut snapshot_to_emit: Option<TaskSnapshot> = None;
    let mut changed = false;
    {
        let mut map = TASKS.lock().unwrap();
        if let Some(task) = map.get_mut(task_id) {
            let next_message = message.map(Arc::<str>::from);
            if task.message != next_message {
                task.message = next_message;
                task.touch();
                snapshot_to_emit = Some(task.snapshot());
                changed = true;
            }
        }
    }

    if let Some(snapshot) = snapshot_to_emit {
        emit_task_update(snapshot);
    }

    changed
}

pub fn append_task_log(task_id: &str, line: impl Into<String>) -> bool {
    let task_id: Arc<str> = Arc::from(task_id);
    {
        let mut logs = TASK_LOGS.lock().unwrap();
        let task_logs = logs.entry(task_id.clone()).or_default();
        if task_logs.entries.len() >= TASK_LOG_LIMIT {
            task_logs.entries.pop_front();
        }
        task_logs.entries.push_back(Arc::from(line.into()));
        task_logs.snapshot = Arc::from(task_logs.entries.iter().cloned().collect::<Vec<_>>());
        task_logs.version = task_logs.version.saturating_add(1);
    }

    if TASK_LOG_EVENTS.receiver_count() > 0 {
        let _ = TASK_LOG_EVENTS.send(task_id);
    }
    true
}

pub fn task_logs_snapshot(task_id: &str) -> TaskLogSnapshot {
    let logs = TASK_LOGS.lock().unwrap();
    logs.get(task_id)
        .map(|buffer| TaskLogSnapshot {
            logs: buffer.snapshot.clone(),
            version: buffer.version,
        })
        .unwrap_or_default()
}

pub fn task_logs(task_id: &str) -> Arc<[Arc<str>]> {
    task_logs_snapshot(task_id).logs
}

pub fn update_progress(task_id: &str, delta_bytes: u64, total: Option<u64>, stage: Option<&str>) {
    update_progress_inner(task_id, delta_bytes, total, stage, None);
}

pub fn update_progress_with_visualization(
    task_id: &str,
    delta_bytes: u64,
    total: Option<u64>,
    stage: Option<&str>,
    visualization: TaskVisualization,
) {
    update_progress_inner(task_id, delta_bytes, total, stage, Some(visualization));
}

fn update_progress_inner(
    task_id: &str,
    delta_bytes: u64,
    total: Option<u64>,
    stage: Option<&str>,
    visualization: Option<TaskVisualization>,
) {
    let mut snapshot_to_emit: Option<TaskSnapshot> = None;
    let mut stage_changed = false;

    {
        let mut map = TASKS.lock().unwrap();
        if let Some(t) = map.get_mut(task_id) {
            if is_terminal_status(t.status.as_ref()) {
                return;
            }
            stage_changed = stage.is_some_and(|next_stage| next_stage != t.stage.as_ref());
            if let Some(tt) = total {
                t.total = Some(tt);
            }
            if let Some(s) = stage {
                t.stage = Arc::from(s);
            }

            if let Some(visualization) = visualization {
                if *t.visualization != visualization {
                    t.visualization = Arc::new(visualization);
                }
            }

            t.touch();
            let now = Instant::now();
            let elapsed = now.duration_since(t.last_instant).as_secs_f64();

            t.done = t.done.saturating_add(delta_bytes);
            if let Some(tot) = t.total {
                if t.done > tot {
                    t.done = tot;
                }
            }

            if stage_changed {
                // A stage boundary starts a new throughput domain. Keeping the previous download
                // EMA here makes verifying/extracting/queued states display a frozen MB/s value.
                t.speed_ema = 0.0;
                t.last_instant = now;
                t.last_done = t.done;
            } else if elapsed >= TASK_PROGRESS_MIN_UPDATE_INTERVAL {
                let diff = t.done.saturating_sub(t.last_done);
                if diff > 0 {
                    let inst_speed = (diff as f64) / elapsed;
                    if t.speed_ema <= 0.0 {
                        t.speed_ema = inst_speed;
                    } else {
                        t.speed_ema = t.speed_ema * (1.0 - TASK_PROGRESS_EMA_ALPHA)
                            + inst_speed * TASK_PROGRESS_EMA_ALPHA;
                    }
                    t.last_instant = now;
                    t.last_done = t.done;
                } else if elapsed >= TASK_PROGRESS_IDLE_RESET_SECONDS && t.speed_ema > 0.0 {
                    t.speed_ema = 0.0;
                    t.last_instant = now;
                    t.last_done = t.done;
                }
            }

            if stage_changed || t.should_emit(now) {
                t.mark_emitted(now);
                t.touch();
                snapshot_to_emit = Some(t.snapshot());
            }
        }
    }

    if let Some(snap) = snapshot_to_emit {
        if stage_changed {
            record_operation_event(&snap, "stage");
        }
        emit_task_update(snap);
    }
}

/// Refresh transfer telemetry even when the network stream is temporarily silent.
///
/// Byte-driven progress reporting otherwise leaves the last non-zero MB/s visible while a
/// request is stalled. Download runtimes call this periodically; it emits only when a pending
/// speed sample becomes ready or a stale rate must be reset.
pub fn refresh_task_telemetry(task_id: &str) -> bool {
    let mut snapshot_to_emit: Option<TaskSnapshot> = None;

    let still_active = {
        let mut map = TASKS.lock().unwrap();
        let Some(task) = map.get_mut(task_id) else {
            return false;
        };
        if is_terminal_status(task.status.as_ref()) {
            return false;
        }

        let now = Instant::now();
        let elapsed = now.duration_since(task.last_instant).as_secs_f64();
        if elapsed >= TASK_PROGRESS_MIN_UPDATE_INTERVAL {
            let diff = task.done.saturating_sub(task.last_done);
            let mut telemetry_changed = false;

            if diff > 0 {
                let inst_speed = (diff as f64) / elapsed;
                task.speed_ema = if task.speed_ema <= 0.0 {
                    inst_speed
                } else {
                    task.speed_ema * (1.0 - TASK_PROGRESS_EMA_ALPHA)
                        + inst_speed * TASK_PROGRESS_EMA_ALPHA
                };
                task.last_instant = now;
                task.last_done = task.done;
                telemetry_changed = true;
            } else if elapsed >= TASK_PROGRESS_IDLE_RESET_SECONDS && task.speed_ema > 0.0 {
                task.speed_ema = 0.0;
                task.last_instant = now;
                task.last_done = task.done;
                telemetry_changed = true;
            }

            if telemetry_changed {
                task.mark_emitted(now);
                task.touch();
                snapshot_to_emit = Some(task.snapshot());
            }
        }

        true
    };

    if let Some(snapshot) = snapshot_to_emit {
        emit_task_update(snapshot);
    }

    still_active
}

pub fn set_total(task_id: &str, total: Option<u64>) {
    let mut snapshot_to_emit: Option<TaskSnapshot> = None;
    {
        let mut map = TASKS.lock().unwrap();
        if let Some(t) = map.get_mut(task_id) {
            t.total = total;
            if let Some(tot) = total {
                if t.done > tot {
                    t.done = tot;
                    if t.last_done > t.done {
                        t.last_done = t.done;
                    }
                }
            }
            t.touch();
            snapshot_to_emit = Some(t.snapshot());
        }
    }

    if let Some(snap) = snapshot_to_emit {
        emit_task_update(snap);
    }
}

pub fn reset_progress(task_id: &str, total: Option<u64>, stage: Option<&str>) {
    let mut snapshot_to_emit: Option<TaskSnapshot> = None;
    let mut stage_changed = false;
    {
        let mut map = TASKS.lock().unwrap();
        if let Some(t) = map.get_mut(task_id) {
            if is_terminal_status(t.status.as_ref()) {
                return;
            }
            t.total = total;
            t.done = 0;
            t.last_done = 0;
            t.speed_ema = 0.0;
            t.last_instant = Instant::now();
            t.visualization = Arc::new(TaskVisualization::default());
            if let Some(stage) = stage {
                stage_changed = t.stage.as_ref() != stage;
                t.stage = Arc::from(stage);
            }
            t.touch();
            snapshot_to_emit = Some(t.snapshot());
        }
    }

    if let Some(snap) = snapshot_to_emit {
        if stage_changed {
            record_operation_event(&snap, "stage");
        }
        emit_task_update(snap);
    }
}

pub fn set_task_visualization(task_id: &str, visualization: Option<TaskVisualization>) -> bool {
    if !TASK_VISUALIZATION_ENABLED {
        return false;
    }

    let mut snapshot_to_emit: Option<TaskSnapshot> = None;
    let mut changed = false;
    {
        let mut map = TASKS.lock().unwrap();
        if let Some(task) = map.get_mut(task_id) {
            if is_terminal_status(task.status.as_ref()) {
                return false;
            }
            let next_visualization = visualization.unwrap_or_default();
            if *task.visualization != next_visualization {
                task.visualization = Arc::new(next_visualization);
                task.touch();
                changed = true;
                let now = Instant::now();
                if task.should_emit(now) {
                    task.mark_emitted(now);
                    snapshot_to_emit = Some(task.snapshot());
                }
            }
        }
    }

    if let Some(snapshot) = snapshot_to_emit {
        emit_task_update(snapshot);
    }

    changed
}

pub fn maybe_set_task_visualization(
    task_id: &str,
    build_visualization: impl FnOnce() -> Option<TaskVisualization>,
) -> bool {
    if !TASK_VISUALIZATION_ENABLED {
        return false;
    }

    set_task_visualization(task_id, build_visualization())
}

pub fn finish_task(task_id: &str, status: &str, message: Option<String>) {
    let mut snapshot_to_emit: Option<TaskSnapshot> = None;
    let mut was_terminal = false;
    {
        let mut map = TASKS.lock().unwrap();
        if let Some(t) = map.get_mut(task_id) {
            was_terminal = is_terminal_status(t.status.as_ref());
            if was_terminal && t.status.as_ref() != status {
                return;
            }
            t.status = Arc::from(status);
            t.cancel_requested = status == "cancelled";
            t.message = message.map(Arc::<str>::from);
            t.paused = false;
            if status == "completed"
                && let Some(total) = t.total
            {
                t.done = total;
            }
            if is_terminal_status(status) {
                t.speed_ema = 0.0;
                t.last_done = t.done;
                t.last_instant = Instant::now();
            }
            {
                // 终态更新只发生一次，Arc::make_mut 的写时拷贝成本可接受。
                let visualization = Arc::make_mut(&mut t.visualization);
                if visualization.worker_total.is_some() || visualization.worker_active.is_some() {
                    visualization.worker_active = Some(0);
                }
                if status == "completed"
                    && let Some(unit_total) = visualization.unit_total
                {
                    visualization.unit_done = Some(unit_total);
                }
                visualization.current_item = None;
            }
            t.touch();
            snapshot_to_emit = Some(t.snapshot());
        }
    }

    if let Some(snap) = snapshot_to_emit {
        debug!(
            task_id,
            status,
            message = snap.message.as_deref().unwrap_or(""),
            "task_manager: task finished"
        );
        if !was_terminal {
            record_operation_event(&snap, status);
        }
        emit_task_update(snap);
    }

    if let Some(control) = task_control(task_id) {
        if status == "cancelled" {
            control.cancel();
        } else {
            control.resume();
        }
    }

    if is_terminal_status(status) {
        clear_task_abort_handle(task_id);
        clear_task_cancel_hook(task_id);
        clear_task_cooperative_cancel(task_id);
    }
}

pub fn cancel_task(task_id: &str) {
    let cooperative = task_uses_cooperative_cancel(task_id);
    let snapshot_to_emit = {
        let mut map = TASKS.lock().unwrap();
        let Some(t) = map.get_mut(task_id) else {
            return;
        };
        if is_terminal_status(t.status.as_ref()) || t.cancel_requested {
            return;
        }

        t.cancel_requested = true;
        t.paused = false;
        t.status = Arc::<str>::from(if cooperative {
            "cancelling"
        } else {
            "cancelled"
        });
        t.message = Some(Arc::<str>::from(if cooperative {
            "正在取消并清理资源"
        } else {
            "user cancelled"
        }));
        t.speed_ema = 0.0;
        t.last_done = t.done;
        t.last_instant = Instant::now();
        {
            let visualization = Arc::make_mut(&mut t.visualization);
            visualization.current_item = Some("正在取消并清理资源".to_string());
            if visualization.worker_total.is_some() || visualization.worker_active.is_some() {
                visualization.worker_active = Some(0);
            }
        }
        t.touch();
        t.snapshot()
    };

    record_operation_event(
        &snapshot_to_emit,
        if cooperative { "cancelling" } else { "cancelled" },
    );
    emit_task_update(snapshot_to_emit);
    if let Some(control) = task_control(task_id) {
        control.cancel();
    }
    // Cancellation propagates down the entire workflow tree.
    let children: Vec<String> = {
        let tasks = TASKS.lock().unwrap();
        tasks.values()
            .filter(|task| task.parent_id.as_deref() == Some(task_id))
            .map(|task| task.id.to_string())
            .collect()
    };
    for child in children {
        cancel_task(&child);
    }
    run_task_cancel_hook(task_id);

    if !cooperative {
        let _ = abort_task(task_id);
    }
}

pub fn is_cancelled(task_id: &str) -> bool {
    task_control(task_id)
        .map(|control| control.cancelled.load(Ordering::Relaxed))
        .unwrap_or(false)
}

pub fn pause_task(task_id: &str) -> bool {
    let mut snapshot_to_emit: Option<TaskSnapshot> = None;
    let mut changed = false;
    {
        let mut map = TASKS.lock().unwrap();
        if let Some(t) = map.get_mut(task_id) {
            if t.supports_pause && !t.cancel_requested && t.status.as_ref() == "running" {
                t.paused = true;
                t.status = Arc::<str>::from("paused");
                t.speed_ema = 0.0;
                t.last_done = t.done;
                t.last_instant = Instant::now();
                t.touch();
                snapshot_to_emit = Some(t.snapshot());
                changed = true;
            }
        }
    }

    if let Some(snap) = snapshot_to_emit {
        emit_task_update(snap);
    }

    if let Some(control) = task_control(task_id) {
        control.pause();
    }

    changed
}

pub fn resume_task(task_id: &str) -> bool {
    let mut snapshot_to_emit: Option<TaskSnapshot> = None;
    let mut changed = false;
    {
        let mut map = TASKS.lock().unwrap();
        if let Some(t) = map.get_mut(task_id) {
            if t.supports_pause && !t.cancel_requested && t.status.as_ref() == "paused" {
                t.paused = false;
                t.status = Arc::<str>::from("running");
                t.speed_ema = 0.0;
                t.last_done = t.done;
                t.last_instant = Instant::now();
                t.last_emit_instant = Instant::now();
                t.touch();
                snapshot_to_emit = Some(t.snapshot());
                changed = true;
            }
        }
    }

    if let Some(snap) = snapshot_to_emit {
        emit_task_update(snap);
    }

    if let Some(control) = task_control(task_id) {
        control.resume();
    }

    changed
}

/// Remove a completed workflow with its descendant snapshots and log buffers.
/// Active children prevent removal so ongoing cleanup can never become orphaned.
pub fn remove_task(task_id: &str) -> bool {
    let removal_order = {
        let tasks = TASKS.lock().unwrap();
        if !tasks.contains_key(task_id) {
            return false;
        }
        let mut ids = vec![task_id.to_string()];
        let mut cursor = 0;
        while cursor < ids.len() {
            let parent = ids[cursor].as_str();
            for task in tasks.values() {
                if task.parent_id.as_deref() == Some(parent) {
                    ids.push(task.id.to_string());
                }
            }
            cursor += 1;
        }
        if ids.iter().skip(1).any(|id| {
            tasks.get(id).is_some_and(|task| !is_terminal_status(task.status.as_ref()))
        }) {
            return false;
        }
        ids
    };

    // Children first, root last. Release task locks before broadcasting events.
    let mut removed_root = false;
    for id in removal_order.into_iter().rev() {
        clear_task_abort_handle(&id);
        clear_task_cancel_hook(&id);
        clear_task_cooperative_cancel(&id);
        let removed_snapshot = TASK_SNAPSHOTS.write().unwrap().remove(id.as_str());
        TASK_LOGS.lock().unwrap().remove(id.as_str());
        let _ = TASK_CONTROLS.write().ok()
            .and_then(|mut controls| controls.remove(id.as_str()));
        let removed = TASKS.lock().unwrap().remove(id.as_str()).is_some();
        if id == task_id {
            removed_root = removed;
        }
        if removed_snapshot.as_ref().is_some_and(|snapshot| snapshot.visibility == TaskVisibility::Visible)
            && TASK_EVENTS.receiver_count() > 0
        {
            let _ = TASK_EVENTS.send(TaskEvent::Removed(Arc::from(id.as_str())));
        }
    }
    removed_root
}

pub async fn wait_until_active(task_id: &str) -> bool {
    match task_control(task_id) {
        Some(control) => wait_until_active_fast(control.as_ref()).await,
        None => false,
    }
}

pub async fn wait_until_active_fast(control: &TaskControl) -> bool {
    loop {
        if control.cancelled.load(Ordering::Relaxed) {
            return false;
        }
        if !control.paused.load(Ordering::Relaxed) {
            return true;
        }

        control.notify.notified().await;
    }
}

pub fn is_cancelled_fast(control: &TaskControl) -> bool {
    control.cancelled.load(Ordering::Relaxed)
}

pub fn get_snapshot(task_id: &str) -> Option<TaskSnapshot> {
    get_snapshot_arc(task_id).map(|snapshot| snapshot.as_ref().clone())
}

pub fn get_snapshot_arc(task_id: &str) -> Option<Arc<TaskSnapshot>> {
    let map = TASK_SNAPSHOTS.read().unwrap();
    map.get(task_id).cloned()
}

pub fn snapshot_arcs() -> Vec<Arc<TaskSnapshot>> {
    let map = TASK_SNAPSHOTS.read().unwrap();
    map.values().cloned().collect()
}

pub fn snapshot_arcs_map() -> HashMap<Arc<str>, Arc<TaskSnapshot>> {
    let map = TASK_SNAPSHOTS.read().unwrap();
    map.values()
        .map(|snapshot| (snapshot.id.clone(), snapshot.clone()))
        .collect()
}

pub fn snapshots_map() -> HashMap<String, TaskSnapshot> {
    let map = TASK_SNAPSHOTS.read().unwrap();
    map.iter()
        .map(|(task_id, snapshot)| (task_id.as_ref().to_owned(), snapshot.as_ref().clone()))
        .collect()
}

pub fn try_snapshots_sorted() -> Option<Vec<TaskSnapshot>> {
    let Ok(map) = TASK_SNAPSHOTS.try_read() else {
        return None;
    };

    let mut snapshots: Vec<_> = map
        .values()
        .map(|snapshot| snapshot.as_ref().clone())
        .collect();
    snapshots.sort_by(|a, b| a.started_at_unix.cmp(&b.started_at_unix));
    Some(snapshots)
}

fn snapshot_sort_key(snapshot: &TaskSnapshot) -> (u64, &str) {
    (snapshot.started_at_unix, snapshot.id.as_ref())
}

fn insert_sorted_limited(
    snapshots: &mut Vec<Arc<TaskSnapshot>>,
    snapshot: &Arc<TaskSnapshot>,
    limit: usize,
) {
    if limit == 0 {
        return;
    }

    let insert_at = snapshots.partition_point(|existing| {
        snapshot_sort_key(existing.as_ref()) <= snapshot_sort_key(snapshot.as_ref())
    });

    if snapshots.len() < limit {
        snapshots.insert(insert_at, snapshot.clone());
        return;
    }

    if insert_at >= limit {
        return;
    }

    snapshots.insert(insert_at, snapshot.clone());
    snapshots.truncate(limit);
}

fn collect_render_snapshots_limited<'a>(
    snapshots: impl Iterator<Item = &'a Arc<TaskSnapshot>>,
    active_limit: usize,
    finished_storage_limit: usize,
    finished_render_limit: usize,
) -> TaskRenderSnapshotLists {
    let mut active = Vec::with_capacity(active_limit);
    let mut finished = Vec::with_capacity(finished_storage_limit.min(finished_render_limit));
    let mut active_total = 0;
    let mut finished_total = 0;

    for snapshot in snapshots {
        if snapshot.visibility == TaskVisibility::Hidden || snapshot.parent_id.is_some() {
            continue;
        }
        if snapshot.is_terminal() {
            finished_total += 1;
            insert_sorted_limited(&mut finished, snapshot, finished_storage_limit);
        } else {
            active_total += 1;
            insert_sorted_limited(&mut active, snapshot, active_limit);
        }
    }

    if finished.len() > finished_render_limit {
        finished.truncate(finished_render_limit);
    }

    TaskRenderSnapshotLists {
        active,
        finished,
        active_total,
        finished_total,
    }
}

pub fn render_snapshots_limited(
    active_limit: usize,
    finished_storage_limit: usize,
    finished_render_limit: usize,
) -> TaskRenderSnapshotLists {
    let map = TASK_SNAPSHOTS.read().unwrap();
    collect_render_snapshots_limited(
        map.values(),
        active_limit,
        finished_storage_limit,
        finished_render_limit,
    )
}

pub fn render_snapshots_from(
    snapshots: impl IntoIterator<Item = Arc<TaskSnapshot>>,
    active_limit: usize,
    finished_storage_limit: usize,
    finished_render_limit: usize,
) -> TaskRenderSnapshotLists {
    let snapshots: Vec<_> = snapshots.into_iter().collect();
    collect_render_snapshots_limited(
        snapshots.iter(),
        active_limit,
        finished_storage_limit,
        finished_render_limit,
    )
}

pub fn try_render_snapshots_limited(
    active_limit: usize,
    finished_storage_limit: usize,
    finished_render_limit: usize,
) -> Option<TaskRenderSnapshotLists> {
    let Ok(map) = TASK_SNAPSHOTS.try_read() else {
        return None;
    };
    Some(collect_render_snapshots_limited(
        map.values(),
        active_limit,
        finished_storage_limit,
        finished_render_limit,
    ))
}

pub fn snapshots_sorted() -> Vec<TaskSnapshot> {
    let map = TASK_SNAPSHOTS.read().unwrap();
    let mut snapshots: Vec<_> = map
        .values()
        .map(|snapshot| snapshot.as_ref().clone())
        .collect();
    snapshots.sort_by(|a, b| a.started_at_unix.cmp(&b.started_at_unix));
    snapshots
}

pub fn live_snapshots_sorted() -> Vec<TaskSnapshot> {
    let map = TASKS.lock().unwrap();
    let mut snapshots: Vec<_> = map.values().map(Task::snapshot).collect();
    snapshots.sort_by(|a, b| a.started_at_unix.cmp(&b.started_at_unix));
    snapshots
}

pub fn task_ids_sorted() -> Vec<String> {
    let map = TASK_SNAPSHOTS.read().unwrap();
    let mut task_ids: Vec<_> = map
        .iter()
        .map(|(task_id, snapshot)| (task_id.clone(), snapshot.started_at_unix))
        .collect();
    task_ids.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));
    task_ids
        .into_iter()
        .map(|(task_id, _)| task_id.as_ref().to_owned())
        .collect()
}

fn emit_task_update(snapshot: TaskSnapshot) {
    let snapshot = Arc::new(snapshot);
    {
        let mut map = TASK_SNAPSHOTS.write().unwrap();
        // Task ownership and UI publication may happen on separate threads.
        // Never let a late, older creation event overwrite a newer child/terminal state.
        if map
            .get(snapshot.id.as_ref())
            .is_some_and(|current| current.sequence > snapshot.sequence)
        {
            return;
        }
        map.insert(snapshot.id.clone(), snapshot.clone());
    }

    if snapshot.visibility == TaskVisibility::Hidden || TASK_EVENTS.receiver_count() == 0 {
        return;
    }

    let _ = TASK_EVENTS.send(TaskEvent::Updated(snapshot));
}

fn unix_now_seconds() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn format_duration_hms(d: Duration) -> String {
    let secs = d.as_secs();
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{:02}:{:02}:{:02}", h, m, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_category_classifies_map_and_resource_tasks_without_names() {
        assert_eq!(operation_category("导出 OBJ", "map_export"), "地图/模型导出");
        assert_eq!(operation_category("导入结构", "map_import"), "地图/结构导入");
        assert_eq!(operation_category("导入资源包", "initializing"), "资源包导入");
        assert_eq!(operation_category("安装 Minecraft", "installing"), "安装");
        assert_eq!(safe_operation_field("task-123"), "task-123");
        assert_eq!(safe_operation_field(r"C:\\Users\\Alice\\secret"), "[已隐藏]");
        assert_eq!(task_chunk_count(Some("48 个 chunk · private.mcstructure")), Some(48));
        assert_eq!(task_chunk_count(Some("private world")), None);
    }

    fn test_snapshot(task_id: &str, sequence: u64) -> Arc<TaskSnapshot> {
        Arc::new(TaskSnapshot {
            id: Arc::from(task_id),
            title: Arc::from("批处理测试"),
            detail: None,
            stage: Arc::from("downloading"),
            total: Some(100),
            done: sequence,
            speed_bytes_per_sec: 0.0,
            eta: Arc::from("unknown"),
            percent: Some(sequence as f64),
            status: Arc::from("running"),
            cancel_requested: false,
            message: None,
            supports_pause: false,
            visualization: None,
            started_at_unix: 0,
            last_update_unix: 0,
            sequence,
            visibility: TaskVisibility::Visible,
            parent_id: None,
            task_type: TaskType::Operation,
        })
    }

    #[test]
    fn task_log_reads_reuse_immutable_snapshot_until_append() {
        let task_id = format!(
            "task-log-snapshot-test-{}",
            TASK_COUNTER.fetch_add(1, Ordering::Relaxed)
        );

        append_task_log(&task_id, "first");
        let first = task_logs_snapshot(&task_id);
        let repeated = task_logs_snapshot(&task_id);
        assert_eq!(first.version, 1);
        assert!(Arc::ptr_eq(&first.logs, &repeated.logs));

        append_task_log(&task_id, "second");
        let second = task_logs_snapshot(&task_id);
        assert_eq!(second.version, 2);
        assert!(!Arc::ptr_eq(&first.logs, &second.logs));
        assert_eq!(second.logs.len(), 2);

        TASK_LOGS.lock().unwrap().remove(task_id.as_str());
    }

    #[test]
    fn task_event_batch_keeps_latest_event_per_task() {
        let events = coalesce_task_events(vec![
            TaskEvent::Updated(test_snapshot("task-a", 1)),
            TaskEvent::Updated(test_snapshot("task-a", 3)),
            TaskEvent::Updated(test_snapshot("task-a", 2)),
            TaskEvent::Removed(Arc::from("task-b")),
            TaskEvent::Updated(test_snapshot("task-b", 4)),
        ]);

        assert_eq!(events.len(), 2);
        assert!(events.iter().any(|event| {
            matches!(
                event,
                TaskEvent::Updated(snapshot)
                    if snapshot.id.as_ref() == "task-a" && snapshot.sequence == 3
            )
        }));
        assert!(events.iter().any(|event| {
            matches!(
                event,
                TaskEvent::Updated(snapshot)
                    if snapshot.id.as_ref() == "task-b" && snapshot.sequence == 4
            )
        }));
    }

    #[test]
    fn only_explicit_terminal_statuses_finish_a_task() {
        for status in [
            "ready",
            "queued",
            "initializing",
            "starting",
            "running",
            "paused",
            "cancelling",
            "future-active-state",
        ] {
            assert!(!is_terminal_status(status), "{status} must remain active");
        }
        for status in ["completed", "cancelled", "error"] {
            assert!(is_terminal_status(status), "{status} must be terminal");
        }
    }

    #[test]
    fn render_snapshots_keep_unknown_non_terminal_statuses_active() {
        let task_id = format!(
            "task-manager-render-active-test-{}",
            TASK_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        create_task_with_details(
            Some(task_id.clone()),
            "活动状态渲染测试",
            None,
            "ready",
            None,
            false,
        );
        {
            let mut tasks = TASKS.lock().expect("task map lock");
            let task = tasks.get_mut(&task_id).expect("test task");
            task.status = Arc::from("future-active-state");
            task.touch();
            emit_task_update(task.snapshot());
        }

        let snapshots = render_snapshots_limited(64, 64, 64);
        assert!(
            snapshots
                .active
                .iter()
                .any(|snapshot| snapshot.id.as_ref() == task_id)
        );
        assert!(remove_task(&task_id));
    }

    #[tokio::test]
    async fn terminal_wait_does_not_treat_queued_as_finished() {
        let task_id = format!(
            "task-manager-terminal-wait-test-{}",
            TASK_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        create_task_with_details(
            Some(task_id.clone()),
            "终态等待测试",
            None,
            "queued",
            None,
            false,
        );

        let task_id_for_waiter = task_id.clone();
        let waiter = tokio::spawn(async move { wait_for_task_terminal(&task_id_for_waiter).await });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished(), "queued task returned as terminal");

        finish_task(&task_id, "completed", Some("done".to_string()));
        let snapshot = waiter
            .await
            .expect("terminal waiter should join")
            .expect("terminal waiter should receive snapshot");
        assert_eq!(snapshot.status.as_ref(), "completed");
        assert!(remove_task(&task_id));
    }

    #[test]
    fn completed_task_finishes_known_progress_total() {
        let task_id = format!(
            "task-manager-complete-progress-test-{}",
            TASK_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        create_task_with_details(
            Some(task_id.clone()),
            "完成进度测试",
            None,
            "testing",
            Some(672),
            false,
        );
        update_progress(&task_id, 671, Some(672), Some("testing"));

        finish_task(&task_id, "completed", None);

        let snapshot = get_snapshot_arc(&task_id).expect("completed task snapshot");
        assert_eq!(snapshot.status.as_ref(), "completed");
        assert_eq!(snapshot.done, 672);
        assert_eq!(snapshot.total, Some(672));
        assert!(remove_task(&task_id));
    }

    #[test]
    fn cancel_task_runs_cancel_hook_and_clears_it() {
        let task_id = format!(
            "task-manager-cancel-hook-test-{}",
            TASK_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let hook_ran = Arc::new(AtomicBool::new(false));
        let hook_ran_for_hook = hook_ran.clone();
        let created_task_id = create_task_with_details(
            Some(task_id.clone()),
            "取消 Hook 测试",
            None,
            "starting",
            None,
            false,
        );
        assert_eq!(created_task_id, task_id);

        register_task_cancel_hook(task_id.clone(), move || {
            hook_ran_for_hook.store(true, Ordering::SeqCst);
        });

        cancel_task(&task_id);

        assert!(hook_ran.load(Ordering::SeqCst));
        assert!(!TASK_CANCEL_HOOKS.lock().unwrap().contains_key(&task_id));
        assert_eq!(
            get_snapshot_arc(&task_id)
                .expect("cancelled task snapshot")
                .status
                .as_ref(),
            "cancelled"
        );
        assert!(remove_task(&task_id));
    }

    #[test]
    fn cooperative_cancel_stays_active_until_owner_finishes_cleanup() {
        let task_id = format!(
            "task-manager-cooperative-cancel-test-{}",
            TASK_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        create_task_with_details(
            Some(task_id.clone()),
            "协作取消测试",
            None,
            "running",
            None,
            false,
        );
        register_task_cooperative_cancel(task_id.clone());

        cancel_task(&task_id);

        let cancelling = get_snapshot_arc(&task_id).expect("cancelling task snapshot");
        assert_eq!(cancelling.status.as_ref(), "cancelling");
        assert!(cancelling.cancel_requested);
        assert!(!cancelling.is_terminal());
        assert!(is_cancelled(&task_id));

        finish_task(&task_id, "cancelled", Some("cleanup complete".to_string()));
        let cancelled = get_snapshot_arc(&task_id).expect("cancelled task snapshot");
        assert_eq!(cancelled.status.as_ref(), "cancelled");
        assert!(cancelled.is_terminal());
        assert!(remove_task(&task_id));
    }

    #[tokio::test]
    async fn cancel_task_publishes_cancelled_snapshot() {
        let task_id = format!(
            "task-manager-cancel-event-test-{}",
            TASK_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let mut events = subscribe_task_events();
        create_task_with_details(
            Some(task_id.clone()),
            "取消事件测试",
            None,
            "running",
            None,
            false,
        );

        cancel_task(&task_id);

        let cancelled = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match events.recv().await {
                    Ok(TaskEvent::Updated(snapshot))
                        if snapshot.id.as_ref() == task_id
                            && snapshot.status.as_ref() == "cancelled" =>
                    {
                        return snapshot;
                    }
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => panic!("task event stream closed"),
                }
            }
        })
        .await
        .expect("cancelled snapshot should be published");

        assert!(cancelled.cancel_requested);
        assert!(cancelled.is_terminal());
        assert!(remove_task(&task_id));
    }

    #[test]
    fn task_snapshot_keeps_semantic_stage_code() {
        let task_id = format!(
            "task-manager-stage-label-test-{}",
            TASK_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let stage = "task_manager_registered_stage_label_test";
        let created_task_id = create_task_with_details(
            Some(task_id.clone()),
            "阶段文案测试",
            None,
            stage,
            None,
            false,
        );
        assert_eq!(created_task_id, task_id);
        assert_eq!(
            get_snapshot_arc(&task_id)
                .expect("task snapshot")
                .stage
                .as_ref(),
            stage
        );
        assert!(remove_task(&task_id));
    }

    #[test]
    fn workflow_parentage_is_recorded_before_child_is_visible() {
        let root = create_task(
            TaskOptions::new("安装 Minecraft", "installing_game")
                .kind(TaskType::Workflow).total(100),
        ).expect("root task");
        let scope = TaskScope::new(&root).expect("root scope");
        let child = scope.create_task(
            TaskOptions::new("安装 LeviLamina", "resolving_levilamina")
                .kind(TaskType::Workflow),
        ).expect("child task");
        let root_snapshot = get_snapshot_arc(&root).expect("root snapshot");
        let child_snapshot = get_snapshot_arc(&child).expect("child snapshot");
        assert_eq!(root_snapshot.parent_id, None);
        assert_eq!(child_snapshot.parent_id.as_deref(), Some(root.as_str()));
        assert_eq!(child_snapshot.task_type, TaskType::Workflow);
        let visible = render_snapshots_limited(usize::MAX, usize::MAX, usize::MAX);
        assert!(visible.active.iter().any(|snapshot| snapshot.id.as_ref() == root));
        assert!(!visible.active.iter().any(|snapshot| snapshot.id.as_ref() == child));
        finish_task(&child, "completed", None);
        finish_task(&root, "completed", None);
        remove_task(&child);
        remove_task(&root);
    }

    #[tokio::test]
    async fn task_scope_is_explicit_and_does_not_change_root_creation() {
        let parent = create_task(
            TaskOptions::new("parent", "running").kind(TaskType::Workflow),
        ).expect("parent");
        let scope = TaskScope::new(&parent).expect("scope");
        let (nested, unrelated) = scope.run(async {
            let nested = create_task(
                TaskOptions::new("child", "running").in_current_scope(),
            ).expect("nested");
            let unrelated = create_task(TaskOptions::new("unrelated", "running"))
                .expect("independent root");
            (nested, unrelated)
        }).await;
        assert_eq!(
            get_snapshot_arc(&nested).unwrap().parent_id.as_deref(),
            Some(parent.as_str()),
        );
        assert_eq!(get_snapshot_arc(&unrelated).unwrap().parent_id, None);
        finish_task(&nested, "completed", None);
        finish_task(&unrelated, "completed", None);
        finish_task(&parent, "completed", None);
        assert!(remove_task(&nested));
        assert!(remove_task(&unrelated));
        assert!(remove_task(&parent));
    }

    #[tokio::test]
    async fn nested_scope_restores_outer_parent_after_return() {
        let parent = create_task(
            TaskOptions::new("parent", "running").kind(TaskType::Workflow),
        ).expect("parent");
        let scope = TaskScope::new(&parent).expect("outer scope");
        let (middle, inner, sibling) = scope.run(async {
            let middle = create_task(
                TaskOptions::new("middle", "running")
                    .kind(TaskType::Workflow)
                    .in_current_scope(),
            ).expect("middle");
            let inner_scope = TaskScope::new(&middle).expect("inner scope");
            let inner = inner_scope.run(async {
                create_task(
                    TaskOptions::new("inner", "running").in_current_scope(),
                ).expect("inner")
            }).await;
            let sibling = create_task(
                TaskOptions::new("sibling", "running").in_current_scope(),
            ).expect("sibling");
            (middle, inner, sibling)
        }).await;
        assert_eq!(get_snapshot_arc(&middle).unwrap().parent_id.as_deref(), Some(parent.as_str()));
        assert_eq!(get_snapshot_arc(&inner).unwrap().parent_id.as_deref(), Some(middle.as_str()));
        assert_eq!(get_snapshot_arc(&sibling).unwrap().parent_id.as_deref(), Some(parent.as_str()));
        for id in [&inner, &sibling, &middle, &parent] {
            finish_task(id, "completed", None);
        }
        for id in [&inner, &sibling, &middle, &parent] {
            assert!(remove_task(id));
        }
    }

    #[test]
    fn removing_workflow_cleans_descendants_and_logs() {
        let root = create_task(
            TaskOptions::new("root", "running").kind(TaskType::Workflow),
        ).expect("root");
        let child = create_task(
            TaskOptions::new("child", "running").parent(&root),
        ).expect("child");
        append_task_log(&child, "child-log");
        finish_task(&root, "completed", None);
        assert!(!remove_task(&root), "active child must block removing the workflow");
        assert!(get_snapshot_arc(&child).is_some());
        finish_task(&child, "completed", None);
        assert!(remove_task(&root));
        assert!(get_snapshot_arc(&root).is_none());
        assert!(get_snapshot_arc(&child).is_none());
        assert!(task_logs(&child).is_empty());
    }

    #[test]
    fn nested_task_rejects_missing_parent_without_publishing_a_root_card() {
        let result = create_task(
            TaskOptions::new("安装 LeviLamina", "resolving_levilamina")
                .kind(TaskType::Workflow).parent("nonexistent-workflow"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn hidden_task_is_excluded_from_render_snapshots() {
        let task_id = format!(
            "task-manager-hidden-test-{}",
            TASK_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        create_task_with_details_and_visibility(
            Some(task_id.clone()),
            "隐藏任务",
            None,
            "running",
            None,
            false,
            TaskVisibility::Hidden,
        );

        let snapshots = render_snapshots_limited(64, 64, 64);

        assert!(
            snapshots
                .active
                .iter()
                .chain(&snapshots.finished)
                .all(|snapshot| snapshot.id.as_ref() != task_id)
        );
        assert!(remove_task(&task_id));
    }
}
