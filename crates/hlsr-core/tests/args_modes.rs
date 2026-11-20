//! Режимы установщика (подпроект 6, задача 1): `Setup`, `Bootstrap`, аргументы установщика и
//! удаления. Самотесты 0.4.7 из `windows-install-selftest.js` («parseArgs», «detect»,
//! «dataCleanupArgs», «updater mode»); сверка с модулями 0.4.7 на корпусе —
//! `crates/hlsr-update/tests/args.rs`.

use hlsr_core::args::{
    LaunchArgs, LaunchMode, ModeEnv, SetupArgs, parse_args_with, parse_setup_args,
};
use std::ffi::OsString;

fn parse(env: ModeEnv, list: &[&str]) -> LaunchArgs {
    parse_args_with(list.iter().map(OsString::from), &env)
}

fn windows() -> ModeEnv {
    ModeEnv::bare(true)
}

fn setup_env() -> ModeEnv {
    ModeEnv {
        portable_executable_file: true,
        ..windows()
    }
}

fn linux_appimage() -> ModeEnv {
    ModeEnv {
        appimage: true,
        bootstrap_payload: true,
        ..ModeEnv::bare(false)
    }
}

fn mode(env: ModeEnv, list: &[&str]) -> LaunchMode {
    parse(env, list).mode
}

// ------------------------------------------------------------ parseArgs

#[test]
fn setup_args_updater_and_uninstall_and_dir_to_the_end_of_the_line() {
    let update = parse_setup_args(&["/S", "--updated", "/D=C:\\Games\\My HLSR"]);
    assert!(update.silent);
    assert!(update.updated);
    assert_eq!(update.dir.as_deref(), Some("C:\\Games\\My HLSR"));

    let quick = parse_setup_args(&["--updated", "/s", "--force-run"]);
    assert!(quick.silent);
    assert!(quick.force_run);
    assert_eq!(quick.dir, None);

    let remove = parse_setup_args(&[
        "--hlsr-uninstall",
        "/S",
        "/HLSRDELETEGAMES",
        "/HLSRDELETESETTINGS",
    ]);
    assert!(remove.uninstall);
    assert!(remove.delete_games);
    assert!(remove.delete_settings);
    assert!(!remove.delete_profile);
}

#[test]
fn setup_args_dir_takes_the_rest_of_the_line_joined_with_spaces_and_trimmed() {
    // /D= — последний аргумент: всё, что после него, — продолжение пути.
    let args = parse_setup_args(&["/S", "/D=C:\\Program", "Files", "(x86)\\HLSR", "/S"]);
    assert_eq!(
        args.dir.as_deref(),
        Some("C:\\Program Files (x86)\\HLSR /S")
    );
    // Пробелы по краям срезаются как `String.prototype.trim` (в том числе U+00A0 и U+FEFF).
    let args = parse_setup_args(&["/D=\u{a0}C:\\HLSR", "\u{feff}"]);
    assert_eq!(args.dir.as_deref(), Some("C:\\HLSR"));
    // Пустой путь — не путь.
    assert_eq!(parse_setup_args(&["/D="]).dir, None);
    assert_eq!(parse_setup_args(&["/D=", "  "]).dir, None);
    // Флаги после /D= не разбираются.
    let args = parse_setup_args(&["/D=C:\\A", "--updated", "/S"]);
    assert!(!args.updated);
    assert!(!args.silent);
    // /D= — только с учётом регистра.
    assert_eq!(parse_setup_args(&["/d=C:\\A"]).dir, None);
}

#[test]
fn setup_args_case_rules() {
    let args = parse_setup_args(&[
        "/s",
        "/hlsrdeletegames",
        "/HlsrDeleteSettings",
        "/HLSRdeleteprofile",
    ]);
    assert!(args.silent && args.delete_games && args.delete_settings && args.delete_profile);
    // Точные флаги регистр не прощают.
    let args = parse_setup_args(&["--UPDATED", "--Force-Run", "--HLSR-UNINSTALL"]);
    assert_eq!(args, SetupArgs::default());
    // `toUpperCase` переводит и «ſ» (U+017F) в «S».
    assert!(parse_setup_args(&["/\u{17f}"]).silent);
    // «/S» должен быть целиком.
    assert!(!parse_setup_args(&["/SS", "S", "-S", "//S"]).silent);
}

