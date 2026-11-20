//! hlsr-core::js против Node 24 (main-процесс 0.4.7):
//! фикстура `node-semantics.json` из `tools/reference/node-semantics.js`.

use hlsr_core::js::collate::{Collator, Sensitivity};
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, Platform};
use hlsr_core::json_text::{js_number, stringify};
use serde_json::Value;
use std::cmp::Ordering;
use std::time::{Duration, SystemTime};

fn fixture() -> Value {
    let file = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/node-semantics.json"
    );
    serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
}

fn str_of(value: &Value) -> &str {
    value.as_str().expect("строка")
}

fn strings(value: &Value) -> Vec<&str> {
    value.as_array().unwrap().iter().map(str_of).collect()
}

#[test]
fn path_matches_node_posix_and_win32() {
    let fixture = fixture();
    for (flavor, platform) in [("posix", Platform::Linux), ("win32", Platform::Win32)] {
        let cases = &fixture["path"][flavor];
        let p = NodePath::new(platform, str_of(&cases["cwd"]));
        let mut failures = Vec::new();
        for case in cases["resolve"].as_array().unwrap() {
            let args = strings(&case[0]);
            let got = p.resolve(&args);
            if got != str_of(&case[1]) {
                failures.push(format!("resolve({args:?}) = {got:?}, Node: {}", case[1]));
            }
        }
        for case in cases["normalize"].as_array().unwrap() {
            let got = p.normalize(str_of(&case[0]));
            if got != str_of(&case[1]) {
                failures.push(format!(
                    "normalize({}) = {got:?}, Node: {}",
                    case[0], case[1]
                ));
            }
        }
        for case in cases["join"].as_array().unwrap() {
            let args = strings(&case[0]);
            let got = p.join(&args);
            if got != str_of(&case[1]) {
                failures.push(format!("join({args:?}) = {got:?}, Node: {}", case[1]));
            }
        }
        for case in cases["basename"].as_array().unwrap() {
            let got = p.basename(str_of(&case[0]));
            if got != str_of(&case[1]) {
                failures.push(format!(
                    "basename({}) = {got:?}, Node: {}",
                    case[0], case[1]
                ));
            }
        }
        for case in cases["extname"].as_array().unwrap() {
            let got = p.extname(str_of(&case[0]));
            if got != str_of(&case[1]) {
                failures.push(format!("extname({}) = {got:?}, Node: {}", case[0], case[1]));
            }
        }
        for case in cases["dirname"].as_array().unwrap() {
            let got = p.dirname(str_of(&case[0]));
            if got != str_of(&case[1]) {
                failures.push(format!("dirname({}) = {got:?}, Node: {}", case[0], case[1]));
            }
        }
        for case in cases["isAbsolute"].as_array().unwrap() {
            let got = p.is_absolute(str_of(&case[0]));
            if Some(got) != case[1].as_bool() {
                failures.push(format!(
                    "isAbsolute({}) = {got}, Node: {}",
                    case[0], case[1]
                ));
            }
        }
        for case in cases["toNamespacedPath"].as_array().unwrap() {
            let got = p.to_namespaced_path(str_of(&case[0]));
            if got != str_of(&case[1]) {
                failures.push(format!(
                    "toNamespacedPath({}) = {got:?}, Node: {}",
                    case[0], case[1]
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "path.{flavor}:\n{}",
            failures.join("\n")
        );
    }
}

fn number_text(n: f64) -> String {
    if n.is_nan() {
        "NaN".to_owned()
    } else if n.is_infinite() {
        if n > 0.0 { "Infinity" } else { "-Infinity" }.to_owned()
    } else {
        js_number(n).to_string()
    }
}

#[test]
fn number_from_string_matches_node() {
    for case in fixture()["number"].as_array().unwrap() {
        let input = str_of(&case[0]);
        assert_eq!(
            number_text(js::to_number(input)),
            str_of(&case[1]),
            "Number({input:?})"
        );
    }
}

#[test]
fn string_order_is_utf16() {
    for case in fixture()["compare"].as_array().unwrap() {
        let (a, b) = (str_of(&case[0]), str_of(&case[1]));
        let expected = match case[2].as_i64().unwrap() {
            -1 => Ordering::Less,
            0 => Ordering::Equal,
            _ => Ordering::Greater,
        };
        assert_eq!(js::cmp_utf16(a, b), expected, "{a:?} < {b:?}");
    }
}

#[test]
fn ignore_case_matches_regexp_i_flag() {
    for case in fixture()["ignoreCase"].as_array().unwrap() {
        let (pattern, text) = (str_of(&case[0]), str_of(&case[1]));
        let mut p = pattern.chars();
        let mut t = text.chars();
        let same = loop {
            match (p.next(), t.next()) {
                (None, None) => break true,
                (Some(a), Some(b))
                    if js::canonicalize_ignore_case(a) == js::canonicalize_ignore_case(b) => {}
                _ => break false,
            }
        };
        assert_eq!(
            same,
            case[2].as_bool().unwrap(),
            "/^{pattern}$/i.test({text:?})"
        );
    }
}

/// Каждый errno Linux 1..=133 — в обе стороны: код и текст ошибки настоящего
/// `fs.openSync` и `fs.promises.open` Node 24 (errno подставлен в open() через
/// LD_PRELOAD; незнакомый libuv — `Unknown system error -N`), имя и текст — как в
/// таблице Node.
#[cfg(target_os = "linux")]
#[test]
fn errno_names_and_fs_messages_match_node() {
    use hlsr_core::js::errno;
    let fixture = fixture();
    let section = &fixture["errno"];
    let mut failures = Vec::new();
    let opened = section["open"].as_array().unwrap();
    assert_eq!(opened.len(), 133);
    for row in opened {
        let raw = row["errno"].as_i64().unwrap() as i32;
        for (channel, call) in [("0", &row["sync"]), ("1", &row["async"])] {
            // libuv повторяет open() при EINTR — тогда подмена отдаёт ENOENT, и
            // строка говорит о повторе, а не о названии кода.
            if call["errno"].as_i64() != Some(-i64::from(raw)) {
                continue;
            }
            let error = std::io::Error::from_raw_os_error(raw);
            let path = format!("/nonexistent-hlsr-errno/{channel}/{raw}");
            let code = errno::code(&error);
            let message = errno::fs_message(&error, "open", &path, None);
            if code.as_deref() != Some(str_of(&call["code"])) || message != str_of(&call["message"])
            {
                failures.push(format!(
                    "errno {raw}: {code:?} {message:?}, Node: {} {}",
                    call["code"], call["message"]
                ));
            }
        }
    }
    for entry in section["table"].as_array().unwrap() {
        let raw = entry[0].as_i64().unwrap() as i32;
        let code = errno::code(&std::io::Error::from_raw_os_error(raw));
        if code.as_deref() != Some(str_of(&entry[1]))
            || errno::description(str_of(&entry[1])) != str_of(&entry[2])
        {
            failures.push(format!(
                "таблица Node: errno {raw} = {} {}, Rust: {code:?}",
                entry[1], entry[2]
            ));
        }
    }
    for entry in section["libuvOnly"].as_array().unwrap() {
        if errno::description(str_of(&entry[0])) != str_of(&entry[1]) {
            failures.push(format!(
                "текст {}: {:?}, Node: {}",
                entry[0],
                errno::description(str_of(&entry[0])),
                entry[1]
            ));
        }
    }
    for case in section["fsErrors"].as_array().unwrap() {
        let error = std::io::Error::from_raw_os_error(case["errno"].as_i64().unwrap() as i32);
        let message = errno::fs_message(
            &error,
            str_of(&case["syscall"]),
            str_of(&case["path"]),
            case["dest"].as_str(),
        );
        if message != str_of(&case["message"]) {
            failures.push(format!("{message:?}, Node: {}", case["message"]));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Перевод кодов Win32 в коды libuv против таблицы, снятой с electron.exe 0.4.7
/// под Wine (`node-semantics-win32.json`): все коды 1..=16000 в обе стороны, тексты.
/// Таблица Rust — чистая функция, поэтому проверяется и на Linux.
#[test]
fn win32_codes_match_libuv_of_electron() {
    use hlsr_core::js::errno;
    let file = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/node-semantics-win32.json"
    );
    let fixture: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    let mut expected = std::collections::HashMap::new();
    for entry in fixture["translate"].as_array().unwrap() {
        let name = str_of(&entry[1]);
        expected.insert(entry[0].as_i64().unwrap() as i32, name);
        assert_eq!(errno::description(name), str_of(&entry[2]), "текст {name}");
    }
    let wrong: Vec<String> = (1..=16000)
        .filter_map(|code| {
            let node = expected.get(&code).copied().unwrap_or("UNKNOWN");
            let ours = errno::win32_code(code);
            (ours != node).then(|| format!("{code}: Rust {ours}, libuv {node}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn stringify_with_gap_matches_json_stringify() {
    for case in fixture()["stringify"].as_array().unwrap() {
        for (gap, expected) in ["", " ", "  ", "\t"]
            .iter()
            .zip(case[1].as_array().unwrap())
        {
            assert_eq!(stringify(&case[0], gap), str_of(expected), "gap {gap:?}");
        }
    }
}

#[test]
fn iso_string_matches_to_iso_string() {
    for case in fixture()["isoString"].as_array().unwrap() {
        let ms = case[0].as_u64().unwrap();
        let time = SystemTime::UNIX_EPOCH + Duration::from_millis(ms);
        assert_eq!(js::iso_string(time), str_of(&case[1]));
    }
}

#[test]
fn buffer_from_base64_matches_node() {
    for case in fixture()["base64"].as_array().unwrap() {
        let input = str_of(&case[0]);
        let hex: String = js::buffer_from_base64(input)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(hex, str_of(&case[1]), "Buffer.from({input:?}, \"base64\")");
    }
}

fn hex_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
        .collect()
}

#[test]
fn buffer_latin1_and_utf8_match_node() {
    let fixture = fixture();
    for case in fixture["latin1Encode"].as_array().unwrap() {
        let input = str_of(&case[0]);
        let hex: String = js::latin1_bytes(input)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(hex, str_of(&case[1]), "Buffer.from({input:?}, \"latin1\")");
    }
    for case in fixture["latin1Decode"].as_array().unwrap() {
        let bytes = hex_bytes(str_of(&case[0]));
        assert_eq!(
            js::latin1_string(&bytes),
            str_of(&case[1]),
            "{} .toString(\"latin1\")",
            case[0]
        );
    }
    for case in fixture["utf8Decode"].as_array().unwrap() {
        let bytes = hex_bytes(str_of(&case[0]));
        assert_eq!(
            js::utf8_string(&bytes),
            str_of(&case[1]),
            "{} .toString(\"utf8\")",
            case[0]
        );
    }
}

#[test]
fn math_round_matches_node() {
    for case in fixture()["round"].as_array().unwrap() {
        let input = str_of(&case[0]);
        let rounded = js::math_round(js::to_number(input));
        assert_eq!(
            number_text(rounded),
            str_of(&case[1]),
            "Math.round({input})"
        );
    }
}

#[test]
fn date_parse_matches_v8() {
    // Фикстура снята при TZ=UTC: дата без зоны — местное время со смещением 0.
    let mut failures = Vec::new();
    for case in fixture()["dateParse"].as_array().unwrap() {
        let input = str_of(&case[0]);
        let got = js::date_parse_with(input, |_| 0);
        let expected = case[1].as_i64();
        if got != expected {
            failures.push(format!(
                "Date.parse({input:?}) = {got:?}, Node: {expected:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn date_parse_reads_zone_less_dates_as_local_time() {
    let three_hours = 3 * 3_600_000;
    assert_eq!(
        js::date_parse_with("2026-10-15T12:00:00", |_| three_hours),
        Some(1_792_065_600_000 - three_hours)
    );
    // Дата без времени и дата с зоной от местного времени не зависят.
    assert_eq!(
        js::date_parse_with("2026-10-15", |_| three_hours),
        Some(1_792_022_400_000)
    );
    assert_eq!(
        js::date_parse_with("Thu, 15 Oct 2026 12:00:00 GMT", |_| three_hours),
        Some(1_792_065_600_000)
    );
}

#[test]
fn encode_uri_component_matches_node() {
    for case in fixture()["encodeURIComponent"].as_array().unwrap() {
        let input = str_of(&case[0]);
        assert_eq!(
            js::encode_uri_component(input),
            str_of(&case[1]),
            "encodeURIComponent({input:?})"
        );
    }
}
#[test]
fn to_fixed_matches_node() {
    for case in fixture()["toFixed"].as_array().unwrap() {
        let input = str_of(&case[0]);
        let digits = case[1].as_u64().unwrap() as usize;
        assert_eq!(
            js::to_fixed(js::to_number(input), digits),
            str_of(&case[2]),
            "({input}).toFixed({digits})"
        );
    }
}

#[test]
fn collator_matches_icu_on_ascii_order_and_pairs() {
    // Язык задан явно: фикстура снята с `localeCompare(…, "en-US")`; язык по умолчанию
    // зависел бы от окружения (`default_locale`).
    let collator = Collator::new("en-US", Sensitivity::Variant);
    let fixture = fixture();
    let mut chars: Vec<String> = (0u8..128)
        .map(|code| char::from(code).to_string())
        .collect();
    chars.sort_by(|a, b| collator.compare(a, b));
    assert_eq!(chars.concat(), str_of(&fixture["localeOrder"]));
    for case in fixture["localeCompare"].as_array().unwrap() {
        let (a, b) = (str_of(&case[0]), str_of(&case[1]));
        let expected = case[2].as_i64().unwrap();
        let got = match collator.compare(a, b) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        };
        assert_eq!(got, expected, "{a:?}.localeCompare({b:?})");
    }
}
