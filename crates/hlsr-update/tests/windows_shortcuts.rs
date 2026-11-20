//! Ярлыки и известные папки: решения установки (какой ярлык писать, какой удалять) на подделках,
//! сверка с `install()` 0.4.7 по фикстуре `windows-layout.json`, настоящие `.lnk` через COM — во
//! временном каталоге (`IShellLinkW`, `IPropertyStore`, AUMID). Рабочий стол, меню «Пуск» и
//! профиль владельца не трогаются.

mod support;

use hlsr_update::windows::known_folders::{self, FakeKnownFolders, KnownFolders};
use hlsr_update::windows::layout::{self, App, Identity};
use hlsr_update::windows::shortcuts::{
    FakeShellLinks, ShellLinks, Shortcuts, wants_desktop_shortcut,
};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("windows-layout.json"));

const EXE: &str = "C:\\HLSR\\Launcher\\HLSR.exe";

fn production() -> Identity {
    layout::identity(None).unwrap()
}

fn setup(root: &Path) -> (Arc<FakeShellLinks>, Shortcuts) {
    let links = Arc::new(FakeShellLinks::new());
    let shortcuts =
        Shortcuts::new(links.clone(), &FakeKnownFolders::under(root), &production()).unwrap();
    (links, shortcuts)
}

#[test]
fn shortcut_paths_follow_the_known_folders() {
    let dir = tempfile::tempdir().unwrap();
    let (_, shortcuts) = setup(dir.path());
    assert_eq!(
        shortcuts.start_menu,
        dir.path()
            .join("AppData/Roaming/Microsoft/Windows/Start Menu/Programs/HLSR.lnk")
    );
    assert_eq!(shortcuts.desktop, dir.path().join("Desktop/HLSR.lnk"));

    let qa = layout::identity(Some(&App::qa())).unwrap();
    let qa_shortcuts = Shortcuts::new(
        Arc::new(FakeShellLinks::new()),
        &FakeKnownFolders::under(dir.path()),
        &qa,
    )
    .unwrap();
    assert!(qa_shortcuts.desktop.ends_with("Desktop/HLSR-QA.lnk"));
}

/// `wantsDesktopShortcut`: выбор человека действует при установке; при обновлении — запись реестра.
#[test]
fn desktop_shortcut_choice() {
    // Установка: выбор человека главнее реестра.
    assert!(wants_desktop_shortcut(Some("0"), false, Some(true)));
    assert!(!wants_desktop_shortcut(Some("1"), false, Some(false)));
    // Без выбора и при обновлении — реестр: нужен всегда, кроме «0».
    assert!(wants_desktop_shortcut(None, false, None));
    assert!(wants_desktop_shortcut(Some("1"), false, None));
    assert!(!wants_desktop_shortcut(Some("0"), false, None));
    assert!(!wants_desktop_shortcut(Some("0"), true, Some(true)));
    assert!(wants_desktop_shortcut(Some("1"), true, Some(false)));
    assert!(wants_desktop_shortcut(Some("да"), true, None));
}

#[test]
fn writing_a_shortcut_fills_every_field() {
    let dir = tempfile::tempdir().unwrap();
    let (links, shortcuts) = setup(dir.path());
    shortcuts.write(&shortcuts.start_menu, EXE).unwrap();
    // Каталоги создаются по пути.
    assert!(shortcuts.start_menu.is_file());
    let info = links.read(&shortcuts.start_menu).unwrap();
    assert_eq!(info.target, EXE);
    assert_eq!(info.cwd, "C:\\HLSR\\Launcher");
    assert_eq!(info.icon, EXE);
    assert_eq!(info.icon_index, 0);
    assert_eq!(info.app_user_model_id, "com.hlsrteam.hlsr");
    assert_eq!(info.description, "HLSR");
}

#[test]
fn a_failed_write_names_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let (links, shortcuts) = setup(dir.path());
    links.set_fail_writes(true);
    let error = shortcuts.write(&shortcuts.desktop, EXE).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("Не удалось создать ярлык {}", shortcuts.desktop.display())
    );
    // install() собирает ошибки в предупреждения и идёт дальше.
    let warnings = shortcuts.install(EXE, false, true);
    assert_eq!(warnings.len(), 2);
    assert!(warnings[0].starts_with("Не удалось создать ярлык "));
}

