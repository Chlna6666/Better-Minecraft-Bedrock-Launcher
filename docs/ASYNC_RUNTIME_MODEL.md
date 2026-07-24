# Async Runtime And GPUI State Model

This document is the source of truth for background execution, durable
workflows, task state, and GPUI state propagation in BMCBL. Read it before
changing `src/tasks`, `src/downloads`, `src/archive`, long-running work in
`src/core`, or any GPUI code that starts or observes background work.

## Design Goal

BMCBL uses one runtime ownership boundary, not one physical thread pool:

```text
Domain service
  -> AppRuntime semantic API
  -> IO / download / archive / CPU execution domain
  -> pure DomainEvent or immutable Snapshot
  -> GPUI foreground consumer
  -> Global or Entity update
  -> cx.notify()
  -> render from entity-local state
```

The model separates two concerns:

1. The active BMCBL `AppRuntime` owns current execution, concurrency budgets,
   and task submission.
2. Domain event bridges own the transition from background data to GPUI state.

New application hosts use `egpui::ApplicationRuntime` and its
`RuntimeProvider`. The BMCBL runtime remains the owner of existing
download/archive/task-manager workflows until the explicit migration task is
completed; these two layers must not be mixed in one workflow.

New egpui applications should use `egpui::UiTaskBridge` for high-frequency
progress. It uses latest-wins snapshots for progress and a separate terminal
channel for completed, cancelled, or failed outcomes. This prevents a large
download or extraction from filling an event queue and starving its terminal
state.

The backend-to-GPUI contract is deliberately split by delivery semantics:

| Data | egpui API | Delivery rule |
| --- | --- | --- |
| Short ordered mutation | `UiHandle` | bounded queue, producer observes backpressure |
| Ordered event stream | `UiStreamBridge<T>` | bounded channel, every queued event is applied |
| Progress/status snapshot | `UiSnapshotBridge<T>` or `UiTaskBridge` | latest value wins, intermediate values may coalesce |
| Completion/cancellation/failure | `UiTaskBridge` terminal | dedicated capacity, terminal is selected before progress |

`UiStreamBridge::bind_stream` is a foreground consumer. Its stream must only
poll a non-blocking, channel-backed adapter; all filesystem, network, decode,
archive, and CPU production belongs to `TaskScope` or the domain runtime.

Changing Tokio worker counts cannot repair a missing event, a dropped state
transition, or an incorrectly classified terminal task.

## Runtime Ownership

`src/tasks/runtime.rs` currently owns the process-wide BMCBL `AppRuntime`. It is
initialized once during startup and owns these execution domains:

| Work | API | Execution domain |
| --- | --- | --- |
| Async network, timers, processes, orchestration | `spawn_io` | General Tokio IO runtime |
| Blocking filesystem or platform calls | `run_io_blocking` | General blocking pool |
| Download workflow | `spawn_download_task` | Download Tokio runtime plus download permits |
| Blocking download writer | `spawn_download_blocking` | Download blocking pool |
| Archive or install workflow | `spawn_archive_task` | Archive Tokio runtime plus archive permits |
| Blocking archive extraction | `run_archive_blocking` | Archive blocking pool |
| Owned CPU work | `run_cpu` | Application Rayon pool |
| Nested Rayon parallelism in synchronous code | `install_cpu` | Application Rayon pool |
| Entity mutation and rendering | `cx.spawn`, `Entity::update` | GPUI foreground executor |

Business modules select the semantic work type. They do not select or construct
the physical executor.

### Forbidden Runtime Patterns

Production business and UI code must not:

- construct `tokio::runtime::Runtime` or `tokio::runtime::Builder`;
- call `Handle::try_current()` or depend on an implicit Tokio context;
- call `tokio::task::spawn_blocking` from a GPUI task;
- construct a Rayon `ThreadPool`;
- use `std::thread::spawn` as a general fallback;
- add another global runtime, executor, or semaphore outside `AppRuntime`;
- ignore a runtime submission error.

