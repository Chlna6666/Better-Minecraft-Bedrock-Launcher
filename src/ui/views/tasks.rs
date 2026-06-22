use crate::tasks::task_manager::{self, TaskSnapshot};
use gpui::{Context, RenderFingerprint, SharedString, Subscription, Task, Timer};
use std::collections::HashMap;
use std::collections::HashSet;
use std::hash::Hash;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[path = "tasks/data.rs"]
mod data;
#[path = "tasks/render.rs"]
mod render;

pub use render::render_tasks_overlay;

const TASKS_PAGE_POLL_INTERVAL_MS: u64 = 500;
const MAX_RENDER_ACTIVE_TASKS: usize = 12;
const MAX_FINISHED_TASKS_IN_LIST: usize = 120;
const MAX_RENDER_FINISHED_TASKS: usize = 20;
const DISABLE_TASKS_PAGE_LAYOUT_NOTIFY: bool = false;
const TASK_CARD_ENTER_ANIMATION_MS: u64 = 240;
const TASK_CARD_COMPLETE_ANIMATION_MS: u64 = 320;
const TASK_CARD_WARNING_ANIMATION_MS: u64 = 220;
const TASK_CARD_EXIT_ANIMATION_MS: u64 = 220;
const TASK_FINISHED_HOLD_COMPLETED_MS: u64 = 1_200;
const TASK_FINISHED_HOLD_CANCELLED_BY_USER_MS: u64 = 400;
const TASK_FINISHED_HOLD_TERMINAL_DEFAULT_MS: u64 = 800;

pub struct TasksPageView {
    _subscriptions: Vec<Subscription>,
    confirm_dialog: Option<TaskConfirmDialog>,
    update_apply_task: Option<Task<anyhow::Result<()>>>,
    render_model: TasksPageRenderModel,
    card_motions: HashMap<Arc<str>, TaskCardMotionState>,
    finished_hold_until: HashMap<Arc<str>, Instant>,
    hidden_finished_ids: HashSet<Arc<str>>,
    user_cancelled_ids: HashSet<Arc<str>>,
    pending_exit_motions: HashMap<Arc<str>, TaskCardMotionKind>,
    transition_cards: HashMap<Arc<str>, TaskTransitionCard>,
    motion_sequence: u64,
    active: bool,
}

#[derive(Clone)]
pub(super) enum TaskConfirmAction {
    CancelTask,
    RemoveTask,
    DeleteDownloadFile,
}

