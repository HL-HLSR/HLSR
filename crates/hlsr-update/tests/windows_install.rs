//! Установка и удаление Windows против 0.4.9: фикстура `windows-install-049.json` из
//! `tools/reference/update/windows-install.js` — сценарии самотестов `windows-install-selftest.js`
//! («install: …», «uninstall: …», «removalCommand…», «удаление из установщика: «Назад»…») и 0.4.9
//! (антивирус, необязательный оверлей LiveSplit, маркер библиотеки; полное удаление с настоящей
//! очисткой библиотеки: чужая установка рядом и глубоко, маркер-ссылка, папка не наша, копия
//! установки, QA-сборка), выполненные настоящими `install.js`/`uninstall.js` во временном каталоге
//! с реестром в памяти и ярлыками-JSON. Здесь те же шаги выполняет Rust с `FakeRegistry`,
//! `FakeShellLinks`, `FakeKnownFolders` и `FakeProcesses` — дважды: из распакованной сборки
//! (путь В) и из нагрузки `HLSR-Setup.exe` (Р1); сравниваются итог, прогресс, журнал, изменения и
//! состояние реестра, вызовы `icacls` и очистки, протокол Discord и дерево файлов.
//!
//! Пути Windows на настоящей ФС (`path.win32`, буквы дисков): только Windows.
#![cfg(windows)]

mod support;

use hlsr_core::json_formats::compact;
use hlsr_core::logger::MemoryLogger;
use hlsr_update::cancel::CancelToken;
use hlsr_update::uninstall::library_cleanup::{
    self, CleanupLibraryResult, CleanupOptions, Removal, StaticSteamLibraries,
};
use hlsr_update::windows::file_list;
use hlsr_update::windows::install::{
    self, ExistingKind, InstallOptions, existing_kind, leftover_name,
};
use hlsr_update::windows::known_folders::FakeKnownFolders;
use hlsr_update::windows::layout::{self, App, Identity};
use hlsr_update::windows::payload::{Payload, Source};
use hlsr_update::windows::processes::{Call, Env, FakeProcesses, SystemProcesses};
use hlsr_update::windows::registry::{FakeRegistry, RegOp, RegValue, Registry};
use hlsr_update::windows::shortcuts::{FakeShellLinks, Shortcuts};
use hlsr_update::windows::uninstall::{self, RemovalTargets, UninstallOptions};
use serde_json::{Map, Value, json};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use support::payload::pack_for_tests;
use support::tree::snapshot;
use support::{Mismatches, fixture, items, opt_str, str_of};

static FIXTURE_049: LazyLock<Value> = LazyLock::new(|| fixture("windows-install-049.json"));

const SOURCE: &str = "setup-unpacked";
const OLD_ID: &str = "{E1666AAA-AC03-4A85-B610-167B9EC8A9DA}";
const ZERO_ID: &str = "{00000000-0000-0000-0000-000000000000}";

/// Временный корень без `\\?\` (как `fs.realpathSync` Node).
fn temp_root() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix("hlsr-update-install-")
        .tempdir()
        .unwrap();
    let real = std::fs::canonicalize(dir.path()).unwrap();
    let text = real.to_string_lossy().into_owned();
    let plain = text
        .strip_prefix(r"\\?\")
        .map_or(text.clone(), str::to_owned);
    (dir, PathBuf::from(plain))
}

/// Замены временного корня и идентификаторов установки, как `makeNormalizer` скрипта.
struct Normalizer {
    root: String,
    ids: RefCell<Vec<String>>,
}

impl Normalizer {
    fn text(&self, value: &str) -> String {
        let replaced = value.replace(&self.root, "{root}");
        let mut out = String::new();
        let mut rest = replaced.as_str();
        while let Some(at) = rest.find('{') {
            out.push_str(&rest[..at]);
            let candidate = rest.get(at..at + 38).unwrap_or("");
            if layout::is_install_id(candidate) && candidate != OLD_ID && candidate != ZERO_ID {
                let mut ids = self.ids.borrow_mut();
                let index = ids
                    .iter()
                    .position(|id| id == candidate)
                    .unwrap_or_else(|| {
                        ids.push(candidate.to_owned());
                        ids.len() - 1
                    });
                out.push_str(&format!("{{ID{}}}", index + 1));
                rest = &rest[at + 38..];
            } else {
                out.push('{');
                rest = &rest[at + 1..];
            }
        }
        out.push_str(rest);
        out
    }

    fn deep(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.text(text)),
            Value::Array(list) => Value::Array(list.iter().map(|item| self.deep(item)).collect()),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, item)| (self.text(key), self.deep(item)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    fn denorm(&self, value: &str) -> String {
        let mut out = value.replace("{root}", &self.root);
        for (index, id) in self.ids.borrow().iter().enumerate() {
            out = out.replace(&format!("{{ID{}}}", index + 1), id);
        }
        out
    }

    fn transform(&self, bytes: Vec<u8>) -> Vec<u8> {
        match String::from_utf8(bytes) {
            Ok(text) => {
                let escaped = serde_json::to_string(&self.root).unwrap();
                let escaped = &escaped[1..escaped.len() - 1];
                self.text(&text.replace(escaped, "{root}")).into_bytes()
            }
            Err(error) => error.into_bytes(),
        }
    }
}

fn app_of(value: &Value) -> App {
    if value.is_null() {
        return App::production();
    }
    App {
        app_id: str_of(&value["appId"]).to_owned(),
        product_name: str_of(&value["productName"]).to_owned(),
    }
}

fn bytes_of(body: &Value, norm: &Normalizer) -> Vec<u8> {
    match body {
        Value::String(text) => norm.denorm(text).into_bytes(),
        other => hex_bytes(str_of(&other["hex"])),
    }
}

fn hex_bytes(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
        .collect()
}

fn join_rel(root: &Path, rel: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in rel.split('/') {
        path.push(part);
    }
    path
}

fn put_file(root: &Path, rel: &str, bytes: &[u8]) {
    let file = join_rel(root, rel);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, bytes).unwrap();
}

