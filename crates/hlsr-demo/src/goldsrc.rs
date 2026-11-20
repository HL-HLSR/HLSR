//! Адаптер рендера Half-Life (WON-сборка HLSR, BUILD 3248) через bxt-rs, `goldsrc.js` 0.4.9:
//! игра на скрытом рабочем столе ([`crate::render_host`]) проигрывает нужные сегменты по
//! порядку (`bxt_play_folder`), bxt-rs сам отдаёт кадры и звук FFmpeg — видео на каждый сегмент
//! (`bxt_cap_separate_start`) без кадров загрузки (`bxt_cap_skip_non_gameplay_frames`).
//! Сегменты склеиваются в один материал, итог кодирует общий слой ([`crate::jobs`]).
//!
//! `bxt_rs.dll` в обычную игру игрока не попадает: Injector внедряет все DLL своей папки,
//! поэтому для рендера Injector, `BunnymodXT.dll` и `bxt_rs.dll` копируются в папку задания, а
//! папка «Bunnymod XT» игрока не меняется. Закреплённая сборка сверяется по sha256 — та копия,
//! которая пойдёт в игру. Вернуть игроку как было: реестр `HKCU\Software\Valve\Half-Life\Settings`
//! и mute `hl.exe` (общее, [`crate::game_render`]), `qconsole.log` и `bxt-rs.log` в папке Half-Life.
//!
//! Строже 0.4.9 (решение владельца 5): рендер ограничен длительностью из заголовков демок —
//! видео сегмента длиннее самой длинной демки набора (+1 с) останавливает рендер.

use crate::errors::DemoError;
use crate::ffmpeg::{RunOptions, RunOutput};
use crate::game_render::{self, Active, FPS, GameRender, Launch, Ports, Spec};
use crate::ghosting::{EstimateProfile, job_id8, root_of};
use crate::goldsrc_demo::{GoldsrcScan, scan_demo};
use crate::goldsrc_render::{self as gsr, LogLine};
use crate::headers::{self, ENGINE_GOLDSRC};
use crate::jobs::{Adapter, Material, RenderProgress, RenderRequest, SegmentProgress};
use crate::library::Described;
use crate::planner::{Source, Timing};
use crate::render_host::{HostEvent, StartRequest};
use crate::source_render::{Picked, Range, command_line};
use hlsr_core::js::number::number_to_string;
use hlsr_core::js::value::to_number;
use hlsr_core::js::{errno, latin1_string, math_round, to_fixed, to_template_string, truthy};
use hlsr_delivery::files_manifest::hash_file_sync;
use hlsr_delivery::ports::FreeSpace;
use serde_json::{Map, Value, json};
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

pub const GAME_ID: &str = "hl1-won";
pub const EXE: &str = "hl.exe";
const HL_DIR: &str = "Half-Life";
const BXT_DIR: &str = "Bunnymod XT";
const DLL: &str = "bxt_rs.dll";
const LOGS: [&str; 2] = ["qconsole.log", "bxt-rs.log"];
/// Профиль оценки времени ([`crate::estimate::profile`]).
pub const PROFILE: &str = "hl1";

/// Что у HL1 своё (`SPEC`). Любой `hl.exe` (Steam HL1, CS 1.6…) — `GOLDSRC_RUNNING`: общий с WON
/// ключ реестра и мьютекс движка. `hud`, `ownHud` — HUD игры выключается, «Персонализация» —
/// стандартная ([`gsr::job_config`]).
pub const SPEC: Spec = Spec {
    game_id: GAME_ID,
    image: EXE,
    registry_key: "HKCU\\Software\\Valve\\Half-Life\\Settings",
    marker: "hl1-render.restore.json",
    reg_file: "hl1-render.settings.reg",
    unmute_pending: "hl1UnmutePending",
    label: "HL1",
    other_running: Some("GOLDSRC_RUNNING"),
    required: &["Half-Life/hl.exe", "Bunnymod XT/Injector.exe"],
    render_root,
    caps: &["keyhud", "audio", "hud", "ownHud"],
};

fn render_root(install: &Path) -> PathBuf {
    install.join(HL_DIR).join("valve_WON").join(gsr::RENDER_DIR)
}

/// До первой строки «Playing demo from»: запуск с bxt-rs ~5 с, загрузка карты.
const FIRST_TIMEOUT: Duration = Duration::from_secs(90);
/// Ни строк журнала, ни прогресса FFmpeg.
const STALL: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(100);
const REPORT: Duration = Duration::from_millis(250);
/// Промежуточное H.264 (ultrafast, CRF 14) для проверки места: в игре и копия в задании.
const INTERMEDIATE_BITS_PER_PIXEL: f64 = 0.35;
const RESERVE_BYTES: f64 = 512.0 * 1024.0 * 1024.0;
/// Строка `qconsole.log` без конца не копится дольше чтения за раз.
const MAX_REST: usize = 1024 * 1024;

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

