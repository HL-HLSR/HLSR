//! Строки интерфейса: `locales/{en,ru}.json` → таблица для `tr()` в Rust.

use crate::{BuildError, TargetOs, invalid, write_if_changed};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;

/// Строки для одной ОС сборки, в порядке `en.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub en: Vec<(String, String)>,
    /// Те же ключи, что в `en`; пустые и отсутствующие строки — английские.
    pub ru: Vec<(String, String)>,
    /// Ключи, для которых в `ru` взята английская строка.
    pub ru_from_en: Vec<String>,
}

/// Прочитать `en.json` и `ru.json` и выбрать вариант для `os`.
///
/// Значение — строка или `{"windows": …, "linux": …}`. Пустая или отсутствующая
/// строка `ru` заменяется английской, как `LocalizationHelper.get` в 0.4.7;
/// ключ попадает в `ru_from_en`. Ошибка: ключ есть в `ru`, но нет в `en`;
/// пустая строка в `en` (0.4.7 показал бы вместо неё сам ключ).
pub fn resolve(en: &Path, ru: &Path, os: TargetOs) -> Result<Resolved, BuildError> {
    let en_list = read_locale(en, os)?;
    let ru_list = read_locale(ru, os)?;

    if let Some((key, _)) = en_list.iter().find(|(_, text)| text.is_empty()) {
        return Err(invalid(en, format!("{key}: пустая строка")));
    }
    let en_keys: HashSet<&str> = en_list.iter().map(|(key, _)| key.as_str()).collect();
    let extra: Vec<&str> = ru_list
        .iter()
        .map(|(key, _)| key.as_str())
        .filter(|key| !en_keys.contains(key))
        .collect();
    if !extra.is_empty() {
        return Err(invalid(
            ru,
            format!("ключей нет в en.json: {}", extra.join(", ")),
        ));
    }

    let ru_map: HashMap<&str, &str> = ru_list
        .iter()
        .map(|(key, text)| (key.as_str(), text.as_str()))
        .collect();
    let mut ru_from_en = Vec::new();
    let ru = en_list
        .iter()
        .map(|(key, en_text)| {
            let text = match ru_map.get(key.as_str()) {
                Some(text) if !text.is_empty() => text.to_string(),
                _ => {
                    ru_from_en.push(key.clone());
                    en_text.clone()
                }
            };
            (key.clone(), text)
        })
        .collect();
    Ok(Resolved {
        en: en_list,
        ru,
        ru_from_en,
    })
}

fn read_locale(path: &Path, os: TargetOs) -> Result<Vec<(String, String)>, BuildError> {
    let text = fs::read_to_string(path).map_err(|source| BuildError::Io {
        path: path.to_owned(),
        source,
    })?;
    let value: Value = serde_json::from_str(&text).map_err(|source| BuildError::Json {
        path: path.to_owned(),
        source,
    })?;
    let Value::Object(map) = value else {
        return Err(invalid(path, "корень — не объект".to_owned()));
    };
    map.into_iter()
        .map(|(key, value)| {
            let text = match value {
                Value::String(text) => text,
                Value::Object(variants) => match variants.get(os.variant_key()) {
                    Some(Value::String(text)) => text.clone(),
                    _ => {
                        return Err(invalid(
                            path,
                            format!("{key}: нет строки варианта {}", os.variant_key()),
                        ));
                    }
                },
                _ => {
                    return Err(invalid(
                        path,
                        format!("{key}: не строка и не варианты платформ"),
                    ));
                }
            };
            Ok((key, text))
        })
        .collect()
}

