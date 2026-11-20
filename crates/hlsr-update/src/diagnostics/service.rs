//! Обработчики `diagnostics-run` и `diagnostics-cancel` из `AppManager.js:846-924`: номер
//! прогона (`runId`), повторный вызов во время прогона получает тот же результат, строки уходят
//! окну по мере готовности, последний результат помнится для отчёта, а сетевые сбои дают
//! уведомление `DIAGNOSTICS_NETWORK`.

use super::runner::{Check, DiagnosticsRunner, RunStatus};
use crate::ports::Logger;
use serde_json::{Value, json};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

/// Куда диагностика сообщает о происходящем.
pub trait DiagnosticsEvents: Send + Sync {
    /// `diagnostics-progress`: строка проверки с номером прогона.
    fn progress(&self, run_id: u64, check: &Check);
    /// Уведомление `DIAGNOSTICS_NETWORK` (уровень `warning`): коды сетевых сбоев через запятую.
    /// Сбой уведомления прогон не ломает.
    fn network_failed(&self, detail: &str);
}

/// Результат `diagnostics-run`.
#[derive(Debug, Clone, PartialEq)]
pub struct RunOutcome {
    pub run_id: u64,
    pub status: RunStatus,
    pub checks: Vec<Check>,
}

impl RunOutcome {
    /// `{runId, status, checks}`.
    pub fn to_value(&self) -> Value {
        json!({
            "runId": self.run_id,
            "status": self.status.as_str(),
            "checks": self.checks.iter().map(Check::to_value).collect::<Vec<_>>(),
        })
    }
}

type Shared = watch::Receiver<Option<Arc<RunOutcome>>>;

#[derive(Default)]
struct State {
    last_run_id: u64,
    active: Option<Shared>,
    last_checks: Option<Vec<Check>>,
}

pub struct DiagnosticsService {
    runner: DiagnosticsRunner,
    events: Arc<dyn DiagnosticsEvents>,
    logger: Arc<dyn Logger>,
    state: Mutex<State>,
}

impl DiagnosticsService {
    pub fn new(
        runner: DiagnosticsRunner,
        events: Arc<dyn DiagnosticsEvents>,
        logger: Arc<dyn Logger>,
    ) -> Arc<DiagnosticsService> {
        Arc::new(DiagnosticsService {
            runner,
            events,
            logger,
            state: Mutex::new(State::default()),
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Строки последнего прогона (`lastDiagnostics`) — для отчёта.
    pub fn last_checks(&self) -> Option<Vec<Check>> {
        self.state().last_checks.clone()
    }

    /// `diagnostics-cancel`.
    pub fn cancel(&self) -> bool {
        self.runner.cancel();
        true
    }

    /// `diagnostics-run`. Прогон идёт в своей задаче tokio: отказ вызывающего ждать его не
    /// обрывает, а второй вызов получает тот же итог (тот же `runId`).
    pub async fn run(self: &Arc<Self>) -> Arc<RunOutcome> {
        let mut receiver = {
            let mut state = self.state();
            if let Some(active) = &state.active {
                self.logger
                    .info("[diagnostics] прогон уже идёт — переиспользую активный runId");
                active.clone()
            } else {
                state.last_run_id += 1;
                let run_id = state.last_run_id;
                let (sender, receiver) = watch::channel(None);
                state.active = Some(receiver.clone());
                let service = self.clone();
                tokio::spawn(async move {
                    let outcome = Arc::new(service.execute(run_id).await);
                    {
                        let mut state = service.state();
                        state.active = None;
                        state.last_checks = Some(outcome.checks.clone());
                    }
                    let _ = sender.send(Some(outcome));
                });
                receiver
            }
        };
        match receiver.wait_for(Option::is_some).await {
            Ok(done) => done.clone().expect("итог прогона"),
            // Задача прогона оборвалась до результата: прогона как будто не было.
            Err(_) => Arc::new(RunOutcome {
                run_id: 0,
                status: RunStatus::Cancelled,
                checks: Vec::new(),
            }),
        }
    }

    async fn execute(&self, run_id: u64) -> RunOutcome {
        let events = self.events.clone();
        let emit = move |check: &Check| events.progress(run_id, check);
        let result = self.runner.run(Some(&emit)).await;

        let failed: Vec<&str> = result
            .checks
            .iter()
            .filter(|check| check.group() == "network" && check.status() == "fail")
            .filter_map(Check::code_str)
            .filter(|code| !code.is_empty())
            .collect();
        let has_failed = result
            .checks
            .iter()
            .any(|check| check.group() == "network" && check.status() == "fail");
        if has_failed {
            let detail = failed.join(", ");
            // Уведомление вторично.
            let _ = catch_unwind(AssertUnwindSafe(|| self.events.network_failed(&detail)));
        }

        RunOutcome {
            run_id,
            status: result.status,
            checks: result.checks,
        }
    }
}
