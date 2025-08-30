use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::time::{Instant, Duration};
use serde_json::Value;
use crate::progress::install_progress::{emit_progress, format_speed, format_eta};

pub struct DownloadProgress {
    pub total: u64,
    pub(crate) downloaded: Arc<AtomicU64>,
    start: Instant,
    last_emit: Instant,
    prev_instant: Instant,
    prev_downloaded: u64,
    first_loop: bool,
    smoothed_speed: f64, // bytes / sec
    tau: f64,            // smoothing time constant (seconds)
}

impl DownloadProgress {
    pub fn new(total: u64) -> Self {
        let now = Instant::now();
        Self {
            total,
            downloaded: Arc::new(AtomicU64::new(0)),
            start: now,
            last_emit: now,
            prev_instant: now,
            prev_downloaded: 0,
            first_loop: true,
            smoothed_speed: 0.0,
            tau: 1.0,
        }
    }

    /// 增加已下载字节（线程安全）
    pub fn update(&self, bytes: usize) {
        self.downloaded.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// 是否需要发送一次 progress（节流）
    pub fn should_emit(&self) -> bool {
        let done = self.downloaded.load(Ordering::Relaxed);
        self.last_emit.elapsed() >= Duration::from_millis(300)
            || (self.total > 0 && done == self.total)
    }

    /// 标记已发送（用于节流）
    pub fn mark_emitted(&mut self) {
        self.last_emit = Instant::now();
    }

    /// 快照：返回 (done, total, elapsed_seconds_since_start)
    pub fn snapshot(&self) -> (u64, u64, f64) {
        let elapsed = self.start.elapsed().as_secs_f64();
        (self.downloaded.load(Ordering::Relaxed), self.total, elapsed)
    }

    /// 返回平滑后的速度（bytes/sec）
    pub fn get_speed(&mut self) -> f64 {
        let now = Instant::now();
        let elapsed_since_prev = now.duration_since(self.prev_instant).as_secs_f64();
        let done = self.downloaded.load(Ordering::Relaxed);

        // 避免极小时间间隔导致的噪声或除以 0
        let delta_time = if elapsed_since_prev <= 1e-9 { 1e-9 } else { elapsed_since_prev };

        let delta_done = (done.saturating_sub(self.prev_downloaded)) as f64;
        let instant_speed = if delta_done <= 0.0 { 0.0 } else { delta_done / delta_time };

        // alpha = 1 - exp(-delta_time / tau)
        let alpha = 1.0 - (-delta_time / self.tau).exp();

        if self.first_loop {
            // 初次以 instant 为初始平滑值
            self.smoothed_speed = instant_speed;
        } else {
            self.smoothed_speed = (1.0 - alpha) * self.smoothed_speed + alpha * instant_speed;
        }

        // 更新 prev
        self.prev_downloaded = done;
        self.prev_instant = now;
        self.first_loop = false;

        self.smoothed_speed
    }

    /// 返回速度字符串（例如 "1.23 MB/s"）
    pub fn get_speed_str(&mut self) -> String {
        let speed = self.get_speed();
        // 使用新的 format_speed
        format_speed(speed as u64, 1.0)
    }

    /// 计算并返回 ETA 字符串（HH:MM:SS），若无法估算返回 "unknown"
    pub fn get_eta_str(&mut self) -> String {
        let (done, total, elapsed) = self.snapshot();
        // 使用新的 format_eta
        format_eta(Some(total), done, elapsed)
    }

    pub fn downloaded_arc(&self) -> Arc<AtomicU64> {
        self.downloaded.clone()
    }
}

/// 报告 progress（异步）
pub async fn report_progress<V>(progress: &mut DownloadProgress, stage: V)
where
    V: Into<Value>,
{
    let (done, total, elapsed) = progress.snapshot();
    let speed_str = progress.get_speed_str();
    let eta_str = progress.get_eta_str();
    let stage_val: Value = stage.into();

    // 发送进度
    let _ = emit_progress(
        done,
        Some(total),
        Some(&speed_str),
        Some(&eta_str),
        Some(stage_val),
    )
        .await;

    // 标记已发送（更新 last_emit）
    progress.mark_emitted();
}
