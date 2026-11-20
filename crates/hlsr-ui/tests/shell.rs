//! Машина состояний оболочки без окна: тосты, окно подтверждения, тексты ошибок, смена языка,
//! секретные достижения, логотип, навигация и колонка (сроки — по часам `ManualEnv`).

use hlsr_core::language::Language;
use hlsr_core::paths::Paths;
use hlsr_core::settings::Settings;
use hlsr_ui::controllers::game_availability;
use hlsr_ui::controllers::nav::{
    LOGO_CLICK_GAP_MS, LOGO_REPAINT, LogoController, NavController, NavPrefs,
};
use hlsr_ui::controllers::shell::{
    Confirm, ConfirmAnswer, MAX_TOASTS, ShellController, TOAST_LIFETIME, initial_choice,
};
use hlsr_ui::env::ManualEnv;
use serde_json::json;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

fn shell() -> (Rc<ManualEnv>, ShellController) {
    let env = ManualEnv::new();
    let shell = ShellController::new(env.clone(), Language::En);
    (env, shell)
}

fn texts(shell: &ShellController) -> Vec<String> {
    shell.toasts().into_iter().map(|toast| toast.text).collect()
}

#[test]
fn toasts_keep_three_and_expire_after_2800_ms() {
    let (env, shell) = shell();
    let changes = Rc::new(Cell::new(0));
    let counter = changes.clone();
    shell.on_change(move || counter.set(counter.get() + 1));
    shell.notify("a", "");
    assert_eq!(shell.toasts()[0].icon, "fas fa-info-circle");
    env.advance(1000.0);
    shell.notify("b", "fas fa-check");
    shell.notify("c", "fas fa-check");
    shell.notify("d", "fas fa-check");
    // Четвёртый вытесняет самый старый.
    assert_eq!(texts(&shell), ["b", "c", "d"]);
    assert_eq!(MAX_TOASTS, 3);
    assert_eq!(shell.toasts()[0].id, 2);
    assert_eq!(changes.get(), 4, "вид узнаёт о каждом тосте");
    // «b», «c», «d» появились в 1 000 мс и живут до 3 800 мс («a» ушёл раньше срока).
    env.advance(TOAST_LIFETIME.as_millis() as f64 - 1.0);
    assert_eq!(texts(&shell), ["b", "c", "d"]);
    env.advance(1.0);
    assert!(shell.toasts().is_empty());
    // Щелчок: закрыть раньше; повтор — ничего.
    shell.notify("e", "");
    let id = shell.toasts()[0].id;
    shell.dismiss_toast(id);
    shell.dismiss_toast(id);
    assert!(shell.toasts().is_empty());
}

#[test]
fn confirm_answers_and_replaces_previous() {
    let (_env, shell) = shell();
    let mut first = shell.ask_confirm(Confirm {
        title: "Удалить?".into(),
        items: vec!["a".into(), "b".into()],
        danger: true,
        ..Confirm::default()
    });
    assert_eq!(shell.confirm().unwrap().title, "Удалить?");
    assert_eq!(first.try_answer(), None);

    // Новый запрос: прежний получает «нет».
    let mut second = shell.ask_confirm(Confirm {
        title: "LiveSplit".into(),
        choices: vec![
            ("70".into(), "Half-Life".into()),
            ("218".into(), "Ghosting".into()),
        ],
        default_choice: Some("218".into()),
        ..Confirm::default()
    });
    assert_eq!(first.try_answer(), Some(ConfirmAnswer::Declined));
    assert_eq!(initial_choice(&shell.confirm().unwrap()), "218");
    shell.resolve_confirm(true, "70");
    assert_eq!(
        second.try_answer(),
        Some(ConfirmAnswer::Choice("70".into()))
    );
    assert!(!shell.confirm_open() && shell.confirm().is_none());

    // Умолчание не из вариантов — первый вариант; «нет» с вариантами — отказ.
    let mut third = shell.ask_confirm(Confirm {
        choices: vec![("a".into(), "A".into())],
        default_choice: Some("zzz".into()),
        ..Confirm::default()
    });
    assert_eq!(initial_choice(&shell.confirm().unwrap()), "a");
    shell.resolve_confirm(false, "a");
    assert_eq!(third.try_answer(), Some(ConfirmAnswer::Declined));
    assert_eq!(initial_choice(&Confirm::default()), "");

    let mut fourth = shell.ask_confirm(Confirm::default());
    shell.resolve_confirm(true, "");
    assert_eq!(fourth.try_answer(), Some(ConfirmAnswer::Accepted));
    assert!(ConfirmAnswer::Accepted.accepted() && !ConfirmAnswer::Declined.accepted());
    // Ответ без открытого окна — ничего.
    shell.resolve_confirm(true, "");
}

