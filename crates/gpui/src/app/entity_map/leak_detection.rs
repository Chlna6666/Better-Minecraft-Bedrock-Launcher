use collections::HashMap;

use super::EntityId;

#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq)]
pub(crate) struct HandleId {
    id: u64, // id of the handle itself, not the pointed at object
}

pub(crate) struct LeakDetector {
    pub(crate) next_handle_id: u64,
    pub(crate) entity_handles: HashMap<EntityId, HashMap<HandleId, Option<backtrace::Backtrace>>>,
}

impl LeakDetector {
    #[track_caller]
    pub fn handle_created(&mut self, entity_id: EntityId) -> HandleId {
        let id = util::post_inc(&mut self.next_handle_id);
        let handle_id = HandleId { id };
        let handles = self.entity_handles.entry(entity_id).or_default();
        handles.insert(
            handle_id,
            log::log_enabled!(target: "gpui::entity_leaks", log::Level::Trace)
                .then(backtrace::Backtrace::new_unresolved),
        );
        handle_id
    }

    pub fn handle_released(&mut self, entity_id: EntityId, handle_id: HandleId) {
        let handles = self.entity_handles.entry(entity_id).or_default();
        handles.remove(&handle_id);
    }

    pub fn assert_released(&mut self, entity_id: EntityId) {
        let handles = self.entity_handles.entry(entity_id).or_default();
        if !handles.is_empty() {
            for backtrace in handles.values_mut() {
                if let Some(mut backtrace) = backtrace.take() {
                    backtrace.resolve();
                    eprintln!("Leaked handle: {:#?}", backtrace);
                } else {
                    eprintln!("Leaked handle: enable gpui::entity_leaks trace logging to find allocation site");
                }
            }
            panic!();
        }
    }
}