/// `resolveRel`: абсолютный путь Windows как есть, иначе от корня.
fn resolve_rel(root: &Path, value: &str, norm: &Normalizer) -> String {
    let text = norm.denorm(value);
    let bytes = text.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\' {
        return text;
    }
    join_rel(root, &text).to_string_lossy().into_owned()
}

/// Реестр подделки в виде памяти самотеста: `{ "HKCU\…": { имя: строка | число } }`.
fn registry_model(registry: &FakeRegistry) -> Value {
    let mut out = Map::new();
    for (key, values) in registry.to_json().as_object().unwrap() {
        let short = key.replacen("HKEY_CURRENT_USER", "HKCU", 1);
        let mut model = Map::new();
        for (name, value) in values.as_object().unwrap() {
            let data = str_of(&value["data"]);
            let item = match str_of(&value["type"]) {
                "REG_DWORD" => {
                    json!(u32::from_str_radix(data.trim_start_matches("0x"), 16).unwrap())
                }
                _ => json!(data),
            };
            model.insert(name.clone(), item);
        }
        out.insert(short, Value::Object(model));
    }
    Value::Object(out)
}

/// Операции подделки, которые дают изменения `apply` 0.4.7.
fn expected_ops(applied: &Value) -> Vec<Value> {
    let mut ops = Vec::new();
    for changes in items(applied) {
        for change in items(changes) {
            let key = str_of(&change["key"]);
            if change["remove"] == json!(true) {
                ops.push(json!({"deleteTree": key}));
                continue;
            }
            for (name, value) in change["values"].as_object().unwrap() {
                ops.push(match value {
                    Value::Null => json!({"delete": [key, name]}),
                    Value::Object(dword) => json!({"write": [key, name, dword["dword"]]}),
                    other => json!({"write": [key, name, other]}),
                });
            }
        }
    }
    ops
}

