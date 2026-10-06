#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct FrameReadyRequestState {
    next_generation: u64,
    registration: Option<u64>,
    unsupported: bool,
}

impl FrameReadyRequestState {
    pub(super) fn begin_registration(&mut self) -> Option<u64> {
        if self.registration.is_some() || self.unsupported {
            return None;
        }

        self.next_generation = self.next_generation.wrapping_add(1);
        let generation = self.next_generation;
        self.registration = Some(generation);
        Some(generation)
    }

    pub(super) fn reject_registration(&mut self, generation: u64) {
        if self.registration == Some(generation) {
            self.registration = None;
        }
    }

    pub(super) fn mark_unsupported(&mut self, generation: u64) {
        if self.registration == Some(generation) {
            self.registration = None;
            self.unsupported = true;
        }
    }

    pub(super) fn consume_if_current(&mut self, generation: u64) -> bool {
        if self.registration != Some(generation) {
            return false;
        }

        self.registration = None;
        true
    }

    pub(super) fn invalidate_scene(&mut self) {
        self.registration = None;
    }

    pub(super) fn invalidate_swapchain(&mut self) {
        self.invalidate_scene();
        self.unsupported = false;
    }
}

#[cfg(test)]
mod tests {
    use super::FrameReadyRequestState;

    #[test]
    fn coalesces_registration_and_old_callbacks_cannot_consume_or_reject_new_ones() {
        let mut state = FrameReadyRequestState::default();
        let old_generation = state.begin_registration().unwrap();
        assert_eq!(state.begin_registration(), None);
        assert!(state.consume_if_current(old_generation));

        let current_generation = state.begin_registration().unwrap();
        assert_ne!(old_generation, current_generation);
        assert!(!state.consume_if_current(old_generation));
        state.reject_registration(old_generation);
        assert_eq!(state.begin_registration(), None);
        assert!(state.consume_if_current(current_generation));
        assert!(state.begin_registration().is_some());
    }

    #[test]
    fn unsupported_backend_is_retried_after_swapchain_invalidation() {
        let mut state = FrameReadyRequestState::default();
        let generation = state.begin_registration().unwrap();
        state.mark_unsupported(generation);
        assert_eq!(state.begin_registration(), None);

        state.invalidate_scene();
        assert_eq!(state.begin_registration(), None);

        state.invalidate_swapchain();
        assert!(state.begin_registration().is_some());
    }

    #[test]
    fn transient_registration_error_can_be_retried() {
        let mut state = FrameReadyRequestState::default();
        let generation = state.begin_registration().unwrap();
        state.reject_registration(generation);

        assert!(state.begin_registration().is_some());
    }
}
