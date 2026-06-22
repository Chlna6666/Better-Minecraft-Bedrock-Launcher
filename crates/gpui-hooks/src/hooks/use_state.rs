use std::cell::RefCell;
use std::rc::Rc;

use crate::hooks::{HasHooks, StateHandle};

struct StateHook<T> {
    value: Rc<RefCell<Rc<T>>>,
}

impl<T> StateHook<T> {
    fn new(value: T) -> Self {
        Self {
            value: Rc::new(RefCell::new(Rc::new(value))),
        }
    }
}

pub trait UseStateHook: HasHooks {
    fn use_state<T>(&self, init: impl FnOnce() -> T) -> StateHandle<T>
    where
        T: 'static,
    {
        let state = self._use_hook(
            || StateHook::new(init()),
            |hook: &mut StateHook<T>| hook.value.clone(),
        );

        StateHandle { value: state }
    }
}

impl<T> UseStateHook for T where T: HasHooks {}
