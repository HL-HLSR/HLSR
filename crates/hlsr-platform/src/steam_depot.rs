//! `game-delivery/steam-depot.js`: скачивание сборки из депо Steam и проверка лицензий через
//! консоль самого клиента Steam. Лаунчер ничего не знает о сети Steam: он отдаёт клиенту команды
//! (`steam://+<команда> <аргументы>` запуском `steam.exe`) и читает ответы из
//! `logs/console_log.txt`. Лицензия на игру — `licenses_for_app <id>`; депо — `download_depot`,
//! причём клиент пишет файлы в `steamapps/content/app_<слот>/depot_<id>`, а этот каталог лаунчер
//! подменяет ссылкой (junction в Windows, симлинк в Linux) на папку внутри библиотеки.
//!
//! Внешний мир — за портами: запуск программ ([`CommandRunner`]), пауза ([`Sleeper`]), часы и
//! поиск Steam ([`DepotSteam`]); в тестах их подменяют, а файлы настоящие.

use crate::process_inspect::node_path;
use crate::steam_locator::{RunningSteam, SteamInstall, SteamLocator};
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, Platform};
use hlsr_core::logger::{Logger, TracingLogger};
use hlsr_delivery::errors::{Code, DeliveryError, Failure, RawError};
use hlsr_delivery::paths::assert_inside_with;
use hlsr_delivery::providers::BoxFuture;
use hlsr_delivery::timing::{Clock, system_clock};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

pub const DEFAULT_TIMEOUT_MS: u64 = 10 * 60 * 1000;
pub const COMMAND_TIMEOUT_MS: u64 = 15_000;
pub const POLL_INTERVAL_MS: u64 = 400;
pub const MAX_CONTENT_SLOT: u32 = 256;

/// Фильтр файлов, под который не подходит ничего: так клиент «разогревается» перед настоящим
/// скачиванием депо.
pub const NO_MATCH_FILTER: &str = "?!";

/// `RESPONSES`: что клиент Steam пишет в консоль в ответ.
pub mod responses {
    pub const DOWNLOAD_OK: &str = "Depot download complete";
    pub const DOWNLOAD_FAILED: &str = "Depot download failed";
    pub const NO_LICENSE: &str = "No active license found for appID";
    pub const PURCHASED: &str = "]  - Purchased : ";
}

/// Депо сборки (`CATALOG[…].depots[…]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Depot {
    pub app_id: u32,
    pub depot_id: u32,
    pub manifest_id: &'static str,
    pub file_filter: Option<&'static str>,
    pub expected_files: Option<u32>,
}

/// Сборка в каталоге депо (`CATALOG[buildId]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogEntry {
    pub requires_app_ids: &'static [u32],
    pub depots: &'static [Depot],
}

/// `CATALOG`.
pub const CATALOG: [(&str, CatalogEntry); 1] = [(
    "hl2-5288",
    CatalogEntry {
        requires_app_ids: &[220],
        depots: &[
            Depot {
                app_id: 220,
                depot_id: 222,
                manifest_id: "3151477805868332059",
                file_filter: Some("bin*;hl2*"),
                expected_files: Some(103),
            },
            Depot {
                app_id: 220,
                depot_id: 221,
                manifest_id: "2694190745568322895",
                file_filter: Some("platform*;hl2*;bin*"),
                expected_files: Some(1058),
            },
            Depot {
                app_id: 243750,
                depot_id: 243752,
                manifest_id: "8806556782944295588",
                file_filter: Some("*FileSystem_Stdio.dll"),
                expected_files: Some(1),
            },
        ],
    },
)];

fn invalid(message: String) -> DeliveryError {
    DeliveryError::new(Code::ManifestInvalid, message)
}

/// `^[0-9]{1,20}$`.
fn is_manifest_id(text: &str) -> bool {
    (1..=20).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit())
}

/// `^[A-Za-z0-9_.*;-]{1,120}$`.
fn is_file_filter(text: &str) -> bool {
    (1..=120).contains(&text.chars().count())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.*;-".contains(&b))
}

/// `Number.isInteger(value) && value > 0` для значения JSON.
fn is_positive_integer(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_f64)
        .is_some_and(|number| number.is_finite() && number.fract() == 0.0 && number > 0.0)
}

/// `assertDepot(depot)`: описание депо `{appId, depotId, manifestId, fileFilter}` верно.
pub fn assert_depot(depot: &Value) -> Result<(), DeliveryError> {
    let field = |name: &str| depot.get(name);
    if !is_positive_integer(field("appId")) {
        return Err(invalid(format!(
            "Некорректный appId депота: {}",
            js::to_template_string(field("appId"))
        )));
    }
    if !is_positive_integer(field("depotId")) {
        return Err(invalid(format!(
            "Некорректный depotId: {}",
            js::to_template_string(field("depotId"))
        )));
    }
    if !is_manifest_id(&js::to_template_string(field("manifestId"))) {
        return Err(invalid(format!(
            "Некорректный manifestId: {}",
            js::to_template_string(field("manifestId"))
        )));
    }
    if field("fileFilter").is_some_and(|filter| !filter.is_null())
        && !is_file_filter(&js::to_template_string(field("fileFilter")))
    {
        return Err(invalid(format!(
            "Некорректный фильтр файлов: {}",
            js::to_template_string(field("fileFilter"))
        )));
    }
    Ok(())
}

