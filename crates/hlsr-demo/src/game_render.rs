//! Общее у адаптеров, которые рендерят настоящую игру на скрытом рабочем столе через помощник
//! ([`crate::render_host`]): Ghosting (DM4) и HL1 (DM5), `game-render.js` 0.4.9.
//!
//! - Реестр игры: снимок до первого запуска, возврат после. Снимок с маркером в корне заданий
//!   живёт, пока не возвращён (после сбоя — следующий старт лаунчера или следующий рендер).
//!   Формат снимка — файл `.reg`, как у `reg.exe export`: маркер и снимок, оставленные упавшим
//!   0.4.9, возвращаются так же. Чтение и запись реестра — Win32, без `reg.exe`.
//! - Звук: игра всегда закрывается заглушённой, mute, запомненный Windows, снимает беззвучный
//!   перезапуск в конце рендера; не снят — флаг в Store, следующий рендер снимет его со всех
//!   сессий, заглушённых до него.
//! - Предусловия: игра не запущена и не занята, Steam.
//! - Папка рендера внутри игры и выход лаунчера посреди рендера.
//!
//! Что у каждой игры своё — [`Spec`]. Рендер идёт через [`GameRender::render`]: он сам
//! останавливает оставшийся помощник и вызывает [`GameRender::finish`] — адаптеру этого делать
//! не нужно.

mod reg;

pub use reg::WinRegistry;

use crate::errors::DemoError;
use crate::jobs;
use crate::planner::{Capabilities, RESOLUTIONS};
use crate::render_host::{Host, HostEvent, StartRequest};
use hlsr_core::js::errno;
use hlsr_core::js::iso::iso_from_ms;
use hlsr_core::js::to_template_string;
use hlsr_core::json_text::stringify;
use hlsr_core::logger::Logger;
use hlsr_core::store::Store;
use hlsr_delivery::paths::remove_within_zone;
use serde_json::{Map, Value};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Частоты кадров рендера игр.
pub const FPS: [u32; 3] = [30, 60, 120];
/// Выход лаунчера посреди рендера: сколько ждать, пока рендер закроет игру и вернёт своё.
const DISPOSE_WAIT: Duration = Duration::from_secs(20);

/// Что у игры своё (`spec`).
#[derive(Debug, Clone)]
pub struct Spec {
    pub game_id: &'static str,
    /// Образ процесса игры (`hl2.exe`, `hl.exe`).
    pub image: &'static str,
    /// Ключ настроек игры в реестре, `HKCU\…`.
    pub registry_key: &'static str,
    /// Маркер снимка в корне заданий.
    pub marker: &'static str,
    /// Снимок ключа (`.reg`) в корне заданий.
    pub reg_file: &'static str,
    /// Флаг Store «mute после рендера не снят».
    pub unmute_pending: &'static str,
    pub label: &'static str,
    /// Код для любого другого процесса с тем же образом (HL1: общий ключ реестра движка).
    pub other_running: Option<&'static str>,
    /// Что должно быть в папке установки (пути от неё).
    pub required: &'static [&'static str],
    /// Папка рендера внутри игры.
    pub render_root: fn(&Path) -> PathBuf,
    /// Что умеет адаптер (`caps`): имена включённых возможностей по порядку.
    pub caps: &'static [&'static str],
}

/// Реестр игрока (`regTool`): ключи вида `HKCU\…`. «Ключа нет» — только наверняка; иначе
/// `REGISTRY_FAILED`.
pub trait Registry: Send + Sync {
    fn exists(&self, key: &str) -> Result<bool, DemoError>;
    /// Снимок ключа со всеми подключами в файл `.reg`.
    fn export_key(&self, key: &str, file: &Path) -> Result<(), DemoError>;
    /// Удалить ключ целиком; ключа нет — не ошибка.
    fn delete_key(&self, key: &str) -> Result<(), DemoError>;
    /// Вернуть снимок `.reg`. Строже 0.4.9 (`reg import` брал из файла всё): только ключ `key`
    /// и его подключи.
    fn import_file(&self, key: &str, file: &Path) -> Result<(), DemoError>;
    /// Снимок читается и разбирается до удаления ключа: битый снимок не оставит игрока без
    /// ключа (строже 0.4.9).
    fn check_file(&self, _key: &str, _file: &Path) -> Result<(), DemoError> {
        Ok(())
    }
}

