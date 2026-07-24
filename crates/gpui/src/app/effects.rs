use std::{
    any::{Any, TypeId},
    sync::atomic::Ordering::SeqCst,
};

use util::ResultExt;

use crate::{AppContext, EntityId, WindowId};

use super::{AnyEntity, App};

const MAX_GLOBAL_OBSERVER_NOTIFICATIONS_PER_FLUSH: usize = 64;

impl App {
    pub(crate) fn push_effect(&mut self, effect: Effect) {
        match &effect {
            Effect::Notify { emitter } => {
                if !self.pending_notifications.insert(*emitter) {
                    return;
                }
            }
            Effect::NotifyGlobalObservers { global_type } => {
                if self.notifying_global_observers.contains(global_type) {
                    self.defer_global_notification(*global_type);
                    return;
                }
                if !self.pending_global_notifications.insert(*global_type) {
                    return;
                }
            }
            _ => {}
        };

        self.pending_effects.push_back(effect);
    }

    fn defer_global_notification(&mut self, global_type: TypeId) {
        if !self.pending_global_notifications.insert(global_type) {
            return;
        }

        let Some(app) = self.this.upgrade() else {
            return;
        };
        let foreground_executor = self.foreground_executor.clone();
        foreground_executor
            .spawn(async move {
                let mut app = app.borrow_mut();
                app.update(|app| {
                    app.pending_global_notifications.remove(&global_type);
                    app.push_effect(Effect::NotifyGlobalObservers { global_type });
                });
            })
            .detach();
    }

    /// Called at the end of [`App::update`] to complete any side effects
    /// such as notifying observers, emitting events, etc. Effects can themselves
    /// cause effects, so we continue looping until all effects are processed.
    pub(in crate::app) fn flush_effects(&mut self) {
        loop {
            self.release_dropped_entities();
            self.release_dropped_focus_handles();
            if let Some(effect) = self.pending_effects.pop_front() {
                match effect {
                    Effect::Notify { emitter } => {
                        self.apply_notify_effect(emitter);
                    }

                    Effect::Emit {
                        emitter,
                        event_type,
                        event,
                    } => self.apply_emit_effect(emitter, event_type, event),

                    Effect::RefreshWindows => {
                        self.apply_refresh_effect();
                    }

                    Effect::NotifyGlobalObservers { global_type } => {
                        self.apply_notify_global_observers_effect(global_type);
                    }

                    Effect::Defer { callback } => {
                        self.apply_defer_effect(callback);
                    }
                    Effect::EntityCreated {
                        entity,
                        tid,
                        window,
                    } => {
                        self.apply_entity_created_effect(entity, tid, window);
                    }
                }
            } else {
                self.request_dirty_window_frames();

                #[cfg(any(test, feature = "test-support"))]
                for window in self
                    .windows
                    .values()
                    .filter_map(|window| {
                        let window = window.as_deref()?;
                        window.invalidator.is_dirty().then_some(window.handle)
                    })
                    .collect::<Vec<_>>()
                {
                    self.update_window(window, |_, window, cx| window.draw(cx).clear())
                        .unwrap();
                }

                if self.pending_effects.is_empty() {
                    self.global_notification_counts.clear();
                    break;
                }
            }
        }
    }

    fn request_dirty_window_frames(&mut self) {
        for window in self
            .windows
            .values()
            .filter_map(|window| {
                let window = window.as_deref()?;
                window.invalidator.is_dirty().then_some(window.handle)
            })
            .collect::<Vec<_>>()
        {
            self.update_window(window, |_, window, _| {
                window.schedule_dirty_frame();
            })
            .log_err();
        }
    }

    /// Repeatedly called during `flush_effects` to release any entities whose
    /// reference count has become zero. We invoke any release observers before dropping
    /// each entity.
    fn release_dropped_entities(&mut self) {
        loop {
            let dropped = self.entities.take_dropped();
            if dropped.is_empty() {
                break;
            }

            for (entity_id, mut entity) in dropped {
                self.observers.remove(&entity_id);
                self.event_listeners.remove(&entity_id);
                for release_callback in self.release_listeners.remove(&entity_id) {
                    release_callback(entity.as_mut(), self);
                }
            }
        }
    }

