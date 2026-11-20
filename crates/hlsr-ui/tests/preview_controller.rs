//! Контроллер превью (`GhostingSettingsPreview.vue` 0.4.7, задачи 33 и 34) без окна:
//! вызовы main (`ghosting-screen-get`, `game-personalization-preview-asset`) и их порядок,
//! ширина кадра по разрешению игры, подписи, «движение», окно во весь лаунчер, файлы игрока
//! (нет файла, не рисуется), перетаскивание оверлея и очередь кадров.

use hlsr_ui::controllers::preview::{Job, PreviewController, Reply, Variant};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

/// Подставной main: журнал вызовов и ответы по очереди (нет ответа — вызов висит).
#[derive(Default)]
struct Main {
    log: RefCell<Vec<(String, Vec<Value>)>>,
    pending: RefCell<VecDeque<(String, Reply)>>,
}

impl Main {
    fn invoke(self: &Rc<Self>) -> hlsr_ui::controllers::preview::Invoke {
        let main = self.clone();
        Rc::new(move |channel, args, reply| {
            main.log.borrow_mut().push((channel.to_owned(), args));
            main.pending
                .borrow_mut()
                .push_back((channel.to_owned(), reply));
        })
    }

    /// Ответить на самый старый вызов канала.
    fn answer(&self, channel: &str, value: Value) {
        let reply = {
            let mut pending = self.pending.borrow_mut();
            let at = pending
                .iter()
                .position(|(c, _)| c == channel)
                .unwrap_or_else(|| panic!("нет вызова {channel}"));
            pending.remove(at).unwrap().1
        };
        reply(value);
    }

    fn channels(&self) -> Vec<String> {
        self.log.borrow().iter().map(|(c, _)| c.clone()).collect()
    }
}

/// Контроллер с подставным потоком рисования: запросы копятся, кадр отдаёт тест.
struct Fixture {
    main: Rc<Main>,
    jobs: Rc<RefCell<Vec<Job>>>,
    controller: Rc<PreviewController>,
}

fn fixture(variant: Variant) -> Fixture {
    let main = Rc::new(Main::default());
    let controller = PreviewController::new(variant, "218", main.invoke());
    let jobs = Rc::new(RefCell::new(Vec::new()));
    {
        let jobs = jobs.clone();
        controller.set_sender(move |job| jobs.borrow_mut().push(job));
    }
    Fixture {
        main,
        jobs,
        controller,
    }
}

impl Fixture {
    /// Отдать кадр на последний запрос.
    fn deliver(&self) {
        let job = self.jobs.borrow().last().cloned().expect("запрос кадра");
        let frame = tiny_skia::Pixmap::new(job.width, 900).unwrap();
        self.controller.frame_ready((job.serial, Ok(frame.into())));
    }
}

fn personalization(patch: Value) -> Value {
    let mut value = hlsr_delivery::ghosting_personalization::default_personalization().to_value();
    let object = value.as_object_mut().unwrap();
    for (k, v) in patch.as_object().unwrap() {
        object.insert(k.clone(), v.clone());
    }
    value
}

#[test]
fn ghosting_frame_takes_the_width_of_the_game_resolution() {
    let f = fixture(Variant::Ghosting);
    f.controller.mount(false, Some((1920, 1080)));
    // Первый кадр запрошен сразу; разрешение — у main.
    assert_eq!(f.jobs.borrow().len(), 1);
    assert_eq!(
        *f.main.log.borrow(),
        vec![("ghosting-screen-get".to_owned(), vec![json!("218")])]
    );
    let state = f.controller.state();
    assert!(state.loading && !state.ready);
    assert_eq!(state.message, "#L_GS_X_OVERLAY_LOADING");
    // Пока ответа нет — экран монитора.
    assert_eq!(
        state.screen,
        Some((
            "#L_GS_X_PREVIEW_SCREEN_DISPLAY".to_owned(),
            "1920\u{d7}1080".to_owned()
        ))
    );
    assert_eq!(state.frame_width, 1600);

    f.deliver();
    let state = f.controller.state();
    assert!(state.ready && !state.loading);
    assert_eq!(state.message, "");

    // 21:9 — кадр шире: round(900 × 2560 / 1080) = 2133.
    f.main.answer(
        "ghosting-screen-get",
        json!({"width": 2560, "height": 1080, "source": "game"}),
    );
    let state = f.controller.state();
    assert_eq!(state.frame_width, 2133);
    assert_eq!(
        state.screen,
        Some((
            "#L_GS_X_PREVIEW_SCREEN_GAME".to_owned(),
            "2560\u{d7}1080".to_owned()
        ))
    );
    // Кадр, который уже рисуется, не прерывается: следующий — по новой ширине.
    assert_eq!(f.jobs.borrow().last().unwrap().width, 1600);
    f.deliver();
    assert_eq!(f.jobs.borrow().last().unwrap().width, 2133);
    // Пределы — 5:4 и 32:9.
    assert_eq!(
        hlsr_ui::render::preview::frame_width_of(Some((1280.0, 1024.0))),
        1125
    );
    assert_eq!(
        hlsr_ui::render::preview::frame_width_of(Some((5120.0, 1080.0))),
        3200
    );
    assert_eq!(hlsr_ui::render::preview::frame_width_of(None), 1600);
}

