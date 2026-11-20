//! Proton против 0.4.7: фикстура `proton.json` (`tools/reference/platform/proton.js`).
//!
//! Ч — чистые функции на корпусе: список архива (`validateArchiveListing`), закреплённый релиз
//! (`safeReleaseAsset`), версии и порядок Wineland, кандидаты обновления, `.sha512sum`, путь umu,
//! очистка вывода команд в префиксе, TLS-ошибки (`net-fallback.js`), постоянные.
//! С — менеджер Proton на временном дереве: поиск, выбор, свои папки, переход на сборку HLSR,
//! проверка обновления и обновление настоящим архивом (`tests/fixtures/proton/*.tar.xz`,
//! распаковка системным `tar`) с поддельной сетью; результаты, события, журнал, байты
//! `proton-selection.json`, дерево `protons/`.

mod proton_support;
mod support;

use hlsr_core::js::path::NodePath;
use hlsr_delivery::ports::MemoryLogger;
use hlsr_platform::linux::prefix_command::{CommandOutput, WaitOptions, wait_for_prefix_command};
use hlsr_platform::linux::proton_manager::{
    self, ProtonEvent, ProtonManager, ProtonManagerOptions, compare_wineland, parse_sha512sum,
    safe_release_asset, update_candidate, validate_archive_listing, validate_archive_listing_with,
    wineland_version,
};
use hlsr_platform::linux::python_runtime;
use hlsr_platform::linux::user_namespaces::umu_root;
use hlsr_platform::net_fallback::is_tls_error;
use hlsr_platform::spawn::{Child, ChildControl, Env};
use proton_support::{FakeNet, FakeRequest, FakeText, Root, attempt, tree_of, write_file};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::{Arc, Mutex};
use support::{Mismatches, fixture, items, str_of};

fn proton() -> Value {
    fixture("proton.json")
}

#[test]
fn constants_match_node() {
    let fixture = proton();
    let constants = &fixture["pure"]["constants"];
    let mut m = Mismatches::default();
    m.check_eq(
        proton_manager::RELEASE_TAG,
        str_of(&constants["releaseTag"]),
        "RELEASE_TAG",
    );
    m.check_eq(
        proton_manager::RELEASES_API,
        str_of(&constants["releasesApi"]),
        "RELEASES_API",
    );
    m.check(
        &proton_manager::pinned_archive().to_json(),
        &constants["pinned"],
        "PINNED_ARCHIVE",
    );
    let python = python_runtime::distribution();
    m.check(
        &json!({"name": python.name, "size": python.size, "sha256": python.sha256, "url": python.url}),
        &constants["python"],
        "DISTRIBUTION",
    );
    m.check_eq(
        hlsr_platform::linux::prefix_command::OUTPUT_LIMIT as u64,
        constants["outputLimit"].as_u64().unwrap(),
        "OUTPUT_LIMIT",
    );
    m.check_eq(
        hlsr_platform::linux::prefix_command::DEFAULT_TIMEOUT_MS,
        constants["timeoutMs"].as_u64().unwrap(),
        "DEFAULT_TIMEOUT_MS",
    );
    m.check_eq(
        hlsr_platform::linux::prefix_command::DEFAULT_KILL_GRACE_MS,
        constants["killGraceMs"].as_u64().unwrap(),
        "DEFAULT_KILL_GRACE_MS",
    );
    m.finish("постоянные Proton");
}

#[test]
fn archive_listings_match_node() {
    let fixture = proton();
    let mut m = Mismatches::default();
    for case in items(&fixture["pure"]["listings"]) {
        let input = &case["input"];
        let (names, verbose, root) = (
            str_of(&input["names"]),
            str_of(&input["verbose"]),
            str_of(&input["root"]),
        );
        let result = if str_of(&input["pattern"]) == "python" {
            validate_archive_listing_with(names, verbose, root, |root| root == "python")
        } else {
            validate_archive_listing(names, verbose, root)
        };
        m.check(
            &attempt(result, |names| json!(names.len())),
            &case["result"],
            format!("validateArchiveListing {input}"),
        );
    }
    m.finish("список архива");
}

#[test]
fn pinned_release_matches_node() {
    let fixture = proton();
    let mut m = Mismatches::default();
    for case in items(&fixture["pure"]["releases"]) {
        let result = safe_release_asset(&case["input"]);
        m.check(
            &attempt(result, |archive| json!({"archive": archive})),
            &case["result"],
            format!("safeReleaseAsset {}", case["input"]),
        );
    }
    m.finish("закреплённый релиз");
}

