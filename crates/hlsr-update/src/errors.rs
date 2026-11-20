//! Коды и ошибки обновления: `LU_*` канала лаунчера (`launcher-channel.js:20-34`), `UPD_*`
//! внешнего апдейтера (`updater/core/errors.js`) и коды выхода установщика Windows
//! (`windows-install/setup.js:26`) и очистки после удаления (`uninstall/library-cleanup.js:9-19`) —
//! в одном месте. Значения — дословно из 0.4.7: их видит окно, журнал и обновлялка прежних версий.

use hlsr_core::js;
use serde_json::{Value, json};

/// `CODES` канала лаунчера: коды проверки метаданных и установщика.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LuCode {
    SchemaInvalid,
    SignatureInvalid,
    Downgrade,
    VersionUnsupported,
    VersionMismatch,
    LatestYmlMismatch,
    InstallerSize,
    InstallerSha256,
    InstallerSha512,
    InstallerUnreadable,
    InstallerTooBig,
    NoMetadata,
    FeedUnavailable,
}

impl LuCode {
    /// Все коды в порядке `CODES` 0.4.7.
    pub const ALL: [LuCode; 13] = [
        LuCode::SchemaInvalid,
        LuCode::SignatureInvalid,
        LuCode::Downgrade,
        LuCode::VersionUnsupported,
        LuCode::VersionMismatch,
        LuCode::LatestYmlMismatch,
        LuCode::InstallerSize,
        LuCode::InstallerSha256,
        LuCode::InstallerSha512,
        LuCode::InstallerUnreadable,
        LuCode::InstallerTooBig,
        LuCode::NoMetadata,
        LuCode::FeedUnavailable,
    ];

    /// Имя ключа в `CODES` 0.4.7 (`SCHEMA`, `UNSUPPORTED`…).
    pub const fn name(self) -> &'static str {
        match self {
            LuCode::SchemaInvalid => "SCHEMA",
            LuCode::SignatureInvalid => "SIGNATURE",
            LuCode::Downgrade => "DOWNGRADE",
            LuCode::VersionUnsupported => "UNSUPPORTED",
            LuCode::VersionMismatch => "VERSION_MISMATCH",
            LuCode::LatestYmlMismatch => "YML",
            LuCode::InstallerSize => "INSTALLER_SIZE",
            LuCode::InstallerSha256 => "INSTALLER_SHA256",
            LuCode::InstallerSha512 => "INSTALLER_SHA512",
            LuCode::InstallerUnreadable => "INSTALLER_UNREADABLE",
            LuCode::InstallerTooBig => "INSTALLER_TOO_BIG",
            LuCode::NoMetadata => "NO_METADATA",
            LuCode::FeedUnavailable => "FEED_UNAVAILABLE",
        }
    }

    /// Строка кода, как её видят окно и журнал.
    pub const fn as_str(self) -> &'static str {
        match self {
            LuCode::SchemaInvalid => "LU_SCHEMA_INVALID",
            LuCode::SignatureInvalid => "LU_SIGNATURE_INVALID",
            LuCode::Downgrade => "LU_DOWNGRADE",
            LuCode::VersionUnsupported => "LU_VERSION_UNSUPPORTED",
            LuCode::VersionMismatch => "LU_VERSION_MISMATCH",
            LuCode::LatestYmlMismatch => "LU_LATEST_YML_MISMATCH",
            LuCode::InstallerSize => "LU_INSTALLER_SIZE",
            LuCode::InstallerSha256 => "LU_INSTALLER_SHA256",
            LuCode::InstallerSha512 => "LU_INSTALLER_SHA512",
            LuCode::InstallerUnreadable => "LU_INSTALLER_UNREADABLE",
            LuCode::InstallerTooBig => "LU_INSTALLER_TOO_BIG",
            LuCode::NoMetadata => "LU_NO_METADATA",
            LuCode::FeedUnavailable => "LU_FEED_UNAVAILABLE",
        }
    }
}

