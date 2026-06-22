use std::rc::Rc;

use crate::hooks::{
    CallbackHandle, Dependencies, Dependency, HasHooks, dependencies_changed, to_dependencies,
};

struct CallbackHook<R> {
    deps: Dependencies,
    callback: Rc<dyn Fn() -> R>,
}

pub trait UseCallbackHook: HasHooks {
    fn use_callback<R, D>(&self, callback: impl Fn() -> R + 'static, deps: D) -> CallbackHandle<R>
    where
        R: 'static,
        D: IntoIterator,
        D::Item: Dependency + Clone + 'static,
    {
        let deps = to_dependencies(deps);
        let callback: Rc<dyn Fn() -> R> = Rc::new(callback);
        let hook_index = self._next_hook_index();
        let mut hooks = self._hooks_storage().borrow_mut();

        if hook_index == hooks.len() {
            hooks.push(Box::new(CallbackHook {
                deps: deps.clone(),
                callback: callback.clone(),
            }));
        }

        let hook = hooks
            .get_mut(hook_index)
            .unwrap_or_else(|| panic!("gpui_hooks: missing hook slot at index {}", hook_index));
        let Some(hook) = hook.downcast_mut::<CallbackHook<R>>() else {
            panic!(
                "gpui_hooks: hook type mismatch at index {} for {}",
                hook_index,
                std::any::type_name::<CallbackHook<R>>()
            );
        };

        if dependencies_changed(&hook.deps, &deps) {
            hook.deps = deps;
            hook.callback = callback.clone();
        }

        CallbackHandle {
            callback: hook.callback.clone(),
        }
    }

    fn use_callback_once<R>(&self, callback: impl Fn() -> R + 'static) -> CallbackHandle<R>
    where
        R: 'static,
    {
        self.use_callback(callback, std::iter::empty::<()>())
    }
}

impl<T> UseCallbackHook for T where T: HasHooks {}