#[test]
fn hl1_preview_asks_main_nothing_and_keeps_1600() {
    let f = fixture(Variant::Hl1);
    f.controller.mount(false, Some((2560, 1080)));
    f.deliver();
    assert!(f.main.log.borrow().is_empty());
    let state = f.controller.state();
    assert_eq!(state.frame_width, 1600);
    assert_eq!(state.screen, None);
    // «Движения» у HL1 нет.
    f.controller.set_section("weapon");
    assert!(!f.controller.state().motion);
    // Прицел пака спрятан переключателем BXT — подпись.
    f.controller.set_value(personalization(json!({
        "crossEnabled": true,
        "crossHideDefault": true,
        "extra": {"packCrosshair": "crosshair-changer"}
    })));
    assert_eq!(f.controller.state().caption, "#L_GS_H_PREVIEW_PACK_HIDDEN");
    f.controller.set_value(personalization(json!({
        "crossEnabled": true,
        "crossHideDefault": true,
        "extra": {"packCrosshair": "default"}
    })));
    assert_eq!(f.controller.state().caption, "");
}

#[test]
fn motion_exists_in_weapon_and_keys_sections_and_resets_on_section_change() {
    let f = fixture(Variant::Ghosting);
    f.controller.mount(false, None);
    f.deliver();
    assert!(!f.controller.state().motion);
    f.controller.set_section("weapon");
    let state = f.controller.state();
    assert!(state.motion);
    assert_eq!(state.caption, "#L_GS_X_OVERLAY_WEAPON");
    f.controller.toggle_motion();
    assert!(f.controller.state().animating);
    // Кадры идут друг за другом: новый — когда пришёл прежний, время растёт.
    let before = f.jobs.borrow().len();
    f.deliver();
    assert_eq!(f.jobs.borrow().len(), before + 1);
    f.controller.set_section("keys");
    let state = f.controller.state();
    assert!(state.motion && !state.animating);
    f.controller.set_section("hud");
    assert!(!f.controller.state().motion);
}

#[test]
fn reduced_motion_draws_the_moving_frame_once_at_a_quarter_second() {
    let f = fixture(Variant::Ghosting);
    f.controller.mount(true, None);
    f.deliver();
    f.controller.set_section("weapon");
    f.controller.toggle_motion();
    f.deliver();
    let last = f.jobs.borrow().last().cloned().unwrap();
    assert_eq!(last.time, 0.25);
    // Без движения ОС — один кадр, без очереди дальше.
    let count = f.jobs.borrow().len();
    f.deliver();
    assert_eq!(f.jobs.borrow().len(), count);
}

#[test]
fn expanded_view_opens_only_when_the_frame_is_ready() {
    let f = fixture(Variant::Ghosting);
    f.controller.mount(false, None);
    f.controller.open_expanded();
    assert!(!f.controller.state().expanded);
    f.deliver();
    f.controller.open_expanded();
    assert!(f.controller.state().expanded);
    f.controller.close_expanded();
    assert!(!f.controller.state().expanded);
}

