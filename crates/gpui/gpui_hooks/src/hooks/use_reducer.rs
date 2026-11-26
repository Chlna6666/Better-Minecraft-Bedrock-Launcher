use std::cell::RefCell;
use std::rc::Rc;

use crate::hooks::{HasHooks, ReducerHandle, SharedState};

struct ReducerHook<T, A> {
    state: SharedState<T>,
    reducer: Rc<dyn Fn(&T, A) -> T>,
}

pub trait UseReducerHook: HasHooks {
    fn use_reducer<T, A>(
        &self,
        reducer: impl Fn(&T, A) -> T + 'static,
        init: impl FnOnce() -> T,
    ) -> ReducerHandle<T, A>
    where
        T: 'static,
        A: 'static,
    {
        let (state, reducer) = self._use_hook(
            || ReducerHook {
                state: Rc::new(RefCell::new(Rc::new(init()))),
                reducer: Rc::new(reducer),
            },
            |hook: &mut ReducerHook<T, A>| (hook.state.clone(), hook.reducer.clone()),
        );

        ReducerHandle { state, reducer }
    }
}

impl<T> UseReducerHook for T where T: HasHooks {}
