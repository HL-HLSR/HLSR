//! Окно установщика без интерфейса (`launcher-update/installer-window.js`): состояние окна (формы
//! карты §4.2), правило «одна долгая операция за раз», отмена, закрытие во время работы. Что делают
//! «Установить», «Открыть», «Удалить» и «Назад», решает [`InstallerBackend`] (Windows —
//! `windows::setup`, Linux — установщик AppImage). Экран Slint (задача 10) только вызывает методы
//! [`InstallerWindow`] вместо IPC 0.4.7 и показывает состояние из слушателя.
//!
//! Методы, которые выполняют долгую работу, синхронны: окно вызывает их не из потока интерфейса.

use crate::cancel::CancelToken;
use hlsr_core::js::{self, Platform};
use hlsr_core::logger::Logger;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, MutexGuard};

/// Код ошибки ОС (`ENOSPC`, `EACCES`…, `UNKNOWN`) у причины или у самой ошибки (`systemCode`): окно
/// объясняет его своими словами, сырой текст остаётся в журнале.
fn system_code(error: &BackendError) -> Option<String> {
    let code = error
        .cause_code
        .as_deref()
        .filter(|code| !code.is_empty())
        .or(error.code.as_deref())
        .unwrap_or_default();
    let os = code.len() > 1
        && code.starts_with('E')
        && code[1..].bytes().all(|byte| byte.is_ascii_uppercase());
    (os || code == "UNKNOWN").then(|| code.to_owned())
}

/// Фаза окна (`phase`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Verifying,
    Installing,
    Done,
    Error,
    Cancelling,
    Cancelled,
    UninstallConfirm,
    Uninstalling,
    Uninstalled,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Verifying => "verifying",
            Phase::Installing => "installing",
            Phase::Done => "done",
            Phase::Error => "error",
            Phase::Cancelling => "cancelling",
            Phase::Cancelled => "cancelled",
            Phase::UninstallConfirm => "uninstall-confirm",
            Phase::Uninstalling => "uninstalling",
            Phase::Uninstalled => "uninstalled",
        }
    }
}

/// План удаления для диалога (`installer-uninstall-plan`): размеры каталога программы и
/// библиотеки. `desktopEntry` — только у Linux; `libraryRoots` и `libraryBlocked` — только у Windows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UninstallPlan {
    pub managed: bool,
    pub install_root: String,
    pub install_bytes: u64,
    pub library_root: Option<String>,
    pub library_exists: bool,
    pub library_bytes: u64,
    pub desktop_entry: Option<String>,
    /// Все папки, которые очистит полное удаление.
    pub library_roots: Option<Vec<String>>,
    /// Почему переключателя игр нет: `other`, `test`, `unverified`.
    pub library_blocked: Option<String>,
}

impl UninstallPlan {
    pub fn to_value(&self) -> Value {
        let mut out = json!({
            "managed": self.managed, "installRoot": self.install_root, "installBytes": self.install_bytes,
            "libraryRoot": self.library_root, "libraryExists": self.library_exists,
            "libraryBytes": self.library_bytes,
        });
        if let Some(entry) = &self.desktop_entry {
            out["desktopEntry"] = json!(entry);
        }
        if let Some(roots) = &self.library_roots {
            out["libraryRoots"] = json!(roots);
            out["libraryBlocked"] = json!(self.library_blocked);
        }
        out
    }
}

/// Состояние окна (`installer-state`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallerState {
    pub phase: Phase,
    pub directory: String,
    /// `ru`/`en`.
    pub language: String,
    pub can_uninstall: bool,
    pub can_choose_directory: bool,
    pub cancellable: bool,
    pub version: Option<String>,
    pub installed_version: Option<String>,
    pub update: bool,
    pub error: Option<String>,
    pub warning: Option<String>,
    /// Проценты установки (Windows); `None` — не показывать.
    pub progress: Option<u32>,
    pub uninstall_plan: Option<UninstallPlan>,
    /// `canGoBack`: `None` — не задано (в 0.4.7 запрещает только `false`).
    pub can_go_back: Option<bool>,
    /// `process.platform` (`win32` — перед установкой шаг про SmartScreen); пустое — ОС сборки.
    pub platform: String,
    /// Код ошибки, которую окно объясняет своим текстом (`ANTIVIRUS`, коды ОС, коды установки).
    pub error_code: Option<String>,
    /// Файл, на котором остановилась установка (`ANTIVIRUS`).
    pub error_file: Option<String>,
}