#[test]
fn confirm_future_resolves_in_async_scenario() {
    let (env, shell) = shell();
    let answer = Rc::new(RefCell::new(None));
    let slot = answer.clone();
    let future = shell.ask_confirm(Confirm::default());
    hlsr_ui::env::Env::spawn_local(
        &*env,
        Box::pin(async move { *slot.borrow_mut() = Some(future.await) }),
    );
    env.run();
    assert_eq!(*answer.borrow(), None);
    shell.resolve_confirm(true, "");
    env.run();
    assert_eq!(*answer.borrow(), Some(ConfirmAnswer::Accepted));
}

#[test]
fn delivery_error_texts_by_code() {
    let (_env, shell) = shell();
    let generic = shell.tr("#L_D_ERROR", &[]);
    assert_ne!(generic, "#L_D_ERROR");
    let known = shell.tr("#L_D_ERR_NO_SPACE", &[]);
    assert_ne!(known, "#L_D_ERR_NO_SPACE", "ключ из en.json");
    assert_eq!(
        shell.error_text("#L_D_ERR_", Some("NO_SPACE"), "#L_D_ERROR"),
        known
    );
    assert_eq!(
        shell.error_text("#L_D_ERR_", Some("NOPE_CODE"), "#L_D_ERROR"),
        generic
    );
    assert_eq!(shell.error_text("#L_D_ERR_", None, "#L_D_ERROR"), generic);
    shell.notify_delivery_error(Some("NOPE_CODE"));
    shell.notify_delivery_error(Some("NO_SPACE"));
    let toasts = shell.toasts();
    assert_eq!(toasts[0].text, generic);
    assert_eq!(toasts[1].text, known);
    assert_eq!(toasts[1].icon, "fas fa-exclamation-triangle");
    // Свой перевод заменяет строки tr.rs.
    shell.set_translator(Rc::new(|_, key, _| format!("<{key}>")));
    assert_eq!(
        shell.error_text("#L_D_ERR_", Some("X"), "#L_D_ERROR"),
        "<#L_D_ERR_X>"
    );
}

