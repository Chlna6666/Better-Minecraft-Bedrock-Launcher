use super::*;

pub(super) fn notify_view<T>(view: &Entity<T>, cx: &mut App)
where
    T: 'static,
{
    view.update(cx, |_view, cx| cx.notify());
}

pub(super) fn notify_weak_view_async<T>(
    view: &WeakEntity<T>,
    cx: &mut AsyncApp,
) -> anyhow::Result<()>
where
    T: 'static,
{
    view.update(cx, |_view, cx| cx.notify())?;
    Ok(())
}

pub(super) fn get_or_create_page_view<T, F>(
    view: &mut Option<Entity<T>>,
    cx: &mut Context<MainWindowView>,
    create: F,
) -> Entity<T>
where
    T: 'static,
    F: FnOnce(&mut Context<MainWindowView>) -> Entity<T>,
{
    if view.is_none() {
        *view = Some(create(cx));
    }
    view.as_ref().expect("page view must exist").clone()
}

pub(super) fn clear_optional_page_view<T>(view: &mut Option<Entity<T>>) {
    *view = None;
}
