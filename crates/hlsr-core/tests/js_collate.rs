//! Порядок строк против `localeCompare` Node 24 (V8 с ICU). Фикстура `node-semantics-collate.json`
//! из `tools/reference/node-semantics-collate.js`: знаки сравнения всех пар и порядок после
//! устойчивой сортировки перемешанных копий списка — для трёх языков и двух `sensitivity`.

use hlsr_core::js::collate::{Collator, Sensitivity, default_locale, posix_locale};
use serde_json::Value;
use std::cmp::Ordering;

fn fixture() -> Value {
    let file = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/node-semantics-collate.json"
    );
    serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
}

fn str_of(value: &Value) -> &str {
    value.as_str().expect("строка")
}

/// Знак сравнения Rust → символ матрицы фикстуры.
fn sign(order: Ordering) -> char {
    match order {
        Ordering::Less => '<',
        Ordering::Equal => '=',
        Ordering::Greater => '>',
    }
}

fn numbers(value: &Value) -> Vec<usize> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_u64().unwrap() as usize)
        .collect()
}

#[test]
fn locale_compare_matches_node_icu_for_pairs_and_sorting() {
    let fixture = fixture();
    let names: Vec<&str> = fixture["names"]
        .as_array()
        .unwrap()
        .iter()
        .map(str_of)
        .collect();
    let mut wrong = Vec::new();
    for (key, case) in fixture["cases"].as_object().unwrap() {
        let (locale, sensitivity) = key.split_once('/').unwrap();
        let sensitivity = match sensitivity {
            "base" => Sensitivity::Base,
            _ => Sensitivity::Variant,
        };
        let collator = Collator::new(locale, sensitivity);
        for (row, expected) in case["matrix"].as_array().unwrap().iter().enumerate() {
            let got: String = names
                .iter()
                .map(|other| sign(collator.compare(names[row], other)))
                .collect();
            for (column, (g, e)) in got.chars().zip(str_of(expected).chars()).enumerate() {
                if g != e {
                    wrong.push(format!(
                        "{key}: {:?} против {:?}: Rust {g}, Node {e}",
                        names[row], names[column]
                    ));
                }
            }
        }
        for pair in case["sorts"].as_array().unwrap() {
            let mut order = numbers(&pair[0]);
            let expected = numbers(&pair[1]);
            order.sort_by(|&a, &b| collator.compare(names[a], names[b]));
            if order != expected {
                wrong.push(format!("{key}: порядок сортировки не совпал"));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{} расхождений:\n{}",
        wrong.len(),
        wrong[..wrong.len().min(40)].join("\n")
    );
}

/// Окружение для `posix_locale` из объекта фикстуры.
fn env_of(env: &Value) -> impl Fn(&str) -> Option<String> + '_ {
    move |name| env.get(name).and_then(Value::as_str).map(str::to_owned)
}

/// Фикстура снята в main-процессе Electron 0.4.7 (не `ELECTRON_RUN_AS_NODE`): Chromium вызывает
/// `setlocale(LC_ALL, "")`, поэтому пустые переменные пропускаются. Локали в наборе установлены
/// (ru_RU, en_US, C.UTF-8) либо разбираются по имени; неустановленная локаль после пустой
/// переменной даёт у Electron `und` и не моделируется (см. [`posix_locale`]).
#[test]
fn default_locale_is_taken_from_the_environment_like_electron_main() {
    let fixture = fixture();
    let mut wrong = Vec::new();
    for case in fixture["defaultLocale"].as_array().unwrap() {
        // Node дописывает к `@модификатору` закрытое расширение `-x-lvariant-…`.
        let node = str_of(&case[1]).split("-x-lvariant-").next().unwrap();
        let ours = posix_locale(env_of(&case[0]));
        if ours != node {
            wrong.push(format!("{}: Rust {ours}, Node {node}", case[0]));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn language_variable_does_not_choose_the_locale_but_lc_all_lc_messages_lang_do() {
    let pick = |env: Value| posix_locale(env_of(&env));
    // LANGUAGE — выбор переводов gettext, ICU его не читает (в отличие от `sys_locale`).
    assert_eq!(
        pick(serde_json::json!({"LANG": "en_US.UTF-8", "LANGUAGE": "ru"})),
        "en-US"
    );
    assert_eq!(
        pick(serde_json::json!({"LANG": "ru_RU.UTF-8", "LANGUAGE": "en_US:en"})),
        "ru-RU"
    );
    assert_eq!(
        pick(serde_json::json!({"LANGUAGE": "ru_RU.UTF-8"})),
        "en-US"
    );
    assert_eq!(pick(serde_json::json!({})), "en-US");
    // Порядок: LC_ALL, LC_MESSAGES, LANG; C и POSIX — en-US и дальше не ищут.
    assert_eq!(
        pick(serde_json::json!({"LC_ALL": "de_DE.UTF-8", "LC_MESSAGES": "ru_RU", "LANG": "fr_FR"})),
        "de-DE"
    );
    assert_eq!(
        pick(serde_json::json!({"LC_MESSAGES": "ru_RU", "LANG": "fr_FR"})),
        "ru-RU"
    );
    assert_eq!(
        pick(serde_json::json!({"LC_ALL": "C", "LANG": "ru_RU.UTF-8"})),
        "en-US"
    );
    assert_eq!(pick(serde_json::json!({"LANG": "POSIX"})), "en-US");
    // Заданная пустая переменная пропускается (glibc, `setlocale(LC_ALL, "")` в main Chromium).
    assert_eq!(
        pick(serde_json::json!({"LC_ALL": "", "LANG": "ru_RU.UTF-8"})),
        "ru-RU"
    );
    assert_eq!(
        pick(serde_json::json!({"LC_ALL": "", "LC_MESSAGES": "", "LANG": "ru_RU.UTF-8"})),
        "ru-RU"
    );
    assert_eq!(
        pick(serde_json::json!({"LC_ALL": "", "LC_MESSAGES": "ru_RU.UTF-8"})),
        "ru-RU"
    );
    // Непустой нет или первая непустая — C/POSIX: ICU читает окружение напрямую, пустая даёт `und`;
    // C.UTF-8 для glibc обычная локаль — en-US.
    assert_eq!(pick(serde_json::json!({"LC_ALL": ""})), "und");
    assert_eq!(pick(serde_json::json!({"LC_ALL": "", "LANG": "C"})), "und");
    assert_eq!(
        pick(serde_json::json!({"LC_ALL": "", "LANG": "C.UTF-8"})),
        "en-US"
    );
    assert_eq!(
        pick(serde_json::json!({"LC_ALL": "C", "LANG": ""})),
        "en-US"
    );
    // Выбранный язык действительно меняет порядок.
    let ordered = |env: Value| {
        Collator::new(&posix_locale(env_of(&env)), Sensitivity::Base)
            .compare("моя.cfg", "config.cfg")
    };
    assert_eq!(
        ordered(serde_json::json!({"LANG": "ru_RU.UTF-8"})),
        Ordering::Less
    );
    assert_eq!(
        ordered(serde_json::json!({"LANG": "ru_RU.UTF-8", "LC_ALL": "C"})),
        Ordering::Greater
    );
    // Пустые LC_ALL и LC_MESSAGES не мешают LANG: у этого пользователя fish оставляет их пустыми.
    assert_eq!(
        ordered(serde_json::json!({"LC_ALL": "", "LC_MESSAGES": "", "LANG": "ru_RU.UTF-8"})),
        Ordering::Less
    );
}

#[test]
fn process_default_locale_can_be_parsed() {
    let locale = default_locale();
    assert!(!locale.is_empty() && !locale.contains('\0') && !locale.contains('_'));
    // Не падает на любом языке и даёт сравнение.
    let _ = Collator::new(&locale, Sensitivity::Variant).compare("a", "b");
}

#[test]
fn russian_puts_cyrillic_before_latin_and_english_after() {
    let ru = Collator::new("ru-RU", Sensitivity::Base);
    let en = Collator::new("en-US", Sensitivity::Base);
    assert_eq!(ru.compare("моя.cfg", "config.cfg"), Ordering::Less);
    assert_eq!(en.compare("моя.cfg", "config.cfg"), Ordering::Greater);
    // base: регистр и ударение не различаются; variant: строчная раньше заглавной.
    assert_eq!(en.compare("Config.cfg", "config.cfg"), Ordering::Equal);
    assert_eq!(
        Collator::new("en-US", Sensitivity::Variant).compare("config.cfg", "Config.cfg"),
        Ordering::Less
    );
}
