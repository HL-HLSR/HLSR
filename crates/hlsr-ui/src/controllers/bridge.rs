//! Мост окна к сервисам для контроллеров экранов: `ipcRenderer.invoke(channel, ...args)` с
//! ответом в потоке интерфейса и `ipcRenderer.on(channel, handler)` с отпиской при `Drop`.
//!
//! Ответ, готовый сразу (фикстуры, быстрые фасады), принимается тут же; ждущий — задачей среды
//! ([`Env::spawn_local`]). События окну забирает один отложенный вызов на мост: 20 раз в секунду,
//! пока есть хотя бы один слушатель.
//!
//! Один мост на окно: слушатели разных экранов (аккаунт, уведомления, «Главная») делят таймер.

use crate::env::Env;
use crate::services::{Reply, ServiceEvent, Services};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::rc::{Rc, Weak};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio::sync::broadcast::{self, error::TryRecvError};

/// Как часто мост забирает события окна.
pub const EVENTS_POLL: Duration = Duration::from_millis(50);

type Handler = Rc<dyn Fn(&Value)>;

struct Hub {
    env: Rc<dyn Env>,
    receiver: RefCell<broadcast::Receiver<ServiceEvent>>,
    handlers: RefCell<Vec<(u64, String, Handler)>>,
    next: Cell<u64>,
    /// Опрос событий запланирован; без слушателей следующий не ставится.
    polling: Cell<bool>,
}

/// Подписка на событие: слушатель снимается, когда подписка уничтожена.
#[must_use = "слушатель снимается вместе с подпиской"]
pub struct Subscription {
    id: u64,
    hub: Weak<Hub>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(hub) = self.hub.upgrade() {
            hub.handlers
                .borrow_mut()
                .retain(|(id, _, _)| *id != self.id);
        }
    }
}

/// Мост окна; клон — тот же мост.
#[derive(Clone)]
pub struct Bridge {
    services: Services,
    hub: Rc<Hub>,
}

impl Bridge {
    pub fn new(services: Services, env: Rc<dyn Env>) -> Bridge {
        let receiver = services.subscribe();
        Bridge {
            services,
            hub: Rc::new(Hub {
                env,
                receiver: RefCell::new(receiver),
                handlers: RefCell::new(Vec::new()),
                next: Cell::new(1),
                polling: Cell::new(false),
            }),
        }
    }

    /// Сервисы окна (хранилища, `send`, `sendSync`).
    pub fn services(&self) -> &Services {
        &self.services
    }

    /// `ipcRenderer.invoke(channel, ...args)`; ответ — в `done`, в потоке интерфейса.
    pub fn invoke(&self, channel: &str, args: Vec<Value>, done: impl FnOnce(Reply) + 'static) {
        let mut reply = self.services.invoke(channel, args);
        match Future::poll(reply.as_mut(), &mut Context::from_waker(Waker::noop())) {
            Poll::Ready(reply) => done(reply),
            Poll::Pending => self
                .hub
                .env
                .spawn_local(Box::pin(async move { done(reply.await) })),
        }
    }

    /// `ipcRenderer.invoke(channel, ...args)` без чтения ответа (`.catch(() => {})`).
    pub fn fire(&self, channel: &str, args: Vec<Value>) {
        self.invoke(channel, args, |_| {});
    }

    /// `ipcRenderer.on(channel, handler)`: `handler(payload)` на каждое событие канала.
    pub fn on(&self, channel: &str, handler: impl Fn(&Value) + 'static) -> Subscription {
        let hub = &self.hub;
        let id = hub.next.get();
        hub.next.set(id + 1);
        hub.handlers
            .borrow_mut()
            .push((id, channel.to_owned(), Rc::new(handler)));
        if !hub.polling.replace(true) {
            Hub::schedule(hub);
        }
        Subscription {
            id,
            hub: Rc::downgrade(hub),
        }
    }

    /// Забрать накопившиеся события сейчас (тесты, первый кадр сцены).
    pub fn pump(&self) {
        Hub::pump(&self.hub);
    }
}

