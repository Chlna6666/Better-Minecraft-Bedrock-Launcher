use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::{HasHooks, UseEffectHook, UseMemoHook, UseReducerHook, UseStateHook};

struct TestElement {
    hooks: RefCell<Vec<Box<dyn Any>>>,
    hook_index: Cell<usize>,
    hook_count: Cell<usize>,
}

impl Default for TestElement {
    fn default() -> Self {
        Self {
            hooks: RefCell::new(Vec::new()),
            hook_index: Cell::new(0),
            hook_count: Cell::new(0),
        }
    }
}

impl HasHooks for TestElement {
    fn _hooks_storage(&self) -> &RefCell<Vec<Box<dyn Any>>> {
        &self.hooks
    }

    fn _hook_index_cell(&self) -> &Cell<usize> {
        &self.hook_index
    }

    fn _hook_count_cell(&self) -> &Cell<usize> {
        &self.hook_count
    }
}

#[test]
fn use_state_preserves_value_between_renders() {
    let element = TestElement::default();

    element._begin_hooks();
    let count = element.use_state(|| 1_u32);
    element._finish_hooks();

    assert_eq!(count.get_cloned(), 1);
    count.set(3);

    element._begin_hooks();
    let count = element.use_state(|| 99_u32);
    element._finish_hooks();

    assert_eq!(count.get_cloned(), 3);
}

#[test]
fn use_memo_updates_only_when_dependencies_change() {
    let element = TestElement::default();
    let compute_count = Rc::new(Cell::new(0));

    element._begin_hooks();
    let memo = element.use_memo(
        {
            let compute_count = compute_count.clone();
            move || {
                compute_count.set(compute_count.get() + 1);
                7_u32
            }
        },
        [1_u32],
    );
    element._finish_hooks();
    assert_eq!(*memo, 7);
    assert_eq!(compute_count.get(), 1);

    element._begin_hooks();
    let memo = element.use_memo(
        {
            let compute_count = compute_count.clone();
            move || {
                compute_count.set(compute_count.get() + 1);
                9_u32
            }
        },
        [1_u32],
    );
    element._finish_hooks();
    assert_eq!(*memo, 7);
    assert_eq!(compute_count.get(), 1);

    element._begin_hooks();
    let memo = element.use_memo(
        {
            let compute_count = compute_count.clone();
            move || {
                compute_count.set(compute_count.get() + 1);
                9_u32
            }
        },
        [2_u32],
    );
    element._finish_hooks();
    assert_eq!(*memo, 9);
    assert_eq!(compute_count.get(), 2);
}

#[test]
fn use_effect_runs_cleanup_on_dependency_change_and_cleanup_hooks() {
    let element = TestElement::default();
    let cleanup_count = Rc::new(Cell::new(0));

    element._begin_hooks();
    element.use_effect(
        {
            let cleanup_count = cleanup_count.clone();
            move || {
                Some(Box::new(move || {
                    cleanup_count.set(cleanup_count.get() + 1);
                }) as Box<dyn FnOnce()>)
            }
        },
        [1_u32],
    );
    element._finish_hooks();
    assert_eq!(cleanup_count.get(), 0);

    element._begin_hooks();
    element.use_effect(
        {
            let cleanup_count = cleanup_count.clone();
            move || {
                Some(Box::new(move || {
                    cleanup_count.set(cleanup_count.get() + 1);
                }) as Box<dyn FnOnce()>)
            }
        },
        [2_u32],
    );
    element._finish_hooks();
    assert_eq!(cleanup_count.get(), 1);

    element.cleanup_hooks();
    assert_eq!(cleanup_count.get(), 2);
}

#[test]
fn use_reducer_dispatch_updates_state() {
    let element = TestElement::default();

    element._begin_hooks();
    let count = element.use_reducer(|state: &u32, action: u32| state + action, || 0);
    element._finish_hooks();

    count.dispatch(2);
    count.dispatch(5);

    element._begin_hooks();
    let count = element.use_reducer(|state: &u32, action: u32| state + action, || 99);
    element._finish_hooks();

    assert_eq!(count.get_cloned(), 7);
}
