use super::*;

#[test]
fn cancelling_a_later_callback_prevents_it_from_running_in_the_same_dispatch() {
    let subscribers = SubscriberSet::new();
    let first = subscribers.insert(1usize, 10);
    let mut second = Some(subscribers.insert(1, 20));
    let mut visited = Vec::new();
    subscribers.retain(&1, |callback| {
        visited.push(*callback);
        drop(second.take());
        true
    });
    assert_eq!(visited, [10]);
    assert_eq!(subscribers.remove(&1).into_iter().collect::<Vec<_>>(), [10]);
    drop(first);
}

#[test]
fn nested_emitters_can_cancel_the_outer_subscription() {
    let subscribers = SubscriberSet::new();
    let mut outer = Some(subscribers.insert(1usize, 10));
    let inner = subscribers.insert(2, 20);
    let mut visited = Vec::new();
    subscribers.retain(&1, |callback| {
        visited.push(*callback);
        subscribers.retain(&2, |callback| {
            visited.push(*callback);
            drop(outer.take());
            true
        });
        true
    });
    assert_eq!(visited, [10, 20]);
    assert_eq!(subscribers.remove(&1).into_iter().count(), 0);
    drop(inner);
}

#[test]
fn test_immediate_and_pending_insertions() {
    let subscribers = SubscriberSet::new();
    let first = subscribers.insert(1usize, 10);
    let (second, activate_second) = subscribers.insert_pending(1, 20);
    let mut visited = Vec::new();
    subscribers.retain(&1, |callback| {
        visited.push(*callback);
        subscribers.insert(1, 30).detach();
        true
    });
    assert_eq!(visited, [10]);
    activate_second();
    assert_eq!(
        subscribers.remove(&1).into_iter().collect::<Vec<_>>(),
        [10, 20, 30]
    );
    drop(first);
    drop(second);

    let (pending, _activate) = subscribers.insert_pending(1, 40);
    assert_eq!(subscribers.remove(&1).into_iter().count(), 0);
    drop(pending);
}

#[test]
fn test_single_immediate_and_pending_insertions() {
    let subscribers = SingleSubscriberSet::new();
    let first = subscribers.insert(10);
    let (second, activate_second) = subscribers.insert_pending(20);
    let mut visited = Vec::new();
    subscribers.retain(|callback| {
        visited.push(*callback);
        subscribers.insert(30).detach();
        true
    });
    assert_eq!(visited, [10]);
    activate_second();
    assert_eq!(
        subscribers.remove().into_iter().collect::<Vec<_>>(),
        [10, 20, 30]
    );
    drop(first);
    drop(second);

    let (pending, _activate) = subscribers.insert_pending(40);
    assert_eq!(subscribers.remove().into_iter().count(), 0);
    drop(pending);
}

#[test]
fn test_activation_after_drop_during_retain() {
    let subscribers = SubscriberSet::new();
    let (first, activate_first) = subscribers.insert_pending(1usize, 10);
    let (second, activate_second) = subscribers.insert_pending(1, 20);
    subscribers.retain(&1, |_| panic!("pending subscriber invoked"));
    activate_first();
    let mut second = Some((second, activate_second));
    let mut visited = Vec::new();

    subscribers.retain(&1, |callback| {
        visited.push(*callback);
        let (subscription, activate) = second.take().expect("only the first callback runs");
        drop(subscription);
        activate();
        true
    });

    assert_eq!(visited, [10]);
    assert_eq!(subscribers.remove(&1).into_iter().collect::<Vec<_>>(), [10]);
    drop(first);
}

#[test]
fn test_single_activation_after_drop_during_retain() {
    let subscribers = SingleSubscriberSet::new();
    let (first, activate_first) = subscribers.insert_pending(10);
    let (second, activate_second) = subscribers.insert_pending(20);
    subscribers.retain(|_| panic!("pending subscriber invoked"));
    activate_first();
    let mut second = Some((second, activate_second));
    let mut visited = Vec::new();

    subscribers.retain(|callback| {
        visited.push(*callback);
        let (subscription, activate) = second.take().expect("only the first callback runs");
        drop(subscription);
        activate();
        true
    });

    assert_eq!(visited, [10]);
    assert_eq!(subscribers.remove().into_iter().collect::<Vec<_>>(), [10]);
    drop(first);
}
