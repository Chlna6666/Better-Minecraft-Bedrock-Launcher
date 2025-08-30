use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::time::Instant;
use serde_json::Value;
use crate::progress::install_progress::{emit_progress, format_speed, format_eta};

pub struct ExtractProgress {
    pub total: u64,
    pub extracted: Arc<AtomicU64>,
    start: Instant,
    last_emit: Instant,
    prev_extracted: u64,
    prev_elapsed: f64,
    first_loop: bool,
    smoothed_speed: f64,
    alpha: f64,
}

impl ExtractProgress {
    pub fn new(total: u64) -> Self {
        let now = Instant::now();
        Self {
            total,
            extracted: Arc::new(AtomicU64::new(0)),
            start: now,
            last_emit: now,
            prev_extracted: 0,
            prev_elapsed: 0.0,
            first_loop: true,
            smoothed_speed: 0.0,
            alpha: 0.3,
        }
    }

    pub fn with_extracted(total: u64, extracted: Arc<AtomicU64>) -> Self {
        let now = Instant::now();
        Self {
            total,
            extracted,
            start: now,
            last_emit: now,
            prev_extracted: 0,
            prev_elapsed: 0.0,
            first_loop: true,
            smoothed_speed: 0.0,
            alpha: 0.3,
        }
    }

    pub fn update(&self, bytes: u64) {
        self.extracted.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn should_emit(&self) -> bool {
        let done = self.extracted.load(Ordering::Relaxed);
        self.last_emit.elapsed() >= Duration::from_millis(300) || (self.total > 0 && done == self.total)
    }

    pub fn mark_emitted(&mut self) {
        self.last_emit = Instant::now();
    }

    pub fn snapshot(&self) -> (u64, u64, f64) {
        let elapsed = self.start.elapsed().as_secs_f64();
        (self.extracted.load(Ordering::Relaxed), self.total, elapsed)
    }

    pub fn get_speed(&mut self) -> f64 {
        let elapsed = self.start.elapsed().as_secs_f64();
        let done = self.extracted.load(Ordering::Relaxed);
        let delta_done = done.saturating_sub(self.prev_extracted);
        let delta_time = (elapsed - self.prev_elapsed).max(1e-6);

        let instant_speed = delta_done as f64 / delta_time;
        if self.first_loop || self.smoothed_speed == 0.0 {
            self.smoothed_speed = done as f64 / elapsed.max(1e-6);
        } else {
            self.smoothed_speed = self.alpha * instant_speed + (1.0 - self.alpha) * self.smoothed_speed;
        }

        self.update_prev();
        self.smoothed_speed
    }

    pub fn get_speed_str(&mut self) -> String {
        format_speed(self.get_speed() as u64, 1.0)
    }

    pub fn get_eta_str(&mut self) -> String {
        let done = self.extracted.load(Ordering::Relaxed);
        if self.total == 0 || done >= self.total {
            return format_eta(Some(self.total), done, 0.0);
        }
        let speed = self.get_speed().max(1e-6);
        let remaining = (self.total - done) as f64;
        format_eta(Some(self.total), done, remaining / speed)
    }

    pub(crate) fn update_prev(&mut self) {
        self.prev_extracted = self.extracted.load(Ordering::Relaxed);
        self.prev_elapsed = self.start.elapsed().as_secs_f64();
        self.first_loop = false;
    }

    pub fn extracted_arc(&self) -> Arc<AtomicU64> {
        self.extracted.clone()
    }
}

pub async fn report_extract_progress<V>(progress: &mut ExtractProgress, stage: V)
where
    V: Into<Value>,
{
    let (done, total, _) = progress.snapshot();
    let speed = progress.get_speed_str();
    let eta = progress.get_eta_str();
    let stage_val: Value = stage.into();

    let _ = emit_progress(done, Some(total), Some(&speed), Some(&eta), Some(stage_val)).await;
}
