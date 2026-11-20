//! Редактор cfg (5b.5, `GameCfgFiles.vue` 0.4.7): логика контроллера без окна — список,
//! открытие, сохранение, поиск и замена, переход к строке, клавиши, своя отмена с
//! группировкой правок как у `textarea` Chromium (решение 23).

use hlsr_core::language::Language;
use hlsr_ui::controllers::cfg_history::{EditKind, History};
use hlsr_ui::controllers::game_cfg_files::{
    self as cf, CfgFiles, Field, FindFlag, Focus, Key, OpenStep, Request, View, place_keys,
};
use serde_json::{Value, json};

fn view() -> View {
    View {
        client_width: 600.0,
        client_height: 300.0,
        ..View::default()
    }
}

/// Редактор с открытым файлом `text` (`ghostingmod/cfg/autoexec.cfg`).
fn opened(text: &str) -> CfgFiles {
    let mut cfg = CfgFiles::new(Language::En);
    cfg.set_target("ghosting-renewed", Value::Null);
    cfg.finish_load(Ok(json!({
        "status": "ok", "running": false, "truncated": false,
        "files": [{"path": "ghostingmod/cfg/autoexec.cfg"}, {"path": "ghostingmod/cfg/config.cfg"}],
    })));
    let path = "ghostingmod/cfg/autoexec.cfg";
    assert_eq!(cfg.open(path), OpenStep::Read);
    cfg.read(path);
    let effect = cfg.finish_read(Ok(read_reply(path, text)));
    assert!(effect.apply && effect.scroll_home);
    cfg
}

fn read_reply(path: &str, text: &str) -> Value {
    json!({
        "status": "ok", "gameId": "ghosting-renewed", "running": false, "path": path,
        "content": text, "encoding": "utf8", "eol": "lf", "mtime": "2026-10-12T18:40:00.000Z",
    })
}

/// Набрать строку в поле, как поле Slint: клавиша, затем новый текст и каретка.
fn type_text(cfg: &mut CfgFiles, typed: &str) {
    for c in typed.chars() {
        let key = if c == '\n' {
            "Enter".to_owned()
        } else {
            c.to_string()
        };
        let outcome = cfg.key(Field::Text, &Key::new(&key), &view());
        assert!(!outcome.handled, "{key}: поле само вставляет символ");
        let (start, end) = cfg.selection();
        let units: Vec<u16> = cfg.text().encode_utf16().collect();
        let mut text = String::from_utf16_lossy(&units[..start]);
        text.push(c);
        text.push_str(&String::from_utf16_lossy(&units[end..]));
        let caret = start + c.len_utf16();
        cfg.user_edit(&text, (caret, caret));
    }
}

/// Backspace или Delete в поле.
fn delete(cfg: &mut CfgFiles, backward: bool) {
    let key = if backward { "Backspace" } else { "Delete" };
    assert!(!cfg.key(Field::Text, &Key::new(key), &view()).handled);
    let (start, end) = cfg.selection();
    let units: Vec<u16> = cfg.text().encode_utf16().collect();
    let (from, to) = if start != end {
        (start, end)
    } else if backward {
        (start.saturating_sub(1), start)
    } else {
        (start, (start + 1).min(units.len()))
    };
    let mut text = String::from_utf16_lossy(&units[..from]);
    text.push_str(&String::from_utf16_lossy(&units[to..]));
    cfg.user_edit(&text, (from, from));
}

fn undo(cfg: &mut CfgFiles) {
    assert!(cfg.key(Field::Text, &Key::new("z").ctrl(), &view()).handled);
}

fn redo(cfg: &mut CfgFiles) {
    assert!(cfg.key(Field::Text, &Key::new("y").ctrl(), &view()).handled);
}

