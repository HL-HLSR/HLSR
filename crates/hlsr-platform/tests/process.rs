//! Чистые функции процессов против 0.4.7: фикстура `process.json` из
//! `tools/reference/platform/process.js` (`windows-tools.js`, `process-inspect.js`,
//! `GameProcessService.processKeyOf`, `qa-launch.js`).

mod support;

use hlsr_core::js::Platform;
use hlsr_delivery::environment::Environment;
use hlsr_platform::game_process_service::process_key_of;
use hlsr_platform::process_inspect::{
    ProcessInspector, SystemProcesses, Tasklist, WindowsInspector, parse_tasklist, same_path_on,
    same_start, to_process_list,
};
use hlsr_platform::windows_tools::{powershell_path, system_root};
use serde_json::{Value, json};
use std::sync::{Arc, LazyLock};
use support::{Mismatches, fixture, items, opt_str};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("process.json"));

fn env_of(vars: &Value) -> Environment {
    Environment::new(
        vars.as_object()
            .expect("env — объект")
            .iter()
            .map(|(name, value)| (name.clone(), value.as_str().unwrap_or_default().to_owned())),
        true,
    )
}

#[test]
fn windows_tools_match_0_4_7() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["windowsTools"]) {
        let env = env_of(&case["env"]);
        m.check(
            &json!({
                "env": case["env"],
                "systemRoot": system_root(&env),
                "powershellPath": powershell_path(&env),
            }),
            case,
            format!("env {}", case["env"]),
        );
    }
    m.finish("windows-tools");
}

/// Самотест `steam-detect-selftest.js` «powershell.exe везде вызывается по полному пути»: в Rust
/// PowerShell не запускается вовсе (решение 2 спецификации) — ни по имени, ни по пути.
#[test]
fn nobody_starts_powershell() {
    fn walk(dir: &std::path::Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                let starts = ["Command::new(\"powershell", "Command::new(&powershell"];
                if starts.iter().any(|call| text.contains(call)) {
                    found.push(path.display().to_string());
                }
            }
        }
    }
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(crates).unwrap() {
        let src = entry.unwrap().path().join("src");
        if src.is_dir() {
            walk(&src, &mut found);
        }
    }
    // Этот файл ищет такие вызовы и сам их не содержит; пути из `src` — только чужой код.
    assert!(found.is_empty(), "PowerShell запускается из {found:?}");
}

#[test]
fn parse_tasklist_matches_0_4_7() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["tasklist"]) {
        let found: Vec<Value> = parse_tasklist(case["output"].as_str().unwrap())
            .iter()
            .map(|item| item.to_value())
            .collect();
        m.check(
            &Value::Array(found),
            &case["result"],
            format!("parseTasklist({})", case["output"]),
        );
    }
    m.finish("parseTasklist");
}

#[test]
fn to_process_list_matches_0_4_7() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["processList"]) {
        m.check(
            &Value::Array(to_process_list(&case["data"])),
            &case["result"],
            format!("toProcessList({})", case["data"]),
        );
    }
    m.finish("toProcessList");
}

#[test]
fn same_path_matches_0_4_7_on_both_platforms() {
    let mut m = Mismatches::default();
    for (key, platform) in [("win32", Platform::Win32), ("linux", Platform::Linux)] {
        for case in items(&FIXTURE["samePath"][key]) {
            m.check_eq(
                same_path_on(platform, opt_str(&case["left"]), opt_str(&case["right"])),
                case["result"].as_bool().unwrap(),
                format!("samePath[{key}]({}, {})", case["left"], case["right"]),
            );
        }
    }
    m.finish("samePath");
}