A dedicated OS thread is allowed only for a documented platform lifecycle that
cannot use an executor, such as a Windows input hook, a process-exit watchdog,
or a blocking foreign callback loop. The thread name, shutdown behavior, and
ownership must be explicit.

## Task State Contract

`src/tasks/task_manager.rs` is the authority for user-visible task state,
progress, cancellation, errors, and event publication.

Only these statuses are terminal:

```text
completed
cancelled
error
```

Every other status is active, including unknown future statuses:

```text
ready
queued
initializing
starting
running
paused
cancelling
```

Consumers must use `TaskSnapshot::is_terminal()` or
`task_manager::is_terminal_status()`. Do not infer completion from a list of
known active statuses. A newly added active status must never cause a workflow
to continue as if its child task had completed.

Use `wait_for_task_terminal(task_id)` for workflow dependencies. It subscribes
before reading the current snapshot, handles broadcast lag with snapshot
recovery, and returns only an explicit terminal snapshot. Do not replace it
with a timer loop.

Task collection views use `subscribe_task_events()` and handle both
`TaskEvent::Updated` and `TaskEvent::Removed`. A snapshot-only subscriber may
use `subscribe_task_updates()` when removal is irrelevant to its workflow.
Removing a task without publishing `Removed` leaves collection caches stale and
is prohibited.

Terminal errors must carry a useful message. A workflow that starts a child
task must validate both:

1. the child reached `completed`;
2. the expected output, such as a file path, is present.

## Durable Workflow Contract

A workflow that must continue after a page closes does not belong to
`cx.spawn`. It belongs in a domain module and runs through `AppRuntime`.

The game installation workflow in `src/core/minecraft/install.rs` is the
reference:

```text
prepare
  -> download or reuse package
  -> wait for explicit download terminal state
  -> start AppX extraction or GDK unpack
  -> wait for explicit install terminal state
  -> publish local-version invalidation
  -> publish Completed or Failed snapshot
```

Workflow producers publish pure data. They must not capture or retain:

- `App`, `AsyncApp`, or `Context<T>`;
- `Window` or `AsyncWindowContext`;
- `Entity<T>` or a page-specific state type;
- render elements or callbacks into a view.

Dropping a page must not cancel an installation unless cancellation is an
explicit product action.

## Background-To-GPUI Bridge

Use `watch` for the latest state of one workflow, `broadcast` for independent
event consumers, `mpsc` for one owned consumer, and `oneshot` for one result.
Events and snapshots must be owned, immutable data that is `Send + 'static`.

The domain module converts its channel into a `Stream`. The adapter owns
channel-specific behavior: a lagged broadcast receiver emits one recovery
signal or authoritative snapshot, while a closed channel ends the stream.
Views must not duplicate `recv`, lag recovery, entity-release matching, and
notification loops.

Bind the domain stream through egpui's application-level bridge:

```rust
let (events, event_task) = UiStreamBridge::install(config, cx, |event, cx| {
    apply_event_to_global(event, cx);
});
scope.spawn_io(move || async move {
    while let Some(event) = domain_events.next().await {
        events.send(event).await?;
    }
    Ok::<_, DomainError>(())
});
```

Store the returned GPUI `Task<()>` for view-scoped streams so dropping the
view cancels only the consumer. Application-lifetime bridges may be retained
by the host. The foreground closure only applies the already-owned event and
notifies the affected entity or global; it does not start backend work.

The producer never calls `cx.update_global`, `Entity::update`, or `cx.notify`.
The foreground consumer is the only owner of GPUI state mutation. A registered
global observer must update and notify each affected entity.

For high-frequency progress, producers may publish at a bounded rate and the
foreground consumer may coalesce non-terminal updates. Terminal events must be
delivered immediately.

### Framework And Application Ownership

GPUI owns only generic concurrency integration:

- foreground `spawn` and background executor primitives;
- `App::spawn_stream` and `Context::spawn_stream`;
- entity release, task cancellation, update leases, and invalidation.