fn segment_name(k: usize) -> String {
    format!("s{:03}", k + 1)
}

fn base_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn remove_force(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// `readTail(file, bytes)`: конец файла (latin1); файла нет — `None`.
fn read_tail(file: &Path, bytes: u64) -> Option<String> {
    let mut handle = fs::File::open(file).ok()?;
    let size = handle.metadata().ok()?.len();
    let length = size.min(bytes);
    handle.seek(SeekFrom::Start(size - length)).ok()?;
    let mut buffer = Vec::with_capacity(length as usize);
    handle.take(length).read_to_end(&mut buffer).ok()?;
    Some(latin1_string(&buffer))
}

/// `gameEnv(env, ffmpegDir)`: FFmpeg инструмента первым в PATH (bxt-rs запускает «ffmpeg» сам,
/// а в системном PATH может быть другой), без Discord RPC и vsync BXT. Остальную чистку
/// (`HLSR_*`, `SLINT_*`, Steam) делает помощник.
pub fn game_env(
    env: impl IntoIterator<Item = (String, String)>,
    ffmpeg_dir: &str,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut system_path = String::new();
    for (key, value) in env {
        let lower = key.to_ascii_lowercase();
        if lower == "path" {
            system_path = value;
        } else if lower != "bxt_disable_discord_rpc" && lower != "bxt_disable_vsync" {
            out.push((key, value));
        }
    }
    let delimiter = if cfg!(windows) { ';' } else { ':' };
    let path = if system_path.is_empty() {
        ffmpeg_dir.to_owned()
    } else {
        format!("{ffmpeg_dir}{delimiter}{system_path}")
    };
    out.push(("PATH".into(), path));
    out.push(("BXT_DISABLE_DISCORD_RPC".into(), "1".into()));
    out.push(("BXT_DISABLE_VSYNC".into(), "1".into()));
    out
}

/// `concatList(files, durations)`: список для concat FFmpeg — длительность каждого файла равна
/// длительности его видео, звук после конца видео сегмента отрезается (`outpoint`).
pub fn concat_list(files: &[String], durations: &[f64]) -> String {
    files
        .iter()
        .zip(durations)
        .map(|(file, duration)| {
            let duration = to_fixed(*duration, 6);
            let file = file.replace('\\', "/").replace('\'', "'\\''");
            format!("file '{file}'\nduration {duration}\noutpoint {duration}\n")
        })
        .collect()
}

/// `^hl1-render\.[\w.-]+\.orig$`: копия журнала рядом с маркером.
fn is_backup_name(name: &str) -> bool {
    name.strip_prefix("hl1-render.")
        .and_then(|rest| rest.strip_suffix(".orig"))
        .is_some_and(|middle| {
            !middle.is_empty()
                && middle
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        })
}

/// Где `bxt_rs.dll` и закреплённый sha256 (`bxtrs()`); `sha256: None` — подмена разработчика.
#[derive(Debug, Clone, PartialEq)]
pub struct Bxtrs {
    pub path: PathBuf,
    pub sha256: Option<String>,
}

/// Запуск FFmpeg (склейка сегментов; подмена в тестах).
pub type RunFfmpeg =
    dyn Fn(&Path, &[String], &RunOptions) -> Result<RunOutput, DemoError> + Send + Sync;
/// `ffprobe` видео сегмента (подмена в тестах).
pub type Ffprobe =
    dyn Fn(&Path, &Path, Option<&dyn Fn() -> bool>) -> Result<Value, DemoError> + Send + Sync;

/// Журнал игрока: копия до рендера (`None` — журнала не было) и где начнутся строки рендера.
struct Log {
    file: PathBuf,
    backup: Option<PathBuf>,
    start: u64,
}

/// Журнал из маркера: пути как строки.
type LogEntry = (String, Option<String>);

fn log_entries(logs: &[Log]) -> Vec<LogEntry> {
    logs.iter()
        .map(|log| {
            (
                log.file.to_string_lossy().into_owned(),
                log.backup
                    .as_ref()
                    .map(|backup| backup.to_string_lossy().into_owned()),
            )
        })
        .collect()
}

/// Журналы из маркера (`marker.logs`, недоверенный JSON).
fn marker_logs(marker: &Map<String, Value>) -> Option<Vec<LogEntry>> {
    let logs = marker.get("logs").filter(|logs| truthy(logs))?;
    Some(
        logs.as_array()
            .map(|list| {
                list.iter()
                    .filter(|log| truthy(log))
                    .map(|log| {
                        let backup = log.get("backup").filter(|b| truthy(b));
                        (
                            to_template_string(log.get("file")),
                            backup.map(|b| to_template_string(Some(b))),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
    )
}

/// Что видит цикл журнала.
struct Watch<'a> {
    picked: &'a [Picked<'a>],
    mod_job_dir: &'a Path,
    hl_dir: &'a Path,
    log_start: u64,
    started: Instant,
}

/// `Hl1Adapter`.
pub struct Hl1Adapter {
    game: GameRender,
    free_space: Arc<dyn FreeSpace>,
    bxtrs: Box<dyn Fn() -> Option<Bxtrs> + Send + Sync>,
    run: Box<RunFfmpeg>,
    ffprobe: Box<Ffprobe>,
    first_timeout: Duration,
    stall: Duration,
}

impl Hl1Adapter {
    pub fn new(
        ports: Ports,
        free_space: Arc<dyn FreeSpace>,
        bxtrs: Box<dyn Fn() -> Option<Bxtrs> + Send + Sync>,
    ) -> Hl1Adapter {
        Hl1Adapter {
            game: GameRender::new(SPEC, ports),
            free_space,
            bxtrs,
            run: Box::new(crate::ffmpeg::run),
            ffprobe: Box::new(crate::ffmpeg::ffprobe),
            first_timeout: FIRST_TIMEOUT,
            stall: STALL,
        }
    }

    /// Подмена FFmpeg и ffprobe (тесты).
    pub fn with_ffmpeg(mut self, run: Box<RunFfmpeg>, ffprobe: Box<Ffprobe>) -> Hl1Adapter {
        self.run = run;
        self.ffprobe = ffprobe;
        self
    }

    /// Подмена ожидания первой демки и застоя (тесты).
    pub fn with_timeouts(mut self, first: Duration, stall: Duration) -> Hl1Adapter {
        self.first_timeout = first;
        self.stall = stall;
        self
    }

    /// Общее рендера игр: сброс mute, возможности, выход лаунчера. Восстановление после сбоя —
    /// [`Hl1Adapter::recover`] (с журналами игрока).
    pub fn game(&self) -> &GameRender {
        &self.game
    }

    fn bxtrs_path(&self) -> Option<Bxtrs> {
        (self.bxtrs)().filter(|found| found.path.exists())
    }

    /// `estimateProfile(item, range)`: один запуск игры, демки набора подряд.
    pub fn estimate_profile(&self, item: &Described, range: Option<Range>) -> EstimateProfile {
        let picked = gsr::plan_segments(&item.segments, range).0.len();
        EstimateProfile {
            key: PROFILE,
            launches: 1.0,
            segments: picked.max(1) as f64,
        }
    }

    /// `canRender(item)`: `Err` — код отказа.
    pub fn can_render(&self, item: &Described) -> Result<(), DemoError> {
        if item.engine != Some(ENGINE_GOLDSRC) || item.game != Some("hl1") {
            return Err(DemoError::new("DEMO_UNSUPPORTED"));
        }
        if !cfg!(windows) {
            return Err(DemoError::new("RENDER_NOT_READY"));
        }
        if self.game.install_path().is_none() {
            return Err(DemoError::new("GAME_NOT_INSTALLED"));
        }
        let protocol = |segment: &crate::library::Segment| {
            segment
                .header
                .as_ref()
                .ok()
                .map(|header| header.net_protocol)
        };
        if item
            .segments
            .iter()
            .any(|segment| protocol(segment) != Some(gsr::NET_PROTOCOL))
        {
            return Err(DemoError::new("DEMO_PROTOCOL_UNSUPPORTED"));
        }
        if !item
            .segments
            .iter()
            .any(|segment| segment.duration().is_some_and(|d| d > 0.0))
        {
            return Err(DemoError::new("DEMO_EMPTY"));
        }
        if self.bxtrs_path().is_none() {
            return Err(DemoError::new("BXTRS_MISSING"));
        }
        Ok(())
    }

    /// `resetLaunch(install)`: беззвучный сброс mute без рендера — `hl.exe` без внедрения.
    pub fn reset_launch(install: &Path, _active: &mut Active) -> Result<Launch, DemoError> {
        let hl_dir = install.join(HL_DIR);
        let exe = hl_dir.join(EXE);
        Ok(Launch {
            command_line: command_line(&exe.to_string_lossy(), &gsr::reset_arguments()),
            exe,
            cwd: hl_dir,
        })
    }

    /// `resetMute()`: mute после рендера снимается без экспорта. Ответ — снят. Общий сброс
    /// возвращает реестр прошлого прерванного рендера без журналов — они сначала.
    pub fn reset_mute(&self) -> bool {
        if self.game.mute_reset_blocker().is_none()
            && let Some(root) = self.game.jobs_root()
            && let Some(marker) = game_render::read_marker(root, &SPEC)
        {
            self.restore_marker_logs(&marker);
        }
        self.game.reset_mute(&Self::reset_launch)
    }

    /// `recover()`: старт лаунчера — реестр и журналы прошлого рендера, прерванного сбоем.
    pub fn recover(&self) -> Option<Map<String, Value>> {
        let marker = self.game.recover()?;
        self.restore_marker_logs(&marker);
        Some(marker)
    }

    /// `saveLogs(hlDir, jobDir)`: журналы, которые игра с bxt-rs дописывает, — копия до запуска
    /// рядом с маркером реестра (файлы корня заданий переживают чистку папок).
    fn save_logs(&self, hl_dir: &Path, job_dir: &Path) -> Result<Vec<Log>, DemoError> {
        let dir = self.game.jobs_root().unwrap_or(job_dir);
        LOGS.iter()
            .map(|name| {
                let file = hl_dir.join(name);
                let backup = dir.join(format!("hl1-render.{name}.orig"));
                match fs::copy(&file, &backup).and_then(|_| fs::metadata(&file)) {
                    Ok(meta) => Ok(Log {
                        file,
                        backup: Some(backup),
                        start: meta.len(),
                    }),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        remove_force(&backup).map_err(fs_error)?;
                        Ok(Log {
                            file,
                            backup: None,
                            start: 0,
                        })
                    }
                    Err(error) => Err(with_message("RENDER_FAILED", format!("{name}: {error}"))),
                }
            })
            .collect()
    }

    /// `restoreLogs(logs)`: пути из маркера проверяются — только журналы в папке Half-Life и
    /// копии рядом с маркером.
    fn restore_logs(&self, logs: &[LogEntry]) {
        let root = self
            .game
            .jobs_root()
            .and_then(|root| std::path::absolute(root).ok());
        for (file, backup) in logs {
            let file = Path::new(file);
            let name = base_name(file);
            let in_hl = file.parent().map(base_name).as_deref() == Some(HL_DIR);
            if !LOGS.contains(&name.as_str()) || !in_hl {
                continue;
            }
            if let Some(backup) = backup {
                let backup = Path::new(backup);
                let beside_marker = || {
                    let dir = backup
                        .parent()
                        .and_then(|dir| std::path::absolute(dir).ok());
                    root.is_none() || dir == root
                };
                if !is_backup_name(&base_name(backup)) || !beside_marker() {
                    continue;
                }
            }
            let result = match backup {
                Some(backup) => {
                    fs::copy(backup, file).and_then(|_| remove_force(Path::new(backup)))
                }
                None => remove_force(file),
            };
            if let Err(error) = result {
                self.game
                    .logger()
                    .warn(&format!("[demo] HL1: {name} не возвращён: {error}"));
            }
        }
    }

    /// Журналы возвращаются один раз: маркер, оставшийся из-за невозвращённого реестра,
    /// переписывается без них — иначе следующий старт удалил бы журнал, созданный игроком уже
    /// после рендера.
    fn forget_marker_logs(&self) {
        let Some(root) = self.game.jobs_root() else {
            return;
        };
        let Some(mut left) = game_render::read_marker(root, &SPEC) else {
            return;
        };
        if !left.get("logs").is_some_and(truthy) {
            return;
        }
        left.shift_remove("logs");
        if let Err(error) = game_render::write_marker(root, &SPEC, &left) {
            self.game
                .logger()
                .warn(&format!("[demo] HL1: маркер рендера не переписан: {error}"));
        }
    }

    fn restore_marker_logs(&self, marker: &Map<String, Value>) {
        if let Some(logs) = marker_logs(marker) {
            self.restore_logs(&logs);
            self.forget_marker_logs();
        }
    }

    /// `checkSpace(...)`: промежуточное видео в игре и его копия в задании.
    fn check_space(
        &self,
        install: &Path,
        request: &RenderRequest,
        seconds: f64,
    ) -> Result<(), DemoError> {
        let (width, height) = (f64::from(request.width), f64::from(request.height));
        let video =
            (width * height * f64::from(request.fps) * INTERMEDIATE_BITS_PER_PIXEL * seconds / 8.0)
                .ceil();
        let game_need = video + RESERVE_BYTES;
        let job_need = video * 2.0 + RESERVE_BYTES;
        let free = |dir: &Path| self.free_space.free_bytes(dir).map(|bytes| bytes as f64);
        let (game_free, job_free) = (free(install), free(request.job_dir));
        // Место не узнать (None) — не отказ, как в 0.4.9 (`gameFree !== null`).
        let short = if root_of(install) == root_of(request.job_dir) {
            game_free.is_some_and(|free| free < game_need + job_need)
        } else {
            game_free.is_some_and(|free| free < game_need)
                || job_free.is_some_and(|free| free < job_need)
        };
        if short {
            return Err(DemoError::new("NO_SPACE"));
        }
        Ok(())
    }

    /// `renderJob(request, active)`.
    fn render_job(
        &self,
        request: &RenderRequest,
        active: &mut Active,
    ) -> Result<Material, DemoError> {
        if !cfg!(windows) {
            return Err(DemoError::new("RENDER_NOT_READY"));
        }
        let install = self
            .game
            .install_path()
            .ok_or_else(|| DemoError::new("GAME_NOT_INSTALLED"))?;
        if !FPS.contains(&request.fps) {
            return Err(DemoError::new("COMBO_UNSUPPORTED"));
        }
        let bxtrs = self
            .bxtrs_path()
            .ok_or_else(|| DemoError::new("BXTRS_MISSING"))?;
        let hl_dir = install.join(HL_DIR);
        let exe = hl_dir.join(EXE);
        self.game.preflight(&exe)?;
        // bxt-rs ищет ffmpeg.exe сначала рядом с hl.exe: чужой там перебил бы наш из PATH.
        if hl_dir.join("ffmpeg.exe").exists() {
            return Err(DemoError::new("FFMPEG_IN_GAME_DIR"));
        }
        let (picked, offset) = gsr::plan_segments(&request.item.segments, Some(request.range));
        if picked.is_empty() {
            return Err(DemoError::new("DEMO_EMPTY"));
        }
        for entry in &picked {
            let number = || json!({"segment": entry.index + 1, "name": entry.segment.name});
            let path = Path::new(&entry.segment.path);
            let meta = fs::symlink_metadata(path)
                .ok()
                .filter(fs::Metadata::is_file)
                .ok_or_else(|| DemoError::new("SEGMENT_MISSING").with_details(number()))?;
            // Файл заменили после того, как набор добавили в список.
            let same = matches!(headers::parse_demo_file(path),
                Ok(header) if header.engine == ENGINE_GOLDSRC && header.duration == entry.segment.duration());
            if meta.len() != entry.segment.size || !same {
                return Err(DemoError::new("SEGMENT_REPLACED").with_details(number()));
            }
        }
        let demo_seconds = picked.iter().fold(0.0, |sum, entry| sum + entry.duration);
        self.check_space(&install, request, demo_seconds)?;
        // Смены карт, кнопки и паузы загрузок — из самих демок; не читается — без них.
        let mut scans: Vec<Option<GoldsrcScan>> = Vec::with_capacity(picked.len());
        for entry in &picked {
            match scan_demo(Path::new(&entry.segment.path), &|| {
                self.game.cancelled(request.cancel)
            }) {
                Ok(scan) => scans.push(Some(scan)),
                Err(error) if error.code == "CANCELLED" => return Err(error),
                Err(error) => {
                    if error.code == "DEMO_READ_FAILED" {
                        self.game.logger().warn(&format!(
                            "[demo] HL1: {} не разобрана: {}",
                            entry.segment.name, error.message
                        ));
                    }
                    scans.push(None);
                }
            }
        }
        if self.game.cancelled(request.cancel) {
            return Err(DemoError::new("CANCELLED"));
        }
        if let Some(marker) = self.game.restore_previous()? {
            self.restore_marker_logs(&marker);
        }

        let mut logs = None;
        let mut foreign = None;
        let result = self.render_game(
            request,
            active,
            &install,
            &bxtrs,
            (&picked, offset, &scans),
            &mut logs,
            &mut foreign,
        );
        // Рядом чужой hl.exe: без сброса mute (WON hl.exe рядом с ним ждал бы мьютекс до 30 с) —
        // реестр сразу, mute снимет следующий рендер.
        if active.host.is_some() {
            self.game.stop_host(active, foreign.is_none());
        }
        self.game.finish(active);
        if let Some(logs) = logs {
            self.restore_logs(&log_entries(&logs));
            self.forget_marker_logs();
        }
        result
    }

    /// Запуск игры и материал (тело `try` в `renderJob`); игру закрывает и всё возвращает
    /// [`Hl1Adapter::render_job`].
    #[allow(clippy::too_many_arguments)]
    fn render_game(
        &self,
        request: &RenderRequest,
        active: &mut Active,
        install: &Path,
        bxtrs: &Bxtrs,
        (picked, offset, scans): (&[Picked], f64, &[Option<GoldsrcScan>]),
        logs: &mut Option<Vec<Log>>,
        foreign: &mut Option<u32>,
    ) -> Result<Material, DemoError> {
        let (fps, width, height) = (request.fps, request.width, request.height);
        let hl_dir = install.join(HL_DIR);
        let exe = hl_dir.join(EXE);
        let job_dir = request.job_dir;
        // Папка задания в игре: bxt_play_folder и exec видят только папку мода.
        let id8 = job_id8(job_dir);
        let dir = format!("{}/{id8}", gsr::RENDER_DIR);

        let saved = logs.insert(self.save_logs(&hl_dir, job_dir)?);
        let entries: Vec<Value> = log_entries(saved)
            .into_iter()
            .map(|(file, backup)| json!({"file": file, "backup": backup}))
            .collect();
        let start_of_log = saved.first().map_or(0, |log| log.start);
        self.game.snapshot(
            active,
            Map::from_iter([("logs".to_owned(), Value::Array(entries))]),
        )?;
        let mod_job_dir = game_render::make_render_dir(&render_root(install), &id8)?;
        active.mod_job_dir = Some(mod_job_dir.clone());
        for (k, entry) in picked.iter().enumerate() {
            let target = mod_job_dir.join(format!("{}.dem", segment_name(k)));
            game_render::link_or_copy(Path::new(&entry.segment.path), &target).map_err(fs_error)?;
        }
        let config = gsr::job_config(fps, &dir, request.hud, request.own_hud);
        fs::write(mod_job_dir.join("job.cfg"), config).map_err(fs_error)?;
        // Injector внедряет все DLL своей папки: своя папка только с нужными.
        let inject_dir = job_dir.join("inject");
        fs::create_dir(&inject_dir).map_err(fs_error)?;
        let injector = inject_dir.join("Injector.exe");
        let bxt_dir = install.join(BXT_DIR);
        fs::copy(bxt_dir.join("Injector.exe"), &injector).map_err(fs_error)?;
        let bxt = bxt_dir.join("BunnymodXT.dll");
        if bxt.exists() {
            fs::copy(&bxt, inject_dir.join("BunnymodXT.dll")).map_err(fs_error)?;
        }
        let dll = inject_dir.join(DLL);
        fs::copy(&bxtrs.path, &dll).map_err(fs_error)?;
        // Закреплённая сборка: сверяется та копия, которая пойдёт в игру.
        if let Some(sha256) = &bxtrs.sha256 {
            let actual =
                hash_file_sync(&dll).map_err(|error| with_message("JOB_FAILED", error.message))?;
            if actual != *sha256 {
                return Err(DemoError::new("BXTRS_BROKEN"));
            }
        }

        let unmute_all = active.muted;
        let ffmpeg_dir = request
            .ffmpeg
            .ffmpeg
            .parent()
            .map(|dir| dir.to_string_lossy().into_owned())
            .unwrap_or_default();
        let env = game_env(
            std::env::vars_os().map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned(),
                )
            }),
            &ffmpeg_dir,
        );
        let exe_text = exe.to_string_lossy().into_owned();
        let mut args = vec![exe_text.clone()];
        args.extend(gsr::game_arguments(width, height, &dir));
        let started = Instant::now();
        let pid = self.game.start_host(
            active,
            &StartRequest {
                command_line: command_line(&injector.to_string_lossy(), &args),
                exe: injector,
                cwd: hl_dir.clone(),
                mute: true,
                unmute_all,
                follow_job: true,
                watch_image: Some("hl".into()),
                reset_exe: Some(exe.clone()),
                reset_command_line: command_line(&exe_text, &gsr::reset_arguments()),
                env: Some(env),
                ..StartRequest::default()
            },
            unmute_all,
        )?;
        let logger = self.game.logger();
        logger.info(&format!(
            "[demo] HL1: Injector PID {pid}, сегментов {}, {width}x{height}@{fps}",
            picked.len()
        ));
        let watch = Watch {
            picked,
            mod_job_dir: &mod_job_dir,
            hl_dir: &hl_dir,
            log_start: start_of_log,
            started,
        };
        self.watch(request, active, &watch, foreign)?;
        // Конец всех демок: игра в меню, закрывается заглушённой, mute снимает перезапуск.
        self.game.stop_host(active, true);
        logger.info(&format!(
            "[demo] HL1: рендер {} с",
            number_to_string(math_round(started.elapsed().as_secs_f64()))
        ));
        let cancelled: &dyn Fn() -> bool = &|| self.game.cancelled(request.cancel);
        if cancelled() {
            return Err(DemoError::new("CANCELLED"));
        }

        let files: Vec<PathBuf> = (0..picked.len())
            .map(|k| {
                mod_job_dir
                    .join("out")
                    .join(format!("{}.mp4", segment_name(k)))
            })
            .collect();
        let mut videos: Vec<f64> = Vec::with_capacity(files.len());
        for file in &files {
            let base = base_name(file);
            if !file.exists() {
                return Err(with_message("RENDER_FAILED", format!("Нет видео {base}")));
            }
            let info = (self.ffprobe)(&request.ffmpeg.ffprobe, file, Some(cancelled))?;
            let stream = info["streams"]
                .as_array()
                .and_then(|streams| streams.iter().find(|s| s["codec_type"] == "video"));
            let duration = to_number(stream.and_then(|stream| stream.get("duration")));
            if duration.is_nan() || duration <= 0.0 {
                return Err(with_message(
                    "RENDER_FAILED",
                    format!("Видео {base} пустое"),
                ));
            }
            // Загрузки съедают ~0,1 с в начале и ~0,15–0,3 с на смене карты; много больше —
            // демка оборвалась, клип вышел бы без конца сегмента. Строже 0.4.9: и не длиннее
            // демки по заголовку — последний сегмент кончается раньше проверки progress.txt.
            let entry = &picked[videos.len()];
            let changes = scans[videos.len()].as_ref().map_or(0, |scan| {
                scan.map_changes
                    .iter()
                    .filter(|change| change.t > 0.0)
                    .count()
            });
            if entry.duration - duration > 1.0 + changes as f64 || duration > entry.duration + 1.0 {
                return Err(with_message(
                    "RENDER_FAILED",
                    format!(
                        "Сегмент {}: видео {} с при демке {} с",
                        entry.index + 1,
                        number_to_string(duration),
                        number_to_string(entry.duration)
                    ),
                ));
            }
            videos.push(duration);
        }
        let material = job_dir.join("material.mkv");
        let material_text = material.to_string_lossy().into_owned();
        let list = job_dir.join("segments.txt");
        let names: Vec<String> = files
            .iter()
            .map(|file| file.to_string_lossy().into_owned())
            .collect();
        fs::write(&list, concat_list(&names, &videos)).map_err(fs_error)?;
        let mut concat: Vec<String> = [
            "-hide_banner",
            "-nostdin",
            "-y",
            "-loglevel",
            "error",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
        ]
        .map(str::to_owned)
        .to_vec();
        concat.push(list.to_string_lossy().into_owned());
        concat.extend(["-map", "0:v:0", "-map", "0:a:0?", "-c", "copy"].map(str::to_owned));
        concat.push(material_text.clone());
        let options = RunOptions {
            cwd: Some(job_dir),
            cancel: Some(cancelled),
            ..RunOptions::default()
        };
        (self.run)(&request.ffmpeg.ffmpeg, &concat, &options)?;

        let cuts = gsr::loss_cuts(picked, &videos, scans);
        let total = videos.iter().fold(0.0, |sum, video| sum + video);
        let demo_seconds = picked.iter().fold(0.0, |sum, entry| sum + entry.duration);
        logger.info(&format!(
            "[demo] HL1: видео {} с при демке {} с, вырезано загрузок {}",
            to_fixed(total, 3),
            to_fixed(demo_seconds, 3),
            cuts.len()
        ));
        let mut overlay = None;
        let mut warnings = Vec::new();
        if request.keyhud {
            let timing = Timing {
                offset,
                cuts: cuts.clone(),
            };
            let events = gsr::key_events(picked, scans, &timing);
            fs::write(
                job_dir.join("keyhud.ass"),
                gsr::keyhud_ass(&events, width, height, total),
            )
            .map_err(fs_error)?;
            overlay = Some("keyhud.ass".to_owned());
            // Демка не разобралась или разбор оборвался: там клавиши не загорятся.
            if scans
                .iter()
                .any(|scan| scan.as_ref().is_none_or(|scan| scan.broken))
            {
                warnings.push("KEYHUD_PARTIAL".to_owned());
            }
        }
        Ok(Material {
            audio: request.audio.then(|| material_text.clone()),
            offset: Some(offset),
            timeline: gsr::timeline_of(picked, scans, Some(request.range)),
            cuts,
            overlay,
            warnings,
            profile: Some(PROFILE.to_owned()),
            launches: Some(1.0),
            segments: Some(picked.len() as f64),
            frames: Some(math_round(total * f64::from(fps))),
            ..Material::new(Source::Video(material_text))
        })
    }

    /// `watch(...)`: новые строки `qconsole.log` (сегменты, конец по «Recording stopped.»
    /// каждого, ошибки) и прогресс FFmpeg текущего сегмента.
    fn watch(
        &self,
        request: &RenderRequest,
        active: &mut Active,
        watch: &Watch,
        foreign: &mut Option<u32>,
    ) -> Result<(), DemoError> {
        let picked = watch.picked;
        let count = picked.len();
        let log_file = watch.hl_dir.join("qconsole.log");
        let progress_file = watch.mod_job_dir.join("progress.txt");
        let total = picked.iter().fold(0.0, |sum, entry| sum + entry.duration);
        // Строже 0.4.9: видео не длиннее демки по заголовку. progress.txt прошлого сегмента живёт,
        // пока его не перепишет FFmpeg нового, — предел по самой длинной демке набора.
        let longest = picked
            .iter()
            .fold(0.0, |max: f64, entry| max.max(entry.duration));
        let mut log_pos = watch.log_start;
        let mut rest: Vec<u8> = Vec::new();
        let mut stopped = 0usize;
        let mut playing: Option<u32> = None;
        let mut last_activity = watch.started;
        let mut last_progress: Option<f64> = None;
        let mut last_report: Option<Instant> = None;
        let mut fraction: f64 = 0.0;
        loop {
            if self.game.cancelled(request.cancel) {
                return Err(DemoError::new("CANCELLED"));
            }
            if let Some(lines) = read_log(&log_file, &mut log_pos, &mut rest) {
                last_activity = Instant::now();
                let mut failure = None;
                for line in lines {
                    match gsr::parse_log_line(&latin1_string(&line)) {
                        Some(LogLine::Error { code, text }) if failure.is_none() => {
                            failure = Some(with_message(code, text));
                        }
                        Some(LogLine::Playing(index)) => playing = Some(index),
                        Some(LogLine::Stopped) => stopped += 1,
                        _ => {}
                    }
                }
                if let Some(error) = failure {
                    return Err(error);
                }
            }
            if stopped >= count {
                return Ok(());
            }
            for event in self.game.poll(active) {
                if let HostEvent::Foreign { pid } = event {
                    foreign.get_or_insert(pid);
                }
            }
            if let Some(pid) = foreign {
                return Err(with_message(
                    "GOLDSRC_RUNNING",
                    format!("Запущен другой hl.exe (PID {pid})"),
                ));
            }
            // Код выхода здесь — у Injector, не у игры: в подробности его не отдаём.
            if let Some(code) = active.exit_code() {
                return Err(with_message(
                    "GAME_EXITED",
                    format!("Задание опустело, код Injector {code}"),
                ));
            }
            if active.host_closed() {
                return Err(with_message("RENDER_HOST_FAILED", "Помощник вышел"));
            }
            let out = read_tail(&progress_file, 2048).and_then(|text| gsr::progress_seconds(&text));
            if out.is_some() && out != last_progress {
                last_progress = out;
                last_activity = Instant::now();
            }
            if let Some(out) = last_progress.filter(|out| *out > longest + 1.0) {
                return Err(with_message(
                    "RENDER_FAILED",
                    format!(
                        "Видео сегмента {} с, а самая длинная демка — {} с",
                        number_to_string(out),
                        number_to_string(longest)
                    ),
                ));
            }
            let now = Instant::now();
            if playing.is_none() && now.duration_since(watch.started) > self.first_timeout {
                // bxt-rs не встал: в bxt-rs.log нет строки о найденном движке.
                let found = read_tail(&watch.hl_dir.join("bxt-rs.log"), 64 * 1024)
                    .is_some_and(|text| text.contains("found the WON engine"));
                return Err(DemoError::new(if found {
                    "RENDER_TIMEOUT"
                } else {
                    "BXTRS_FAILED"
                }));
            }
            if now.duration_since(last_activity) > self.stall {
                return Err(DemoError::new("RENDER_TIMEOUT"));
            }
            if last_report.is_none_or(|at| now.duration_since(at) >= REPORT) {
                last_report = Some(now);
                let done = picked[..stopped.min(count)]
                    .iter()
                    .fold(0.0, |sum, entry| sum + entry.duration);
                let current = playing
                    .filter(|index| stopped < *index as usize)
                    .and_then(|index| picked.get((index as usize).checked_sub(1)?));
                let part = current.map_or(0.0, |entry| {
                    last_progress.unwrap_or(0.0).min(entry.duration)
                });
                // Доля не убывает: progress.txt прошлого сегмента живёт до нового FFmpeg.
                fraction = fraction.max(((done + part) / total.max(0.001)).min(0.999));
                (request.on_progress)(&RenderProgress {
                    fraction,
                    segment: playing.map(|index| SegmentProgress {
                        index,
                        count: count as u32,
                    }),
                    message: None,
                });
            }
            thread::sleep(POLL);
        }
    }
}

