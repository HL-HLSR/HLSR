//! Аргументы запуска: режим выбирается, как в `entry.js` и `index.js` 0.4.7.
//!
//! Подпроект 6 добавляет режимы установщика: `Setup` (Windows: `HLSR-Setup-<версия>.exe`, в
//! 0.4.7 — `PORTABLE_EXECUTABLE_FILE` от NSIS-обёртки) и `Bootstrap` (Linux: AppImage с вшитым
//! архивом, в 0.4.7 — отдельное приложение Electron `installer/bootstrap-main.js`), и разбор
//! аргументов установщика и удаления как `windows-install/layout.js:parseArgs`.

use crate::js;
use std::ffi::OsString;

const UPDATER_FLAG: &str = "--hlsr-updater";
const UNINSTALL_FLAG: &str = "--hlsr-uninstall";
/// Путь к установщику, из которого запущено удаление: «Назад» снова откроет его, когда
/// каталог программы будет удалён (`layout.RETURN_SETUP_FLAG`).
const RETURN_SETUP_FLAG: &str = "--hlsr-return-setup";
const CLEANUP_FLAG: &str = "--hlsr-uninstall-cleanup";
const UPDATED_FLAG: &str = "--updated";
const FORCE_RUN_FLAG: &str = "--force-run";
const SELECT_GAME_ARG: &str = "--select-game=";
/// Переменная NSIS-обёртки electron-builder: по ней 0.4.7 узнаёт распакованный установщик.
/// Её же ставит апдейтер 0.4.7, когда обновляется чанками (`updater/core/install.js:88-94`).
pub const PORTABLE_EXECUTABLE_FILE: &str = "PORTABLE_EXECUTABLE_FILE";
/// Переменная среды выполнения AppImage: путь к самому AppImage.
pub const APPIMAGE: &str = "APPIMAGE";
/// Описание вшитого архива рядом с exe AppImage (`embedded-payload.js`: `resources/payload`).
pub const BOOTSTRAP_PAYLOAD: &str = "resources/payload/payload.json";

/// Что запускает этот процесс.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    Launcher,
    /// Удаление Windows-сборки (`HLSR.exe --hlsr-uninstall`).
    Uninstall,
    /// Очистка данных после удаления.
    UninstallCleanup,
    /// Внешний апдейтер Windows.
    Updater,
    /// Установщик Windows (`HLSR-Setup-<версия>.exe`): нагрузка в своём exe или
    /// `PORTABLE_EXECUTABLE_FILE` (`setup.detect`).
    Setup,
    /// Установщик AppImage (Linux): запуск из AppImage с вшитым `resources/payload`.
    Bootstrap,
}

/// Признаки среды, по которым выбираются режимы установщика: в 0.4.7 их читали `setup.detect`
/// (`PORTABLE_EXECUTABLE_FILE`) и отдельная точка входа AppImage. Отдельной структурой, чтобы
/// выбор режима проверялся без настоящего окружения.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModeEnv {
    /// Сборка и ОС — Windows: удаление, апдейтер и установщик есть только у неё.
    pub windows: bool,
    /// `PORTABLE_EXECUTABLE_FILE` задана и не пуста.
    pub portable_executable_file: bool,
    /// В конце своего exe есть нагрузка установщика (Р1; формат хвоста — `hlsr-update`,
    /// `windows::payload::has_trailer`; его проверяет приложение и кладёт сюда).
    pub setup_payload: bool,
    /// `APPIMAGE` задана и не пуста.
    pub appimage: bool,
    /// Рядом с exe лежит `resources/payload/payload.json`.
    pub bootstrap_payload: bool,
}

impl ModeEnv {
    /// Окружение без признаков установщика: только ОС.
    pub fn bare(windows: bool) -> ModeEnv {
        ModeEnv {
            windows,
            ..ModeEnv::default()
        }
    }