#[test]
fn file_names_errors_and_line_numbers_follow_the_component() {
    assert_eq!(cf::base_name("ghostingmod/cfg/config.cfg"), "config.cfg");
    assert_eq!(
        cf::dir_name("ghostingmod/cfg/config.cfg"),
        "ghostingmod/cfg"
    );
    assert_eq!(cf::base_name("autoexec.cfg"), "autoexec.cfg");
    assert_eq!(cf::dir_name("autoexec.cfg"), "");
    assert_eq!(cf::dir_name("/x.cfg"), "");
    assert!(cf::is_game_config("Half-Life/valve_WON/CONFIG.cfg"));
    assert!(cf::is_game_config("config.cfg"));
    assert!(!cf::is_game_config("myconfig.cfg"));
    assert_eq!(cf::error_key(Some("TOO_LARGE")), "#L_CF_ERR_TOO_LARGE");
    assert_eq!(cf::error_key(Some("EBUSY")), "#L_CF_ERR_IO");
    assert_eq!(cf::error_key(None), "#L_CF_ERR_IO");
    assert_eq!(cf::line_numbers(1), "1");
    assert_eq!(cf::line_numbers(4), "1\n2\n3\n4");
    assert_eq!(
        cf::escape_regexp("a.b*(c)[d]{e}|f^$\\+?"),
        "a\\.b\\*\\(c\\)\\[d\\]\\{e\\}\\|f\\^\\$\\\\\\+\\?"
    );
    assert_eq!(cf::parse_int(" 12abc"), Some(12));
    assert_eq!(cf::parse_int("1e3"), Some(1));
    assert_eq!(cf::parse_int("-5"), Some(-5));
    assert_eq!(cf::parse_int(""), None);
    assert_eq!(cf::parse_int("x1"), None);
}

#[test]
fn utf16_and_byte_offsets_convert_both_ways() {
    let text = "a€😀b";
    // a — 1 байт / 1 единица, € — 3 / 1, 😀 — 4 / 2.
    assert_eq!(cf::utf16_at(text, 0), 0);
    assert_eq!(cf::utf16_at(text, 1), 1);
    assert_eq!(cf::utf16_at(text, 4), 2);
    assert_eq!(cf::utf16_at(text, 8), 4);
    assert_eq!(cf::utf16_at(text, 9), 5);
    // Середина символа — к его началу.
    assert_eq!(cf::utf16_at(text, 2), 1);
    assert_eq!(cf::byte_at(text, 0), 0);
    assert_eq!(cf::byte_at(text, 2), 4);
    assert_eq!(cf::byte_at(text, 3), 4, "середина пары — начало символа");
    assert_eq!(cf::byte_at(text, 4), 8);
    assert_eq!(cf::byte_at(text, 99), text.len());
}

#[test]
fn set_selection_range_clamps_like_chromium() {
    assert_eq!(cf::selection_range(2, 5, 10), (2, 5));
    assert_eq!(
        cf::selection_range(-1, -1, 10),
        (10, 10),
        "−1 — 4294967295, конец текста"
    );
    assert_eq!(cf::selection_range(-5, 2, 10), (2, 2));
    assert_eq!(
        cf::selection_range(7, 3, 10),
        (3, 3),
        "начало после конца — к концу"
    );
    assert_eq!(cf::selection_range(4, 99, 10), (4, 10));
}

#[test]
fn the_list_loads_filters_and_reports_errors() {
    let mut cfg = CfgFiles::new(Language::En);
    let request = cfg.set_target("218", Value::Null).unwrap();
    assert_eq!(
        request.invoke(),
        ("game-cfg-files-list", vec![json!("218"), Value::Null])
    );
    assert!(cfg.loading);
    assert_eq!(
        cfg.list_state(),
        Some((false, "Looking for cfg files…".to_owned()))
    );
    cfg.finish_load(Ok(json!({
        "status": "ok", "running": true, "truncated": true,
        "files": [{"path": "ghostingmod/cfg/autoexec.cfg"}, {"path": "ghostingmod/cfg/Config.cfg"}],
    })));
    assert!(cfg.truncated && cfg.running && !cfg.loading);
    assert_eq!(cfg.list_state(), None);
    cfg.filter = "  CONFIG ".to_owned();
    assert_eq!(cfg.shown_files(), vec!["ghostingmod/cfg/Config.cfg"]);
    cfg.filter = "zzz".to_owned();
    assert_eq!(cfg.list_state(), Some((false, "Nothing found.".to_owned())));
    // Тот же target — без запроса; другая сборка — reset и список заново.
    assert_eq!(cfg.set_target("218", Value::Null), None);
    assert!(cfg.set_target("218", json!("4260")).is_some());
    cfg.finish_load(Ok(json!({"status": "error", "code": "NOT_INSTALLED"})));
    assert_eq!(
        cfg.list_state(),
        Some((true, "The build is not installed.".to_owned()))
    );
    assert!(cfg.files.is_empty());
    cfg.begin_load();
    cfg.finish_load(Err("boom".to_owned()));
    assert_eq!(
        cfg.list_state(),
        Some((true, "Couldn't read or write the file.".to_owned()))
    );
    cfg.begin_load();
    cfg.filter.clear();
    cfg.finish_load(Ok(json!({"status": "ok", "files": []})));
    assert_eq!(
        cfg.list_state(),
        Some((false, "The build folder has no cfg files.".to_owned()))
    );
}