#[test]
fn frame_error_before_the_first_frame_shows_the_error() {
    let f = fixture(Variant::Ghosting);
    f.controller.mount(false, None);
    let serial = f.jobs.borrow()[0].serial;
    f.controller.frame_ready((serial, Err("нет кадра".into())));
    let state = f.controller.state();
    assert!(!state.ready && !state.loading);
    assert_eq!(state.message, "#L_GS_X_OVERLAY_ERROR");
}

#[test]
fn player_files_are_asked_after_the_first_frame_and_errors_are_captioned() {
    let f = fixture(Variant::Ghosting);
    f.controller.mount(false, None);
    f.controller.set_value(personalization(json!({
        "extra": {
            "gh_hud_font": "C:\\fonts\\hud.ttf",
            "gh_spray_file": "spray.png",
            "gh_showfps_font": "Arial"
        }
    })));
    // До первого кадра файлы не спрашиваются (loadAssets — после loadLayers).
    let asked = |f: &Fixture| {
        f.main
            .log
            .borrow()
            .iter()
            .filter(|(c, _)| c == "game-personalization-preview-asset")
            .map(|(_, a)| a.clone())
            .collect::<Vec<_>>()
    };
    // set_value до готовности уже начинает загрузку, как watch value 0.4.7.
    let first = asked(&f);
    assert_eq!(
        first,
        vec![
            vec![
                json!("218"),
                json!({"kind": "font", "value": "C:\\fonts\\hud.ttf"})
            ],
            vec![json!("218"), json!({"kind": "font", "value": "Arial"})],
            vec![json!("218"), json!({"kind": "spray", "value": "spray.png"})],
        ]
    );
    f.deliver();
    // Тот же набор файлов — заново не спрашивается.
    assert_eq!(asked(&f).len(), 3);
    // Системный шрифт не нашёлся — без подписи; файл шрифта не нашёлся — «недоступен».
    f.main.answer(
        "game-personalization-preview-asset",
        json!({"status": "missing"}),
    );
    assert_eq!(f.controller.state().caption, "#L_GS_X_OVERLAY_ASSET_ERROR");
    assert!(f.controller.state().caption_error);
    f.main.answer(
        "game-personalization-preview-asset",
        json!({"status": "missing"}),
    );
    assert_eq!(f.controller.state().caption, "#L_GS_X_OVERLAY_ASSET_ERROR");
    // Спрей прочитан, но это не картинка — «не рисуется».
    f.main.answer(
        "game-personalization-preview-asset",
        json!({"status": "ok", "dataUrl": "data:image/png;base64,AAAA"}),
    );
    assert_eq!(f.controller.state().caption, "#L_GS_X_OVERLAY_ASSET_DECODE");
    // Окно снова открыли — новая попытка тех же файлов.
    f.controller.set_active(false);
    f.controller.set_active(true);
    assert_eq!(asked(&f).len(), 6);
    assert_eq!(f.controller.state().caption, "");
    // ghosting-screen-get — при каждом открытии.
    assert_eq!(
        f.main
            .channels()
            .iter()
            .filter(|c| *c == "ghosting-screen-get")
            .count(),
        2
    );
}

#[test]
fn dragged_overlay_becomes_percent_position_of_the_free_space() {
    let f = fixture(Variant::Hl1);
    f.controller.mount(false, None);
    f.deliver();
    let overlay = hlsr_delivery::livesplit_overlay::default_overlay().to_value();
    f.controller.set_overlay(Some(overlay.clone()), None, true);
    let handle = f
        .controller
        .state()
        .handle
        .expect("ручка в разделе оверлея");
    let emitted = Rc::new(RefCell::new(None));
    {
        let emitted = emitted.clone();
        f.controller
            .on_overlay(move |value| *emitted.borrow_mut() = Some(value));
    }
    // Влево до упора и на середину по высоте свободного места.
    let free_y = 900.0 - handle.height;
    f.controller.move_overlay(0.0, free_y / 2.0);
    let value = emitted.borrow().clone().expect("emit('overlay')");
    assert_eq!(value["x"], json!(0.0));
    assert_eq!(value["y"], json!(50.0));
    // Тот же угол — без события.
    *emitted.borrow_mut() = None;
    f.controller.set_overlay(Some(value.clone()), None, true);
    f.controller.move_overlay(0.0, free_y / 2.0);
    assert!(emitted.borrow().is_none());
    // Вне раздела оверлея ручки нет, выключенный оверлей не виден.
    let mut off = overlay;
    off["enabled"] = json!(false);
    f.controller.set_overlay(Some(off), None, false);
    assert!(f.controller.state().handle.is_none());
}