/// Новые целые строки `qconsole.log` с `pos`; `None` — файла нет или ничего не прибавилось.
/// Движок мог начать журнал заново — тогда с начала.
fn read_log(file: &Path, pos: &mut u64, rest: &mut Vec<u8>) -> Option<Vec<Vec<u8>>> {
    let mut handle = fs::File::open(file).ok()?;
    let size = handle.metadata().ok()?.len();
    if size < *pos {
        *pos = 0;
        // Строже 0.4.9: хвост старого журнала не склеивается с началом нового.
        rest.clear();
    }
    if size == *pos {
        return None;
    }
    let mut chunk = Vec::new();
    handle.seek(SeekFrom::Start(*pos)).ok()?;
    handle
        .take((size - *pos).min(1024 * 1024))
        .read_to_end(&mut chunk)
        .ok()?;
    *pos += chunk.len() as u64;
    rest.extend_from_slice(&chunk);
    let mut lines: Vec<Vec<u8>> = rest.split(|b| *b == b'\n').map(<[u8]>::to_vec).collect();
    *rest = lines.pop().unwrap_or_default();
    // Строже 0.4.9: строка без конца не копится без предела.
    if rest.len() > MAX_REST {
        rest.clear();
    }
    Some(lines)
}

impl Adapter for Hl1Adapter {
    fn render(&self, request: &RenderRequest) -> Result<Material, DemoError> {
        self.game
            .render(request.cancel, |active| self.render_job(request, active))
    }
}
