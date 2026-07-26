use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use futures::{channel::mpsc, stream};

use crate::{AppContext as _, Task, TestAppContext};

const READY_ITEM_COUNT: usize = 8;

#[gpui::test]
fn app_spawn_stream_yields_between_ready_items(cx: &mut TestAppContext) {
    let applied_count = Rc::new(Cell::new(0));
    let count_observed_by_other_task = Rc::new(Cell::new(None));

    let stream_task = cx.update({
        let applied_count = applied_count.clone();
        move |cx| {
            cx.spawn_stream(stream::iter(0..READY_ITEM_COUNT), move |_, _| {
                applied_count.set(applied_count.get() + 1);
            })
        }
    });
    let other_task = cx.update({
        let applied_count = applied_count.clone();
        let count_observed_by_other_task = count_observed_by_other_task.clone();
        move |cx| {
            cx.spawn(async move |_cx| {
                count_observed_by_other_task.set(Some(applied_count.get()));
            })
        }
    });

    cx.run_until_parked();

    assert_eq!(applied_count.get(), READY_ITEM_COUNT);
    assert!(
        count_observed_by_other_task
            .get()
            .is_some_and(|count| count < READY_ITEM_COUNT)
    );
    drop((stream_task, other_task));
}

#[gpui::test]
fn context_spawn_stream_yields_between_ready_items(cx: &mut TestAppContext) {
    let stream_task = Rc::new(RefCell::new(None::<Task<()>>));
    let applied_count = Rc::new(Cell::new(0));
    let count_observed_by_other_task = Rc::new(Cell::new(None));

    let entity = cx.update({
        let stream_task = stream_task.clone();
        let applied_count = applied_count.clone();
        move |cx| {
            cx.new(|cx| {
                let task = cx.spawn_stream(
                    stream::iter(0..READY_ITEM_COUNT),
                    move |state: &mut Vec<usize>, item, _| {
                        state.push(item);
                        applied_count.set(applied_count.get() + 1);
                    },
                );
                *stream_task.borrow_mut() = Some(task);
                Vec::new()
            })
        }
    });
    let other_task = cx.update({
        let applied_count = applied_count.clone();
        let count_observed_by_other_task = count_observed_by_other_task.clone();
        move |cx| {
            cx.spawn(async move |_cx| {
                count_observed_by_other_task.set(Some(applied_count.get()));
            })
        }
    });

    cx.run_until_parked();

    assert_eq!(
        cx.read(|cx| entity.read(cx).clone()),
        (0..READY_ITEM_COUNT).collect::<Vec<_>>()
    );
    assert!(
        count_observed_by_other_task
            .get()
            .is_some_and(|count| count < READY_ITEM_COUNT)
    );
    drop((stream_task, other_task));
}

#[gpui::test]
fn ready_stream_consumers_make_interleaved_progress(cx: &mut TestAppContext) {
    let applied_items = Rc::new(RefCell::new(Vec::new()));
    let first_task = cx.update({
        let applied_items = applied_items.clone();
        move |cx| {
            cx.spawn_stream(stream::iter(0..READY_ITEM_COUNT), move |item, _| {
                applied_items.borrow_mut().push(('a', item));
            })
        }
    });
    let second_task = cx.update({
        let applied_items = applied_items.clone();
        move |cx| {
            cx.spawn_stream(stream::iter(0..READY_ITEM_COUNT), move |item, _| {
                applied_items.borrow_mut().push(('b', item));
            })
        }
    });

    cx.run_until_parked();

    let applied_items = applied_items.borrow();
    let first_second_stream_item = applied_items
        .iter()
        .position(|(stream, _)| *stream == 'b')
        .expect("second stream should be consumed");
    let last_first_stream_item = applied_items
        .iter()
        .rposition(|(stream, _)| *stream == 'a')
        .expect("first stream should be consumed");
    assert!(first_second_stream_item < last_first_stream_item);
    assert_eq!(
        applied_items
            .iter()
            .filter(|(stream, _)| *stream == 'a')
            .map(|(_, item)| *item)
            .collect::<Vec<_>>(),
        (0..READY_ITEM_COUNT).collect::<Vec<_>>()
    );
    assert_eq!(
        applied_items
            .iter()
            .filter(|(stream, _)| *stream == 'b')
            .map(|(_, item)| *item)
            .collect::<Vec<_>>(),
        (0..READY_ITEM_COUNT).collect::<Vec<_>>()
    );
    drop((first_task, second_task));
}

#[gpui::test]
fn channel_backed_stream_resumes_after_pending(cx: &mut TestAppContext) {
    let (sender, receiver) = mpsc::unbounded();
    let applied_items = Rc::new(RefCell::new(Vec::new()));
    let stream_task = cx.update({
        let applied_items = applied_items.clone();
        move |cx| {
            cx.spawn_stream(receiver, move |item, _| {
                applied_items.borrow_mut().push(item);
            })
        }
    });

    cx.run_until_parked();
    assert!(applied_items.borrow().is_empty());

    sender
        .unbounded_send(3)
        .expect("stream consumer should remain connected");
    sender
        .unbounded_send(5)
        .expect("stream consumer should remain connected");
    cx.run_until_parked();

    assert_eq!(*applied_items.borrow(), vec![3, 5]);
    drop((sender, stream_task));
}