/// Процессы системы (`process-inspect`).
pub trait Inspector: Send + Sync {
    /// Путь exe каждого процесса с образом `image` (`None` — путь не прочитан); `Err` — список
    /// не получен.
    fn find_by_image_name(&self, image: &str) -> Result<Vec<Option<String>>, String>;
    fn same_path(&self, left: &str, right: &str) -> bool;
}

/// `steamStatus()`: `ok` или код.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SteamStatus {
    pub ok: bool,
    pub code: Option<String>,
}

type Callback<T> = Box<dyn Fn() -> T + Send + Sync>;

/// Связи с лаунчером (`options` конструктора).
pub struct Ports {
    /// Рендер есть только на Windows.
    pub windows: bool,
    /// Корень заданий: маркер и снимок реестра живут в нём.
    pub jobs_root: Option<PathBuf>,
    /// Store `demo-tool.json`: флаги `*UnmutePending`.
    pub store: Option<Arc<Mutex<Store>>>,
    pub logger: Arc<dyn Logger>,
    pub registry: Box<dyn Registry>,
    pub inspector: Box<dyn Inspector>,
    /// Путь установки из библиотеки лаунчера (`None` — не установлена).
    pub install_path: Callback<Option<PathBuf>>,
    /// Игра с этим id запущена из лаунчера.
    pub game_running: Box<dyn Fn(&str) -> bool + Send + Sync>,
    /// Лаунчер занят игрой (установка, обновление).
    pub game_busy: Callback<bool>,
    pub steam_status: Callback<SteamStatus>,
    /// Основной монитор в DIP (`None` — не узнать).
    pub screen_size: Callback<Option<(u32, u32)>>,
    /// Новый помощник рендера.
    pub host_factory: Callback<Box<dyn Host>>,
}

/// Беззвучный запуск игры для сброса mute (`resetLaunch`).
#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    pub exe: PathBuf,
    pub cwd: PathBuf,
    pub command_line: String,
}

/// Идущий рендер (`this.active`).
pub struct Active {
    pub host: Option<Box<dyn Host>>,
    /// Папка задания внутри игры: удаляется в конце.
    pub mod_job_dir: Option<PathBuf>,
    /// Реестр снят и ждёт возврата.
    pub registry: bool,
    /// Windows, возможно, помнит mute игры из-за нас (до подтверждённого сброса).
    pub muted: bool,
    /// Флаг mute с прошлого раза.
    pub pending_before: bool,
    /// Заглушённые до помощника сессии — свои (`watchAudio(…, unmuteAll)`).
    unmute_all: bool,
}

impl Active {
    fn new(muted: bool, pending_before: bool) -> Active {
        Active {
            host: None,
            mod_job_dir: None,
            registry: false,
            muted,
            pending_before,
            unmute_all: false,
        }
    }

    /// Помощник: код выхода игры, вышедшей самой.
    pub fn exit_code(&self) -> Option<u32> {
        self.host.as_ref().and_then(|host| host.exit_code())
    }

    /// Помощник закончил работу.
    pub fn host_closed(&self) -> bool {
        self.host.as_ref().is_some_and(|host| host.closed())
    }
}

#[derive(Default)]
struct Flags {
    active: bool,
    resetting: bool,
    disposing: bool,
}

fn with_message(code: &str, message: impl Into<String>) -> DemoError {
    DemoError {
        message: message.into(),
        ..DemoError::new(code)
    }
}

/// Ошибка ОС, как её покажет `publicError`: `NO_SPACE` или `JOB_FAILED`.
fn fs_error(error: io::Error) -> DemoError {
    let code = if errno::code(&error).as_deref() == Some("ENOSPC") {
        "NO_SPACE"
    } else {
        "JOB_FAILED"
    };
    with_message(code, error.to_string())
}

