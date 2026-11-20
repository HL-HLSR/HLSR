//! Страница «Демо» на фикстурах: окно говорит с главным процессом как 0.4.9 — все 22 канала
//! страницы (`demo-list` … `demo-bxtrs-cancel`), события заданий и бросание файлов.

use crate::app::LauncherRoot;
use crate::shell::root::ShellContext;
use gpui::{ExternalPaths, FileDropEvent, Modifiers, TestAppContext, VisualTestContext};
use hlsr_ui::fixtures::{FixtureServices, Scene, Variant};
use hlsr_ui::services::Services;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub(super) fn fixtures(invoke: Value) -> Arc<FixtureServices> {
    let scene: Scene = serde_json::from_value(json!({
        "id": "t", "route": "/launcher", "platform": "win32",
        "window": {"width": 1600, "height": 900},
        "themes": ["hlsr"], "languages": ["ru"], "scales": [1],
        "now": "2026-11-17T12:00:00.000Z", "timezone": "UTC",
        "store": {"settings": {"config": {"language": 1}}},
        "invoke": invoke
    }))
    .unwrap();
    let variant = Variant {
        theme: "hlsr".into(),
        lang: "ru".into(),
        scale: 1.0,
        zoom: None,
        rail: None,
    };
    Arc::new(FixtureServices::new(&scene, &variant).unwrap())
}

pub(super) fn calls(fx: &FixtureServices, channel: &str) -> Vec<Value> {
    fx.log()
        .into_iter()
        .filter(|entry| entry["kind"] == "invoke" && entry["channel"] == channel)
        .map(|entry| entry["args"].clone())
        .collect()
}

pub(super) fn settle(cx: &mut VisualTestContext, frames: usize) {
    for _ in 0..frames {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }
}

pub(super) fn click(cx: &mut VisualTestContext, name: &str) {
    let at = crate::snap::bounds_of(name).unwrap_or_else(|| panic!("нет цели {name}"));
    cx.simulate_click(at.center(), Modifiers::none());
    settle(cx, 4);
}

pub(super) fn section(cx: &mut VisualTestContext, demo: bool) {
    cx.update(|_, cx| {
        let nav = &ShellContext::get(cx).expect("оболочка").nav;
        if demo {
            nav.select_demo();
        } else {
            nav.select_section("home");
        }
    });
    settle(cx, 10);
}

fn item(id: &str, extra: Value) -> Value {
    let mut item = json!({"id": id, "name": format!("{id}.dem"), "source": "game",
        "gameId": "ghosting-renewed", "game": "ghosting", "kind": "demo", "map": "d1_trainstation_01",
        "duration": 144.46, "segmentCount": 1, "mtimeMs": 1_763_380_800_000_i64, "size": 3_145_728,
        "supported": true});
    for (key, value) in extra.as_object().unwrap() {
        item[key] = value.clone();
    }
    item
}

