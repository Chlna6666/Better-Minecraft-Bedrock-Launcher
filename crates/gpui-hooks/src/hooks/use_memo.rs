use std::rc::Rc;

use crate::hooks::{
    Dependencies, Dependency, HasHooks, MemoHandle, dependencies_changed, to_dependencies,
};

struct MemoHook<T> {
    deps: Dependencies,
    value: Rc<T>,
}

pub trait UseMemoHook: HasHooks {
    fn use_memo<T, D>(&self, compute: impl FnOnce() -> T, deps: D) -> MemoHandle<T>
    where
        T: 'static,
        D: IntoIterator,
        D::Item: Dependency + Clone + 'static,
    {
        let deps = to_dependencies(deps);
        let hook_index = self._next_hook_index();
        let mut hooks = self._hooks_storage().borrow_mut();

        if hook_index == hooks.len() {
            let memo = Rc::new(compute());
            hooks.push(Box::new(MemoHook {
                deps,
                value: memo.clone(),
            }));
            return MemoHandle { value: memo };
        }

        let hook = hooks
            .get_mut(hook_index)
            .unwrap_or_else(|| panic!("gpui_hooks: missing hook slot at index {}", hook_index));
        let Some(hook) = hook.downcast_mut::<MemoHook<T>>() else {
            panic!(
                "gpui_hooks: hook type mismatch at index {} for {}",
                hook_index,
                std::any::type_name::<MemoHook<T>>()
            );
        };

        if dependencies_changed(&hook.deps, &deps) {
            hook.deps = deps;
            hook.value = Rc::new(compute());
        }

        MemoHandle {
            value: hook.value.clone(),
        }
    }

    fn use_memo_once<T>(&self, compute: impl FnOnce() -> T) -> MemoHandle<T>
    where
        T: 'static,
    {
        self.use_memo(compute, std::iter::empty::<()>())
    }
}

impl<T> UseMemoHook for T where T: HasHooks {}
