//! Настройки игры страницы «Персонализация» (`GameSettingsModal.vue` embedded 0.4.9): сессия на `FixtureServices` по сценам каталога —
//! журнал вызовов при открытии против `ipc-log.json` Electron, порядок сохранения, ошибки
//! параметров, запись удаляется с подтверждением, настройки из cfg по цепочке `exec`
//! (`launcher-ui-selftest.js`, `hl1-selftest.js`: тесты окна и импорта).

#[path = "game_settings/import.rs"]
mod import;

use hlsr_core::language::Language;
use hlsr_ui::controllers::game_settings::{ConfigMode, Env, Event, Session, View, Zone};
use hlsr_ui::env::ManualEnv;
use hlsr_ui::fixtures::{self, FixtureServices, Scene, Variant};
use hlsr_ui::services::Services;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

struct Rig {
    session: Rc<Session>,
    fixture: Arc<FixtureServices>,
    events: Rc<RefCell<Vec<Event>>>,
    renders: Rc<RefCell<usize>>,
}

fn scene(id: &str) -> Scene {
    Scene::load(&fixtures::screens_dir(), id).unwrap()
}

fn rig_with(scene: &Scene, game: &str, title: &str, linux: bool, lang: Language) -> Rig {
    let variant = Variant {
        theme: "hlsr".into(),
        lang: if lang == Language::Ru { "ru" } else { "en" }.into(),
        scale: 1.0,
        zoom: None,
        rail: None,
    };
    let fixture = Arc::new(FixtureServices::new(scene, &variant).unwrap());
    let services: Services = fixture.clone();
    let ui = ManualEnv::new();
    let session = Session::new(
        services,
        Env {
            game_id: game.to_owned(),
            game_title: title.to_owned(),
            initial_section: String::new(),
            linux,
            language: lang,
            translator: Rc::new(hlsr_ui::tr),
            zone: Zone::Fixed(chrono::FixedOffset::east_opt(0).unwrap()),
        },
        ui,
    );
    let events: Rc<RefCell<Vec<Event>>> = Rc::default();
    let sink = events.clone();
    session.set_sink(move |event| sink.borrow_mut().push(event));
    let renders: Rc<RefCell<usize>> = Rc::default();
    let counter = renders.clone();
    session.set_render(move || *counter.borrow_mut() += 1);
    Rig {
        session,
        fixture,
        events,
        renders,
    }
}

fn rig(scene: &Scene) -> Rig {
    rig_with(scene, "218", "Ghosting Renewed", false, Language::En)
}

impl Rig {
    fn mount(&self) -> View {
        self.session.mounted();
        self.session.view()
    }

    fn channels(&self) -> Vec<String> {
        self.fixture
            .log()
            .iter()
            .filter(|entry| entry["kind"] == "invoke")
            .map(|entry| entry["channel"].as_str().unwrap().to_owned())
            .collect()
    }

    fn calls_to(&self, channel: &str) -> Vec<Value> {
        self.fixture
            .log()
            .iter()
            .filter(|entry| entry["kind"] == "invoke" && entry["channel"] == channel)
            .map(|entry| entry["args"].clone())
            .collect()
    }
}

/// Сцена с добавленными или заменёнными ответами каналов.
fn with_invoke(id: &str, replies: Value) -> Scene {
    let mut scene = scene(id);
    for (channel, reply) in replies.as_object().unwrap() {
        scene.invoke.insert(channel.clone(), reply.clone());
    }
    scene
}

#[test]
fn opening_the_window_makes_the_same_calls_as_electron() {
    for id in ["game-settings-launch", "game-settings-configs"] {
        // Сцены 0.4.7 не знают канала проверки 0.4.9.
        let scene = with_invoke(id, json!({"player-config-check": {"status": "nothing"}}));
        let rig = rig(&scene);
        rig.mount();
        // Журнал сцены начинается вызовами оболочки; окно — с `game-settings-get`.
        let expected = &scene.expect.ipc_log["en"];
        let expected: Vec<Value> = expected
            .as_array()
            .unwrap()
            .iter()
            .skip_while(|entry| entry["channel"] != "game-settings-get")
            .cloned()
            .collect();
        assert!(expected.len() >= 5, "{id}");
        // 0.4.9 добавляет к журналу 0.4.7 проверку конфига HL1 (`loadRuleCheck`) после чтения
        // конфига; остальное — как в 0.4.7.
        let actual: Vec<Value> = rig.fixture.log();
        let check = |entry: &Value| entry["channel"] == "player-config-check";
        let read = actual
            .iter()
            .position(|entry| entry["channel"] == "game-config-read")
            .unwrap();
        let first_check = actual.iter().position(check).unwrap();
        assert!(first_check > read, "{id}");
        assert_eq!(actual[first_check]["args"], json!(["218", "scriptless"]));
        let without: Vec<Value> = actual.iter().filter(|e| !check(e)).cloned().collect();
        assert_eq!(without, expected, "{id}");
        assert!(
            rig.fixture.problems().is_empty(),
            "{:?}",
            rig.fixture.problems()
        );
    }
}