fn depot_value(depot: &Depot) -> Value {
    json!({
        "appId": depot.app_id,
        "depotId": depot.depot_id,
        "manifestId": depot.manifest_id,
        "fileFilter": depot.file_filter,
    })
}

/// `catalogFor(buildId)`: сборка объявлена в каталоге депо.
pub fn catalog_for(build_id: &str) -> Result<&'static CatalogEntry, DeliveryError> {
    let Some((_, entry)) = CATALOG.iter().find(|(id, _)| *id == build_id) else {
        return Err(
            invalid(format!("Сборка {build_id} не объявлена в каталоге депотов"))
                .with_details(json!({"buildId": build_id})),
        );
    };
    for depot in entry.depots {
        assert_depot(&depot_value(depot))?;
    }
    Ok(entry)
}

// ------------------------------------------------------------------------------ порты

/// То, что службе нужно от поиска Steam (общее для Windows и Linux).
pub trait DepotSteam: Send + Sync {
    /// `locateSteam()`.
    fn locate_steam(&self) -> Result<SteamInstall, DeliveryError>;
    /// `assertSteamRunning(steam)`.
    fn assert_steam_running(&self, steam: &SteamInstall) -> Result<RunningSteam, DeliveryError>;
}

impl DepotSteam for SteamLocator {
    fn locate_steam(&self) -> Result<SteamInstall, DeliveryError> {
        SteamLocator::locate_steam(self)
    }

    fn assert_steam_running(&self, steam: &SteamInstall) -> Result<RunningSteam, DeliveryError> {
        SteamLocator::assert_steam_running(self, steam)
    }
}

impl DepotSteam for crate::linux::steam_locator::SteamLocator {
    fn locate_steam(&self) -> Result<SteamInstall, DeliveryError> {
        crate::linux::steam_locator::SteamLocator::locate_steam(self)
    }

    /// На Linux проверка не смотрит на найденную установку.
    fn assert_steam_running(&self, _steam: &SteamInstall) -> Result<RunningSteam, DeliveryError> {
        crate::linux::steam_locator::SteamLocator::assert_steam_running(self)
    }
}

/// Код ошибки запуска у `execFile`: число (код выхода) или имя (`ENOENT`, `EACCES`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunCode {
    Exit(i64),
    Name(String),
}

/// `result.error` из `defaultRun`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunError {
    /// `error.killed`: процесс убит по тайм-ауту.
    pub killed: bool,
    /// `error.signal`.
    pub signal: Option<String>,
    pub code: Option<RunCode>,
    pub message: String,
}

/// Итог `run(file, args, options)` (`defaultRun` не отклоняется никогда).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunResult {
    pub error: Option<RunError>,
    pub status: i64,
    pub stdout: String,
    pub stderr: String,
}

/// Запуск программы без оболочки и без окна, с тайм-аутом (`execFile`).
pub trait CommandRunner: Send + Sync {
    fn run<'a>(
        &'a self,
        file: &'a str,
        args: &'a [String],
        timeout_ms: u64,
    ) -> BoxFuture<'a, RunResult>;
}

/// Настоящий запуск (`defaultRun`).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run<'a>(
        &'a self,
        file: &'a str,
        args: &'a [String],
        timeout_ms: u64,
    ) -> BoxFuture<'a, RunResult> {
        Box::pin(async move {
            let mut command = tokio::process::Command::new(file);
            // Без переменных окна лаунчера (решение 3 спецификации).
            crate::child_env::strip_ui_environment(&mut command);
            command
                .args(args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);
            #[cfg(windows)]
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: windowsHide
            let child = match command.spawn() {
                Ok(child) => child,
                Err(error) => {
                    // Rust сам ищет программу в PATH и тогда отвечает без кода ОС.
                    let code = hlsr_core::js::errno::code(&error)
                        .map(std::borrow::Cow::into_owned)
                        .unwrap_or_else(|| match error.kind() {
                            std::io::ErrorKind::NotFound => "ENOENT".to_owned(),
                            std::io::ErrorKind::PermissionDenied => "EACCES".to_owned(),
                            _ => "UNKNOWN".to_owned(),
                        });
                    return RunResult {
                        error: Some(RunError {
                            killed: false,
                            signal: None,
                            message: format!("spawn {file} {code}"),
                            code: Some(RunCode::Name(code)),
                        }),
                        status: 1,
                        stdout: String::new(),
                        stderr: String::new(),
                    };
                }
            };
            match tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait_with_output())
                .await
            {
                // `kill_on_drop` убивает процесс, когда будущее сброшено.
                Err(_) => RunResult {
                    error: Some(RunError {
                        killed: true,
                        signal: Some("SIGTERM".to_owned()),
                        code: None,
                        message: format!("Command failed: {file}"),
                    }),
                    status: 1,
                    stdout: String::new(),
                    stderr: String::new(),
                },
                Ok(Err(error)) => RunResult {
                    error: Some(RunError {
                        killed: false,
                        signal: None,
                        code: Some(RunCode::Name("UNKNOWN".to_owned())),
                        message: error.to_string(),
                    }),
                    status: 1,
                    ..RunResult::default()
                },
                Ok(Ok(output)) => {
                    let code = output.status.code().map_or(1, i64::from);
                    RunResult {
                        error: (code != 0).then(|| RunError {
                            killed: false,
                            signal: None,
                            code: Some(RunCode::Exit(code)),
                            message: format!("Command failed: {file}"),
                        }),
                        status: code,
                        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                    }
                }
            }
        })
    }
}