pub(super) fn invoke() -> Value {
    let gh = item("gh", json!({}));
    let set = item(
        "set",
        json!({"source": "manual", "gameId": null, "game": "hl1", "kind": "segments",
            "segmentCount": 2, "map": "c1a0"}),
    );
    let detail = |item: &Value, render: Value, segments: Value| {
        json!({"status": "ok", "item": item, "segments": segments,
            "markers": [{"t": 25.85, "type": "map", "label": "d1_trainstation_02"}], "render": render})
    };
    let ready = json!({"ok": true, "code": null,
        "view": {"keyhud": false, "hud": true, "ownConfig": true, "ownHud": true}});
    let segments = json!([{"name": "a.dem", "duration": 60}, {"name": "b.dem", "duration": 84.46}]);
    let swapped = json!([{"name": "b.dem", "duration": 84.46}, {"name": "a.dem", "duration": 60}]);
    let bxt = json!({"ok": false, "code": "BXTRS_MISSING"});
    json!({
        "demo-list": {"status": "ok", "items": [gh, set]},
        "demo-inspect": {"$args": [
            [["gh"], detail(&gh, ready.clone(), json!([{"name": "gh.dem", "duration": 144.46}]))],
            [["set"], detail(&set, bxt.clone(), segments)]
        ]},
        "demo-ffmpeg-status": {"$seq": [
            {"status": "ok", "ffmpeg": {"state": "ready", "installable": true}},
            {"status": "ok", "ffmpeg": {"state": "not-installed", "installable": true,
                "pinnedVersion": "9.0.2", "sizeBytes": 104857600}}
        ]},
        "demo-bxtrs-status": {"status": "ok", "bxtrs": {"state": "missing", "installable": true,
            "pinnedVersion": "0.2", "sizeBytes": 5242880, "sourceLink": "bxtrs-source",
            "licenseLink": "bxtrs-license"}},
        "demo-settings-get": {"status": "ok", "preview": {"keyhud": false}},
        "demo-rendered-list": {"status": "ok", "outputDir": "C:\\Videos\\HLSR", "items": [
            {"name": "gh.mp4", "size": 1048576, "mtimeMs": 1_763_380_800_000_i64,
             "video": {"duration": 20, "width": 1920, "height": 1080, "fps": 60, "speed": 1},
             "source": {"game": "ghosting", "map": "d1_trainstation_01"}, "markerCount": 1}]},
        "demo-estimate": {"status": "ok", "threshold": 60, "items": [{"seconds": 12}]},
        "demo-preview-start": {"status": "ok", "jobId": "p1"},
        "demo-cancel": {"status": "ok", "cancelled": true},
        "demo-pick-files": {"status": "cancelled"},
        "demo-remove": {"status": "ok", "items": [gh]},
        "demo-reorder": detail(&set, bxt, swapped),
        "demo-add-paths": {"status": "ok", "added": [],
            "rejected": [{"name": "x.dem", "code": "DEMO_NOT_A_DEMO"}], "items": [gh, set]},
        "demo-ffmpeg-install": {"$pending": true},
        "demo-ffmpeg-cancel": {"status": "ok", "cancelled": true},
        "demo-bxtrs-install": {"$pending": true},
        "demo-bxtrs-cancel": {"status": "ok", "cancelled": true},
        "demo-rendered-open": {"status": "ok", "name": "gh.mp4", "url": "hlsr-media://r/1",
            "video": {"duration": 20, "width": 1920, "height": 1080, "fps": 60, "speed": 1,
                      "range": {"from": 0, "to": 20}},
            "source": {"game": "ghosting", "map": "d1_trainstation_01"},
            "markers": [{"t": 0, "type": "map", "label": "d1_trainstation_01", "sourceTime": 0}],
            "sourceId": "gh"},
        "demo-rendered-reveal": {"status": "ok"},
        "demo-rendered-delete": {"status": "ok", "deleted": "C:\\Videos\\HLSR\\gh.mp4",
            "outputDir": "C:\\Videos\\HLSR", "items": []},
        "demo-preview-clear": {"status": "ok", "cleared": false},
        "demo-export-start": {"status": "ok", "jobId": "e1"},
        "open-external-link": {"status": "ok"},
        "discord-rpc-activity": null
    })
}

/// Страница в окне (её создала оболочка при входе в раздел).
pub(super) fn panel() -> gpui::Entity<super::panel::DemoPanel> {
    super::panel::LAST
        .with(|last| last.borrow().clone())
        .and_then(|weak| weak.upgrade())
        .expect("страница «Демо»")
}