/// Истинность значения JS (`if (marker.muted)`).
fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

// ---------------------------------------------------------------- маркер и реестр

/// `readMarker(root, spec)`: маркер версии 1 или ничего.
pub fn read_marker(root: &Path, spec: &Spec) -> Option<Map<String, Value>> {
    let text = fs::read_to_string(root.join(spec.marker)).ok()?;
    match serde_json::from_str(&text).ok()? {
        Value::Object(data) if data.get("version").and_then(Value::as_f64) == Some(1.0) => {
            Some(data)
        }
        _ => None,
    }
}

/// `writeMarker(root, spec, data)`: `JSON.stringify(data, null, 2)` через `.tmp`.
pub fn write_marker(root: &Path, spec: &Spec, data: &Map<String, Value>) -> io::Result<()> {
    let file = root.join(spec.marker);
    let mut tmp = file.clone().into_os_string();
    tmp.push(".tmp");
    fs::write(&tmp, stringify(&Value::Object(data.clone()), "  ") + "\n")?;
    fs::rename(&tmp, &file)
}

fn now_iso() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    iso_from_ms(ms)
}

/// `snapshotRegistry(reg, root, spec, extra)`: снимок ключа до первого запуска игры. Маркер
/// пишется до запуска: если лаунчер упадёт посреди рендера, следующий старт вернёт реестр по
/// нему. Невозвращённый снимок не перезаписывается никогда.
pub fn snapshot_registry(
    reg: &dyn Registry,
    root: &Path,
    spec: &Spec,
    extra: Map<String, Value>,
) -> Result<Map<String, Value>, DemoError> {
    // Битый маркер или чужой версии — тоже: под ним может лежать невозвращённый снимок
    // (строже 0.4.9).
    if root.join(spec.marker).exists() {
        return Err(with_message(
            "REGISTRY_FAILED",
            "Прошлый снимок реестра ещё не возвращён",
        ));
    }
    let existed = reg.exists(spec.registry_key)?;
    let reg_file = root.join(spec.reg_file);
    if existed {
        reg.export_key(spec.registry_key, &reg_file)?;
    } else {
        match fs::remove_file(&reg_file) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(fs_error(error)),
            _ => {}
        }
    }
    let mut marker = Map::new();
    marker.insert("version".into(), Value::from(1));
    marker.insert("existed".into(), Value::Bool(existed));
    marker.insert("at".into(), Value::String(now_iso()));
    marker.insert("muted".into(), Value::Bool(false));
    marker.extend(extra);
    write_marker(root, spec, &marker).map_err(fs_error)?;
    Ok(marker)
}

/// `restoreRegistry(reg, root, spec)`: вернуть ключ как был (ключа не было — удалить созданный
/// игрой). Ключ — всегда наш, из маркера берётся только «был ли». Маркер и снимок удаляются
/// только после удачного возврата.
pub fn restore_registry(
    reg: &dyn Registry,
    root: &Path,
    spec: &Spec,
) -> Result<Option<Map<String, Value>>, DemoError> {
    let Some(marker) = read_marker(root, spec) else {
        return Ok(None);
    };
    let reg_file = root.join(spec.reg_file);
    let existed = truthy(marker.get("existed"));
    if existed && !reg_file.exists() {
        return Err(with_message("REGISTRY_FAILED", "Нет снимка реестра"));
    }
    if existed {
        reg.check_file(spec.registry_key, &reg_file)?;
    }
    reg.delete_key(spec.registry_key)?;
    if existed {
        reg.import_file(spec.registry_key, &reg_file)?;
    }
    for file in [reg_file, root.join(spec.marker)] {
        match fs::remove_file(&file) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(fs_error(error)),
            _ => {}
        }
    }
    Ok(Some(marker))
}

// ---------------------------------------------------------------- папка рендера в игре

