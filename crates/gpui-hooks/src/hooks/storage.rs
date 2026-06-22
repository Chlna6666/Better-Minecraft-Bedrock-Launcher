use std::any::Any;
use std::cell::{Cell, RefCell};

pub trait Hook: Any {
    fn as_any(&self) -> &dyn Any
    where
        Self: Sized,
    {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any
    where
        Self: Sized,
    {
        self
    }
}

impl<T> Hook for T where T: Any {}

pub trait HasHooks {
    fn _hooks_storage(&self) -> &RefCell<Vec<Box<dyn Any>>>;
    fn _hook_index_cell(&self) -> &Cell<usize>;
    fn _hook_count_cell(&self) -> &Cell<usize>;

    #[inline]
    fn _begin_hooks(&self) {
        self._hook_index_cell().set(0);
    }

    #[inline]
    fn _next_hook_index(&self) -> usize {
        let hook_index = self._hook_index_cell().get();
        self._hook_index_cell().set(hook_index + 1);
        hook_index
    }

    #[inline]
    fn _finish_hooks(&self) {
        let hook_count = self._hook_index_cell().get();
        let previous_hook_count = self._hook_count_cell().get();

        if previous_hook_count != 0 && previous_hook_count != hook_count {
            panic!(
                "gpui_hooks: hook count changed between renders: previous={}, current={}",
                previous_hook_count, hook_count
            );
        }

        self._hooks_storage().borrow_mut().truncate(hook_count);
        self._hook_count_cell().set(hook_count);
        self._hook_index_cell().set(0);
    }

    #[inline]
    fn cleanup_hooks(&self) {
        self._hooks_storage().borrow_mut().clear();
        self._hook_count_cell().set(0);
        self._hook_index_cell().set(0);
    }

    #[inline]
    fn _use_hook<T, R>(&self, init: impl FnOnce() -> T, use_hook: impl FnOnce(&mut T) -> R) -> R
    where
        T: Any,
    {
        let hook_index = self._next_hook_index();
        let mut hooks = self._hooks_storage().borrow_mut();

        if hook_index == hooks.len() {
            hooks.push(Box::new(init()));
        }

        let hook = hooks
            .get_mut(hook_index)
            .unwrap_or_else(|| panic!("gpui_hooks: missing hook slot at index {}", hook_index));

        let Some(typed_hook) = hook.downcast_mut::<T>() else {
            panic!(
                "gpui_hooks: hook type mismatch at index {} for {}",
                hook_index,
                std::any::type_name::<T>()
            );
        };

        use_hook(typed_hook)
    }
}