#[test]
fn versions_and_order_match_node() {
    let fixture = proton();
    let mut m = Mismatches::default();
    for case in items(&fixture["pure"]["versions"]) {
        let name = str_of(&case["name"]);
        let actual = wineland_version(name).map_or(Value::Null, |version| version.to_json());
        m.check(
            &actual,
            &case["version"],
            format!("winelandVersion {name:?}"),
        );
    }
    for case in items(&fixture["pure"]["compare"]) {
        let a = wineland_version(str_of(&case["a"])).unwrap();
        let b = wineland_version(str_of(&case["b"])).unwrap();
        let sign = compare_wineland(&a, &b) as i64;
        m.check_eq(
            sign,
            case["sign"].as_i64().unwrap(),
            format!("compareWineland {case}"),
        );
    }
    for case in items(&fixture["pure"]["candidates"]) {
        let actual = update_candidate(&case["input"]).map_or(Value::Null, |c| c.to_json());
        m.check(
            &actual,
            &case["result"],
            format!("updateCandidate {}", case["input"]),
        );
    }
    for case in items(&fixture["pure"]["sums"]) {
        let text = str_of(&case["text"]);
        let result = parse_sha512sum(text, "proton-wineland-11.0-20991231-hlsr99-x86_64.tar.xz");
        m.check(
            &attempt(result, |hash| json!(hash)),
            &case["result"],
            format!("parseSha512sum {text:?}"),
        );
    }
    m.finish("версии, кандидаты и .sha512sum");
}

#[test]
fn umu_roots_match_node() {
    let fixture = proton();
    let mut m = Mismatches::default();
    for case in items(&fixture["pure"]["umuRoots"]) {
        let env: Env = case["env"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, value)| (key.clone(), str_of(value).to_owned()))
            .collect();
        let actual = umu_root(&env).to_string_lossy().replace('\\', "/");
        // Нет ни XDG, ни папки umu: домашняя папка этой машины (в фикстуре — машины съёма).
        let expected = if str_of(&case["root"]).ends_with("/.local/share/umu")
            && !env.contains_key("UMU_FOLDERS_PATH")
        {
            format!(
                "{}/.local/share/umu",
                std::env::home_dir()
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            )
        } else {
            str_of(&case["root"]).replace('\\', "/")
        };
        m.check_eq(actual, expected, format!("umuRoot {}", case["env"]));
    }
    m.finish("путь umu");
}

#[test]
fn tls_errors_match_node() {
    let fixture = proton();
    let mut m = Mismatches::default();
    for case in items(&fixture["pure"]["tls"]) {
        let input = &case["input"];
        let actual = is_tls_error(input["code"].as_str(), str_of(&input["message"]));
        m.check_eq(
            actual,
            case["tls"].as_bool().unwrap(),
            format!("isTlsError {input}"),
        );
    }
    m.finish("TLS-ошибки");
}

struct Silent;

impl ChildControl for Silent {
    fn kill(&self, _signal: &str) -> bool {
        true
    }
    fn kill_group(&self, _signal: &str) -> bool {
        false
    }
}

#[tokio::test]
async fn cleaned_output_matches_node() {
    let fixture = proton();
    let mut m = Mismatches::default();
    for case in items(&fixture["pure"]["outputs"]) {
        let text = str_of(&case["text"]);
        let (child, controller) = Child::new(Some(1), Arc::new(Silent));
        controller.stdout(text.as_bytes().to_vec());
        controller.stderr(text.as_bytes().to_vec());
        controller.exit(Some(0), None);
        let options = WaitOptions {
            label: Some("probe".to_owned()),
            ..WaitOptions::default()
        };
        let output = wait_for_prefix_command(&child, &options).await;
        let CommandOutput { stdout, stderr } = output.unwrap();
        m.check_eq(
            stdout.as_str(),
            str_of(&case["cleaned"]),
            format!("cleanOutput {text:?}"),
        );
        m.check_eq(
            stderr.as_str(),
            str_of(&case["cleaned"]),
            format!("cleanOutput {text:?}"),
        );
    }
    m.finish("очистка вывода");
}

// ---------------------------------------------------------------- менеджер

