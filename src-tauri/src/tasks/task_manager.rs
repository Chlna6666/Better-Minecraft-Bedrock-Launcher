// src/task/task_manager.rs
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::{debug, error};
use tauri::{AppHandle, Emitter, Manager};

// ==========================================
// 1. 全局 AppHandle 用于发送事件
// ==========================================
// 我们需要一个地方存 AppHandle，因为 task_manager 是静态/全局的，无法直接访问 context
static APP_HANDLE: Lazy<Mutex<Option<AppHandle>>> = Lazy::new(|| Mutex::new(None));

static TASK_COUNTER: AtomicU64 = AtomicU64::new(1);
static TASKS: Lazy<Mutex<HashMap<String, Task>>> = Lazy::new(|| Mutex::new(HashMap::new()));

// 初始化 AppHandle (需要在 main.rs setup 中调用)
pub fn init_task_manager(app: AppHandle) {
    let mut handle = APP_HANDLE.lock().unwrap();
    *handle = Some(app);
    debug!("任务管理器: AppHandle 已初始化，事件推送系统就绪");
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TaskSnapshot {
    pub id: String,
    pub stage: String,
    pub total: Option<u64>,
    pub done: u64,
    /// 0.0 if unknown
    pub speed_bytes_per_sec: f64,
    /// formatted "HH:MM:SS" or "unknown"
    pub eta: String,
    /// percentage 0.0..100.0 or None when unknown
    pub percent: Option<f64>,
    pub status: String, // "running" | "completed" | "cancelled" | "error"
    pub message: Option<String>,
    pub started_at_unix: u64,
    pub last_update_unix: u64,
}

struct Task {
    id: String,
    stage: String,
    total: Option<u64>,
    done: u64,
    start_instant: Instant,
    last_instant: Instant,
    last_done: u64,
    speed_ema: f64, // bytes/sec exponential moving average
    status: String,
    message: Option<String>,
    cancelled: bool,
    last_emit_instant: Instant,
}

impl Task {
    fn new(id: String, stage: &str, total: Option<u64>) -> Self {
        let now = Instant::now();
        Self {
            id,
            stage: stage.to_string(),
            total,
            done: 0,
            start_instant: now,
            last_instant: now,
            last_done: 0,
            speed_ema: 0.0,
            status: "running".to_string(),
            message: None,
            cancelled: false,
            last_emit_instant: now,
        }
    }

    fn snapshot(&self) -> TaskSnapshot {
        // 如果 total 已知，则用 clamp 后的 done 计算 percent/eta
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
                    format_duration_hms(Duration::from_secs_f64(remain / self.speed_ema))
                } else {
                    "unknown".to_string()
                };
                (Some(percent), eta, done_clamped)
            }
            None => (None, "unknown".to_string(), self.done),
        };

        TaskSnapshot {
            id: self.id.clone(),
            stage: self.stage.clone(),
            total: self.total,
            done: done_clamped,
            speed_bytes_per_sec: self.speed_ema,
            eta: eta_str,
            percent: percent_opt,
            status: self.status.clone(),
            message: self.message.clone(),
            started_at_unix: unix_now_seconds() - (self.start_instant.elapsed().as_secs()),
            last_update_unix: unix_now_seconds(),
        }
    }
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

pub fn create_task(id_opt: Option<String>, initial_stage: &str, total: Option<u64>) -> String {
    let id = id_opt.unwrap_or_else(|| {
        let id_num = TASK_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("task-{}-{}", id_num, unix_now_seconds())
    });
    let task = Task::new(id.clone(), initial_stage, total);

    // 创建时获取一次快照用于立即推送
    let initial_snapshot = task.snapshot();

    let mut map = TASKS.lock().unwrap();
    map.insert(id.clone(), task);
    drop(map); // 尽早释放锁

    debug!("任务管理器: 已创建任务 ID: {}", id);
    // 立即推送初始状态
    emit_task_update(initial_snapshot);

    id
}