#[test]
fn ghosting_opens_on_its_look_and_lists_its_sections() {
    let rig = rig(&scene("game-settings-launch"));
    let view = rig.mount();
    let ids: Vec<&str> = view.tabs.iter().map(|tab| tab.id).collect();
    // 0.4.9: первым — внешний вид игры, «Клавиатура» — обычный раздел, «Демо» есть и у Ghosting.
    assert_eq!(
        ids,
        [
            "personal", "configs", "keyboard", "launch", "demos", "ghosts", "version"
        ]
    );
    assert_eq!(view.active_id, "personal");
    assert!(view.footer.hidden, "у «Персонализации» свои кнопки");
    rig.session.select_section("configs");
    let view = rig.session.view();
    assert_eq!(view.active_id, "configs");
    assert_eq!(view.config_mode, ConfigMode::Editor);
    let editor = view.editor.as_ref().unwrap();
    // 0.4.9: «Файлы cfg», затем «Игра» со всеми разделами каталога Source; показана «Игра».
    assert_eq!(editor.tabs[0].title, "cfg files");
    assert_eq!(editor.active, "cfg-game");
    assert!(!editor.sections.is_empty());
    assert_eq!(
        view.path_text,
        "C:\\HLSR\\ghosting-renewed\\ghostingmod\\cfg"
    );
    assert!(!view.build_select, "у Ghosting одна сборка");
    assert_eq!(view.runs.len(), 2);
    assert_eq!(view.runs_summary, "Runs: 2");
    // Состояние показано один раз за круг, а не на каждый вызов.
    assert!(*rig.renders.borrow() >= 1);
}

#[test]
fn the_launch_tab_of_each_game_lists_its_own_options() {
    let launch = |game: &str, title: &str, linux: bool| {
        let rig = rig_with(
            &scene("game-settings-launch"),
            game,
            title,
            linux,
            Language::En,
        );
        rig.mount();
        rig.session.select_section("launch");
        rig.session.view()
    };
    let ghosting = launch("218", "Ghosting Renewed", false);
    assert_eq!(ghosting.active_id, "launch");
    assert_eq!(ghosting.toggles.len(), 8);
    assert_eq!(ghosting.toggles[0].label, "Open developer console");
    assert_eq!(ghosting.toggles[0].hint, "-console");
    assert!(!ghosting.goldsrc && !ghosting.language_available && !ghosting.linux);
    assert_eq!(ghosting.args_placeholder, "+fps_max 300 -high");

    let hl1 = launch("70", "Half-Life", false);
    assert!(hl1.goldsrc);
    // RInput включён: виден патч курсора; сопровождающая подпись — ключ перевода.
    assert_eq!(hl1.toggles[0].label, "Raw Input (RInput)");
    assert_eq!(hl1.toggles[1].label, "Menu cursor patch (RInput)");
    assert_eq!(
        hl1.toggles[0].hint,
        "Raw mouse input without Windows acceleration. Menu clicks become unreliable with it; the console always works"
    );
    assert_eq!(hl1.resolution.label, "Game setting");
    assert_eq!(hl1.refresh.label, "Game setting");
    assert_eq!(hl1.args_placeholder, "-heapsize 524288 +fps_max 100");

    let linux = launch("218", "Ghosting Renewed", true);
    assert!(linux.linux);
}