fn ops_value(ops: &[RegOp]) -> Vec<Value> {
    let short = |key: &str| key.replacen("HKEY_CURRENT_USER", "HKCU", 1);
    ops.iter()
        .map(|op| match op {
            RegOp::Write { key, name, value } => {
                let data = match value {
                    RegValue::Dword(number) => json!(number),
                    other => json!(other.data_text()),
                };
                json!({"write": [short(key), name, data]})
            }
            RegOp::DeleteValue { key, name } => json!({"delete": [short(key), name]}),
            RegOp::DeleteTree { key } => json!({"deleteTree": short(key)}),
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// Распакованная сборка (`resources/install-files.json` рядом): путь В и самотесты 0.4.7.
    Dir,
    /// Нагрузка `HLSR-Setup.exe` (Р1).
    Payload,
}

struct World {
    root: PathBuf,
    norm: Normalizer,
    registry: FakeRegistry,
    links: Arc<FakeShellLinks>,
    log: MemoryLogger,
    app: App,
    mode: Mode,
}

impl World {
    fn src(&self) -> PathBuf {
        self.root.join(SOURCE)
    }

    fn shortcuts(&self, id: &Identity) -> Shortcuts {
        Shortcuts::new(self.links.clone(), &FakeKnownFolders::under(&self.root), id).unwrap()
    }

    fn source(&self) -> Source {
        match self.mode {
            Mode::Dir => Source::Dir(self.src()),
            Mode::Payload => {
                let exe = self.src().join("HLSR-Setup.exe");
                pack_for_tests(&self.src(), &exe);
                Source::Payload(Box::new(Payload::open(&exe).expect("нагрузка открывается")))
            }
        }
    }

    fn tree(&self) -> Value {
        let transform = |_rel: &str, bytes: Vec<u8>| self.norm.transform(bytes);
        let snap = snapshot(&self.root, Some(&transform));
        Value::Array(
            items(&snap)
                .iter()
                .filter(|entry| {
                    let path = str_of(&entry["path"]);
                    path != SOURCE && !path.starts_with(&format!("{SOURCE}/"))
                })
                .cloned()
                .collect(),
        )
    }
}

fn run_scenario(scenario: &Value, mode: Mode, mismatches: &mut Mismatches) {
    let label = str_of(&scenario["label"]);
    let steps = items(&scenario["steps"]);
    if mode == Mode::Payload
        && steps
            .iter()
            .any(|step| step.get("tamper").is_some() || step.get("vanish").is_some())
    {
        // Подмена файла после списка в нагрузке — отказ самой нагрузки (см. windows_payload.rs);
        // пропавший файл в нагрузку не упаковать.
        return;
    }
    if mode == Mode::Payload
        && steps
            .iter()
            .any(|step| opt_str(&step["installDir"]).is_some_and(|dir| dir.starts_with(SOURCE)))
    {
        // У нагрузки нет распакованного каталога: читается только exe, и каталог установки рядом с
        // ним допустим (exe внутри каталога установки — BAD_DIR, см. windows_payload.rs).
        return;
    }
    let (_guard, root) = temp_root();
    let world = World {
        norm: Normalizer {
            root: root.to_string_lossy().into_owned(),
            ids: RefCell::new(Vec::new()),
        },
        root,
        registry: FakeRegistry::new(),
        links: Arc::new(FakeShellLinks::new()),
        log: MemoryLogger::default(),
        app: App::production(),
        mode,
    };
    let mut world = world;
    let records = items(&scenario["records"]);
    for (index, (step, expected)) in steps.iter().zip(records).enumerate() {
        let context = format!("{label} [{mode:?}] шаг {index} ({})", str_of(&step["op"]));
        match str_of(&step["op"]) {
            "build" => {
                let src = world.src();
                world.app = app_of(&step["app"]);
                std::fs::create_dir_all(src.join("resources")).unwrap();
                for rel in step["remove"].as_array().into_iter().flatten() {
                    let _ = std::fs::remove_file(join_rel(&src, str_of(rel)));
                }
                for pair in items(&step["files"]) {
                    put_file(&src, str_of(&pair[0]), &bytes_of(&pair[1], &world.norm));
                }
                let list = file_list::build(&src, str_of(&step["version"]), &world.app).unwrap();
                std::fs::write(src.join(file_list::RELATIVE), compact(&list.to_value())).unwrap();
                for pair in step["tamper"].as_array().into_iter().flatten() {
                    put_file(&src, str_of(&pair[0]), &bytes_of(&pair[1], &world.norm));
                }
                for rel in step["vanish"].as_array().into_iter().flatten() {
                    std::fs::remove_file(join_rel(&src, str_of(rel))).unwrap();
                }
            }
            "junction" => {
                hlsr_update::windows::file_ops::create_junction(
                    &join_rel(&world.root, str_of(&step["target"])),
                    &join_rel(&world.root, str_of(&step["path"])),
                )
                .unwrap();
            }
            "put" => {
                for dir in step["dirs"].as_array().into_iter().flatten() {
                    std::fs::create_dir_all(join_rel(&world.root, str_of(dir))).unwrap();
                }
                for pair in step["files"].as_array().into_iter().flatten() {
                    put_file(
                        &world.root,
                        str_of(&pair[0]),
                        &bytes_of(&pair[1], &world.norm),
                    );
                }
            }
            "remove" => {
                for rel in items(&step["paths"]) {
                    install::remove_all(&join_rel(&world.root, str_of(rel))).unwrap();
                }
            }
            "registry" => {
                let id = layout::identity(Some(&world.app)).unwrap();
                let key = match str_of(&step["key"]) {
                    "install" => id.install_key.clone(),
                    "uninstall" => id.uninstall_key.clone(),
                    other => other.to_owned(),
                };
                for (name, value) in step["values"].as_object().unwrap() {
                    let data = world.norm.denorm(str_of(value));
                    world
                        .registry
                        .write(&key, name, &RegValue::Sz(data))
                        .unwrap();
                }
            }
            "shortcut" => {
                let id = layout::identity(Some(&world.app)).unwrap();
                let shortcuts = world.shortcuts(&id);
                let file = if str_of(&step["which"]) == "startMenu" {
                    shortcuts.start_menu.clone()
                } else {
                    shortcuts.desktop.clone()
                };
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                let body = json!({"target": str_of(&step["target"]), "cwd": "", "icon": "", "iconIndex": 0,
                    "appUserModelId": "", "description": ""});
                std::fs::write(file, body.to_string()).unwrap();
            }
            "plan" | "identityOf" | "ownership" | "install" | "uninstall" => {
                let actual = run_action(&mut world, step);
                mismatches.check(&actual, expected, &context);
            }
            other => panic!("шаг {other}"),
        }
    }
}

fn run_action(world: &mut World, step: &Value) -> Value {
    let identity_app = if step["identity"].is_null() {
        world.app.clone()
    } else {
        app_of(&step["identity"])
    };
    let id = layout::identity(Some(&identity_app)).unwrap();
    let norm = &world.norm;
    let install_dir = resolve_rel(
        &world.root,
        step["installDir"].as_str().unwrap_or("HLSR/Launcher"),
        norm,
    );
    let library_root = match &step["libraryRoot"] {
        Value::Null if step.get("libraryRoot").is_some() => None,
        Value::String(rel) => Some(resolve_rel(&world.root, rel, norm)),
        _ => Some(resolve_rel(&world.root, "Library", norm)),
    };
    let op = str_of(&step["op"]);
    if op == "plan" {
        let registry =
            (step["registry"] == json!(true)).then_some(&world.registry as &dyn Registry);
        let plan = uninstall::plan(
            &install_dir,
            library_root.as_deref(),
            &uninstall::identity_of(&install_dir),
            registry,
            &|_| None,
        );
        return json!({"op": "plan", "plan": norm.deep(&plan.to_value())});
    }
    if op == "identityOf" {
        let found = uninstall::identity_of(&install_dir);
        let app_id = Some(found.app_id).filter(|app_id| !app_id.is_empty());
        return json!({"op": "identityOf", "identity": {"appId": app_id, "productName": found.product_name,
            "guid": found.guid, "installKey": found.install_key}});
    }
    if op == "ownership" {
        let dir = match step["dir"].as_str() {
            Some(dir) => resolve_rel(&world.root, dir, norm),
            None => layout::win_path().resolve(&[&install_dir]),
        };
        let values: Vec<(String, RegValue)> = step["values"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, value)| (name.clone(), RegValue::Sz(norm.denorm(str_of(value)))))
            .collect();
        let problem = uninstall::ownership_problem(&dir, &values, &|_| None, &id);
        return json!({"op": "ownership", "problem": norm.deep(&json!(problem))});
    }

    world.log.take();
    let ops_before = world.registry.ops().len();
    world
        .registry
        .set_fail_writes(step["registryFail"] == json!(true));
    world
        .links
        .set_fail_writes(step["shortcutsFail"] == json!(true));
    let shortcuts = world.shortcuts(&id);
    let mut record = Map::new();
    record.insert("op".to_owned(), json!(op));

    if op == "install" {
        let source = world.source();
        let status = step["icaclsStatus"].as_i64().unwrap_or(0) as i32;
        let processes = FakeProcesses::new().on_run(move |_| Some(status));
        let cancel = CancelToken::new();
        if step["abort"] == json!("before") {
            cancel.cancel();
        }
        let abort_after_last = step["abort"] == json!("afterLast");
        let progress = RefCell::new(Vec::new());
        let committed = RefCell::new(false);
        let mut on_progress = |done: u64, total: u64| {
            progress.borrow_mut().push(json!([done, total]));
            if abort_after_last && done == total {
                cancel.cancel();
            }
        };
        let mut on_commit = || *committed.borrow_mut() = true;
        let library = library_root.clone().unwrap();
        let setup_file = step["setupFile"].as_str();
        let mut options = InstallOptions::new(
            &source,
            &install_dir,
            &world.registry,
            &shortcuts,
            &world.log,
        );
        options.cancel = Some(&cancel);
        options.env = &|_| None;
        options.update = step["update"] == json!(true);
        options.desktop_shortcut = step["desktopShortcut"].as_bool();
        options.setup_file = setup_file;
        options.library_root = &library;
        options.sleep = &|_| {};
        options.on_progress = Some(&mut on_progress);
        options.on_commit = Some(&mut on_commit);
        options.processes = Some(&processes);
        let result = match install::install(options) {
            Ok(result) => {
                json!({"ok": true, "installDir": result.install_dir, "installId": result.install_id,
                "version": result.version, "guid": result.identity.guid, "warnings": result.warnings})
            }
            Err(error) => json!({"ok": false, "code": error.code, "message": error.message,
                "file": error.file, "cause": error.cause.and_then(|cause| cause.code)}),
        };
        if let Source::Payload(payload) = &source {
            std::fs::remove_file(payload.exe()).unwrap();
        }
        let icacls: Vec<Value> = processes
            .calls()
            .iter()
            .map(|call| match call {
                Call::Run(command, _) => json!([command.file, command.args]),
                Call::Spawn(command) => json!(["spawn", command.file]),
            })
            .collect();
        record.insert("result".to_owned(), result);
        record.insert("committed".to_owned(), json!(*committed.borrow()));
        record.insert("progress".to_owned(), Value::Array(progress.into_inner()));
        record.insert("icacls".to_owned(), json!(icacls));
    } else {
        let calls = Mutex::new(Vec::new());
        let protocols = Mutex::new(Vec::new());
        let spec = step["cleanup"].clone();
        let denorm_root = spec["root"].as_str().map(|root| norm.denorm(root));
        let env_pairs: Vec<(String, String)> = step["env"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(name, value)| (name.clone(), norm.denorm(str_of(value))))
            .collect();
        let env_json: Map<String, Value> = env_pairs
            .iter()
            .map(|(name, value)| (name.clone(), json!(value)))
            .collect();
        let lookup = |name: &str| {
            env_pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        // Без cleanup в шаге — настоящая очистка (домашний каталог Z:\nohome, без Steam).
        let cleanup = |root: &str, install_dir: &str, own_marker: Option<&str>| {
            calls
                .lock()
                .unwrap()
                .push(json!([root, {"purge": true, "installDir": install_dir,
                "env": env_json, "ownMarker": own_marker}]));
            if spec.is_null() {
                let steam = StaticSteamLibraries::default();
                return library_cleanup::cleanup_library(
                    root,
                    &CleanupOptions {
                        env: Some(&lookup),
                        home: Some("Z:\\nohome".to_owned()),
                        steam: Some(&steam),
                        install_dir: Some(install_dir.to_owned()),
                        purge: true,
                        own_marker: own_marker.map(str::to_owned),
                        ..CleanupOptions::default()
                    },
                );
            }
            if spec["ok"] == json!(true) {
                CleanupLibraryResult::Done {
                    code: 0,
                    root: root.to_owned(),
                    zones: Vec::new(),
                    root_removed: false,
                    removal: Removal::default(),
                    left: Vec::new(),
                }
            } else {
                CleanupLibraryResult::Refused {
                    code: spec["code"].as_i64().unwrap() as i32,
                    reason: spec["reason"].as_str().map(str::to_owned),
                    root: denorm_root.clone(),
                }
            }
        };
        let protocol_fail = step["protocolFail"] == json!(true);
        let remove_protocol = |name: &str| {
            protocols.lock().unwrap().push(name.to_owned());
            !protocol_fail
        };
        let data_dirs: Vec<String> = step["dataDirs"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|rel| resolve_rel(&world.root, str_of(rel), norm))
            .collect();
        let temp_root = step["tempRoot"]
            .as_str()
            .map(|rel| PathBuf::from(resolve_rel(&world.root, rel, norm)));
        let dir = layout::win_path().resolve(&[&install_dir]);
        let result = uninstall::uninstall(UninstallOptions {
            install_dir: &dir,
            library_root: library_root.as_deref(),
            remove_games: step["removeGames"] == json!(true),
            registry: &world.registry,
            shortcuts: &shortcuts,
            cleanup_library: &cleanup,
            identity: &id,
            env: &lookup,
            log: &world.log,
            data_dirs: &data_dirs,
            updater_cache_name: step["updaterCacheName"].as_str(),
            temp_root: temp_root.as_deref(),
            remove_protocol_client: Some(&remove_protocol),
            is_alive: None,
        });
        record.insert(
            "result".to_owned(),
            json!({"status": result.status.as_str(), "warnings": result.warnings,
                "gamesRemoved": result.games_removed, "removeProgram": result.remove_program,
                "removeData": result.remove_data, "removeParent": result.remove_parent,
                "ownMarker": result.own_marker}),
        );
        record.insert(
            "cleanupCalls".to_owned(),
            Value::Array(calls.into_inner().unwrap()),
        );
        record.insert(
            "protocols".to_owned(),
            json!(protocols.into_inner().unwrap()),
        );
    }
    world.registry.set_fail_writes(false);
    world.links.set_fail_writes(false);
    let logs: Vec<Value> = world
        .log
        .take()
        .into_iter()
        .map(|(level, message)| json!([level, message]))
        .collect();
    let ops = world.registry.ops();
    record.insert("logs".to_owned(), json!(logs));
    record.insert("ops".to_owned(), json!(ops_value(&ops[ops_before..])));
    record.insert("registry".to_owned(), registry_model(&world.registry));
    let tree = world.tree();
    record.insert("tree".to_owned(), tree);
    norm.deep(&Value::Object(record))
}

/// Ожидание фикстуры в виде, который строит тест: изменения `apply` — операциями подделки.
fn expected_record(expected: &Value) -> Value {
    let mut out = expected.clone();
    if let Some(applied) = out.as_object_mut().and_then(|map| map.remove("applied")) {
        out["ops"] = json!(expected_ops(&applied));
    }
    out
}

fn check_scenarios(mode: Mode) {
    let mut mismatches = Mismatches::default();
    for scenario in items(&FIXTURE_049["scenarios"]) {
        let mut adjusted = scenario.clone();
        let records: Vec<Value> = items(&scenario["records"])
            .iter()
            .map(|record| {
                if matches!(str_of(&record["op"]), "install" | "uninstall") {
                    expected_record(record)
                } else {
                    record.clone()
                }
            })
            .collect();
        adjusted["records"] = Value::Array(records);
        run_scenario(&adjusted, mode, &mut mismatches);
    }
    mismatches.finish(&format!("установка и удаление ({mode:?})"));
}

#[test]
fn install_and_uninstall_scenarios_match_node_from_unpacked_build() {
    check_scenarios(Mode::Dir);
}

#[test]
fn install_scenarios_match_node_from_setup_payload() {
    check_scenarios(Mode::Payload);
}

#[test]
fn removal_command_matches_node() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE_049["removals"]) {
        let env_pairs: Vec<(String, String)> = case["env"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, value)| (name.clone(), str_of(value).to_owned()))
            .collect();
        let env = Env::from_pairs(env_pairs.clone());
        let options = &case["options"];
        let data: Vec<String> = options["data"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|item| str_of(item).to_owned())
            .collect();
        let targets = RemovalTargets {
            data: &data,
            parent: options["parent"].as_str(),
            pid: options["pid"].as_u64().map(|pid| pid as u32),
            own_marker: options["ownMarker"].as_str(),
        };
        let actual = match uninstall::removal_command(
            case["dir"].as_str(),
            &env,
            case["relaunch"].as_str(),
            &targets,
        ) {
            Ok(command) => {
                let added: Map<String, Value> = command
                    .env
                    .vars()
                    .iter()
                    .filter(|(name, _)| !env_pairs.iter().any(|(known, _)| known == name))
                    .map(|(name, value)| (name.clone(), json!(value)))
                    .collect();
                assert!(command.hidden);
                let launcher = uninstall::launcher_command(&command);
                assert!(launcher.hidden);
                json!({"file": command.file, "args": command.args, "added": added,
                    "launcher": {"file": launcher.file, "args": launcher.args,
                        "powershell": launcher.env.get("HLSR_POWERSHELL")}})
            }
            Err(error) => json!({"error": error}),
        };
        let mut expected = case.clone();
        let expected = expected.as_object_mut().unwrap();
        for key in ["dir", "env", "relaunch", "options"] {
            expected.remove(key);
        }
        mismatches.check(
            &actual,
            &Value::Object(expected.clone()),
            case["dir"].to_string(),
        );
    }
    mismatches.finish("removalCommand");
}

