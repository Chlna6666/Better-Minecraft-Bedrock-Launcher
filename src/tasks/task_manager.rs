use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, broadcast};
use tokio::task::AbortHandle;
use tracing::debug;

const TASK_EMIT_INTERVAL_MS: u128 = 250;
const TASK_PROGRESS_MIN_UPDATE_INTERVAL: f64 = 0.25;
const TASK_PROGRESS_IDLE_RESET_SECONDS: f64 = 2.0;
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
static TASK_STAGE_LABELS: Lazy<RwLock<HashMap<Arc<str>, Arc<str>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));
static TASK_LOGS: Lazy<Mutex<HashMap<Arc<str>, VecDeque<Arc<str>>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

static TASK_UPDATES: Lazy<broadcast::Sender<Arc<TaskSnapshot>>> = Lazy::new(|| {
    // Best-effort: drop updates if there are no receivers, or buffer overflow happens.
    let (tx, _rx) = broadcast::channel(256);
    tx
});

#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadVisualization {
    pub index: u32,
    pub label: Option<String>,
    pub active: bool,
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
    pub visualization: Option<TaskVisualization>,
    pub started_at_unix: u64,
    pub last_update_unix: u64,
    #[serde(default)]
    pub sequence: u64,
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
    visualization: TaskVisualization,
    last_emit_instant: Instant,
    sequence: u64,
}

impl Task {
    fn new(
        id: Arc<str>,
        title: Arc<str>,
        detail: Option<Arc<str>>,
        stage: &str,
        total: Option<u64>,
        supports_pause: bool,
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
            visualization: TaskVisualization::default(),
            last_emit_instant: now,
            sequence: 0,
        }
    }

    fn touch(&mut self) {
        self.sequence = self.sequence.saturating_add(1);
    }

    fn snapshot(&self) -> TaskSnapshot {
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
            stage: localize_task_stage(self.stage.as_ref()),
            total: self.total,
            done: done_clamped,
            speed_bytes_per_sec: self.speed_ema,
            eta: eta_str,
            percent: percent_opt,
            status: self.status.clone(),
            cancel_requested: self.cancel_requested,
            message: self.message.clone(),
            supports_pause: self.supports_pause,
            visualization: (!self.visualization.is_empty()).then(|| self.visualization.clone()),
            started_at_unix: unix_now_seconds()
                .saturating_sub(self.start_instant.elapsed().as_secs()),
            last_update_unix: unix_now_seconds(),
            sequence: self.sequence,
        }
    }

    fn should_emit(&self, now: Instant) -> bool {
        now.duration_since(self.last_emit_instant).as_millis() as u128 >= TASK_EMIT_INTERVAL_MS
    }

    fn mark_emitted(&mut self, now: Instant) {
        self.last_emit_instant = now;
    }
}

fn localize_task_stage(stage: &str) -> Arc<str> {
    if let Ok(stage_labels) = TASK_STAGE_LABELS.read()
        && let Some(label) = stage_labels.get(stage)
    {
        return label.clone();
    }

    match stage {
        "ready" => Arc::<str>::from("等待开始"),
        "queued" => Arc::<str>::from("排队中"),
        "starting" => Arc::<str>::from("准备中"),
        other => Arc::<str>::from(other),
    }
}

fn default_task_title(_stage: &str) -> Arc<str> {
    Arc::<str>::from("后台任务")
}

fn is_terminal_status(status: &str) -> bool {
    matches!(status, "completed" | "cancelled" | "error")
}

pub fn subscribe_task_updates() -> broadcast::Receiver<Arc<TaskSnapshot>> {
    TASK_UPDATES.subscribe()
}

pub fn task_visualization_enabled() -> bool {
    TASK_VISUALIZATION_ENABLED
}

