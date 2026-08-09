use crate::sound_effect::SoundEffectController;
use gpui::{App, Global};
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;

pub(crate) struct SoundEffectState {
    controller: Arc<Mutex<SoundEffectController>>,
    operation_gate: Arc<AsyncMutex<()>>,
}

impl Global for SoundEffectState {}

impl Default for SoundEffectState {
    fn default() -> Self {
        Self {
            controller: Arc::new(Mutex::new(SoundEffectController::default())),
            operation_gate: Arc::new(AsyncMutex::new(())),
        }
    }
}

impl SoundEffectState {
    pub(crate) fn play_c4_sequence(&self, cx: &mut App) {
        self.spawn_operation(
            "play C4 easter egg sound",
            cx,
            SoundEffectController::play_c4_sequence,
        );
    }

    pub(crate) fn stop(&self, cx: &mut App) {
        self.spawn_operation("stop C4 easter egg sound", cx, |controller| {
            controller.stop();
            Ok(())
        });
    }

    fn spawn_operation(
        &self,
        operation: &'static str,
        cx: &mut App,
        callback: impl FnOnce(&mut SoundEffectController) -> anyhow::Result<()> + Send + 'static,
    ) {
        let controller = self.controller.clone();
        let operation_gate = self.operation_gate.clone();
        cx.spawn(async move |_cx| {
            let _operation_guard = operation_gate.lock().await;
            match crate::tasks::runtime::run_io_blocking(move || {
                let mut controller = match controller.lock() {
                    Ok(controller) => controller,
                    Err(poisoned) => {
                        tracing::warn!("sound effect: recovering poisoned controller lock");
                        poisoned.into_inner()
                    }
                };
                callback(&mut controller)
            })
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(%error, operation, "sound effect operation failed")
                }
                Err(error) => tracing::warn!(%error, operation, "sound effect worker failed"),
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }
}
