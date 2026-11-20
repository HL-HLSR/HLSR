//! Уборка следов Electron 0.4.7–0.4.9 после перехода (Р12 спецификации, карта §0.10): на первом
//! запуске после подтверждения квитанции `receipt {from, to}` убираются копии внешнего апдейтера
//! Electron `%TEMP%\HLSR-Updater-*` любого возраста (0.4.x убирала их сама и только старше суток, а
//! каждая — больше 250 МБ), кеш electron-updater `%LOCALAPPDATA%\<имя>-updater` (у QA-сборки —
//! `hlsr-qa-updater`; в нём установщик ~125 МБ), профиль Chromium внешнего апдейтера
//! `%APPDATA%\HLSR Updater` и профиль установщика AppImage `~/.config/hlsr/installer`. У 0.4.8 и
//! 0.4.9 (Electron 45) следы те же: имя пакета, `updater-launch.js` и профиль установщика не
//! менялись.
//!
//! Из профиля Chromium лаунчера (каталог данных) — только кеши ([`CHROMIUM_CACHES`], общая
//! спецификация, «Данные пользователя», п. 4).
//!
//! Ручная установка поверх 0.4.x (окно `HLSR-Setup` или `/S` без квитанции апдейтера) оставляет
//! в каталоге данных метку [`SETUP_MARK`]; уборку по ней делает первый запуск новой версии, как
//! по квитанции, — не сам установщик: до этого запуска загрузка 0.4.x в кеше electron-updater
//! остаётся путём назад.
//!
//! Не трогается: `~/.local/opt/hlsr/versions/0.4.x-*` (это `previous`, откат), `Local Storage`,
//! `Local State`, `Cookies`, `online-session.json` в каталоге данных — до следующего обновления после
//! перехода (общая спецификация, «Данные пользователя»), и данные 0.4.9, которые читает новая
//! версия (`demo-tool.json`, `launch-days.json`… в `%APPDATA%\hlsr`, `%LOCALAPPDATA%\hlsr\demo-jobs`).
//! Ошибка уборки — только строка журнала.

use crate::ports::Logger;
use crate::updater::launch::is_stage_name;
use std::path::{Path, PathBuf};

/// Кеши Chromium в профиле лаунчера 0.4.x: Chromium и Dawn создают их заново, данных в них нет.
/// Лаунчер 0.5 в каталоге данных папок с этими именами не создаёт.
pub const CHROMIUM_CACHES: [&str; 9] = [
    "Cache",
    "Code Cache",
    "DawnCache",
    "DawnGraphiteCache",
    "DawnWebGPUCache",
    "GPUCache",
    "GPUPersistentCache",
    "GrShaderCache",
    "ShaderCache",
];

/// Метка в каталоге данных: установщик поставил новую версию поверх Electron 0.4.x.
pub const SETUP_MARK: &str = "electron-traces.pending";

/// В каталоге установки Windows — Electron 0.4.x: `resources\app.asar` есть у любой его сборки, у
/// 0.5 для Windows — нет (заглушка `app.asar` — только в архиве Linux, `hlsr-release archive`).
pub fn electron_install(dir: &Path) -> bool {
    dir.join("resources").join("app.asar").is_file()
}

/// Оставить [`SETUP_MARK`] после установки поверх 0.4.x. Ошибка — строка журнала: без метки
/// следы просто останутся, как раньше.
pub fn mark_setup(data_dir: &Path, log: &dyn Logger) {
    let written = std::fs::create_dir_all(data_dir)
        .and_then(|()| std::fs::write(data_dir.join(SETUP_MARK), b""));
    match written {
        Ok(()) => log.info("[setup] поставлено поверх 0.4.x: следы Electron уберёт первый запуск"),
        Err(error) => log.warn(&format!(
            "[setup] метка уборки следов 0.4.x не записана: {error}"
        )),
    }
}