#[test]
fn changes_before_the_first_frame_reach_the_frame_after_it() {
    // HL1: ответ game-personalization-get и оверлей приходят, пока рисуется первый кадр
    // (снимок на момент mount) — следующий кадр по ним, как первый draw() 0.4.7 после
    // loadLayers.
    let f = fixture(Variant::Hl1);
    f.controller.mount(false, None);
    let value = personalization(json!({"crossEnabled": true, "crossSize": 14}));
    f.controller.set_value(value.clone());
    let overlay = hlsr_delivery::livesplit_overlay::default_overlay().to_value();
    f.controller.set_overlay(Some(overlay), None, true);
    assert_eq!(f.jobs.borrow().len(), 1, "первый кадр не прерывается");
    assert!(f.jobs.borrow()[0].overlay.is_none());
    f.deliver();
    assert_eq!(f.jobs.borrow().len(), 2);
    let next = f.jobs.borrow().last().cloned().unwrap();
    assert_eq!(next.value, value);
    assert!(next.overlay.is_some());
    assert!(f.controller.state().handle.is_some(), "ручка оверлея");
    f.deliver();
    assert_eq!(f.jobs.borrow().len(), 2);

    // Ghosting: разрешение игры пришло до первого кадра — следующий кадр по его ширине,
    // один (кадр loadAssets и «ещё раз» — одно и то же).
    let f = fixture(Variant::Ghosting);
    f.controller.mount(false, None);
    f.main.answer(
        "ghosting-screen-get",
        json!({"width": 2560, "height": 1080, "source": "game"}),
    );
    assert_eq!(f.jobs.borrow().len(), 1);
    f.deliver();
    assert_eq!(f.jobs.borrow().len(), 2);
    assert_eq!(f.jobs.borrow().last().unwrap().width, 2133);
    f.deliver();
    assert_eq!(f.jobs.borrow().len(), 2);

    // Закрытое превью до первого кадра изменений не копит: кадр — при открытии.
    let f = fixture(Variant::Hl1);
    f.controller.mount(false, None);
    f.controller.set_active(false);
    f.controller.set_value(value.clone());
    f.deliver();
    assert_eq!(f.jobs.borrow().len(), 1);
    f.controller.set_active(true);
    assert_eq!(f.jobs.borrow().len(), 2);
    assert_eq!(f.jobs.borrow().last().unwrap().value, value);
}

#[test]
fn ghost_and_spray_sections_show_their_scene_0_4_9() {
    use hlsr_ui::controllers::preview::{Frame, SceneLine};
    use hlsr_ui::render::ghost_scene::{Kind, SprayInfo};
    let f = fixture(Variant::Ghosting);
    f.controller.mount(false, Some((2560, 1080)));
    f.deliver();
    f.controller.set_section("ghosts");
    // Сцена ещё не рисовалась — её кадр запрошен сразу (свой поток, кадр HUD в работе не
    // ждёт), пока «Загрузка».
    let state = f.controller.state();
    assert!(state.portrait && state.loading);
    assert_eq!(state.scene, Some(Kind::Ghost));
    assert_eq!(state.screen, None, "у сцены нет строки разрешения");
    let job = f.jobs.borrow().last().cloned().unwrap();
    assert_eq!((job.scene, job.width), (Some(Kind::Ghost), 675));
    f.deliver();
    assert!(f.controller.state().ready);
    assert_eq!(
        f.controller.state().scene_lines,
        vec![SceneLine::Ghost(220)]
    );
    // Ширина портрета — от окна вида: 400 × 500 → 0,8 × 900; пределы 0,45 и 1,2.
    f.controller.set_stage_box(400.0, 500.0);
    assert_eq!(f.controller.state().frame_width, 720);
    assert_eq!(f.jobs.borrow().last().unwrap().width, 720);
    f.controller.set_stage_box(100.0, 500.0);
    assert_eq!(f.controller.state().frame_width, 405);
    f.controller.set_stage_box(900.0, 500.0);
    assert_eq!(f.controller.state().frame_width, 1080);

    // «Эффекты и спреи»: кадр 1600, подпись — от кадра.
    f.controller.set_section("misc");
    f.deliver();
    let job = f.jobs.borrow().last().cloned().unwrap();
    assert_eq!((job.scene, job.width), (Some(Kind::Spray), 1600));
    let spray = Some(SprayInfo {
        units: 38.4,
        texels: 64,
    });
    f.controller.frame_ready((
        job.serial,
        Ok(Frame {
            pixmap: tiny_skia::Pixmap::new(1600, 900).unwrap(),
            spray,
        }),
    ));
    assert_eq!(
        f.controller.state().scene_lines,
        vec![
            SceneLine::SpraySize {
                units: 38.4,
                texels: 64
            },
            SceneLine::Key("#L_GS_X_SCENE_SPRAY_EXAMPLE"),
        ]
    );
    f.controller
        .set_value(personalization(json!({"extra": {"gh_sprays": false}})));
    assert_eq!(
        f.controller.state().scene_lines,
        vec![SceneLine::Key("#L_GS_X_SCENE_SPRAYS_OFF")]
    );
    // Назад к HUD: кадр с разрешением игры, без сцены.
    f.controller.set_section("hud");
    let state = f.controller.state();
    assert!(state.scene.is_none() && !state.portrait && state.scene_lines.is_empty());
}

