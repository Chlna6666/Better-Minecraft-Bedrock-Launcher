//! Independent CPU-side render preparation scheduler.
//!
//! GPUI owns the native window and the thread-affine gfx device. The sidecar
//! instead owns a bounded, latest-wins queue for the expensive Scene traversal
//! and immutable draw preparation. It never touches ExtensionDevice, pipeline
//! handles, textures, GPU fences, or native command encoders. At publication
//! the renderer consumes a completed snapshot without blocking on the worker.
//! Final offscreen GPU submission continues through the shared gfx-core
//! extension pass, which owns the required cross-backend synchronization.
use super::{Mat4, Scene, SceneView};
use crate::{Camera, PreparedScene};
use rayon::ThreadPool;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};

const MAX_SIDECAR_THREADS: usize = 6;

#[derive(Default)]
pub(super) struct Mailbox {
    inner: Mutex<MailboxState>,
    /// Zero-contention cancellation token read by CPU RenderJobs. The
    /// mutable mailbox remains under its short critical-section mutex.
    generation: AtomicU64,
}
impl std::fmt::Debug for Mailbox {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ScenePreparationMailbox")
    }
}
#[derive(Default)]
struct MailboxState {
    revision: u64,
    target: Option<Target>,
    pending: Option<Request>,
    published: Option<Completed>,
    running: bool,
}
struct Target {
    scene: Arc<Scene>,
    transform: Mat4,
}
struct Request {
    revision: u64,
    scene: Arc<Scene>,
    transform: Mat4,
    camera: Camera,
}
struct Completed {
    scene: Arc<Scene>,
    transform: Mat4,
    camera: Camera,
    prepared: PreparedScene,
}
impl Mailbox {
    fn enqueue(&self, scene: Arc<Scene>, transform: Mat4, camera: Camera) -> bool {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if state.target.as_ref().is_some_and(|target| {
            Arc::ptr_eq(&target.scene, &scene) && target.transform == transform
        }) {
            return false;
        }
        state.revision = state.revision.wrapping_add(1);
        let revision = state.revision;
        self.generation.store(revision, Ordering::Release);
        state.target = Some(Target { scene: scene.clone(), transform });
        state.pending = Some(Request { revision, scene, transform, camera });
        state.published = None;
        if state.running {
            return false;
        }
        state.running = true;
        true
    }

    fn status(&self, scene: &Arc<Scene>, transform: Mat4) -> (bool, bool) {
        let state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if !state.target.as_ref().is_some_and(|target| {
            Arc::ptr_eq(&target.scene, scene) && target.transform == transform
        }) {
            return (false, false);
        }
        let waiting = state.pending.is_some() || state.running;
        (waiting && state.published.is_none(),
         waiting || state.published.is_some())
    }

    fn discard(&self, scene: &Arc<Scene>, transform: Mat4) {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if !state.target.as_ref().is_some_and(|target| {
            Arc::ptr_eq(&target.scene, scene) && target.transform == transform
        }) {
            return;
        }
        // Invalidate the running job without waiting for the worker.
        state.revision = state.revision.wrapping_add(1);
        self.generation.store(state.revision, Ordering::Release);
        state.pending = None;
        state.published = None;
    }

    fn take(&self, scene: &Arc<Scene>, transform: Mat4, camera: Camera) -> Option<PreparedScene> {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let ready = state.published.as_ref()?;
        if !Arc::ptr_eq(&ready.scene, scene)
            || ready.transform != transform
            || (ready.prepared.uses_lod() && ready.camera != camera)
        {
            return None;
        }
        state.published.take().map(|ready| ready.prepared)
    }

