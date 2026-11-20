//! Зависимости обновления от внешнего мира — трейты (спецификация ПП6, архитектура). Реализации
//! для ОС — в модулях крейта; для тестов — подделки. Трейты добавляются задачами по мере надобности
//! (по строке): реестр, известные папки, ярлыки, процессы, диалоги — задачи 3–7.

use crate::cancel::CancelToken;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Журнал служб: общий с доставкой и онлайном (`hlsr_core::logger`).
pub use hlsr_core::logger::{Logger, MemoryLogger, TracingLogger};

/// Windows (задача 4): реестр, известные папки, ярлыки — трейты и подделки живут рядом с кодом
/// в `windows::*`, здесь только общий вход.
pub use crate::windows::known_folders::{FakeKnownFolders, KnownFolders};
pub use crate::windows::registry::{FakeRegistry, Registry};
pub use crate::windows::shortcuts::{FakeShellLinks, ShellLinks};
/// Задача 5: процессы, блокировка экземпляра, библиотеки Steam, телеметрия удаления.
pub use crate::{
    uninstall::cleanup_cli::UninstallTelemetry,
    uninstall::library_cleanup::{StaticSteamLibraries, SteamLibraries},
    windows::processes::{FakeProcesses, Processes},
    windows::setup::{FakeLock, InstanceLock},
};

/// Будущее трейтов-портов: трейты используются как `dyn`.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

// ------------------------------------------------------------------- часы

/// Часы: `Date.now()`.
pub trait Clock: Send + Sync {
    /// Миллисекунды от 1970-01-01 UTC.
    fn now_ms(&self) -> i64;
}

/// Часы системы.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as i64)
    }
}

/// Подделка для тестов: время задаёт тест.
#[derive(Debug, Default)]
pub struct FakeClock(AtomicI64);

impl FakeClock {
    pub fn new(ms: i64) -> FakeClock {
        FakeClock(AtomicI64::new(ms))
    }

    pub fn set(&self, ms: i64) {
        self.0.store(ms, Ordering::SeqCst);
    }

    pub fn advance(&self, ms: i64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for FakeClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

// ------------------------------------------------------------------- HTTP

/// Параметры одного GET (`options` `httpsGet`): простой соединения, предел тела, отмена.
#[derive(Debug, Clone, Default)]
pub struct GetOptions {
    /// Простой соединения; `None` или нулевой — 20 с (`DEFAULT_TIMEOUT_MS`).
    pub timeout: Option<Duration>,
    /// Предел тела; `None` — 256 КиБ (`MAX_BODY`).
    pub max_body: Option<usize>,
    /// `signal`.
    pub cancel: Option<CancelToken>,
}

/// Почему GET не удался. Тексты — как у `Error` в `LauncherUpdateGate.js`: их показывает журнал.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GetError {
    #[error("URL вне allowlist: {0}")]
    NotAllowed(String),
    #[error("слишком много редиректов")]
    TooManyRedirects,
    #[error("HTTP {0}")]
    Status(u16),
    #[error("ответ слишком большой")]
    TooLarge,
    #[error("таймаут")]
    Timeout,
    /// `AbortError` Node: сообщение дословно.
    #[error("The operation was aborted")]
    Aborted,
    /// Обрыв, отказ в соединении, ошибка TLS: тексты Node/libuv не воспроизводятся.
    #[error("{0}")]
    Network(String),
}

impl GetError {
    /// Класс ошибки для сверки с 0.4.7 (тексты сетевых ошибок у Node свои).
    pub fn kind(&self) -> String {
        match self {
            GetError::NotAllowed(_) => "not-allowed".to_owned(),
            GetError::TooManyRedirects => "redirects".to_owned(),
            GetError::Status(status) => format!("status:{status}"),
            GetError::TooLarge => "too-large".to_owned(),
            GetError::Timeout => "timeout".to_owned(),
            GetError::Aborted => "aborted".to_owned(),
            GetError::Network(_) => "network".to_owned(),
        }
    }
}

/// `httpGet` шлюза обновления: тело ответа 200 или ошибка.
pub trait HttpGet: Send + Sync {
    fn get<'a>(
        &'a self,
        url: &'a str,
        options: &'a GetOptions,
    ) -> BoxFuture<'a, Result<Vec<u8>, GetError>>;
}

/// Ответ подделки по адресу.
type Responder = Box<dyn Fn(&str) -> Result<Vec<u8>, GetError> + Send + Sync>;

/// Подделка для тестов: ответ даёт функция по адресу; запросы записываются.
pub struct FakeHttpGet {
    respond: Responder,
    calls: Mutex<Vec<(String, GetOptions)>>,
}

impl FakeHttpGet {
    pub fn new(
        respond: impl Fn(&str) -> Result<Vec<u8>, GetError> + Send + Sync + 'static,
    ) -> FakeHttpGet {
        FakeHttpGet {
            respond: Box::new(respond),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Адреса запросов по порядку.
    pub fn urls(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|(url, _)| url.clone())
            .collect()
    }

    /// Запросы по порядку: адрес и параметры.
    pub fn calls(&self) -> Vec<(String, GetOptions)> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl HttpGet for FakeHttpGet {
    fn get<'a>(
        &'a self,
        url: &'a str,
        options: &'a GetOptions,
    ) -> BoxFuture<'a, Result<Vec<u8>, GetError>> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((url.to_owned(), options.clone()));
        let result = (self.respond)(url);
        Box::pin(async move { result })
    }
}