/// `managerTree(root)` скрипта.
fn manager_tree(root: &Root) {
    let proton = |dir: &str| {
        write_file(
            Path::new(&root.at(&format!("{dir}/proton"))),
            "#!/bin/sh\n",
            Some(0o755),
        )
    };
    proton("protons/proton-wineland-11.0-20261003-hlsr3-x86_64");
    proton("protons/proton-wineland-11.0-20260913-hlsr2-x86_64");
    proton("protons/proton-cachyos-11.0-20260703-slr-x86_64");
    proton("protons/proton-wineland-11.0-20260913-x86_64");
    proton("protons/Proton-Test");
    write_file(
        Path::new(&root.at("protons/not-proton/readme.txt")),
        "x",
        None,
    );
    write_file(Path::new(&root.at("protons/stray-file")), "x", None);
    proton(".steam/root/compatibilitytools.d/GE-Proton9-20");
    proton(".local/share/Steam/steamapps/common/Proton - Experimental");
    write_file(
        Path::new(&root.at(".local/share/Steam/steamapps/common/Half-Life 2/hl2.exe")),
        "x",
        None,
    );
    proton("lib2/steamapps/common/Proton 9.0");
    proton(".config/heroic/tools/proton/Heroic-Proton");
    let library = root.at("lib2").replace('\\', "\\\\");
    write_file(
        Path::new(&root.at(".local/share/Steam/steamapps/libraryfolders.vdf")),
        format!(
            "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{library}\"\n\t}}\n\t\"1\"\t\t\"relative/lib\"\n\t\"contentstatsid\"\t\t\"1\"\n}}\n"
        ),
        None,
    );
    proton("custom/My Proton");
    std::fs::create_dir_all(root.at("custom/empty")).unwrap();
}

/// Запись внутри корня сценария (системные папки Steam не в счёт).
fn inside(root: &Root, entry: &Value) -> bool {
    let path = entry["path"].as_str().unwrap_or("");
    path == root.text || path.starts_with(&format!("{}{}", root.text, NodePath::native().sep()))
}

fn filter_status(root: &Root, status: &Value) -> Value {
    let mut status = status.clone();
    let installed: Vec<Value> = status["installed"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| inside(root, entry))
        .cloned()
        .collect();
    status["installed"] = Value::Array(installed);
    status
}

/// `brief(status)` скрипта.
fn brief(status: &Value) -> Value {
    let installed: Vec<Value> = status["installed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            json!(format!(
                "{}|{}|{}",
                str_of(&entry["name"]),
                str_of(&entry["source"]),
                entry["recommended"]
            ))
        })
        .collect();
    json!({
        "installed": installed,
        "selected": status["selected"],
        "phase": status["phase"],
        "percent": status["percent"],
        "error": status["error"],
        "operation": status["operation"],
        "update": status["update"],
    })
}