/// «removalCommand: PowerShell ждёт выхода без консольных окон, путь не попадает в текст команды»;
/// скрипт запускает короткий PowerShell (`launcherCommand`) и ждёт его выхода.
#[test]
fn removal_command_waits_without_console_windows() {
    let dir = r#"C:\a" & del C:\x %TEMP%"#;
    let env = Env::from_pairs([("SystemRoot", "C:\\Windows")]);
    let none = RemovalTargets::default();
    let command = uninstall::removal_command(Some(dir), &env, None, &none).unwrap();
    assert_eq!(command.env.get("HLSR_REMOVE_DIR").as_deref(), Some(dir));
    assert!(!command.args.join(" ").contains("del C:"));
    let script = command.args.last().unwrap();
    assert!(script.contains("Start-Sleep"));
    assert!(script.contains("Remove-NoFollow $_"));
    let lower = script.to_lowercase();
    assert!(!lower.contains("ping") && !lower.contains("timeout") && !lower.contains("cmd"));
    for bad in ["relative\\dir", "C:\\a\nb"] {
        assert!(
            uninstall::removal_command(Some(bad), &env, None, &none)
                .unwrap_err()
                .contains("Недопустимый")
        );
    }

    let processes = FakeProcesses::new();
    let temp = Path::new("C:\\Temp");
    let data = ["C:\\U\\hlsr".to_owned()];
    let targets = RemovalTargets {
        data: &data,
        parent: Some("C:\\HLSR"),
        pid: Some(42),
        own_marker: Some("{}"),
    };
    uninstall::schedule_removal(
        &processes,
        Some("C:\\HLSR\\Launcher"),
        &env,
        None,
        &targets,
        temp,
    )
    .unwrap();
    let calls = processes.calls();
    let Call::Run(started, timeout) = &calls[0] else {
        panic!("ожидался короткий PowerShell с ожиданием выхода");
    };
    assert_eq!(*timeout, Some(uninstall::SCHEDULE_TIMEOUT));
    assert_eq!(
        started.file,
        "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
    );
    assert!(started.hidden);
    assert!(started.args.last().unwrap().contains("-EncodedCommand"));
    assert_eq!(started.cwd.as_deref(), Some("C:\\Temp"));
    for (name, value) in [
        ("HLSR_REMOVE_DIR", "C:\\HLSR\\Launcher"),
        ("HLSR_REMOVE_DATA", "C:\\U\\hlsr"),
        ("HLSR_REMOVE_PARENT", "C:\\HLSR"),
        ("HLSR_WAIT_PID", "42"),
        ("HLSR_OWN_MARKER", "{}"),
    ] {
        assert_eq!(started.env.get(name).as_deref(), Some(value), "{name}");
    }
    assert_eq!(started.env.get("HLSR_RELAUNCH"), None);
    // Код выхода не 0 — ошибка с кодом.
    let failing = FakeProcesses::new().on_run(|_| Some(1));
    let error = uninstall::schedule_removal(
        &failing,
        Some("C:\\HLSR\\Launcher"),
        &env,
        None,
        &none,
        temp,
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "скрипт удаления не запущен (код 1)");
}

