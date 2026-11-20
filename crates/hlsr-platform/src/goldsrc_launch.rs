//! Запуск GoldSrc (`goldsrc-launch.js`): HL1 WON — мод описывает `liblist.gam`, `steam_appid.txt`
//! лежит рядом с `hl.exe`, а сама игра стартует только через инжектор Bunnymod XT. Функции — те
//! же, что у 0.4.7, над настоящей ФС; Linux-`LaunchService` берёт их через порт [`GoldSrc`]
//! ([`GoldSrcLaunch`]).

use crate::child_env::Env;
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self};
use hlsr_delivery::errors::{Code, DeliveryError, Failure};
use hlsr_delivery::file_ops::remove_file_sync;
use hlsr_delivery::game_binds::Encoding;
use hlsr_delivery::launch_args::{self, Settings};
use hlsr_delivery::manifests::LaunchConfig;
use hlsr_delivery::node_fs;
use hlsr_delivery::paths::assert_inside;
use hlsr_delivery::ports::Logger;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// `BXT_LOG_FILE`: лог Bunnymod XT в папке журналов лаунчера (один на все запуски).
pub const BXT_LOG_FILE: &str = "bxt.log";

/// `GOLDSRC_TOGGLES`: переключатели HL1 и их аргументы. Source-флаги (`-novid`, `-dx9`,
/// `-insecure`…) сюда не попадают: движок WON их не знает.
pub const GOLDSRC_TOGGLES: [(&str, &[&str]); 6] = [
    ("console", &["-console"]),
    ("windowed", &["-windowed"]),
    ("developer", &["-dev"]),
    ("condebug", &["-condebug"]),
    ("nojoy", &["-nojoy"]),
    ("noaafonts", &["-noaafonts"]),
];

/// Файлы GoldSrc (`resolveFiles`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoldSrcFiles {
    pub executable: PathBuf,
    pub game_directory: PathBuf,
    pub injector: PathBuf,
}

/// RInput (`resolveRInput`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RInputPlan {
    pub executable: PathBuf,
    /// Имя процесса игры, которого RInput ждёт (`hl.exe`).
    pub process_name: String,
    pub missing: bool,
}

/// `isGoldSrc(config)`.
pub fn is_goldsrc(config: &LaunchConfig) -> bool {
    config.engine == Some("goldsrc")
}

