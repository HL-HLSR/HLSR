//! Настройки телеметрии и онлайна (`telemetry/config.js`). Значения — дословно из 0.4.7.
//! Адрес API в сборке не подменяется: службы принимают `endpoint`/`apiBase` в конструкторе
//! (как в 0.4.7), а подменяет его только тест или `dev-hooks`.

pub const BACKEND_CONFIGURED: bool = true;

pub const API_HOST: &str = "api.hlsr.pro";
pub const API_BASE: &str = "https://api.hlsr.pro";

/// `ENDPOINTS.batch`.
pub const ENDPOINT_BATCH: &str = "https://api.hlsr.pro/v1/telemetry/batch";
/// `ENDPOINTS.online`.
pub const ENDPOINT_ONLINE: &str = "https://api.hlsr.pro/v1/public/online";

pub const SCHEMA_VERSION: u32 = 1;

/// `QUEUE`.
pub mod queue {
    pub const FILE_NAME: &str = "telemetry-queue.jsonl";
    pub const MAX_EVENTS: usize = 1000;
    pub const MAX_BYTES: usize = 1024 * 1024;
    pub const BATCH_SIZE: usize = 50;
    pub const MAX_BATCH_BYTES: usize = 64 * 1024;
}

/// `TRANSPORT`.
pub mod transport {
    pub const TIMEOUT_MS: u64 = 15_000;
    pub const BACKOFF_BASE_MS: u64 = 30 * 1000;
    pub const BACKOFF_MAX_MS: u64 = 30 * 60 * 1000;
    pub const JITTER_RATIO: f64 = 0.25;
    pub const MAX_CONSECUTIVE_FAILURES: u32 = 10;
    pub const FLUSH_INTERVAL_MS: u64 = 60 * 1000;
}

/// `HEARTBEAT`.
pub mod heartbeat {
    pub const LAUNCHER_MS: u64 = 120 * 1000;
    pub const GAME_MS: u64 = 60 * 1000;
    pub const JITTER_RATIO: f64 = 0.2;
}

/// `ONLINE`.
pub mod online {
    pub const POLL_MS: u64 = 60 * 1000;
    pub const TIMEOUT_MS: u64 = 8000;
    pub const STALE_AFTER_MS: u64 = 5 * 60 * 1000;
}