#[test]
fn only_our_own_shortcuts_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    let (links, shortcuts) = setup(dir.path());
    shortcuts.write(&shortcuts.start_menu, EXE).unwrap();
    // На рабочем столе чужой HLSR.lnk (другая установка).
    shortcuts
        .write(&shortcuts.desktop, "D:\\Other\\HLSR\\HLSR.exe")
        .unwrap();
    assert!(shortcuts.points_to(&shortcuts.start_menu, "c:\\hlsr\\launcher\\hlsr.exe"));
    assert!(!shortcuts.points_to(&shortcuts.desktop, EXE));
    assert!(!shortcuts.points_to(&dir.path().join("нет.lnk"), EXE));
    assert!(shortcuts.remove_own(EXE).is_empty());
    assert!(!shortcuts.start_menu.exists());
    assert!(shortcuts.desktop.exists(), "чужой ярлык остался на месте");
    assert_eq!(links.written().len(), 2);
}

/// `install()`: недостающие ярлыки при обновлении восстанавливаются, существующие не трогаются.
#[test]
fn install_writes_missing_shortcuts_only_on_update() {
    let dir = tempfile::tempdir().unwrap();
    let (links, shortcuts) = setup(dir.path());
    assert!(shortcuts.install(EXE, false, false).is_empty());
    assert_eq!(links.written(), vec![shortcuts.start_menu.clone()]);

    // Обновление: меню уже есть — не трогаем; рабочий стол нужен и отсутствует — пишем.
    assert!(shortcuts.install(EXE, true, true).is_empty());
    assert_eq!(
        links.written(),
        vec![shortcuts.start_menu.clone(), shortcuts.desktop.clone()]
    );
    // Второе обновление ничего не пишет.
    assert!(shortcuts.install(EXE, true, true).is_empty());
    assert_eq!(links.written().len(), 2);
    // Установка человеком пишет заново (замена).
    shortcuts.install(EXE, false, true);
    assert_eq!(links.written().len(), 4);
}

/// Какие ярлыки писал настоящий `install()` 0.4.7 в каждом сценарии фикстуры.
#[test]
fn shortcut_decisions_match_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["installs"]) {
        let label = str_of(&case["label"]);
        let app = App {
            app_id: str_of(&case["app"]["appId"]).to_owned(),
            product_name: str_of(&case["app"]["productName"]).to_owned(),
        };
        let id = layout::identity(Some(&app)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let links = Arc::new(FakeShellLinks::new());
        let shortcuts =
            Shortcuts::new(links.clone(), &FakeKnownFolders::under(dir.path()), &id).unwrap();
        for existing in items(&case["existingShortcuts"]) {
            let file = match str_of(existing) {
                "startMenu" => &shortcuts.start_menu,
                _ => &shortcuts.desktop,
            };
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, "old").unwrap();
        }
        let update = case["update"].as_bool().unwrap();
        let registry_value =
            case["registryBefore"][id.install_key.as_str()][layout::DESKTOP_VALUE].as_str();
        let create_desktop =
            wants_desktop_shortcut(registry_value, update, case["desktopShortcut"].as_bool());
        let warnings = shortcuts.install(EXE, update, create_desktop);
        let written: Vec<&str> = links
            .written()
            .iter()
            .map(|file| {
                if *file == shortcuts.start_menu {
                    "startMenu"
                } else {
                    "desktop"
                }
            })
            .collect();
        mismatches.check_eq(
            written
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>(),
            items(&case["shortcutsWritten"])
                .iter()
                .map(|name| str_of(name).to_owned())
                .collect(),
            format!("{label}: записанные ярлыки"),
        );
        mismatches.check_eq(
            warnings.len(),
            items(&case["warnings"]).len(),
            format!("{label}: предупреждения"),
        );
    }
    mismatches.finish("install: ярлыки");
}

// ------------------------------------------------------------------- известные папки

#[test]
fn fake_known_folders_live_under_the_root() {
    let root = PathBuf::from("T");
    let folders = FakeKnownFolders::under(&root);
    assert_eq!(folders.desktop().unwrap(), root.join("Desktop"));
    assert_eq!(
        folders.appdata().unwrap(),
        root.join("AppData").join("Roaming")
    );
    assert_eq!(
        folders.local_appdata().unwrap(),
        root.join("AppData").join("Local")
    );
    assert_eq!(folders.temp(), root.join("Temp"));
    assert!(
        folders
            .start_menu_programs()
            .unwrap()
            .ends_with("Start Menu/Programs")
    );
    assert_eq!(
        known_folders::start_menu_of(Path::new(r"C:\Users\X\AppData\Roaming")),
        PathBuf::from(r"C:\Users\X\AppData\Roaming\Microsoft\Windows\Start Menu\Programs")
    );
}