fn unlink(path: &Path) -> bool {
    fs::remove_file(path)
        .or_else(|_| fs::remove_dir(path))
        .is_ok()
}

/// `cleanupRenderDirs(root, keep)`: остатки прошлых рендеров — только внутри `root` (папка
/// рендера игры), по ссылкам не ходим: ссылка снимается сама. Ответ — сколько убрано.
pub fn cleanup_render_dirs(root: &Path, keep: Option<&str>) -> usize {
    let Ok(meta) = fs::symlink_metadata(root) else {
        return 0;
    };
    if meta.file_type().is_symlink() {
        return usize::from(unlink(root));
    }
    if !meta.is_dir() {
        // Файл на месте папки — не наш, не трогаем (строже 0.4.9); рендер упадёт на создании папки.
        return 0;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if keep.is_some_and(|keep| name.to_string_lossy().to_lowercase() == keep.to_lowercase()) {
            continue;
        }
        let full = entry.path();
        let Ok(meta) = fs::symlink_metadata(&full) else {
            continue;
        };
        // Занятое уберётся в следующий раз.
        let done = if meta.file_type().is_symlink() {
            unlink(&full)
        } else {
            remove_within_zone(root, &full).is_ok()
        };
        if done {
            removed += 1;
        }
    }
    if keep.is_none() {
        let _ = fs::remove_dir(root);
    }
    removed
}

/// `makeRenderDir(root, name)`: своя папка задания внутри папки рендера игры (`root` — не
/// ссылка).
pub fn make_render_dir(root: &Path, name: &str) -> Result<PathBuf, DemoError> {
    cleanup_render_dirs(root, None);
    fs::create_dir_all(root).map_err(fs_error)?;
    let meta = fs::symlink_metadata(root).map_err(fs_error)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        let base = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        return Err(with_message("RENDER_FAILED", format!("{base} — ссылка")));
    }
    let dir = root.join(name);
    fs::create_dir(&dir).map_err(fs_error)?;
    Ok(dir)
}

/// `linkOrCopy(source, target)`: жёсткая ссылка, иначе копия; `target` не перезаписывается.
pub fn link_or_copy(source: &Path, target: &Path) -> io::Result<()> {
    if fs::hard_link(source, target).is_ok() {
        return Ok(());
    }
    let mut from = fs::File::open(source)?;
    let mut to = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)?;
    io::copy(&mut from, &mut to).map(|_| ())
}

/// `maxHeightFor(screen)`: наибольшее разрешение из списка, которое помещается на основной
/// монитор: скрытый стол живёт в той же конфигурации дисплеев, окно больше экрана обрезается.
/// Игры не DPI-aware — размер экрана в DIP.
pub fn max_height_for(screen: Option<(u32, u32)>) -> u32 {
    RESOLUTIONS
        .iter()
        .rfind(|item| screen.is_some_and(|(w, h)| item.width <= w && item.height <= h))
        .unwrap_or(&RESOLUTIONS[0])
        .height
}

// ---------------------------------------------------------------- адаптер

/// `GameRenderAdapter`: общее у адаптеров рендера игр.
pub struct GameRender {
    pub spec: Spec,
    ports: Ports,
    flags: Mutex<Flags>,
    changed: Condvar,
}

impl GameRender {
    pub fn new(spec: Spec, ports: Ports) -> GameRender {
        GameRender {
            spec,
            ports,
            flags: Mutex::new(Flags::default()),
            changed: Condvar::new(),
        }
    }

    fn flags(&self) -> MutexGuard<'_, Flags> {
        self.flags.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn warn(&self, message: &str) {
        self.ports.logger.warn(message);
    }

    pub fn logger(&self) -> &Arc<dyn Logger> {
        &self.ports.logger
    }

    pub fn jobs_root(&self) -> Option<&Path> {
        self.ports.jobs_root.as_deref()
    }

    /// Предел высоты (основной монитор) и частоты кадров; свои возможности — `spec.caps`.
    pub fn capabilities(&self) -> Capabilities {
        Capabilities {
            max_height: max_height_for((self.ports.screen_size)()),
            fps: FPS.to_vec(),
        }
    }

