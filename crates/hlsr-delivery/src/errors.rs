//! Ошибки доставки (`errors.js`): коды, `DeliveryError`, перевод чужих ошибок,
//! блокировка файла антивирусом на Windows.

use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, Platform, errno};
use serde_json::{Value, json};
use std::path::Path;

macro_rules! codes {
    ($($variant:ident => $name:literal,)*) => {
        /// Код ошибки доставки (`CODES`); строка — как в 0.4.9.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Code {
            $($variant,)*
        }

        impl Code {
            /// Все коды в порядке `CODES`.
            pub const ALL: &[Code] = &[$(Code::$variant,)*];

            /// Имя кода: `NO_SPACE`, `FILE_LOCKED`…
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Code::$variant => $name,)*
                }
            }
        }
    };
}

codes! {
    ManifestNotFound => "MANIFEST_NOT_FOUND",
    ManifestInvalid => "MANIFEST_INVALID",
    ManifestDisabled => "MANIFEST_DISABLED",
    ManifestVersionDraft => "MANIFEST_VERSION_DRAFT",
    LauncherTooOld => "LAUNCHER_TOO_OLD",

    RemoteDisabled => "REMOTE_DISABLED",
    ProviderUnsupported => "PROVIDER_UNSUPPORTED",
    ProviderLinkFailed => "PROVIDER_LINK_FAILED",
    ProviderForbidden => "PROVIDER_FORBIDDEN",

    NetworkError => "NETWORK_ERROR",
    NetworkTimeout => "NETWORK_TIMEOUT",
    HttpError => "HTTP_ERROR",
    TooManyRedirects => "TOO_MANY_REDIRECTS",
    InsecureRedirect => "INSECURE_REDIRECT",
    RangeNotSupported => "RANGE_NOT_SUPPORTED",
    ContentLengthMismatch => "CONTENT_LENGTH_MISMATCH",

    SizeMismatch => "SIZE_MISMATCH",
    ChecksumMismatch => "CHECKSUM_MISMATCH",

    NoSpace => "NO_SPACE",
    PathNotWritable => "PATH_NOT_WRITABLE",
    PathInvalid => "PATH_INVALID",

    ArchiveListFailed => "ARCHIVE_LIST_FAILED",
    ArchiveUnsafeEntry => "ARCHIVE_UNSAFE_ENTRY",
    ArchiveLimitExceeded => "ARCHIVE_LIMIT_EXCEEDED",
    ExtractFailed => "EXTRACT_FAILED",
    RequiredFileMissing => "REQUIRED_FILE_MISSING",

    ToolUnknown => "TOOL_UNKNOWN",
    ToolExecutableMissing => "TOOL_EXECUTABLE_MISSING",
    ToolInstallFailed => "TOOL_INSTALL_FAILED",
    ToolLaunchFailed => "TOOL_LAUNCH_FAILED",
    ToolRemoveFailed => "TOOL_REMOVE_FAILED",

    AlreadyRunning => "ALREADY_RUNNING",
    Cancelled => "CANCELLED",
    InstallFailed => "INSTALL_FAILED",
    PreserveFailed => "PRESERVE_FAILED",
    AntivirusBlocked => "ANTIVIRUS_BLOCKED",

    PackFailed => "PACK_FAILED",
    PackBadFormat => "PACK_BAD_FORMAT",
    PackNewerFormat => "PACK_NEWER_FORMAT",
    PackUnknownGame => "PACK_UNKNOWN_GAME",
    PackTooLarge => "PACK_TOO_LARGE",
    PackDepotBuild => "PACK_DEPOT_BUILD",
    PackVersionUnknown => "PACK_VERSION_UNKNOWN",
    // В архиве изменены файлы сборки, а оригиналы не скачать (нет сети или зеркала версии).
    PackOriginalsUnavailable => "PACK_ORIGINALS_UNAVAILABLE",
    // Перед экспортом: файлы сборки изменены, окно спрашивает, продолжать ли.
    PackModified => "PACK_MODIFIED",

    CustomPackUnknown => "CUSTOM_PACK_UNKNOWN",
    CustomPackNotAvailable => "CUSTOM_PACK_NOT_AVAILABLE",
    CustomPackFileUnknown => "CUSTOM_PACK_FILE_UNKNOWN",

    NotInstalled => "NOT_INSTALLED",
    VersionMismatch => "VERSION_MISMATCH",
    PatchChainMissing => "PATCH_CHAIN_MISSING",
    PatchFailed => "PATCH_FAILED",
    FileLocked => "FILE_LOCKED",
    GameRunning => "GAME_RUNNING",
    VerifyFailed => "VERIFY_FAILED",
    RepairSourceMissing => "REPAIR_SOURCE_MISSING",
    RepairFailed => "REPAIR_FAILED",
    RollbackFailed => "ROLLBACK_FAILED",
    FilesManifestMissing => "FILES_MANIFEST_MISSING",

    ProcessNotFound => "PROCESS_NOT_FOUND",
    ProcessMismatch => "PROCESS_MISMATCH",
    StopTimeout => "STOP_TIMEOUT",
    StopFailed => "STOP_FAILED",

    ChannelDisabled => "CHANNEL_DISABLED",
    ChannelUnavailable => "CHANNEL_UNAVAILABLE",
    SignatureInvalid => "SIGNATURE_INVALID",
    UpdateRequired => "UPDATE_REQUIRED",

    BuildMetadataMissing => "BUILD_METADATA_MISSING",
    BuildMetadataInvalid => "BUILD_METADATA_INVALID",
    BuildMetadataMismatch => "BUILD_METADATA_MISMATCH",
    BuildMetadataUnsupported => "BUILD_METADATA_UNSUPPORTED",
    BuildRequiresNewerLauncher => "BUILD_REQUIRES_NEWER_LAUNCHER",

    SteamNotInstalled => "STEAM_NOT_INSTALLED",
    SteamNotRunning => "STEAM_NOT_RUNNING",
    SteamElevated => "STEAM_ELEVATED",
    SteamProcessMismatch => "STEAM_PROCESS_MISMATCH",
    SourceSdkNotInstalled => "SOURCE_SDK_NOT_INSTALLED",
    SourceSdkIncomplete => "SOURCE_SDK_INCOMPLETE",
    GameExecutableMissing => "GAME_EXECUTABLE_MISSING",
    GameDirectoryMissing => "GAME_DIRECTORY_MISSING",
    LibraryPathCaseConflict => "LIBRARY_PATH_CASE_CONFLICT",
    SteamAppIdMismatch => "STEAM_APP_ID_MISMATCH",
    LaunchConfigInvalid => "LAUNCH_CONFIG_INVALID",
    LaunchFailed => "LAUNCH_FAILED",
    ProtonWrapperMissing => "PROTON_WRAPPER_MISSING",
    // Запуск игры, пока инструмент «Демо» рендерит её в фоне.
    RenderActive => "RENDER_ACTIVE",

    SteamNotFound => "STEAM_NOT_FOUND",
    SteamAppNotOwned => "STEAM_APP_NOT_OWNED",
    SteamContentBusy => "STEAM_CONTENT_BUSY",
    SteamLinkFailed => "STEAM_LINK_FAILED",
    SteamDepotFailed => "STEAM_DEPOT_FAILED",
    SteamTimeout => "STEAM_TIMEOUT",
    LicenseCheckUnavailable => "LICENSE_CHECK_UNAVAILABLE",
    SteamManagedBuild => "STEAM_MANAGED_BUILD",
}

