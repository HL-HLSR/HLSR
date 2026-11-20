//! Очередь задач окна в потоке интерфейса: `async`-сценарии 0.4.7 (`await ipcRenderer.invoke`)
//! идут в том же порядке, что в JS.
//!
//! Задача идёт сразу и до первого ожидания (как вызов `async`-функции), дальше её продолжает
//! очередь по кругу: каждый `await` ответа канала отдаёт управление остальным (см.
//! [`yield_now`]), поэтому при готовых ответах (фикстуры, быстрые фасады) вызовы каналов
//! чередуются так же, как микрозадачи JS: сначала все четыре загрузки окна начнут свои первые
//! вызовы, потом каждая продолжится. Ответ, которого ещё нет, будит очередь из любого потока:
//! продолжение выполняется в потоке интерфейса ([`Dispatch`] среды).

use crate::env::Dispatch;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::{Context, Poll, Wake, Waker};

type Job = Pin<Box<dyn Future<Output = ()>>>;

thread_local! {
    /// Очереди потока интерфейса по номеру: сюда возвращается пробуждение из другого потока.
    static QUEUES: RefCell<HashMap<u64, Weak<Inner>>> = RefCell::new(HashMap::new());
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct Flag {
    ready: AtomicBool,
    queue: u64,
    dispatch: Dispatch,
}

impl Wake for Flag {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.ready.store(true, Ordering::SeqCst);
        // Из потока интерфейса (первый опрос, `yield_now`) очередь уже идёт; из чужого потока ответ
        // канала — продолжение в цикле событий.
        let queue = self.queue;
        (self.dispatch)(Box::new(move || {
            QUEUES.with(|queues| {
                let found = queues.borrow().get(&queue).and_then(Weak::upgrade);
                if let Some(inner) = found {
                    Tasks { inner }.run();
                }
            });
        }));
    }
}

struct Task {
    job: RefCell<Option<Job>>,
    flag: Arc<Flag>,
}

struct Inner {
    id: u64,
    dispatch: Dispatch,
    queue: RefCell<VecDeque<Rc<Task>>>,
    running: Cell<bool>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        let id = self.id;
        let _ = QUEUES.try_with(|queues| {
            if let Ok(mut queues) = queues.try_borrow_mut() {
                queues.remove(&id);
            }
        });
    }
}

/// Очередь задач; клон — та же очередь.
#[derive(Clone)]
pub struct Tasks {
    inner: Rc<Inner>,
}

impl Tasks {
    /// Очередь; `dispatch` возвращает пробуждение из чужого потока в поток интерфейса.
    pub fn new(dispatch: Dispatch) -> Tasks {
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        let inner = Rc::new(Inner {
            id,
            dispatch,
            queue: RefCell::new(VecDeque::new()),
            running: Cell::new(false),
        });
        QUEUES.with(|queues| queues.borrow_mut().insert(id, Rc::downgrade(&inner)));
        Tasks { inner }
    }

    fn task(&self, job: Job, ready: bool) -> Rc<Task> {
        Rc::new(Task {
            job: RefCell::new(Some(job)),
            flag: Arc::new(Flag {
                ready: AtomicBool::new(ready),
                queue: self.inner.id,
                dispatch: self.inner.dispatch.clone(),
            }),
        })
    }

    /// Запустить задачу: идёт сразу, до первого ожидания; остальное — в [`Tasks::run`].
    pub fn spawn(&self, job: impl Future<Output = ()> + 'static) {
        let task = self.task(Box::pin(job), true);
        if !Self::poll(&task) {
            self.inner.queue.borrow_mut().push_back(task);
        }
    }

    /// Выполнить `action` позже остальных ждущих (как `Promise.then` Vue-обновления): после
    /// продолжений, уже стоящих в очереди.
    pub fn defer(&self, action: impl FnOnce() + 'static) {
        let task = self.task(
            Box::pin(async move {
                action();
            }),
            true,
        );
        self.inner.queue.borrow_mut().push_back(task);
    }

    /// Один опрос; `true` — задача закончена.
    fn poll(task: &Rc<Task>) -> bool {
        let Some(mut job) = task.job.borrow_mut().take() else {
            return true;
        };
        let waker = Waker::from(task.flag.clone());
        let mut cx = Context::from_waker(&waker);
        task.flag.ready.store(false, Ordering::SeqCst);
        match job.as_mut().poll(&mut cx) {
            Poll::Ready(()) => true,
            Poll::Pending => {
                *task.job.borrow_mut() = Some(job);
                false
            }
        }
    }