#[test]
fn opening_asks_before_dropping_unsaved_edits() {
    let mut cfg = opened("sensitivity \"2.5\"\n");
    assert_eq!(cfg.open("ghostingmod/cfg/autoexec.cfg"), OpenStep::Nothing);
    assert_eq!(cfg.open("ghostingmod/cfg/config.cfg"), OpenStep::Read);
    assert_eq!(
        cfg.read("ghostingmod/cfg/config.cfg").invoke(),
        (
            "game-cfg-file-read",
            vec![
                json!("ghosting-renewed"),
                Value::Null,
                json!("ghostingmod/cfg/config.cfg")
            ]
        )
    );
    // Ответ — снова autoexec.cfg: дальше правки в нём.
    cfg.finish_read(Ok(read_reply(
        "ghostingmod/cfg/autoexec.cfg",
        "sensitivity \"2.5\"\n",
    )));
    type_text(&mut cfg, "x");
    assert!(cfg.dirty());
    assert_eq!(cfg.open("ghostingmod/cfg/config.cfg"), OpenStep::Confirm);
    // Ошибка чтения: прежний файл остаётся, в подвале — текст ошибки.
    cfg.read("ghostingmod/cfg/config.cfg");
    assert!(cfg.reading);
    let effect = cfg.finish_read(Ok(json!({"status": "error", "code": "TOO_LARGE"})));
    assert!(!effect.apply && !cfg.reading);
    assert_eq!(
        cfg.foot_note(),
        "The file is too large for the editor (over 512 KB)."
    );
    assert_eq!(cfg.text(), "xsensitivity \"2.5\"\n");
    // Открытый при открытом поиске файл фокус не забирает.
    cfg.find.open = true;
    cfg.read("ghostingmod/cfg/config.cfg");
    let effect = cfg.finish_read(Ok(read_reply("ghostingmod/cfg/config.cfg", "a\nb")));
    assert_eq!(effect.focus, Focus::None);
    assert_eq!(cfg.selection(), (0, 0));
    assert!(!cfg.dirty() && !cfg.history().can_undo());
}

#[test]
fn saving_writes_meta_and_reports_in_the_footer() {
    let mut cfg = opened("fps_max 100\n");
    assert_eq!(cfg.begin_save(), None, "без правок нечего сохранять");
    assert_eq!(
        cfg.foot_note(),
        "No need to close the game: run exec in the console after saving."
    );
    type_text(&mut cfg, "//");
    let request = cfg.begin_save().unwrap();
    assert_eq!(
        request,
        Request::Write {
            game_id: "ghosting-renewed".to_owned(),
            build: Value::Null,
            path: "ghostingmod/cfg/autoexec.cfg".to_owned(),
            content: "//fps_max 100\n".to_owned(),
            meta: json!({"encoding": "utf8", "eol": "lf", "mtime": "2026-10-12T18:40:00.000Z"}),
        }
    );
    assert_eq!(cfg.begin_save(), None, "уже сохраняется");
    type_text(&mut cfg, " ");
    cfg.finish_save(
        Ok(json!({"status": "ok", "mtime": "2026-10-17T10:00:00.000Z", "running": false})),
        "//fps_max 100\n",
    );
    assert_eq!(cfg.foot_note(), "autoexec.cfg saved.");
    assert!(cfg.dirty(), "правка после начала записи не сохранена");
    let request = cfg.begin_save().unwrap();
    let Request::Write { meta, .. } = request else {
        panic!()
    };
    assert_eq!(meta["mtime"], "2026-10-17T10:00:00.000Z");
    cfg.finish_save(
        Ok(json!({"status": "ok", "mtime": "x", "running": true})),
        "// fps_max 100\n",
    );
    assert_eq!(
        cfg.foot_note(),
        "autoexec.cfg saved. Run exec autoexec.cfg in the game console to apply it."
    );
    type_text(&mut cfg, "!");
    cfg.begin_save();
    cfg.finish_save(Ok(json!({"status": "error", "code": "CHANGED"})), "");
    let notice = cfg.notice.clone().unwrap();
    assert!(notice.error);
    assert_eq!(
        notice.text,
        "The file changed on disk after you opened it. Reopen it so you don't overwrite those changes."
    );
    // «Вернуть»: текст файла, подвал без итога, каретка в конце.
    let effect = cfg.revert();
    assert!(effect.apply && !cfg.dirty() && cfg.notice.is_none());
    assert_eq!(cfg.selection(), (15, 15));
}

