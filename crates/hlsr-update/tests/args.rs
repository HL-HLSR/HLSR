//! Режимы запуска и аргументы установщика против 0.4.7: фикстура `args.json` из
//! `tools/reference/update/args.js` (`layout.parseArgs`, `setup.detect`, `setup.dataCleanupArgs`,
//! `updater-mode.isUpdaterInvocation`, `cleanup-cli.isCleanupInvocation` и порядок режимов
//! `entry.js` → `index.js`; коды `UPD_*`, коды выхода установщика и очистки). Сами функции живут в
//! `hlsr-core::args`; их самотесты 0.4.7 — `crates/hlsr-core/tests/args_modes.rs`.

mod support;

use hlsr_core::args::{LaunchMode, ModeEnv, SetupArgs, parse_args_with, parse_setup_args};
use hlsr_update::errors::{UpdCode, UpdaterError, cleanup_exit, setup_exit};
use serde_json::{Value, json};
use std::ffi::OsString;
use std::sync::LazyLock;
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("args.json"));

fn strings(value: &Value) -> Vec<String> {
    items(value).iter().map(|s| str_of(s).to_owned()).collect()
}

fn args_json(args: &SetupArgs) -> Value {
    json!({
        "silent": args.silent,
        "updated": args.updated,
        "forceRun": args.force_run,
        "uninstall": args.uninstall,
        "returnSetup": args.return_setup,
        "dir": args.dir,
        "deleteGames": args.delete_games,
        "deleteSettings": args.delete_settings,
        "deleteProfile": args.delete_profile,
    })
}

#[test]
fn parse_args_matches_node() {
    let cases = items(&FIXTURE["parseArgs"]);
    assert!(cases.len() >= 60, "в фикстуре {} случаев", cases.len());
    let mut mismatches = Mismatches::default();
    for case in cases {
        let argv = strings(&case["argv"]);
        mismatches.check(
            &args_json(&parse_setup_args(&argv)),
            &case["args"],
            format!("parseArgs({argv:?})"),
        );
    }
    mismatches.finish("layout.parseArgs");
}

fn mode_name(mode: LaunchMode) -> &'static str {
    match mode {
        LaunchMode::Updater => "updater",
        LaunchMode::Uninstall => "uninstall",
        LaunchMode::Setup => "setup",
        LaunchMode::UninstallCleanup => "cleanup",
        LaunchMode::Launcher => "launcher",
        // Установщика AppImage в 0.4.7 нет в аргументах: отдельное приложение.
        LaunchMode::Bootstrap => "bootstrap",
    }
}

#[test]
fn launch_modes_match_node() {
    let cases = items(&FIXTURE["modes"]);
    assert!(cases.len() >= 480, "в фикстуре {} случаев", cases.len());
    let mut mismatches = Mismatches::default();
    for case in cases {
        let argv = strings(&case["argv"]);
        let windows = str_of(&case["platform"]) == "win32";
        let portable = case["env"]["PORTABLE_EXECUTABLE_FILE"]
            .as_str()
            .is_some_and(|value| !value.is_empty());
        let env = ModeEnv {
            portable_executable_file: portable,
            ..ModeEnv::bare(windows)
        };
        let parsed = parse_args_with(argv.iter().map(OsString::from), &env);
        mismatches.check_eq(
            mode_name(parsed.mode).to_owned(),
            str_of(&case["mode"]).to_owned(),
            format!(
                "{} {:?} env={}",
                str_of(&case["platform"]),
                argv,
                case["env"]
            ),
        );
        // В режимах установщика и удаления аргументы разобраны так же, как `parseArgs`.
        let wants_setup = matches!(parsed.mode, LaunchMode::Setup | LaunchMode::Uninstall);
        mismatches.check_eq(
            parsed.setup.is_some(),
            wants_setup,
            format!("аргументы установщика {argv:?}"),
        );
        if let Some(setup) = parsed.setup {
            mismatches.check_eq(
                setup,
                parse_setup_args(&argv),
                format!("аргументы {argv:?}"),
            );
        }
    }
    mismatches.finish("режимы запуска");
}

#[test]
fn data_cleanup_args_match_node() {
    for case in items(&FIXTURE["dataCleanup"]) {
        let args = SetupArgs {
            delete_games: case["args"]["deleteGames"].as_bool().unwrap(),
            delete_settings: case["args"]["deleteSettings"].as_bool().unwrap(),
            delete_profile: case["args"]["deleteProfile"].as_bool().unwrap(),
            ..SetupArgs::default()
        };
        assert_eq!(
            json!(args.data_cleanup_args()),
            case["result"],
            "{}",
            case["args"]
        );
    }
}

#[test]
fn codes_match_node() {
    let constants = &FIXTURE["constants"];

    // UPD_*: те же имена и строки, ничего лишнего.
    let upd = constants["updaterErrorCodes"].as_object().unwrap();
    assert_eq!(upd.len(), UpdCode::ALL.len());
    for (name, value) in upd {
        let code = UpdCode::ALL
            .iter()
            .find(|code| code.name() == name)
            .unwrap_or_else(|| panic!("нет кода {name}"));
        assert_eq!(code.as_str(), str_of(value), "{name}");
    }

    let exit = &constants["setupExit"];
    assert_eq!(exit["OK"], setup_exit::OK);
    assert_eq!(exit["FAILED"], setup_exit::FAILED);
    assert_eq!(exit["IN_USE"], setup_exit::IN_USE);
    assert_eq!(exit["BAD_DIR"], setup_exit::BAD_DIR);
    assert_eq!(constants["setupExit"].as_object().unwrap().len(), 4);

    let cleanup = &constants["cleanupCodes"];
    for (name, value) in [
        ("OK", cleanup_exit::OK),
        ("USAGE", cleanup_exit::USAGE),
        ("NOTHING_TO_DO", cleanup_exit::NOTHING_TO_DO),
        ("REFUSED_PATH", cleanup_exit::REFUSED_PATH),
        ("REFUSED_MARKER", cleanup_exit::REFUSED_MARKER),
        ("REFUSED_REPARSE", cleanup_exit::REFUSED_REPARSE),
        ("REFUSED_RUNNING", cleanup_exit::REFUSED_RUNNING),
        ("PARTIAL", cleanup_exit::PARTIAL),
        ("INTERNAL", cleanup_exit::INTERNAL),
    ] {
        assert_eq!(cleanup[name], value, "{name}");
    }
    assert_eq!(cleanup.as_object().unwrap().len(), 9);
}

#[test]
fn updater_error_to_public_matches_node() {
    for case in items(&FIXTURE["toPublic"]) {
        let code = UpdCode::ALL
            .iter()
            .find(|code| code.as_str() == str_of(&case["code"]))
            .copied()
            .unwrap();
        let error = UpdaterError::new(code, case["message"].as_str().unwrap_or(""));
        assert_eq!(error.to_public(), case["result"], "{}", case["code"]);
    }
    // Обычная ошибка под запасным кодом.
    let wrapped = UpdaterError::wrap(&"диск полон", None);
    assert_eq!(wrapped.code, UpdCode::InstallFailed);
    assert_eq!(wrapped.message, "диск полон");
    let wrapped = UpdaterError::wrap(&"x", Some(UpdCode::RelaunchFailed));
    assert_eq!(wrapped.code, UpdCode::RelaunchFailed);
}