impl std::fmt::Display for LuCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `CODES` внешнего апдейтера Windows (`updater/core/errors.js`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpdCode {
    AlreadyRunning,
    BadPlan,
    PlanUnreadable,
    StagingFailed,
    DownloadFailed,
    DownloadHttp,
    DownloadTimeout,
    DownloadSize,
    NoSpace,
    Cancelled,
    UrlNotAllowed,
    FilenameMismatch,
    VersionMismatch,
    SignatureInvalid,
    HlsrStillRunning,
    InstallFailed,
    InstallExitCode,
    InstallNotApplied,
    RelaunchFailed,
}

impl UpdCode {
    /// Все коды в порядке `CODES` 0.4.7.
    pub const ALL: [UpdCode; 19] = [
        UpdCode::AlreadyRunning,
        UpdCode::BadPlan,
        UpdCode::PlanUnreadable,
        UpdCode::StagingFailed,
        UpdCode::DownloadFailed,
        UpdCode::DownloadHttp,
        UpdCode::DownloadTimeout,
        UpdCode::DownloadSize,
        UpdCode::NoSpace,
        UpdCode::Cancelled,
        UpdCode::UrlNotAllowed,
        UpdCode::FilenameMismatch,
        UpdCode::VersionMismatch,
        UpdCode::SignatureInvalid,
        UpdCode::HlsrStillRunning,
        UpdCode::InstallFailed,
        UpdCode::InstallExitCode,
        UpdCode::InstallNotApplied,
        UpdCode::RelaunchFailed,
    ];

    /// Имя ключа в `CODES` 0.4.7 (`BAD_PLAN`, `INSTALL_FAILED`…).
    pub const fn name(self) -> &'static str {
        match self {
            UpdCode::AlreadyRunning => "ALREADY_RUNNING",
            UpdCode::BadPlan => "BAD_PLAN",
            UpdCode::PlanUnreadable => "PLAN_UNREADABLE",
            UpdCode::StagingFailed => "STAGING_FAILED",
            UpdCode::DownloadFailed => "DOWNLOAD_FAILED",
            UpdCode::DownloadHttp => "DOWNLOAD_HTTP",
            UpdCode::DownloadTimeout => "DOWNLOAD_TIMEOUT",
            UpdCode::DownloadSize => "DOWNLOAD_SIZE",
            UpdCode::NoSpace => "NO_SPACE",
            UpdCode::Cancelled => "CANCELLED",
            UpdCode::UrlNotAllowed => "URL_NOT_ALLOWED",
            UpdCode::FilenameMismatch => "FILENAME_MISMATCH",
            UpdCode::VersionMismatch => "VERSION_MISMATCH",
            UpdCode::SignatureInvalid => "SIGNATURE_INVALID",
            UpdCode::HlsrStillRunning => "HLSR_STILL_RUNNING",
            UpdCode::InstallFailed => "INSTALL_FAILED",
            UpdCode::InstallExitCode => "INSTALL_EXIT_CODE",
            UpdCode::InstallNotApplied => "INSTALL_NOT_APPLIED",
            UpdCode::RelaunchFailed => "RELAUNCH_FAILED",
        }
    }

    /// Строка кода, как её видит окно апдейтера.
    pub const fn as_str(self) -> &'static str {
        match self {
            UpdCode::AlreadyRunning => "UPD_ALREADY_RUNNING",
            UpdCode::BadPlan => "UPD_BAD_PLAN",
            UpdCode::PlanUnreadable => "UPD_PLAN_UNREADABLE",
            UpdCode::StagingFailed => "UPD_STAGING_FAILED",
            UpdCode::DownloadFailed => "UPD_DOWNLOAD_FAILED",
            UpdCode::DownloadHttp => "UPD_DOWNLOAD_HTTP",
            UpdCode::DownloadTimeout => "UPD_DOWNLOAD_TIMEOUT",
            UpdCode::DownloadSize => "UPD_DOWNLOAD_SIZE",
            UpdCode::NoSpace => "UPD_NO_SPACE",
            UpdCode::Cancelled => "UPD_CANCELLED",
            UpdCode::UrlNotAllowed => "UPD_URL_NOT_ALLOWED",
            UpdCode::FilenameMismatch => "UPD_FILENAME_MISMATCH",
            UpdCode::VersionMismatch => "UPD_VERSION_MISMATCH",
            UpdCode::SignatureInvalid => "UPD_SIGNATURE_INVALID",
            UpdCode::HlsrStillRunning => "UPD_HLSR_STILL_RUNNING",
            UpdCode::InstallFailed => "UPD_INSTALL_FAILED",
            UpdCode::InstallExitCode => "UPD_INSTALL_EXIT_CODE",
            UpdCode::InstallNotApplied => "UPD_INSTALL_NOT_APPLIED",
            UpdCode::RelaunchFailed => "UPD_RELAUNCH_FAILED",
        }
    }
}

