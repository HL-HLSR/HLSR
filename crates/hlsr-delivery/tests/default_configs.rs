//! Выбор стандартного cfg по игре против 0.4.7: фикстура `default-configs.json` из
//! `tools/reference/delivery/default-configs.js` (`default-configs.js`): `autoexec.cfg` у Ghosting,
//! `userconfig.cfg` у HL1, прочие игры и HL1 без папки игры не трогаются. Сценарии — данные: файлы и шаги.

mod support;

use hlsr_core::js::Platform;
use hlsr_delivery::cfg_search::{CfgSearch, SteamLanguage};
use hlsr_delivery::default_configs::{ensure_default_config, ensure_default_config_in};
use hlsr_delivery::errors::Failure;
use hlsr_delivery::manifests;
use hlsr_delivery::ports::FakeRegistry;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use support::scenario::{self, common_step, normalize_text, path_in, temp_root};
use support::{Mismatches, fixture, items, opt_str, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("default-configs.json"));

fn failure(root: &Path, failure: &Failure) -> Value {
    match failure {
        Failure::Delivery(error) => json!({"error": {
            "kind": "delivery",
            "code": error.code.as_str(),
            "message": normalize_text(root, &error.message),
        }}),
        Failure::Raw(error) => json!({"error": {"kind": "raw", "code": error.code}}),
    }
}

fn step(root: &Path, search: &CfgSearch, step: &Value) -> Value {
    if let Some(common) = common_step(root, step) {
        return common;
    }
    assert_eq!(str_of(&step["op"]), "ensure");
    let install: PathBuf = match opt_str(&step["installDir"]) {
        Some(dir) => path_in(root, dir),
        None => root.to_path_buf(),
    };
    let game_id = str_of(&step["gameId"]);
    let result = match &step["config"] {
        Value::String(name) if name == "null" || name == "undefined" => {
            ensure_default_config(search, game_id, &install, None, Platform::Linux)
        }
        // Имя игры из manifests.js; для неизвестной игры настроек запуска нет — как `undefined`.
        Value::String(name) => ensure_default_config(
            search,
            game_id,
            &install,
            manifests::launch_config_of(name),
            Platform::Linux,
        ),
        // Свой объект настроек запуска: из него читается только папка игры.
        config => ensure_default_config_in(
            search,
            game_id,
            &install,
            opt_str(&config["gameDirectory"]).unwrap_or_default(),
            Platform::Linux,
        ),
    };
    match result {
        Ok(replaced) => json!({"ok": replaced}),
        Err(error) => failure(root, &error),
    }
}

#[test]
fn ensure_default_config_on_scenarios() {
    let scenarios = items(&FIXTURE["scenarios"]);
    assert!(scenarios.len() > 30, "сценариев: {}", scenarios.len());
    let mut mismatches = Mismatches::default();
    for scenario in scenarios {
        let name = str_of(&scenario["name"]);
        if scenario["unix"].as_bool().unwrap_or(false) && cfg!(windows) {
            continue;
        }
        if scenario::is_root_user() && scenario["steps"].to_string().contains("\"chmod\"") {
            continue;
        }
        let (temp, root) = temp_root();
        let search = CfgSearch::new(SteamLanguage::new(Arc::new(FakeRegistry::default())));
        scenario::setup(&root, &scenario["setup"]);
        assert_eq!(
            items(&scenario["steps"]).len(),
            items(&scenario["results"]).len(),
            "шагов и результатов поровну"
        );
        for (index, item) in items(&scenario["steps"]).iter().enumerate() {
            let actual = step(&root, &search, item);
            mismatches.check(
                &actual,
                &scenario["results"][index],
                format!("{name}: шаг {index} {item}"),
            );
        }
        scenario::open_up(&root);
        drop(temp);
    }
    mismatches.finish("default-configs: сценарии");
}