pub fn update_progress(task_id: &str, delta_bytes: u64, total: Option<u64>, stage: Option<&str>) {
    const MIN_UPDATE_INTERVAL: f64 = 0.5; // 计算速度的间隔
    const IDLE_RESET_SECONDS: f64 = 5.0;
    const EMA_ALPHA: f64 = 0.3;
    const EMIT_INTERVAL_MS: u128 = 200; // [设置] 推送频率 200ms
    // 准备一个变量存 Snapshot，以便在锁外推送
    let mut snapshot_to_emit: Option<TaskSnapshot> = None;

    {
        let mut map = TASKS.lock().unwrap();
        if let Some(t) = map.get_mut(task_id) {
            if let Some(tt) = total { t.total = Some(tt); }
            if let Some(s) = stage { t.stage = s.to_string(); }

            let now = Instant::now();
            let elapsed = now.duration_since(t.last_instant).as_secs_f64();

            t.done = t.done.saturating_add(delta_bytes);
            if let Some(tot) = t.total {
                if t.done > tot { t.done = tot; }
            }

            // 计算速度 (保持原逻辑)
            if elapsed >= MIN_UPDATE_INTERVAL {
                let diff = t.done.saturating_sub(t.last_done);
                if diff > 0 {
                    let inst_speed = (diff as f64) / elapsed;
                    if t.speed_ema <= 0.0 { t.speed_ema = inst_speed; }
                    else { t.speed_ema = t.speed_ema * (1.0 - EMA_ALPHA) + inst_speed * EMA_ALPHA; }
                    t.last_instant = now;
                    t.last_done = t.done;
                } else if elapsed >= IDLE_RESET_SECONDS {
                    t.speed_ema = 0.0;
                    t.last_instant = now;
                    t.last_done = t.done;
                }
            }

            // [新增] 检查是否需要推送事件 (节流 200ms)
            // 或者是 stage 发生变化（stage 变化通常是重要节点，立即推送）
            let stage_changed = stage.is_some();
            if stage_changed || now.duration_since(t.last_emit_instant).as_millis() >= EMIT_INTERVAL_MS {
                t.last_emit_instant = now;
                snapshot_to_emit = Some(t.snapshot());
            }
        }
    } // 锁在这里释放

    // 在锁外发送事件，避免阻塞
    if let Some(snap) = snapshot_to_emit {
        emit_task_update(snap);
    }
}

/// Set a known total (useful if content-length arrives later)
pub fn set_total(task_id: &str, total: Option<u64>) {
    let mut map = TASKS.lock().unwrap();
    if let Some(t) = map.get_mut(task_id) {
        t.total = total;
        // 如果 done 已经大于新 total，则修正 done（避免 percent>100）
        if let Some(tot) = total {
            if t.done > tot {
                t.done = tot;
                // 同步 last_done 以防下一次速度计算把超额部分当作新增
                if t.last_done > t.done {
                    t.last_done = t.done;
                }
            }
        }
    }
}

/// Mark task finished with status: "completed" | "cancelled" | "error"
pub fn finish_task(task_id: &str, status: &str, message: Option<String>) {
    let mut snapshot_to_emit: Option<TaskSnapshot> = None;
    {
        let mut map = TASKS.lock().unwrap();
        if let Some(t) = map.get_mut(task_id) {
            t.status = status.to_string();
            t.message = message.clone();
            // 完成状态必须强制推送
            snapshot_to_emit = Some(t.snapshot());
        }
    }

    if let Some(snap) = snapshot_to_emit {
        debug!("任务管理器: 任务 {} 结束，状态: {}", task_id, status);
        emit_task_update(snap);
    }
}

/// Cancel a task (sets cancelled flag). It's up to caller to check this flag and stop work.
pub fn cancel_task(task_id: &str) {
    let mut snapshot_to_emit: Option<TaskSnapshot> = None;
    {
        let mut map = TASKS.lock().unwrap();
        if let Some(t) = map.get_mut(task_id) {
            t.cancelled = true;
            t.status = "cancelled".to_string();
            snapshot_to_emit = Some(t.snapshot());
            debug!("任务管理器: 任务 {} 已标记为取消", task_id);
        }
    }
    if let Some(snap) = snapshot_to_emit {
        emit_task_update(snap);
    }
}

/// Check whether a task is cancelled
pub fn is_cancelled(task_id: &str) -> bool {
    let map = TASKS.lock().unwrap();
    map.get(task_id).map(|t| t.cancelled).unwrap_or(false)
}

/// Get snapshot (clone) for front-end consumption
pub fn get_snapshot(task_id: &str) -> Option<TaskSnapshot> {
    let map = TASKS.lock().unwrap();
    map.get(task_id).map(|t| t.snapshot())
}

fn emit_task_update(snapshot: TaskSnapshot) {
    tokio::spawn(async move {
        let handle_guard = APP_HANDLE.lock().unwrap();
        if let Some(handle) = handle_guard.as_ref() {
            let event_name = format!("task-update::{}", snapshot.id);
            // 调试时可以打开下面这行，确认后端确实执行到了 emit
            // debug!("正在推送事件: {} -> Status: {}", event_name, snapshot.status);

            if let Err(e) = handle.emit(&event_name, snapshot) {
                error!("任务管理器: 推送事件失败 [{}]: {}", event_name, e);
            }
        } else {
            // [关键] 如果控制台看到这条红字，说明 main.rs 没初始化！
            error!("任务管理器错误: APP_HANDLE 未初始化！请在 main.rs 中调用 init_task_manager");
        }
    });
}