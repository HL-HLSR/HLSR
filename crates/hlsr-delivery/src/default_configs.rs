//! Файл игрока по игре (`default-configs.js` 0.4.9): userconfig.cfg у HL1, autoexec.cfg у
//! Ghosting и сборок HL2. Единственный путь подготовки — [`prepare_config`]: переход с 0.4.8
//! (если нужен), иначе шаблон вместо отсутствующего или стокового. Зовётся первым везде, где
//! читают или пишут конфиги, — раньше любого ensure: иначе стандартный файл 0.4.8 теряет строки
//! exec до перехода.
//!
//! [`ensure_default_config`] — путь 0.4.7 (стандартный cfg без разделов); его зовёт только запуск
//! `hlsr-platform`, пока тот не перейдёт на [`prepare_for_launch`] (задача F2l).

use crate::bind_presets;
use crate::cfg_search::{CfgSearch, LanguageOption};
use crate::errors::Failure;
use crate::game_binds;
use crate::game_binds_install;
use crate::ghosting_personalization;
use crate::goldsrc_personalization;
use crate::hl1_guide;
use crate::manifests::LaunchConfig;
use crate::player_config::{self, Ctx, Error, PrepareOptions, Prepared};
use hlsr_core::js::Platform;
use serde_json::Value;
use std::path::Path;

/// `prepareConfig(gameId, installPath, launchConfig, options)`: `None` — у игры нет файла игрока.
pub fn prepare_config(
    search: &CfgSearch,
    game_id: &str,
    install_path: &Path,
    launch_config: Option<&LaunchConfig>,
    platform: Platform,
    options: &PrepareOptions,
) -> Result<Option<Prepared>, Error> {
    let game_directory = if game_id == "ghosting-renewed" {
        "ghostingmod"
    } else {
        match launch_config.map(|config| config.game_directory) {
            Some(directory)
                if bind_presets::engine_for(game_id).is_some() && !directory.is_empty() =>
            {
                directory
            }
            _ => return Ok(None),
        }
    };
    let ctx = Ctx {
        search,
        install: install_path,
        game_id,
        game_directory,
        platform,
    };
    player_config::prepare(&ctx, options).map(Some)
}

/// `prepareForLaunch(...)`: запуск игры — только подготовка файла игрока и строка auto_alias в
/// game.cfg для скриптов гайда. Сам файл игрока на запуске не переписывается.
pub fn prepare_for_launch(
    search: &CfgSearch,
    game_id: &str,
    install_path: &Path,
    launch_config: Option<&LaunchConfig>,
    settings: Option<&Value>,
    platform: Platform,
    options: &PrepareOptions,
) -> Result<Option<Prepared>, Error> {
    let options = PrepareOptions {
        settings,
        ..*options
    };
    let result = prepare_config(
        search,
        game_id,
        install_path,
        launch_config,
        platform,
        &options,
    )?;
    let binds = settings
        .filter(|settings| hlsr_core::js::truthy(settings))
        .and_then(|settings| settings.get("binds"))
        .unwrap_or(&Value::Null);
    let clean = game_binds::sanitize_overrides(binds);
    if result.is_some()
        && let Some(engine) = bind_presets::engine_for(game_id)
        && let Some(config) = launch_config
        && bind_presets::needs_auto_alias(game_id, platform, &clean)
    {
        let files = search.files_of(
            install_path,
            config.game_directory,
            engine,
            LanguageOption::Detect,
        )?;
        game_binds_install::ensure_exec_line(
            &files.path_of(hl1_guide::GAME_CFG),
            hl1_guide::AUTO_ALIAS,
            game_binds::engine_of(engine).encoding,
        )?;
    }
    Ok(result)
}

/// `ensureDefaultConfig(gameId, installPath, launchConfig)` 0.4.7: `true` — файл записан.
pub fn ensure_default_config(
    search: &CfgSearch,
    game_id: &str,
    install_path: &Path,
    launch_config: Option<&LaunchConfig>,
    platform: Platform,
) -> Result<bool, Failure> {
    let game_directory = launch_config.map_or("", |config| config.game_directory);
    ensure_default_config_in(search, game_id, install_path, game_directory, platform)
}

/// То же по папке игры из настроек запуска (пустая строка — настроек или папки нет).
pub fn ensure_default_config_in(
    search: &CfgSearch,
    game_id: &str,
    install_path: &Path,
    game_directory: &str,
    platform: Platform,
) -> Result<bool, Failure> {
    match game_id {
        "ghosting-renewed" => Ok(ghosting_personalization::ensure_default_autoexec(
            install_path,
            platform,
        )?),
        "hl1-won" if !game_directory.is_empty() => {
            goldsrc_personalization::ensure_default_userconfig(search, install_path, game_directory)
        }
        _ => Ok(false),
    }
}