BMCBL owns physical runtimes, concurrency budgets, domain channels, snapshots,
lag recovery, retry rules, and workflow semantics for its existing workflows.
`egpui` owns the replaceable generic application provider and lifecycle for new
hosts. Do not add Tokio, download, archive, Minecraft, music, or task-manager
policy to GPUI. Do not add another page-local bridge when a domain stream
already exists.

Invalidation events use at-least-once coalescing. If an invalidation arrives
while a refresh is running, preserve one pending refresh even when the current
refresh was already forced. Multiple pending invalidations may collapse into
one follow-up refresh, but none may be silently discarded.

## Render Contract

Render code is a pure projection of stable GPUI-owned state:

```rust
pub struct DownloadView {
    task_snapshots: HashMap<Arc<str>, Arc<TaskSnapshot>>,
}

fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
    let snapshot = self.task_snapshots.get(self.active_task_id.as_ref());
    render_task(snapshot)
}
```

Render methods must not:

- acquire a cross-thread `Mutex` or `RwLock`;
- read a live task-manager or service snapshot;
- start a network, filesystem, parsing, decoding, or persistence operation;
- create or cancel background work;
- use a timer to discover state changes;
- convert an error into an empty successful state.

Initial snapshot reads and lag recovery are allowed in lifecycle or event
consumer code outside render.

## Cancellation, Shutdown, And Errors

- A task that supports cancellation registers its `AbortHandle` or cooperative
  cancellation hook with `task_manager`.
- Cancellation is a terminal state and must publish a final snapshot.
- Dropping a GPUI `Task` cancels it. Store the task when its work is
  view-scoped; detach only work intentionally independent of the view.
- A timeout does not imply that blocking work stopped. Concurrency permits stay
  owned until the underlying operation exits.
- In egpui's default provider, the blocking permit is moved into the actual
  synchronous closure. Cancelling the outer future cannot release capacity
  while the OS call or extraction step is still running.
- Spawn failure, join failure, panic, channel closure, and missing output are
  distinct failures and must be logged or propagated.
- Pending or deduplication flags are cleared on success, cancellation, spawn
  failure, join failure, and domain failure. A logged error must not leave UI
  commands permanently disabled.
- Never use `let _ =` for a fallible send, spawn, update, or workflow action.
- User-recoverable failures must reach visible UI state or a toast with useful
  context.

## Polling Exception

Polling is permitted only when an external system cannot publish a change
event. It must be documented as a fallback and satisfy all of these:

- successful local commands update GPUI immediately;
- the polling interval and ownership are explicit;
- only one poller exists for the domain;
- overlapping requests are prevented;
- stale results are rejected with a generation or request identifier;
- page teardown cancels a page-owned poller;
- polling errors do not erase the last valid snapshot.

EasyTier peer discovery is currently such a fallback. Start and stop operations
must still update foreground state immediately.

## Review Checklist

Before approving async, task, or GPUI state code:

- [ ] The work uses the correct `AppRuntime` semantic API.
- [ ] No new runtime, Rayon pool, implicit Tokio-context probe, or generic
      system-thread fallback was added.
- [ ] Durable work is owned outside GPUI.
- [ ] Child workflows wait for explicit terminal status.
- [ ] Cancellation, spawn failure, join failure, and domain failure are
      observable.
- [ ] Producers publish only pure data.
- [ ] Domain channels expose a stream with explicit lag and closure semantics.
- [ ] `App::spawn_stream` or `Context::spawn_stream` owns foreground mutation.
- [ ] Render reads only entity-local or global UI snapshots.
- [ ] Broadcast lag has snapshot recovery.
- [ ] Invalidations received during refresh schedule a follow-up refresh.
- [ ] Terminal events bypass progress coalescing.
- [ ] Polling is justified as an external-system fallback.
- [ ] A regression test covers any changed state transition.

Useful static checks:

```powershell
rg "Handle::try_current|tokio::task::spawn_blocking|Runtime::new" src
rg "ThreadPoolBuilder|rayon::ThreadPool" src
rg "std::thread::spawn|thread::spawn|thread::Builder" src
rg "Timer::after" src/ui
```

Every remaining match must be a facade implementation, test, animation timer,
documented external fallback, or documented OS lifecycle thread.