/// Пауза между опросами консоли Steam.
pub trait Sleeper: Send + Sync {
    fn sleep<'a>(&'a self, ms: u64) -> BoxFuture<'a, ()>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TokioSleeper;

impl Sleeper for TokioSleeper {
    fn sleep<'a>(&'a self, ms: u64) -> BoxFuture<'a, ()> {
        Box::pin(tokio::time::sleep(Duration::from_millis(ms)))
    }
}

// ------------------------------------------------------------------------ хвост консоли

/// `ConsoleTail`: новые строки файла с момента создания хвоста. Файл усекли — читаем с начала;
/// последняя строка без перевода строки ждёт продолжения.
pub struct ConsoleTail {
    file: String,
    position: u64,
    buffer: String,
}

impl ConsoleTail {
    /// Хвост начинается с текущего конца файла (нет файла — с начала).
    pub fn new(file: &str) -> ConsoleTail {
        ConsoleTail {
            position: std::fs::metadata(file).map_or(0, |meta| meta.len()),
            file: file.to_owned(),
            buffer: String::new(),
        }
    }

    /// `read()`: завершённые строки, дописанные с прошлого раза.
    pub fn read(&mut self) -> Vec<String> {
        let Ok(meta) = std::fs::metadata(&self.file) else {
            return Vec::new();
        };
        let size = meta.len();
        if size < self.position {
            self.position = 0;
        }
        if size == self.position {
            return Vec::new();
        }

        let chunk = (|| -> std::io::Result<Vec<u8>> {
            let mut handle = std::fs::File::open(&self.file)?;
            handle.seek(SeekFrom::Start(self.position))?;
            let mut buf = Vec::with_capacity((size - self.position) as usize);
            handle.take(size - self.position).read_to_end(&mut buf)?;
            Ok(buf)
        })();
        let Ok(chunk) = chunk else {
            return Vec::new();
        };
        self.position += chunk.len() as u64;
        self.buffer.push_str(&js::utf8_string(&chunk));

        let mut lines: Vec<String> = split_lines(&self.buffer);
        self.buffer = lines.pop().unwrap_or_default();
        lines
    }
}

/// `text.split(/\r?\n/)`.
fn split_lines(text: &str) -> Vec<String> {
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_owned())
        .collect()
}

// ------------------------------------------------------------------------------ служба

/// Что нужно от Steam для одной команды: пути клиента.
struct SteamPaths {
    exe: String,
    base_args: Vec<String>,
    content_dir: String,
    console_log: String,
}

/// Параметры конструктора (`options`).
#[derive(Default)]
pub struct DepotOptions {
    pub locator: Option<Arc<dyn DepotSteam>>,
    pub logger: Option<Arc<dyn Logger>>,
    pub run: Option<Arc<dyn CommandRunner>>,
    pub command_timeout_ms: Option<u64>,
    pub sleep: Option<Arc<dyn Sleeper>>,
    pub now: Option<Clock>,
    pub timeout_ms: Option<u64>,
    pub poll_ms: Option<u64>,
    pub platform: Option<Platform>,
}

#[derive(Default)]
struct State {
    owned_cache: HashSet<u64>,
    owned_session_pid: Option<u32>,
}

/// Ответ `ownershipOf`: `{appId, owned, code, message}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ownership {
    /// Что спросили (как пришло: число, строка…).
    pub app_id: Value,
    /// `true`/`false`; `None` — узнать не удалось.
    pub owned: Option<bool>,
    pub code: Option<Code>,
    pub message: Option<String>,
    pub cached: bool,
}

impl Ownership {
    /// Объект ответа: `cached` только у ответа из памяти.
    pub fn to_value(&self) -> Value {
        let mut object = serde_json::Map::new();
        object.insert("appId".into(), self.app_id.clone());
        object.insert("owned".into(), json!(self.owned));
        object.insert("code".into(), json!(self.code.map(|code| code.as_str())));
        if let Some(message) = &self.message {
            object.insert("message".into(), json!(message));
        }
        if self.cached {
            object.insert("cached".into(), json!(true));
        }
        Value::Object(object)
    }
}

/// Ход скачивания одного депо (`onProgress`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadProgress {
    pub depot_id: u32,
    pub index: usize,
    pub total: usize,
    pub files: u64,
    pub expected_files: Option<u64>,
}

pub type ProgressFn<'a> = &'a (dyn Fn(&DownloadProgress) + Send + Sync);

/// Параметры `download(buildId, targetDir, options)`.
#[derive(Default, Clone, Copy)]
pub struct DownloadOptions<'a> {
    /// `libraryRoot`: ссылки на слот разрешены только в эту библиотеку.
    pub library_root: Option<&'a str>,
    pub on_progress: Option<ProgressFn<'a>>,
}

