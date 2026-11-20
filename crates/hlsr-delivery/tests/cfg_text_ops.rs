//! Операции редактора cfg против 0.4.7: фикстура `cfg-text-ops.json` из
//! `tools/reference/delivery/cfg-text-ops.js` и самотест `scripts/cfg-text-ops-selftest.js`
//! с теми же случаями.

mod support;

use hlsr_delivery::cfg_text_ops::{
    self, Edit, FindAll, Match, Options, PatternError, build_pattern, find_all,
};
use serde_json::{Value, json};
use std::sync::LazyLock;
use support::{Mismatches, fixture, items, opt_str, str_of};

static FIXTURE: LazyLock<Value> = LazyLock::new(|| fixture("cfg-text-ops.json"));

/// Операция над строками: текст, начало и конец выделения → правка.
type EditOp = Box<dyn Fn(&str, usize, usize) -> Option<Edit>>;

fn expand(text: &Value) -> String {
    match text {
        Value::String(text) => text.clone(),
        spec => {
            let count = spec["repeat"][1].as_u64().unwrap() as usize;
            str_of(&spec["repeat"][0]).repeat(count)
        }
    }
}

fn text(index: usize) -> String {
    expand(&FIXTURE["texts"][index])
}

fn options_of(bits: u64) -> Options {
    Options {
        regex: bits & 4 != 0,
        whole_word: bits & 2 != 0,
        case_sensitive: bits & 1 != 0,
    }
}

fn flat(matches: &[Match]) -> Value {
    json!(
        matches
            .iter()
            .flat_map(|m| [m.start, m.end])
            .collect::<Vec<_>>()
    )
}

fn matches_of(flat: &Value) -> Vec<Match> {
    let numbers: Vec<usize> = items(flat)
        .iter()
        .map(|n| n.as_u64().unwrap() as usize)
        .collect();
    numbers
        .chunks(2)
        .map(|pair| Match {
            start: pair[0],
            end: pair[1],
        })
        .collect()
}

fn edit_value(edit: &Edit) -> Value {
    json!([
        edit.start,
        edit.end,
        edit.insert,
        edit.sel_start,
        edit.sel_end
    ])
}

fn edit_object(edit: &Edit) -> Value {
    json!({"start": edit.start, "end": edit.end, "insert": edit.insert, "selStart": edit.sel_start, "selEnd": edit.sel_end})
}

fn opt_edit(edit: Option<Edit>) -> Value {
    edit.as_ref().map_or(Value::Null, edit_value)
}

/// Применить правку так же, как окно: срез по единицам UTF-16.
fn apply(text: &str, edit: &Edit) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut out = String::from_utf16_lossy(&units[..edit.start]);
    out.push_str(&edit.insert);
    out.push_str(&String::from_utf16_lossy(&units[edit.end..]));
    out
}

// -------------------------------------------------------------- фикстура

#[test]
fn pattern_building_matches() {
    let mut mismatches = Mismatches::default();
    assert_eq!(
        cfg_text_ops::MAX_MATCHES as u64,
        FIXTURE["maxMatches"].as_u64().unwrap()
    );
    for case in items(&FIXTURE["build"]["cases"]) {
        let options = options_of(case["options"].as_u64().unwrap());
        let query = str_of(&case["query"]);
        let (error, flags) = match build_pattern(query, &options) {
            Ok(pattern) => (Value::Null, json!(pattern.flags())),
            Err(PatternError::Empty) => (json!("EMPTY"), Value::Null),
            Err(PatternError::BadRegex(_)) => (json!("BAD_REGEX"), Value::Null),
        };
        let context = format!("buildPattern({query:?}, {})", case["options"]);
        mismatches.check(&error, &case["error"], &context);
        mismatches.check(&flags, &case["flags"], &context);
    }
    mismatches.finish("buildPattern");
}

fn find_value(found: &FindAll) -> Value {
    json!({
        "matches": flat(&found.matches),
        "error": if found.bad_regex { json!("BAD_REGEX") } else { Value::Null },
    })
}

fn check_find(section: &str) {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE[section]) {
        let source = text(case["text"].as_u64().unwrap() as usize);
        let options = options_of(case["options"].as_u64().unwrap());
        let found = find_all(&source, str_of(&case["query"]), &options);
        let expected = json!({"matches": case["matches"], "error": case["error"]});
        let context = format!(
            "findAll({:?}, {}, {})",
            case["text"], case["query"], case["options"]
        );
        mismatches.check(&find_value(&found), &expected, &context);
        // capped есть у 0.4.7 только когда выражение разобрано.
        if !case["capped"].is_null() {
            mismatches.check(
                &json!(found.capped),
                &case["capped"],
                format!("{context}: capped"),
            );
        }
    }
    mismatches.finish(section);
}

