//! QA-подмена API профиля (`online-profile/local-qa.js`): локальные проверки могут перенаправить
//! весь трафик профиля на `http://127.0.0.1` и на свою папку данных, но только во временной папке
//! `hlsr-profile-qa-*`. Модуль есть только в отладочной сборке и с `dev-hooks`: выпуск 0.4.7
//! («упакованный») подмену не принимал, у выпуска 0.5 её нет вовсе.

use hlsr_core::js::fs as js_fs;
use hlsr_core::js::path::NodePath;
use hlsr_core::json_text::json_parse;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use url::Url;

/// Папка и адрес подмены.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QaSettings {
    pub root: PathBuf,
    /// `url.origin`: `http://127.0.0.1:18895`.
    pub api_base: String,
    /// `<root>/fake-steam.json`.
    pub account_file: PathBuf,
}

/// Подмена не принята; текст — как у исключения 0.4.7.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QaError {
    #[error("Profile QA requires an isolated temporary directory")]
    NotIsolated,
    #[error("Profile QA requires a loopback API")]
    NotLoopback,
    /// `new URL(HLSR_PROFILE_QA_API)` не разобрал адрес (`TypeError: Invalid URL`).
    #[error("Invalid URL")]
    InvalidUrl,
}

/// Значения, от которых зависит выбор: переменные окружения, `os.tmpdir()` и текущая папка.
#[derive(Debug, Clone, Default)]
pub struct QaEnv {
    /// `HLSR_PROFILE_QA_ROOT`.
    pub root: Option<String>,
    /// `HLSR_PROFILE_QA_API`.
    pub api: Option<String>,
    /// `os.tmpdir()`.
    pub tmpdir: String,
    /// `process.cwd()`: от неё разрешается относительный `root`.
    pub cwd: String,
}

impl QaEnv {
    /// Окружение процесса.
    pub fn from_process() -> QaEnv {
        QaEnv {
            root: std::env::var("HLSR_PROFILE_QA_ROOT").ok(),
            api: std::env::var("HLSR_PROFILE_QA_API").ok(),
            tmpdir: os_tmpdir(),
            cwd: std::env::current_dir()
                .map(|dir| dir.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }
    }
}

/// `os.tmpdir()`: `TMPDIR`, `TMP`, `TEMP` (в Windows — `TEMP`, `TMP`, `SystemRoot\temp`), без
/// косой черты в конце.
pub fn os_tmpdir() -> String {
    let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    #[cfg(windows)]
    let mut path = var("TEMP").or_else(|| var("TMP")).unwrap_or_else(|| {
        format!(
            "{}\\temp",
            var("SystemRoot")
                .or_else(|| var("windir"))
                .unwrap_or_default()
        )
    });
    #[cfg(not(windows))]
    let mut path = var("TMPDIR")
        .or_else(|| var("TMP"))
        .or_else(|| var("TEMP"))
        .unwrap_or_else(|| "/tmp".to_owned());
    let separator = if cfg!(windows) { '\\' } else { '/' };
    if path.len() > 1 && path.ends_with(separator) && !path.ends_with(":\\") {
        path.pop();
    }
    path
}

/// `settings(app, env)`: `None`, если подмена не запрошена (`HLSR_PROFILE_QA_ROOT` пуст).
pub fn settings(env: &QaEnv) -> Result<Option<QaSettings>, QaError> {
    let Some(root) = env.root.as_deref().filter(|root| !root.is_empty()) else {
        return Ok(None);
    };
    let path = NodePath::new(hlsr_core::js::Platform::CURRENT, env.cwd.clone());
    let root = path.resolve(&[root]);
    if path.dirname(&root) != env.tmpdir || !path.basename(&root).starts_with("hlsr-profile-qa-") {
        return Err(QaError::NotIsolated);
    }
    let api = env
        .api
        .as_deref()
        .filter(|api| !api.is_empty())
        .unwrap_or("http://127.0.0.1:18895");
    let url = Url::parse(api).map_err(|_| QaError::InvalidUrl)?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || !url.username().is_empty()
        || url.password().is_some_and(|password| !password.is_empty())
        || url.path() != "/"
    {
        return Err(QaError::NotLoopback);
    }
    let account_file = PathBuf::from(path.join(&[&root, "fake-steam.json"]));
    Ok(Some(QaSettings {
        root: PathBuf::from(root),
        api_base: url.origin().ascii_serialization(),
        account_file,
    }))
}

/// `prepare(app)`: то же и создание папки. Папку данных (`app.setPath("userData", …)`) заменяет
/// вызывающий.
pub fn prepare(env: &QaEnv) -> Result<Option<QaSettings>, QaPrepareError> {
    let qa = settings(env)?;
    if let Some(qa) = &qa {
        std::fs::create_dir_all(&qa.root).map_err(QaPrepareError::Io)?;
    }
    Ok(qa)
}

/// Ошибка [`prepare`].
#[derive(Debug, thiserror::Error)]
pub enum QaPrepareError {
    #[error(transparent)]
    Qa(#[from] QaError),
    #[error("не удалось создать папку подмены: {0}")]
    Io(std::io::Error),
}

fn read_json(file: &Path) -> Option<Value> {
    let bytes = js_fs::read_file(file).ok()?;
    json_parse(&String::from_utf8_lossy(&bytes)).ok()
}

/// `account(qa)`: содержимое `fake-steam.json`; нет файла или он не JSON — `{}`.
pub fn account(qa: &QaSettings) -> Value {
    read_json(&qa.account_file).unwrap_or_else(|| json!({}))
}

/// `friends(qa)`: содержимое `fake-friends.json`; нет файла или он не JSON — `[]`.
pub fn friends(qa: &QaSettings) -> Value {
    read_json(&qa.root.join("fake-friends.json")).unwrap_or_else(|| json!([]))
}