/// «удаление из установщика: «Назад» открывает установщик только после удаления каталога».
#[test]
fn back_relaunches_setup_only_after_the_directory_is_gone() {
    let env = Env::from_pairs([("SystemRoot", "C:\\Windows")]);
    let command = uninstall::removal_command(
        Some("C:\\HLSR\\Launcher"),
        &env,
        Some("C:\\x\\HLSR-Setup.exe"),
        &RemovalTargets::default(),
    )
    .unwrap();
    assert_eq!(
        command.env.get("HLSR_RELAUNCH").as_deref(),
        Some("C:\\x\\HLSR-Setup.exe")
    );
    let script = command.args.last().unwrap();
    assert!(script.find("Start-Process").unwrap() > script.rfind("Remove-NoFollow").unwrap());
    assert!(script.contains(
        "-not ($d -and (Test-Path -LiteralPath $d))) { Start-Process -FilePath $env:HLSR_RELAUNCH"
    ));
    assert!(!script.contains("HLSR-Setup"));
}

/// Протокол Discord удаляется, только если ведёт к этому `HLSR.exe` (`removeAsDefaultProtocolClient`).
#[test]
fn discord_protocol_is_removed_only_when_it_points_here() {
    let key = format!("HKCU\\Software\\Classes\\{}", uninstall::DISCORD_PROTOCOL);
    let command_key = format!("{key}\\shell\\open\\command");
    let exe = "C:\\HLSR\\Launcher\\HLSR.exe";
    let ours =
        FakeRegistry::new().with_value(&command_key, "", RegValue::Sz(format!("\"{exe}\" \"%1\"")));
    assert!(uninstall::remove_protocol_client(
        &ours,
        exe,
        uninstall::DISCORD_PROTOCOL
    ));
    assert_eq!(ours.read(&command_key), None);
    let other = FakeRegistry::new().with_value(
        &command_key,
        "",
        RegValue::Sz("\"C:\\Other\\HLSR.exe\" \"%1\"".to_owned()),
    );
    assert!(uninstall::remove_protocol_client(
        &other,
        exe,
        uninstall::DISCORD_PROTOCOL
    ));
    assert!(other.read(&command_key).is_some());
    let failing =
        FakeRegistry::new().with_value(&command_key, "", RegValue::Sz(format!("\"{exe}\" \"%1\"")));
    failing.set_fail_writes(true);
    assert!(!uninstall::remove_protocol_client(
        &failing,
        exe,
        uninstall::DISCORD_PROTOCOL
    ));
    assert!(uninstall::remove_protocol_client(
        &FakeRegistry::new(),
        exe,
        uninstall::DISCORD_PROTOCOL
    ));
}