#[test]
fn plain_search_matches() {
    check_find("plain");
}

#[test]
fn regex_search_matches() {
    check_find("regex");
}

#[test]
fn random_regex_syntax_matches() {
    check_find("fuzz");
}

#[test]
fn match_cap_matches() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["capped"]) {
        let source =
            expand(&json!({"repeat": case["repeat"]})) + opt_str(&case["tail"]).unwrap_or_default();
        let found = find_all(&source, str_of(&case["query"]), &Options::default());
        let context = format!("findAll(a × {})", case["repeat"][1]);
        mismatches.check(&json!(found.matches.len()), &case["count"], &context);
        mismatches.check(&flat(&found.matches[..2]), &case["first"], &context);
        mismatches.check(
            &flat(&found.matches[found.matches.len() - 1..]),
            &case["last"],
            &context,
        );
        mismatches.check(&json!(found.capped), &case["capped"], &context);
    }
    mismatches.finish("предел совпадений");
}

#[test]
fn nearest_match_matches() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["nearest"]) {
        let matches = matches_of(&case[0]);
        let from = case[1].as_u64().unwrap() as usize;
        let backward = case[2].as_bool().unwrap_or(false);
        let actual =
            cfg_text_ops::nearest_match(&matches, from, backward).map_or(-1, |at| at as i64);
        mismatches.check(
            &json!(actual),
            &case[3],
            format!("nearestMatch({}, {from}, {backward})", case[0]),
        );
    }
    mismatches.finish("nearestMatch");
}

#[test]
fn replacements_match() {
    let mut mismatches = Mismatches::default();
    for case in items(&FIXTURE["replace"]["all"]) {
        let source = text(case["text"].as_u64().unwrap() as usize);
        let options = options_of(case["options"].as_u64().unwrap());
        let result = cfg_text_ops::replace_all(
            &source,
            str_of(&case["query"]),
            str_of(&case["replacement"]),
            &options,
        );
        let actual = json!({
            "count": result.count,
            "error": if result.bad_regex { json!("BAD_REGEX") } else { Value::Null },
            "edit": result.edit.as_ref().map_or(Value::Null, edit_object),
        });
        let expected =
            json!({"count": case["count"], "error": case["error"], "edit": case["edit"]});
        mismatches.check(
            &actual,
            &expected,
            format!(
                "replaceAll({}, {}, {})",
                case["text"], case["query"], case["replacement"]
            ),
        );
    }
    for case in items(&FIXTURE["replace"]["one"]) {
        let source = text(case["text"].as_u64().unwrap() as usize);
        let options = options_of(case["options"].as_u64().unwrap());
        let query = str_of(&case["query"]);
        let found = find_all(&source, query, &options);
        let chosen = found.matches[case["match"].as_u64().unwrap() as usize];
        let edit = cfg_text_ops::replace_one(
            &source,
            chosen,
            query,
            str_of(&case["replacement"]),
            &options,
        );
        mismatches.check(
            &edit_object(&edit),
            &case["edit"],
            format!(
                "replaceOne({}, {query}, {}, совпадение {})",
                case["text"], case["replacement"], case["match"]
            ),
        );
    }
    mismatches.finish("замена");
}