impl std::fmt::Display for UpdCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Код ошибки апдейтера: свой `UPD_*` или код канала `LU_*`, который проверка плана
/// (`plan.js`) и установщика (`session.js`) передаёт окну как есть.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpdaterCode {
    Upd(UpdCode),
    Lu(LuCode),
}

impl UpdaterCode {
    /// Строка кода, как её видит окно апдейтера.
    pub const fn as_str(self) -> &'static str {
        match self {
            UpdaterCode::Upd(code) => code.as_str(),
            UpdaterCode::Lu(code) => code.as_str(),
        }
    }
}

impl From<UpdCode> for UpdaterCode {
    fn from(code: UpdCode) -> UpdaterCode {
        UpdaterCode::Upd(code)
    }
}

impl From<LuCode> for UpdaterCode {
    fn from(code: LuCode) -> UpdaterCode {
        UpdaterCode::Lu(code)
    }
}

impl PartialEq<UpdCode> for UpdaterCode {
    fn eq(&self, other: &UpdCode) -> bool {
        *self == UpdaterCode::Upd(*other)
    }
}

impl std::fmt::Display for UpdaterCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `UpdaterError`: ошибка апдейтера с кодом `UPD_*` (или `LU_*` канала) и подробностями для журнала.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct UpdaterError {
    pub code: UpdaterCode,
    /// `message || code`.
    pub message: String,
    /// `details || null`.
    pub details: Option<Value>,
}

impl UpdaterError {
    /// `new UpdaterError(code, message, details)`; пустое сообщение заменяется кодом.
    pub fn new(code: impl Into<UpdaterCode>, message: impl Into<String>) -> UpdaterError {
        let code = code.into();
        let message = message.into();
        UpdaterError {
            code,
            message: if message.is_empty() {
                code.as_str().to_owned()
            } else {
                message
            },
            details: None,
        }
    }

    pub fn with_details(mut self, details: Value) -> UpdaterError {
        self.details = Some(details);
        self
    }

    /// `toPublic()`: то, что уходит в окно апдейтера — код и сообщение до 400 символов.
    pub fn to_public(&self) -> Value {
        json!({
            "code": self.code.as_str(),
            "message": js::slice_utf16(&self.message, 400),
        })
    }

    /// `toUpdaterError(error, fallbackCode)` для чужой ошибки: её текст под кодом
    /// `fallback` (по умолчанию `UPD_INSTALL_FAILED`). Своя `UpdaterError` остаётся как есть.
    pub fn wrap(error: &dyn std::fmt::Display, fallback: Option<UpdCode>) -> UpdaterError {
        UpdaterError::new(
            fallback.unwrap_or(UpdCode::InstallFailed),
            error.to_string(),
        )
    }
}

/// Коды выхода установщика Windows: обновлятор прежней версии считает обновление состоявшимся
/// только при `OK` (`setup.js`: `EXIT`).
pub mod setup_exit {
    pub const OK: i32 = 0;
    pub const FAILED: i32 = 1;
    pub const IN_USE: i32 = 3;
    pub const BAD_DIR: i32 = 4;
}

/// Коды выхода очистки после удаления, `--hlsr-uninstall-cleanup` (`library-cleanup.js`: `CODES`).
pub mod cleanup_exit {
    pub const OK: i32 = 0;
    pub const USAGE: i32 = 2;
    pub const NOTHING_TO_DO: i32 = 3;
    pub const REFUSED_PATH: i32 = 10;
    pub const REFUSED_MARKER: i32 = 11;
    pub const REFUSED_REPARSE: i32 = 12;
    pub const REFUSED_RUNNING: i32 = 13;
    pub const PARTIAL: i32 = 14;
    pub const INTERNAL: i32 = 20;
}