    /// Путь установки, если в нём есть всё из `spec.required`.
    pub fn install_path(&self) -> Option<PathBuf> {
        let found = (self.ports.install_path)()?;
        self.spec
            .required
            .iter()
            .all(|name| found.join(name).exists())
            .then_some(found)
    }

    pub fn is_active(&self) -> bool {
        self.flags().active
    }

    fn pending_flag(&self) -> bool {
        self.ports.store.as_ref().is_some_and(|store| {
            store
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(self.spec.unmute_pending)
                == Some(&Value::Bool(true))
        })
    }

    pub fn set_pending(&self, value: bool) {
        if let Some(store) = &self.ports.store {
            store
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .set(self.spec.unmute_pending, Value::Bool(value));
        }
    }

    /// Старт лаунчера: маркер после сбоя — вернуть реестр. Mute после сбоя без процесса игры не
    /// снять: флаг, снимет следующий рендер. Ответ — маркер (HL1 вернёт по нему журналы).
    pub fn recover(&self) -> Option<Map<String, Value>> {
        let root = self.ports.jobs_root.as_deref()?;
        let marker = read_marker(root, &self.spec)?;
        if truthy(marker.get("muted")) {
            self.set_pending(true);
        }
        let label = self.spec.label;
        // Игра игрока (Steam, ярлык) при выходе перезапишет возвращённое: вернёт следующий
        // рендер, после preflight (строже 0.4.9).
        if !matches!(self.ports.inspector.find_by_image_name(self.spec.image), Ok(running) if running.is_empty())
        {
            self.warn(&format!(
                "[demo] реестр {label} после сбоя не возвращён (вернёт следующий рендер): игра запущена"
            ));
            return None;
        }
        match restore_registry(&*self.ports.registry, root, &self.spec) {
            Ok(_) => self.warn(&format!(
                "[demo] рендер {label} прерван сбоем ({}): реестр игры возвращён",
                to_template_string(marker.get("at"))
            )),
            Err(error) => self.warn(&format!(
                "[demo] реестр {label} после сбоя не возвращён (вернёт следующий рендер): {}",
                error.message
            )),
        }
        Some(marker)
    }

    /// Старт лаунчера: устаревшие папки рендера внутри игры.
    pub fn cleanup_stale(&self) -> usize {
        if self.is_active() {
            return 0;
        }
        self.install_path().map_or(0, |install| {
            cleanup_render_dirs(&(self.spec.render_root)(&install), None)
        })
    }

    /// `steamCode()`: `None` — Steam готов, иначе код для окна.
    pub fn steam_code(&self) -> Option<String> {
        let steam = (self.ports.steam_status)();
        if steam.ok {
            return None;
        }
        Some(match steam.code.as_deref() {
            Some(code @ ("STEAM_NOT_INSTALLED" | "STEAM_ELEVATED")) => code.to_owned(),
            _ => "STEAM_NOT_RUNNING".to_owned(),
        })
    }

    /// Игра не запущена (ни из лаунчера, ни мимо него) и не занята, Steam готов.
    pub fn preflight(&self, exe: &Path) -> Result<(), DemoError> {
        if (self.ports.game_running)(self.spec.game_id) {
            return Err(DemoError::new("GAME_RUNNING"));
        }
        if (self.ports.game_busy)() {
            return Err(DemoError::new("GAME_BUSY"));
        }
        // Игра, запущенная не из лаунчера (Steam, ярлык): тот же exe. Список не получен — отказ,
        // как у `mute_reset_blocker` (строже 0.4.9).
        let running = self
            .ports
            .inspector
            .find_by_image_name(self.spec.image)
            .map_err(|_| DemoError::new("GAME_RUNNING"))?;
        let exe = exe.to_string_lossy();
        if running
            .iter()
            .flatten()
            .any(|path| !path.is_empty() && self.ports.inspector.same_path(path, &exe))
        {
            return Err(DemoError::new("GAME_RUNNING"));
        }
        if let Some(code) = self.spec.other_running
            && !running.is_empty()
        {
            return Err(DemoError::new(code));
        }
        match self.steam_code() {
            Some(code) => Err(DemoError::new(code)),
            None => Ok(()),
        }
    }