#[test]
fn language_change_bumps_the_generation_and_keeps_toasts() {
    let (_env, shell) = shell();
    shell.notify("остаётся", "");
    let root = std::env::temp_dir().join(format!("hlsr-ui-shell-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut settings = Settings::open(&Paths::under(&root));
    assert!(
        !shell.set_language(&mut settings, Language::En),
        "тот же язык"
    );
    assert_eq!(shell.lang_rev(), 0);
    assert!(shell.set_language(&mut settings, Language::Ru));
    assert_eq!((shell.lang_rev(), shell.language()), (1, Language::Ru));
    assert_eq!(
        shell.tr("#L_D_ERROR", &[]),
        hlsr_ui::tr(Language::Ru, "#L_D_ERROR", &[])
    );
    assert_eq!(
        Settings::open(&Paths::under(&root)).language(),
        Some(Language::Ru)
    );
    // Тосты живут в оболочке, не в рамке.
    assert_eq!(texts(&shell), ["остаётся"]);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn secret_achievement_goes_to_listener() {
    let (_env, shell) = shell();
    shell.unlock_achievement("wake-up"); // без слушателя — только журнал
    let got = Rc::new(RefCell::new(Vec::new()));
    let sink = got.clone();
    shell.on_unlock_achievement(move |id| sink.borrow_mut().push(id.to_owned()));
    shell.unlock_achievement("wake-up");
    assert_eq!(*got.borrow(), ["wake-up"]);
}

#[derive(Default)]
struct Prefs(Cell<bool>);

impl NavPrefs for Prefs {
    fn rail_expanded(&self) -> bool {
        self.0.get()
    }

    fn set_rail_expanded(&self, expanded: bool) {
        self.0.set(expanded);
    }
}

#[test]
fn logo_repaints_for_1500_ms_and_three_quick_clicks_unlock_back_to_past() {
    let (env, shell) = shell();
    let unlocked = Rc::new(RefCell::new(Vec::new()));
    let sink = unlocked.clone();
    shell.on_unlock_achievement(move |id| sink.borrow_mut().push(id.to_owned()));
    let logo = LogoController::with_random(env.clone(), &shell, Rc::new(|| 0.5));
    logo.double_click();
    assert_eq!(logo.color(), Some(hlsr_ui::Rgba::rgb(125, 125, 125)));
    // Новый щелчок отодвигает возврат цвета.
    env.advance(1000.0);
    logo.double_click();
    env.advance(LOGO_REPAINT.as_millis() as f64 - 1.0);
    assert!(logo.color().is_some());
    env.advance(1.0);
    assert_eq!(logo.color(), None);
    // 0.4.9: перекраски достижения не дают — только три щелчка подряд.
    for _ in 0..3 {
        logo.double_click();
        env.advance(1500.0);
    }
    assert!(unlocked.borrow().is_empty());
    // Перерыв больше 650 мс начинает счёт заново.
    assert!(!logo.click());
    env.advance(LOGO_CLICK_GAP_MS + 1.0);
    assert!(!logo.click());
    env.advance(LOGO_CLICK_GAP_MS);
    assert!(!logo.click());
    assert!(unlocked.borrow().is_empty());
    env.advance(100.0);
    assert!(logo.click(), "третий подряд — пони");
    assert_eq!(*unlocked.borrow(), ["back-to-past"]);
    // Счёт сброшен: следующий щелчок — снова первый.
    env.advance(100.0);
    assert!(!logo.click());
}

#[test]
fn toasts_go_to_the_window_layer_when_it_is_there() {
    let (_env, shell) = shell();
    let got = Rc::new(RefCell::new(Vec::new()));
    let sink = got.clone();
    shell.on_toast(move |text, icon, ms| {
        sink.borrow_mut()
            .push((text.to_owned(), icon.to_owned(), ms))
    });
    shell.notify("a", "");
    shell.notify_for("путь", "fas fa-folder", 15_000);
    assert_eq!(
        *got.borrow(),
        [
            ("a".into(), "fas fa-info-circle".into(), 0),
            ("путь".into(), "fas fa-folder".into(), 15_000)
        ]
    );
    assert!(shell.toasts().is_empty(), "очередь контроллера не нужна");
}

#[test]
fn navigation_follows_the_shell_data() {
    let (_env, shell) = shell();
    let prefs = Rc::new(Prefs::default());
    let nav = NavController::new(&shell, prefs.clone());
    let background = Rc::new(RefCell::new(Vec::new()));
    let sink = background.clone();
    nav.on_background_game(move |game| sink.borrow_mut().push(game.map(str::to_owned)));
    assert_eq!(
        (nav.section(), nav.selected_game()),
        ("games".into(), "218".into())
    );
    assert_eq!(nav.selected_index(), Some(0));

    // Колонка в «Играх» запоминается, на других разделах — до ухода.
    nav.toggle_rail();
    assert!(nav.rail_open() && prefs.0.get());
    nav.select_section("home");
    assert!(!nav.rail_open());
    nav.toggle_rail();
    assert!(nav.rail_open() && nav.rail_peek());
    assert_eq!(nav.rpc_activity(), "home");
    // 0.4.9: «Персонализация» — раздел `configs`; первый заход — на выбранной игре.
    nav.select_section("personal");
    assert_eq!(nav.section(), "home", "раздела personal больше нет");
    assert_eq!(nav.configs_game(), "");
    nav.select_section("configs");
    assert!(!nav.rail_peek());
    assert_eq!(nav.configs_game(), "218");
    assert_eq!(nav.rpc_activity(), "personalization");
    assert_eq!(*background.borrow(), [None]);

    // Игра: «Игры» и арт; недоступная — тост.
    nav.select_game("70");
    assert_eq!(
        (nav.section(), nav.selected_index()),
        ("games".into(), Some(2))
    );
    assert_eq!(*background.borrow(), [None, Some("70".into())]);
    nav.select_game("50");
    assert_eq!(nav.selected_game(), "70");
    assert_eq!(shell.toasts().len(), 1);

    // Онлайн и инструменты колонки.
    nav.set_online(&json!({"generatedAt": 1, "games": {"218": {"online": 3}}}));
    assert_eq!(nav.games().len(), 3);
    let state = |nav: &NavController| -> Vec<String> {
        nav.games().into_iter().map(|g| g.online_state).collect()
    };
    assert_eq!(state(&nav), ["unknown"; 3]);
    // Неофициальная сборка: счётчиков нет вовсе, не «нет данных».
    nav.show_online(false);
    assert_eq!(state(&nav), [""; 3]);
    nav.show_online(true);
    assert_eq!(state(&nav), ["unknown"; 3]);
    nav.set_tools(&json!([{"toolId": "livesplit", "shortName": "LiveSplit", "icon": "x"}]));
    assert_eq!(nav.selected_tool(), "livesplit");
    assert_eq!(nav.tools()[0].title, "LiveSplit");
    nav.select_tool("livesplit");
    assert_eq!(nav.active_tab(), "tools");
    assert_eq!(nav.rpc_activity(), "livesplit");
    assert_eq!(background.borrow().last(), Some(&None));

    // «Демо» — вкладка «Игр» без арта игры; выбор игры возвращает вкладку игр.
    nav.select_section("home");
    nav.select_demo();
    assert_eq!(
        (nav.section(), nav.active_tab(), nav.rpc_activity()),
        ("games".into(), "demo".into(), "demo")
    );
    assert_eq!(background.borrow().last(), Some(&None));
    nav.select_game("218");
    assert_eq!(nav.rpc_activity(), "games");
    assert_eq!(background.borrow().last(), Some(&Some("218".into())));
    // Игра «Персонализации» уже выбрана — другая игра её не меняет.
    nav.select_game("70");
    nav.select_section("configs");
    assert_eq!(nav.configs_game(), "218");
}

#[test]
fn column_games_hide_episodes() {
    let ids: Vec<&str> = game_availability::column_games()
        .iter()
        .map(|entry| entry.game.id)
        .collect();
    assert_eq!(ids, ["218", "220", "70"]);
    assert!(
        game_availability::column_games()
            .iter()
            .all(|entry| entry.available)
    );
    let names: Vec<&str> = game_availability::available_games()
        .iter()
        .map(|game| game.name)
        .collect();
    assert_eq!(names, ["Half-Life 2: Ghosting", "Half-Life 2", "Half-Life"]);
    assert!(!game_availability::is_available("50"));
    assert_eq!(game_availability::startup_game(None), "218");
    assert_eq!(
        game_availability::startup_game(Some(&serde_json::json!(70))),
        "70"
    );
    assert_eq!(
        game_availability::startup_game(Some(&serde_json::json!("380"))),
        "218"
    );
}