impl InstallerState {
    pub fn to_value(&self) -> Value {
        json!({
            "platform": self.platform,
            "phase": self.phase.as_str(), "directory": self.directory, "language": self.language,
            "canUninstall": self.can_uninstall, "canChooseDirectory": self.can_choose_directory,
            "cancellable": self.cancellable, "version": self.version,
            "installedVersion": self.installed_version, "update": self.update, "error": self.error,
            "warning": self.warning, "progress": self.progress,
            "uninstallPlan": self.uninstall_plan.as_ref().map(UninstallPlan::to_value),
            "canGoBack": self.can_go_back,
            "errorCode": self.error_code, "errorFile": self.error_file,
        })
    }
}

/// Изменение состояния (`send(patch)`): заданные поля заменяются, прочие остаются.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatePatch {
    pub phase: Option<Phase>,
    pub directory: Option<String>,
    pub language: Option<String>,
    pub can_uninstall: Option<bool>,
    pub cancellable: Option<bool>,
    pub version: Option<Option<String>>,
    pub installed_version: Option<Option<String>>,
    pub update: Option<bool>,
    pub error: Option<Option<String>>,
    pub warning: Option<Option<String>>,
    pub progress: Option<Option<u32>>,
    pub uninstall_plan: Option<Option<UninstallPlan>>,
    pub can_go_back: Option<Option<bool>>,
    pub error_code: Option<Option<String>>,
    pub error_file: Option<Option<String>>,
}

impl StatePatch {
    pub fn phase(phase: Phase) -> StatePatch {
        StatePatch {
            phase: Some(phase),
            ..StatePatch::default()
        }
    }

    pub fn with_error(mut self, error: Option<String>) -> StatePatch {
        self.error = Some(error);
        self
    }

    pub fn with_warning(mut self, warning: Option<String>) -> StatePatch {
        self.warning = Some(warning);
        self
    }

    pub fn with_progress(mut self, progress: Option<u32>) -> StatePatch {
        self.progress = Some(progress);
        self
    }

    pub fn with_cancellable(mut self, cancellable: bool) -> StatePatch {
        self.cancellable = Some(cancellable);
        self
    }

    pub fn apply(&self, state: &mut InstallerState) {
        // Новая ошибка без кода не наследует код прежней (ENOSPC, ANTIVIRUS…).
        if matches!(&self.error, Some(Some(error)) if !error.is_empty())
            && self.error_code.is_none()
        {
            state.error_code = None;
            state.error_file = None;
        }
        macro_rules! set {
            ($($field:ident),*) => {$(
                if let Some(value) = &self.$field {
                    state.$field = value.clone();
                }
            )*};
        }
        set!(
            phase,
            directory,
            language,
            can_uninstall,
            cancellable,
            version,
            installed_version,
            update,
            error,
            warning,
            progress,
            uninstall_plan,
            can_go_back,
            error_code,
            error_file
        );
    }
}

/// Ошибка действия окна (исключение метода backend в 0.4.7): текст для окна; код и файл, по
/// которым окно объясняет её своими словами (0.4.9); исходная ошибка ОС (`error.cause`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackendError {
    pub message: String,
    pub code: Option<String>,
    pub file: Option<String>,
    pub cause_code: Option<String>,
    /// Текст причины — только в журнал.
    pub cause_message: Option<String>,
}

impl From<String> for BackendError {
    fn from(message: String) -> BackendError {
        BackendError {
            message,
            ..BackendError::default()
        }
    }
}

impl From<&str> for BackendError {
    fn from(message: &str) -> BackendError {
        BackendError::from(message.to_owned())
    }
}

/// `ctx` методов backend: изменения состояния, сигнал отмены, признак закрытого окна, `commit`.
pub struct Ctx<'a> {
    window: &'a InstallerWindow,
    cancel: Option<CancelToken>,
}

