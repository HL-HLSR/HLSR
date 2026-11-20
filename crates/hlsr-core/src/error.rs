//! Ошибки основы и коды выхода процесса.

/// Ошибка основы лаунчера.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// Не удалось определить папку данных или журналов (нет HOME, APPDATA, LOCALAPPDATA).
    #[error("не найдена папка данных: не задана переменная {0}")]
    NoDataDir(&'static str),
    /// Журнал не подключён: подписчик или мост `log` уже установлен.
    #[error("журнал не подключён: {0}")]
    Logging(String),
    /// Не удалось проверить или захватить единственный экземпляр.
    #[error("единственный экземпляр: {0}")]
    Instance(std::io::Error),
    /// Не прочитан localStorage 0.4.7 (`Local Storage/leveldb`).
    #[error("localStorage 0.4.7: {0}")]
    LocalStorage(String),
}

/// Коды выхода. 3 и 4 не использовать: их занимает установщик 0.4.7.
pub mod exit {
    pub const OK: i32 = 0;
    pub const STARTUP_ERROR: i32 = 1;
    pub const MODE_NOT_IMPLEMENTED: i32 = 5;
    pub const OTHER_LAUNCHER_RUNNING: i32 = 6;
}