/// Итог `download`: `{status: "downloaded", buildId, targetDir, depots: [{depotId, dir}]}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Downloaded {
    pub build_id: String,
    pub target_dir: String,
    pub depots: Vec<(u32, String)>,
}

impl Downloaded {
    pub fn to_value(&self) -> Value {
        json!({
            "status": "downloaded",
            "buildId": self.build_id,
            "targetDir": self.target_dir,
            "depots": self
                .depots
                .iter()
                .map(|(id, dir)| json!({"depotId": id, "dir": dir}))
                .collect::<Vec<_>>(),
        })
    }
}

/// Найденное в консоли: какой образец и в какой строке.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub needle: &'static str,
    pub line: String,
}

struct Slot {
    number: u32,
    path: String,
}

/// `SteamDepotService`.
pub struct SteamDepotService {
    locator: Option<Arc<dyn DepotSteam>>,
    logger: Arc<dyn Logger>,
    run: Arc<dyn CommandRunner>,
    command_timeout_ms: u64,
    sleeper: Arc<dyn Sleeper>,
    now: Clock,
    timeout_ms: u64,
    poll_ms: u64,
    platform: Platform,
    path: NodePath,
    state: Mutex<State>,
}

fn or_default(value: Option<u64>, default: u64) -> u64 {
    value.filter(|value| *value != 0).unwrap_or(default)
}

/// `Math.round(ms / 1000)`.
fn seconds_text(ms: u64) -> String {
    js::math_round(ms as f64 / 1000.0).to_string()
}

