//! Independent CPU-side render preparation scheduler.
//!
//! GPUI owns the native window and the thread-affine gfx device. The sidecar
//! instead owns a bounded, latest-wins queue for the expensive Scene traversal
//! and immutable draw preparation. It never touches ExtensionDevice, pipeline
//! handles, textures, GPU fences, or native command encoders. At publication
//! the renderer consumes a completed snapshot without blocking on the worker.
//! Final offscreen GPU submission continues through the shared gfx-core
//! extension pass, which owns the required cross-backend synchronization.
use super::{Mat4, Scene, SceneView, SceneViewId};
use crate::{Camera, PreparedScene};
use rayon::ThreadPool;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, OnceLock},
};

const MAX_RETAINED_VIEWS: usize = 32;
const MAX_SIDECAR_THREADS: usize = 6;

#[derive(Default)]
struct Mailbox {
    inner: Mutex<MailboxState>,
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
            let mut prepared = PreparedScene::default();
            // Aspect ratio is finalized from the actual render target only
            // after the snapshot is consumed. The later update_camera call
            // refreshes frustum/alpha ordering for the exact viewport.
            let completed = prepared
                .update_transformed(&request.scene, request.camera, 1.0, request.transform)
                .is_ok()
                .then_some(prepared);
            let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if state.revision == request.revision {
                if let Some(prepared) = completed {
                    state.published = Some(Completed {
                        scene: request.scene, transform: request.transform,
                        camera: request.camera, prepared,
                    });
                }
            }
        }
    }
}

#[derive(Default)]
struct SidecarQueue {
    slots: HashMap<SceneViewId, Arc<Mailbox>>,
    order: VecDeque<SceneViewId>,
}
impl SidecarQueue {
    fn mailbox(&mut self, id: SceneViewId) -> Arc<Mailbox> {
        if let Some(existing) = self.slots.get(&id) {
            return existing.clone();
        }
        while self.slots.len() >= MAX_RETAINED_VIEWS {
            if let Some(oldest) = self.order.pop_front() {
                self.slots.remove(&oldest);
            }
        }
        let slot = Arc::new(Mailbox::default());
        self.slots.insert(id, slot.clone());
        self.order.push_back(id);
        slot
    }
    fn forget(&mut self, id: SceneViewId) {
        self.slots.remove(&id);
        self.order.retain(|entry| *entry != id);
    }
}

static SIDECAR: OnceLock<Mutex<SidecarQueue>> = OnceLock::new();
static WORKERS: OnceLock<Option<ThreadPool>> = OnceLock::new();

fn scheduler() -> &'static Mutex<SidecarQueue> {
    SIDECAR.get_or_init(|| Mutex::new(SidecarQueue::default()))
}
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
    let slot = {
        let mut queue = scheduler().lock().unwrap_or_else(|e| e.into_inner());
        queue.mailbox(view.id)
    };
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

fn current_mailbox(view: &SceneView) -> Option<Arc<Mailbox>> {
    let queue = scheduler().lock().unwrap_or_else(|e| e.into_inner());
    queue.slots.get(&view.id).cloned()
}

/// True while an immutable CPU job is not ready. Full scene replacement may
/// retain the last GPU publication instead of blocking the device owner.
pub(super) fn pending(view: &SceneView) -> bool {
    current_mailbox(view).is_some_and(|slot| {
        slot.status(&view.scene, view.scene_transform).0
    })
}

/// Keep the GPUI offscreen extension ticking until the sidecar producer has
/// delivered and the GPU owner has consumed its final completed publication.
pub(super) fn needs_frame(view: &SceneView) -> bool {
    current_mailbox(view).is_some_and(|slot| {
        slot.status(&view.scene, view.scene_transform).1
    })
}

/// GPU owner used its exact existing synchronous fast path. Cancel redundant
/// queued work; an already executing traversal self-discards by revision.
pub(super) fn discard(view: &SceneView) {
    if let Some(slot) = current_mailbox(view) {
        slot.discard(&view.scene, view.scene_transform);
    }
}

/// Never waits. GPU owner takes only a matching finished snapshot and then
/// applies actual device capabilities, projection and current camera.
/// A cache miss keeps the existing synchronous preparation fallback.
pub(super) fn try_take(
    view: &SceneView,
    aspect: f32,
    gpu_culling: bool,
) -> anyhow::Result<Option<PreparedScene>> {
    let Some(mut prepared) = current_mailbox(view).and_then(|slot| slot.take(
        &view.scene, view.scene_transform, view.camera,
    )) else {
        return Ok(None);
    };
    prepared.set_gpu_culling(gpu_culling);
    prepared.set_culling_bounds(view.culling_bounds);
    prepared.update_camera(view.camera, aspect)?;
    Ok(Some(prepared))
}

/// Evict CPU-side preparation when the GPU owner retires a scene. Already
/// running work is reference-counted and finishes without resurrecting it.
pub(super) fn forget(id: SceneViewId) {
    if let Some(queue) = SIDECAR.get() {
        queue.lock().unwrap_or_else(|e| e.into_inner()).forget(id);
    }
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
    fn cache_eviction_keeps_the_number_of_live_mailboxes_bounded() {
        let mut cache = SidecarQueue::default();
        for n in 0..MAX_RETAINED_VIEWS + 8 {
            cache.mailbox(SceneViewId(n as u64 + 1));
        }
        assert_eq!(cache.slots.len(), MAX_RETAINED_VIEWS);
        assert!(!cache.slots.contains_key(&SceneViewId(1)));
        cache.forget(SceneViewId((MAX_RETAINED_VIEWS + 8) as u64));
        assert_eq!(cache.slots.len(), MAX_RETAINED_VIEWS - 1);
    }
}
