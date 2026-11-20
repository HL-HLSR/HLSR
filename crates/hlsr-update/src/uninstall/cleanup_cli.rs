//! `HLSR.exe --hlsr-uninstall-cleanup …` (`uninstall/cleanup-cli.js`): очистка данных после
//! удаления для скриптов, старых деинсталляторов и тихого удаления (`/HLSRDELETE*`). Флаги
//! `--games --purge --settings --keep-profile --delete-profile --print-library --library
//! --install-dir --report`; путь библиотеки — из `library.json` профиля; коды выхода — `CODES`
//! `library-cleanup.js`. Отчёт `--report`: для `--print-library` — путь в UTF-16LE без BOM, иначе
//! строки UTF-8 через CRLF.

use super::library_cleanup::{self as cleanup, CleanupOptions, EnvLookup, SteamLibraries};
use crate::errors::cleanup_exit as codes;
use hlsr_core::json_text::parse_js_json;
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;

/// Флаг режима.
pub const FLAG: &str = "--hlsr-uninstall-cleanup";

/// `isCleanupInvocation(argv)`: флаг среди аргументов (точно).
pub fn is_cleanup_invocation<S: AsRef<str>>(argv: &[S]) -> bool {
    argv.iter().any(|arg| arg.as_ref() == FLAG)
}

/// Разобранные флаги (`parseArgs`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanupArgs {
    pub games: bool,
    pub purge: bool,
    pub settings: bool,
    pub keep_profile: bool,
    pub delete_profile: bool,
    pub print_library: bool,
    /// `--library <путь>` или `--library=<путь>`; `None` — не задан (или нет значения).
    pub library: Option<String>,
    pub install_dir: Option<String>,
    pub report: Option<String>,
}

impl CleanupArgs {
    /// Вид 0.4.7: `purge`/`printLibrary` — только когда заданы, пути — когда есть значение.
    pub fn to_value(&self) -> Value {
        let mut out = json!({
            "games": self.games, "settings": self.settings,
            "keepProfile": self.keep_profile, "deleteProfile": self.delete_profile,
        });
        if self.purge {
            out["purge"] = json!(true);
        }
        if self.print_library {
            out["printLibrary"] = json!(true);
        }
        for (name, value) in [
            ("library", &self.library),
            ("installDir", &self.install_dir),
            ("report", &self.report),
        ] {
            if let Some(value) = value {
                out[name] = json!(value);
            }
        }
        out
    }
}

/// `parseArgs(argv)`: все аргументы (и `argv[0]`); `--library` без значения в конце — не задан.
pub fn parse_args<S: AsRef<str>>(argv: &[S]) -> CleanupArgs {
    let mut args = CleanupArgs::default();
    let mut index = 0;
    let next = |index: &mut usize| {
        *index += 1;
        argv.get(*index).map(|arg| arg.as_ref().to_owned())
    };
    while index < argv.len() {
        let arg = argv[index].as_ref();
        match arg {
            "--games" => args.games = true,
            "--purge" => args.purge = true,
            "--settings" => args.settings = true,
            "--keep-profile" => args.keep_profile = true,
            "--delete-profile" => args.delete_profile = true,
            "--print-library" => args.print_library = true,
            "--library" => args.library = next(&mut index),
            "--install-dir" => args.install_dir = next(&mut index),
            "--report" => args.report = next(&mut index),
            _ => {
                if let Some(value) = arg.strip_prefix("--library=") {
                    args.library = Some(value.to_owned());
                } else if let Some(value) = arg.strip_prefix("--install-dir=") {
                    args.install_dir = Some(value.to_owned());
                } else if let Some(value) = arg.strip_prefix("--report=") {
                    args.report = Some(value.to_owned());
                }
            }
        }
        index += 1;
    }
    args
}

/// `libraryPathFrom(userDataDir)`: непустой `libraryPath` из `library.json` (без пробелов по краям).
pub fn library_path_from(user_data: &Path) -> Option<String> {
    let bytes = hlsr_core::js::fs::read_file(&user_data.join("library.json")).ok()?;
    let data = parse_js_json(&hlsr_core::js::utf8_string(&bytes)).ok()?;
    let path = data.get("libraryPath")?.as_str()?;
    let trimmed = hlsr_core::js::trim(path);
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// Событие телеметрии «запущено удаление» (`reportUninstall` 0.4.7; реализация — ПП4
/// `report_uninstall`). Строки (`telemetry: …`) идут в отчёт через `say`.
pub trait UninstallTelemetry: Send + Sync {
    fn report(&self, user_data: &str, launcher_version: &str, say: &mut dyn FnMut(&str));
}

fn worst(a: i32, b: i32) -> i32 {
    if a == codes::OK || a == codes::NOTHING_TO_DO {
        b
    } else {
        a
    }
}

/// Параметры `run(options)`.
#[derive(Default)]
pub struct RunOptions<'a> {
    pub argv: Vec<String>,
    /// Не печатать в stdout.
    pub silent: bool,
    pub user_data: Option<String>,
    pub app_name: Option<String>,
    pub launcher_version: Option<String>,
    pub logs_dir: Option<String>,
    pub updater_cache: Option<String>,
    pub install_dir: Option<String>,
    /// `None` — окружение процесса.
    pub env: Option<EnvLookup<'a>>,
    pub home: Option<String>,
    pub steam: Option<&'a dyn SteamLibraries>,
    /// `None` — событие удаления не отправляется (`reportUninstall: false`).
    pub telemetry: Option<&'a dyn UninstallTelemetry>,
    pub is_alive: Option<&'a dyn Fn(u32) -> bool>,
}

