//! Запуск внешнего апдейтера (`launcher-update/updater-launch.js`): копия лаунчера в
//! `%TEMP%\HLSR-Updater-<ms36>-<12hex>\HLSRUpdater.exe`, рядом `update-plan.json` (два пробела) и
//! `resources\QA_TRUST`, если он есть у лаунчера; запуск `--hlsr-updater --plan=<файл>` отвязанным
//! процессом; процесс должен прожить 1,2 с (ждём не дольше 15 с). Копии старше суток убираются.
//!
//! Отступление (решение 14): копируется один `HLSR.exe` — Electron 0.4.7 копировал весь каталог
//! без `resources` плюс `resources\app.asar`, без которого его exe не запускался.

use super::processes::{ProcessLauncher, Spawn};
use crate::ports::{BoxFuture, Clock, Logger};
use hlsr_core::js;
use hlsr_core::json_text::stringify;
use serde_json::Value;
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// `UPDATER_EXE`.
pub const UPDATER_EXE: &str = "HLSRUpdater.exe";
/// `QA_TRUST`: ключи QA-канала для апдейтера (`resources\QA_TRUST`).
pub const QA_TRUST: &str = "QA_TRUST";
/// `update-plan.json` рядом с копией.
pub const PLAN_FILE: &str = "update-plan.json";
/// `STARTUP_TIMEOUT_MS`.
pub const STARTUP_TIMEOUT: Duration = Duration::from_millis(15_000);
/// `stableMs` по умолчанию.
pub const STARTUP_STABLE: Duration = Duration::from_millis(1200);
/// Копии старше этого убираются (`maxAgeMs`).
pub const STAGE_MAX_AGE_MS: f64 = 24.0 * 60.0 * 60.0 * 1000.0;
/// Флаг режима апдейтера (`updater-mode.js`: `FLAG`).
pub const UPDATER_FLAG: &str = "--hlsr-updater";

/// Шесть случайных байт (`crypto.randomBytes(6)`): ключи `RandomState` процесса, время и PID.
pub fn random_suffix() -> [u8; 6] {
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos()),
    );
    hasher.write_u32(std::process::id());
    let bytes = hasher.finish().to_le_bytes();
    [bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]]
}

/// `uniqueDir(tempRoot)`: `HLSR-Updater-<Date.now() в base36>-<12 hex>`.
pub fn unique_dir(temp_root: &Path, now_ms: i64, suffix: [u8; 6]) -> PathBuf {
    temp_root.join(format!(
        "HLSR-Updater-{}-{}",
        js::base36(now_ms.max(0) as u128),
        js::text::hex(&suffix)
    ))
}

/// Имя копии апдейтера (`/^HLSR-Updater-[a-z0-9]+-[a-f0-9]{12}$/`).
pub fn is_stage_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("HLSR-Updater-") else {
        return false;
    };
    let Some((middle, suffix)) = rest.rsplit_once('-') else {
        return false;
    };
    !middle.is_empty()
        && middle
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && suffix.len() == 12
        && suffix
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Развёрнутая копия.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    pub dir: PathBuf,
    pub exe: PathBuf,
    pub plan_file: PathBuf,
}

/// `stage({appExe, resourcesPath, dir})`: `HLSR.exe` → `<dir>\HLSRUpdater.exe`, `QA_TRUST` →
/// `<dir>\resources\QA_TRUST`.
pub fn stage(app_exe: &Path, resources: &Path, dir: &Path) -> Result<Staged, String> {
    let fs_error = |error: std::io::Error, syscall: &str, path: &Path| {
        js::errno::fs_message(&error, syscall, &path.to_string_lossy(), None)
    };
    std::fs::create_dir_all(dir).map_err(|error| fs_error(error, "mkdir", dir))?;
    let exe = dir.join(UPDATER_EXE);
    std::fs::copy(app_exe, &exe).map_err(|error| fs_error(error, "copyfile", app_exe))?;

    let qa_trust = resources.join(QA_TRUST);
    if qa_trust.is_file() {
        let target = dir.join("resources");
        std::fs::create_dir_all(&target).map_err(|error| fs_error(error, "mkdir", &target))?;
        std::fs::copy(&qa_trust, target.join(QA_TRUST))
            .map_err(|error| fs_error(error, "copyfile", &qa_trust))?;
    }

    if !exe.is_file() {
        return Err("копия Updater не собралась".to_owned());
    }
    Ok(Staged {
        dir: dir.to_owned(),
        plan_file: dir.join(PLAN_FILE),
        exe,
    })
}

/// `writePlan(planFile, plan)`: `JSON.stringify(plan, null, 2)` без перевода строки в конце.
pub fn write_plan(plan_file: &Path, plan: &Value) -> Result<(), String> {
    std::fs::write(plan_file, stringify(plan, "  "))
        .map_err(|error| js::errno::fs_message(&error, "open", &plan_file.to_string_lossy(), None))
}

