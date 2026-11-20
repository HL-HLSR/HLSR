//! Среда логики (`hlsr_ui::env::Env`) на GPUI: задачи и отложенные вызовы — исполнители окна,
//! часы — часы исполнителя (в тестах GPUI их двигает `advance_clock`).

use gpui::{App, BackgroundExecutor, ForegroundExecutor};
use hlsr_ui::controllers::tasks::LocalFuture;
use hlsr_ui::env::Env;
use std::rc::Rc;
use std::time::{Duration, Instant};

pub struct GpuiEnv {
    foreground: ForegroundExecutor,
    background: BackgroundExecutor,
    start: Instant,
}

impl GpuiEnv {
    pub fn new(cx: &App) -> Rc<GpuiEnv> {
        let background = cx.background_executor().clone();
        Rc::new(GpuiEnv {
            foreground: cx.foreground_executor().clone(),
            start: background.now(),
            background,
        })
    }
}

impl Env for GpuiEnv {
    fn spawn_local(&self, task: LocalFuture) {
        self.foreground.spawn(task).detach();
    }

    fn after(&self, delay: Duration, action: Box<dyn FnOnce()>) {
        let timer = self.background.timer(delay);
        self.foreground
            .spawn(async move {
                timer.await;
                action();
            })
            .detach();
    }

    fn now(&self) -> f64 {
        self.background
            .now()
            .duration_since(self.start)
            .as_secs_f64()
            * 1000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[gpui::test]
    fn timers_and_tasks_run_on_the_ui_thread(cx: &mut gpui::TestAppContext) {
        let env = cx.update(|cx| GpuiEnv::new(cx));
        let log = Rc::new(RefCell::new(Vec::new()));
        for (name, ms) in [("b", 20), ("a", 10)] {
            let log = log.clone();
            env.after(
                Duration::from_millis(ms),
                Box::new(move || log.borrow_mut().push(name)),
            );
        }
        let task_log = log.clone();
        env.spawn_local(Box::pin(async move { task_log.borrow_mut().push("task") }));
        cx.run_until_parked();
        assert_eq!(*log.borrow(), ["task"]);
        cx.executor().advance_clock(Duration::from_millis(15));
        cx.run_until_parked();
        assert_eq!(*log.borrow(), ["task", "a"]);
        cx.executor().advance_clock(Duration::from_millis(5));
        cx.run_until_parked();
        assert_eq!(*log.borrow(), ["task", "a", "b"]);
        assert_eq!(env.now(), 20.0);
    }
}
