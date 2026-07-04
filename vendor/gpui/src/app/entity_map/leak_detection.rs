use collections::HashMap;

use super::EntityId;

static LEAK_BACKTRACE: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| std::env::var("LEAK_BACKTRACE").is_ok_and(|b| !b.is_empty()));

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
            LEAK_BACKTRACE.then(backtrace::Backtrace::new_unresolved),
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
                    eprintln!("Leaked handle: export LEAK_BACKTRACE to find allocation site");
                }
            }
            panic!();
        }
    }
}
