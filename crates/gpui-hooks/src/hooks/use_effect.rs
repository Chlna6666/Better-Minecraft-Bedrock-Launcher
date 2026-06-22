use crate::hooks::{Dependencies, Dependency, HasHooks, dependencies_changed, to_dependencies};

struct EffectHook {
    deps: Dependencies,
    cleanup: Option<Box<dyn FnOnce()>>,
}

impl Drop for EffectHook {
    fn drop(&mut self) {
        if let Some(cleanup) = self.cleanup.take() {
            cleanup();
        }
    }
}

pub trait UseEffectHook: HasHooks {
    fn use_effect<D>(&self, effect: impl FnOnce() -> Option<Box<dyn FnOnce()>>, deps: D)
    where
        D: IntoIterator,
        D::Item: Dependency + Clone + 'static,
    {
        let deps = to_dependencies(deps);
        let hook_index = self._next_hook_index();
        let mut hooks = self._hooks_storage().borrow_mut();

        if hook_index == hooks.len() {
            hooks.push(Box::new(EffectHook {
                deps: deps.clone(),
                cleanup: effect(),
            }));
            return;
        }

        let hook = hooks
            .get_mut(hook_index)
            .unwrap_or_else(|| panic!("gpui_hooks: missing hook slot at index {}", hook_index));
        let Some(hook) = hook.downcast_mut::<EffectHook>() else {
            panic!(
                "gpui_hooks: hook type mismatch at index {} for {}",
                hook_index,
                std::any::type_name::<EffectHook>()
            );
        };

        if dependencies_changed(&hook.deps, &deps) {
            if let Some(cleanup) = hook.cleanup.take() {
                cleanup();
            }

            hook.deps = deps;
            hook.cleanup = effect();
        }
    }

    fn use_effect_once(&self, effect: impl FnOnce() -> Option<Box<dyn FnOnce()>>) {
        self.use_effect(effect, std::iter::empty::<()>());
    }
}

impl<T> UseEffectHook for T where T: HasHooks {}
