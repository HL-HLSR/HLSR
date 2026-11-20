//! Режим внешнего апдейтера (`launcher-update/updater-mode.js` и `updater/main.js` без окна):
//! копия `HLSRUpdater.exe` запускается с `--hlsr-updater --plan=<файл>`, пишет журнал
//! `%LOCALAPPDATA%\HLSR\logs\updater.log`, держит `%LOCALAPPDATA%\HLSR\updater.lock`, проверяет
//! план (ключи QA-канала — из `resources\QA_TRUST`), качает и ставит в `<папка копии>\work`,
//! после успеха выходит через 2,5 с, а если обновление не состоялось и окно закрыли — запускает
//! прежний лаунчер. Окно (460×280, «HLSR Updater») — задача 10: оно получает [`UpdaterEvent`] и
//! вызывает [`UpdaterMain::cancel`]/[`UpdaterMain::close`].
//!
//! Отступления: профиль Chromium «HLSR Updater» не нужен (`app.setName`/`userData` 0.4.7);
//! строки окна (`locales.js`) переезжают в локали интерфейса (задача 10), поэтому `updater:init`
//! несёт язык и версию без таблицы строк; в первой строке журнала вместо версии Electron — версия
//! HLSR. `QA_TRUST` читается только в отладочной сборке и с `dev-hooks` (подмена ключей доверия —
//! только там, Global Constraints плана); у 0.4.7 его читал и выпуск.

use super::log::UpdaterLog;
use super::paths;
use super::plan::{self, Plan, ValidateOptions};
use super::session::{SessionDeps, SessionEvent, UpdateSession};
use super::single_instance::{self, InstanceLock};
use crate::errors::{UpdCode, UpdaterError};
use crate::ports::{Clock, Logger};
use hlsr_core::json_text::json_parse;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// `FLAG`.
pub const FLAG: &str = super::launch::UPDATER_FLAG;
/// `APP_NAME` и `WINDOW_TITLE`.
pub const APP_NAME: &str = "HLSR Updater";
pub const WINDOW_TITLE: &str = "HLSR Updater";
/// `WINDOW`: 460×280.
pub const WINDOW_SIZE: (u32, u32) = (460, 280);
/// Выход после успешного обновления.
pub const QUIT_DELAY: Duration = Duration::from_millis(2500);

/// `isUpdaterInvocation(argv, platform)`: `--hlsr-updater` среди аргументов (без `argv[0]`), Windows.
pub fn is_updater_invocation(args: &[String], windows: bool) -> bool {
    windows && args.iter().any(|arg| arg == FLAG)
}

/// `planPathFromArgv(argv)`: первый `--plan=<путь>` после `argv[0]` (`/^--plan=(.+)$/`).
pub fn plan_path_from_argv(argv: &[String]) -> Option<String> {
    argv.iter().skip(1).find_map(|arg| {
        let path = arg.strip_prefix("--plan=")?;
        let line_break = |c: char| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}');
        (!path.is_empty() && !path.contains(line_break)).then(|| path.to_owned())
    })
}

/// `readPlanFile(file)`.
pub fn read_plan_file(file: Option<&str>) -> Result<Value, UpdaterError> {
    let Some(file) = file.filter(|file| !file.is_empty()) else {
        return Err(UpdaterError::new(
            UpdCode::PlanUnreadable,
            "путь к плану не передан",
        ));
    };
    let raw = std::fs::read(file).map_err(|error| {
        UpdaterError::new(
            UpdCode::PlanUnreadable,
            format!(
                "план не читается: {}",
                hlsr_core::js::errno::fs_message(&error, "open", file, None)
            ),
        )
    })?;
    json_parse(&String::from_utf8_lossy(&raw))
        .map_err(|_| UpdaterError::new(UpdCode::PlanUnreadable, "план не разбирается как JSON"))
}

/// Ключи QA-канала из `QA_TRUST`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QaTrust {
    pub public_keys: Vec<String>,
    pub download_base: Option<String>,
}

impl QaTrust {
    pub fn to_json(&self) -> Value {
        json!({"publicKeys": self.public_keys, "downloadBase": self.download_base})
    }

    pub fn validate_options(&self) -> ValidateOptions {
        ValidateOptions {
            public_keys: Some(self.public_keys.clone()),
            download_base: self.download_base.clone(),
        }
    }
}