#[test]
fn toggles_and_selects_edit_the_draft_and_the_payload_goes_to_main_as_a_plain_copy() {
    let scene = with_invoke(
        "game-settings-launch",
        json!({"game-settings-save": {"status": "ok", "settings": {"console": true}}}),
    );
    let rig = rig(&scene);
    rig.mount();
    rig.session.select_section("launch");
    rig.session.set_flag("console", true);
    rig.session.set_flag("excludeCpu0", true);
    rig.session.set_resolution("1280x720");
    rig.session.set_args("+fps_max 300");
    let view = rig.session.view();
    assert!(view.toggles[0].checked && view.cpu0);
    assert_eq!(view.args, "+fps_max 300");
    rig.session.submit();
    // Нет правок конфигов — сразу game-settings-save; payload — ключи EMPTY.
    let save = rig.calls_to("game-settings-save");
    assert_eq!(save.len(), 1);
    assert_eq!(save[0][0], "218");
    let payload = &save[0][1];
    assert_eq!(payload["console"], true);
    assert_eq!(payload["excludeCpu0"], true);
    assert_eq!(payload["width"], 1280);
    assert_eq!(payload["height"], 720);
    assert_eq!(payload["launchArgs"], "+fps_max 300");
    assert!(payload.get("liveSplitOverlay").is_none());
    // Страница не закрывается: черновик — из сохранённых настроек, правок больше нет.
    assert_eq!(
        *rig.events.borrow(),
        [Event::Saved(json!({"console": true}))]
    );
    let view = rig.session.view();
    assert!(!view.saving);
    assert!(!rig.session.has_unsaved());
    assert!(!view.footer.save_enabled);
}

// launcher-ui-selftest.js: «game settings reach main as a plain copy»: сперва конфиги, потом
// настройки, оба вызова с обычными значениями.
#[test]
fn config_changes_are_written_before_the_settings() {
    let scene = with_invoke(
        "game-settings-configs",
        json!({
            "game-config-write": {"status": "ok"},
            "game-settings-save": {"status": "ok", "settings": {}},
        }),
    );
    let rig = rig(&scene);
    rig.mount();
    rig.session.config_set_number("sensitivity", "2.5");
    assert_eq!(rig.session.edits()["sensitivity"], json!(2.5));
    let view = rig.session.view();
    let editor = view.editor.as_ref().unwrap();
    let game = editor
        .tabs
        .iter()
        .find(|tab| tab.id == editor.active)
        .unwrap();
    assert_eq!(game.edits, 1);
    let before = rig.channels().len();
    rig.session.submit();
    let tail: Vec<String> = rig.channels()[before..].to_vec();
    // После сохранения — проверка конфига, и записанный конфиг читается заново (с проверкой).
    assert_eq!(
        tail,
        [
            "game-config-write",
            "game-settings-save",
            "player-config-check",
            "game-config-read",
            "player-config-check"
        ]
    );
    assert_eq!(
        rig.calls_to("game-config-write"),
        [json!(["218", null, {"sensitivity": 2.5}])]
    );
    assert_eq!(rig.events.borrow().len(), 1);
}

#[test]
#[ignore = "в 0.4.9 #L_GS_CFG_RUNNING разрешает сохранять при запущенной игре (exec %0%), перенос — G5"]
fn a_running_game_keeps_the_window_open_on_the_configs_tab() {
    let scene = with_invoke(
        "game-settings-configs",
        json!({
            "game-config-write": {"status": "error", "code": "GAME_RUNNING"},
            "game-settings-save": {"status": "ok", "settings": {}},
        }),
    );
    let rig = rig(&scene);
    rig.mount();
    rig.session.select_section("launch");
    rig.session.config_set_number("sensitivity", "2.5");
    rig.session.submit();
    // Настройки не сохранены, окно открыто, вкладка «Конфиги», конфиг прочитан заново.
    assert!(rig.calls_to("game-settings-save").is_empty());
    assert!(rig.events.borrow().is_empty());
    let view = rig.session.view();
    assert_eq!(view.active_id, "configs");
    assert_eq!(
        view.alert,
        "The game is running. Close it to save these settings: on exit it rewrites config.cfg."
    );
    assert!(view.alert_error);
    assert_eq!(rig.calls_to("game-config-read").len(), 2);
    assert!(!view.saving);
}