/// 0.4.9 (A-6): итог 0.4.7, только без одного из времён (нет, пусто, не разобрать) — не совпадает.
#[test]
fn same_start_matches_0_4_9() {
    let parsed = |value: &Value| opt_str(value).and_then(hlsr_core::js::date_parse).is_some();
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["sameStart"]) {
        let strict = parsed(&case["expected"]) && parsed(&case["actual"]);
        m.check_eq(
            same_start(opt_str(&case["expected"]), opt_str(&case["actual"])),
            strict && case["result"].as_bool().unwrap(),
            format!("sameStart({}, {})", case["expected"], case["actual"]),
        );
    }
    m.finish("sameStart");
}

/// Система, которую не должны спрашивать: имя образа отвергается раньше.
struct NeverAsked;

impl SystemProcesses for NeverAsked {
    fn inspect(
        &self,
        _pid: u32,
    ) -> Result<Option<hlsr_platform::process_inspect::ProcessInfo>, String> {
        panic!("система не должна спрашиваться");
    }

    fn find_by_image_name(
        &self,
        image_name: &str,
    ) -> Result<Vec<hlsr_platform::process_inspect::ProcessInfo>, String> {
        panic!("система не должна спрашиваться про {image_name:?}");
    }
}

impl Tasklist for NeverAsked {
    fn run(&self, filter: &str) -> Result<String, String> {
        panic!("tasklist не должен спрашиваться: {filter}");
    }
}

#[test]
fn invalid_image_names_are_rejected_before_asking_the_system() {
    let inspector = WindowsInspector::new(Arc::new(NeverAsked), Arc::new(NeverAsked));
    for case in items(&FIXTURE["badImageNames"]) {
        assert_eq!(case["result"], json!([]), "0.4.7: {}", case["name"]);
        // Не строки (`null`, число, объект) до Rust не доходят: тип параметра — строка.
        if let Some(name) = case["name"].as_str() {
            assert!(inspector.find_by_image_name(name).is_empty(), "{name:?}");
        }
    }
}

#[test]
fn process_key_matches_0_4_7() {
    let mut m = Mismatches::default();
    for case in items(&FIXTURE["processKey"]) {
        // Ключ считается по `${pid}`: строковый и дробный PID в JS печатаются как есть.
        let pid = match &case["pid"] {
            Value::String(text) => hlsr_core::js::to_number(text),
            other => other.as_f64().unwrap(),
        };
        m.check_eq(
            process_key_of(
                pid,
                opt_str(&case["executablePath"]),
                opt_str(&case["startedAt"]),
            ),
            case["result"].as_str().unwrap().to_owned(),
            format!("processKeyOf({})", case),
        );
    }
    m.finish("processKeyOf");
}

#[cfg(any(debug_assertions, feature = "dev-hooks"))]
mod qa_launch {
    use super::*;
    use hlsr_platform::qa_launch::{DELAY_FLAG, parse_args, startup_delay};

    #[test]
    fn parse_args_matches_0_4_7() {
        let mut m = Mismatches::default();
        for case in items(&FIXTURE["qaParseArgs"]) {
            m.check(
                &json!(parse_args(case["raw"].as_str().unwrap())),
                &case["result"],
                format!("parseArgs({})", case["raw"]),
            );
        }
        m.finish("parseArgs");
        // `parseArgs(null)`, `parseArgs(undefined)`, `parseArgs(0)`: без значения — пусто.
        for case in items(&FIXTURE["qaParseArgsNullish"]) {
            assert_eq!(case["result"], json!([]), "{}", case["input"]);
        }
    }

    #[test]
    fn startup_delay_matches_0_4_7() {
        let mut m = Mismatches::default();
        for case in items(&FIXTURE["qaStartupDelay"]) {
            let vars: Vec<(String, String)> = if case["unset"].as_bool() == Some(true) {
                Vec::new()
            } else {
                vec![(
                    DELAY_FLAG.to_owned(),
                    case["value"].as_str().unwrap().to_owned(),
                )]
            };
            let env = Environment::new(vars, false);
            m.check(
                &hlsr_core::json_text::js_number(startup_delay(&env)),
                &case["result"],
                format!("startupDelay({})", case["value"]),
            );
        }
        m.finish("startupDelay");
    }
}