/// `qaTrustOptions(resourcesPath)`: `{publicKeys, downloadBase}` из `<resources>\QA_TRUST` или
/// `None` (файла нет, не JSON, нет непустого списка ключей).
pub fn qa_trust_options(resources: &Path) -> Option<QaTrust> {
    let marker = resources.join(super::launch::QA_TRUST);
    let text = std::fs::read(&marker).ok()?;
    let data = json_parse(&String::from_utf8_lossy(&text)).ok()?;
    let keys = data.get("publicKeys")?.as_array()?;
    if keys.is_empty() {
        return None;
    }
    Some(QaTrust {
        public_keys: keys.iter().map(hlsr_core::js::to_string).collect(),
        download_base: data
            .get("downloadBase")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// `logsDir()`: `<LOCALAPPDATA или tmpdir>\HLSR\logs`.
pub fn logs_dir(local_app_data: Option<&Path>, temp_dir: &Path) -> PathBuf {
    lock_dir(local_app_data, temp_dir).join("logs")
}

/// `lockDir()`: `<LOCALAPPDATA или tmpdir>\HLSR`.
pub fn lock_dir(local_app_data: Option<&Path>, temp_dir: &Path) -> PathBuf {
    local_app_data
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(temp_dir)
        .join("HLSR")
}

/// Что получает окно апдейтера (`updater:init`, `updater:stage`, `updater:progress`) и когда
/// приложению выйти.
#[derive(Debug, Clone, PartialEq)]
pub enum UpdaterEvent {
    /// `{language, targetVersion}`.
    Init {
        language: String,
        target_version: String,
    },
    /// `{stage, cancellable, …}`.
    Stage(Value),
    /// `{done, total, bytesPerSecond}`.
    Progress(Value),
    /// `app.quit()`.
    Quit,
}

/// Откуда апдейтер берёт окружение (`process.argv`, `app.getPath("exe")`, переменные).
#[derive(Debug, Clone)]
pub struct UpdaterConfig {
    /// `process.argv`, `argv[0]` — exe.
    pub argv: Vec<String>,
    /// Копия `HLSRUpdater.exe`; рабочий каталог — `<её каталог>\work`.
    pub exe: PathBuf,
    /// `resources` рядом с копией (`QA_TRUST`).
    pub resources: PathBuf,
    /// `%LOCALAPPDATA%`.
    pub local_app_data: Option<PathBuf>,
    /// `os.tmpdir()`.
    pub temp_dir: PathBuf,
    pub pid: u32,
    pub app_version: String,
    /// Читать ли `QA_TRUST`; в выпуске без `dev-hooks` он не читается в любом случае.
    pub qa_trust: bool,
}

type EventSink = Arc<dyn Fn(UpdaterEvent) + Send + Sync>;

#[derive(Default)]
struct State {
    session: Option<Arc<UpdateSession>>,
    language: String,
    launcher_exe: Option<String>,
    installed: bool,
    lock: Option<InstanceLock>,
}

/// Апдейтер без окна (`updater/main.js`).
pub struct UpdaterMain {
    config: UpdaterConfig,
    deps: Arc<dyn SessionDeps>,
    log: Arc<UpdaterLog>,
    clock: Arc<dyn Clock>,
    is_alive: Arc<dyn Fn(f64) -> bool + Send + Sync>,
    on_event: EventSink,
    state: Mutex<State>,
}

impl UpdaterMain {
    /// `app.on("ready")` до окна: журнал открыт, первая строка записана.
    pub fn new(
        config: UpdaterConfig,
        deps: Arc<dyn SessionDeps>,
        clock: Arc<dyn Clock>,
        is_alive: Arc<dyn Fn(f64) -> bool + Send + Sync>,
        on_event: impl Fn(UpdaterEvent) + Send + Sync + 'static,
    ) -> Arc<UpdaterMain> {
        let log_file =
            logs_dir(config.local_app_data.as_deref(), &config.temp_dir).join("updater.log");
        let log = Arc::new(UpdaterLog::new(Some(log_file)).with_clock(clock.clone()));
        log.info(&format!(
            "[updater] запуск, версия HLSR {}",
            config.app_version
        ));
        Arc::new(UpdaterMain {
            config,
            deps,
            log,
            clock,
            is_alive,
            on_event: Arc::new(on_event),
            state: Mutex::new(State {
                language: "ru".to_owned(),
                ..State::default()
            }),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn log(&self) -> Arc<UpdaterLog> {
        self.log.clone()
    }

    fn send(&self, event: UpdaterEvent) {
        (self.on_event)(event);
    }

    fn send_error(&self, error: &UpdaterError) {
        self.send(UpdaterEvent::Stage(
            json!({"stage": "error", "error": error.to_public()}),
        ));
    }

    /// Блокировка, затем [`Self::start`]; занято — окно сразу показывает `UPD_ALREADY_RUNNING`.
    pub async fn run(self: &Arc<Self>) {
        let dir = lock_dir(self.config.local_app_data.as_deref(), &self.config.temp_dir);
        match single_instance::acquire(
            &dir,
            self.config.pid,
            self.clock.now_ms(),
            self.is_alive.as_ref(),
        ) {
            Ok(lock) => self.lock().lock = Some(lock),
            Err(error) => {
                self.log
                    .warn(&format!("[updater] {}: {}", error.code, error.message));
                let language = self.lock().language.clone();
                self.send(UpdaterEvent::Init {
                    language,
                    target_version: String::new(),
                });
                self.send_error(&error);
                return;
            }
        }
        if let Err(error) = self.start().await {
            self.log.error(&format!(
                "[updater] непредвиденная ошибка: {}",
                error.message
            ));
            self.send_error(&error);
        }
    }

    /// `start()`: план, рабочий каталог, сессия.
    async fn start(self: &Arc<Self>) -> Result<(), UpdaterError> {
        let plan = match self.read_and_validate() {
            Ok(plan) => plan,
            Err(error) => {
                self.log.error(&format!(
                    "[updater] план отклонён: {} {}",
                    error.code, error.message
                ));
                self.send_error(&error);
                return Ok(());
            }
        };
        self.log.info(&format!(
            "[updater] план принят: {} -> {}",
            hlsr_core::js::to_template_string(plan.current_version.as_ref()),
            plan.target_version
        ));
        self.lock().launcher_exe = Some(plan.launcher_exe.clone());

        let work_dir = self
            .config
            .exe
            .parent()
            .map_or_else(|| PathBuf::from("work"), |dir| dir.join("work"));
        let work = work_dir.to_string_lossy().into_owned();
        if let Err(error) = paths::assert_safe_work_dir(
            &work,
            &self.config.temp_dir.to_string_lossy(),
            Some(&plan.install_dir),
        ) {
            self.log
                .error(&format!("[updater] {}: {}", error.code, error.message));
            self.send_error(&error);
            return Ok(());
        }
        let installer = work_dir.join(&plan.installer.filename);
        paths::assert_writable(
            &installer.to_string_lossy(),
            &[&work],
            &[&plan.receipt_file],
        )?;

        let events = self.on_event.clone();
        let session = Arc::new(UpdateSession::new(
            plan.clone(),
            work_dir,
            self.log.clone(),
            self.deps.clone(),
            move |event| match event {
                SessionEvent::Stage(payload) => events(UpdaterEvent::Stage(payload)),
                SessionEvent::Progress(payload) => events(UpdaterEvent::Progress(payload)),
                _ => {}
            },
        ));
        let language = {
            let mut state = self.lock();
            state.session = Some(session.clone());
            state.language.clone()
        };
        self.send(UpdaterEvent::Init {
            language,
            target_version: plan.target_version.clone(),
        });

        let result = session.run().await;
        if result.is_ok() {
            self.lock().installed = true;
            self.log
                .info("[updater] обновление завершено, лаунчер запущен");
            let events = self.on_event.clone();
            tokio::spawn(async move {
                tokio::time::sleep(QUIT_DELAY).await;
                events(UpdaterEvent::Quit);
            });
        }
        Ok(())
    }

    fn read_and_validate(&self) -> Result<Plan, UpdaterError> {
        let raw = read_plan_file(plan_path_from_argv(&self.config.argv).as_deref())?;
        let language = if raw.get("language").and_then(Value::as_str) == Some("en") {
            "en"
        } else {
            "ru"
        };
        self.lock().language = language.to_owned();
        self.send(UpdaterEvent::Init {
            language: language.to_owned(),
            target_version: raw
                .get("targetVersion")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        });
        let qa = (cfg!(any(debug_assertions, feature = "dev-hooks")) && self.config.qa_trust)
            .then(|| qa_trust_options(&self.config.resources))
            .flatten();
        if qa.is_some() {
            self.log
                .warn("[updater] ВКЛЮЧЕНО QA-доверие (файл QA_TRUST): это не боевой режим");
        }
        let options = qa.map(|qa| qa.validate_options()).unwrap_or_default();
        plan::validate(&raw, &options)
    }

    /// `allowClose()`: окно можно закрыть, если сессии нет, она кончилась или ещё отменяема.
    pub fn allow_close(&self) -> bool {
        match &self.lock().session {
            None => true,
            Some(session) => session.finished() || session.is_cancellable(),
        }
    }

    /// `updater:cancel`.
    pub fn cancel(&self) {
        let Some(session) = self.lock().session.clone() else {
            return;
        };
        let accepted = session.cancel();
        self.log.info(&format!(
            "[updater] отмена {}",
            if accepted {
                "принята"
            } else {
                "отклонена (поздно)"
            }
        ));
    }

    /// `updater:close`: прежний лаунчер, если обновление не состоялось, затем выход.
    pub fn close(&self) {
        self.relaunch_launcher_if_needed();
        self.send(UpdaterEvent::Quit);
    }

    /// `relaunchLauncherIfNeeded()`.
    pub fn relaunch_launcher_if_needed(&self) {
        let (installed, exe) = {
            let state = self.lock();
            (state.installed, state.launcher_exe.clone())
        };
        let Some(exe) = exe.filter(|_| !installed) else {
            return;
        };
        match self.deps.relaunch(&exe) {
            Ok(()) => self
                .log
                .info("[updater] обновление не состоялось — запущена прежняя версия"),
            Err(error) => self.log.warn(&format!(
                "[updater] прежнюю версию запустить не удалось: {}",
                error.message
            )),
        }
    }

    /// `will-quit`: блокировка снимается.
    pub fn will_quit(&self) {
        if let Some(lock) = self.lock().lock.take() {
            lock.release();
        }
    }
}