impl Ctx<'_> {
    /// `send(patch)`.
    pub fn send(&self, patch: StatePatch) {
        self.window.send(patch);
    }

    /// `signal`: есть только во время долгой операции.
    pub fn cancel_token(&self) -> Option<&CancelToken> {
        self.cancel.as_ref()
    }

    /// Окно уже закрыто.
    pub fn closed(&self) -> bool {
        self.window.lock().closed
    }

    pub fn state(&self) -> InstallerState {
        self.window.state()
    }

    /// `commit()`: дальше операция не отменяется (`cancellable: false`).
    pub fn commit(&self) {
        self.send(StatePatch::default().with_cancellable(false));
    }
}

/// Действия окна. Ошибка — текст для окна (в 0.4.7 исключение метода).
pub trait InstallerBackend: Send + Sync {
    /// Есть ли выбор каталога (`backend.selectDirectory`).
    fn can_select_directory(&self) -> bool {
        false
    }
    fn select_directory(&self, directory: &str, ctx: &Ctx<'_>) -> Result<(), String> {
        let _ = (directory, ctx);
        Ok(())
    }
    fn install(&self, desktop_shortcut: bool, ctx: &Ctx<'_>) -> Result<bool, BackendError>;
    fn launch(&self, ctx: &Ctx<'_>) -> Result<bool, String>;
    /// План удаления; `None` — `false` 0.4.7 (диалога нет).
    fn uninstall_plan(&self, ctx: &Ctx<'_>) -> Option<UninstallPlan>;
    fn uninstall(&self, remove_games: bool, ctx: &Ctx<'_>) -> Result<bool, String>;
    /// Есть ли свой «Назад» (`backend.back`).
    fn has_back(&self) -> bool {
        false
    }
    fn back(&self, ctx: &Ctx<'_>) -> bool {
        let _ = ctx;
        false
    }
}

type Listener = Box<dyn Fn(&InstallerState) + Send + Sync>;
type Destroy = Box<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Inner {
    state: InstallerState,
    busy: bool,
    controller: Option<CancelToken>,
    closed: bool,
    closing: bool,
    choosing_directory: bool,
}

/// Команды заголовка окна (`installer-window`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowCommand {
    Minimize,
    Close,
}

/// Окно установщика: состояние и правила занятости (`openInstallerWindow`).
pub struct InstallerWindow {
    backend: Arc<dyn InstallerBackend>,
    inner: Mutex<Inner>,
    listener: Mutex<Option<Listener>>,
    destroy: Mutex<Option<Destroy>>,
    log: Option<Arc<dyn Logger>>,
}

impl InstallerWindow {
    /// Окно с начальным состоянием: `platform` — ОС сборки, если не задана; `canChooseDirectory` —
    /// по backend, `cancellable` — нет.
    pub fn new(backend: Arc<dyn InstallerBackend>, state: InstallerState) -> InstallerWindow {
        let mut state = state;
        if state.platform.is_empty() {
            state.platform = Platform::CURRENT.as_str().to_owned();
        }
        state.can_choose_directory = backend.can_select_directory();
        state.cancellable = false;
        InstallerWindow {
            backend,
            inner: Mutex::new(Inner {
                state,
                ..Inner::default()
            }),
            listener: Mutex::new(None),
            destroy: Mutex::new(None),
            log: None,
        }
    }

    /// Журнал ошибок операций (`options.log`): сырой текст и причина ОС.
    pub fn with_log(mut self, log: Arc<dyn Logger>) -> InstallerWindow {
        self.log = Some(log);
        self
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Слушатель `installer-state`.
    pub fn set_listener(&self, listener: impl Fn(&InstallerState) + Send + Sync + 'static) {
        *self
            .listener
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(listener));
    }

    /// `window.destroy()`: окно закрывается по завершении операции, во время которой его закрыли.
    pub fn set_on_destroy(&self, destroy: impl Fn() + Send + Sync + 'static) {
        *self
            .destroy
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(destroy));
    }

    /// `installer-get-state`.
    pub fn state(&self) -> InstallerState {
        self.lock().state.clone()
    }

    fn send(&self, patch: StatePatch) {
        let (state, closed) = {
            let mut inner = self.lock();
            patch.apply(&mut inner.state);
            (inner.state.clone(), inner.closed)
        };
        if !closed
            && let Some(listener) = self
                .listener
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_ref()
        {
            listener(&state);
        }
    }

    fn ctx(&self) -> Ctx<'_> {
        Ctx {
            window: self,
            cancel: self.lock().controller.clone(),
        }
    }

    fn destroy_now(&self) {
        if let Some(destroy) = self
            .destroy
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
        {
            destroy();
        }
        self.closed();
    }

    /// Долгие операции: одна за раз; закрытие окна во время работы ждёт её конца (`exclusive`).
    fn exclusive(
        &self,
        cancellable: bool,
        run: impl FnOnce(&Ctx<'_>) -> Result<bool, BackendError>,
    ) -> bool {
        let token = {
            let mut inner = self.lock();
            if inner.busy || inner.choosing_directory || inner.closing {
                return false;
            }
            inner.busy = true;
            let token = CancelToken::new();
            inner.controller = Some(token.clone());
            token
        };
        self.send(StatePatch {
            cancellable: Some(cancellable),
            error: Some(None),
            warning: Some(None),
            error_code: Some(None),
            error_file: Some(None),
            ..StatePatch::default()
        });
        let ctx = Ctx {
            window: self,
            cancel: Some(token.clone()),
        };
        let result = match run(&ctx) {
            Ok(value) => value,
            Err(_) if token.is_cancelled() => {
                self.send(StatePatch {
                    phase: Some(Phase::Cancelled),
                    error: Some(None),
                    progress: Some(None),
                    ..StatePatch::default()
                });
                false
            }
            Err(error) => {
                if let Some(log) = &self.log {
                    let cause = error
                        .cause_message
                        .as_deref()
                        .map(|cause| format!("\n{cause}"))
                        .unwrap_or_default();
                    log.error(&format!("[installer] {}{cause}", error.message));
                }
                // ANTIVIRUS и коды ОС окно объясняет своим текстом.
                let code = if error.code.as_deref() == Some("ANTIVIRUS") {
                    error.code.clone()
                } else {
                    system_code(&error).or_else(|| error.code.clone())
                };
                self.send(StatePatch {
                    phase: Some(Phase::Error),
                    error: Some(Some(js::slice_utf16(&error.message, 400))),
                    error_code: Some(code),
                    error_file: Some(error.file),
                    ..StatePatch::default()
                });
                false
            }
        };
        let destroy = {
            let mut inner = self.lock();
            inner.busy = false;
            inner.controller = None;
            inner.closing && !inner.closed
        };
        self.send(StatePatch::default().with_cancellable(false));
        if destroy {
            self.destroy_now();
        }
        result
    }

    /// `installer-install`: не из фазы `done`; отменяемая операция.
    pub fn install(&self, desktop_shortcut: bool) -> bool {
        if self.lock().state.phase == Phase::Done {
            return false;
        }
        let backend = self.backend.clone();
        self.exclusive(true, |ctx| backend.install(desktop_shortcut, ctx))
    }

    /// `installer-cancel`: только во время работы и пока `cancellable`.
    pub fn cancel(&self) -> bool {
        let token = {
            let inner = self.lock();
            if !inner.busy || !inner.state.cancellable {
                return false;
            }
            match &inner.controller {
                Some(token) => token.clone(),
                None => return false,
            }
        };
        self.send(StatePatch {
            phase: Some(Phase::Cancelling),
            cancellable: Some(false),
            ..StatePatch::default()
        });
        token.cancel();
        true
    }

    /// `installer-select-directory`: `pick` — диалог выбора каталога (начальный каталог → выбор или
    /// `None`). Ответ — состояние после выбора или `None` (ничего не выбрано, окно занято).
    pub fn select_directory(
        &self,
        pick: impl FnOnce(&str) -> Option<String>,
    ) -> Result<Option<InstallerState>, String> {
        let start = {
            let mut inner = self.lock();
            if inner.busy
                || inner.choosing_directory
                || inner.closing
                || !self.backend.can_select_directory()
            {
                return Ok(None);
            }
            inner.choosing_directory = true;
            inner.state.directory.clone()
        };
        let finish = |this: &InstallerWindow| this.lock().choosing_directory = false;
        let Some(chosen) = pick(&start) else {
            finish(self);
            return Ok(None);
        };
        if self.lock().closed {
            finish(self);
            return Ok(None);
        }
        let result = self.backend.select_directory(&chosen, &self.ctx());
        finish(self);
        result.map(|()| Some(self.state()))
    }

    /// `installer-launch`: только из фазы `done` и не во время работы.
    pub fn launch(&self) -> bool {
        {
            let inner = self.lock();
            if inner.state.phase != Phase::Done || inner.busy {
                return false;
            }
        }
        match self.backend.launch(&self.ctx()) {
            Ok(launched) => launched,
            Err(message) => {
                self.send(
                    StatePatch::phase(Phase::Done).with_error(Some(js::slice_utf16(&message, 400))),
                );
                false
            }
        }
    }

    /// `installer-uninstall-plan`.
    pub fn uninstall_plan(&self) -> Option<UninstallPlan> {
        if self.lock().busy {
            return None;
        }
        self.backend.uninstall_plan(&self.ctx())
    }

    /// `installer-back`: только из `uninstalled`; свой «Назад» backend, иначе тот же каталог
    /// проверяется заново, как при выборе папки, иначе окно возвращается к установке.
    pub fn back(&self) -> bool {
        let directory = {
            let inner = self.lock();
            if inner.busy
                || inner.closing
                || inner.state.phase != Phase::Uninstalled
                || inner.state.can_go_back == Some(false)
            {
                return false;
            }
            inner.state.directory.clone()
        };
        if self.backend.has_back() {
            return self.backend.back(&self.ctx());
        }
        if self.backend.can_select_directory()
            && self
                .backend
                .select_directory(&directory, &self.ctx())
                .is_ok()
        {
            return true;
        }
        self.send(StatePatch {
            phase: Some(Phase::Idle),
            update: Some(false),
            installed_version: Some(None),
            can_uninstall: Some(false),
            error: Some(None),
            warning: Some(None),
            progress: Some(None),
            ..StatePatch::default()
        });
        true
    }

    /// `installer-uninstall`: только если `canUninstall`; не отменяется.
    pub fn uninstall(&self, remove_games: bool) -> bool {
        if !self.lock().state.can_uninstall {
            return false;
        }
        let backend = self.backend.clone();
        self.exclusive(false, |ctx| {
            backend
                .uninstall(remove_games, ctx)
                .map_err(BackendError::from)
        })
    }

    /// `installer-open-discord`: помощь при ошибке установки, только в фазе `error`. `open` открывает
    /// Discord HLSR по своей ссылке (`LINKS.discord`): страница может только попросить, но не
    /// передать адрес.
    pub fn open_discord(&self, open: impl FnOnce()) -> bool {
        if self.lock().state.phase != Phase::Error {
            return false;
        }
        open();
        true
    }

    /// Закрыть окно (кнопка, Alt+F4). Во время работы окно остаётся до её конца, отменяемая работа
    /// отменяется; `true` — закрыть можно сейчас.
    pub fn request_close(&self) -> bool {
        let token = {
            let mut inner = self.lock();
            if !inner.busy {
                drop(inner);
                self.destroy_now();
                return true;
            }
            inner.closing = true;
            match &inner.controller {
                Some(token) if inner.state.cancellable => Some(token.clone()),
                _ => None,
            }
        };
        if let Some(token) = token {
            self.send(StatePatch {
                phase: Some(Phase::Cancelling),
                cancellable: Some(false),
                ..StatePatch::default()
            });
            token.cancel();
        }
        false
    }

    /// Окно закрыто (`closed`): изменения больше не показываются, работа отменяется.
    pub fn closed(&self) {
        let token = {
            let mut inner = self.lock();
            inner.closed = true;
            inner.controller.clone()
        };
        if let Some(token) = token {
            token.cancel();
        }
    }

    pub fn is_closed(&self) -> bool {
        self.lock().closed
    }
}