#[test]
fn a_running_game_warns_about_config_cfg() {
    let mut cfg = CfgFiles::new(Language::Ru);
    cfg.set_target("70", Value::Null);
    cfg.finish_load(Ok(json!({"status": "ok", "running": true, "files": [{"path": "Half-Life/valve_WON/config.cfg"}]})));
    cfg.read("Half-Life/valve_WON/config.cfg");
    let mut reply = read_reply("Half-Life/valve_WON/config.cfg", "bind w +forward\n");
    reply["running"] = json!(true);
    cfg.finish_read(Ok(reply));
    assert!(
        cfg.foot_note()
            .starts_with("Игра запущена и перепишет config.cfg")
    );
}

#[test]
fn find_counts_matches_errors_and_caps() {
    let mut cfg = opened("bind w +forward\nbind s +back\nBIND a +moveleft\n");
    cfg.find.open = true;
    cfg.find.query = "bind".to_owned();
    assert_eq!(cfg.find_count_text(), "1 of 3", "ближайшее после курсора");
    cfg.find.case_sensitive = true;
    assert_eq!(cfg.find_count_text(), "1 of 2");
    cfg.find.case_sensitive = false;
    // Выделено второе совпадение — оно и есть текущее.
    cfg.user_select((16, 20));
    assert_eq!(cfg.find_index(), 1);
    assert_eq!(cfg.find_count_text(), "2 of 3");
    cfg.find.query = "nothing".to_owned();
    assert_eq!(cfg.find_count_text(), "No results");
    cfg.find.regex = true;
    cfg.find.query = "(".to_owned();
    assert_eq!(cfg.find_count_text(), "Invalid expression");
    cfg.find.query.clear();
    assert_eq!(cfg.find_count_text(), "");

    let mut many = opened(&"x".repeat(10_001));
    many.find.open = true;
    many.find.query = "x".to_owned();
    assert_eq!(many.find_count_text(), "1 of ≥10000");
    many.set_language(Language::Ru);
    assert_eq!(many.find_count_text(), "1 из ≥10000");
}

#[test]
fn stepping_wraps_and_reveals_the_match() {
    let text: String = (1..=60).map(|n| format!("line{n}\n")).collect();
    let mut cfg = opened(&text);
    cfg.find.open = true;
    cfg.find.query = "line5".to_owned();
    let v = view();
    let effect = cfg.step(1, &v);
    // «line5» в строке 5: видна, прокрутка та же.
    assert_eq!(cfg.selection(), (24, 29));
    assert_eq!(effect.scroll, Some((0.0, 0.0)));
    let effect = cfg.step(1, &v);
    // «line50» в строке 50: ниже видимого — строка уходит к середине.
    let top = 49.0 * cf::LINE_HEIGHT - 150.0;
    assert_eq!(effect.scroll, Some((top, 0.0)));
    for _ in 0..10 {
        cfg.step(1, &v);
    }
    // 1 + 10 совпадений (line5, line50…line59): по кругу снова первое.
    assert_eq!(cfg.selection(), (24, 29));
    cfg.step(-1, &v);
    assert_eq!(cf::utf16_at(cfg.text(), 0), 0);
    let (start, _) = cfg.selection();
    assert_eq!(&cfg.text()[start..start + 6], "line59");
}

#[test]
fn reveal_scrolls_horizontally_past_a_third_of_the_view() {
    let text = format!("{}target\n", " ".repeat(200));
    let mut cfg = opened(&text);
    let mut v = view();
    v.char_width = 7.5;
    let effect = cfg.reveal(200, 206, &v);
    let left = 200.0 * 7.5 - 600.0 / 3.0;
    assert_eq!(effect.scroll, Some((0.0, left)));
    v.scroll_left = left;
    // Уже видно — не двигается.
    assert_eq!(cfg.reveal(200, 206, &v).scroll, Some((0.0, left)));
}