    fn run(self: &Arc<Self>) {
        loop {
            let request = {
                let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                match state.pending.take() {
                    Some(request) => request,
                    None => {
                        state.running = false;
                        break;
                    }
                }
            };
            let prepared_started = std::time::Instant::now();
            let mut prepared = PreparedScene::default();
            // Aspect ratio is finalized from the actual render target only
            // after the snapshot is consumed. The later update_camera call
            // refreshes frustum/alpha ordering for the exact viewport.
            let completed = prepared
                .update_transformed_interruptible(
                    &request.scene,
                    request.camera,
                    1.0,
                    request.transform,
                    || self.generation.load(Ordering::Acquire) != request.revision,
                )
                .ok()
                .filter(|&finished| finished)
                .map(|_| prepared);
            let elapsed = prepared_started.elapsed();
            let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let current = state.revision == request.revision;
            let draw_count = completed.as_ref().map_or(0, |prepared| prepared.draws.len());
            if current {
                if let Some(prepared) = completed {
                    state.published = Some(Completed {
                        scene: request.scene, transform: request.transform,
                        camera: request.camera, prepared,
                    });
                }
            }
            let next_pending = state.pending.is_some();
            drop(state);
            if std::env::var_os("GPUI_3D_TRACE_TIMINGS").is_some() {
                eprintln!(
                    "gpui3d sidecar_cpu_prepared revision={} current={} draws={} elapsed_ms={:.2} next_pending={}",
                    request.revision,
                    current,
                    draw_count,
                    elapsed.as_secs_f64() * 1000.0,
                    next_pending,
                );
            }
        }
    }
}

// A SceneView owns its own latest-wins mailbox and releases it at drop.
// No global per-frame map lock, stale IDs, or cache eviction of active views.
static WORKERS: OnceLock<Option<ThreadPool>> = OnceLock::new();

fn pool() -> Option<&'static ThreadPool> {
    WORKERS.get_or_init(|| {
        let threads = std::thread::available_parallelism()
            .map(usize::from).unwrap_or(2).div_ceil(2).clamp(1, MAX_SIDECAR_THREADS);
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|index| format!("gpui-3d-sidecar-{index}"))
            .build()
            .ok()
    }).as_ref()
}

/// Stage an immutable Scene for background traversal. No UI or GPU context
/// is held while the work runs; a newer revision replaces queued old work.
pub(super) fn schedule(view: &SceneView) {
    if std::env::var_os("GPUI_3D_DISABLE_SIDECAR").is_some()
        // The thread handoff is only worthwhile for nontrivial scenes;
        // small UI widgets are cheaper to prepare directly on GPU owner.
        || view.scene.len() < 64
        || !view.animation.is_empty()
    {
        return;
    }
    let slot = Arc::clone(&view.sidecar);
    if slot.enqueue(view.scene.clone(), view.scene_transform, view.camera) {
        if let Some(pool) = pool() {
            pool.spawn(move || slot.run());
        } else {
            // A dedicated worker-pool initialization failure must not break
            // rendering. Rayon global workers remain an async fallback.
            rayon::spawn(move || slot.run());
        }
    }
}

/// True while an immutable CPU job is not ready. Full scene replacement may
/// retain the last GPU publication instead of blocking the device owner.
pub(super) fn pending(view: &SceneView) -> bool {
    view.sidecar.status(&view.scene, view.scene_transform).0
}

/// Keep the GPUI offscreen extension ticking until the sidecar producer has
/// delivered and the GPU owner has consumed its final completed publication.
pub(super) fn needs_frame(view: &SceneView) -> bool {
    view.sidecar.status(&view.scene, view.scene_transform).1
}

/// GPU owner used its exact existing synchronous fast path. Cancel redundant
/// queued work; an already executing traversal self-discards by revision.
pub(super) fn discard(view: &SceneView) {
    view.sidecar.discard(&view.scene, view.scene_transform);
}

