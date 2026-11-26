use std::cell::RefCell;
use std::rc::Rc;

use crate::hooks::{HasHooks, RefHandle};

struct RefHook<T> {
    value: Rc<RefCell<T>>,
}

impl<T> RefHook<T> {
    fn new(value: T) -> Self {
        Self {
            value: Rc::new(RefCell::new(value)),
        }
    }
}

pub trait UseRefHook: HasHooks {
    fn use_ref<T>(&self, init: impl FnOnce() -> T) -> RefHandle<T>
    where
        T: 'static,
    {
        let value = self._use_hook(
            || RefHook::new(init()),
            |hook: &mut RefHook<T>| hook.value.clone(),
        );

        RefHandle { value }
    }
}

impl<T> UseRefHook for T where T: HasHooks {}