/// Файл Rust с `EN` и `RU`: пары (ключ, текст 0.4.7 с `%N%`), по возрастанию
/// ключа — для двоичного поиска. Неизменившийся файл не перезаписывается.
pub fn write_rust_table(path: &Path, resolved: &Resolved) -> io::Result<()> {
    let mut out = String::from(
        "// Сгенерировано hlsr-ui-build из locales/*.json, не править.\n\
         // Пары отсортированы по ключу для двоичного поиска.\n",
    );
    for (name, list) in [("EN", &resolved.en), ("RU", &resolved.ru)] {
        let mut sorted: Vec<&(String, String)> = list.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        let _ = write!(out, "\npub static {name}: &[(&str, &str)] = &[\n");
        for (key, text) in sorted {
            let _ = writeln!(out, "    ({key:?}, {text:?}),");
        }
        out.push_str("];\n");
    }
    write_if_changed(path, out.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TargetOs;
    use std::path::{Path, PathBuf};

    fn ui(path: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../hlsr-ui")
            .join(path)
    }

    fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    fn resolve_text(en: &str, ru: &str, os: TargetOs) -> Result<Resolved, crate::BuildError> {
        let tmp = tempfile::tempdir().unwrap();
        let en = write(tmp.path(), "en.json", en);
        let ru = write(tmp.path(), "ru.json", ru);
        resolve(&en, &ru, os)
    }

    fn pairs(list: &[(String, String)]) -> Vec<(&str, &str)> {
        list.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect()
    }

    #[test]
    fn platform_variants_and_ru_fallback() {
        let en = r##"{"#A": {"windows": "W", "linux": "L"}, "#B": "b", "#C": "c"}"##;
        let ru = r##"{"#A": {"windows": "Ш", "linux": "Л"}, "#B": ""}"##;
        let r = resolve_text(en, ru, TargetOs::Linux).unwrap();
        assert_eq!(pairs(&r.en), [("#A", "L"), ("#B", "b"), ("#C", "c")]);
        assert_eq!(pairs(&r.ru), [("#A", "Л"), ("#B", "b"), ("#C", "c")]);
        assert_eq!(r.ru_from_en, ["#B", "#C"]);

        let r = resolve_text(en, ru, TargetOs::Windows).unwrap();
        assert_eq!(pairs(&r.en)[0], ("#A", "W"));
        assert_eq!(pairs(&r.ru)[0], ("#A", "Ш"));
    }

    #[test]
    fn resolve_rejects_bad_files() {
        let ok = r##"{"#A": "a"}"##;
        for (en, ru, what) in [
            (ok, r##"{"#A": "а", "#Z": "з"}"##, "#Z"),
            (r##"{"#A": 1}"##, ok, "#A"),
            (r##"{"#A": {"windows": "W"}}"##, ok, "linux"),
            (r##"{"#A": ""}"##, ok, "#A"),
            ("[]", ok, "объект"),
            ("{", ok, "JSON"),
        ] {
            let error = resolve_text(en, ru, TargetOs::Linux)
                .unwrap_err()
                .to_string();
            assert!(error.contains(what), "{en} / {ru}: {error}");
        }
    }

    #[test]
    fn rust_table_is_sorted_and_escaped() {
        let tmp = tempfile::tempdir().unwrap();
        let resolved = Resolved {
            en: vec![
                ("#B".into(), "b %0%".into()),
                ("#A".into(), "a \"q\"\n\\".into()),
            ],
            ru: vec![("#B".into(), "б".into()), ("#A".into(), "а".into())],
            ru_from_en: vec![],
        };
        let path = tmp.path().join("strings.rs");
        write_rust_table(&path, &resolved).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let en_at = text.find("pub static EN").unwrap();
        let ru_at = text.find("pub static RU").unwrap();
        let en_part = &text[en_at..ru_at];
        assert!(en_part.find("\"#A\"").unwrap() < en_part.find("\"#B\"").unwrap());
        assert!(en_part.contains(r##"("#A", "a \"q\"\n\\"),"##), "{text}");
        assert!(en_part.contains(r##"("#B", "b %0%"),"##), "{text}");
        assert!(text[ru_at..].contains(r##"("#A", "а"),"##), "{text}");
    }

    #[test]
    fn writes_only_changed_files() {
        let tmp = tempfile::tempdir().unwrap();
        let resolved = Resolved {
            en: vec![("#A".into(), "a".into())],
            ru: vec![("#A".into(), "а".into())],
            ru_from_en: vec![],
        };
        let table = tmp.path().join("strings.rs");
        write_rust_table(&table, &resolved).unwrap();
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        std::fs::File::options()
            .write(true)
            .open(&table)
            .unwrap()
            .set_modified(old)
            .unwrap();
        write_rust_table(&table, &resolved).unwrap();
        assert_eq!(std::fs::metadata(&table).unwrap().modified().unwrap(), old);
    }

    #[test]
    fn real_locales_resolve_for_both_targets() {
        for os in [TargetOs::Windows, TargetOs::Linux] {
            let r = resolve(&ui("locales/en.json"), &ui("locales/ru.json"), os).unwrap();
            // 0.4.9: 2481 строка рендерера, 2 трея и 113 окон установщика и апдейтера.
            assert_eq!(r.en.len(), 2596);
            assert_eq!(r.ru.len(), 2596);
            assert!(r.ru_from_en.is_empty());
        }
    }
}