#[test]
fn launch_option_errors_land_next_to_their_field() {
    let cases = [
        (
            json!({"status": "error", "error": {"code": "FORBIDDEN_PARAM", "param": "-steam"}}),
            false,
            "The -steam option is protected and cannot be changed",
        ),
        (
            json!({"status": "error", "error": {"code": "TOO_LONG"}}),
            false,
            "Launch options are too long (512 characters max)",
        ),
        (
            json!({"status": "error", "error": {"code": "WRAPPER_INVALID", "param": "-x", "field": "protonOptions"}}),
            true,
            "A program is expected before %command%, not -x. Put game options after %command%",
        ),
        // Нет `error` в ответе — общий текст у параметров запуска.
        (
            json!({"status": "error"}),
            false,
            "Launch options could not be saved",
        ),
        // Пустой ответ — тоже.
        (Value::Null, false, "Launch options could not be saved"),
    ];
    for (reply, proton, text) in cases {
        let scene = with_invoke("game-settings-launch", json!({"game-settings-save": reply}));
        let rig = rig_with(&scene, "218", "Ghosting Renewed", true, Language::En);
        rig.mount();
        rig.session.submit();
        let view = rig.session.view();
        let (own, other) = if proton {
            (&view.proton_error, &view.args_error)
        } else {
            (&view.args_error, &view.proton_error)
        };
        assert_eq!(own, text);
        assert_eq!(other, "");
        assert!(rig.events.borrow().is_empty(), "окно остаётся открытым");
        assert!(!view.saving);
        // Правка поля гасит свою ошибку.
        if proton {
            rig.session.set_proton("X=1 %command%");
        } else {
            rig.session.set_args("-high");
        }
        let view = rig.session.view();
        assert_eq!(
            if proton {
                &view.proton_error
            } else {
                &view.args_error
            },
            ""
        );
    }
}

#[test]
fn saving_keeps_the_page_open_and_takes_the_saved_draft() {
    let scene = with_invoke(
        "game-settings-configs",
        json!({
            "game-config-write": {"status": "ok"},
            "game-settings-save": {"status": "ok", "settings": {"console": true, "launchArgs": "-x"}},
        }),
    );
    let rig = rig(&scene);
    rig.mount();
    assert!(!rig.session.has_unsaved());
    rig.session.config_set_number("sensitivity", "2.5");
    assert!(rig.session.has_unsaved());
    rig.session.select_section("keyboard");
    let reads = rig.calls_to("game-config-read").len();
    rig.session.submit();
    assert_eq!(rig.events.borrow().len(), 1, "сохранено, страница на месте");
    let view = rig.session.view();
    assert_eq!(view.active_id, "keyboard");
    assert_eq!(view.args, "-x");
    assert!(view.toggles[0].checked);
    // Правки конфигов записаны: окно читает конфиг заново.
    assert!(rig.session.edits().is_empty());
    assert_eq!(rig.calls_to("game-config-read").len(), reads + 1);
}

#[test]
fn a_config_changed_outside_stops_the_save_and_offers_a_reload() {
    let scene = with_invoke(
        "game-settings-launch",
        json!({"game-settings-save": {"status": "error", "error": {"code": "CONFIG_CHANGED"}}}),
    );
    let rig = rig(&scene);
    rig.mount();
    rig.session.select_section("launch");
    rig.session.set_flag("console", true);
    rig.session.submit();
    let footer = rig.session.view().footer;
    assert!(footer.file_changed && footer.note_error);
    assert_eq!(footer.note, rig.session.view().footer.note);
    assert!(rig.session.file_changed());
    let gets = rig.calls_to("game-settings-get").len();
    rig.session.reload_after_change();
    assert!(!rig.session.file_changed());
    assert_eq!(rig.calls_to("game-settings-get").len(), gets + 1);
    // «Отмена» отбрасывает правки: настройки читаются заново.
    rig.session.set_flag("console", true);
    assert!(rig.session.has_unsaved());
    rig.session.discard_changes();
    assert!(!rig.session.has_unsaved());
    assert!(!rig.session.view().toggles[0].checked);
}