    /// Repeatedly called during `flush_effects` to handle a focused handle being dropped.
    fn release_dropped_focus_handles(&mut self) {
        self.focus_handles
            .clone()
            .write()
            .retain(|handle_id, focus| {
                if focus.ref_count.load(SeqCst) == 0 {
                    for window_handle in self.windows() {
                        window_handle
                            .update(self, |_, window, _| {
                                if window.focus == Some(handle_id) {
                                    window.blur();
                                }
                            })
                            .unwrap();
                    }
                    false
                } else {
                    true
                }
            });
    }

    fn apply_notify_effect(&mut self, emitter: EntityId) {
        self.pending_notifications.remove(&emitter);

        self.observers
            .clone()
            .retain(&emitter, |handler| handler(self));
    }

    fn apply_emit_effect(&mut self, emitter: EntityId, event_type: TypeId, event: Box<dyn Any>) {
        self.event_listeners
            .clone()
            .retain(&emitter, |(stored_type, handler)| {
                if *stored_type == event_type {
                    handler(event.as_ref(), self)
                } else {
                    true
                }
            });
    }

    fn apply_refresh_effect(&mut self) {
        self.pending_refresh_windows = false;
        for window in self.windows.values_mut() {
            if let Some(window) = window.as_deref_mut() {
                window.refresh();
            }
        }
    }

    fn apply_notify_global_observers_effect(&mut self, type_id: TypeId) {
        self.pending_global_notifications.remove(&type_id);
        if self.notifying_global_observers.contains(&type_id) {
            self.defer_global_notification(type_id);
            return;
        }

        let count = self
            .global_notification_counts
            .entry(type_id)
            .and_modify(|count| *count += 1)
            .or_insert(1);
        if *count > MAX_GLOBAL_OBSERVER_NOTIFICATIONS_PER_FLUSH {
            log::warn!(
                "deferred global observer notification for {:?} after {} same-flush iterations",
                type_id,
                *count
            );
            self.defer_global_notification(type_id);
            return;
        }

        self.notifying_global_observers.insert(type_id);
        self.global_observers
            .clone()
            .retain(&type_id, |observer| observer(self));
        self.notifying_global_observers.remove(&type_id);
    }

    fn apply_defer_effect(&mut self, callback: Box<dyn FnOnce(&mut Self) + 'static>) {
        callback(self);
    }

    fn apply_entity_created_effect(
        &mut self,
        entity: AnyEntity,
        tid: TypeId,
        window: Option<WindowId>,
    ) {
        self.new_entity_observers.clone().retain(&tid, |observer| {
            if let Some(id) = window {
                self.update_window_id(id, {
                    let entity = entity.clone();
                    |_, window, cx| (observer)(entity, &mut Some(window), cx)
                })
                .expect("All windows should be off the stack when flushing effects");
            } else {
                (observer)(entity.clone(), &mut None, self)
            }
            true
        });
    }
}

/// These effects are processed at the end of each application update cycle.
pub(crate) enum Effect {
    Notify {
        emitter: EntityId,
    },
    Emit {
        emitter: EntityId,
        event_type: TypeId,
        event: Box<dyn Any>,
    },
    RefreshWindows,
    NotifyGlobalObservers {
        global_type: TypeId,
    },
    Defer {
        callback: Box<dyn FnOnce(&mut App) + 'static>,
    },
    EntityCreated {
        entity: AnyEntity,
        tid: TypeId,
        window: Option<WindowId>,
    },
}

impl std::fmt::Debug for Effect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Effect::Notify { emitter } => write!(f, "Notify({})", emitter),
            Effect::Emit { emitter, .. } => write!(f, "Emit({:?})", emitter),
            Effect::RefreshWindows => write!(f, "RefreshWindows"),
            Effect::NotifyGlobalObservers { global_type } => {
                write!(f, "NotifyGlobalObservers({:?})", global_type)
            }
            Effect::Defer { .. } => write!(f, "Defer(..)"),
            Effect::EntityCreated { entity, .. } => write!(f, "EntityCreated({:?})", entity),
        }
    }
}