    /// Снимок прошлого рендера, не возвращённый после сбоя, — сначала вернуть его: новый
    /// снимок поверх потерял бы значения игрока. Ответ — его маркер.
    pub fn restore_previous(&self) -> Result<Option<Map<String, Value>>, DemoError> {
        let Some(root) = self.ports.jobs_root.as_deref() else {
            return Ok(None);
        };
        let Some(marker) = read_marker(root, &self.spec) else {
            return Ok(None);
        };
        restore_registry(&*self.ports.registry, root, &self.spec)?;
        self.warn(&format!(
            "[demo] {}: возвращён реестр прошлого прерванного рендера",
            self.spec.label
        ));
        Ok(Some(marker))
    }

    /// Снимок реестра до любых изменений (ошибка — игра не запускается). `extra` — что ещё
    /// вернуть после сбоя (HL1: журналы игрока).
    pub fn snapshot(
        &self,
        active: &mut Active,
        extra: Map<String, Value>,
    ) -> Result<(), DemoError> {
        // Без корня заданий реестр игрока не вернуть — игру не запускаем (строже 0.4.9).
        let Some(root) = self.ports.jobs_root.as_deref() else {
            return Err(with_message("REGISTRY_FAILED", "Нет корня заданий"));
        };
        let mut fields = Map::new();
        fields.insert("muted".into(), Value::Bool(active.muted));
        fields.extend(extra);
        snapshot_registry(&*self.ports.registry, root, &self.spec, fields)?;
        active.registry = true;
        Ok(())
    }

    /// Отмена задания или выход лаунчера.
    pub fn cancelled(&self, cancel: &dyn Fn() -> bool) -> bool {
        cancel() || self.flags().disposing
    }

    /// `render(request)`: рендер по одному. Идёт фоновый сброс mute (секунды) — дождаться его.
    /// `job` — сам рендер адаптера (`renderJob`); после него оставшийся помощник
    /// останавливается со сбросом mute и вызывается [`GameRender::finish`].
    pub fn render<T>(
        &self,
        cancel: &dyn Fn() -> bool,
        job: impl FnOnce(&mut Active) -> Result<T, DemoError>,
    ) -> Result<T, DemoError> {
        {
            let mut flags = self.flags();
            while flags.resetting {
                flags = self.changed.wait(flags).unwrap_or_else(|p| p.into_inner());
            }
            if flags.active {
                return Err(DemoError::new("JOB_BUSY"));
            }
            flags.active = true;
        }
        // muted — Windows, возможно, помнит mute игры из-за нас (до события audio-reset ok).
        // Сначала — как было до рендера (флаг прошлого раза).
        let pending_before = self.pending_flag();
        let mut active = Active::new(pending_before, pending_before);
        let result = job(&mut active);
        if active.host.is_some() {
            self.stop_host(&mut active, true);
        }
        self.finish(&mut active);
        // Отмена гасит игру и её процессы: их ошибки после неё — та же отмена.
        let result = result.map_err(|error| {
            if self.cancelled(cancel) {
                DemoError::new("CANCELLED")
            } else {
                error
            }
        });
        {
            let mut flags = self.flags();
            flags.active = false;
            flags.disposing = false;
        }
        self.changed.notify_all();
        result
    }

