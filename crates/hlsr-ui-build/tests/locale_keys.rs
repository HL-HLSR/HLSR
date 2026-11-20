//! Каждый ключ строки из кода `hlsr-ui/src` есть в `locales/en.json` и `ru.json` (строки 0.4.9):
//! пропавший ключ `tr()` показал бы в интерфейсе как есть, а число строк этого не ловит.

use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Ключи потоков 0.4.7, которых в 0.4.9 нет. Перенос на 0.4.9 убирает их из кода и строку отсюда:
/// подтверждение ремонта → окно RepairModal (G3), «Проверить обновления» в меню игры (G2),
/// вкладка «Лаунчер» персонализации (G4).
const STALE: [&str; 6] = [
    "#L_C_REPAIR",
    "#L_GM_REPAIR_CONFIRM",
    "#L_GM_REPAIR_DOWNLOAD",
    "#L_GM_REPAIR_MIRROR",
    "#L_GM_UPDATES",
    "#L_PZ_LAUNCHER",
];
/// Начала ключей, к которым `count_word` дописывает `_ONE`/`_FEW`/`_MANY`.
const PREFIXES: [&str; 2] = ["#L_PLAYERS_UNIT_ACHIEVEMENTS", "#L_PLAYERS_UNIT_LAUNCHES"];

fn ui(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../hlsr-ui")
        .join(path)
}

fn locale(name: &str) -> Map<String, Value> {
    match serde_json::from_str(&fs::read_to_string(ui(name)).unwrap()).unwrap() {
        Value::Object(map) => map,
        _ => panic!("{name}: корень — не объект"),
    }
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for path in fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
    {
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// `#L_…`, `#UI_…` и прочие `#ВЕРХНИЙ_…` (у цвета `#RRGGBB` нет `_`), `installer.…`, `updater.…`.
fn is_key(text: &str) -> bool {
    let word = |s: &str, extra: char| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == extra)
    };
    match text.strip_prefix('#') {
        Some(rest) => {
            rest.starts_with(|c: char| c.is_ascii_uppercase())
                && rest.contains('_')
                && word(rest, '_')
        }
        None => ["installer.", "updater."].iter().any(|prefix| {
            text.strip_prefix(prefix)
                .is_some_and(|rest| word(rest, '.'))
        }),
    }
}

#[test]
fn every_key_used_in_code_is_in_both_locales() {
    let (en, ru) = (locale("locales/en.json"), locale("locales/ru.json"));
    let mut files = Vec::new();
    rust_files(&ui("src"), &mut files);
    let mut checked = 0;
    let mut missing = BTreeSet::new();
    for file in &files {
        let text = fs::read_to_string(file).unwrap();
        let quotes: Vec<usize> = text.match_indices('"').map(|(at, _)| at).collect();
        for key in quotes
            .windows(2)
            .map(|w| &text[w[0] + 1..w[1]])
            .filter(|s| is_key(s))
        {
            checked += 1;
            // `"#L_ACH_"` и `PREFIXES` — начало ключа, остальное — ключ целиком.
            let prefix = if key.ends_with('_') {
                Some(key.to_owned())
            } else {
                PREFIXES.contains(&key).then(|| format!("{key}_"))
            };
            let known = |map: &Map<String, Value>| match &prefix {
                Some(prefix) => map.keys().any(|k| k.starts_with(prefix.as_str())),
                None => map.contains_key(key),
            };
            if !(known(&en) && known(&ru)) && !STALE.contains(&key) {
                let at = file.strip_prefix(ui("src")).unwrap().display();
                missing.insert(format!("{key} ({at})"));
            }
        }
    }
    assert!(
        checked > 0,
        "в {} не найдено ни одного ключа",
        ui("src").display()
    );
    assert!(
        missing.is_empty(),
        "ключей нет в en.json или ru.json:\n{}",
        missing.into_iter().collect::<Vec<_>>().join("\n")
    );
}