impl SteamDepotService {
    pub fn new(options: DepotOptions) -> SteamDepotService {
        let platform = options.platform.unwrap_or(Platform::CURRENT);
        SteamDepotService {
            locator: options.locator,
            logger: options.logger.unwrap_or_else(|| Arc::new(TracingLogger)),
            run: options.run.unwrap_or_else(|| Arc::new(SystemRunner)),
            command_timeout_ms: or_default(options.command_timeout_ms, COMMAND_TIMEOUT_MS),
            sleeper: options.sleep.unwrap_or_else(|| Arc::new(TokioSleeper)),
            now: options.now.unwrap_or_else(system_clock),
            timeout_ms: or_default(options.timeout_ms, DEFAULT_TIMEOUT_MS),
            poll_ms: or_default(options.poll_ms, POLL_INTERVAL_MS),
            platform,
            path: node_path(platform),
            state: Mutex::new(State::default()),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn locator(&self) -> Result<&Arc<dyn DepotSteam>, DeliveryError> {
        self.locator
            .as_ref()
            .ok_or_else(|| DeliveryError::new(Code::SteamNotFound, "Локатор Steam не задан"))
    }

    /// `requireSteamSession()`: Steam запущен; сеанс сменился — лицензии проверяются заново.
    fn require_steam_session(&self) -> Result<RunningSteam, DeliveryError> {
        let locator = self.locator()?;
        let running = locator.assert_steam_running(&locator.locate_steam()?)?;
        let pid = (running.pid != 0).then_some(running.pid);

        let mut state = self.state();
        if state.owned_session_pid != pid {
            if !state.owned_cache.is_empty() {
                self.logger
                    .info("[steam-depot] сеанс Steam сменился — лицензии проверяются заново");
            }
            state.owned_cache.clear();
            state.owned_session_pid = pid;
        }
        Ok(running)
    }

    /// `steamPaths()`.
    fn steam_paths(&self) -> Result<SteamPaths, DeliveryError> {
        let steam = self.locator()?.locate_steam()?;
        if steam.install_path.is_empty() || steam.steam_exe.is_empty() {
            return Err(DeliveryError::new(
                Code::SteamNotFound,
                format!(
                    "Локатор Steam вернул неполные данные: installPath={}, steamExe={}",
                    steam.install_path, steam.steam_exe
                ),
            ));
        }
        Ok(SteamPaths {
            content_dir: self
                .path
                .join(&[&steam.install_path, "steamapps", "content"]),
            console_log: self
                .path
                .join(&[&steam.install_path, "logs", "console_log.txt"]),
            exe: steam.steam_exe,
            // Flatpak-Steam в Linux запускается как `flatpak run com.valvesoftware.Steam`.
            base_args: steam.steam_args.unwrap_or_default(),
        })
    }

    /// `sendCommand(paths, parts)`.
    async fn send_command(
        &self,
        paths: &SteamPaths,
        parts: &[String],
    ) -> Result<RunResult, DeliveryError> {
        let mut argv = paths.base_args.clone();
        argv.push(format!("steam://+{}", parts[0]));
        argv.extend(parts[1..].iter().cloned());

        self.logger
            .info(&format!("[steam-depot] команда: {}", parts.join(" ")));

        let result = self
            .run
            .run(&paths.exe, &argv, self.command_timeout_ms)
            .await;

        if let Some(error) = &result.error {
            let timed_out = error.killed || error.signal.as_deref() == Some("SIGTERM");
            let not_started = matches!(error.code, Some(RunCode::Name(_)));

            if timed_out {
                return Err(DeliveryError::new(
                    Code::SteamTimeout,
                    format!(
                        "Steam не принял команду за {} с — вероятно, он ещё запускается",
                        seconds_text(self.command_timeout_ms)
                    ),
                ));
            }
            if not_started {
                return Err(DeliveryError::new(
                    Code::SteamNotRunning,
                    format!("Не удалось обратиться к Steam: {}", error.message),
                ));
            }
            let code = match &error.code {
                Some(RunCode::Exit(code)) => code.to_string(),
                _ => "null".to_owned(),
            };
            self.logger.info(&format!(
                "[steam-depot] Steam вышел с кодом {code} на команде {} — ответ ждём в логе",
                parts[0]
            ));
        }
        Ok(result)
    }

    /// `waitFor(tail, needles, options)`: ждать строку с одним из образцов.
    async fn wait_for(
        &self,
        tail: &mut ConsoleTail,
        needles: &[&'static str],
        timeout_ms: Option<u64>,
        mut on_poll: Option<&mut (dyn FnMut() + Send)>,
    ) -> Result<Found, DeliveryError> {
        let timeout = or_default(timeout_ms, self.timeout_ms);
        let deadline = (self.now)() + timeout as i64;

        loop {
            for line in tail.read() {
                for needle in needles {
                    if line.contains(needle) {
                        return Ok(Found { needle, line });
                    }
                }
            }

            if let Some(on_poll) = on_poll.as_deref_mut() {
                on_poll();
            }

            if (self.now)() >= deadline {
                return Err(DeliveryError::new(
                    Code::SteamTimeout,
                    format!("Steam не ответил за {} с", seconds_text(timeout)),
                ));
            }

            self.sleeper.sleep(self.poll_ms).await;
        }
    }

    /// `assertOwns(appIds)`: все игры есть на аккаунте.
    pub async fn assert_owns(&self, app_ids: &[u32]) -> Result<(), DeliveryError> {
        if app_ids.is_empty() {
            return Ok(());
        }

        self.require_steam_session()?;

        let unchecked: Vec<u32> = {
            let state = self.state();
            app_ids
                .iter()
                .copied()
                .filter(|app_id| !state.owned_cache.contains(&u64::from(*app_id)))
                .collect()
        };
        if unchecked.is_empty() {
            return Ok(());
        }

        for app_id in unchecked {
            let verdict = self.ownership_of(&json!(app_id), Some(60_000)).await;
            if verdict.owned == Some(true) {
                continue;
            }
            return Err(DeliveryError::new(
                verdict.code.unwrap_or(Code::SteamAppNotOwned),
                verdict
                    .message
                    .filter(|message| !message.is_empty())
                    .unwrap_or_else(|| format!("Игры нет на аккаунте Steam (AppID {app_id})")),
            )
            .with_details(json!({"appId": app_id})));
        }
        Ok(())
    }

    /// `ownershipOf(appId, options)`.
    pub async fn ownership_of(&self, app_id: &Value, timeout_ms: Option<u64>) -> Ownership {
        let verdict = |owned, code: Option<Code>, message: Option<String>| Ownership {
            app_id: app_id.clone(),
            owned,
            code,
            message,
            cached: false,
        };

        // `Number.isInteger(appId) && appId > 0`
        let number = app_id
            .as_f64()
            .filter(|number| number.is_finite() && number.fract() == 0.0 && *number > 0.0);
        let Some(number) = number else {
            return verdict(
                None,
                Some(Code::ManifestInvalid),
                Some(format!(
                    "Некорректный appId: {}",
                    js::to_template_string(Some(app_id))
                )),
            );
        };
        let key = number as u64;
        let id_text = js::to_template_string(Some(app_id));

        let paths = match self
            .steam_paths()
            .and_then(|paths| self.require_steam_session().map(|_| paths))
        {
            Ok(paths) => paths,
            Err(error) => return verdict(None, Some(error.code), Some(error.message)),
        };

        if self.state().owned_cache.contains(&key) {
            return Ownership {
                cached: true,
                ..verdict(Some(true), None, None)
            };
        }

        let found = async {
            let mut tail = ConsoleTail::new(&paths.console_log);
            self.send_command(&paths, &["licenses_for_app".to_owned(), id_text.clone()])
                .await?;
            self.wait_for(
                &mut tail,
                &[responses::NO_LICENSE, responses::PURCHASED],
                Some(or_default(timeout_ms, 60_000)),
                None,
            )
            .await
        }
        .await;
        let found = match found {
            Ok(found) => found,
            Err(error) => return verdict(None, Some(error.code), Some(error.message)),
        };

        if found.needle == responses::NO_LICENSE {
            return verdict(
                Some(false),
                Some(Code::SteamAppNotOwned),
                Some(format!("Игры нет на аккаунте Steam (AppID {id_text})")),
            );
        }

        self.state().owned_cache.insert(key);
        self.logger
            .info(&format!("[steam-depot] лицензия на {id_text} подтверждена"));

        verdict(Some(true), None, None)
    }

    // ------------------------------------------------------------------- слоты загрузки

    /// `sweepContentSlots(contentDir, libraryRoot)`: брошенные ссылки `app_<n>` на папки
    /// библиотеки снимаются.
    pub fn sweep_content_slots(&self, content_dir: &str, library_root: Option<&str>) -> usize {
        let Some(library_root) = library_root.filter(|root| !root.is_empty()) else {
            return 0;
        };
        let Ok(entries) = std::fs::read_dir(content_dir) else {
            return 0;
        };
        let names: Vec<String> = entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();

        let mut removed = 0;
        for name in names {
            if !is_slot_name(&name) {
                continue;
            }
            let full = self.path.join(&[content_dir, &name]);
            let Ok(meta) = std::fs::symlink_metadata(&full) else {
                continue;
            };
            if !meta.file_type().is_symlink() {
                continue;
            }
            let Ok(target) = std::fs::read_link(&full) else {
                continue;
            };
            let target = link_target_text(&target);
            if assert_inside_with(
                &self.path,
                library_root,
                &target,
                Some("Цель ссылки в каталоге Steam"),
            )
            .is_err()
            {
                continue;
            }
            if self.unlink_content_slot(&full) {
                removed += 1;
                self.logger
                    .info(&format!("[steam-depot] снята ссылка {name} → {target}"));
            }
        }

        if removed > 0 {
            self.logger
                .info(&format!("[steam-depot] убрано брошенных ссылок: {removed}"));
        }
        removed
    }

    /// `freeContentSlot(contentDir)`: первый свободный слот от 2.
    fn free_content_slot(&self, content_dir: &str) -> Result<Slot, DeliveryError> {
        for number in 2..=MAX_CONTENT_SLOT {
            let candidate = self.path.join(&[content_dir, &format!("app_{number}")]);
            if std::fs::symlink_metadata(&candidate).is_err() {
                return Ok(Slot {
                    number,
                    path: candidate,
                });
            }
        }
        Err(DeliveryError::new(
            Code::SteamContentBusy,
            format!(
                "В каталоге загрузок Steam не нашлось свободного слота (проверено {MAX_CONTENT_SLOT})"
            ),
        ))
    }

    /// `linkContentSlot(slotPath, targetDir, libraryRoot)`.
    async fn link_content_slot(
        &self,
        slot_path: &str,
        target_dir: &str,
        library_root: &str,
    ) -> Result<(), Failure> {
        assert_inside_with(
            &self.path,
            library_root,
            target_dir,
            Some("Каталог загрузки депота"),
        )?;

        std::fs::create_dir_all(target_dir)
            .map_err(|error| RawError::fs(&error, "mkdir", Path::new(target_dir)))?;

        // Без `cmd.exe /c mklink` (A-4): путь библиотеки с «&» не должен разбираться как команда.
        let linked = if self.platform == Platform::Win32 {
            create_junction(target_dir, slot_path)
        } else {
            symlink_dir(target_dir, slot_path)
        };
        linked.map_err(|error| {
            DeliveryError::new(
                Code::SteamLinkFailed,
                format!(
                    "Не удалось связать каталог загрузок Steam с {target_dir}: {}",
                    symlink_error_text(&error, target_dir, slot_path)
                ),
            )
            .into()
        })
    }

    /// `unlinkContentSlot(slotPath)`: снять ссылку (содержимое цели не трогается).
    pub fn unlink_content_slot(&self, slot_path: impl AsRef<Path>) -> bool {
        let slot_path = slot_path.as_ref();
        let Ok(meta) = std::fs::symlink_metadata(slot_path) else {
            return false;
        };
        if !meta.file_type().is_symlink() {
            self.logger.warn(&format!(
                "[steam-depot] {} не является ссылкой — не трогаю",
                slot_path.display()
            ));
            return false;
        }

        if std::fs::remove_dir(slot_path).is_ok() {
            return true;
        }
        match std::fs::remove_file(slot_path) {
            Ok(()) => true,
            Err(error) => {
                let raw = RawError::fs(&error, "unlink", slot_path);
                self.logger.warn(&format!(
                    "[steam-depot] ссылку {} снять не удалось: {}",
                    slot_path.display(),
                    raw.message
                ));
                false
            }
        }
    }

    /// `download(buildId, targetDir, options)`: все депо сборки — в `targetDir/depot_<id>`.
    pub async fn download(
        &self,
        build_id: &str,
        target_dir: &str,
        options: DownloadOptions<'_>,
    ) -> Result<Downloaded, Failure> {
        let entry = catalog_for(build_id)?;
        let paths = self.steam_paths()?;
        let Some(library_root) = options.library_root.filter(|root| !root.is_empty()) else {
            return Err(DeliveryError::new(
                Code::PathInvalid,
                "Не задан корень библиотеки для загрузки депота",
            )
            .into());
        };

        self.assert_owns(entry.requires_app_ids).await?;

        self.sweep_content_slots(&paths.content_dir, Some(library_root));

        let slot = self.free_content_slot(&paths.content_dir)?;
        std::fs::create_dir_all(&paths.content_dir)
            .map_err(|error| RawError::fs(&error, "mkdir", Path::new(&paths.content_dir)))?;
        self.link_content_slot(&slot.path, target_dir, library_root)
            .await?;

        let result = self
            .download_depots(
                entry,
                build_id,
                target_dir,
                &paths,
                &slot,
                options.on_progress,
            )
            .await;
        self.unlink_content_slot(&slot.path);
        result
    }

    async fn download_depots(
        &self,
        entry: &CatalogEntry,
        build_id: &str,
        target_dir: &str,
        paths: &SteamPaths,
        slot: &Slot,
        on_progress: Option<ProgressFn<'_>>,
    ) -> Result<Downloaded, Failure> {
        let log = &paths.console_log;
        let words =
            |parts: &[&str]| -> Vec<String> { parts.iter().map(|p| (*p).to_owned()).collect() };

        let mut timeout_tail = ConsoleTail::new(log);
        self.send_command(paths, &words(&["DepotDownloadProgressTimeout", "30"]))
            .await?;
        self.wait_for(
            &mut timeout_tail,
            &["DepotDownloadProgressTimeout"],
            Some(30_000),
            None,
        )
        .await?;

        for (index, depot) in entry.depots.iter().enumerate() {
            let mut prime_filter_tail = ConsoleTail::new(log);
            self.send_command(
                paths,
                &words(&["sDepotDownloadFileFilter", NO_MATCH_FILTER]),
            )
            .await?;
            self.wait_for(
                &mut prime_filter_tail,
                &["sDepotDownloadFileFilter"],
                Some(30_000),
                None,
            )
            .await?;

            let mut prime_tail = ConsoleTail::new(log);
            self.send_command(
                paths,
                &[
                    "download_depot".to_owned(),
                    depot.app_id.to_string(),
                    depot.depot_id.to_string(),
                    depot.manifest_id.to_owned(),
                ],
            )
            .await?;
            self.wait_for(
                &mut prime_tail,
                &[responses::DOWNLOAD_OK, responses::DOWNLOAD_FAILED],
                Some(120_000),
                None,
            )
            .await?;

            let mut filter_tail = ConsoleTail::new(log);
            self.send_command(
                paths,
                &words(&[
                    "sDepotDownloadFileFilter",
                    depot.file_filter.unwrap_or(NO_MATCH_FILTER),
                ]),
            )
            .await?;
            self.wait_for(
                &mut filter_tail,
                &["sDepotDownloadFileFilter"],
                Some(30_000),
                None,
            )
            .await?;

            let mut tail = ConsoleTail::new(log);

            self.send_command(
                paths,
                &[
                    "download_depot".to_owned(),
                    slot.number.to_string(),
                    depot.depot_id.to_string(),
                    depot.manifest_id.to_owned(),
                ],
            )
            .await?;

            let depot_dir = self
                .path
                .join(&[target_dir, &format!("depot_{}", depot.depot_id)]);

            let mut poll = || {
                let Some(on_progress) = on_progress else {
                    return;
                };
                on_progress(&DownloadProgress {
                    depot_id: depot.depot_id,
                    index,
                    total: entry.depots.len(),
                    files: self.count_files(&depot_dir),
                    expected_files: depot.expected_files.map(u64::from).filter(|n| *n != 0),
                });
            };
            let found = self
                .wait_for(
                    &mut tail,
                    &[responses::DOWNLOAD_OK, responses::DOWNLOAD_FAILED],
                    None,
                    Some(&mut poll),
                )
                .await?;

            if found.needle == responses::DOWNLOAD_FAILED {
                let foreign = !entry.requires_app_ids.contains(&depot.app_id);
                return Err(DeliveryError::new(
                    Code::SteamDepotFailed,
                    format!(
                        "Steam не смог скачать депот {}{}",
                        depot.depot_id,
                        if foreign {
                            format!(
                                ". Он принадлежит приложению {} — проверьте, что оно есть в библиотеке Steam (Source SDK Base 2013 Multiplayer ставится бесплатно)",
                                depot.app_id
                            )
                        } else {
                            String::new()
                        }
                    ),
                )
                .with_details(json!({
                    "depotId": depot.depot_id,
                    "manifestId": depot.manifest_id,
                    "appId": depot.app_id,
                }))
                .into());
            }

            let files = self.count_files(&depot_dir);
            let floor = match depot.expected_files {
                Some(expected) if expected != 0 => 1.max(expected / 2),
                _ => 1,
            };

            if files < u64::from(floor) {
                return Err(DeliveryError::new(
                    Code::SteamDepotFailed,
                    format!(
                        "Депот {} скачан не полностью: файлов {files}, ожидалось около {}",
                        depot.depot_id,
                        match depot.expected_files {
                            Some(expected) if expected != 0 => expected.to_string(),
                            _ => "нескольких".to_owned(),
                        }
                    ),
                )
                .with_details(json!({
                    "depotId": depot.depot_id,
                    "manifestId": depot.manifest_id,
                    "files": files,
                }))
                .into());
            }

            self.logger.info(&format!(
                "[steam-depot] депот {} скачан в {depot_dir}: файлов {files}",
                depot.depot_id
            ));
        }

        Ok(Downloaded {
            build_id: build_id.to_owned(),
            target_dir: target_dir.to_owned(),
            depots: entry
                .depots
                .iter()
                .map(|depot| {
                    (
                        depot.depot_id,
                        self.path
                            .join(&[target_dir, &format!("depot_{}", depot.depot_id)]),
                    )
                })
                .collect(),
        })
    }

    /// `countFiles(dir)`: файлы под `dir` без ссылок; нет папки — 0.
    pub fn count_files(&self, dir: &str) -> u64 {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        let mut total = 0;
        for item in entries.flatten() {
            let Ok(kind) = item.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                total += self.count_files(&item.path().to_string_lossy());
            } else {
                total += 1;
            }
        }
        total
    }
}

/// `/^app_\d+$/`.
fn is_slot_name(name: &str) -> bool {
    name.strip_prefix("app_")
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

/// Цель ссылки так, как её отдаёт `readlinkSync` Node: без префикса `\\?\` и `\??\`.
fn link_target_text(target: &Path) -> String {
    let text = target.to_string_lossy().into_owned();
    for prefix in ["\\\\?\\", "\\??\\"] {
        if let Some(stripped) = text.strip_prefix(prefix) {
            return stripped.to_owned();
        }
    }
    text
}

#[cfg(unix)]
fn symlink_dir(target: &str, slot: &str) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, slot)
}

#[cfg(not(unix))]
fn symlink_dir(_target: &str, _slot: &str) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "симлинки каталогов для Linux недоступны на этой ОС",
    ))
}