#[test]
fn leftover_names_follow_the_install_pattern() {
    assert_eq!(
        leftover_name(".Launcher.hlsr-new-0123abcd"),
        Some("Launcher")
    );
    assert_eq!(
        leftover_name(".Launcher.hlsr-old-89abcdef"),
        Some("Launcher")
    );
    assert_eq!(leftover_name(".a.b.hlsr-old-00000000"), Some("a.b"));
    assert_eq!(leftover_name(".Launcher.hlsr-new-0123ABCD"), None);
    assert_eq!(leftover_name(".Launcher.hlsr-new-0123abc"), None);
    assert_eq!(leftover_name("Launcher.hlsr-new-0123abcd"), None);
    assert_eq!(leftover_name("..hlsr-new-0123abcd"), None);
    assert_eq!(leftover_name(".Launcher.hlsr-tmp-0123abcd"), None);
    assert_eq!(leftover_name(".Лаунчер.hlsr-new-0123abcd"), Some("Лаунчер"));
    // Кириллица в последних 18 байтах соседнего каталога: не остаток и без паники.
    assert_eq!(leftover_name(".aИгры и программы"), None);
    assert_eq!(leftover_name(".aaИгры и программы"), None);
    assert_eq!(leftover_name(".x.hlsr-new-0123abcЖ"), None);
    assert_eq!(leftover_name(".x.hlsr-нов-0123abcd"), None);
}

#[test]
fn existing_kind_tells_ours_from_foreign() {
    let (_guard, root) = temp_root();
    assert_eq!(
        existing_kind(&root.join("none")).unwrap(),
        ExistingKind::None
    );
    std::fs::create_dir(root.join("empty")).unwrap();
    assert_eq!(
        existing_kind(&root.join("empty")).unwrap(),
        ExistingKind::Empty
    );
    put_file(&root, "ours/hlsr.EXE", b"x");
    assert_eq!(
        existing_kind(&root.join("ours")).unwrap(),
        ExistingKind::Hlsr
    );
    put_file(&root, "marker/.HLSR-install-owner.json", b"x");
    assert_eq!(
        existing_kind(&root.join("marker")).unwrap(),
        ExistingKind::Hlsr
    );
    put_file(&root, "foreign/notes.txt", b"x");
    assert_eq!(
        existing_kind(&root.join("foreign")).unwrap(),
        ExistingKind::Foreign
    );
    put_file(&root, "file", b"x");
    assert_eq!(
        existing_kind(&root.join("file")).unwrap(),
        ExistingKind::Foreign
    );
}

/// Каталог занят (открытый без общего доступа файл внутри): 5 попыток по 400 мс при установке
/// человеком, затем `IN_USE`; рабочая установка и соседи нетронуты.
#[cfg(windows)]
#[test]
fn busy_install_directory_is_in_use_after_retries() {
    use std::os::windows::fs::OpenOptionsExt;
    let (_guard, root) = temp_root();
    let src = root.join(SOURCE);
    put_file(&src, "HLSR.exe", b"exe 1");
    put_file(&src, "resources/app.asar", b"asar");
    let list = file_list::build(&src, "1.0.0", &App::production()).unwrap();
    std::fs::write(src.join(file_list::RELATIVE), compact(&list.to_value())).unwrap();
    let install_dir = root.join("HLSR").join("Launcher");
    put_file(&install_dir, "HLSR.exe", b"running");
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(install_dir.join("HLSR.exe"))
        .unwrap();
    let registry = FakeRegistry::new();
    let id = layout::identity(None).unwrap();
    let shortcuts = Shortcuts::new(
        Arc::new(FakeShellLinks::new()),
        &FakeKnownFolders::under(&root),
        &id,
    )
    .unwrap();
    let log = MemoryLogger::default();
    let sleeps = Mutex::new(Vec::new());
    let sleep = |duration: std::time::Duration| sleeps.lock().unwrap().push(duration);
    let source = Source::Dir(src);
    let dir = install_dir.to_string_lossy().into_owned();
    let library = root.join("Library").to_string_lossy().into_owned();
    let mut options = InstallOptions::new(&source, &dir, &registry, &shortcuts, &log);
    options.env = &|_| None;
    options.library_root = &library;
    options.sleep = &sleep;
    let error = install::install(options).unwrap_err();
    drop(held);
    assert_eq!(error.code.as_deref(), Some("IN_USE"), "{error}");
    assert!(
        error.message.starts_with("Файлы HLSR заняты (E"),
        "{}",
        error.message
    );
    assert_eq!(
        *sleeps.lock().unwrap(),
        vec![std::time::Duration::from_millis(400); 4]
    );
    assert_eq!(
        std::fs::read(install_dir.join("HLSR.exe")).unwrap(),
        b"running"
    );
    let names = hlsr_core::js::fs::read_dir_names(&root.join("HLSR")).unwrap();
    assert_eq!(names, ["Launcher"], "стадия убрана, резервной папки нет");
    assert!(registry.ops().is_empty());
}