/// `String(value)`: поля нет — `undefined`.
fn js_text(value: Option<&str>) -> &str {
    value.unwrap_or("undefined")
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `insideInstall(installPath, relative, label)`.
fn inside_install(
    install_path: &Path,
    relative: &str,
    label: &str,
) -> Result<PathBuf, DeliveryError> {
    let path = NodePath::native();
    let joined = path.join(&[&text(install_path), &relative.replace('/', path.sep())]);
    assert_inside(install_path, Path::new(&joined), Some(label))
}

/// `resolveFiles(installPath, config)`: `hl.exe`, папка мода с `liblist.gam` и инжектор.
pub fn resolve_files(
    install_path: &Path,
    config: &LaunchConfig,
) -> Result<GoldSrcFiles, DeliveryError> {
    let executable = inside_install(install_path, config.executable, "Исполняемый файл игры")?;
    let game_directory = inside_install(install_path, config.game_directory, "Каталог мода")?;
    let injector = inside_install(
        install_path,
        js_text(config.injector),
        "Инжектор Bunnymod XT",
    )?;
    if !executable.exists() {
        return Err(DeliveryError::new(
            Code::GameExecutableMissing,
            format!("В установленной сборке нет {}", config.executable),
        )
        .with_details(json!({"executable": text(&executable)})));
    }
    if !game_directory.join("liblist.gam").exists() {
        return Err(DeliveryError::new(
            Code::GameDirectoryMissing,
            format!("В каталоге {} нет liblist.gam", config.game_directory),
        )
        .with_details(json!({"gameDirectory": text(&game_directory)})));
    }
    if !injector.exists() {
        return Err(DeliveryError::new(
            Code::GameExecutableMissing,
            format!("В установленной сборке нет {}", js_text(config.injector)),
        )
        .with_details(json!({"injector": text(&injector)})));
    }
    Ok(GoldSrcFiles {
        executable,
        game_directory,
        injector,
    })
}

/// `enforceSteamAppIdFile(installPath, config, fs, logger)`: старый `steam.dll` берёт AppID из
/// этого файла; без него игра не узнает, под какой лицензией её запустили.
pub fn enforce_steam_app_id_file(
    install_path: &Path,
    config: &LaunchConfig,
    logger: &dyn Logger,
) -> Result<PathBuf, DeliveryError> {
    let relative = js_text(config.steam_app_id_file);
    let file = inside_install(install_path, relative, "steam_appid.txt")?;
    let current = node_fs::read_text(&file, Encoding::Utf8).ok();
    let expected = config.steam_app_id.to_string();
    if let Some(current) = &current {
        let normalized = js::trim(current.strip_prefix('\u{feff}').unwrap_or(current));
        if normalized == expected {
            return Ok(file);
        }
    }
    if let Err(error) = node_fs::write_bytes(&file, format!("{expected}\n").as_bytes()) {
        return Err(DeliveryError::new(
            Code::SteamAppIdMismatch,
            format!("Не удалось записать steam_appid.txt: {}", error.message),
        )
        .with_details(json!({"file": text(&file)})));
    }
    logger.info(&format!("[launch] {relative} приведён к {expected}"));
    Ok(file)
}

/// Значение переключателя из [`GOLDSRC_TOGGLES`].
fn toggle(settings: &Settings, key: &str) -> bool {
    match key {
        "console" => settings.console,
        "windowed" => settings.windowed,
        "developer" => settings.developer,
        "condebug" => settings.condebug,
        "nojoy" => settings.nojoy,
        "noaafonts" => settings.noaafonts,
        _ => false,
    }
}

/// `mergeArguments(config, settings)`: доверенные аргументы, переключатели, разрешение и частота,
/// строка игрока. Если игрок сам вписал `-w`/`-h` или `-freq`, его значения главнее.
pub fn merge_arguments(config: &LaunchConfig, settings: &Value) -> Vec<String> {
    let value = launch_args::sanitize_settings(settings).settings;
    let user = launch_args::validate_args_string(&Value::String(value.launch_args.clone()))
        .unwrap_or_default();
    let user_names: HashSet<String> = user
        .iter()
        .filter_map(|token| launch_args::param_name_of(token))
        .collect();

    let mut result: Vec<String> = config
        .arguments
        .iter()
        .map(|arg| (*arg).to_owned())
        .collect();
    let mut seen: HashSet<String> = result.iter().map(|arg| arg.to_lowercase()).collect();
    let mut push = |result: &mut Vec<String>, token: &str| {
        if token.starts_with('-') && !seen.insert(token.to_lowercase()) {
            return;
        }
        result.push(token.to_owned());
    };

    for (key, tokens) in GOLDSRC_TOGGLES {
        if toggle(&value, key) {
            for token in tokens {
                push(&mut result, token);
            }
        }
    }
    if value.width != 0
        && value.height != 0
        && !user_names.contains("w")
        && !user_names.contains("h")
    {
        result.extend([
            "-w".to_owned(),
            value.width.to_string(),
            "-h".to_owned(),
            value.height.to_string(),
        ]);
    }
    if value.refresh_rate != 0 && !user_names.contains("freq") {
        result.extend(["-freq".to_owned(), value.refresh_rate.to_string()]);
    }
    for token in &user {
        push(&mut result, token);
    }
    result
}

/// `resolveRInput(installPath, config, settings)`: RInput подменяет ввод мыши на raw input и
/// внедряется в уже запущенный `hl.exe` отдельным процессом. По умолчанию включён.
pub fn resolve_rinput(
    install_path: &Path,
    config: &LaunchConfig,
    settings: &Value,
) -> Result<Option<RInputPlan>, DeliveryError> {
    let Some(relative) = config.rinput_executable else {
        return Ok(None);
    };
    if !launch_args::sanitize_settings(settings).settings.rinput {
        return Ok(None);
    }
    let executable = inside_install(install_path, relative, "RInput")?;
    let missing = !executable.exists();
    Ok(Some(RInputPlan {
        process_name: NodePath::native().basename(config.executable),
        executable,
        missing,
    }))
}

/// `/^_*[A-Z][A-Z0-9_]*$/`: имя переменной окружения из настроек запуска.
fn is_environment_name(name: &str) -> bool {
    let rest = name.trim_start_matches('_');
    let mut chars = rest.chars();
    chars.next().is_some_and(|first| first.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// `environmentOf(config)`: переменные окружения игры из настроек запуска.
pub fn environment_of(config: &LaunchConfig) -> Env {
    config
        .environment
        .iter()
        .filter(|(name, _)| is_environment_name(name))
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

/// `prepareBxtLog(logsDir, fs, logger)`: лог Bunnymod XT — что он нашёл в движке и `client.dll`.
/// Файл один на все запуски и удаляется перед каждым: BXT дописывает в конец. `None` — лога нет.
pub fn prepare_bxt_log(logs_dir: Option<&Path>, logger: &dyn Logger) -> Option<PathBuf> {
    let logs_dir = logs_dir.filter(|dir| !dir.as_os_str().is_empty())?;
    let path = NodePath::native();
    let file = PathBuf::from(path.join(&[&text(logs_dir), BXT_LOG_FILE]));
    let prepared = node_fs::mkdir_recursive(logs_dir)
        .map_err(Failure::from)
        .and_then(|()| remove_file_sync(&file, None));
    if let Err(error) = prepared {
        let message = match error {
            Failure::Delivery(error) => error.message,
            Failure::Raw(error) => error.message,
        };
        logger.warn(&format!("[launch] лог BXT не подготовлен: {message}"));
        return None;
    }
    logger.info(&format!("[launch] лог BXT: {}", text(&file)));
    Some(file)
}

/// `goldsrc-launch.js` для Linux-`LaunchService`: файлы, `steam_appid.txt`, аргументы, RInput,
/// окружение, журнал BXT.
pub trait GoldSrc: Send + Sync {
    fn resolve_files(
        &self,
        install_path: &Path,
        config: &LaunchConfig,
    ) -> Result<GoldSrcFiles, DeliveryError>;
    fn enforce_steam_app_id_file(
        &self,
        install_path: &Path,
        config: &LaunchConfig,
        logger: &dyn Logger,
    ) -> Result<PathBuf, DeliveryError>;
    fn merge_arguments(&self, config: &LaunchConfig, settings: &Value) -> Vec<String>;
    fn resolve_rinput(
        &self,
        install_path: &Path,
        config: &LaunchConfig,
        settings: &Value,
    ) -> Result<Option<RInputPlan>, DeliveryError>;
    fn environment_of(&self, config: &LaunchConfig) -> Env;
    fn prepare_bxt_log(&self, logs_dir: Option<&Path>, logger: &dyn Logger) -> Option<PathBuf>;
}

/// [`GoldSrc`] над настоящей ФС — функции этого модуля.
#[derive(Debug, Clone, Copy, Default)]
pub struct GoldSrcLaunch;

impl GoldSrc for GoldSrcLaunch {
    fn resolve_files(
        &self,
        install_path: &Path,
        config: &LaunchConfig,
    ) -> Result<GoldSrcFiles, DeliveryError> {
        resolve_files(install_path, config)
    }

    fn enforce_steam_app_id_file(
        &self,
        install_path: &Path,
        config: &LaunchConfig,
        logger: &dyn Logger,
    ) -> Result<PathBuf, DeliveryError> {
        enforce_steam_app_id_file(install_path, config, logger)
    }

    fn merge_arguments(&self, config: &LaunchConfig, settings: &Value) -> Vec<String> {
        merge_arguments(config, settings)
    }

    fn resolve_rinput(
        &self,
        install_path: &Path,
        config: &LaunchConfig,
        settings: &Value,
    ) -> Result<Option<RInputPlan>, DeliveryError> {
        resolve_rinput(install_path, config, settings)
    }

    fn environment_of(&self, config: &LaunchConfig) -> Env {
        environment_of(config)
    }

    fn prepare_bxt_log(&self, logs_dir: Option<&Path>, logger: &dyn Logger) -> Option<PathBuf> {
        prepare_bxt_log(logs_dir, logger)
    }
}