/// Где искать следы 0.4.7–0.4.9. Каталог, которого нет, пропускается.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyDirs {
    /// `%TEMP%` (`os.tmpdir()`): копии `HLSR-Updater-<ms36>-<12hex>`.
    pub temp: Option<PathBuf>,
    /// Кеш electron-updater `%LOCALAPPDATA%\<имя>-updater` (`updaterCacheDirName` из
    /// `app-update.yml`: `hlsr-updater`, у QA-сборки `hlsr-qa-updater`).
    pub updater_cache: Option<PathBuf>,
    /// `%APPDATA%`: `HLSR Updater`.
    pub app_data: Option<PathBuf>,
    /// Каталог данных лаунчера на Linux (`~/.config/hlsr`): `installer`.
    pub linux_data_dir: Option<PathBuf>,
    /// Профиль Chromium лаунчера 0.4.x — каталог данных (`%APPDATA%\hlsr`, `~/.config/hlsr`):
    /// [`CHROMIUM_CACHES`].
    pub chromium_profile: Option<PathBuf>,
}

impl LegacyDirs {
    /// Windows: `%TEMP%`, кеш electron-updater, `%APPDATA%`.
    pub fn windows(
        temp: PathBuf,
        updater_cache: Option<PathBuf>,
        app_data: Option<PathBuf>,
    ) -> LegacyDirs {
        LegacyDirs {
            temp: Some(temp),
            updater_cache,
            app_data,
            ..LegacyDirs::default()
        }
    }

    /// Linux: каталог данных лаунчера.
    pub fn linux(data_dir: PathBuf) -> LegacyDirs {
        LegacyDirs {
            linux_data_dir: Some(data_dir.clone()),
            chromium_profile: Some(data_dir),
            ..LegacyDirs::default()
        }
    }
}

/// Что убрано и что не удалось.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Cleaned {
    pub removed: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, String)>,
}

/// Настоящий каталог (не ссылка и не файл).
fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .is_ok_and(|meta| meta.is_dir() && !meta.file_type().is_symlink())
}

/// Каталоги-следы, которые есть на диске, в порядке уборки.
pub fn targets(dirs: &LegacyDirs) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(temp) = &dirs.temp
        && let Ok(entries) = std::fs::read_dir(temp)
    {
        let mut stages: Vec<PathBuf> = entries
            .flatten()
            .filter(|entry| is_stage_name(&entry.file_name().to_string_lossy()))
            .map(|entry| entry.path())
            .filter(|path| is_real_dir(path))
            .collect();
        stages.sort();
        out.extend(stages);
    }
    let fixed = [
        dirs.updater_cache.clone(),
        dirs.app_data.as_ref().map(|dir| dir.join("HLSR Updater")),
        dirs.linux_data_dir
            .as_ref()
            .map(|dir| dir.join("installer")),
    ];
    out.extend(fixed.into_iter().flatten().filter(|path| is_real_dir(path)));
    if let Some(profile) = &dirs.chromium_profile {
        out.extend(
            CHROMIUM_CACHES
                .iter()
                .map(|name| profile.join(name))
                .filter(|path| is_real_dir(path)),
        );
    }
    out
}

/// Убрать следы 0.4.7–0.4.9. Каждый каталог — отдельно: занятый (ещё работающая копия апдейтера) не
/// мешает остальным.
pub fn cleanup_legacy_traces(dirs: &LegacyDirs, log: &dyn Logger) -> Cleaned {
    let mut cleaned = Cleaned::default();
    for target in targets(dirs) {
        match std::fs::remove_dir_all(&target) {
            Ok(()) => {
                log.info(&format!(
                    "[update] следы 0.4.7–0.4.9: убран {}",
                    target.display()
                ));
                cleaned.removed.push(target);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                log.warn(&format!(
                    "[update] следы 0.4.7–0.4.9: {} не убран: {error}",
                    target.display()
                ));
                cleaned.failed.push((target, error.to_string()));
            }
        }
    }
    cleaned
}

/// Уборка на запуске новой версии: если квитанция подтвердила эту версию (`confirmed`) или
/// установщик оставил метку `mark` ([`SETUP_MARK`]). Метка снимается, только когда убрано всё:
/// занятый след уберёт следующий запуск. `None` — убирать нечего.
pub fn cleanup_on_launch(
    dirs: &LegacyDirs,
    confirmed: bool,
    mark: &Path,
    log: &dyn Logger,
) -> Option<Cleaned> {
    let marked = mark.is_file();
    if !(confirmed || marked) {
        return None;
    }
    let cleaned = cleanup_legacy_traces(dirs, log);
    if marked
        && cleaned.failed.is_empty()
        && let Err(error) = std::fs::remove_file(mark)
    {
        log.warn(&format!("[update] метка {SETUP_MARK} не удалена: {error}"));
    }
    Some(cleaned)
}