impl Code {
    /// Код по имени из `CODES`.
    pub fn from_name(name: &str) -> Option<Code> {
        Code::ALL.iter().copied().find(|code| code.as_str() == name)
    }

    /// Код по имени, как его принимает конструктор `DeliveryError`:
    /// незнакомое имя — `INSTALL_FAILED`.
    pub fn coerce(name: &str) -> Code {
        Code::from_name(name).unwrap_or(Code::InstallFailed)
    }
}

impl std::fmt::Display for Code {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `DeliveryError`: код, текст для игрока и подробности.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct DeliveryError {
    pub code: Code,
    pub message: String,
    pub details: Option<Value>,
}

impl DeliveryError {
    /// `new DeliveryError(code, message)`: пустое сообщение — имя кода.
    pub fn new(code: Code, message: impl Into<String>) -> DeliveryError {
        let message = message.into();
        DeliveryError {
            code,
            message: if message.is_empty() {
                code.as_str().to_owned()
            } else {
                message
            },
            details: None,
        }
    }

    /// Третий аргумент конструктора: ложное в JS значение (`null`, `0`, `""`) — без подробностей.
    pub fn with_details(mut self, details: Value) -> DeliveryError {
        self.details = js::truthy(&details).then_some(details);
        self
    }

    /// `toPublic()`: `{code, message, details}`.
    pub fn to_public(&self) -> Value {
        json!({
            "code": self.code.as_str(),
            "message": self.message,
            "details": self.details.clone().unwrap_or(Value::Null),
        })
    }
}

/// Ошибка не из доставки (fs, процесс, 7-Zip) в тех полях исключения Node,
/// которые читают `toDeliveryError` и `isAntivirusBlock`.
#[derive(Debug, Clone, Default, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct RawError {
    /// `error.code`: `EBUSY`, `UNKNOWN`, `ERR_FS_EISDIR`…
    pub code: Option<String>,
    /// `error.syscall`: `open`, `copyfile`…
    pub syscall: Option<String>,
    /// `error.path`.
    pub path: Option<String>,
    /// `error.message`.
    pub message: String,
    /// `error.stderr` — вывод 7-Zip.
    pub stderr: Option<String>,
}