/// Never waits. GPU owner takes only a matching finished snapshot and then
/// applies actual device capabilities, projection and current camera.
/// A cache miss keeps the existing synchronous preparation fallback.
pub(super) fn try_take(
    view: &SceneView,
    aspect: f32,
    gpu_culling: bool,
) -> anyhow::Result<Option<PreparedScene>> {
    let Some(mut prepared) = view.sidecar.take(
        &view.scene, view.scene_transform, view.camera,
    ) else {
        return Ok(None);
    };
    prepared.set_gpu_culling(gpu_culling);
    prepared.set_culling_bounds(view.culling_bounds);
    prepared.update_camera(view.camera, aspect)?;
    Ok(Some(prepared))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Vec3;

    fn camera() -> Camera {
        Camera::orthographic(Vec3::ZERO, -Vec3::Z, Vec3::Y, 4.0, 0.1, 100.0)
    }

    #[test]
    fn queue_is_latest_wins_and_discards_superseded_layouts() {
        let slot = Arc::new(Mailbox::default());
        let first = Arc::new(Scene::new());
        let second = Arc::new(Scene::new());
        assert!(slot.enqueue(first.clone(), Mat4::IDENTITY, camera()));
        assert!(!slot.enqueue(second.clone(), Mat4::IDENTITY, camera()));
        slot.run();
        assert!(slot.take(&first, Mat4::IDENTITY, camera()).is_none());
        assert!(slot.take(&second, Mat4::IDENTITY, camera()).is_some());
        assert!(slot.take(&second, Mat4::IDENTITY, camera()).is_none());
    }

    #[test]
    fn gpu_owner_fallback_cancels_a_superseded_cpu_snapshot() {
        let slot = Arc::new(Mailbox::default());
        let scene = Arc::new(Scene::new());
        assert!(slot.enqueue(scene.clone(), Mat4::IDENTITY, camera()));
        assert_eq!(slot.status(&scene, Mat4::IDENTITY), (true, true));
        slot.discard(&scene, Mat4::IDENTITY);
        slot.run();
        assert_eq!(slot.status(&scene, Mat4::IDENTITY), (false, false));
        assert!(slot.take(&scene, Mat4::IDENTITY, camera()).is_none());
    }

    #[test]
    fn mailbox_cancellation_token_invalidates_old_work() {
        let slot = Mailbox::default();
        let old = Arc::new(Scene::new());
        let next = Arc::new(Scene::new());
        assert!(slot.enqueue(old, Mat4::IDENTITY, camera()));
        let first = slot.generation.load(Ordering::Acquire);
        assert!(!slot.enqueue(next.clone(), Mat4::IDENTITY, camera()));
        assert_ne!(slot.generation.load(Ordering::Acquire), first);
        assert!(slot.generation.load(Ordering::Acquire) > first);
        slot.discard(&next, Mat4::IDENTITY);
        assert_ne!(slot.generation.load(Ordering::Acquire), first + 1);
    }

    #[test]
    fn repeated_immutable_scene_does_not_enqueue_more_work() {
        let slot = Arc::new(Mailbox::default());
        let scene = Arc::new(Scene::new());
        assert!(slot.enqueue(scene.clone(), Mat4::IDENTITY, camera()));
        assert!(!slot.enqueue(scene.clone(), Mat4::IDENTITY, camera()));
        slot.run();
        assert!(!slot.enqueue(scene.clone(), Mat4::IDENTITY, camera()));
        assert!(slot.take(&scene, Mat4::IDENTITY, camera()).is_some());
    }

    #[test]
    fn preparation_mailboxes_have_independent_scene_lifetimes() {
        let scene = Arc::new(Scene::new());
        let left = Arc::new(Mailbox::default());
        let right = Arc::new(Mailbox::default());
        assert!(left.enqueue(scene.clone(), Mat4::IDENTITY, camera()));
        assert!(right.enqueue(scene.clone(), Mat4::IDENTITY, camera()));
        left.run();
        assert!(left.take(&scene, Mat4::IDENTITY, camera()).is_some());
        assert_eq!(right.status(&scene, Mat4::IDENTITY), (true, true));
        right.run();
        assert!(right.take(&scene, Mat4::IDENTITY, camera()).is_some());
        assert_eq!(Arc::strong_count(&left), 1);
     }
}