/// `os.tmpdir()` Node 24 на Windows; ожидания сняты с настоящего Node на этой машине.
#[test]
fn node_tmpdir_rules() {
    let tmpdir = |pairs: &[(&str, &str)]| {
        let map: std::collections::HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        known_folders::node_tmpdir(&|name: &str| map.get(name).cloned())
    };
    assert_eq!(
        tmpdir(&[("TEMP", "C:\\Users\\X\\AppData\\Local\\Temp")]),
        "C:\\Users\\X\\AppData\\Local\\Temp"
    );
    assert_eq!(
        tmpdir(&[("TEMP", "C:\\Users\\X\\Temp\\")]),
        "C:\\Users\\X\\Temp"
    );
    assert_eq!(tmpdir(&[("TEMP", "C:\\")]), "C:\\");
    assert_eq!(tmpdir(&[("TEMP", "C:\\Temp\\\\")]), "C:\\Temp\\");
    assert_eq!(tmpdir(&[("TEMP", ""), ("TMP", "D:\\Tmp\\")]), "D:\\Tmp");
    assert_eq!(tmpdir(&[("TEMP", "x")]), "x");
    assert_eq!(tmpdir(&[("TEMP", "\\")]), "\\");
    assert_eq!(tmpdir(&[("TEMP", "Игры\\")]), "Игры");
    assert_eq!(tmpdir(&[("SystemRoot", "C:\\WINNT")]), "C:\\WINNT\\temp");
    assert_eq!(tmpdir(&[("windir", "E:\\W")]), "E:\\W\\temp");
    assert_eq!(tmpdir(&[]), "C:\\Windows\\temp");
}

// ------------------------------------------------------------------- настоящие .lnk

#[cfg(windows)]
mod system {
    use super::*;
    use hlsr_update::windows::known_folders::SystemKnownFolders;
    use hlsr_update::windows::shortcuts::{ShortcutSpec, SystemShellLinks};

    fn spec(target: &str) -> ShortcutSpec {
        ShortcutSpec {
            target: target.to_owned(),
            args: String::new(),
            cwd: Path::new(target)
                .parent()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            icon: target.to_owned(),
            icon_index: 0,
            app_user_model_id: "com.hlsrteam.hlsr".to_owned(),
            description: "HLSR".to_owned(),
        }
    }

    #[test]
    fn a_real_lnk_keeps_every_field() {
        let dir = tempfile::tempdir().unwrap();
        let links = SystemShellLinks;
        let file = dir.path().join("HLSR.lnk");
        links
            .write(&file, &spec("C:\\Fixture\\HLSR\\Launcher\\HLSR.exe"))
            .unwrap();
        let info = links.read(&file).unwrap();
        assert_eq!(info.target, "C:\\Fixture\\HLSR\\Launcher\\HLSR.exe");
        assert_eq!(info.cwd, "C:\\Fixture\\HLSR\\Launcher");
        assert_eq!(info.icon, "C:\\Fixture\\HLSR\\Launcher\\HLSR.exe");
        assert_eq!(info.icon_index, 0);
        assert_eq!(info.description, "HLSR");
        assert_eq!(info.args, "");
        assert_eq!(info.app_user_model_id, "com.hlsrteam.hlsr");
    }

    /// Ярлык игры (`GameActions.js`): аргументы сохраняются, AUMID не задан — его и нет.
    #[test]
    fn a_real_lnk_keeps_arguments_and_skips_an_empty_app_id() {
        let dir = tempfile::tempdir().unwrap();
        let links = SystemShellLinks;
        let file = dir.path().join("HLSR - Half-Life (WON).lnk");
        let game = ShortcutSpec {
            args: "--select-game=hl1-won".to_owned(),
            app_user_model_id: String::new(),
            description: "HLSR - Half-Life (WON)".to_owned(),
            ..spec("C:\\Игры\\HLSR\\HLSR.exe")
        };
        links.write(&file, &game).unwrap();
        let info = links.read(&file).unwrap();
        assert_eq!(info.target, game.target);
        assert_eq!(info.args, "--select-game=hl1-won");
        assert_eq!(info.description, "HLSR - Half-Life (WON)");
        assert_eq!(info.app_user_model_id, "");
    }

