//! Интеграция с рабочим столом Linux против 0.4.7: фикстура `desktop-integration.json`
//! (`tools/reference/update/desktop-integration.js`).
//!
//! Везде — байты: скрипт команды `hlsr` (`cliScript`), экранирование `Exec` (`desktopExecPath`) и текст
//! `hlsr.desktop`, который записал 0.4.7. На Linux — ещё сценарии `ensureAppImageDesktop`,
//! `ensureManagedCli`, `managedCliPath` и обновления кешей меню на настоящей ФС во временном корне (пути
//! фикстуры — от этого корня): итог, файлы после вызова (текст, режим, ссылки) и команды.

mod support;

use hlsr_update::linux::desktop_integration;
use serde_json::{Value, json};
use std::sync::LazyLock;
use support::{Mismatches, fixture, items, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("desktop-integration.json"));

#[test]
fn cli_scripts_match_0_4_7() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["scripts"]) {
        let root = str_of(&case["root"]);
        mismatches.check(
            &json!(desktop_integration::cli_script(root)),
            &case["script"],
            format!("cliScript «{root}»"),
        );
    }
    mismatches.finish("cliScript");
}

#[test]
fn exec_paths_match_0_4_7() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["execPaths"]) {
        let file = str_of(&case["file"]);
        mismatches.check(
            &json!(desktop_integration::desktop_exec_path(file)),
            &case["exec"],
            format!("desktopExecPath {file:?}"),
        );
    }
    mismatches.finish("desktopExecPath");
}

#[test]
fn desktop_entries_match_0_4_7() {
    let mut checked = 0;
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["desktop"]) {
        let executable = str_of(&case["options"]["executable"]);
        for op in items(&case["ops"]) {
            if op[0] == "write" && str_of(&op[1]).contains("/applications/hlsr.desktop") {
                let exec = desktop_integration::desktop_exec_path(executable).unwrap();
                mismatches.check(
                    &json!(desktop_integration::desktop_entry(&exec)),
                    &op[2],
                    format!("hlsr.desktop «{}»", str_of(&case["name"])),
                );
                mismatches.check(&op[3], &json!(0o644), "режим hlsr.desktop");
                checked += 1;
            }
        }
    }
    mismatches.finish("hlsr.desktop");
    assert!(checked >= 5, "сверено записей: {checked}");
}

#[cfg(target_os = "linux")]
mod scenarios {
    //! Сценарии на настоящей ФС: путь фикстуры `/a/b` — `<временный корень>/a/b`.

    use super::*;
    use hlsr_update::linux::{FakeRunner, desktop_integration::AppImageDesktop};
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    struct Fs {
        dir: tempfile::TempDir,
    }