    /// `muteResetBlocker()`: можно ли сейчас снять mute, запомненный после рендера (флаг
    /// `unmutePending`): `None` — можно, иначе причина. Не трогаем игру игрока и чужие игры на
    /// том же образе (HL1: Steam HL1, CS 1.6), без Steam игра не стартует.
    pub fn mute_reset_blocker(&self) -> Option<String> {
        if !self.pending_flag() {
            return Some("no-flag".into());
        }
        {
            let flags = self.flags();
            if flags.active || flags.resetting {
                return Some("RENDER_ACTIVE".into());
            }
        }
        if !self.ports.windows {
            return Some("RENDER_NOT_READY".into());
        }
        if self.install_path().is_none() {
            return Some("GAME_NOT_INSTALLED".into());
        }
        if (self.ports.game_running)(self.spec.game_id) || (self.ports.game_busy)() {
            return Some("GAME_RUNNING".into());
        }
        match self.ports.inspector.find_by_image_name(self.spec.image) {
            Ok(running) if running.is_empty() => {}
            _ => return Some("GAME_RUNNING".into()),
        }
        self.steam_code()
    }

    /// `resetMute()`: беззвучный сброс без рендера — помощник один раз запускает игру на
    /// скрытом столе и снимает mute с её сессий. Реестр — снимок и возврат, как у рендера (игра
    /// при старте читает и меняет свои настройки). Ответ — mute снят.
    pub fn reset_mute(
        &self,
        reset_launch: &dyn Fn(&Path, &mut Active) -> Result<Launch, DemoError>,
    ) -> bool {
        if self.mute_reset_blocker().is_some() {
            return false;
        }
        {
            let mut flags = self.flags();
            if flags.active || flags.resetting {
                return false;
            }
            flags.active = true;
            flags.resetting = true;
        }
        let mut active = Active::new(true, true);
        let run = |active: &mut Active| -> Result<(), DemoError> {
            // Снимок реестра — в корне заданий; после чистки его может не быть.
            if let Some(root) = self.ports.jobs_root.as_deref() {
                jobs::prepare_root(Some(root))?;
            }
            self.restore_previous()?;
            self.snapshot(active, Map::new())?;
            let install = self
                .install_path()
                .ok_or_else(|| DemoError::new("GAME_NOT_INSTALLED"))?;
            let launch = reset_launch(&install, active)?;
            self.start_host(
                active,
                &StartRequest {
                    reset_only: true,
                    exe: launch.exe.clone(),
                    cwd: launch.cwd,
                    command_line: launch.command_line.clone(),
                    reset_exe: Some(launch.exe),
                    reset_command_line: launch.command_line,
                    mute: false,
                    ..StartRequest::default()
                },
                true,
            )?;
            self.stop_host(active, true);
            Ok(())
        };
        if let Err(error) = run(&mut active) {
            self.warn(&format!(
                "[demo] {}: сброс mute без рендера не удался: {}",
                self.spec.label, error.message
            ));
        }
        if active.host.is_some() {
            self.stop_host(&mut active, true);
        }
        self.finish(&mut active);
        {
            let mut flags = self.flags();
            flags.active = false;
            flags.resetting = false;
            flags.disposing = false;
        }
        self.changed.notify_all();
        self.ports.logger.info(&format!(
            "[demo] {}: mute после рендера {}",
            self.spec.label,
            if active.muted {
                "не снят — попробуем позже"
            } else {
                "снят без нового экспорта"
            }
        ));
        !active.muted
    }

    /// Новый помощник для `active` и запуск игры; `unmute_all` — заглушённые до него сессии
    /// считаются своими (mute прошлой пачки или сбоя). Ответ — PID.
    pub fn start_host(
        &self,
        active: &mut Active,
        request: &StartRequest,
        unmute_all: bool,
    ) -> Result<u32, DemoError> {
        // Прежний помощник без stop_host унёс бы живую игру, mute и реестр.
        if active.host.is_some() {
            return Err(with_message("RENDER_HOST_FAILED", "Помощник уже запущен"));
        }
        active.unmute_all = unmute_all;
        let host = active.host.insert((self.ports.host_factory)());
        host.start(request)
    }

    /// События помощника с прошлого вызова: звук учитывается здесь (`watchAudio`), остальное
    /// (`Foreign`, `Exited`…) — адаптеру.
    pub fn poll(&self, active: &mut Active) -> Vec<HostEvent> {
        let events = active
            .host
            .as_mut()
            .map(|host| host.events())
            .unwrap_or_default();
        self.absorb(active, &events);
        events
    }