#[test]
fn setup_args_return_setup_takes_the_next_argument() {
    let args = parse_setup_args(&[
        "--hlsr-uninstall",
        "--hlsr-return-setup",
        "C:\\Temp\\HLSR-Setup-0.5.0.exe",
    ]);
    assert!(args.uninstall);
    assert_eq!(
        args.return_setup.as_deref(),
        Some("C:\\Temp\\HLSR-Setup-0.5.0.exe")
    );
    // Без значения (последний аргумент) флаг ничего не берёт.
    assert_eq!(
        parse_setup_args(&["--hlsr-return-setup"]).return_setup,
        None
    );
    // Следующий аргумент забирается как есть, даже если он сам флаг.
    let args = parse_setup_args(&["--hlsr-return-setup", "--hlsr-uninstall"]);
    assert_eq!(args.return_setup.as_deref(), Some("--hlsr-uninstall"));
    assert!(!args.uninstall);
}

#[test]
fn unknown_arguments_are_accepted_silently() {
    // 0.4.7 на Linux перезапускает лаунчер как `hlsr --no-sandbox <прежние аргументы>`.
    let list = [
        "--no-sandbox",
        "--force-run",
        "--updated=1",
        "--whatever",
        "/X",
        "",
        "--select-game=hl1-won",
    ];
    for env in [windows(), ModeEnv::bare(false), linux_appimage()] {
        let parsed = parse(
            ModeEnv {
                bootstrap_payload: false,
                appimage: false,
                ..env
            },
            &list,
        );
        assert_eq!(parsed.mode, LaunchMode::Launcher, "{env:?}");
        assert_eq!(parsed.select_game.as_deref(), Some("hl1-won"));
        assert_eq!(parsed.forwarded.len(), list.len());
    }
    // `--force-run` — известный флаг установщика, остальное разбор не замечает.
    assert_eq!(
        parse_setup_args(&["--no-sandbox", "--updated=1", "--whatever", "/X", ""]),
        SetupArgs::default()
    );
    assert!(parse_setup_args(&list).force_run);
}

#[test]
fn data_cleanup_args_follow_the_old_nsis_uninstaller_flags() {
    let args = |settings, profile| SetupArgs {
        delete_settings: settings,
        delete_profile: profile,
        ..SetupArgs::default()
    };
    assert_eq!(args(false, false).data_cleanup_args(), Vec::<&str>::new());
    assert_eq!(
        args(true, false).data_cleanup_args(),
        ["--settings", "--keep-profile"]
    );
    assert_eq!(args(true, true).data_cleanup_args(), ["--settings"]);
    assert_eq!(args(false, true).data_cleanup_args(), ["--delete-profile"]);
    // Игры флаги очистки данных не затрагивают.
    let games = SetupArgs {
        delete_games: true,
        ..SetupArgs::default()
    };
    assert!(games.data_cleanup_args().is_empty());
}

// ------------------------------------------------------------ режимы

/// `detect`: установщик только из распакованного Setup или с флагом удаления.
#[test]
fn setup_mode_needs_the_payload_or_the_portable_executable_variable() {
    assert_eq!(mode(windows(), &[]), LaunchMode::Launcher);
    assert_eq!(mode(windows(), &["/S"]), LaunchMode::Launcher);
    assert_eq!(mode(setup_env(), &["/S"]), LaunchMode::Setup);
    assert_eq!(mode(setup_env(), &[]), LaunchMode::Setup);
    // Нагрузка в своём exe (Р1) — то же, что `PORTABLE_EXECUTABLE_FILE`.
    let payload = ModeEnv {
        setup_payload: true,
        ..windows()
    };
    assert_eq!(mode(payload, &["/S", "--updated"]), LaunchMode::Setup);
    // На Linux установщика Windows нет.
    let linux = ModeEnv {
        portable_executable_file: true,
        setup_payload: true,
        ..ModeEnv::bare(false)
    };
    assert_eq!(mode(linux, &["/S"]), LaunchMode::Launcher);
}

#[test]
fn setup_mode_carries_the_setup_args() {
    let parsed = parse(
        setup_env(),
        &[
            "--updated",
            "/S",
            "--force-run",
            "/D=C:\\HLSR Test\\Launcher",
        ],
    );
    let setup = parsed.setup.expect("аргументы установщика");
    assert!(setup.updated && setup.silent && setup.force_run);
    assert_eq!(setup.dir.as_deref(), Some("C:\\HLSR Test\\Launcher"));
    // В режиме лаунчера они не нужны.
    assert_eq!(parse(windows(), &["/S"]).setup, None);
}