/// `spawnUpdater(exe, planFile)`: `--hlsr-updater --plan=<файл>` отвязанным процессом из каталога
/// копии; PID 0 — «процесс Updater не запустился».
pub fn spawn_updater(
    launcher: &dyn ProcessLauncher,
    exe: &Path,
    plan_file: &Path,
    env: Vec<(String, String)>,
) -> Result<u32, String> {
    let command = Spawn {
        file: exe.to_owned(),
        args: vec![
            UPDATER_FLAG.to_owned(),
            format!("--plan={}", plan_file.to_string_lossy()),
        ],
        cwd: exe.parent().map(Path::to_path_buf),
        env,
        hidden: false,
        detached: true,
    };
    match launcher.spawn(&command)? {
        0 => Err("процесс Updater не запустился".to_owned()),
        pid => Ok(pid),
    }
}

/// Пауза ожидания (`sleep`).
pub type Sleep<'a> = &'a (dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync);

/// `waitForStartup(pid, {timeoutMs, stableMs})`: `Ok` — процесс прожил `stable`; `Err` — причина.
pub async fn wait_for_startup(
    pid: u32,
    is_alive: &(dyn Fn(u32) -> bool + Send + Sync),
    clock: &dyn Clock,
    sleep: Sleep<'_>,
    timeout: Duration,
    stable: Duration,
) -> Result<(), String> {
    let deadline = clock.now_ms() + timeout.as_millis() as i64;
    let until = clock.now_ms() + stable.as_millis() as i64;
    while clock.now_ms() < deadline {
        if !is_alive(pid) {
            return Err("процесс завершился сразу после запуска".to_owned());
        }
        if clock.now_ms() >= until {
            return Ok(());
        }
        sleep(stable.min(Duration::from_millis(300))).await;
    }
    Err("таймаут ожидания запуска".to_owned())
}

/// Параметры [`launch`].
pub struct LaunchOptions<'a> {
    /// `HLSR.exe` лаунчера (`app.getPath("exe")`).
    pub app_exe: &'a Path,
    /// `resources` рядом с ним.
    pub resources: &'a Path,
    pub temp_root: &'a Path,
    /// Каталог копии; `None` — новый [`unique_dir`].
    pub dir: Option<PathBuf>,
    /// План (`buildUpdatePlan` службы обновления).
    pub plan: &'a Value,
    pub launcher: &'a dyn ProcessLauncher,
    /// Окружение апдейтера.
    pub env: Vec<(String, String)>,
    pub clock: &'a dyn Clock,
    pub sleep: Sleep<'a>,
    pub logger: &'a dyn Logger,
}

/// Запущенный апдейтер.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launched {
    pub pid: u32,
    pub staged: Staged,
}

/// `launch(options)`.
pub async fn launch(options: LaunchOptions<'_>) -> Result<Launched, String> {
    let dir = options
        .dir
        .clone()
        .unwrap_or_else(|| unique_dir(options.temp_root, options.clock.now_ms(), random_suffix()));
    let staged = stage(options.app_exe, options.resources, &dir)?;
    options.logger.info(&format!(
        "[updater-launch] копия развёрнута: {}",
        staged
            .dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    write_plan(&staged.plan_file, options.plan)?;
    let pid = spawn_updater(
        options.launcher,
        &staged.exe,
        &staged.plan_file,
        options.env.clone(),
    )?;
    options
        .logger
        .info(&format!("[updater-launch] Updater запущен, PID {pid}"));
    let launcher = options.launcher;
    let alive = move |pid: u32| launcher.is_alive(pid);
    if let Err(reason) = wait_for_startup(
        pid,
        &alive,
        options.clock,
        options.sleep,
        STARTUP_TIMEOUT,
        STARTUP_STABLE,
    )
    .await
    {
        options
            .logger
            .error(&format!("[updater-launch] Updater не поднялся: {reason}"));
        return Err(reason);
    }
    Ok(Launched { pid, staged })
}

/// `cleanupOldStages({tempRoot, maxAgeMs, now})`: сколько каталогов копий удалено. Занятый
/// каталог удалится в следующий раз.
pub fn cleanup_old_stages(temp_root: &Path, max_age_ms: f64, now_ms: i64) -> usize {
    let Ok(entries) = std::fs::read_dir(temp_root) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !is_stage_name(&name.to_string_lossy()) {
            continue;
        }
        let full = entry.path();
        let Ok(metadata) = std::fs::metadata(&full) else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }
        let Ok(mtime) = js::fs::mtime_ms(&metadata) else {
            continue;
        };
        if (now_ms as f64) - mtime < max_age_ms {
            continue;
        }
        if std::fs::remove_dir_all(&full).is_ok() {
            removed += 1;
        }
    }
    removed
}