    fn absorb(&self, active: &mut Active, events: &[HostEvent]) {
        for event in events {
            match event {
                HostEvent::Muted { was, .. } if !*was || active.unmute_all => {
                    self.mark_muted(active, true)
                }
                HostEvent::AudioReset { ok } => self.ports.logger.info(&format!(
                    "[demo] сброс mute после игры: {}",
                    if *ok { "снят" } else { "не удался" }
                )),
                _ => {}
            }
        }
    }

    /// `stopHost(active, reset)`: закрыть игру. `reset` — снять запомненный mute (конец
    /// рендера); без Steam перезапуск-сброс не стартует — тогда mute остаётся под флагом.
    /// Снятым mute считается только после подтверждённого сброса: «сессию не видели» (отмена
    /// или падение до появления звука, а Windows помнит mute прошлой пачки или сбоя) ничего не
    /// снимает.
    pub fn stop_host(&self, active: &mut Active, reset: bool) {
        let Some(mut host) = active.host.take() else {
            return;
        };
        host.stop(reset && self.steam_code().is_none());
        let events = host.events();
        self.absorb(active, &events);
        if host.audio_reset() == Some(true) {
            self.mark_muted(active, false);
        } else if host.pending() == Some(true) {
            self.mark_muted(active, true);
        }
    }

    /// Конец задания (и выход лаунчера): mute, реестр, папка в игре, маркер. Флаг снимается,
    /// только если mute этого рендера (и прошлого) снят подтверждённым сбросом.
    pub fn finish(&self, active: &mut Active) {
        if active.muted {
            self.set_pending(true);
        } else if active.pending_before {
            self.set_pending(false);
        }
        if active.registry
            && let Some(root) = self.ports.jobs_root.as_deref()
        {
            match restore_registry(&*self.ports.registry, root, &self.spec) {
                Ok(_) => active.registry = false,
                Err(error) => self.warn(&format!(
                    "[demo] реестр {} не возвращён (вернётся при следующем запуске): {}",
                    self.spec.label, error.message
                )),
            }
        }
        if let Some(dir) = active.mod_job_dir.clone() {
            let parent = dir.parent().unwrap_or(&dir).to_path_buf();
            match remove_within_zone(&parent, &dir) {
                Ok(()) => {
                    // Не пустая — остаётся.
                    let _ = fs::remove_dir(&parent);
                    active.mod_job_dir = None;
                }
                Err(error) => {
                    self.warn(&format!("[demo] папка рендера в игре не удалена: {error}"))
                }
            }
        }
    }

    /// Маркер повторяет состояние mute: после сбоя лаунчера его прочтёт `recover`.
    pub fn mark_muted(&self, active: &mut Active, muted: bool) {
        active.muted = muted;
        let Some(root) = self.ports.jobs_root.as_deref() else {
            return;
        };
        let Some(mut marker) = read_marker(root, &self.spec) else {
            return;
        };
        if marker.get("muted") == Some(&Value::Bool(muted)) {
            return;
        }
        marker.insert("muted".into(), Value::Bool(muted));
        // Маркер — страховка, рендер не останавливаем.
        let _ = write_marker(root, &self.spec, &marker);
    }

    /// `disposeSync()`: выход лаунчера посреди рендера. Рендер видит отмену, помощник закрывает
    /// игру (через своё задание) и снимает mute; рендер возвращает реестр и папку. Не успел за
    /// 20 с — mute помечается к снятию следующим рендером, реестр вернёт `recover` при старте.
    pub fn dispose(&self) {
        let mut flags = self.flags();
        if !flags.active {
            return;
        }
        flags.disposing = true;
        let (flags, timeout) = self
            .changed
            .wait_timeout_while(flags, DISPOSE_WAIT, |flags| flags.active)
            .unwrap_or_else(|p| p.into_inner());
        drop(flags);
        if timeout.timed_out() {
            self.set_pending(true);
        }
    }
}