/// `symlinkSync(target, slot, "junction")`: каталог `slot` с точкой повторного анализа
/// `IO_REPARSE_TAG_MOUNT_POINT` на абсолютный `target`.
/// shortcut: копия `hlsr_update::windows::file_ops::create_junction` (платформа от `hlsr-update` не
/// зависит); вынести в `hlsr-core`, когда понадобится третьему крейту.
#[cfg(windows)]
fn create_junction(target: &str, slot: &str) -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_REPARSE_POINT;
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;

    let absolute = std::path::absolute(target)?;
    let mut plain = absolute.to_string_lossy().into_owned();
    if let Some(rest) = plain.strip_prefix(r"\\?\") {
        plain = rest.to_owned();
    }
    let substitute: Vec<u16> = format!(r"\??\{plain}").encode_utf16().collect();
    let print: Vec<u16> = plain.encode_utf16().collect();
    // REPARSE_DATA_BUFFER (MountPointReparseBuffer): заголовок 8 байт, 4 смещения/длины по u16,
    // затем обе строки с нулями на конце.
    let sub_bytes = substitute.len() * 2;
    let print_bytes = print.len() * 2;
    let data_len = 8 + sub_bytes + 2 + print_bytes + 2;
    let mut buffer: Vec<u8> = Vec::with_capacity(8 + data_len);
    buffer.extend_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
    buffer.extend_from_slice(&(data_len as u16).to_le_bytes());
    buffer.extend_from_slice(&0u16.to_le_bytes());
    buffer.extend_from_slice(&0u16.to_le_bytes());
    buffer.extend_from_slice(&(sub_bytes as u16).to_le_bytes());
    buffer.extend_from_slice(&((sub_bytes + 2) as u16).to_le_bytes());
    buffer.extend_from_slice(&(print_bytes as u16).to_le_bytes());
    for unit in substitute.iter().chain(&[0]).chain(&print).chain(&[0]) {
        buffer.extend_from_slice(&unit.to_le_bytes());
    }

    std::fs::create_dir(slot)?;
    let wide = hlsr_core::win32::wide(Path::new(slot));
    // SAFETY: строка пути завершена нулём; дескриптор закрывается ниже.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let error = std::io::Error::last_os_error();
        let _ = std::fs::remove_dir(slot);
        return Err(error);
    }
    let mut returned = 0u32;
    // SAFETY: буфер жив до конца вызова, длина указана верно; дескриптор открыт на запись.
    let ok = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_SET_REPARSE_POINT,
            buffer.as_ptr().cast(),
            buffer.len() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    let error = (ok == 0).then(std::io::Error::last_os_error);
    // SAFETY: дескриптор получен от CreateFileW и закрывается один раз.
    unsafe { CloseHandle(handle) };
    if let Some(error) = error {
        let _ = std::fs::remove_dir(slot);
        return Err(error);
    }
    Ok(())
}