#[test]
fn line_operations_match() {
    let lines = &FIXTURE["lines"];
    let texts: Vec<String> = items(&lines["texts"])
        .iter()
        .map(|t| str_of(t).to_owned())
        .collect();
    let mut mismatches = Mismatches::default();
    let selection = |case: &Value| {
        (
            case[0].as_u64().unwrap() as usize,
            case[1].as_u64().unwrap() as usize,
            case[2].as_u64().unwrap() as usize,
        )
    };
    for case in items(&lines["spans"]) {
        let (index, start, end) = selection(case);
        let span = cfg_text_ops::line_span(&texts[index], start, end);
        mismatches.check(
            &json!([span.start, span.end]),
            &case[3],
            format!("lineSpan({index}, {start}, {end})"),
        );
    }
    let edits: [(&str, EditOp); 7] = [
        (
            "toggle",
            Box::new(|t, s, e| Some(cfg_text_ops::toggle_comment(t, s, e))),
        ),
        (
            "duplicate",
            Box::new(|t, s, e| Some(cfg_text_ops::duplicate_lines(t, s, e))),
        ),
        (
            "delete",
            Box::new(|t, s, e| Some(cfg_text_ops::delete_lines(t, s, e))),
        ),
        (
            "moveUp",
            Box::new(|t, s, e| cfg_text_ops::move_lines(t, s, e, -1)),
        ),
        (
            "moveDown",
            Box::new(|t, s, e| cfg_text_ops::move_lines(t, s, e, 1)),
        ),
        (
            "indent",
            Box::new(|t, s, e| Some(cfg_text_ops::indent_lines(t, s, e, false))),
        ),
        (
            "outdent",
            Box::new(|t, s, e| Some(cfg_text_ops::indent_lines(t, s, e, true))),
        ),
    ];
    for (section, operation) in &edits {
        for case in items(&lines[section]) {
            let (index, start, end) = selection(case);
            let actual = opt_edit(operation(&texts[index], start, end));
            mismatches.check(
                &actual,
                &case[3],
                format!("{section}({index}, {start}, {end})"),
            );
        }
    }
    for case in items(&lines["lineCol"]) {
        let index = case[0].as_u64().unwrap() as usize;
        let pos = case[1].as_u64().unwrap() as usize;
        let got = cfg_text_ops::line_col(&texts[index], pos);
        mismatches.check(
            &json!([got.line, got.col]),
            &json!([case[2], case[3]]),
            format!("lineCol({index}, {pos})"),
        );
    }
    for case in items(&lines["lineStart"]) {
        let index = case[0].as_u64().unwrap() as usize;
        let line = case[1].as_u64().unwrap() as usize;
        mismatches.check(
            &json!(cfg_text_ops::line_start(&texts[index], line)),
            &case[2],
            format!("lineStart({index}, {line})"),
        );
    }
    for case in items(&lines["lineCount"]) {
        let index = case[0].as_u64().unwrap() as usize;
        mismatches.check(
            &json!(cfg_text_ops::line_count(&texts[index])),
            &case[1],
            format!("lineCount({index})"),
        );
    }
    mismatches.finish("операции со строками");
}

// ------------------------------------------------------------------ самотест

#[test]
fn search_case_whole_word_regex_bad_expression() {
    let text = "bind w +forward\nBIND s +back\nunbind w";
    let count = |query: &str, options: Options| find_all(text, query, &options).matches.len();
    assert_eq!(count("bind", Options::default()), 3);
    assert_eq!(
        count(
            "bind",
            Options {
                case_sensitive: true,
                ..Options::default()
            }
        ),
        2
    );
    assert_eq!(
        count(
            "bind",
            Options {
                whole_word: true,
                ..Options::default()
            }
        ),
        2
    );
    let regex = Options {
        regex: true,
        ..Options::default()
    };
    let found = find_all(text, "\\+\\w+", &regex);
    let units: Vec<u16> = text.encode_utf16().collect();
    let words: Vec<String> = found
        .matches
        .iter()
        .map(|m| String::from_utf16_lossy(&units[m.start..m.end]))
        .collect();
    assert_eq!(words, ["+forward", "+back"]);
    assert_eq!(
        count("a.b", Options::default()),
        0,
        "без режима регулярки точка — точка"
    );
    assert!(find_all(text, "(", &regex).bad_regex);
    assert_eq!(count("x*", regex), 0, "пустые совпадения пропускаются");
    let empty = find_all(text, "", &Options::default());
    assert!(!empty.bad_regex, "пустой запрос — не ошибка");
    assert!(empty.matches.is_empty());
}

#[test]
fn nearest_match_forward_and_backward_in_a_circle() {
    let matches = [Match { start: 2, end: 4 }, Match { start: 10, end: 12 }];
    assert_eq!(cfg_text_ops::nearest_match(&matches, 0, false), Some(0));
    assert_eq!(cfg_text_ops::nearest_match(&matches, 5, false), Some(1));
    assert_eq!(cfg_text_ops::nearest_match(&matches, 13, false), Some(0));
    assert_eq!(cfg_text_ops::nearest_match(&matches, 10, true), Some(0));
    assert_eq!(cfg_text_ops::nearest_match(&matches, 1, true), Some(1));
}

#[test]
fn replace_one_and_all_with_groups_in_a_regex() {
    let text = "sensitivity 2\nm_yaw 0.022\nsensitivity 3";
    let regex = Options {
        regex: true,
        ..Options::default()
    };
    let all = cfg_text_ops::replace_all(text, "sensitivity (\\d)", "sensitivity $1.5", &regex);
    assert_eq!(all.count, 2);
    assert_eq!(
        apply(text, all.edit.as_ref().unwrap()),
        "sensitivity 2.5\nm_yaw 0.022\nsensitivity 3.5"
    );
    let literal = cfg_text_ops::replace_all(text, "$1", "x", &Options::default());
    assert_eq!(literal.edit, None);
    let found = find_all(text, "sensitivity", &Options::default());
    let one = cfg_text_ops::replace_one(
        text,
        found.matches[1],
        "sensitivity",
        "sens",
        &Options::default(),
    );
    assert_eq!(apply(text, &one), "sensitivity 2\nm_yaw 0.022\nsens 3");
    assert_eq!(
        cfg_text_ops::replace_all(text, "$", "€", &Options::default()).count,
        0,
        "доллар без регулярки ищется буквально"
    );
}