/// Сцена раздела не ждёт кадра HUD в работе (свой поток), опоздавший кадр HUD отброшен, кадр
/// приходит со своей сценой (вид не ставит HUD в окно призрака); раздел, сменённый у закрытого
/// превью, рисуется первым же кадром после открытия.
#[test]
fn scene_does_not_wait_for_hud_frame_and_frames_carry_their_scene() {
    use hlsr_ui::render::ghost_scene::Kind;
    let f = fixture(Variant::Ghosting);
    let got = Rc::new(RefCell::new(Vec::new()));
    {
        let got = got.clone();
        f.controller
            .on_frame(move |_, scene| got.borrow_mut().push(scene));
    }
    f.controller.mount(false, None);
    // Первый кадр HUD; после него — ещё один (файлы игрока), он в работе.
    f.deliver();
    f.controller.set_section("ghosts");
    let kinds: Vec<_> = f.jobs.borrow().iter().map(|job| job.scene).collect();
    assert_eq!(kinds, [None, None, Some(Kind::Ghost)]);
    f.deliver();
    let stale = f.jobs.borrow()[1].clone();
    let frame = tiny_skia::Pixmap::new(stale.width, 900).unwrap();
    f.controller.frame_ready((stale.serial, Ok(frame.into())));
    assert_eq!(*got.borrow(), [None, Some(Kind::Ghost)]);
    assert!(f.controller.state().ready);
    f.controller.set_section("meters");
    f.deliver();
    f.controller.set_active(false);
    let sent = f.jobs.borrow().len();
    f.controller.set_section("ghosts");
    assert_eq!(f.jobs.borrow().len(), sent, "закрытое превью не рисует");
    f.controller.set_active(true);
    assert_eq!(f.jobs.borrow().last().unwrap().scene, Some(Kind::Ghost));
}

#[test]
fn failed_scene_is_tried_again_on_section_change_and_reopen_0_4_9() {
    use hlsr_ui::render::ghost_scene::Kind;
    let f = fixture(Variant::Ghosting);
    f.controller.mount(false, None);
    f.deliver();
    f.controller.set_section("ghosts");
    let fail = |f: &Fixture| {
        let job = f.jobs.borrow().last().cloned().unwrap();
        assert_eq!(job.scene, Some(Kind::Ghost));
        f.controller
            .frame_ready((job.serial, Err("нет сцены".into())));
        assert_eq!(f.controller.state().message, "#L_GS_X_OVERLAY_ERROR");
    };
    fail(&f);
    // Значение сцену заново не грузит (как `watch value` 0.4.9).
    let sent = f.jobs.borrow().len();
    f.controller
        .set_value(personalization(json!({"extra": {"gh_sprays": false}})));
    assert_eq!(f.jobs.borrow().len(), sent);
    // Другой раздел и назад — `loadScene` снова.
    f.controller.set_section("hud");
    f.deliver();
    f.controller.set_section("ghosts");
    assert!(f.controller.state().loading);
    fail(&f);
    // Страницу закрыли и открыли — тоже.
    f.controller.set_active(false);
    f.controller.set_active(true);
    assert!(f.controller.state().loading);
    f.deliver();
    assert!(f.controller.state().ready);
}