#[cfg(not(windows))]
fn create_junction(_target: &str, _slot: &str) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "junction есть только в Windows",
    ))
}

/// Текст ошибки `symlinkSync` Node: `EEXIST: file already exists, symlink '<цель>' -> '<слот>'`.
fn symlink_error_text(error: &std::io::Error, target: &str, slot: &str) -> String {
    RawError::fs_with_dest(error, "symlink", Path::new(target), Some(Path::new(slot))).message
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// Junction без оболочки: путь с `&` (его `cmd /c` разобрал бы как вторую команду) — ссылка на
    /// цель, через неё виден файл; занятый слот — `EEXIST` для `STEAM_LINK_FAILED`.
    #[test]
    fn a_junction_that_cannot_be_made_is_steam_link_failed() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("Lib & echo x").join("Downloads");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("a.txt"), "a").unwrap();
        let slot = dir.path().join("slot & del");
        let (target, slot) = (target.to_string_lossy(), slot.to_string_lossy());

        create_junction(&target, &slot).unwrap();
        assert!(
            std::fs::symlink_metadata(&*slot)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(Path::new(&*slot).join("a.txt")).unwrap(),
            "a"
        );

        let error = create_junction(&target, &slot).unwrap_err();
        assert!(
            symlink_error_text(&error, &target, &slot).starts_with("EEXIST: "),
            "{error}"
        );
        std::fs::remove_dir(&*slot).unwrap();
        assert!(Path::new(&*target).join("a.txt").exists());
    }
}
