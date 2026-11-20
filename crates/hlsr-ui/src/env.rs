//! Цикл событий потока интерфейса для логики: задачи, отложенные вызовы, часы. В приложении его
//! даёт GPUI (`hlsr-gpui`), в тестах и сценах без окна — [`ManualEnv`].

use crate::controllers::tasks::{LocalFuture, LocalTasks};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// Среда потока интерфейса.
pub trait Env {
    /// Задача потока интерфейса: опрашивается в нём, будить её можно из любого потока.
    fn spawn_local(&self, task: LocalFuture);
    /// `action` в потоке интерфейса не раньше чем через `delay`.
    fn after(&self, delay: Duration, action: Box<dyn FnOnce()>);
    /// Монотонные часы, мс (`performance.now()`).
    fn now(&self) -> f64;
}

/// Как вернуть работу из другого потока в поток интерфейса.
pub type Dispatch = Arc<dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync>;

/// [`Dispatch`] на задаче среды: работа выполняется в потоке интерфейса в порядке прихода. Задача
/// кончается, когда уничтожены все копии [`Dispatch`].
pub fn dispatch(env: &dyn Env) -> Dispatch {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Box<dyn FnOnce() + Send>>();
    env.spawn_local(Box::pin(async move {
        while let Some(job) = rx.recv().await {
            job();
        }
    }));
    Arc::new(move |job| {
        if tx.send(job).is_err() {
            tracing::warn!("[env] поток интерфейса закрыт, работа не передана");
        }
    })
}

type Timer = (f64, u64, Box<dyn FnOnce()>);

/// Среда без цикла событий: часы двигает [`ManualEnv::advance`], задачи опрашивает
/// [`ManualEnv::run`].
#[derive(Default)]
pub struct ManualEnv {
    tasks: LocalTasks,
    now: Cell<f64>,
    timers: RefCell<Vec<Timer>>,
    seq: Cell<u64>,
}

impl ManualEnv {
    pub fn new() -> Rc<ManualEnv> {
        Rc::new(ManualEnv::default())
    }

    /// Довести разбуженные задачи до следующего ожидания.
    pub fn run(&self) {
        self.tasks.run_until_stalled();
    }

    /// Сдвинуть часы на `ms`: отложенные вызовы — по сроку (равные — в порядке постановки),
    /// задачи — после каждого.
    pub fn advance(&self, ms: f64) {
        let end = self.now.get() + ms;
        loop {
            self.run();
            let due = {
                let mut timers = self.timers.borrow_mut();
                let next = timers
                    .iter()
                    .enumerate()
                    .filter(|(_, (at, _, _))| *at <= end)
                    .min_by(|(_, a), (_, b)| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
                    .map(|(index, _)| index);
                next.map(|index| timers.remove(index))
            };
            let Some((at, _, action)) = due else { break };
            self.now.set(at.max(self.now.get()));
            action();
        }
        self.now.set(end);
    }

    /// Сколько отложенных вызовов ещё ждёт.
    pub fn pending_timers(&self) -> usize {
        self.timers.borrow().len()
    }
}

impl Env for ManualEnv {
    fn spawn_local(&self, task: LocalFuture) {
        self.tasks.spawn(task);
    }

    fn after(&self, delay: Duration, action: Box<dyn FnOnce()>) {
        let seq = self.seq.get() + 1;
        self.seq.set(seq);
        let at = self.now.get() + delay.as_secs_f64() * 1000.0;
        self.timers.borrow_mut().push((at, seq, action));
    }

    fn now(&self) -> f64 {
        self.now.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timers_fire_in_deadline_order_and_tasks_run_between() {
        let env = ManualEnv::new();
        let log = Rc::new(RefCell::new(Vec::new()));
        for (name, ms) in [("b", 20), ("a", 10), ("c", 20)] {
            let log = log.clone();
            let clock = env.clone();
            env.after(
                Duration::from_millis(ms),
                Box::new(move || log.borrow_mut().push((name, clock.now()))),
            );
        }
        let task_log = log.clone();
        env.spawn_local(Box::pin(async move {
            task_log.borrow_mut().push(("task", 0.0))
        }));
        env.advance(15.0);
        assert_eq!(*log.borrow(), [("task", 0.0), ("a", 10.0)]);
        assert_eq!(env.now(), 15.0);
        env.advance(5.0);
        assert_eq!(log.borrow()[2..], [("b", 20.0), ("c", 20.0)]);
        assert_eq!(env.pending_timers(), 0);
    }

    #[test]
    fn dispatched_work_runs_in_the_ui_thread_in_order() {
        let env = ManualEnv::new();
        let send = dispatch(&*env);
        let (tx, rx) = std::sync::mpsc::channel();
        let ui = std::thread::current().id();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for n in 0..3 {
                    let tx = tx.clone();
                    send(Box::new(move || {
                        tx.send((n, std::thread::current().id())).unwrap()
                    }));
                }
            });
        });
        assert!(rx.try_recv().is_err(), "до опроса задач — ничего");
        env.run();
        let got: Vec<_> = rx.try_iter().collect();
        assert_eq!(got, [(0, ui), (1, ui), (2, ui)]);
    }
}