    /// Довести готовые задачи до следующего ожидания, по кругу, пока есть готовые.
    pub fn run(&self) {
        if self.inner.running.replace(true) {
            return;
        }
        loop {
            let mut progressed = false;
            let count = self.inner.queue.borrow().len();
            for _ in 0..count {
                let Some(task) = self.inner.queue.borrow_mut().pop_front() else {
                    break;
                };
                if task.flag.ready.swap(false, Ordering::SeqCst) {
                    progressed = true;
                    // Опрос выставил флаг заново, если задача снова готова (yield_now).
                    if !Self::poll(&task) {
                        self.inner.queue.borrow_mut().push_back(task);
                    }
                } else {
                    self.inner.queue.borrow_mut().push_back(task);
                }
            }
            if !progressed {
                break;
            }
        }
        self.inner.running.set(false);
    }

    /// Сколько задач ждёт (ответа канала или следующего круга).
    pub fn pending(&self) -> usize {
        self.inner.queue.borrow().len()
    }
}

/// `await` с передачей хода остальным: задача продолжится на следующем круге очереди.
pub async fn yield_now() {
    struct Yield(bool);

    impl Future for Yield {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if self.0 {
                Poll::Ready(())
            } else {
                self.0 = true;
                // Готова сразу: флаг выставлен, `run` продолжит её на следующем круге. Из чужого
                // потока пробуждения нет — только `wake_by_ref` в потоке интерфейса.
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    Yield(false).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Без цикла событий: продолжение — по `run`.
    fn tasks() -> Tasks {
        Tasks::new(Arc::new(|_| {}))
    }

    #[test]
    fn tasks_interleave_like_js_microtasks() {
        let tasks = tasks();
        let log = Rc::new(RefCell::new(Vec::<String>::new()));
        for name in ["a", "b", "c"] {
            let log = log.clone();
            tasks.spawn(async move {
                log.borrow_mut().push(format!("{name}1"));
                yield_now().await;
                log.borrow_mut().push(format!("{name}2"));
                yield_now().await;
                log.borrow_mut().push(format!("{name}3"));
            });
        }
        // Каждая задача дошла до первого ожидания сразу.
        assert_eq!(*log.borrow(), ["a1", "b1", "c1"]);
        tasks.run();
        assert_eq!(
            *log.borrow(),
            ["a1", "b1", "c1", "a2", "b2", "c2", "a3", "b3", "c3"]
        );
        assert_eq!(tasks.pending(), 0);
    }

    #[test]
    fn a_deferred_action_runs_after_the_continuations_already_queued() {
        let tasks = tasks();
        let log = Rc::new(RefCell::new(Vec::<&'static str>::new()));
        let l = log.clone();
        let t = tasks.clone();
        tasks.spawn(async move {
            l.borrow_mut().push("a1");
            yield_now().await;
            l.borrow_mut().push("a2");
            let l2 = l.clone();
            t.defer(move || l2.borrow_mut().push("flush"));
            yield_now().await;
            l.borrow_mut().push("a3");
        });
        let l = log.clone();
        tasks.spawn(async move {
            l.borrow_mut().push("b1");
            yield_now().await;
            l.borrow_mut().push("b2");
        });
        tasks.run();
        assert_eq!(*log.borrow(), ["a1", "b1", "a2", "b2", "flush", "a3"]);
    }

    #[test]
    fn a_task_waiting_for_a_reply_resumes_when_it_is_woken() {
        let tasks = tasks();
        let slot: Rc<RefCell<Option<Waker>>> = Rc::default();
        let done = Rc::new(Cell::new(false));
        let ready = Rc::new(Cell::new(false));
        struct Wait(Rc<Cell<bool>>, Rc<RefCell<Option<Waker>>>);
        impl Future for Wait {
            type Output = ();
            fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
                if self.0.get() {
                    Poll::Ready(())
                } else {
                    *self.1.borrow_mut() = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        }
        let (d, r, s) = (done.clone(), ready.clone(), slot.clone());
        tasks.spawn(async move {
            Wait(r, s).await;
            d.set(true);
        });
        tasks.run();
        assert!(!done.get());
        assert_eq!(tasks.pending(), 1);
        // Ответ пришёл: флаг выставлен (без цикла событий продолжение — по `run`).
        ready.set(true);
        slot.borrow_mut().take().unwrap().wake();
        tasks.run();
        assert!(done.get());
        assert_eq!(tasks.pending(), 0);
    }
}