#[test]
fn opening_find_takes_the_selected_line() {
    let mut cfg = opened("bind \"w\" \"+forward\"\nalias a.b\n");
    cfg.user_select((20, 29));
    let effect = cfg.open_find(false);
    assert_eq!(cfg.find.query, "alias a.b");
    assert_eq!((effect.focus, effect.select), (Focus::Find, true));
    cfg.close_find();
    cfg.find.regex = true;
    cfg.open_find(true);
    assert_eq!(cfg.find.query, "alias a\\.b");
    assert!(cfg.find.replace);
    // Замена с запросом — фокус в «Заменить на»; без запроса — в «Найти».
    let effect = cfg.open_find(true);
    assert_eq!(effect.focus, Focus::Replace);
    cfg.user_select((0, 0));
    cfg.find.query.clear();
    assert_eq!(cfg.open_find(true).focus, Focus::Find);
    // Выделение в несколько строк в запрос не идёт.
    cfg.user_select((0, 30));
    cfg.open_find(false);
    assert_eq!(cfg.find.query, "");
    let effect = cfg.close_find();
    assert!(!cfg.find.open && !cfg.find.replace);
    assert_eq!(effect.focus, Focus::Text);
    cfg.toggle_flag(FindFlag::WholeWord);
    assert!(cfg.find.whole_word);
}

#[test]
fn replace_one_then_all_are_single_undo_steps() {
    let mut cfg = opened("bind w +forward\nbind s +back\n");
    cfg.find.open = true;
    cfg.find.replace = true;
    cfg.find.query = "bind".to_owned();
    cfg.find.replacement = "unbind".to_owned();
    // Совпадение не выделено — «Заменить» только переходит к нему.
    cfg.replace_current(&view());
    assert_eq!(cfg.selection(), (0, 4));
    assert_eq!(cfg.text(), "bind w +forward\nbind s +back\n");
    cfg.replace_current(&view());
    assert_eq!(cfg.text(), "unbind w +forward\nbind s +back\n");
    assert_eq!(cfg.selection(), (18, 22), "дальше — следующее совпадение");
    cfg.find.query = "+".to_owned();
    cfg.find.replacement = "-".to_owned();
    cfg.user_select((3, 3));
    let effect = cfg.replace_everything(&view());
    assert!(effect.apply);
    assert_eq!(cfg.text(), "unbind w -forward\nbind s -back\n");
    assert_eq!(cfg.selection(), (3, 3));
    assert_eq!(cfg.foot_note(), "Replaced: 2. Ctrl+Z undoes it.");
    undo(&mut cfg);
    assert_eq!(cfg.text(), "unbind w +forward\nbind s +back\n");
    undo(&mut cfg);
    assert_eq!(cfg.text(), "bind w +forward\nbind s +back\n");
    assert_eq!(cfg.selection(), (0, 4), "отмена выделяет заменённое");
    redo(&mut cfg);
    assert_eq!(cfg.text(), "unbind w +forward\nbind s +back\n");
    assert_eq!(cfg.selection(), (6, 6), "повтор — каретка после вставки");
}

#[test]
fn go_to_line_clamps_and_parses_like_parse_int() {
    let mut cfg = opened("a\nbb\nccc\n");
    cfg.user_select((3, 3));
    let effect = cfg.open_goto();
    assert_eq!(cfg.goto_line, "2");
    assert_eq!((effect.focus, effect.select), (Focus::Goto, true));
    assert!(!cfg.find.open);
    for (input, at) in [
        ("3", 5),
        ("99", 9),
        ("", 0),
        ("0", 0),
        ("-4", 0),
        ("2.9", 2),
    ] {
        cfg.open_goto();
        cfg.goto_line = input.to_owned();
        let effect = cfg.go_to_line(&view());
        assert_eq!(cfg.selection(), (at, at), "{input:?}");
        assert_eq!(effect.focus, Focus::Text);
        assert!(!cfg.goto_open);
    }
}

#[test]
fn go_to_line_reveals_in_the_field_without_the_goto_bar() {
    // goToLine закрывает строку перехода, а reveal идёт в $nextTick: поле уже выше на её высоту
    // (.cf__find: 7 + 30 + 7 + рамка 1).
    assert_eq!(cf::GOTO_BAR_HEIGHT, 45.0);
    let text: String = (1..=60).map(|n| format!("line{n}\n")).collect();
    let mut cfg = opened(&text);
    cfg.open_goto();
    cfg.goto_line = "45".to_owned();
    let v = view();
    let effect = cfg.go_to_line(&v);
    let top = 44.0 * cf::LINE_HEIGHT - (300.0 + cf::GOTO_BAR_HEIGHT) / 2.0;
    assert_eq!(effect.scroll, Some((top, 0.0)));
    // Без открытой строки перехода (кнопка уже закрыла её) — поле прежней высоты.
    cfg.goto_line = "45".to_owned();
    let mut v = view();
    v.scroll_top = 0.0;
    let effect = cfg.go_to_line(&v);
    assert_eq!(effect.scroll, Some((44.0 * cf::LINE_HEIGHT - 150.0, 0.0)));
}