#[gpui::test]
fn the_demo_page_talks_to_the_main_process_like_0_4_9(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let fx = fixtures(invoke());
    let services: Services = fx.clone();
    let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
    cx.run_until_parked();
    section(cx, true);

    // activated(): списки и статусы; выбор — первая поддерживаемая (Ghosting раньше набора).
    for channel in [
        "demo-list",
        "demo-ffmpeg-status",
        "demo-bxtrs-status",
        "demo-settings-get",
        "demo-rendered-list",
    ] {
        assert_eq!(calls(&fx, channel), [json!([])], "{channel}");
    }
    assert_eq!(calls(&fx, "demo-inspect"), [json!(["gh"])]);

    // «Играть»: оценка, затем предпросмотр с видом из demo-settings-get.
    click(cx, "dmp-play");
    assert_eq!(
        calls(&fx, "demo-estimate"),
        [json!(["gh", [{"preset": "preview", "range": null}]])]
    );
    assert_eq!(
        calls(&fx, "demo-preview-start"),
        [
            json!(["gh", {"range": null, "keyhud": false, "hud": true, "ownConfig": true, "ownHud": true}])
        ]
    );
    fx.emit(
        "demo-job-progress",
        json!({"jobId": "p1", "kind": "preview", "phase": "render", "percent": 40}),
    );
    settle(cx, 4);
    click(cx, "dmp-preview-cancel");
    assert_eq!(calls(&fx, "demo-cancel"), [json!(["p1"])]);
    fx.emit(
        "demo-job-error",
        json!({"jobId": "p1", "kind": "preview", "error": {"code": "CANCELLED"}}),
    );
    settle(cx, 4);
    let cancelled = panel().read_with(cx, |panel, _| panel.state().preview_cancelled());
    assert!(cancelled, "«Предпросмотр отменён» на экране");

    // Экспорт (окно G13 зовёт метод страницы).
    let page = panel();
    page.update(cx, |panel, cx| {
        panel.start_export(json!({"preset": "discord"}), cx)
    });
    settle(cx, 4);
    assert_eq!(
        calls(&fx, "demo-export-start"),
        [json!(["gh", {"preset": "discord"}])]
    );
    fx.emit(
        "demo-job-done",
        json!({"jobId": "e1", "kind": "export", "result": {"file": "C:\\Videos\\HLSR\\gh2.mp4", "fileName": "gh2.mp4"}}),
    );
    settle(cx, 4);
    assert_eq!(
        calls(&fx, "demo-rendered-list").len(),
        2,
        "новое видео — в «Отрендеренных»"
    );

    // Набор HL1: рендер без bxt-rs — карточка bxt-rs, «Скачать» и «Отмена»; порядок и «Убрать».
    click(cx, "dmp-item-set");
    assert_eq!(calls(&fx, "demo-inspect").last(), Some(&json!(["set"])));
    click(cx, "dff-install");
    assert_eq!(calls(&fx, "demo-bxtrs-install"), [json!([])]);
    click(cx, "dff-cancel");
    assert_eq!(calls(&fx, "demo-bxtrs-cancel"), [json!([])]);
    click(cx, "dmp-move-0-1");
    assert_eq!(calls(&fx, "demo-reorder"), [json!(["set", [1, 0]])]);
    click(cx, "dmp-remove");
    assert_eq!(calls(&fx, "demo-remove"), [json!(["set"])]);
    assert_eq!(calls(&fx, "demo-inspect").last(), Some(&json!(["gh"])));

    // «Добавить» и бросание файлов: демки — в канал, чужое — в строку итога.
    click(cx, "dmp-add");
    assert_eq!(calls(&fx, "demo-pick-files"), [json!([])]);
    let paths = ExternalPaths(
        [PathBuf::from(r"C:\d\x.dem"), PathBuf::from(r"C:\d\a.txt")]
            .into_iter()
            .collect(),
    );
    // Над списком страницы.
    let at = crate::snap::bounds_of("dmp-item-gh")
        .expect("запись")
        .center();
    cx.simulate_event(FileDropEvent::Entered {
        position: at,
        paths,
    });
    settle(cx, 2);
    cx.simulate_event(FileDropEvent::Submit { position: at });
    cx.simulate_event(FileDropEvent::Ended);
    settle(cx, 4);
    assert_eq!(calls(&fx, "demo-add-paths"), [json!([[r"C:\d\x.dem"]])]);
    let notice = panel().read_with(cx, |panel, _| panel.state().notice.clone());
    let notice = notice.expect("итог добавления");
    assert!(notice.error);
    assert_eq!(notice.rejected.len(), 2, "{notice:?}");

    // «Отрендеренные»: открыть, показать в папке, удалить с вопросом.
    click(cx, "dmp-tab-rendered");
    assert_eq!(calls(&fx, "demo-rendered-list").len(), 3);
    click(cx, "dmp-video-0");
    assert_eq!(calls(&fx, "demo-rendered-open"), [json!(["gh.mp4"])]);
    click(cx, "dmp-rendered-reveal");
    assert_eq!(calls(&fx, "demo-rendered-reveal"), [json!(["gh.mp4"])]);
    click(cx, "dmp-rendered-delete");
    settle(cx, 20);
    click(cx, "confirm-accept");
    settle(cx, 10);
    assert_eq!(calls(&fx, "demo-rendered-delete"), [json!(["gh.mp4"])]);

    // Уход со страницы гасит предпросмотр; возврат — статусы заново (FFmpeg пропал).
    section(cx, false);
    assert_eq!(calls(&fx, "demo-preview-clear"), [json!([])]);
    section(cx, true);
    assert_eq!(calls(&fx, "demo-ffmpeg-status").len(), 2);
    click(cx, "dmp-tab-demos");
    click(cx, "dff-install");
    assert_eq!(calls(&fx, "demo-ffmpeg-install"), [json!([])]);
    click(cx, "dff-cancel");
    assert_eq!(calls(&fx, "demo-ffmpeg-cancel"), [json!([])]);

    // Все 22 канала страницы вызваны.
    let channels: std::collections::BTreeSet<String> = fx
        .log()
        .into_iter()
        .filter(|entry| entry["kind"] == "invoke")
        .filter_map(|entry| entry["channel"].as_str().map(str::to_owned))
        .filter(|channel| channel.starts_with("demo-"))
        .collect();
    assert_eq!(channels.len(), 22, "{channels:?}");
}