impl RawError {
    /// Ошибка с одним текстом (`new Error(message)`).
    pub fn new(message: impl Into<String>) -> RawError {
        RawError {
            message: message.into(),
            ..RawError::default()
        }
    }

    /// Ошибка `fs` Node: код и текст libuv, системный вызов, путь.
    pub fn fs(error: &std::io::Error, syscall: &str, path: &Path) -> RawError {
        Self::fs_with_dest(error, syscall, path, None)
    }

    /// Ошибка `fs` Node с двумя путями (`copyfile`, `rename`): `… '<откуда>' -> '<куда>'`.
    pub fn fs_with_dest(
        error: &std::io::Error,
        syscall: &str,
        path: &Path,
        dest: Option<&Path>,
    ) -> RawError {
        let path = path.to_string_lossy().into_owned();
        let dest = dest.map(|d| d.to_string_lossy().into_owned());
        RawError {
            code: errno::code(error).map(std::borrow::Cow::into_owned),
            syscall: Some(syscall.to_owned()),
            message: errno::fs_message(error, syscall, &path, dest.as_deref()),
            path: Some(path),
            stderr: None,
        }
    }
}

/// Исключение, которое в 0.4.7 ловит `toDeliveryError`: своя ошибка или чужая.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Failure {
    #[error(transparent)]
    Delivery(#[from] DeliveryError),
    #[error(transparent)]
    Raw(#[from] RawError),
}

// Windows: файл, который антивирус признал вирусом (ERROR_VIRUS_INFECTED,
// ERROR_VIRUS_DELETED), libuv не знает и отдаёт как «UNKNOWN: unknown
// error, open …». 7-Zip пишет системный текст ошибки («…contains a virus…»,
// «…содержит вирус…»). Защитник Windows так помечает инструменты спидрана,
// например инжектор Bunnymod XT в сборке HL1.
const FS_SYSCALLS: [&str; 7] = [
    "open", "read", "rename", "copyfile", "unlink", "write", "close",
];

/// `isAntivirusBlock(error, platform)`.
pub fn is_antivirus_block(error: &RawError, platform: Platform) -> bool {
    if platform != Platform::Win32 {
        return false;
    }
    let syscall_blocked = error
        .syscall
        .as_deref()
        .is_some_and(|syscall| FS_SYSCALLS.contains(&syscall));
    if error.code.as_deref() == Some("UNKNOWN") && syscall_blocked {
        return true;
    }
    let text = format!(
        "{} {}",
        error.stderr.as_deref().unwrap_or_default(),
        error.message
    );
    mentions_virus(&text)
}

/// `/\bvirus\b|potentially unwanted|вирус/i`: флаг `i` без `u`, `\b` — по ASCII-словам.
fn mentions_virus(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let word = |c: Option<&char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_');
    (0..chars.len()).any(|at| {
        let virus = matches_at(&chars, at, "virus")
            && (at == 0 || !word(chars.get(at - 1)))
            && !word(chars.get(at + 5));
        virus || matches_at(&chars, at, "potentially unwanted") || matches_at(&chars, at, "вирус")
    })
}

fn matches_at(chars: &[char], at: usize, needle: &str) -> bool {
    let mut position = at;
    for expected in needle.chars() {
        match chars.get(position) {
            Some(&c)
                if js::canonicalize_ignore_case(c) == js::canonicalize_ignore_case(expected) =>
            {
                position += 1;
            }
            _ => return false,
        }
    }
    true
}

/// `antivirusError(filePath, error)`: `reason` — текст исходной ошибки (первые 300 единиц UTF-16).
pub fn antivirus_error(file_path: Option<&str>, reason: &str) -> DeliveryError {
    let file_path = file_path.filter(|p| !p.is_empty());
    let name = match file_path {
        Some(p) => format!("«{}»", NodePath::new(Platform::Win32, "").basename(p)),
        None => "файл игры".to_owned(),
    };
    DeliveryError::new(
        Code::AntivirusBlocked,
        format!(
            "Windows не дал открыть {name}: скорее всего, его заблокировал антивирус. \
             Защитник Windows бывает так реагирует на инструменты спидрана (Bunnymod XT). \
             Добавьте папку библиотеки HLSR в исключения антивируса и повторите установку."
        ),
    )
    .with_details(json!({"path": file_path, "reason": js::slice_utf16(reason, 300)}))
}

/// `preserveError(error)` на этой ОС: не скопировался файл игрока перед подменой сборки
/// (InstallService, depot-install). Прежняя сборка ещё цела: игроку надо освободить файл и повторить.
pub fn preserve_error(failure: Failure) -> DeliveryError {
    preserve_error_on(Platform::CURRENT, failure)
}

/// `preserveError` с `process.platform` = `platform`.
pub fn preserve_error_on(platform: Platform, failure: Failure) -> DeliveryError {
    let error = match failure {
        Failure::Delivery(error) => return error,
        Failure::Raw(error) => error,
    };
    if is_antivirus_block(&error, platform) {
        return antivirus_error(error.path.as_deref(), &error.message);
    }
    let path = error.path.as_deref().filter(|p| !p.is_empty());
    let file = path.map_or_else(|| "игрока".to_owned(), |p| format!("«{p}»"));
    let details = json!({"path": path});
    if error.code.as_deref() == Some("ENOSPC") {
        return DeliveryError::new(
            Code::NoSpace,
            format!("Не хватило места, чтобы перенести файл {file}. Прежняя сборка не тронута."),
        )
        .with_details(details);
    }
    // `error.code || error.message || String(error)`: у Error без текста String() — «Error».
    let reason = [error.code.as_deref(), Some(error.message.as_str())]
        .into_iter()
        .flatten()
        .find(|text| !text.is_empty())
        .unwrap_or("Error");
    DeliveryError::new(
        Code::PreserveFailed,
        format!(
            "Не удалось перенести файл {file} ({reason}). \
             Закрой программу, которая его держит, и повтори. Прежняя сборка не тронута."
        ),
    )
    .with_details(details)
}

/// `toDeliveryError(error, fallbackCode)` на этой ОС.
pub fn to_delivery_error(failure: Failure, fallback: Option<Code>) -> DeliveryError {
    to_delivery_error_on(Platform::CURRENT, failure, fallback)
}

/// `toDeliveryError` с `process.platform` = `platform`.
pub fn to_delivery_error_on(
    platform: Platform,
    failure: Failure,
    fallback: Option<Code>,
) -> DeliveryError {
    match failure {
        Failure::Delivery(error) => error,
        Failure::Raw(error) if is_antivirus_block(&error, platform) => {
            antivirus_error(error.path.as_deref(), &error.message)
        }
        Failure::Raw(error) => {
            // `error.message || String(error)`: у Error без текста String() — «Error».
            let message = if error.message.is_empty() {
                "Error".to_owned()
            } else {
                error.message
            };
            DeliveryError::new(fallback.unwrap_or(Code::InstallFailed), message)
        }
    }
}