#[test]
fn editor_keys_follow_on_key_and_on_editor_key() {
    let mut cfg = opened("a\nb\nc");
    let v = view();
    // Ctrl+S — сохранение.
    let outcome = cfg.key(Field::Text, &Key::new("s").ctrl(), &v);
    assert!(outcome.handled && outcome.effect.save);
    // Ctrl+/ — комментарий строки курсора.
    cfg.key(Field::Text, &Key::new("/").ctrl(), &v);
    assert_eq!(cfg.text(), "// a\nb\nc");
    assert_eq!(cfg.selection(), (0, 4));
    // Ctrl+D — копия строк.
    cfg.user_select((5, 5));
    cfg.key(Field::Text, &Key::new("d").ctrl(), &v);
    assert_eq!(cfg.text(), "// a\nb\nb\nc");
    assert_eq!(cfg.selection(), (7, 7));
    // Ctrl+Shift+K — удалить строку.
    cfg.key(Field::Text, &Key::new("K").ctrl().shift(), &v);
    assert_eq!(cfg.text(), "// a\nb\nc");
    // Alt+↑ / Alt+↓.
    cfg.user_select((7, 7));
    cfg.key(Field::Text, &Key::new("ArrowUp").alt(), &v);
    assert_eq!(cfg.text(), "// a\nc\nb");
    let outcome = cfg.key(Field::Text, &Key::new("ArrowUp").alt(), &v);
    assert_eq!(cfg.text(), "c\n// a\nb");
    let outcome2 = cfg.key(Field::Text, &Key::new("ArrowUp").alt(), &v);
    assert!(
        outcome.handled && outcome2.handled,
        "в начале — preventDefault без правки"
    );
    assert_eq!(cfg.text(), "c\n// a\nb");
    // Tab — табуляция, Shift+Tab — снять отступ строки, Tab по нескольким строкам — отступ.
    cfg.user_select((0, 0));
    cfg.key(Field::Text, &Key::new("Tab"), &v);
    assert_eq!(cfg.text(), "\tc\n// a\nb");
    assert_eq!(cfg.selection(), (1, 1));
    cfg.key(Field::Text, &Key::new("Tab").shift(), &v);
    assert_eq!(cfg.text(), "c\n// a\nb");
    cfg.user_select((0, 4));
    cfg.key(Field::Text, &Key::new("Tab"), &v);
    assert_eq!(cfg.text(), "\tc\n\t// a\nb");
    // Обычная буква — полю (не обработана).
    assert!(!cfg.key(Field::Text, &Key::new("q"), &v).handled);
    // Ctrl+/ в русской раскладке: на клавише Slash — «.», код тот же (event.code === "Slash").
    cfg.user_select((0, 0));
    let outcome = cfg.key(Field::Text, &Key::new(".").ctrl().code("Slash"), &v);
    assert!(outcome.handled);
    assert_eq!(cfg.text(), "\t// c\n\t// a\nb");
    assert!(
        !cfg.key(Field::Text, &Key::new(".").ctrl().code("Period"), &v)
            .handled
    );
}

#[test]
fn find_keys_and_escape_order() {
    let mut cfg = opened("bind w +forward\nbind s +back\n");
    let v = view();
    // F3 без поиска открывает поиск, с поиском — шаг.
    let outcome = cfg.key(Field::Text, &Key::new("F3"), &v);
    assert!(outcome.handled && cfg.find.open);
    assert_eq!(outcome.effect.focus, Focus::Find);
    cfg.find.query = "bind".to_owned();
    cfg.key(Field::Find, &Key::new("Enter"), &v);
    assert_eq!(cfg.selection(), (0, 4));
    cfg.key(Field::Find, &Key::new("Enter"), &v);
    assert_eq!(cfg.selection(), (16, 20));
    cfg.key(Field::Find, &Key::new("Enter").shift(), &v);
    assert_eq!(cfg.selection(), (0, 4));
    cfg.key(Field::Text, &Key::new("F3").shift(), &v);
    assert_eq!(cfg.selection(), (16, 20));
    // Ctrl+H — замена; Enter в «Заменить на» — заменить, Ctrl+Alt+Enter — всё.
    cfg.key(Field::Find, &Key::new("h").ctrl(), &v);
    assert!(cfg.find.replace);
    cfg.find.replacement = "BIND".to_owned();
    cfg.key(Field::Replace, &Key::new("Enter"), &v);
    assert_eq!(cfg.text(), "bind w +forward\nBIND s +back\n");
    let mut all = Key::new("Enter").ctrl();
    all.alt = true;
    cfg.key(Field::Replace, &all, &v);
    assert_eq!(cfg.text(), "BIND w +forward\nBIND s +back\n");
    // Esc: памятка, затем переход, затем поиск; без открытого — окну.
    cfg.toggle_keys();
    let outcome = cfg.key(Field::Find, &Key::new("Escape"), &v);
    assert!(outcome.handled && outcome.stop && !cfg.keys_open && cfg.find.open);
    assert_eq!(outcome.effect.focus, Focus::KeysToggle);
    cfg.key(Field::Other, &Key::new("g").ctrl(), &v);
    assert!(cfg.goto_open && !cfg.find.open);
    let outcome = cfg.key(Field::Goto, &Key::new("Escape"), &v);
    assert!(outcome.stop && !cfg.goto_open);
    assert_eq!(outcome.effect.focus, Focus::Text);
    let outcome = cfg.key(Field::Text, &Key::new("Escape"), &v);
    assert!(!outcome.handled && !outcome.stop, "Esc уходит окну");
    // Ctrl+G в тексте и Enter в «Строка».
    cfg.key(Field::Text, &Key::new("g").ctrl(), &v);
    cfg.goto_line = "2".to_owned();
    cfg.key(Field::Goto, &Key::new("Enter"), &v);
    assert_eq!(cfg.selection(), (16, 16));
}