    #[test]
    fn a_real_lnk_is_replaced_and_survives_cyrillic() {
        let dir = tempfile::tempdir().unwrap();
        let links = SystemShellLinks;
        let file = dir.path().join("Игра HLSR.lnk");
        links
            .write(&file, &spec("C:\\Игры\\HLSR\\Launcher\\HLSR.exe"))
            .unwrap();
        links.write(&file, &spec("C:\\Другое\\HLSR.exe")).unwrap();
        let info = links.read(&file).unwrap();
        assert_eq!(info.target, "C:\\Другое\\HLSR.exe");
        assert_eq!(info.cwd, "C:\\Другое");
        assert!(links.read(&dir.path().join("нет.lnk")).is_err());
        assert!(
            links
                .write(
                    &dir.path().join("нет каталога").join("x.lnk"),
                    &spec("C:\\X\\a.exe")
                )
                .is_err()
        );
    }

    /// Ярлык, записанный через `Shortcuts`, узнаётся по цели без учёта регистра и удаляется.
    #[test]
    fn real_links_through_shortcuts() {
        let dir = tempfile::tempdir().unwrap();
        let shortcuts = Shortcuts::new(
            Arc::new(SystemShellLinks),
            &FakeKnownFolders::under(dir.path()),
            &production(),
        )
        .unwrap();
        assert!(shortcuts.install(EXE, false, true).is_empty());
        assert!(shortcuts.points_to(&shortcuts.start_menu, "c:\\hlsr\\launcher\\hlsr.exe"));
        assert!(shortcuts.points_to(&shortcuts.desktop, EXE));
        assert!(!shortcuts.points_to(&shortcuts.desktop, "C:\\Other\\HLSR.exe"));
        assert!(shortcuts.remove_own(EXE).is_empty());
        assert!(!shortcuts.start_menu.exists() && !shortcuts.desktop.exists());
    }

    /// Независимое чтение оболочкой: PowerShell (`WScript.Shell`, `Shell.Application`) видит цель,
    /// рабочий каталог, описание, значок и AUMID так же.
    #[test]
    fn the_shell_reads_the_same_lnk() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("HLSR.lnk");
        SystemShellLinks
            .write(&file, &spec("C:\\Fixture\\HLSR\\Launcher\\HLSR.exe"))
            .unwrap();
        let script = r#"
$ErrorActionPreference = 'Stop'
$lnk = (New-Object -ComObject WScript.Shell).CreateShortcut($env:HLSR_TEST_LNK)
$item = (New-Object -ComObject Shell.Application).NameSpace((Split-Path $env:HLSR_TEST_LNK)).ParseName((Split-Path -Leaf $env:HLSR_TEST_LNK))
[Console]::OutputEncoding = [Text.Encoding]::UTF8
@($lnk.TargetPath, $lnk.WorkingDirectory, $lnk.Description, $lnk.IconLocation, $item.ExtendedProperty('System.AppUserModel.ID')) -join "`n"
"#;
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("HLSR_TEST_LNK", &file)
            .output();
        let Ok(output) = output else {
            eprintln!("пропуск: powershell не запустился");
            return;
        };
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8_lossy(&output.stdout).replace('\r', "");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines,
            vec![
                "C:\\Fixture\\HLSR\\Launcher\\HLSR.exe",
                "C:\\Fixture\\HLSR\\Launcher",
                "HLSR",
                "C:\\Fixture\\HLSR\\Launcher\\HLSR.exe,0",
                "com.hlsrteam.hlsr",
            ]
        );
    }

    /// Известные папки ОС определяются; рабочий стол владельца только называется, не открывается.
    #[test]
    fn system_known_folders_resolve() {
        let folders = SystemKnownFolders;
        for path in [
            folders.desktop().unwrap(),
            folders.appdata().unwrap(),
            folders.local_appdata().unwrap(),
            folders.start_menu_programs().unwrap(),
            folders.temp(),
        ] {
            assert!(path.is_absolute(), "{}", path.display());
        }
        assert!(
            folders
                .start_menu_programs()
                .unwrap()
                .ends_with("Microsoft\\Windows\\Start Menu\\Programs")
        );
    }
}