impl Hub {
    /// Следующий опрос через [`EVENTS_POLL`]; без слушателей опрос кончается.
    fn schedule(hub: &Rc<Hub>) {
        let weak = Rc::downgrade(hub);
        hub.env.after(
            EVENTS_POLL,
            Box::new(move || {
                let Some(hub) = weak.upgrade() else { return };
                Hub::pump(&hub);
                if hub.handlers.borrow().is_empty() {
                    hub.polling.set(false);
                } else {
                    Hub::schedule(&hub);
                }
            }),
        );
    }

    fn pump(hub: &Rc<Hub>) {
        loop {
            let event = hub.receiver.borrow_mut().try_recv();
            match event {
                Ok(event) => {
                    // Слушатели могут (сн)ять подписки, пока идёт рассылка.
                    let handlers: Vec<Handler> = hub
                        .handlers
                        .borrow()
                        .iter()
                        .filter(|(_, channel, _)| *channel == event.channel)
                        .map(|(_, _, handler)| handler.clone())
                        .collect();
                    for handler in handlers {
                        handler(&event.payload);
                    }
                }
                Err(TryRecvError::Lagged(_)) => {}
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
            }
        }
    }
}

/// Перевод на текущем языке: ключ, аргументы → строка (`$t`).
pub type Tr = Rc<dyn Fn(&str, &[&str]) -> String>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::ManualEnv;
    use crate::services::{BoxFuture, MainProcess, StoreRead};
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use tokio::sync::oneshot;

    struct Fake {
        events: broadcast::Sender<ServiceEvent>,
        reply: Mutex<Option<oneshot::Receiver<Reply>>>,
    }

    impl MainProcess for Fake {
        fn invoke(&self, _: &str, _: Vec<Value>) -> BoxFuture<'static, Reply> {
            let reply = self.reply.lock().unwrap().take().unwrap();
            Box::pin(async move { reply.await.unwrap() })
        }
        fn send(&self, _: &str, _: Vec<Value>) {}
        fn send_sync(&self, _: &str, _: Vec<Value>) -> Value {
            Value::Null
        }
        fn store_read(&self, _: &str) -> StoreRead {
            StoreRead::Missing
        }
        fn store_write(&self, _: &str, _: Value) -> bool {
            false
        }
        fn store_quarantine(&self, _: &str) -> Value {
            Value::Null
        }
        fn subscribe(&self) -> broadcast::Receiver<ServiceEvent> {
            self.events.subscribe()
        }
    }

    #[test]
    fn events_are_polled_while_someone_listens_and_late_replies_arrive() {
        let env = ManualEnv::new();
        let (answer_tx, answer_rx) = oneshot::channel();
        let fake = Arc::new(Fake {
            events: broadcast::channel(8).0,
            reply: Mutex::new(Some(answer_rx)),
        });
        let bridge = Bridge::new(fake.clone(), env.clone());
        let got = Rc::new(RefCell::new(Vec::new()));
        let sink = got.clone();
        let subscription = bridge.on("x", move |value| sink.borrow_mut().push(value.clone()));
        let event = |payload| ServiceEvent {
            channel: "x".into(),
            payload,
        };
        fake.events.send(event(json!(1))).unwrap();
        env.advance(49.0);
        assert!(got.borrow().is_empty());
        env.advance(1.0);
        assert_eq!(*got.borrow(), [json!(1)]);
        drop(subscription);
        fake.events.send(event(json!(2))).unwrap();
        env.advance(50.0);
        assert_eq!(*got.borrow(), [json!(1)], "слушатель снят");
        assert_eq!(env.pending_timers(), 0, "без слушателей опрос кончился");

        let answer = Rc::new(RefCell::new(None));
        let slot = answer.clone();
        bridge.invoke("slow", vec![], move |reply| {
            *slot.borrow_mut() = Some(reply)
        });
        env.run();
        assert!(answer.borrow().is_none());
        answer_tx.send(Ok(json!("ok"))).unwrap();
        env.run();
        assert_eq!(*answer.borrow(), Some(Ok(json!("ok"))));
    }
}