#[test]
fn typing_is_one_undo_step_until_the_caret_moves() {
    let mut cfg = opened("");
    type_text(&mut cfg, "hello\nworld");
    assert_eq!(
        cfg.history().undo_len(),
        1,
        "набор с переводами строк — один шаг"
    );
    // Стрелка закрывает набор: дальше — новый шаг.
    cfg.user_select((5, 5));
    type_text(&mut cfg, "!");
    assert_eq!(cfg.text(), "hello!\nworld");
    assert_eq!(cfg.history().undo_len(), 2);
    undo(&mut cfg);
    assert_eq!(cfg.text(), "hello\nworld");
    assert_eq!(cfg.selection(), (5, 5));
    undo(&mut cfg);
    assert_eq!(cfg.text(), "");
    assert_eq!(cfg.selection(), (0, 0));
    redo(&mut cfg);
    assert_eq!(cfg.text(), "hello\nworld");
    assert_eq!(cfg.selection(), (11, 11));
    // Ctrl+Shift+Z — тоже повтор.
    assert!(
        cfg.key(Field::Text, &Key::new("Z").ctrl().shift(), &view())
            .handled
    );
    assert_eq!(cfg.text(), "hello!\nworld");
}

#[test]
fn deletes_group_by_direction_and_typing_joins_an_open_delete() {
    let mut cfg = opened("");
    type_text(&mut cfg, "abcdef");
    delete(&mut cfg, true);
    delete(&mut cfg, true);
    assert_eq!(cfg.text(), "abcd");
    // Backspace после набора — новый шаг, Backspace подряд — в нём же.
    assert_eq!(cfg.history().undo_len(), 2);
    // Набор после удаления дописывается в открытую команду удаления.
    type_text(&mut cfg, "XY");
    assert_eq!(cfg.history().undo_len(), 2);
    // Delete после набора — снова новый шаг.
    cfg.user_select((0, 0));
    delete(&mut cfg, false);
    delete(&mut cfg, false);
    assert_eq!(cfg.text(), "cdXY");
    assert_eq!(cfg.history().undo_len(), 3);
    undo(&mut cfg);
    assert_eq!(cfg.text(), "abcdXY");
    undo(&mut cfg);
    assert_eq!(cfg.text(), "abcdef");
    assert_eq!(cfg.selection(), (6, 6));
    undo(&mut cfg);
    assert_eq!(cfg.text(), "");
    assert!(!cfg.history().can_undo());
}

#[test]
fn paste_cut_and_program_edits_are_steps_of_their_own() {
    let mut cfg = opened("");
    type_text(&mut cfg, "ab");
    // Ctrl+V — поле вставляет само.
    assert!(!cfg.key(Field::Text, &Key::new("v").ctrl(), &view()).handled);
    cfg.user_edit("abPASTE", (7, 7));
    type_text(&mut cfg, "c");
    assert_eq!(cfg.history().undo_len(), 3);
    // Ctrl+/ — правка программы, затем набор — новый шаг.
    cfg.key(Field::Text, &Key::new("/").ctrl(), &view());
    assert_eq!(cfg.text(), "// abPASTEc");
    assert_eq!(cfg.selection(), (0, 11));
    cfg.user_select((11, 11));
    type_text(&mut cfg, "d");
    assert_eq!(cfg.text(), "// abPASTEcd");
    assert_eq!(cfg.history().undo_len(), 5);
    // Ctrl+X.
    cfg.user_select((0, 3));
    assert!(!cfg.key(Field::Text, &Key::new("x").ctrl(), &view()).handled);
    cfg.user_edit("abPASTEcd", (0, 0));
    assert_eq!(cfg.history().undo_len(), 6);
    undo(&mut cfg);
    assert_eq!(cfg.text(), "// abPASTEcd");
    assert_eq!(cfg.selection(), (0, 3));
    // Новая правка стирает повтор.
    type_text(&mut cfg, "z");
    assert!(!cfg.history().can_redo());
}

