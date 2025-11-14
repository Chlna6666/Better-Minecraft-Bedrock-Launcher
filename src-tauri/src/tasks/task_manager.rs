// src/task/task_manager.rs
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tracing::debug;

static TASK_COUNTER: AtomicU64 = AtomicU64::new(1);
static TASKS: Lazy<Mutex<HashMap<String, Task>>> = Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Clone, Serialize, Deserialize)]
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
        }
    }

    fn snapshot(&self) -> TaskSnapshot {
        // 如果 total 已知，则用 clamp 后的 done 计算 percent/eta
        let (percent_opt, eta_str, done_clamped) = match self.total {
            Some(tot) => {
                let done_clamped = std::cmp::min(self.done, tot);
                let percent = if tot == 0 { 0.0 } else { (done_clamped as f64 / tot as f64) * 100.0 };
                let eta = if self.speed_ema > 1e-6 && tot > done_clamped {
                    let remain = (tot - done_clamped) as f64;
                    format_duration_hms(Duration::from_secs_f64(remain / self.speed_ema))
                } else {
                    "unknown".to_string()
                };
                (Some(percent), eta, done_clamped)
            }
            None => {
                // total unknown -> percent None, eta unknown, done_clamped = done
                (None, "unknown".to_string(), self.done)
            }
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
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn format_duration_hms(d: Duration) -> String {
    let secs = d.as_secs();
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{:02}:{:02}:{:02}", h, m, s)
}

/// Create and register a new task; returns generated task_id.
pub fn create_task(initial_stage: &str, total: Option<u64>) -> String {
    let id_num = TASK_COUNTER.fetch_add(1, Ordering::Relaxed);
    let id = format!("task-{}-{}", id_num, unix_now_seconds());
    let task = Task::new(id.clone(), initial_stage, total);
    let mut map = TASKS.lock().unwrap();
    map.insert(id.clone(), task);
    id
}

pub fn update_progress(task_id: &str, delta_bytes: u64, total: Option<u64>, stage: Option<&str>) {
    const MIN_UPDATE_INTERVAL: f64 = 0.5; // seconds
    const IDLE_RESET_SECONDS: f64 = 5.0;
    const EMA_ALPHA: f64 = 0.3;

    let mut map = TASKS.lock().unwrap();
    if let Some(t) = map.get_mut(task_id) {
        // update total if provided
        if let Some(tt) = total {
            t.total = Some(tt);
        }

        // update stage if provided
        if let Some(s) = stage {
            t.stage = s.to_string();
        }

        // time delta
        let now = Instant::now();
        let elapsed = now.duration_since(t.last_instant).as_secs_f64();

        // update done (累加)
        t.done = t.done.saturating_add(delta_bytes);

        // 如果已知 total，确保 done 不超过 total（clamp）
        if let Some(tot) = t.total {
            if t.done > tot {
                t.done = tot;
            }
        }

        // 只有在足够时间间隔才更新速度 EMA，避免瞬时速率因 very small elapsed 导致异常
        if elapsed >= MIN_UPDATE_INTERVAL {
            let diff = t.done.saturating_sub(t.last_done);
            if diff > 0 {
                let inst_speed = (diff as f64) / elapsed;
                if t.speed_ema <= 0.0 {
                    t.speed_ema = inst_speed;
                } else {
                    t.speed_ema = t.speed_ema * (1.0 - EMA_ALPHA) + inst_speed * EMA_ALPHA;
                }
                // commit last_*
                t.last_instant = now;
                t.last_done = t.done;
            } else {
                // 没有新增字节
                if elapsed >= IDLE_RESET_SECONDS {
                    t.speed_ema = 0.0;
                    t.last_instant = now;
                    t.last_done = t.done;
                }
                // 否则保持之前的 speed_ema，不更新 last_*
            }
        }
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
    let mut map = TASKS.lock().unwrap();
    if let Some(t) = map.get_mut(task_id) {
        t.status = status.to_string();
        t.message = message;
    }
}

/// Cancel a task (sets cancelled flag). It's up to caller to check this flag and stop work.
pub fn cancel_task(task_id: &str) {
    let mut map = TASKS.lock().unwrap();
    if let Some(t) = map.get_mut(task_id) {
        t.cancelled = true;
        t.status = "cancelled".to_string();
        debug!("task_manager: task {} marked cancelled", task_id);
    } else {
        debug!("task_manager: cancel_task called but task not found: {}", task_id);
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