pub fn register_task_stage_labels(labels: impl IntoIterator<Item = (&'static str, &'static str)>) {
    let Ok(mut stage_labels) = TASK_STAGE_LABELS.write() else {
        return;
    };
    for (stage, label) in labels {
        stage_labels.insert(Arc::<str>::from(stage), Arc::<str>::from(label));
    }
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

pub fn create_task(id_opt: Option<String>, initial_stage: &str, total: Option<u64>) -> String {
    create_task_with_options(id_opt, initial_stage, total, false)
}

pub fn create_task_with_details(
    id_opt: Option<String>,
    title: impl Into<String>,
    detail: Option<String>,
    initial_stage: &str,
    total: Option<u64>,
    supports_pause: bool,
) -> String {
    let id = id_opt.unwrap_or_else(|| {
        let id_num = TASK_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("task-{}-{}", id_num, unix_now_seconds())
    });
    let id_arc: Arc<str> = Arc::from(id.as_str());

    let task = Task::new(
        id_arc,
        Arc::from(title.into()),
        detail.map(Arc::<str>::from),
        initial_stage,
        total,
        supports_pause,
    );
    let mut task = task;
    task.touch();
    let snapshot = task.snapshot();

    let mut map = TASKS.lock().unwrap();
    map.insert(id.clone(), task);
    drop(map);

    register_task_control(id.clone(), Arc::new(TaskControl::new()));

    debug!("task_manager: created task {}", id);
    emit_task_update(snapshot);
    id
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
    let mut logs = TASK_LOGS.lock().unwrap();
    let task_id: Arc<str> = Arc::from(task_id);
    let task_logs = logs.entry(task_id).or_default();
    if task_logs.len() >= TASK_LOG_LIMIT {
        task_logs.pop_front();
    }
    task_logs.push_back(Arc::from(line.into()));
    true
}

pub fn task_logs(task_id: &str) -> Arc<[Arc<str>]> {
    let logs = TASK_LOGS.lock().unwrap();
    logs.get(task_id)
        .map(|entries| Arc::<[Arc<str>]>::from(entries.iter().cloned().collect::<Vec<_>>()))
        .unwrap_or_else(|| Arc::<[Arc<str>]>::from([]))
}

pub fn update_progress(task_id: &str, delta_bytes: u64, total: Option<u64>, stage: Option<&str>) {
    let mut snapshot_to_emit: Option<TaskSnapshot> = None;

    {
        let mut map = TASKS.lock().unwrap();
        if let Some(t) = map.get_mut(task_id) {
            if is_terminal_status(t.status.as_ref()) {
                return;
            }
            let stage_changed = stage.is_some_and(|next_stage| next_stage != t.stage.as_ref());
            if let Some(tt) = total {
                t.total = Some(tt);
            }
            if let Some(s) = stage {
                t.stage = Arc::from(s);
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

            if elapsed >= TASK_PROGRESS_MIN_UPDATE_INTERVAL {
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
                } else if elapsed >= TASK_PROGRESS_IDLE_RESET_SECONDS {
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
        emit_task_update(snap);
    }
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
            t.visualization = TaskVisualization::default();
            if let Some(stage) = stage {
                t.stage = Arc::from(stage);
            }
            t.touch();
            snapshot_to_emit = Some(t.snapshot());
        }
    }

    if let Some(snap) = snapshot_to_emit {
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
            if task.visualization != next_visualization {
                task.visualization = next_visualization;
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
    {
        let mut map = TASKS.lock().unwrap();
        if let Some(t) = map.get_mut(task_id) {
            if is_terminal_status(t.status.as_ref()) && t.status.as_ref() != status {
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
            if t.visualization.worker_total.is_some() || t.visualization.worker_active.is_some() {
                t.visualization.worker_active = Some(0);
            }
            if status == "completed" {
                if let Some(unit_total) = t.visualization.unit_total {
                    t.visualization.unit_done = Some(unit_total);
                }
            }
            t.visualization.current_item = None;
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
    }
}

pub fn cancel_task(task_id: &str) {
    run_task_cancel_hook(task_id);
    let _ = abort_task(task_id);
    let mut snapshot_to_emit: Option<TaskSnapshot> = None;
    {
        let mut map = TASKS.lock().unwrap();
        if let Some(t) = map.get_mut(task_id) {
            if is_terminal_status(t.status.as_ref()) {
                return;
            }
            t.cancel_requested = true;
            t.paused = false;
            t.status = Arc::<str>::from("cancelled");
            t.message = Some(Arc::<str>::from("user cancelled"));
            t.speed_ema = 0.0;
            t.last_done = t.done;
            t.last_instant = Instant::now();
            t.visualization.current_item = Some("正在取消并清理资源".to_string());
            if t.visualization.worker_total.is_some() || t.visualization.worker_active.is_some() {
                t.visualization.worker_active = Some(0);
            }
            t.touch();
            snapshot_to_emit = Some(t.snapshot());
        }
    }

    if let Some(snap) = snapshot_to_emit {
        emit_task_update(snap);
    }

    if let Some(control) = task_control(task_id) {
        control.cancel();
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

pub fn remove_task(task_id: &str) -> bool {
    clear_task_abort_handle(task_id);
    clear_task_cancel_hook(task_id);
    let _ = TASK_SNAPSHOTS.write().unwrap().remove(task_id);
    TASK_LOGS.lock().unwrap().remove(task_id);
    let _ = TASK_CONTROLS
        .write()
        .ok()
        .and_then(|mut map| map.remove(task_id));
    let mut map = TASKS.lock().unwrap();
    map.remove(task_id).is_some()
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
        match snapshot.status.as_ref() {
            "running" | "paused" | "cancelling" => {
                active_total += 1;
                insert_sorted_limited(&mut active, snapshot, active_limit);
            }
            "completed" | "cancelled" | "error" => {
                finished_total += 1;
                insert_sorted_limited(&mut finished, snapshot, finished_storage_limit);
            }
            _ => {}
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
        map.insert(snapshot.id.clone(), snapshot.clone());
    }

    if TASK_UPDATES.receiver_count() == 0 {
        return;
    }

    let _ = TASK_UPDATES.send(snapshot);
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
    fn registered_task_stage_label_localizes_snapshot() {
        let task_id = format!(
            "task-manager-stage-label-test-{}",
            TASK_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let stage = "task_manager_registered_stage_label_test";
        register_task_stage_labels([(stage, "注册阶段文案")]);

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
            "注册阶段文案"
        );
        assert!(remove_task(&task_id));
    }
}