#[test]
fn history_caps_at_a_thousand_steps() {
    let mut history = History::new();
    let mut text = String::new();
    for n in 0..1005 {
        let next = format!("{text}{}", n % 10);
        history.record(EditKind::Command, &text, &next, (0, 0), (0, 0));
        text = next;
    }
    assert_eq!(history.undo_len(), 1000);
}

#[test]
fn a_delete_edit_starting_after_its_end_deletes_backward() {
    // move/delete_lines 0.4.7 при каретке в 0 у текста с пустой первой строкой дают start > end;
    // setSelectionRange ставит пустое выделение, execCommand("delete") стирает символ перед ним.
    let mut cfg = opened("\nab");
    cfg.user_select((2, 2));
    let edit = hlsr_delivery::cfg_text_ops::Edit {
        start: 3,
        end: 1,
        insert: String::new(),
        sel_start: -1,
        sel_end: -1,
    };
    cfg.apply_edit(&edit);
    assert_eq!(cfg.text(), "ab");
    assert_eq!(cfg.selection(), (2, 2), "−1 — конец текста");
}

#[test]
fn the_shortcut_sheet_sits_below_or_above_its_button() {
    // Места снизу хватает.
    let place = place_keys((1500.0, 100.0, 30.0, 30.0), (1600.0, 900.0), 390.0);
    assert!(place.below);
    assert_eq!(
        (place.top, place.right, place.max_height),
        (136.0, 70.0, 752.0)
    );
    // Снизу мало, сверху больше — над кнопкой.
    let place = place_keys((1500.0, 700.0, 30.0, 30.0), (1600.0, 900.0), 390.0);
    assert!(!place.below);
    assert_eq!((place.bottom, place.max_height), (206.0, 682.0));
    // Кнопка у правого края — отступ 12.
    let place = place_keys((1590.0, 100.0, 30.0, 30.0), (1600.0, 900.0), 100.0);
    assert_eq!(place.right, 12.0);
}

#[test]
fn shortcuts_and_flags_match_the_component() {
    assert_eq!(cf::SHORTCUTS.len(), 13);
    assert_eq!(cf::SHORTCUTS[8].keys, &["Ctrl", "Shift", "K"]);
    assert_eq!(cf::SHORTCUTS[9].keys, &["Alt", "↑", "↓"]);
    let texts: Vec<&str> = FindFlag::ALL.iter().map(|f| f.text()).collect();
    assert_eq!(texts, ["Aa", "W", ".*"]);
}

// 0.4.9: основной файл игрока — первым в списке и с меткой (`player` ответа списка); `openAt`
// ставит курсор на строку проверки.
#[test]
fn the_player_file_goes_first_and_open_at_finds_the_line() {
    let mut cfg = CfgFiles::new(Language::En);
    cfg.set_target("hl1-won", Value::Null);
    cfg.finish_load(Ok(json!({
        "status": "ok", "player": "valve/userconfig.cfg",
        "files": [{"path": "valve/autoexec.cfg"}, {"path": "valve/config.cfg"}, {"path": "valve/UserConfig.cfg"}],
    })));
    assert!(cfg.is_player_file("valve/userconfig.cfg"));
    assert_eq!(
        cfg.shown_files(),
        [
            "valve/UserConfig.cfg",
            "valve/autoexec.cfg",
            "valve/config.cfg"
        ]
    );
    cfg.filter = "auto".to_owned();
    assert_eq!(cfg.shown_files(), ["valve/autoexec.cfg"]);
    let cfg = opened("bind w +forward\nbind я +jump\n");
    assert_eq!(cfg.line_span(2), Some((16, 28)));
    assert_eq!(cfg.line_span(3), Some((29, 29)));
    assert_eq!(cfg.line_span(4), None);
    assert_eq!(cfg.line_span(0), None);
}
