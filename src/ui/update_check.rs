use crate::utils::updater::ReleaseSummary;
use futures::channel::oneshot;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const OWNER: &str = "Chlna6666";
const REPO: &str = "Better-Minecraft-Bedrock-Launcher";

pub(crate) struct UpdateCheckGate {
    active: AtomicBool,
}

impl UpdateCheckGate {
    pub(crate) const fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
        }
    }

    pub(crate) fn try_begin(&self) -> bool {
        self.active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub(crate) fn finish(&self) {
        self.active.store(false, Ordering::Release);
    }

    #[cfg(test)]
    fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct UpdateCheckOutcome {
    pub(crate) available: Option<ReleaseSummary>,
    pub(crate) error: Option<String>,
}

impl UpdateCheckOutcome {
    fn no_update() -> Self {
        Self {
            available: None,
            error: None,
        }
    }

    pub(crate) fn with_error(error: impl Into<String>) -> Self {
        Self {
            available: None,
            error: Some(error.into()),
        }
    }

    fn with_release(release: ReleaseSummary) -> Self {
        Self {
            available: Some(release),
            error: None,
        }
    }
}

pub(crate) fn check_for_updates_blocking() -> UpdateCheckOutcome {
    let started_at = Instant::now();
    tracing::info!(
        thread = ?std::thread::current().id(),
        "update check started"
    );

    let outcome = match crate::utils::updater::check_updates_blocking(
        OWNER.to_string(),
        REPO.to_string(),
        None,
    ) {
        Ok(value) => update_check_outcome_from_value(value),
        Err(error) => UpdateCheckOutcome::with_error(error),
    };

    let elapsed = started_at.elapsed();
    if let Some(error) = outcome.error.as_ref() {
        tracing::warn!(
            elapsed_ms = elapsed.as_millis(),
            error = %error,
            "update check finished with error"
        );
    } else {
        tracing::info!(
            elapsed_ms = elapsed.as_millis(),
            available = outcome.available.is_some(),
            "update check finished"
        );
    }

    outcome
}

pub(crate) fn spawn_update_check_thread(
    reason: &'static str,
) -> oneshot::Receiver<UpdateCheckOutcome> {
    let (sender, receiver) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(sender)));
    let thread_sender = Arc::clone(&sender);
    let builder = std::thread::Builder::new().name(format!("update-check-{reason}"));

    match builder.spawn(move || {
        tracing::info!(
            reason,
            thread = ?std::thread::current().id(),
            "update check worker thread started"
        );
        let outcome = check_for_updates_blocking();
        let sender = thread_sender
            .lock()
            .ok()
            .and_then(|mut sender| sender.take());
        if let Some(sender) = sender {
            if sender.send(outcome).is_err() {
                tracing::warn!(reason, "update check result receiver dropped");
            }
        }
    }) {
        Ok(_) => {}
        Err(error) => {
            let sender = sender.lock().ok().and_then(|mut sender| sender.take());
            if let Some(sender) = sender {
                let outcome =
                    UpdateCheckOutcome::with_error(format!("启动更新检查线程失败: {error}"));
                if sender.send(outcome).is_err() {
                    tracing::warn!(
                        reason,
                        "update check result receiver dropped after spawn failure"
                    );
                }
            }
        }
    }

    receiver
}

pub(crate) fn update_check_outcome_from_value(value: serde_json::Value) -> UpdateCheckOutcome {
    let update_available = value
        .get("update_available")
        .and_then(|flag| flag.as_bool())
        .unwrap_or(false);

    if !update_available {
        return UpdateCheckOutcome::no_update();
    }

    let Some(selected_release) = value.get("selected_release").cloned() else {
        return UpdateCheckOutcome::with_error("检查更新响应缺少 selected_release");
    };

    match serde_json::from_value::<ReleaseSummary>(selected_release) {
        Ok(release) => UpdateCheckOutcome::with_release(release),
        Err(error) => UpdateCheckOutcome::with_error(format!("解析更新版本信息失败: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn release_value() -> Value {
        json!({
            "tag": "v9.9.9",
            "name": "BMCBL 9.9.9",
            "prerelease": false,
            "published_at": "2026-05-18T00:00:00Z",
            "asset_name": "BMCBL.exe",
            "asset_url": "https://example.com/BMCBL.exe",
            "asset_size": 42,
            "body": "release notes"
        })
    }

    #[test]
    fn no_update_has_no_release_or_error() {
        let outcome = update_check_outcome_from_value(json!({
            "update_available": false,
            "selected_release": release_value(),
        }));

        assert!(outcome.available.is_none());
        assert!(outcome.error.is_none());
    }

    #[test]
    fn update_available_returns_selected_release() {
        let outcome = update_check_outcome_from_value(json!({
            "update_available": true,
            "selected_release": release_value(),
        }));

        let Some(release) = outcome.available else {
            panic!("expected selected release");
        };

        assert_eq!(release.tag, "v9.9.9");
        assert_eq!(release.asset_size, Some(42));
        assert!(outcome.error.is_none());
    }

    #[test]
    fn malformed_selected_release_returns_visible_error() {
        let outcome = update_check_outcome_from_value(json!({
            "update_available": true,
            "selected_release": {
                "name": "missing required fields"
            },
        }));

        assert!(outcome.available.is_none());

        let Some(error) = outcome.error else {
            panic!("expected visible parse error");
        };

        assert!(
            error.contains("解析更新版本信息失败"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn update_check_gate_allows_one_inflight_check() {
        let gate = UpdateCheckGate::new();

        assert!(gate.try_begin());
        assert!(!gate.try_begin());
        assert!(gate.is_active());

        gate.finish();

        assert!(!gate.is_active());
        assert!(gate.try_begin());
    }
}
