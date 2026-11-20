//! Фоновые задачи tokio, паника которых не роняет лаунчер.

use std::any::Any;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::task::{Context, Poll};

/// Запустить задачу; её паника пишется в журнал (`фоновая задача {name} упала: …`),
/// приложение продолжает работу. Отмена через `JoinHandle::abort` отменяет саму задачу.
pub fn spawn_logged<F>(name: &'static str, fut: F) -> tokio::task::JoinHandle<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        if let Err(payload) = (CatchUnwind(Box::pin(fut))).await {
            tracing::error!(
                target: "launcher",
                "фоновая задача {name} упала: {}",
                payload_text(payload.as_ref())
            );
        }
    })
}

struct CatchUnwind<F>(Pin<Box<F>>);

impl<F: Future> Future for CatchUnwind<F> {
    type Output = Result<F::Output, Box<dyn Any + Send>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let fut = self.0.as_mut();
        match catch_unwind(AssertUnwindSafe(|| fut.poll(cx))) {
            Ok(Poll::Ready(output)) => Poll::Ready(Ok(output)),
            Ok(Poll::Pending) => Poll::Pending,
            Err(payload) => Poll::Ready(Err(payload)),
        }
    }
}

pub(crate) fn payload_text(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("(не строка)")
}

/// Выполнить `f`, перехватив панику: `Err(текст паники)`. Для обработчиков, чьё падение не должно
/// ронять вызвавшего (в 0.4.7 исключение обработчика ловил `try/catch` и писал в журнал).
/// Единственное место, где крейты лаунчера перехватывают панику синхронного вызова.
pub fn catch_panic<R>(f: impl FnOnce() -> R) -> Result<R, String> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|payload| payload_text(payload.as_ref()).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catch_panic_returns_the_value_or_the_panic_text() {
        assert_eq!(catch_panic(|| 7), Ok(7));
        assert_eq!(
            catch_panic(|| -> () { panic!("литерал") }),
            Err("литерал".to_owned())
        );
        let number = 5;
        assert_eq!(
            catch_panic(|| -> () { panic!("значение {number}") }),
            Err("значение 5".to_owned())
        );
        assert_eq!(
            catch_panic(|| -> () { std::panic::panic_any(5_u8) }),
            Err("(не строка)".to_owned())
        );
    }
}
