use std::cell::{Ref, RefCell, RefMut};
use std::ops::Deref;
use std::rc::Rc;

pub(crate) type SharedState<T> = Rc<RefCell<Rc<T>>>;

#[derive(Clone)]
pub struct StateHandle<T> {
    pub(crate) value: SharedState<T>,
}

impl<T> StateHandle<T> {
    #[must_use]
    pub fn snapshot(&self) -> Rc<T> {
        self.value.borrow().clone()
    }

    pub fn with<R>(&self, read: impl FnOnce(&T) -> R) -> R {
        let value = self.value.borrow();
        read(value.as_ref())
    }

    pub fn set(&self, value: T) {
        *self.value.borrow_mut() = Rc::new(value);
    }

    pub fn update(&self, update: impl FnOnce(&T) -> T) {
        let next = {
            let value = self.value.borrow();
            update(value.as_ref())
        };
        *self.value.borrow_mut() = Rc::new(next);
    }
}

impl<T> StateHandle<T>
where
    T: Clone,
{
    #[must_use]
    pub fn get_cloned(&self) -> T {
        self.with(Clone::clone)
    }
}

#[derive(Clone)]
pub struct MemoHandle<T> {
    pub(crate) value: Rc<T>,
}

impl<T> MemoHandle<T> {
    #[must_use]
    pub fn get(&self) -> &T {
        self.value.as_ref()
    }
}

impl<T> Deref for MemoHandle<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.get()
    }
}

#[derive(Clone)]
pub struct RefHandle<T> {
    pub(crate) value: Rc<RefCell<T>>,
}

impl<T> RefHandle<T> {
    #[must_use]
    pub fn borrow(&self) -> Ref<'_, T> {
        self.value.borrow()
    }

    #[must_use]
    pub fn borrow_mut(&self) -> RefMut<'_, T> {
        self.value.borrow_mut()
    }

    pub fn with<R>(&self, read: impl FnOnce(&T) -> R) -> R {
        let value = self.value.borrow();
        read(&value)
    }

    pub fn with_mut<R>(&self, write: impl FnOnce(&mut T) -> R) -> R {
        let mut value = self.value.borrow_mut();
        write(&mut value)
    }
}

#[derive(Clone)]
pub struct CallbackHandle<R> {
    pub(crate) callback: Rc<dyn Fn() -> R>,
}

impl<R> CallbackHandle<R> {
    pub fn invoke(&self) -> R {
        (self.callback)()
    }
}

impl<R> Deref for CallbackHandle<R> {
    type Target = dyn Fn() -> R;

    fn deref(&self) -> &Self::Target {
        self.callback.as_ref()
    }
}

type Reducer<T, A> = dyn Fn(&T, A) -> T;

#[derive(Clone)]
pub struct ReducerHandle<T, A> {
    pub(crate) state: SharedState<T>,
    pub(crate) reducer: Rc<Reducer<T, A>>,
}

impl<T, A> ReducerHandle<T, A> {
    #[must_use]
    pub fn snapshot(&self) -> Rc<T> {
        self.state.borrow().clone()
    }

    pub fn with<R>(&self, read: impl FnOnce(&T) -> R) -> R {
        let value = self.state.borrow();
        read(value.as_ref())
    }

    pub fn dispatch(&self, action: A) {
        let next = {
            let state = self.state.borrow();
            (self.reducer)(state.as_ref(), action)
        };
        *self.state.borrow_mut() = Rc::new(next);
    }
}

impl<T, A> ReducerHandle<T, A>
where
    T: Clone,
{
    #[must_use]
    pub fn get_cloned(&self) -> T {
        self.with(Clone::clone)
    }
}
