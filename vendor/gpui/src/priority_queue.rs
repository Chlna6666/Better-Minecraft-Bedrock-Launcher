use crate::Priority;
use parking_lot::Mutex;
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

struct PriorityQueues<T> {
    high: VecDeque<T>,
    medium: VecDeque<T>,
    low: VecDeque<T>,
}

impl<T> PriorityQueues<T> {
    fn is_empty(&self) -> bool {
        self.high.is_empty() && self.medium.is_empty() && self.low.is_empty()
    }

    fn len(&self) -> usize {
        self.high.len() + self.medium.len() + self.low.len()
    }
}

struct PriorityQueueState<T> {
    queues: Mutex<PriorityQueues<T>>,
    sender_count: AtomicUsize,
    receiver_count: AtomicUsize,
}

impl<T> PriorityQueueState<T> {
    fn push(queues: &mut PriorityQueues<T>, priority: Priority, item: T) {
        match priority {
            Priority::RealtimeAudio => unreachable!(
                "Realtime audio priority runs on a dedicated thread and is never queued"
            ),
            Priority::High => queues.high.push_back(item),
            Priority::Medium => queues.medium.push_back(item),
            Priority::Low => queues.low.push_back(item),
        }
    }

    fn pop(queues: &mut PriorityQueues<T>) -> Option<(Priority, T)> {
        queues
            .high
            .pop_front()
            .map(|item| (Priority::High, item))
            .or_else(|| {
                queues
                    .medium
                    .pop_front()
                    .map(|item| (Priority::Medium, item))
            })
            .or_else(|| queues.low.pop_front().map(|item| (Priority::Low, item)))
    }
}

/// Error returned when sending to a priority queue fails because all receivers are gone.
#[derive(Debug)]
pub struct PrioritySendError<T>(pub T);

/// Error returned when reading from a priority queue fails because all senders are gone.
#[derive(Debug)]
pub struct PriorityRecvError;

/// A sender for a prioritized task queue.
pub(crate) struct PriorityQueueSender<T> {
    state: Arc<PriorityQueueState<T>>,
}

impl<T> Clone for PriorityQueueSender<T> {
    fn clone(&self) -> Self {
        self.state.sender_count.fetch_add(1, Ordering::AcqRel);
        Self {
            state: self.state.clone(),
        }
    }
}

impl<T> PriorityQueueSender<T> {
    /// Sends an item to the queue with the requested priority.
    pub fn send(&self, priority: Priority, item: T) -> Result<(), PrioritySendError<T>> {
        if self.state.receiver_count.load(Ordering::Acquire) == 0 {
            return Err(PrioritySendError(item));
        }

        {
            let mut queues = self.state.queues.lock();
            PriorityQueueState::push(&mut queues, priority, item);
        }
        Ok(())
    }
}

impl<T> Drop for PriorityQueueSender<T> {
    fn drop(&mut self) {
        self.state.sender_count.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A receiver for a prioritized task queue.
pub(crate) struct PriorityQueueReceiver<T> {
    state: Arc<PriorityQueueState<T>>,
}

impl<T> Clone for PriorityQueueReceiver<T> {
    fn clone(&self) -> Self {
        self.state.receiver_count.fetch_add(1, Ordering::AcqRel);
        Self {
            state: self.state.clone(),
        }
    }
}

impl<T> PriorityQueueReceiver<T> {
    /// Creates a new priority queue sender/receiver pair.
    pub fn new() -> (PriorityQueueSender<T>, Self) {
        let state = Arc::new(PriorityQueueState {
            queues: Mutex::new(PriorityQueues {
                high: VecDeque::new(),
                medium: VecDeque::new(),
                low: VecDeque::new(),
            }),
            sender_count: AtomicUsize::new(1),
            receiver_count: AtomicUsize::new(1),
        });

        (
            PriorityQueueSender {
                state: state.clone(),
            },
            Self { state },
        )
    }

    /// Returns the total number of queued items.
    pub fn len(&self) -> usize {
        self.state.queues.lock().len()
    }

    /// Returns true when there are no queued items.
    pub fn is_empty(&self) -> bool {
        self.state.queues.lock().is_empty()
    }

    /// Attempts to pop the next queued item without blocking.
    pub fn try_pop(&self) -> Result<Option<(Priority, T)>, PriorityRecvError> {
        let mut queues = self.state.queues.lock();
        if queues.is_empty() && self.state.sender_count.load(Ordering::Acquire) == 0 {
            return Err(PriorityRecvError);
        }
        Ok(PriorityQueueState::pop(&mut queues))
    }
}

impl<T> Drop for PriorityQueueReceiver<T> {
    fn drop(&mut self) {
        self.state.receiver_count.fetch_sub(1, Ordering::AcqRel);
    }
}