    impl Fs {
        fn new(files: &Value) -> Fs {
            let fs = Fs {
                dir: tempfile::tempdir().unwrap(),
            };
            for (path, node) in files.as_object().unwrap() {
                let target = fs.map(path);
                std::fs::create_dir_all(target.parent().unwrap()).unwrap();
                if node["type"] == "symlink" {
                    let link = str_of(&node["target"]);
                    let link = if link.starts_with('/') {
                        fs.map(link)
                    } else {
                        PathBuf::from(link)
                    };
                    std::os::unix::fs::symlink(link, &target).unwrap();
                } else {
                    std::fs::write(&target, str_of(&node["data"])).unwrap();
                    let mode = node["mode"].as_u64().unwrap_or(0o644) as u32;
                    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode))
                        .unwrap();
                }
            }
            fs
        }

        fn root(&self) -> &Path {
            self.dir.path()
        }

        fn map(&self, path: &str) -> PathBuf {
            if path.starts_with('/') {
                PathBuf::from(format!("{}{path}", self.root().display()))
            } else {
                PathBuf::from(path)
            }
        }

        fn unmap(&self, value: &Value) -> Value {
            let root = self.root().to_string_lossy().into_owned();
            match value {
                Value::String(text) => Value::String(text.replace(&root, "")),
                Value::Array(list) => {
                    Value::Array(list.iter().map(|item| self.unmap(item)).collect())
                }
                Value::Object(map) => Value::Object(
                    map.iter()
                        .map(|(k, v)| (k.clone(), self.unmap(v)))
                        .collect(),
                ),
                other => other.clone(),
            }
        }

        /// Файлы и ссылки (без каталогов), как `tree` модели скрипта.
        fn tree(&self) -> Value {
            let mut out = Vec::new();
            fn walk(fs: &Fs, dir: &Path, out: &mut Vec<Value>) {
                for entry in std::fs::read_dir(dir).unwrap() {
                    let entry = entry.unwrap();
                    let path = entry.path();
                    let meta = std::fs::symlink_metadata(&path).unwrap();
                    let shown = fs.unmap(&json!(path.to_string_lossy()));
                    if meta.file_type().is_symlink() {
                        let target = std::fs::read_link(&path)
                            .unwrap()
                            .to_string_lossy()
                            .into_owned();
                        out.push(json!({"path": shown, "symlink": fs.unmap(&json!(target))}));
                    } else if meta.is_dir() {
                        walk(fs, &path, out);
                    } else {
                        let data = std::fs::read_to_string(&path).unwrap();
                        out.push(json!({"path": shown, "data": fs.unmap(&json!(data)), "mode": meta.permissions().mode() & 0o777}));
                    }
                }
            }
            walk(self, self.root(), &mut out);
            out.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
            Value::Array(out)
        }
    }

    fn spawned(runner: &FakeRunner, fs: &Fs) -> Value {
        Value::Array(
            runner
                .calls()
                .iter()
                .map(|(command, timeout)| {
                    json!({"command": command.file.to_string_lossy(), "args": fs.unmap(&json!(command.args)), "timeout": timeout.as_millis() as u64})
                })
                .collect(),
        )
    }

    fn expected_spawned(case: &Value) -> Value {
        Value::Array(
            items(&case["spawned"])
                .iter()
                .map(|item| json!({"command": item["command"], "args": item["args"], "timeout": item["timeout"]}))
                .collect(),
        )
    }

    fn env() -> Vec<(String, String)> {
        vec![("HLSR_KEEP".to_owned(), "1".to_owned())]
    }

    #[test]
    fn ensure_app_image_desktop_matches_0_4_7() {
        let mut mismatches = Mismatches::default();
        for case in items(&FIXTURE["desktop"]) {
            let name = str_of(&case["name"]);
            let fs = Fs::new(&case["files"]);
            let options = &case["options"];
            let runner = FakeRunner::new();
            let result = desktop_integration::ensure_app_image_desktop(
                &AppImageDesktop {
                    executable: fs
                        .map(str_of(&options["executable"]))
                        .to_string_lossy()
                        .into_owned(),
                    icon_source: fs.map(str_of(&options["iconSource"])),
                    data_home: fs.map(str_of(&options["dataHome"])),
                    refresh: options["refresh"].as_bool() != Some(false),
                },
                &runner,
                &env(),
            );
            let result = match result {
                Ok(value) => json!({"ok": value}),
                Err(error) => json!({"error": error}),
            };
            mismatches.check(
                &fs.unmap(&result),
                &case["result"],
                format!("«{name}»: итог"),
            );
            mismatches.check(&fs.tree(), &case["tree"], format!("«{name}»: файлы"));
            mismatches.check(
                &spawned(&runner, &fs),
                &expected_spawned(case),
                format!("«{name}»: команды"),
            );
        }
        mismatches.finish("ensureAppImageDesktop");
    }

    #[test]
    fn managed_cli_matches_0_4_7() {
        let mut mismatches = Mismatches::default();
        for case in items(&FIXTURE["cli"]) {
            let name = str_of(&case["name"]);
            let fs = Fs::new(&case["files"]);
            let options = &case["options"];
            let result = desktop_integration::ensure_managed_cli(
                &fs.map(str_of(&options["installRoot"])),
                &fs.map(str_of(&options["home"])),
            );
            let result = match result {
                Ok(value) => json!({"ok": value}),
                Err(error) => json!({"error": error}),
            };
            mismatches.check(
                &fs.unmap(&result),
                &case["result"],
                format!("ensureManagedCli «{name}»: итог"),
            );
            mismatches.check(
                &fs.tree(),
                &case["tree"],
                format!("ensureManagedCli «{name}»: файлы"),
            );
        }
        for case in items(&FIXTURE["managedCli"]) {
            let name = str_of(&case["name"]);
            let fs = Fs::new(&case["files"]);
            let options = &case["options"];
            let result = desktop_integration::managed_cli_path(
                &fs.map(str_of(&options["installRoot"])),
                &fs.map(str_of(&options["home"])),
            );
            let result = json!({"ok": result.map(|path| path.to_string_lossy().into_owned())});
            mismatches.check(
                &fs.unmap(&result),
                &case["result"],
                format!("managedCliPath «{name}»"),
            );
        }
        mismatches.finish("ensureManagedCli");
    }

    #[test]
    fn menu_cache_commands_match_0_4_7() {
        let fs = Fs::new(&json!({}));
        let data = fs.map("/home/user/.local/share");
        let source_env = vec![
            (
                "APPIMAGE".to_owned(),
                "/tmp/.mount/HLSR.AppImage".to_owned(),
            ),
            ("APPDIR".to_owned(), "/tmp/.mount".to_owned()),
            ("LD_LIBRARY_PATH".to_owned(), "/tmp/.mount/lib".to_owned()),
            ("LD_PRELOAD".to_owned(), "x.so".to_owned()),
            ("QT_PLUGIN_PATH".to_owned(), "/q".to_owned()),
            ("QT_QPA_PLATFORM_PLUGIN_PATH".to_owned(), "/qp".to_owned()),
            ("XDG_DATA_HOME".to_owned(), "/ignored".to_owned()),
            ("HLSR_KEEP".to_owned(), "1".to_owned()),
        ];
        let mut mismatches = Mismatches::default();
        for (index, case) in items(&FIXTURE["refresh"]).iter().enumerate() {
            let runner = FakeRunner::new();
            if index == 0 {
                desktop_integration::refresh_menu_caches(&data, &runner, &source_env);
            } else {
                desktop_integration::refresh_kde_menu_cache(&data, &runner, &source_env);
            }
            mismatches.check(
                &spawned(&runner, &fs),
                &expected_spawned(case),
                str_of(&case["name"]),
            );
            if index == 1 {
                for (command, _) in runner.calls() {
                    let mut env: Vec<(String, String)> = command.env.clone();
                    env.sort();
                    let shown: serde_json::Map<String, Value> = env
                        .into_iter()
                        .map(|(k, v)| (k, fs.unmap(&json!(v))))
                        .collect();
                    mismatches.check(
                        &Value::Object(shown),
                        &items(&case["spawned"])[0]["env"],
                        "окружение kbuildsycoca",
                    );
                }
            }
        }
        mismatches.finish("refresh");
    }
}