/// Ссылка пользователя на каталог (junction) переносится в новую версию junction-ом на ту же цель
/// (`fs.symlinkSync(…, "junction")`), а не копией содержимого.
#[cfg(windows)]
#[test]
fn user_junction_survives_an_update_as_a_junction() {
    let (_guard, root) = temp_root();
    let src = root.join(SOURCE);
    put_file(&src, "HLSR.exe", b"exe 2");
    put_file(&src, "resources/app.asar", b"asar");
    let list = file_list::build(&src, "2.0.0", &App::production()).unwrap();
    std::fs::write(src.join(file_list::RELATIVE), compact(&list.to_value())).unwrap();
    let install_dir = root.join("HLSR").join("Launcher");
    put_file(&install_dir, "HLSR.exe", b"exe 1");
    put_file(&root, "Demos/run.dem", b"demo");
    hlsr_update::windows::file_ops::create_junction(
        &root.join("Demos"),
        &install_dir.join("demos"),
    )
    .unwrap();
    assert_eq!(
        std::fs::read(install_dir.join("demos").join("run.dem")).unwrap(),
        b"demo"
    );
    let registry = FakeRegistry::new();
    let id = layout::identity(None).unwrap();
    let shortcuts = Shortcuts::new(
        Arc::new(FakeShellLinks::new()),
        &FakeKnownFolders::under(&root),
        &id,
    )
    .unwrap();
    let log = MemoryLogger::default();
    let source = Source::Dir(src);
    let dir = install_dir.to_string_lossy().into_owned();
    let library = root.join("Library").to_string_lossy().into_owned();
    let mut options = InstallOptions::new(&source, &dir, &registry, &shortcuts, &log);
    options.env = &|_| None;
    options.library_root = &library;
    options.sleep = &|_| {};
    options.update = true;
    install::install(options).unwrap();
    let link = install_dir.join("demos");
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        hlsr_update::windows::file_ops::read_link(&link).unwrap(),
        root.join("Demos")
    );
    assert_eq!(std::fs::read(link.join("run.dem")).unwrap(), b"demo");
    assert_eq!(
        std::fs::read(root.join("Demos").join("run.dem")).unwrap(),
        b"demo"
    );
}

/// Временный корень на другом томе, чем `%TEMP%` (первый из `D:\`, `E:\`, куда можно писать), иначе
/// рядом с `%TEMP%`: тогда проверяется всё, кроме перехода между томами.
fn other_volume_root() -> (tempfile::TempDir, PathBuf) {
    let temp_drive = std::env::temp_dir().to_string_lossy()[..1].to_uppercase();
    for drive in ["D", "E"] {
        if drive == temp_drive {
            continue;
        }
        if let Ok(dir) = tempfile::Builder::new()
            .prefix("hlsr-drives-")
            .tempdir_in(format!("{drive}:\\"))
        {
            let path = dir.path().to_path_buf();
            return (dir, path);
        }
    }
    eprintln!("второго тома нет — проверка на одном томе");
    temp_root()
}

/// Сборка `version` в `<root>\setup-unpacked` и параметры установки обновлением в `dir`.
fn update_world(root: &Path, version: &str) -> (Source, FakeRegistry, Shortcuts, MemoryLogger) {
    let src = root.join(SOURCE);
    put_file(&src, "HLSR.exe", format!("exe {version}").as_bytes());
    put_file(&src, "resources/app.asar", b"asar");
    let list = file_list::build(&src, version, &App::production()).unwrap();
    std::fs::write(src.join(file_list::RELATIVE), compact(&list.to_value())).unwrap();
    let id = layout::identity(None).unwrap();
    let shortcuts = Shortcuts::new(
        Arc::new(FakeShellLinks::new()),
        &FakeKnownFolders::under(root),
        &id,
    )
    .unwrap();
    (
        Source::Dir(src),
        FakeRegistry::new(),
        shortcuts,
        MemoryLogger::default(),
    )
}

/// Обновление с другого тома: сборка во `%TEMP%` (C:), лаунчер на D: в пути с пробелом и кириллицей.
/// Новая версия встаёт на место, файлы пользователя остаются, рядом нет стадий и резервных копий.
#[test]
fn update_from_temp_on_another_volume_into_a_spaced_cyrillic_dir() {
    let (_temp_guard, temp) = temp_root();
    let (_guard, volume) = other_volume_root();
    let install_dir = volume.join("HLSR Тест").join("Launcher");
    put_file(&install_dir, "HLSR.exe", b"exe 1.0.0");
    put_file(&install_dir, "userdata/Мой конфиг.cfg", b"bind w +forward");
    let (source, registry, shortcuts, log) = update_world(&temp, "2.0.0");
    let dir = install_dir.to_string_lossy().into_owned();
    let library = volume.join("HLSR Тест").to_string_lossy().into_owned();
    let mut options = InstallOptions::new(&source, &dir, &registry, &shortcuts, &log);
    options.env = &|_| None;
    options.library_root = &library;
    options.sleep = &|_| {};
    options.update = true;
    let result = install::install(options).unwrap();
    assert_eq!(result.install_dir, dir);
    assert_eq!(
        std::fs::read(install_dir.join("HLSR.exe")).unwrap(),
        b"exe 2.0.0"
    );
    assert_eq!(
        std::fs::read(install_dir.join("userdata").join("Мой конфиг.cfg")).unwrap(),
        b"bind w +forward"
    );
    let mut names = hlsr_core::js::fs::read_dir_names(&volume.join("HLSR Тест")).unwrap();
    names.sort();
    assert_eq!(names, [".hlsr", "Downloads", "Games", "Launcher"]);
    let location = registry
        .read(&layout::identity(None).unwrap().install_key)
        .and_then(|values| install::sz_value(&values, "InstallLocation"));
    assert_eq!(location.as_deref(), Some(dir.as_str()));
}

