//! Очередь задач одного потока без цикла событий: сценарии оболочки ждут ответов сервисов и окон
//! (`await`). [`LocalTasks`] опрашивает задачу, когда её разбудили (ответ окна пришёл); готовые
//! сразу ответы (фикстуры) идут без пробуждений. Ею живут тесты и [`crate::env::ManualEnv`]; в
//! приложении задачи идут через цикл событий GPUI.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Wake, Waker};

/// Задача потока интерфейса.
pub type LocalFuture = Pin<Box<dyn Future<Output = ()>>>;

struct Flag(AtomicBool);

impl Wake for Flag {
    fn wake(self: Arc<Self>) {
        self.0.store(true, Ordering::SeqCst);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct Task {
    future: LocalFuture,
    woken: Arc<Flag>,
}

/// Очередь задач одного потока: порядок — порядок постановки (как микрозадачи JS).
#[derive(Default)]
pub struct LocalTasks {
    queue: RefCell<VecDeque<Task>>,
    running: Cell<bool>,
}

impl LocalTasks {
    pub fn new() -> LocalTasks {
        LocalTasks::default()
    }

    /// Поставить задачу; первый опрос — в [`LocalTasks::run_until_stalled`].
    pub fn spawn(&self, future: impl Future<Output = ()> + 'static) {
        self.queue.borrow_mut().push_back(Task {
            future: Box::pin(future),
            woken: Arc::new(Flag(AtomicBool::new(true))),
        });
    }

    /// Сколько задач ещё не закончилось.
    pub fn pending(&self) -> usize {
        self.queue.borrow().len()
    }

    /// Опрашивать разбуженные задачи по очереди, пока будить некого. Задачи, поставленные во время
    /// опроса, идут в конец очереди и опрашиваются в этом же вызове.
    pub fn run_until_stalled(&self) {
        if self.running.replace(true) {
            return;
        }
        // Паника в `poll` не оставляет флаг поднятым: иначе следующие вызовы выходили бы сразу.
        struct Running<'a>(&'a Cell<bool>);
        impl Drop for Running<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        let _running = Running(&self.running);
        loop {
            let mut progressed = false;
            let count = self.queue.borrow().len();
            for _ in 0..count {
                let Some(mut task) = self.queue.borrow_mut().pop_front() else {
                    break;
                };
                if !task.woken.0.swap(false, Ordering::SeqCst) {
                    self.queue.borrow_mut().push_back(task);
                    continue;
                }
                progressed = true;
                let waker = Waker::from(task.woken.clone());
                match task.future.as_mut().poll(&mut Context::from_waker(&waker)) {
                    Poll::Ready(()) => {}
                    Poll::Pending => self.queue.borrow_mut().push_back(task),
                }
            }
            let woken = self
                .queue
                .borrow()
                .iter()
                .any(|task| task.woken.0.load(Ordering::SeqCst));
            if !progressed && !woken {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn tasks_run_in_order_and_wake_on_answers() {
        let tasks = LocalTasks::new();
        let log = Rc::new(RefCell::new(Vec::new()));
        let (tx, rx) = tokio::sync::oneshot::channel::<u32>();
        let first = log.clone();
        tasks.spawn(async move {
            first.borrow_mut().push("a1");
            let value = rx.await.unwrap();
            first
                .borrow_mut()
                .push(if value == 7 { "a2" } else { "a?" });
        });
        let second = log.clone();
        tasks.spawn(async move {
            second.borrow_mut().push("b");
        });
        tasks.run_until_stalled();
        assert_eq!(*log.borrow(), ["a1", "b"]);
        assert_eq!(tasks.pending(), 1);
        tx.send(7).unwrap();
        tasks.run_until_stalled();
        assert_eq!(*log.borrow(), ["a1", "b", "a2"]);
        assert_eq!(tasks.pending(), 0);
    }

    #[test]
    fn a_panicking_task_does_not_stall_the_queue() {
        let tasks = LocalTasks::new();
        tasks.spawn(async { panic!("задача") });
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            tasks.run_until_stalled();
        }));
        assert!(caught.is_err());
        let done = Rc::new(Cell::new(false));
        let flag = done.clone();
        tasks.spawn(async move { flag.set(true) });
        tasks.run_until_stalled();
        assert!(done.get());
    }

    #[test]
    fn a_never_answered_task_stays_pending() {
        let tasks = LocalTasks::new();
        tasks.spawn(std::future::pending());
        tasks.run_until_stalled();
        tasks.run_until_stalled();
        assert_eq!(tasks.pending(), 1);
    }
}
