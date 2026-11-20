//! Мост tokio ↔ GPUI (схема `gpui_tokio` Zed, Apache-2.0). Фасады (`DeliveryApi`, `PlatformApi`,
//! `OnlineApi`, `UpdateApi`) живут на tokio; рантайм — в [`Global`] окна ([`init`]).
//!
//! - [`spawn`]: задача tokio, которую ждёт задача GPUI; брошенная `Task` снимает её (`abort`).
//! - [`invoke`]/[`call`]/[`quit`]: работа в потоке окна из любого потока (замена
//!   `slint::invoke_from_event_loop`); до [`init`] работа копится и выполняется, когда цикл событий
//!   пошёл.
//! - [`listen`]: события фасадов (`broadcast`) → `entity.update` в потоке окна. Приёмник tokio
//!   будится из любого исполнителя, отдельный `async_channel` не нужен.

use gpui::{App, AppContext, Context, Global, Task};
use std::future::Future;
use std::sync::Mutex;
use tokio::sync::{broadcast, mpsc, oneshot};

struct Tokio(tokio::runtime::Handle);

impl Global for Tokio {}

type Job = Box<dyn FnOnce(&mut App) + Send>;

struct Queue {
    tx: mpsc::UnboundedSender<Job>,
    rx: Option<mpsc::UnboundedReceiver<Job>>,
}

impl Queue {
    fn new() -> Queue {
        let (tx, rx) = mpsc::unbounded_channel();
        Queue { tx, rx: Some(rx) }
    }
}

/// Очередь работы потока окна: одна на процесс; следующий [`init`] (тесты) начинает новую.
static QUEUE: Mutex<Option<Queue>> = Mutex::new(None);

/// Рантайм tokio для [`spawn`] и разбор очереди [`invoke`] — в начале `Application::run`.
pub fn init(cx: &mut App, runtime: tokio::runtime::Handle) {
    cx.set_global(Tokio(runtime));
    run_queue(cx);
}

/// Только разбор очереди [`invoke`] (окна режимов без tokio).
pub fn run_queue(cx: &mut App) {
    let rx = {
        let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        let queue = queue.get_or_insert_with(Queue::new);
        if queue.rx.is_none() {
            *queue = Queue::new();
        }
        queue.rx.take()
    };
    let Some(mut rx) = rx else { return };
    cx.spawn(async move |cx| {
        while let Some(job) = rx.recv().await {
            cx.update(job);
        }
    })
    .detach();
}

/// Выполнить `job` в потоке окна; `false` — цикла событий больше нет.
pub fn invoke(job: impl FnOnce(&mut App) + Send + 'static) -> bool {
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    queue
        .get_or_insert_with(Queue::new)
        .tx
        .send(Box::new(job))
        .is_ok()
}

/// Цикл событий кончился: [`invoke`] больше не принимает работу и отвечает `false`. Иначе очередь
/// принимала бы её и после выхода (её разбор GPUI не снимает), а службы, которые на выходе ждут
/// окно, ждали бы каждая до своего тайм-аута. Следующий [`init`] (тесты) открывает новую.
pub fn close() {
    let (tx, _) = mpsc::unbounded_channel();
    *QUEUE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Queue { tx, rx: None });
}

/// Ответ потока окна для задачи tokio; `None` — цикла событий нет или он кончился раньше ответа.
pub async fn call<R: Send + 'static>(
    job: impl FnOnce(&mut App) -> R + Send + 'static,
) -> Option<R> {
    let (tx, rx) = oneshot::channel();
    if !invoke(move |cx| {
        let _ = tx.send(job(cx));
    }) {
        return None;
    }
    rx.await.ok()
}

/// `app.quit()` из любого потока.
pub fn quit() -> bool {
    invoke(|cx| cx.quit())
}

/// Задача tokio, которую ждёт GPUI. Брошенная `Task` снимает задачу tokio.
pub fn spawn<F>(cx: &App, future: F) -> Task<Result<F::Output, tokio::task::JoinError>>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let join = cx.global::<Tokio>().0.spawn(future);
    let abort = AbortOnDrop(join.abort_handle());
    cx.background_spawn(async move {
        let _abort = abort;
        join.await
    })
}

struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// События фасада → `on_event` сущности в потоке окна. Задача кончается с сущностью или
/// закрытым каналом; держать её (`Task`) или `detach`.
pub fn listen<T, V>(
    cx: &mut Context<V>,
    mut events: broadcast::Receiver<T>,
    mut on_event: impl FnMut(&mut V, T, &mut Context<V>) + 'static,
) -> Task<()>
where
    T: Clone + Send + 'static,
    V: 'static,
{
    cx.spawn(async move |this, cx| {
        loop {
            match events.recv().await {
                Ok(event) => {
                    if this
                        .update(cx, |this, cx| on_event(this, event, cx))
                        .is_err()
                    {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!("[bridge] окно отстало от событий: {skipped}");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Sink(Vec<u32>);

    #[gpui::test]
    fn tokio_work_comes_back_to_the_ui_thread(cx: &mut gpui::TestAppContext) {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        // Задачи tokio будят исполнитель GPUI из своих потоков.
        cx.executor().allow_parking();
        // Работа до `init` не теряется.
        let early = Arc::new(AtomicBool::new(false));
        let flag = early.clone();
        assert!(invoke(move |_| flag.store(true, Ordering::SeqCst)));
        cx.update(|cx| init(cx, runtime.handle().clone()));
        cx.run_until_parked();
        assert!(early.load(Ordering::SeqCst));

        // Из потока tokio — в поток окна и обратно.
        let pending = runtime.spawn(call(|cx| cx.has_global::<Tokio>()));
        let answer = cx
            .foreground_executor()
            .block_test(async { pending.await.unwrap() });
        assert_eq!(answer, Some(true));

        // События фасада → сущность.
        let (tx, rx) = broadcast::channel(8);
        let sink = cx.new(|cx| {
            listen(cx, rx, |sink: &mut Sink, n, _| sink.0.push(n)).detach();
            Sink(Vec::new())
        });
        tx.send(1).unwrap();
        tx.send(2).unwrap();
        cx.run_until_parked();
        assert_eq!(sink.read_with(cx, |sink, _| sink.0.clone()), [1, 2]);

        // Задача tokio, которую ждёт GPUI.
        let task = cx.update(|cx| spawn(cx, async { 6 * 7 }));
        let value = cx.foreground_executor().block_test(task).unwrap();
        assert_eq!(value, 42);
    }
}