#[test]
fn player_files_are_decoded_off_the_ui_thread() {
    let env = hlsr_ui::env::ManualEnv::new();
    let f = fixture(Variant::Ghosting);
    let _worker =
        hlsr_ui::controllers::preview::with_worker(&f.controller, hlsr_ui::env::dispatch(&*env));
    // Кадры — снова тесту; разбор файлов остаётся у потока рисования.
    {
        let jobs = f.jobs.clone();
        f.controller
            .set_sender(move |job| jobs.borrow_mut().push(job));
    }
    f.controller.mount(false, None);
    f.deliver();
    f.controller.set_value(personalization(
        json!({"extra": {"gh_spray_file": "spray.png"}}),
    ));
    f.main.answer(
        "game-personalization-preview-asset",
        json!({"status": "ok", "dataUrl": "data:image/png;base64,AAAA"}),
    );
    // Ответ разбирается в фоне: в потоке интерфейса итога ещё нет.
    let decode = "#L_GS_X_OVERLAY_ASSET_DECODE";
    assert_ne!(f.controller.state().caption, decode);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while f.controller.state().caption != decode {
        assert!(
            std::time::Instant::now() < deadline,
            "итог разбора не пришёл"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
        env.run();
    }
}

#[test]
fn own_ghost_model_and_hud_mod_are_asked_from_main() {
    use hlsr_ui::controllers::preview::SceneLine;
    let f = fixture(Variant::Ghosting);
    f.controller.mount(false, None);
    f.deliver();
    f.controller.set_section("ghosts");
    f.deliver();
    f.controller.set_hud_mod(true);
    f.controller.set_value(personalization(
        json!({"extra": {"gh_model": "models/zombie.mdl"}}),
    ));
    let asked: Vec<Value> = f
        .main
        .log
        .borrow()
        .iter()
        .filter(|(c, _)| c == "game-personalization-preview-asset")
        .map(|(_, a)| a[1].clone())
        .collect();
    assert_eq!(
        asked.last().unwrap(),
        &json!({"kind": "model", "value": "models/zombie.mdl"})
    );
    assert!(asked.contains(&json!({"kind": "font", "value": "resource/din1451alt2.ttf"})));
    // Шрифт мода не нашёлся (первый запрос набора hud_mod) — без подписи.
    f.main.answer(
        "game-personalization-preview-asset",
        json!({"status": "missing"}),
    );
    f.main.answer(
        "game-personalization-preview-asset",
        json!({"status": "missing"}),
    );
    assert_eq!(f.controller.state().caption, "");
    f.main.answer(
        "game-personalization-preview-asset",
        json!({"status": "error", "code": "ASSET_NOT_FOUND"}),
    );
    let state = f.controller.state();
    assert_eq!(state.caption, "#L_GS_X_OVERLAY_ASSET_ERROR");
    assert_eq!(
        state.scene_lines,
        vec![
            SceneLine::Ghost(220),
            SceneLine::Key("#L_GS_X_SCENE_GHOST_MODEL")
        ]
    );
    // Модель прочиталась — силуэт уходит в кадр.
    f.controller.set_value(personalization(
        json!({"extra": {"gh_model": "models/box.mdl"}}),
    ));
    f.main.answer(
        "game-personalization-preview-asset",
        json!({"status": "missing"}),
    );
    f.main.answer(
        "game-personalization-preview-asset",
        json!({"status": "ok", "mesh": {"positions": [0, 0, 0, 8, 0, 0, 0, 0, 72]}}),
    );
    f.deliver();
    let job = f.jobs.borrow().last().cloned().unwrap();
    assert!(job.assets.ghost_model.is_some() && job.assets.hud_mod);
    assert_eq!(
        f.controller.state().scene_lines[1],
        SceneLine::Key("#L_GS_X_SCENE_GHOST_MODEL_POSE")
    );
}