#[test]
fn uninstall_wins_over_setup_and_cleanup_and_loses_to_the_updater() {
    // `detect`: `args.uninstall` проверяется раньше `PORTABLE_EXECUTABLE_FILE`.
    assert_eq!(
        mode(setup_env(), &["--hlsr-uninstall"]),
        LaunchMode::Uninstall
    );
    assert_eq!(
        mode(
            setup_env(),
            &["--hlsr-uninstall", "--hlsr-uninstall-cleanup"]
        ),
        LaunchMode::Uninstall
    );
    assert_eq!(
        mode(setup_env(), &["--hlsr-uninstall", "--hlsr-updater"]),
        LaunchMode::Updater
    );
    // Установщик главнее очистки (`index.js`: `windowsSetup` проверяется первым).
    assert_eq!(
        mode(setup_env(), &["--hlsr-uninstall-cleanup", "--games"]),
        LaunchMode::Setup
    );
    assert_eq!(
        mode(windows(), &["--hlsr-uninstall-cleanup", "--games"]),
        LaunchMode::UninstallCleanup
    );
}

#[test]
fn uninstall_carries_return_setup_and_the_old_uninstaller_flags() {
    let parsed = parse(
        windows(),
        &[
            "--hlsr-uninstall",
            "/S",
            "/HLSRDELETEGAMES",
            "--hlsr-return-setup",
            "C:\\Temp\\HLSR-Setup.exe",
        ],
    );
    assert_eq!(parsed.mode, LaunchMode::Uninstall);
    let setup = parsed.setup.expect("аргументы удаления");
    assert!(setup.uninstall && setup.silent && setup.delete_games);
    assert_eq!(
        setup.return_setup.as_deref(),
        Some("C:\\Temp\\HLSR-Setup.exe")
    );
    // `--hlsr-uninstall`, которое забрал `--hlsr-return-setup`, режим удаления не включает.
    assert_eq!(
        mode(windows(), &["--hlsr-return-setup", "--hlsr-uninstall"]),
        LaunchMode::Launcher
    );
}

/// «updater mode: only the flag on Windows switches the entry point».
#[test]
fn updater_mode_is_the_flag_on_windows_only() {
    assert_eq!(
        mode(windows(), &["--hlsr-updater", "--plan=C:\\p.json"]),
        LaunchMode::Updater
    );
    assert_eq!(
        mode(windows(), &["--plan=C:\\p.json"]),
        LaunchMode::Launcher
    );
    assert_eq!(
        mode(ModeEnv::bare(false), &["--hlsr-updater"]),
        LaunchMode::Launcher
    );
    // Флаг ищется среди всех аргументов, в том числе после /D=: `isUpdaterInvocation`.
    assert_eq!(
        mode(windows(), &["/D=C:\\A", "--hlsr-updater"]),
        LaunchMode::Updater
    );
    // Установщик Windows не включается и не мешает апдейтеру.
    assert_eq!(mode(setup_env(), &["--hlsr-updater"]), LaunchMode::Updater);
}

#[test]
fn bootstrap_mode_is_an_appimage_with_the_payload_next_to_the_executable() {
    assert_eq!(mode(linux_appimage(), &[]), LaunchMode::Bootstrap);
    assert_eq!(
        mode(linux_appimage(), &["--no-sandbox"]),
        LaunchMode::Bootstrap
    );
    // Без `payload.json` или без AppImage — обычный лаунчер.
    let no_payload = ModeEnv {
        bootstrap_payload: false,
        ..linux_appimage()
    };
    assert_eq!(mode(no_payload, &[]), LaunchMode::Launcher);
    let not_appimage = ModeEnv {
        appimage: false,
        ..linux_appimage()
    };
    assert_eq!(mode(not_appimage, &[]), LaunchMode::Launcher);
    // Установщик AppImage главнее очистки: аргументы ему безразличны.
    assert_eq!(
        mode(linux_appimage(), &["--hlsr-uninstall-cleanup"]),
        LaunchMode::Bootstrap
    );
    // На Windows AppImage нет.
    let windows_appimage = ModeEnv {
        windows: true,
        ..linux_appimage()
    };
    assert_eq!(mode(windows_appimage, &[]), LaunchMode::Launcher);
}

#[test]
fn modes_other_than_setup_and_uninstall_have_no_setup_args() {
    assert_eq!(parse(windows(), &["--hlsr-updater"]).setup, None);
    assert_eq!(parse(windows(), &["--hlsr-uninstall-cleanup"]).setup, None);
    assert_eq!(parse(linux_appimage(), &[]).setup, None);
}