#[test]
fn comment_goes_by_the_smallest_indent_and_is_removed() {
    let text = "a\n  b\n\nc";
    let on = cfg_text_ops::toggle_comment(text, 0, text.len());
    let commented = apply(text, &on);
    assert_eq!(commented, "// a\n//   b\n\n// c");
    let off = cfg_text_ops::toggle_comment(&commented, 0, commented.len());
    assert_eq!(apply(&commented, &off), text);
    let indented = "  x\n    y";
    assert_eq!(
        apply(indented, &cfg_text_ops::toggle_comment(indented, 0, 1)),
        "  // x\n    y"
    );
}

#[test]
fn lines_duplicate_delete_move_indent() {
    let text = "one\ntwo\nthree";
    assert_eq!(
        apply(text, &cfg_text_ops::duplicate_lines(text, 5, 5)),
        "one\ntwo\ntwo\nthree"
    );
    assert_eq!(
        apply(text, &cfg_text_ops::delete_lines(text, 5, 5)),
        "one\nthree"
    );
    assert_eq!(
        apply(text, &cfg_text_ops::delete_lines(text, 10, 10)),
        "one\ntwo"
    );
    assert_eq!(
        apply(text, &cfg_text_ops::move_lines(text, 5, 5, -1).unwrap()),
        "two\none\nthree"
    );
    assert_eq!(
        apply(text, &cfg_text_ops::move_lines(text, 5, 5, 1).unwrap()),
        "one\nthree\ntwo"
    );
    assert_eq!(cfg_text_ops::move_lines(text, 0, 0, -1), None);
    assert_eq!(cfg_text_ops::move_lines(text, 10, 10, 1), None);
    // выделение до начала следующей строки её не захватывает
    assert_eq!(
        apply(text, &cfg_text_ops::indent_lines(text, 0, 8, false)),
        "\tone\n\ttwo\nthree"
    );
    let mixed = "\tone\n  two";
    assert_eq!(
        apply(mixed, &cfg_text_ops::indent_lines(mixed, 0, 9, true)),
        "one\ntwo"
    );
}

#[test]
fn line_and_column_start_of_line() {
    let text = "ab\ncde\nf";
    let at = cfg_text_ops::line_col(text, 4);
    assert_eq!((at.line, at.col), (2, 2));
    assert_eq!(cfg_text_ops::line_start(text, 3), 7);
    assert_eq!(cfg_text_ops::line_start(text, 9), text.len());
    assert_eq!(cfg_text_ops::line_count(text), 3);
}

/// Как у 0.4.7: при курсоре в позиции 0 текста с пустой первой строкой `lineSpan`
/// даёт `start > end`, а `moveLines` вверх возвращает выделение `−1`.
#[test]
fn caret_at_zero_before_an_empty_first_line_is_reproduced_as_is() {
    let span = cfg_text_ops::line_span("\n\nabc", 0, 0);
    assert_eq!((span.start, span.end), (1, 0));
    let edit = cfg_text_ops::move_lines("\n\nabc", 0, 0, -1).expect("правка есть");
    assert_eq!(
        edit,
        Edit {
            start: 1,
            end: 0,
            insert: "\n".to_owned(),
            sel_start: -1,
            sel_end: -1
        }
    );
}

// ----------------------------------------------- отступление от 0.4.7: зависание

/// В 0.4.7 `findAll("a😀b", "x*", { regex: true })` не возвращается: после пустого
/// совпадения перед суррогатной парой `lastIndex += 1` попадает в середину пары, V8
/// возвращается к её началу и находит то же пустое совпадение. Rust шагает на целый
/// символ и находит остальные совпадения.
#[test]
fn empty_match_before_an_astral_character_does_not_hang() {
    let regex = Options {
        regex: true,
        ..Options::default()
    };
    assert!(find_all("a😀b", "x*", &regex).matches.is_empty());
    // 😀 занимает единицы 0–1, x — 2, 😀 — 3–4.
    assert_eq!(
        find_all("😀x😀", "x*", &regex).matches,
        [Match { start: 2, end: 3 }]
    );
    let all = cfg_text_ops::replace_all("😀x😀", "x*", "-", &regex);
    assert_eq!(apply("😀x😀", all.edit.as_ref().unwrap()), "😀-😀");
}