/// Каталог лаунчера — junction на другой том (лаунчер перенесли и оставили ссылку): обновление
/// встаёт в настоящий каталог, ссылка остаётся ссылкой, рядом с ней ничего не появляется.
#[test]
fn update_through_a_junction_lands_in_the_real_dir() {
    let (_temp_guard, temp) = temp_root();
    let (_guard, volume) = other_volume_root();
    let real = volume.join("Лаунчер");
    put_file(&real, "HLSR.exe", b"exe 1.0.0");
    put_file(&real, "userdata/keep.txt", b"keep");
    let link = temp.join("HLSR").join("Launcher");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    hlsr_update::windows::file_ops::create_junction(&real, &link).unwrap();
    let (source, registry, shortcuts, log) = update_world(&temp, "2.0.0");
    let dir = link.to_string_lossy().into_owned();
    let library = temp.join("Library").to_string_lossy().into_owned();
    let mut options = InstallOptions::new(&source, &dir, &registry, &shortcuts, &log);
    options.env = &|_| None;
    options.library_root = &library;
    options.sleep = &|_| {};
    options.update = true;
    let result = install::install(options).unwrap();
    assert_eq!(result.install_dir, real.to_string_lossy());
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(std::fs::read(link.join("HLSR.exe")).unwrap(), b"exe 2.0.0");
    assert_eq!(
        std::fs::read(real.join("userdata").join("keep.txt")).unwrap(),
        b"keep"
    );
    assert_eq!(
        hlsr_core::js::fs::read_dir_names(&temp.join("HLSR")).unwrap(),
        ["Launcher"]
    );
    assert_eq!(
        hlsr_core::js::fs::read_dir_names(&volume).unwrap(),
        ["Лаунчер"]
    );
}

#[test]
fn new_install_ids_are_uppercase_uuid_v4() {
    let first = hlsr_update::windows::file_ops::new_install_id();
    let second = hlsr_update::windows::file_ops::new_install_id();
    assert!(layout::is_install_id(&first), "{first}");
    assert_ne!(first, second);
    assert_eq!(&first[15..16], "4");
    assert!(matches!(&first[20..21], "8" | "9" | "A" | "B"));
    let suffix = hlsr_update::windows::file_ops::random_suffix();
    assert_eq!(suffix.len(), 8);
    assert!(
        suffix
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    );
}

/// Живой скрипт удаления (настоящий PowerShell, без ожидания PID): каталог программы, данные и
/// опустевшая папка HLSR уходят; цель junction, каталог другой установки HLSR и read-only файл с
/// жёсткой ссылкой наружу остаются. Только на невидимом столе: `python
/// scripts/slint/headless-win.py -- cargo test -p hlsr-update --test windows_install -- --ignored
/// live_removal`.
#[test]
#[ignore = "запускает PowerShell; только через headless-win.py"]
fn live_removal_script_removes_own_and_keeps_foreign() {
    assert!(
        std::env::var_os("HLSR_HEADLESS_DESKTOP").is_some(),
        "только на невидимом столе"
    );
    let (_guard, root) = temp_root();
    let own = r#"{"schemaVersion":1,"appId":"x","installId":"{OWN}"}"#;
    let program = root.join("HLSR").join("Launcher");
    put_file(&program, "HLSR.exe", b"exe");
    put_file(
        &program,
        ".hlsr-install-owner.json",
        format!("{own}\r\n").as_bytes(),
    );
    put_file(&program, "resources/app.asar", b"asar");
    let data = root.join("AppData").join("hlsr");
    put_file(&data, "settings.json", b"{}");
    put_file(&root, "Outside/keep.txt", b"keep");
    hlsr_update::windows::file_ops::create_junction(&root.join("Outside"), &data.join("link"))
        .unwrap();
    put_file(&data, "foreign/.hlsr-install-owner.json", b"{\"other\":1}");
    put_file(&data, "foreign/HLSR.exe", b"other");
    put_file(&root, "Outside/hard.txt", b"hard");
    std::fs::hard_link(root.join("Outside").join("hard.txt"), data.join("hard.txt")).unwrap();
    let mut readonly = std::fs::metadata(data.join("hard.txt"))
        .unwrap()
        .permissions();
    readonly.set_readonly(true);
    std::fs::set_permissions(data.join("hard.txt"), readonly).unwrap();

    let text = |path: &Path| path.to_string_lossy().into_owned();
    let data_dirs = [text(&data)];
    let parent = text(&root.join("HLSR"));
    uninstall::schedule_removal(
        &SystemProcesses,
        Some(&text(&program)),
        &Env::from_process(),
        None,
        &RemovalTargets {
            data: &data_dirs,
            parent: Some(&parent),
            pid: None,
            own_marker: Some(own),
        },
        &root,
    )
    .unwrap();
    // Данные с чужим остатком скрипт повторяет 60 раз по секунде, папку HLSR удаляет после.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while root.join("HLSR").exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    assert!(!program.exists(), "каталог программы удалён");
    assert!(!root.join("HLSR").exists(), "опустевшая папка HLSR удалена");
    assert!(!data.join("settings.json").exists());
    assert!(!data.join("link").exists(), "junction удалён сам");
    assert!(
        root.join("Outside").join("keep.txt").exists(),
        "цель junction цела"
    );
    assert!(
        data.join("foreign").join("HLSR.exe").exists(),
        "другая установка цела"
    );
    assert!(
        data.join("hard.txt").exists(),
        "read-only с жёсткой ссылкой остаётся"
    );
    let mut writable = std::fs::metadata(data.join("hard.txt"))
        .unwrap()
        .permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    writable.set_readonly(false);
    std::fs::set_permissions(data.join("hard.txt"), writable).unwrap();
}