async fn manager_scenario(scenario: &Value, m: &mut Mismatches) {
    let name = str_of(&scenario["name"]);
    let root = Root::new(&format!("hlsr-proton-{name}-"));
    manager_tree(&root);
    let archive = proton_support::update_archive();
    let (sha256, sha512) = proton_support::digests(&archive);
    let fixture_archive = &scenario["archive"];
    m.check(
        &json!({
            "name": "proton-wineland-11.0-20991231-hlsr99-x86_64.tar.xz",
            "size": archive.len(),
            "sha256": sha256,
            "sha512": sha512,
        }),
        fixture_archive,
        format!("{name}: архив обновления"),
    );
    let net = Arc::new(FakeNet::default());
    {
        let mut texts = net.texts.lock().unwrap();
        texts.insert(
            proton_manager::RELEASES_API.to_owned(),
            str_of(&scenario["releases"]).to_owned(),
        );
        let releases: Value = serde_json::from_str(str_of(&scenario["releases"])).unwrap();
        let sums_url = str_of(&releases[2]["assets"][1]["browser_download_url"]).to_owned();
        texts.insert(sums_url, str_of(&scenario["sums"]).to_owned());
    }
    *net.body.lock().unwrap() = if scenario["corrupt"].as_bool().unwrap() {
        b"corrupt".to_vec()
    } else {
        archive.clone()
    };
    let logger = Arc::new(MemoryLogger::default());
    let manager = ProtonManager::new(ProtonManagerOptions {
        data_dir: root.path.clone(),
        home: Some(root.path.clone()),
        logger: Some(logger.clone()),
        read_text: Some(Arc::new(FakeText(net.clone()))),
        request: Some(Arc::new(FakeRequest(net.clone()))),
        in_use: Some(Arc::new(|_: &Path| false)),
        pinned: None,
    });
    let events: Arc<Mutex<Vec<ProtonEvent>>> = Arc::default();
    {
        let events = events.clone();
        manager.subscribe(Arc::new(move |event: &ProtonEvent| {
            events.lock().unwrap().push(event.clone())
        }));
    }
    let selection = || {
        std::fs::read_to_string(root.at("proton-selection.json"))
            .map(|text| Value::String(root.norm_text(&text)))
            .unwrap_or(Value::Null)
    };
    for (index, expected) in items(&scenario["steps"]).iter().enumerate() {
        let step = &expected["step"];
        let op = str_of(&step["op"]);
        let p = step["path"].as_str().map(|path| {
            if path.is_empty() || path.starts_with('/') {
                path.to_owned()
            } else {
                root.at(path)
            }
        });
        let entry = |entry: hlsr_platform::linux::proton_manager::ProtonEntry| entry.to_json();
        let result = match op {
            "status" => json!({"ok": filter_status(&root, &manager.status())}),
            "discover" => {
                let list: Vec<Value> = manager
                    .discover()
                    .into_iter()
                    .map(entry)
                    .filter(|e| inside(&root, e))
                    .collect();
                json!({"ok": list})
            }
            "selected" => json!({"ok": manager.selected().map_or(Value::Null, entry)}),
            "select" => attempt(manager.select(p.as_deref().unwrap()), entry),
            "add-custom" => attempt(manager.add_custom(p.as_deref().unwrap()), |item| {
                item.map_or(Value::Null, entry)
            }),
            "ensure-reference" => attempt(manager.ensure_reference().await, |item| {
                item.map_or(Value::Null, entry)
            }),
            "check-for-update" => json!({
                "ok": manager.check_for_update().await.map_or(Value::Null, |c| c.to_json())
            }),
            "apply-update" => attempt(manager.apply_update().await, |item| {
                item.map_or(Value::Null, entry)
            }),
            "managed-wineland" => json!({
                "ok": manager.managed_wineland().iter().map(|w| w.to_json()).collect::<Vec<_>>()
            }),
            "write-selection" => {
                let text = root
                    .norm_text(str_of(&step["text"]))
                    .replace("<root>", &root.text.replace('\\', "\\\\"));
                std::fs::write(root.at("proton-selection.json"), text).unwrap();
                json!({"ok": null})
            }
            "remove" => {
                let _ = std::fs::remove_dir_all(p.as_deref().unwrap());
                json!({"ok": null})
            }
            other => panic!("неизвестный шаг {other}"),
        };
        let context = format!("{name}: шаг {index} {step}");
        m.check(
            &root.norm(&result),
            &expected["result"],
            format!("{context}: результат"),
        );
        let actual_events: Vec<Value> = events
            .lock()
            .unwrap()
            .drain(..)
            .map(|event| {
                let payload = if event.name == "state" {
                    brief(&root.norm(&filter_status(&root, &event.payload)))
                } else {
                    root.norm(&event.payload)
                };
                json!({"event": event.name, "payload": payload})
            })
            .collect();
        m.check(
            &Value::Array(actual_events),
            &expected["events"],
            format!("{context}: события"),
        );
        let logs: Vec<Value> = logger
            .take()
            .into_iter()
            .map(|(level, message)| json!({"level": level, "message": root.norm_text(&message)}))
            .collect();
        m.check(
            &Value::Array(logs),
            &expected["logs"],
            format!("{context}: журнал"),
        );
        m.check(
            &selection(),
            &expected["selection"],
            format!("{context}: proton-selection.json"),
        );
    }
    m.check(
        &Value::Array(net.reads()),
        &scenario["reads"],
        format!("{name}: чтения"),
    );
    m.check(
        &json!(net.requests()),
        &scenario["requests"],
        format!("{name}: загрузки"),
    );
    m.check(
        &tree_of(&root.path.join("protons"), &[&root.text]),
        &scenario["protons"],
        format!("{name}: дерево protons"),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manager_scenarios_match_node() {
    let fixture = proton();
    let mut m = Mismatches::default();
    for scenario in items(&fixture["manager"]) {
        manager_scenario(scenario, &mut m).await;
    }
    m.finish("менеджер Proton");
}