    /// Окружение этого процесса. Хвост нагрузки установщика не проверяется (`setup_payload`
    /// остаётся `false`): формат хвоста знает `hlsr-update`, приложение подставляет его само.
    pub fn from_process() -> ModeEnv {
        let set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
        let appimage = set(APPIMAGE);
        ModeEnv {
            windows: cfg!(windows),
            portable_executable_file: set(PORTABLE_EXECUTABLE_FILE),
            setup_payload: false,
            appimage,
            bootstrap_payload: appimage
                && std::env::current_exe()
                    .ok()
                    .and_then(|exe| exe.parent().map(|dir| dir.join(BOOTSTRAP_PAYLOAD)))
                    .is_some_and(|payload| payload.is_file()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchArgs {
    pub mode: LaunchMode,
    /// Аргументы установщика и удаления Windows (`layout.parseArgs`): есть в режимах `Setup` и
    /// `Uninstall`.
    pub setup: Option<SetupArgs>,
    /// Игра из ярлыка (`--select-game=<id>`).
    pub select_game: Option<String>,
    /// Все аргументы строками: их получает первый экземпляр.
    pub forwarded: Vec<String>,
}

/// Разобрать аргументы без `argv[0]` для текущей ОС и окружения процесса.
pub fn parse_args<I: IntoIterator<Item = OsString>>(args: I) -> LaunchArgs {
    parse_args_with(args, &ModeEnv::from_process())
}

/// [`parse_args`] для заданной ОС без признаков установщика: удаление и апдейтер есть только у
/// Windows.
pub fn parse_args_for<I: IntoIterator<Item = OsString>>(args: I, windows: bool) -> LaunchArgs {
    parse_args_with(args, &ModeEnv::bare(windows))
}

/// Порядок режимов: `entry.js` (апдейтер, раньше всего), затем `index.js:59-62` — удаление и
/// установщик Windows (`setup.detect`), и только потом очистка; установщик AppImage в 0.4.7 —
/// отдельное приложение, аргументы ему безразличны, поэтому он главнее очистки. Незнакомые
/// аргументы (`--no-sandbox`, `--force-run`, что угодно) принимаются молча.
pub fn parse_args_with<I: IntoIterator<Item = OsString>>(args: I, env: &ModeEnv) -> LaunchArgs {
    let windows = env.windows;
    let forwarded: Vec<String> = args
        .into_iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let has = |flag: &str| forwarded.iter().any(|arg| arg == flag);

    // layout.parseArgs не смотрит дальше /D=…, а `--hlsr-return-setup` забирает следующий аргумент.
    let setup = windows.then(|| parse_setup_args(&forwarded));
    let mode = if windows && has(UPDATER_FLAG) {
        LaunchMode::Updater
    } else if setup.as_ref().is_some_and(|args| args.uninstall) {
        LaunchMode::Uninstall
    } else if windows && (env.portable_executable_file || env.setup_payload) {
        LaunchMode::Setup
    } else if !windows && env.appimage && env.bootstrap_payload {
        LaunchMode::Bootstrap
    } else if has(CLEANUP_FLAG) {
        LaunchMode::UninstallCleanup
    } else {
        LaunchMode::Launcher
    };
    let setup = setup.filter(|_| matches!(mode, LaunchMode::Setup | LaunchMode::Uninstall));

    // GameActions.parseSelectGameArg: решает первый такой аргумент.
    let select_game = forwarded
        .iter()
        .find_map(|arg| arg.strip_prefix(SELECT_GAME_ARG))
        .filter(|id| is_game_id(id))
        .map(str::to_owned);

    LaunchArgs {
        mode,
        setup,
        select_game,
        forwarded,
    }
}

/// `^[a-z0-9]+(-[a-z0-9]+)*$`
fn is_game_id(id: &str) -> bool {
    id.split('-').all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    })
}

/// Аргументы установщика и удаления Windows: `layout.parseArgs` 0.4.7.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SetupArgs {
    /// `/S` в любом регистре.
    pub silent: bool,
    /// `--updated` (точно): запуск обновлятором.
    pub updated: bool,
    /// `--force-run` (точно): запустить лаунчер после установки.
    pub force_run: bool,
    /// `--hlsr-uninstall`.
    pub uninstall: bool,
    /// `--hlsr-return-setup <setup.exe>`.
    pub return_setup: Option<String>,
    /// `/D=<каталог>`: последний аргумент, путь до конца строки (с пробелами).
    pub dir: Option<String>,
    /// `/HLSRDELETEGAMES` в любом регистре (прежний деинсталлятор NSIS).
    pub delete_games: bool,
    /// `/HLSRDELETESETTINGS`.
    pub delete_settings: bool,
    /// `/HLSRDELETEPROFILE`.
    pub delete_profile: bool,
}

/// `parseArgs(argv)` без `argv[0]`: `/S` — тихий режим, `/D=<каталог>` — последний аргумент, путь
/// до конца строки; остальные аргументы, незнакомые тоже, пропускаются молча. Регистр у `/S` и
/// `/HLSRDELETE*` любой (`toUpperCase` JS и Rust одинаково переводят и `ſ` в `S`), у `--updated`,
/// `--force-run`, `--hlsr-uninstall`, `--hlsr-return-setup` и `/D=` — точное совпадение.
pub fn parse_setup_args<S: AsRef<str>>(args: &[S]) -> SetupArgs {
    let mut out = SetupArgs::default();
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_ref();
        if let Some(rest) = arg.strip_prefix("/D=") {
            let mut parts = vec![rest];
            parts.extend(args[index + 1..].iter().map(AsRef::as_ref));
            let joined = parts.join(" ");
            let dir = js::trim(&joined);
            out.dir = (!dir.is_empty()).then(|| dir.to_owned());
            break;
        }
        let upper = arg.to_uppercase();
        if upper == "/S" {
            out.silent = true;
        } else if arg == UPDATED_FLAG {
            out.updated = true;
        } else if arg == FORCE_RUN_FLAG {
            out.force_run = true;
        } else if arg == UNINSTALL_FLAG {
            out.uninstall = true;
        } else if arg == RETURN_SETUP_FLAG && index + 1 < args.len() {
            index += 1;
            out.return_setup = Some(args[index].as_ref().to_owned());
        } else if upper == "/HLSRDELETEGAMES" {
            out.delete_games = true;
        } else if upper == "/HLSRDELETESETTINGS" {
            out.delete_settings = true;
        } else if upper == "/HLSRDELETEPROFILE" {
            out.delete_profile = true;
        }
        index += 1;
    }
    out
}

impl SetupArgs {
    /// `dataCleanupArgs(args)`: флаги тихого удаления прежнего NSIS-деинсталлятора для очистки
    /// данных (`--hlsr-uninstall-cleanup`). `/HLSRDELETESETTINGS` сохраняет статистику, если не
    /// добавлен `/HLSRDELETEPROFILE`.
    pub fn data_cleanup_args(&self) -> Vec<&'static str> {
        if self.delete_settings {
            return if self.delete_profile {
                vec!["--settings"]
            } else {
                vec!["--settings", "--keep-profile"]
            };
        }
        if self.delete_profile {
            return vec!["--delete-profile"];
        }
        Vec::new()
    }
}