#[test]
fn settings_that_failed_to_load_are_never_saved_over_the_players_own() {
    let scene = with_invoke(
        "game-settings-launch",
        json!({"game-settings-get": {"status": "error", "error": {"code": "SETTINGS_UNAVAILABLE"}}}),
    );
    let rig = rig(&scene);
    rig.mount();
    rig.session.select_section("launch");
    assert!(!rig.session.draft_loaded());
    rig.session.set_flag("console", true);
    assert!(!rig.session.view().footer.save_enabled);
    rig.session.submit();
    assert!(rig.calls_to("game-settings-save").is_empty());
}

#[test]
fn ghost_rows_ask_before_deleting_and_remove_only_the_confirmed_one() {
    let scene = with_invoke(
        "game-settings-launch",
        json!({"game-runs-delete": {"status": "ok"}}),
    );
    let rig = rig(&scene);
    rig.mount();
    rig.session.select_section("ghosts");
    let rows = rig.session.view().runs;
    assert_eq!(
        rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        ["two.run", "one.run"]
    );
    // Без вопроса удаление не идёт.
    rig.session.delete_run(&rows[0].id);
    assert!(rig.calls_to("game-runs-delete").is_empty());
    rig.session.confirm_run_delete(&rows[0].id);
    assert!(rig.session.view().runs[0].confirm);
    // «Отмена» закрывает вопрос.
    rig.session.cancel_run_delete();
    assert!(!rig.session.view().runs[0].confirm);
    rig.session.confirm_run_delete(&rows[0].id);
    rig.session.delete_run(&rows[0].id);
    assert_eq!(
        rig.calls_to("game-runs-delete"),
        [json!(["218", "two.run"])]
    );
    let rows = rig.session.view().runs;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "one.run");
    assert_eq!(rows[0].size, "9\u{a0}B");
    assert_eq!(rows[0].date, "12.10.2026 18:40");
}

#[test]
fn a_failed_deletion_keeps_the_row_and_says_why() {
    let scene = with_invoke(
        "game-settings-launch",
        json!({"game-runs-delete": {"status": "error", "code": "GAME_RUNNING"}}),
    );
    let rig = rig(&scene);
    rig.mount();
    rig.session.select_section("ghosts");
    let id = rig.session.view().runs[0].id.clone();
    rig.session.confirm_run_delete(&id);
    rig.session.delete_run(&id);
    let view = rig.session.view();
    assert_eq!(view.runs.len(), 2);
    assert!(view.runs[0].confirm, "вопрос остаётся для повтора");
    assert_eq!(
        view.runs_error,
        "The game is running. Close it and try again"
    );
}

#[test]
fn the_version_tab_shows_the_facts_of_the_installed_build() {
    let rig = rig(&scene("game-settings-launch"));
    rig.mount();
    let facts = rig.session.view().facts;
    assert_eq!(facts.len(), 7);
    assert_eq!(facts[1].label, "Game version");
    assert_eq!(facts[1].value, "1.0.9");
    assert_eq!(facts[4].value, "01.10.2026");
    // metadataState "legacy".
    assert_eq!(facts[5].value, "No metadata (legacy package)");
    assert_eq!(facts[0].value, "No data");
}

#[test]
fn an_uninstalled_game_has_no_editor_and_says_so() {
    let scene = with_invoke(
        "game-settings-configs",
        json!({"game-config-read": {"$args": [[["220", "4260"], {"status": "not-installed", "gameId": "hl2-4260", "engine": "source"}]]}}),
    );
    let rig = rig_with(&scene, "220", "Half-Life 2", false, Language::En);
    let view = rig.mount();
    assert_eq!(view.config_mode, ConfigMode::Empty);
    assert_eq!(view.empty_text, "Game is not installed");
    assert_eq!(view.path_text, "Game is not installed");
    assert!(!view.open_folder_enabled);
    // HL2: три сборки, выбор сборки виден; отсутствующая помечена.
    assert!(view.build_select);
    assert_eq!(view.build.items.len(), 3);
    assert_eq!(view.build.value, "4260");
    assert_eq!(view.build.label, "4260 (not installed)");
    // У HL2 нет «Призраков»: записи не читаются.
    assert!(rig.calls_to("game-runs-list").is_empty());
    // Без папки конфигов нет и файлов: список файлов не просили.
    assert!(rig.calls_to("game-cfg-files-list").is_empty());
}

