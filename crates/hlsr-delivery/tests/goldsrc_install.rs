//! Персонализация HL1 на диске против 0.4.7: фикстура `goldsrc-install.json` из
//! `tools/reference/delivery/goldsrc-install.js`. Сценарий — данные (файлы и шаги), его выполняют
//! `goldsrc-personalization.js` в Node 24 и этот крейт на одинаковых деревьях во временных папках;
//! сравниваются ответ каждого шага и дерево файлов (путь, размер, sha256, текст latin1).
//!
//! Сценарии, помеченные `unix`, зависят от ОС (ссылки, права, обратная косая в имени) и на Windows
//! пропускаются. Язык Steam не читается (реестр пуст), как в Linux.

mod support;

use hlsr_delivery::cfg_search::{CfgSearch, SteamLanguage};
use hlsr_delivery::errors::Failure;
use hlsr_delivery::game_binds_install::{self, ImportSource, InstallImport, RemoveImport};
use hlsr_delivery::goldsrc_personalization::{self as gp, Applied};
use hlsr_delivery::manifests;
use hlsr_delivery::ports::FakeRegistry;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use support::scenario::{self, common_step, normalize_text, path_in, rel, temp_root};
use support::{Mismatches, fixture, items, opt_str, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("goldsrc-install.json"));

const MOD: &str = "Half-Life/valve_WON";

struct Runner {
    root: PathBuf,
    _temp: tempfile::TempDir,
    search: CfgSearch,
}

impl Runner {
    fn new() -> Runner {
        let (temp, root) = temp_root();
        // Реестра нет: язык Steam не читается, как в Linux.
        let search = CfgSearch::new(SteamLanguage::new(Arc::new(FakeRegistry::default())));
        Runner {
            root,
            _temp: temp,
            search,
        }
    }

    /// Ошибка шага так, как её описывает скрипт.
    fn failure(&self, failure: &Failure) -> Value {
        match failure {
            Failure::Delivery(error) => json!({"error": {
                "kind": "delivery",
                "code": error.code.as_str(),
                "message": normalize_text(&self.root, &error.message),
            }}),
            Failure::Raw(error) => json!({"error": {"kind": "raw", "code": error.code}}),
        }
    }

    fn outcome<T>(&self, result: Result<T, Failure>, map: impl FnOnce(T) -> Value) -> Value {
        match result {
            Ok(value) => json!({"ok": map(value)}),
            Err(failure) => self.failure(&failure),
        }
    }

    fn apply(&self, install: &std::path::Path, step: &Value) -> Value {
        let settings = &step["settings"];
        let result = match &step["config"] {
            Value::String(id) if id == "null" || id == "empty" => {
                gp::apply_to(&self.search, install, "", None, settings)
            }
            Value::String(id) => {
                let config = manifests::launch_config_of(id).expect("настройки запуска игры");
                gp::apply_to_install(&self.search, install, config, settings)
            }
            config => gp::apply_to(
                &self.search,
                install,
                opt_str(&config["gameDirectory"]).unwrap_or_default(),
                opt_str(&config["engine"]),
                settings,
            ),
        };
        self.outcome(result, |applied| match applied {
            Applied::Skipped => json!({"status": "skipped"}),
            Applied::Applied {
                file,
                binds_file,
                appended,
            } => json!({
                "status": "applied",
                "file": rel(&self.root, &file),
                "bindsFile": binds_file.map(|file| rel(&self.root, &file)),
                "appended": appended,
            }),
        })
    }

    fn step(&self, step: &Value) -> Value {
        if let Some(common) = common_step(&self.root, step) {
            return common;
        }
        let install: PathBuf = match opt_str(&step["installDir"]) {
            Some(dir) => path_in(&self.root, dir),
            None => self.root.clone(),
        };
        match str_of(&step["op"]) {
            "apply" => self.apply(&install, step),
            "ensureDefault" => self.outcome(
                gp::ensure_default_userconfig(
                    &self.search,
                    &install,
                    opt_str(&step["gameDirectory"]).unwrap_or(MOD),
                ),
                |replaced| json!(replaced),
            ),
            "import" => self.outcome(
                game_binds_install::install_import(
                    &self.search,
                    &install,
                    "hl1-won",
                    MOD,
                    ImportSource::Text(str_of(&step["text"])),
                    &[],
                ),
                |result| match result {
                    InstallImport::Unsupported => json!({"status": "unsupported", "files": null}),
                    InstallImport::Invalid => json!({"status": "invalid", "files": null}),
                    InstallImport::Ok { files } => json!({"status": "ok", "files": files}),
                },
            ),
            "removeImport" => self.outcome(
                game_binds_install::remove_import(&self.search, &install, "hl1-won", MOD),
                |result| match result {
                    RemoveImport::Unsupported => json!({"status": "unsupported"}),
                    RemoveImport::Ok { .. } => json!({"status": "ok"}),
                },
            ),
            other => panic!("неизвестный шаг {other}"),
        }
    }
}

#[test]
fn apply_to_install_and_the_default_userconfig_on_scenarios() {
    let scenarios = items(&FIXTURE["scenarios"]);
    assert!(scenarios.len() > 60, "сценариев: {}", scenarios.len());
    let mut mismatches = Mismatches::default();
    for scenario in scenarios {
        let name = str_of(&scenario["name"]);
        if scenario["unix"].as_bool().unwrap_or(false) && cfg!(windows) {
            continue;
        }
        if name.starts_with("права") && scenario::is_root_user() {
            continue;
        }
        let runner = Runner::new();
        scenario::setup(&runner.root, &scenario["setup"]);
        assert_eq!(
            items(&scenario["steps"]).len(),
            items(&scenario["results"]).len(),
            "шагов и результатов поровну"
        );
        for (index, step) in items(&scenario["steps"]).iter().enumerate() {
            let actual = runner.step(step);
            mismatches.check(
                &actual,
                &scenario["results"][index],
                format!("{name}: шаг {index} {step}"),
            );
        }
        scenario::open_up(&runner.root);
    }
    mismatches.finish("goldsrc-personalization: сценарии");
}

#[test]
fn the_hashes_of_replaceable_userconfigs_are_those_of_the_scenarios() {
    use hlsr_delivery::hl1_default_userconfig::REPLACEABLE_SHA256;
    let constants = &FIXTURE["constants"];
    for key in ["stockSha256", "ru040Sha256", "previousSha256"] {
        let hash = str_of(&constants[key]);
        assert!(REPLACEABLE_SHA256.contains(&hash), "{key}: {hash}");
    }
}