/// Отмеченное инспектором в кадре: `вид:id` и подписи (цели стенда — именем контрола).
fn dump(cx: &mut VisualTestContext) -> Vec<String> {
    // Без dev-hooks кадр таблицы инспектора начинает тест, а не корень окна.
    cx.update(|window, _| {
        crate::live::begin_frame();
        window.refresh();
    });
    cx.run_until_parked();
    let dump = cx.update(|window, cx| crate::live::snapshot(window, cx));
    dump["controls"]
        .as_array()
        .expect("список")
        .iter()
        .flat_map(|c| {
            let kind = c["kind"].as_str().unwrap_or_default().to_owned();
            let mut names = vec![format!("{kind}:{}", c["id"].as_str().unwrap_or_default())];
            names.extend(
                c["target"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|t| format!("{kind}:{t}")),
            );
            names
        })
        .collect()
}

/// Щелчок по середине отмеченного контрола `id` (не цели стенда).
fn click_control(cx: &mut VisualTestContext, id: &str) {
    let dump = cx.update(|window, cx| crate::live::snapshot(window, cx));
    let scale = dump["scale"].as_f64().expect("масштаб") as f32;
    let item = dump["controls"]
        .as_array()
        .expect("список")
        .iter()
        .find(|item| item["id"] == id)
        .unwrap_or_else(|| panic!("нет контрола {id}"))
        .clone();
    let [x, y, w, h] = [0, 1, 2, 3].map(|i| item["rect"][i].as_f64().unwrap() as f32 / scale);
    let at = gpui::point(
        crate::scale::zpx(1., x + w / 2.),
        crate::scale::zpx(1., y + h / 2.),
    );
    cx.simulate_click(at, Modifiers::none());
    settle(cx, 4);
}

fn assert_marked(cx: &mut VisualTestContext, wanted: &[&str]) {
    let names = dump(cx);
    for want in wanted {
        assert!(names.iter().any(|n| n == want), "нет {want} в {names:?}");
    }
}

/// Инспектор видит интерактив «Демо»: ссылки, переключатели вида, кнопки плеера, ползунок
/// громкости, пункты скорости, «Отмена» подтверждения.
#[gpui::test]
fn the_inspector_sees_every_demo_control(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let fx = fixtures(invoke());
    let services: Services = fx.clone();
    let (_, cx) = cx.add_window_view(|_, cx| LauncherRoot::launcher(services, cx));
    cx.run_until_parked();
    section(cx, true);

    assert_marked(cx, &["link:dvs-more-btn", "button:dmp-export"]);
    click(cx, "dvs-more");
    assert_marked(
        cx,
        &[
            "toggle:dvs-keyhud",
            "toggle:dvs-hud",
            "toggle:dvs-ownConfig",
            "toggle:dvs-ownHud",
        ],
    );
    click(cx, "dvs-more");
    panel().update(cx, |panel, cx| {
        panel.set_range(
            Some(hlsr_ui::controllers::demo_tool::ClipRange { from: 1., to: 3. }),
            cx,
        )
    });
    settle(cx, 4);
    assert_marked(cx, &["link:dmp-range-reset"]);

    click(cx, "dmp-tab-rendered");
    click(cx, "dmp-video-0");
    assert_marked(
        cx,
        &[
            "button:dpl-rate",
            "button:dpl-mute",
            "slider:dpl-volume",
            "button:dmp-rendered-reveal",
        ],
    );
    click_control(cx, "dpl-rate");
    assert_marked(cx, &["option:dpl-rate-2"]);
    click(cx, "dmp-rendered-delete");
    settle(cx, 20);
    assert_marked(cx, &["button:confirm-cancel", "button:confirm-accept"]);
}
