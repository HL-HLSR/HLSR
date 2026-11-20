//! Отмена (`AbortController` 0.4.7) и общий итог идущей операции (промис, который ждут
//! несколько вызовов: `this.pending`, `this.fontsPending`).

use std::future::Future;
use std::sync::Arc;
use tokio::sync::watch;

/// `AbortController`/`AbortSignal`: подать сигнал, узнать, подан ли, дождаться его.
#[derive(Clone)]
pub struct Abort {
    flag: Arc<watch::Sender<bool>>,
}

impl Default for Abort {
    fn default() -> Abort {
        Abort {
            flag: Arc::new(watch::channel(false).0),
        }
    }
}

impl std::fmt::Debug for Abort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Abort")
            .field("aborted", &self.is_aborted())
            .finish()
    }
}

impl Abort {
    pub fn new() -> Abort {
        Abort::default()
    }

    /// `controller.abort()`.
    pub fn abort(&self) {
        self.flag.send_replace(true);
    }

    /// `signal.aborted`.
    pub fn is_aborted(&self) -> bool {
        *self.flag.borrow()
    }

    /// Готово, когда сигнал подан (поданный раньше — сразу).
    pub async fn aborted(&self) {
        let mut receiver = self.flag.subscribe();
        let _ = receiver.wait_for(|aborted| *aborted).await;
    }
}

/// Итог операции, которую ждут несколько вызовов. Операция идёт в своей задаче tokio (промис JS
/// нельзя бросить): брошенный ожидающий её не останавливает.
pub(crate) struct Shared<T: Clone> {
    result: watch::Receiver<Option<T>>,
}

impl<T: Clone> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Shared {
            result: self.result.clone(),
        }
    }
}

impl<T: Clone + Send + Sync + 'static> Shared<T> {
    /// Запустить `future`; `finally` выполняется до того, как итог увидят ожидающие
    /// (`.finally(() => { this.pending = null; })`).
    pub(crate) fn spawn<F>(future: F, finally: impl FnOnce() + Send + 'static) -> Shared<T>
    where
        F: Future<Output = T> + Send + 'static,
    {
        let (sender, result) = watch::channel(None);
        tokio::spawn(async move {
            let value = future.await;
            finally();
            sender.send_replace(Some(value));
        });
        Shared { result }
    }

    /// Дождаться итога.
    pub(crate) async fn wait(&self) -> T {
        let mut receiver = self.result.clone();
        let value = receiver
            .wait_for(Option::is_some)
            .await
            .expect("задача операции не бросает отправитель до итога");
        value.clone().expect("итог есть")
    }
}