#[derive(Clone)]
struct TaskConfirmDialog {
    task_id: Arc<str>,
    title: SharedString,
    description: SharedString,
    action: TaskConfirmAction,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskCardMotionKind {
    Enter,
    Complete,
    Warn,
    Exit,
}

#[derive(Clone)]
struct TaskCardMotionState {
    kind: TaskCardMotionKind,
    sequence: u64,
}

#[derive(Clone)]
pub(super) struct TaskTransitionCard {
    model: TaskCardViewModel,
    motion: TaskCardMotionKind,
    sequence: u64,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct TaskCardViewModel {
    pub(super) id: Arc<str>,
    pub(super) title: Arc<str>,
    pub(super) detail: Option<Arc<str>>,
    pub(super) stage: Arc<str>,
    pub(super) status: Arc<str>,
    pub(super) worker_total: Option<u32>,
    pub(super) worker_active: Option<u32>,
    pub(super) percent_text: Arc<str>,
    pub(super) amount_text: Arc<str>,
    pub(super) speed_text: Option<Arc<str>>,
    pub(super) eta_text: Option<Arc<str>>,
    pub(super) message: Option<Arc<str>>,
    pub(super) percent_basis_points: Option<u16>,
    pub(super) started_at_unix: u64,
    pub(super) last_update_unix: u64,
    pub(super) can_pause: bool,
    pub(super) can_cancel: bool,
    pub(super) can_remove: bool,
}

#[derive(Clone)]
pub(super) struct TasksPageRenderModel {
    pub(super) loading: bool,
    pub(super) signature: u64,
    pub(super) total_count: usize,
    pub(super) active_total: usize,
    pub(super) finished_total: usize,
    pub(super) thread_total: usize,
    pub(super) active: Vec<TaskCardViewModel>,
    pub(super) finished: Vec<TaskCardViewModel>,
}

impl TasksPageRenderModel {
    fn loading() -> Self {
        Self {
            loading: true,
            signature: 0,
            total_count: 0,
            active_total: 0,
            finished_total: 0,
            thread_total: 0,
            active: Vec::new(),
            finished: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TaskRenderBucket {
    Active,
    Finished,
    Hidden,
}

struct TaskRenderEntry {
    started_at_unix: u64,
    sequence: u64,
    model: TaskCardViewModel,
}

struct TaskRenderIndexEntry {
    bucket: TaskRenderBucket,
    sequence: u64,
}

struct TaskRenderCache {
    task_index: HashMap<Arc<str>, TaskRenderIndexEntry>,
    active_entries: HashMap<Arc<str>, TaskRenderEntry>,
    finished_entries: HashMap<Arc<str>, TaskRenderEntry>,
    active_total: usize,
    finished_total: usize,
    last_signature: Option<u64>,
}

impl TaskRenderEntry {
    fn from_snapshot(snapshot: &TaskSnapshot) -> Self {
        Self {
            started_at_unix: snapshot.started_at_unix,
            sequence: snapshot.sequence,
            model: build_task_card_model(snapshot),
        }
    }
}

impl TaskRenderCache {
    fn new() -> Self {
        Self {
            task_index: HashMap::new(),
            active_entries: HashMap::new(),
            finished_entries: HashMap::new(),
            active_total: 0,
            finished_total: 0,
            last_signature: None,
        }
    }

    fn from_snapshots(snapshots: Vec<Arc<TaskSnapshot>>) -> Self {
        let mut this = Self::new();
        for snapshot in snapshots {
            this.apply_snapshot(snapshot.as_ref());
        }
        this
    }

    fn mark_rendered(&mut self, signature: u64) {
        self.last_signature = Some(signature);
    }

    fn apply_snapshot(&mut self, snapshot: &TaskSnapshot) {
        let next_bucket = task_render_bucket(snapshot.status.as_ref());
        let next_sequence = snapshot.sequence;
        let previous_bucket = self.task_index.get(snapshot.id.as_ref()).and_then(|entry| {
            if next_sequence < entry.sequence {
                None
            } else {
                Some(entry.bucket)
            }
        });
        if self
            .task_index
            .get(snapshot.id.as_ref())
            .is_some_and(|entry| next_sequence < entry.sequence)
        {
            return;
        }

        if let Some(previous_bucket) = previous_bucket {
            if previous_bucket != next_bucket {
                self.decrement_bucket(previous_bucket);
                self.increment_bucket(next_bucket);
            }
        } else if next_bucket != TaskRenderBucket::Hidden {
            self.increment_bucket(next_bucket);
        }

        match next_bucket {
            TaskRenderBucket::Active => {
                self.task_index.insert(
                    snapshot.id.clone(),
                    TaskRenderIndexEntry {
                        bucket: next_bucket,
                        sequence: next_sequence,
                    },
                );
                self.active_entries.insert(
                    snapshot.id.clone(),
                    TaskRenderEntry::from_snapshot(snapshot),
                );
                self.finished_entries.remove(snapshot.id.as_ref());
            }
            TaskRenderBucket::Finished => {
                self.task_index.insert(
                    snapshot.id.clone(),
                    TaskRenderIndexEntry {
                        bucket: next_bucket,
                        sequence: next_sequence,
                    },
                );
                self.finished_entries.insert(
                    snapshot.id.clone(),
                    TaskRenderEntry::from_snapshot(snapshot),
                );
                self.active_entries.remove(snapshot.id.as_ref());
                self.trim_finished_entries();
            }
            TaskRenderBucket::Hidden => {
                self.task_index.remove(snapshot.id.as_ref());
                self.active_entries.remove(snapshot.id.as_ref());
                self.finished_entries.remove(snapshot.id.as_ref());
            }
        }
    }

    fn take_changed_render_model(&mut self) -> Option<TasksPageRenderModel> {
        let next_render_model = self.build_render_model();
        if self.last_signature == Some(next_render_model.signature) {
            return None;
        }

        self.last_signature = Some(next_render_model.signature);
        Some(next_render_model)
    }

    fn build_render_model(&self) -> TasksPageRenderModel {
        let mut active_entries = Vec::with_capacity(MAX_RENDER_ACTIVE_TASKS);
        let mut finished_entries =
            Vec::with_capacity(MAX_FINISHED_TASKS_IN_LIST.min(MAX_RENDER_FINISHED_TASKS));

        for entry in self.active_entries.values() {
            insert_sorted_limited_entries(&mut active_entries, entry, MAX_RENDER_ACTIVE_TASKS);
        }
        for entry in self.finished_entries.values() {
            insert_sorted_limited_entries(&mut finished_entries, entry, MAX_FINISHED_TASKS_IN_LIST);
        }

        if finished_entries.len() > MAX_RENDER_FINISHED_TASKS {
            finished_entries.truncate(MAX_RENDER_FINISHED_TASKS);
        }

        let active: Vec<_> = active_entries
            .into_iter()
            .map(|entry| entry.model.clone())
            .collect();
        let finished: Vec<_> = finished_entries
            .into_iter()
            .map(|entry| entry.model.clone())
            .collect();
        let signature = compute_render_model_signature(
            false,
            self.active_total,
            self.finished_total,
            0,
            &active,
            &finished,
        );

        TasksPageRenderModel {
            loading: false,
            signature,
            total_count: self.active_total + self.finished_total,
            active_total: self.active_total,
            finished_total: self.finished_total,
            thread_total: 0,
            active,
            finished,
        }
    }

    fn increment_bucket(&mut self, bucket: TaskRenderBucket) {
        match bucket {
            TaskRenderBucket::Active => {
                self.active_total += 1;
            }
            TaskRenderBucket::Finished => {
                self.finished_total += 1;
            }
            TaskRenderBucket::Hidden => {}
        }
    }

    fn decrement_bucket(&mut self, bucket: TaskRenderBucket) {
        match bucket {
            TaskRenderBucket::Active => {
                self.active_total = self.active_total.saturating_sub(1);
            }
            TaskRenderBucket::Finished => {
                self.finished_total = self.finished_total.saturating_sub(1);
            }
            TaskRenderBucket::Hidden => {}
        }
    }

    fn trim_finished_entries(&mut self) {
        while self.finished_entries.len() > MAX_FINISHED_TASKS_IN_LIST {
            let Some(oldest_id) = self
                .finished_entries
                .iter()
                .min_by(|(left_id, left_entry), (right_id, right_entry)| {
                    left_entry
                        .started_at_unix
                        .cmp(&right_entry.started_at_unix)
                        .then_with(|| left_id.as_ref().cmp(right_id.as_ref()))
                })
                .map(|(task_id, _)| task_id.clone())
            else {
                break;
            };
            self.finished_entries.remove(oldest_id.as_ref());
        }
    }
}

fn is_entity_released_error(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string().contains("entity released"))
}

fn task_subject(snapshot: &TaskSnapshot) -> String {
    match snapshot.detail.as_deref() {
        Some(detail) if !detail.trim().is_empty() => {
            format!("{} {}", snapshot.title, detail.trim())
        }
        _ => snapshot.title.as_ref().to_owned(),
    }
}

fn is_generic_task_title(title: &str) -> bool {
    matches!(
        title,
        "下载游戏包"
            | "下载资源文件"
            | "下载缓存文件"
            | "导入安装包"
            | "安装游戏"
            | "下载任务"
            | "安装任务"
            | "后台任务"
    )
}

fn non_empty_arc(text: Option<&Arc<str>>) -> Option<Arc<str>> {
    text.filter(|value| !value.trim().is_empty()).cloned()
}

fn format_task_amount(done: u64, total: Option<u64>) -> Arc<str> {
    Arc::from(match total {
        Some(total) => format!(
            "{} / {}",
            crate::utils::format_bytes::format_bytes_compact(done),
            crate::utils::format_bytes::format_bytes_compact(total)
        ),
        None => format!(
            "已完成 {}",
            crate::utils::format_bytes::format_bytes_compact(done)
        ),
    })
}

fn format_task_speed(speed_bytes_per_sec: f64) -> Arc<str> {
    const KILOBYTE: f64 = 1024.0;
    const MEGABYTE: f64 = KILOBYTE * 1024.0;
    const GIGABYTE: f64 = MEGABYTE * 1024.0;

    let speed_text = if speed_bytes_per_sec < KILOBYTE {
        format!("{:.0} B/s", speed_bytes_per_sec.round())
    } else if speed_bytes_per_sec < MEGABYTE {
        format!(
            "{:.1} KB/s",
            (speed_bytes_per_sec / KILOBYTE * 10.0).round() / 10.0
        )
    } else if speed_bytes_per_sec < GIGABYTE {
        format!(
            "{:.1} MB/s",
            (speed_bytes_per_sec / MEGABYTE * 10.0).round() / 10.0
        )
    } else {
        format!(
            "{:.1} GB/s",
            (speed_bytes_per_sec / GIGABYTE * 10.0).round() / 10.0
        )
    };

    Arc::from(speed_text)
}

fn task_render_bucket(status: &str) -> TaskRenderBucket {
    match status {
        "running" | "paused" | "cancelling" => TaskRenderBucket::Active,
        "completed" | "cancelled" | "error" => TaskRenderBucket::Finished,
        _ => TaskRenderBucket::Hidden,
    }
}

fn build_task_card_model(snapshot: &TaskSnapshot) -> TaskCardViewModel {
    let display_title = if is_generic_task_title(snapshot.title.as_ref()) {
        non_empty_arc(snapshot.detail.as_ref()).unwrap_or_else(|| snapshot.title.clone())
    } else {
        snapshot.title.clone()
    };

    TaskCardViewModel {
        id: snapshot.id.clone(),
        title: display_title,
        detail: non_empty_arc(snapshot.detail.as_ref()),
        stage: snapshot.stage.clone(),
        status: snapshot.status.clone(),
        worker_total: snapshot
            .visualization
            .as_ref()
            .and_then(|visualization| visualization.worker_total),
        worker_active: snapshot
            .visualization
            .as_ref()
            .and_then(|visualization| visualization.worker_active),
        percent_text: Arc::from(
            snapshot
                .percent
                .map(|value| format!("{value:.0}%"))
                .unwrap_or_else(|| "进行中".to_string()),
        ),
        amount_text: format_task_amount(snapshot.done, snapshot.total),
        speed_text: (snapshot.speed_bytes_per_sec > 0.0)
            .then(|| format_task_speed(snapshot.speed_bytes_per_sec)),
        eta_text: (snapshot.eta.as_ref() != "unknown").then(|| snapshot.eta.clone()),
        message: non_empty_arc(snapshot.message.as_ref()),
        percent_basis_points: snapshot
            .percent
            .map(|value| (value.clamp(0.0, 100.0) * 100.0).round() as u16),
        started_at_unix: snapshot.started_at_unix,
        last_update_unix: snapshot.last_update_unix,
        can_pause: snapshot.supports_pause
            && matches!(snapshot.status.as_ref(), "running" | "paused"),
        can_cancel: matches!(
            snapshot.status.as_ref(),
            "running" | "paused" | "cancelling"
        ),
        can_remove: matches!(
            snapshot.status.as_ref(),
            "completed" | "cancelled" | "error"
        ),
    }
}

fn hash_optional_text(hasher: &mut RenderFingerprint, text: Option<&Arc<str>>) {
    match text {
        Some(text) => {
            true.hash(hasher);
            text.hash(hasher);
        }
        None => false.hash(hasher),
    }
}

fn hash_task_card_model(hasher: &mut RenderFingerprint, model: &TaskCardViewModel) {
    model.id.hash(hasher);
    model.title.hash(hasher);
    hash_optional_text(hasher, model.detail.as_ref());
    model.stage.hash(hasher);
    model.status.hash(hasher);
    model.worker_total.hash(hasher);
    model.worker_active.hash(hasher);
    model.percent_text.hash(hasher);
    model.amount_text.hash(hasher);
    hash_optional_text(hasher, model.speed_text.as_ref());
    hash_optional_text(hasher, model.eta_text.as_ref());
    hash_optional_text(hasher, model.message.as_ref());
    model.percent_basis_points.hash(hasher);
    model.started_at_unix.hash(hasher);
    model.last_update_unix.hash(hasher);
    model.can_pause.hash(hasher);
    model.can_cancel.hash(hasher);
    model.can_remove.hash(hasher);
}

fn task_render_entry_sort_key(entry: &TaskRenderEntry) -> (u64, &str) {
    (entry.started_at_unix, entry.model.id.as_ref())
}

fn insert_sorted_limited_entries<'a>(
    entries: &mut Vec<&'a TaskRenderEntry>,
    entry: &'a TaskRenderEntry,
    limit: usize,
) {
    if limit == 0 {
        return;
    }

    let insert_at = entries.partition_point(|existing| {
        task_render_entry_sort_key(existing) <= task_render_entry_sort_key(entry)
    });

    if entries.len() < limit {
        entries.insert(insert_at, entry);
        return;
    }

    if insert_at >= limit {
        return;
    }

    entries.insert(insert_at, entry);
    entries.truncate(limit);
}

fn compute_render_model_signature(
    loading: bool,
    active_total: usize,
    finished_total: usize,
    thread_total: usize,
    active: &[TaskCardViewModel],
    finished: &[TaskCardViewModel],
) -> u64 {
    let mut hasher = RenderFingerprint::new();
    loading.hash(&mut hasher);
    active_total.hash(&mut hasher);
    finished_total.hash(&mut hasher);
    thread_total.hash(&mut hasher);
    active.len().hash(&mut hasher);
    finished.len().hash(&mut hasher);
    for item in active {
        hash_task_card_model(&mut hasher, item);
    }
    for item in finished {
        hash_task_card_model(&mut hasher, item);
    }
    hasher.value()
}

pub(super) fn build_render_model() -> TasksPageRenderModel {
    let snapshot_lists = task_manager::render_snapshots_limited(
        MAX_RENDER_ACTIVE_TASKS,
        MAX_FINISHED_TASKS_IN_LIST,
        MAX_RENDER_FINISHED_TASKS,
    );

    let active_total = snapshot_lists.active_total;
    let finished_total = snapshot_lists.finished_total;
    let thread_total = snapshot_lists
        .active
        .iter()
        .filter_map(|snapshot| snapshot.visualization.as_ref())
        .map(|visualization| visualization.worker_total.unwrap_or(0) as usize)
        .sum();
    let active: Vec<_> = snapshot_lists
        .active
        .iter()
        .map(|snapshot| build_task_card_model(snapshot))
        .collect();
    let finished: Vec<_> = snapshot_lists
        .finished
        .iter()
        .map(|snapshot| build_task_card_model(snapshot))
        .collect();
    let signature = compute_render_model_signature(
        false,
        active_total,
        finished_total,
        thread_total,
        &active,
        &finished,
    );

    TasksPageRenderModel {
        loading: false,
        signature,
        total_count: active_total + finished_total,
        active_total,
        finished_total,
        thread_total,
        active,
        finished,
    }
}

fn is_terminal_task_status(status: &str) -> bool {
    matches!(status, "completed" | "cancelled" | "error")
}

fn terminal_hold_duration_ms(status: &str, user_cancelled: bool) -> Option<u64> {
    match status {
        "completed" => Some(TASK_FINISHED_HOLD_COMPLETED_MS),
        "cancelled" => Some(if user_cancelled {
            TASK_FINISHED_HOLD_CANCELLED_BY_USER_MS
        } else {
            TASK_FINISHED_HOLD_TERMINAL_DEFAULT_MS
        }),
        "error" => Some(TASK_FINISHED_HOLD_TERMINAL_DEFAULT_MS),
        _ => None,
    }
}

fn hold_deadline(now: Instant, status: &str, user_cancelled: bool) -> Option<Instant> {
    terminal_hold_duration_ms(status, user_cancelled)
        .map(|duration_ms| now + Duration::from_millis(duration_ms))
}

fn hold_deadline_from_update(
    now: Instant,
    now_unix_seconds: u64,
    last_update_unix: u64,
    status: &str,
    user_cancelled: bool,
) -> Option<Instant> {
    let duration_ms = terminal_hold_duration_ms(status, user_cancelled)?;
    let elapsed_ms = now_unix_seconds
        .saturating_sub(last_update_unix)
        .saturating_mul(1_000);
    let remaining_ms = duration_ms.saturating_sub(elapsed_ms);
    Some(now + Duration::from_millis(remaining_ms))
}

fn should_hide_finished(deadline: Option<Instant>, now: Instant) -> bool {
    deadline.is_some_and(|deadline| now >= deadline)
}

fn terminal_hold_elapsed(
    now_unix_seconds: u64,
    last_update_unix: u64,
    status: &str,
    user_cancelled: bool,
) -> bool {
    let Some(duration_ms) = terminal_hold_duration_ms(status, user_cancelled) else {
        return false;
    };
    now_unix_seconds
        .saturating_sub(last_update_unix)
        .saturating_mul(1_000)
        >= duration_ms
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn prune_terminal_visibility_state(
    visible_models: &HashMap<Arc<str>, TaskCardViewModel>,
    finished_hold_until: &mut HashMap<Arc<str>, Instant>,
    hidden_finished_ids: &mut HashSet<Arc<str>>,
    user_cancelled_ids: &mut HashSet<Arc<str>>,
    pending_exit_motions: &mut HashMap<Arc<str>, TaskCardMotionKind>,
) {
    finished_hold_until.retain(|task_id, _| {
        visible_models
            .get(task_id)
            .is_some_and(|model| is_terminal_task_status(model.status.as_ref()))
    });
    hidden_finished_ids.retain(|task_id| {
        visible_models
            .get(task_id)
            .is_some_and(|model| is_terminal_task_status(model.status.as_ref()))
    });
    user_cancelled_ids.retain(|task_id| {
        visible_models
            .get(task_id)
            .is_some_and(|model| model.status.as_ref() == "cancelled")
    });
    pending_exit_motions.retain(|task_id, _| {
        visible_models
            .get(task_id)
            .is_some_and(|model| is_terminal_task_status(model.status.as_ref()))
    });
}

impl TasksPageView {
    pub(super) fn apply_render_model(
        &mut self,
        next_render_model: TasksPageRenderModel,
        cx: &mut Context<Self>,
    ) {
        let next_render_model = self.prepare_render_model(next_render_model);
        if self.render_model.signature == next_render_model.signature {
            return;
        }

        let previous_models = visible_task_models(&self.render_model);
        let next_models = visible_task_models(&next_render_model);
        let next_visible_ids: HashSet<_> = next_models.keys().cloned().collect();

        for (task_id, next_model) in &next_models {
            if let Some(previous_model) = previous_models.get(task_id.as_ref()) {
                if previous_model.status.as_ref() != next_model.status.as_ref()
                    && is_terminal_task_status(next_model.status.as_ref())
                {
                    self.schedule_terminal_visibility(task_id.clone(), next_model, cx);
                }
            } else if is_terminal_task_status(next_model.status.as_ref()) {
                self.schedule_terminal_visibility(task_id.clone(), next_model, cx);
            } else {
                self.schedule_card_motion(
                    task_id.clone(),
                    TaskCardMotionKind::Enter,
                    TASK_CARD_ENTER_ANIMATION_MS,
                    cx,
                );
            }
        }

        for (task_id, previous_model) in &previous_models {
            if next_visible_ids.contains(task_id.as_ref()) {
                continue;
            }

            if matches!(previous_model.status.as_ref(), "cancelled" | "cancelling")
                || self.pending_exit_motions.contains_key(task_id.as_ref())
            {
                let motion = self
                    .pending_exit_motions
                    .remove(task_id.as_ref())
                    .unwrap_or(TaskCardMotionKind::Exit);
                let transition_sequence = self.next_motion_sequence();
                self.transition_cards.insert(
                    task_id.clone(),
                    TaskTransitionCard {
                        model: previous_model.clone(),
                        motion,
                        sequence: transition_sequence,
                    },
                );
                self.schedule_transition_cleanup(
                    task_id.clone(),
                    transition_sequence,
                    TASK_CARD_EXIT_ANIMATION_MS,
                    cx,
                );
            }
        }

        self.render_model = next_render_model;
        self.notify_task_layout(cx);
    }

    pub(super) fn notify_task_layout(&self, cx: &mut Context<Self>) {
        if DISABLE_TASKS_PAGE_LAYOUT_NOTIFY {
            return;
        }

        if self.active {
            cx.notify();
        }
    }

    pub(super) fn task_motion_kind(&self, task_id: &str) -> Option<TaskCardMotionKind> {
        self.card_motions.get(task_id).map(|state| state.kind)
    }

    pub(super) fn transition_cards(&self) -> Vec<TaskTransitionCard> {
        let mut cards: Vec<_> = self.transition_cards.values().cloned().collect();
        cards.sort_by(|left, right| {
            left.model
                .started_at_unix
                .cmp(&right.model.started_at_unix)
                .then_with(|| left.model.id.as_ref().cmp(right.model.id.as_ref()))
        });
        cards
    }

    pub(super) fn mark_user_cancelled(&mut self, task_id: Arc<str>) {
        self.user_cancelled_ids.insert(task_id);
    }

    fn schedule_card_motion(
        &mut self,
        task_id: Arc<str>,
        kind: TaskCardMotionKind,
        duration_ms: u64,
        cx: &mut Context<Self>,
    ) {
        let sequence = self.next_motion_sequence();
        self.card_motions
            .insert(task_id.clone(), TaskCardMotionState { kind, sequence });
        let task_id_for_cleanup = task_id.clone();
        cx.spawn(async move |handle, cx| -> anyhow::Result<()> {
            Timer::after(Duration::from_millis(duration_ms)).await;
            handle.update(cx, move |this, cx| {
                if this
                    .card_motions
                    .get(task_id_for_cleanup.as_ref())
                    .is_some_and(|state| state.sequence == sequence)
                {
                    this.card_motions.remove(task_id_for_cleanup.as_ref());
                    this.notify_task_layout(cx);
                }
            })?;
            Ok(())
        })
        .detach_and_log_err(cx);
    }

    pub(super) fn schedule_exit_motion(
        &mut self,
        task_id: Arc<str>,
        _model: TaskCardViewModel,
        _cx: &mut Context<Self>,
    ) {
        self.pending_exit_motions
            .insert(task_id.clone(), TaskCardMotionKind::Exit);
    }

    fn schedule_transition_cleanup(
        &mut self,
        task_id: Arc<str>,
        sequence: u64,
        duration_ms: u64,
        cx: &mut Context<Self>,
    ) {
        let task_id_for_cleanup = task_id.clone();
        cx.spawn(async move |handle, cx| -> anyhow::Result<()> {
            Timer::after(Duration::from_millis(duration_ms)).await;
            handle.update(cx, move |this, cx| {
                if this
                    .transition_cards
                    .get(task_id_for_cleanup.as_ref())
                    .is_some_and(|entry| entry.sequence == sequence)
                {
                    this.transition_cards.remove(task_id_for_cleanup.as_ref());
                    this.pending_exit_motions
                        .remove(task_id_for_cleanup.as_ref());
                    this.notify_task_layout(cx);
                }
            })?;
            Ok(())
        })
        .detach_and_log_err(cx);
    }

    fn prepare_render_model(
        &mut self,
        mut render_model: TasksPageRenderModel,
    ) -> TasksPageRenderModel {
        let raw_models = visible_task_models(&render_model);
        prune_terminal_visibility_state(
            &raw_models,
            &mut self.finished_hold_until,
            &mut self.hidden_finished_ids,
            &mut self.user_cancelled_ids,
            &mut self.pending_exit_motions,
        );

        let now_unix_seconds = current_unix_seconds();
        for model in &render_model.finished {
            if !is_terminal_task_status(model.status.as_ref())
                || self.finished_hold_until.contains_key(model.id.as_ref())
                || self.hidden_finished_ids.contains(model.id.as_ref())
            {
                continue;
            }

            let user_cancelled = self.user_cancelled_ids.contains(model.id.as_ref());
            if terminal_hold_elapsed(
                now_unix_seconds,
                model.last_update_unix,
                model.status.as_ref(),
                user_cancelled,
            ) {
                self.hidden_finished_ids.insert(model.id.clone());
                self.user_cancelled_ids.remove(model.id.as_ref());
                self.pending_exit_motions.remove(model.id.as_ref());
            }
        }

        render_model.finished.retain(|model| {
            !self.hidden_finished_ids.contains(model.id.as_ref())
                || !is_terminal_task_status(model.status.as_ref())
        });
        render_model.finished_total = render_model.finished.len();
        render_model.total_count = render_model.active_total + render_model.finished_total;
        render_model.signature = compute_render_model_signature(
            render_model.loading,
            render_model.active_total,
            render_model.finished_total,
            render_model.thread_total,
            &render_model.active,
            &render_model.finished,
        );
        render_model
    }

    fn schedule_terminal_visibility(
        &mut self,
        task_id: Arc<str>,
        model: &TaskCardViewModel,
        cx: &mut Context<Self>,
    ) {
        if self.finished_hold_until.contains_key(task_id.as_ref())
            || self.hidden_finished_ids.contains(task_id.as_ref())
        {
            return;
        }

        let user_cancelled = self.user_cancelled_ids.contains(task_id.as_ref());
        let now = Instant::now();
        let Some(deadline) = hold_deadline_from_update(
            now,
            current_unix_seconds(),
            model.last_update_unix,
            model.status.as_ref(),
            user_cancelled,
        ) else {
            return;
        };

        self.finished_hold_until.insert(task_id.clone(), deadline);
        if deadline <= now {
            self.schedule_finished_hide(task_id, deadline, cx);
            return;
        }

        let (motion_kind, duration_ms) = if model.status.as_ref() == "completed" {
            (
                TaskCardMotionKind::Complete,
                TASK_CARD_COMPLETE_ANIMATION_MS,
            )
        } else {
            (TaskCardMotionKind::Warn, TASK_CARD_WARNING_ANIMATION_MS)
        };
        self.schedule_card_motion(task_id.clone(), motion_kind, duration_ms, cx);
        self.schedule_finished_hide(task_id, deadline, cx);
    }

    fn schedule_finished_hide(
        &mut self,
        task_id: Arc<str>,
        deadline: Instant,
        cx: &mut Context<Self>,
    ) {
        let wait_duration = deadline.saturating_duration_since(Instant::now());
        let task_id_for_hide = task_id.clone();
        cx.spawn(async move |handle, cx| -> anyhow::Result<()> {
            Timer::after(wait_duration).await;
            handle.update(cx, move |this, cx| {
                if !should_hide_finished(
                    this.finished_hold_until
                        .get(task_id_for_hide.as_ref())
                        .copied(),
                    Instant::now(),
                ) {
                    return;
                }

                this.finished_hold_until.remove(task_id_for_hide.as_ref());
                this.hidden_finished_ids.insert(task_id_for_hide.clone());
                this.user_cancelled_ids.remove(task_id_for_hide.as_ref());
                this.pending_exit_motions
                    .insert(task_id_for_hide.clone(), TaskCardMotionKind::Exit);
                this.apply_render_model(build_render_model(), cx);
            })?;
            Ok(())
        })
        .detach_and_log_err(cx);
    }

    fn next_motion_sequence(&mut self) -> u64 {
        self.motion_sequence = self.motion_sequence.saturating_add(1);
        self.motion_sequence
    }
}

impl Drop for TasksPageView {
    fn drop(&mut self) {
        let _ = self.update_apply_task.take();
    }
}

fn visible_task_models(
    render_model: &TasksPageRenderModel,
) -> HashMap<Arc<str>, TaskCardViewModel> {
    let mut models =
        HashMap::with_capacity(render_model.active.len() + render_model.finished.len());
    for model in render_model
        .active
        .iter()
        .chain(render_model.finished.iter())
    {
        models.insert(model.id.clone(), model.clone());
    }
    models
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_model(task_id: &str, status: &str) -> TaskCardViewModel {
        TaskCardViewModel {
            id: Arc::from(task_id),
            title: Arc::from("Task"),
            detail: None,
            stage: Arc::from("下载中"),
            status: Arc::from(status),
            worker_total: None,
            worker_active: None,
            percent_text: Arc::from("50%"),
            amount_text: Arc::from("1 / 2 MB"),
            speed_text: None,
            eta_text: None,
            message: None,
            percent_basis_points: Some(5_000),
            started_at_unix: 1,
            last_update_unix: 1,
            can_pause: false,
            can_cancel: false,
            can_remove: true,
        }
    }

    #[test]
    fn terminal_task_remains_visible_before_deadline() {
        let now = Instant::now();
        let deadline = hold_deadline(now, "completed", false).unwrap();

        assert!(!should_hide_finished(
            Some(deadline),
            now + Duration::from_millis(1_199)
        ));
    }

    #[test]
    fn completed_task_hides_after_completed_hold_window() {
        let now = Instant::now();
        let deadline = hold_deadline(now, "completed", false).unwrap();

        assert!(should_hide_finished(
            Some(deadline),
            now + Duration::from_millis(1_200)
        ));
    }

    #[test]
    fn user_cancelled_task_uses_shorter_hold_window() {
        let now = Instant::now();
        let deadline = hold_deadline(now, "cancelled", true).unwrap();

        assert!(!should_hide_finished(
            Some(deadline),
            now + Duration::from_millis(399)
        ));
        assert!(should_hide_finished(
            Some(deadline),
            now + Duration::from_millis(400)
        ));
    }

    #[test]
    fn stale_terminal_task_deadline_has_no_remaining_hold_time() {
        let now = Instant::now();
        let deadline = hold_deadline_from_update(now, 12, 10, "completed", false).unwrap();

        assert_eq!(deadline.saturating_duration_since(now), Duration::ZERO);
    }

    #[test]
    fn terminal_hold_elapsed_uses_snapshot_update_time() {
        assert!(!terminal_hold_elapsed(11, 10, "completed", false));
        assert!(terminal_hold_elapsed(12, 10, "completed", false));
    }

    #[test]
    fn stale_hidden_state_is_cleared_when_task_returns_active() {
        let task_id: Arc<str> = Arc::from("task-1");
        let mut finished_hold_until = HashMap::from([(task_id.clone(), Instant::now())]);
        let mut hidden_finished_ids = HashSet::from([task_id.clone()]);
        let mut user_cancelled_ids = HashSet::from([task_id.clone()]);
        let mut pending_exit_motions = HashMap::from([(task_id.clone(), TaskCardMotionKind::Exit)]);
        let visible_models = HashMap::from([(task_id.clone(), test_model("task-1", "running"))]);

        prune_terminal_visibility_state(
            &visible_models,
            &mut finished_hold_until,
            &mut hidden_finished_ids,
            &mut user_cancelled_ids,
            &mut pending_exit_motions,
        );

        assert!(finished_hold_until.is_empty());
        assert!(hidden_finished_ids.is_empty());
        assert!(user_cancelled_ids.is_empty());
        assert!(pending_exit_motions.is_empty());
    }
}