/// Итог: код выхода и строки отчёта.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunResult {
    pub code: i32,
    pub lines: Vec<String>,
}

fn yes(flag: bool) -> &'static str {
    if flag { "yes" } else { "no" }
}

fn truthy(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|text| !text.is_empty())
}

/// `run(options)`: строки печатаются в `stdout` (если не `silent`) и пишутся в `--report`.
pub fn run(options: &RunOptions<'_>, stdout: &mut dyn Write) -> RunResult {
    let args = parse_args(&options.argv);
    let mut lines: Vec<String> = Vec::new();
    let silent = options.silent;
    let mut say = |text: &str, out: &mut dyn Write| {
        lines.push(text.to_owned());
        if !silent {
            let _ = writeln!(out, "{text}");
        }
    };
    let user_data = truthy(&options.user_data);
    let mut code = codes::NOTHING_TO_DO;

    if args.print_library {
        let library = user_data
            .and_then(|dir| library_path_from(Path::new(dir)))
            .unwrap_or_default();
        if let Some(report) = truthy(&args.report) {
            let bytes: Vec<u8> = library.encode_utf16().flat_map(u16::to_le_bytes).collect();
            if std::fs::write(report, bytes).is_err() {
                return RunResult {
                    code: codes::INTERNAL,
                    lines: Vec::new(),
                };
            }
        }
        if !silent {
            let _ = writeln!(stdout, "{library}");
        }
        return RunResult {
            code: if library.is_empty() {
                codes::NOTHING_TO_DO
            } else {
                codes::OK
            },
            lines: vec![library],
        };
    }

    say(
        &format!(
            "HLSR uninstall cleanup: games={} purge={} settings={} keepProfile={} deleteProfile={}",
            yes(args.games),
            yes(args.purge),
            yes(args.settings),
            yes(args.keep_profile),
            yes(args.delete_profile)
        ),
        stdout,
    );

    if let (Some(dir), Some(telemetry)) = (user_data, options.telemetry) {
        let version = truthy(&options.launcher_version).unwrap_or("0.0.0");
        let mut collected = Vec::new();
        telemetry.report(dir, version, &mut |text| collected.push(text.to_owned()));
        for text in collected {
            say(&text, stdout);
        }
    }

    let install_dir = truthy(&args.install_dir)
        .or(truthy(&options.install_dir))
        .map(str::to_owned);
    let base = || CleanupOptions {
        env: options.env,
        home: options.home.clone(),
        steam: options.steam,
        install_dir: install_dir.clone(),
        is_alive: options.is_alive,
        ..CleanupOptions::default()
    };

    if args.games {
        let library = truthy(&args.library)
            .map(str::to_owned)
            .or_else(|| user_data.and_then(|dir| library_path_from(Path::new(dir))));
        match library {
            None => {
                say(
                    "games: путь библиотеки неизвестен, ничего не удалено",
                    stdout,
                );
                code = worst(code, codes::NOTHING_TO_DO);
            }
            Some(library) => {
                let result = cleanup::cleanup_library(
                    &library,
                    &CleanupOptions {
                        purge: args.purge,
                        ..base()
                    },
                );
                if result.ok() {
                    let tail = if result.root_removed() {
                        " (корень удалён)"
                    } else {
                        " (корень сохранён)"
                    };
                    say(
                        &format!(
                            "games: очищено {} объект(ов) в {}{tail}",
                            result.removed(),
                            result.root().unwrap_or_default()
                        ),
                        stdout,
                    );
                } else {
                    say(
                        &format!(
                            "games: ОТКАЗ ({}) {} путь={}",
                            result.code(),
                            result.reason().unwrap_or_default(),
                            result.root().unwrap_or(&library)
                        ),
                        stdout,
                    );
                }
                code = worst(code, result.code());
            }
        }
    }

    if args.settings {
        let result = cleanup::cleanup_settings(&CleanupOptions {
            user_data: user_data.map(str::to_owned),
            logs_dir: options.logs_dir.clone(),
            updater_cache: options.updater_cache.clone(),
            app_name: options.app_name.clone(),
            preserve_profile: args.keep_profile,
            ..base()
        });
        if result.ok() {
            say(
                &format!("settings: очищено {} объект(ов)", result.removal.removed),
                stdout,
            );
        } else {
            say(
                &format!(
                    "settings: частичный отказ, не удалено {} объект(ов)",
                    result.removal.failed.len()
                ),
                stdout,
            );
        }
        code = worst(code, result.code);
    } else if args.delete_profile {
        let result = cleanup::cleanup_profile(&CleanupOptions {
            user_data: user_data.map(str::to_owned),
            ..base()
        });
        if result.ok() {
            say(
                &format!("profile: удалено {} объект(ов)", result.removal.removed),
                stdout,
            );
        } else {
            say(
                &format!(
                    "profile: отказ, не удалено {} объект(ов)",
                    result.removal.failed.len()
                ),
                stdout,
            );
        }
        code = worst(code, result.code);
    }

    if let Some(report) = truthy(&args.report) {
        let text = lines.join("\r\n") + "\r\n";
        let _ = std::fs::write(report, text);
    }
    RunResult { code, lines }
}