#[test]
fn another_build_drops_the_edits_and_reads_its_own_config() {
    let scene = with_invoke(
        "game-settings-configs",
        json!({"game-config-info": {"$args": [
            [["220", null], {"status": "ok", "path": "C:\\HLSR\\hl2\\cfg", "exists": true, "build": "4260", "builds": [{"build": "4260", "installed": true}, {"build": "2707", "installed": true}]}],
            [["220", "2707"], {"status": "ok", "path": "C:\\HLSR\\hl2-2707\\cfg", "exists": true, "build": "2707", "builds": [{"build": "4260", "installed": true}, {"build": "2707", "installed": true}]}],
        ]},
        "game-config-read": {"$args": [
            [["220", "4260"], {"status": "ok", "gameId": "hl2-4260", "engine": "source", "running": false, "values": {"sensitivity": "3"}, "sources": {}}],
            [["220", "2707"], {"status": "ok", "gameId": "hl2-2707", "engine": "source-old", "running": false, "values": {"sensitivity": "5"}, "sources": {}}],
        ]}}),
    );
    let rig = rig_with(&scene, "220", "Half-Life 2", false, Language::En);
    rig.mount();
    assert_eq!(rig.session.config_build().as_deref(), Some("4260"));
    rig.session.config_set_number("sensitivity", "2.5");
    assert!(!rig.session.edits().is_empty());
    rig.session.select_build("2707");
    assert!(rig.session.edits().is_empty());
    assert_eq!(rig.session.config_build().as_deref(), Some("2707"));
    assert_eq!(
        rig.calls_to("game-config-info").last().unwrap(),
        &json!(["220", "2707"])
    );
    assert_eq!(
        rig.calls_to("game-config-read").last().unwrap(),
        &json!(["220", "2707"])
    );
    assert_eq!(rig.session.view().path_text, "C:\\HLSR\\hl2-2707\\cfg");
}

#[test]
fn a_detached_session_finishes_its_calls_but_neither_shows_nor_reports_anything() {
    let scene = with_invoke(
        "game-settings-launch",
        json!({"game-settings-save": {"status": "ok", "settings": {}}}),
    );
    let rig = rig(&scene);
    rig.mount();
    let shown = *rig.renders.borrow();
    let files_lists = rig.calls_to("game-cfg-files-list").len();
    rig.session.detach();
    rig.session.set_flag("startTimer", true);
    rig.session.submit();
    // Как `async`-метод уничтоженного компонента Vue: сохранение дошло до главного процесса,
    // `emit` и обновление экрана — пустые операции.
    assert_eq!(rig.calls_to("game-settings-save").len(), 1);
    assert_eq!(rig.calls_to("game-cfg-files-list").len(), files_lists);
    assert!(rig.events.borrow().is_empty(), "{:?}", rig.events.borrow());
    assert_eq!(*rig.renders.borrow(), shown);
    assert_eq!(rig.session.tasks().pending(), 0);
}

/// «Пианино» (0.4.9): занята каждая клавиша всей схемы в сохранённом — `runner-secret-unlock`
/// один раз, на следующих перерисовках — нет; снятая клавиша — снова «не всё».
#[test]
fn every_saved_key_bound_unlocks_the_piano_once() {
    use hlsr_ui::keyboard_layout::{Engine, layout_for};
    let binds: serde_json::Map<String, Value> = layout_for(Engine::GoldSrc)
        .board_keys
        .iter()
        .map(|key| {
            (
                (*key).to_owned(),
                json!({"command": "+jump", "source": "config.cfg"}),
            )
        })
        .collect();
    let scene = with_invoke(
        "game-settings-launch",
        json!({"game-settings-get": {"status": "ok", "settings": {"binds": {}}, "bindsGameId": "hl1-won",
            "keyBinds": {"binds": binds, "aliases": {}, "cvars": {}, "files": ["config.cfg"]}}}),
    );
    let rig = rig_with(&scene, "70", "Half-Life", false, Language::En);
    let view = rig.mount();
    assert!(view.keyboard.as_ref().is_some_and(|kb| kb.every_key_bound));
    assert_eq!(rig.calls_to("runner-secret-unlock"), [json!(["piano"])]);
    // Черновик снимает пробел, но «Пианино» смотрит на сохранённое: не повторяется.
    rig.session.kb_select("SPACE");
    rig.session.kb_assign("");
    rig.session.view();
    assert_eq!(rig.calls_to("runner-secret-unlock").len(), 1);
}
