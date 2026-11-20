//! Режим сцены сверочного стенда (dev-hooks, `hlsr-snap screens`, V0): окно лаунчера на фикстурах
//! сцены вместо настоящего запуска — `FixtureServices` вместо фасадов, окно `1600×900 × зум`
//! варианта, шаги сцены и кадр ([`hlsr_gpui::snap`]), рядом с кадром — журнал вызовов окна и итог
//! шагов. Экземпляр, перенос и фасады не трогаются: прогоны идут параллельно и рядом с настоящим
//! лаунчером.
//!
//! Окружение (его ставит `hlsr-snap`): `HLSR_SNAP_SCENE` — id сцены, `HLSR_SNAP_VARIANT` — вариант
//! (JSON [`Variant`]), `HLSR_SNAP_OUT` — путь без расширения (`.png`, `.ipc-log.json`,
//! `.run.json`), `HLSR_SNAP_SCREENS` — каталог сцен (по умолчанию каталог 0.4.9 репозитория),
//! `HLSR_LIVE_DISPLAY` — монитор окна, как у живой проверки (по умолчанию основной).
//! `prefs` варианта пишутся в `launcher-prefs.json` каталога данных (`HLSR_DATA_DIR` — временный).

use gpui::{AnyView, AppContext, Bounds, Context, IntoElement, QuitMode, Render, Window, px, size};
use hlsr_core::js::truthy;
use hlsr_core::launcher_prefs::LauncherPrefs;
use hlsr_core::paths::Paths;
use hlsr_gpui::app::LauncherRoot;
use hlsr_gpui::shell::background::BackgroundProbe;
use hlsr_gpui::snap::{Outcome, Plan};
use hlsr_gpui::theme::Theme;
use hlsr_ui::fixtures::{FixtureServices, Scene, Variant, screens_049_dir};
use hlsr_ui::services::Services;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// После открытия окна: первый кадр и картинки, которые декодируются в фоне.
const OPEN_SETTLE: Duration = Duration::from_millis(1500);
/// После каждого шага: пружины и переходы 0.4.9 (до ~0,5 с).
const STEP_SETTLE: Duration = Duration::from_millis(600);
/// Перед снимком (плюс `settleMs` сцены).
const SHOT_SETTLE: Duration = Duration::from_millis(300);

/// Корень окна сцены: окно лаунчера или вид одного экрана.
struct Root(AnyView);

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.0.clone()
    }
}

/// Режим сцены: `None` — не он (нет `HLSR_SNAP_SCENE`), иначе код выхода.
pub fn run(paths: &Paths) -> Option<i32> {
    let id = std::env::var("HLSR_SNAP_SCENE").ok()?;
    Some(match run_scene(&id, paths) {
        Ok(()) => 0,
        Err(error) => {
            tracing::error!("[snap] {id}: {error}");
            eprintln!("hlsr-snap: {id}: {error}");
            1
        }
    })
}

fn env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("нет {name}"))
}

/// `<out>.<ext>`: в ключе варианта есть точка (`-z0.75-`), `with_extension` её съел бы.
fn beside(out: &Path, ext: &str) -> PathBuf {
    PathBuf::from(format!("{}.{ext}", out.display()))
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))
}

fn run_scene(id: &str, paths: &Paths) -> Result<(), String> {
    let dir = std::env::var("HLSR_SNAP_SCREENS").map_or_else(|_| screens_049_dir(), PathBuf::from);
    let scene = Scene::load(&dir, id)?;
    let variant: Variant = serde_json::from_str(&env("HLSR_SNAP_VARIANT")?)
        .map_err(|e| format!("HLSR_SNAP_VARIANT: {e}"))?;
    let out = PathBuf::from(env("HLSR_SNAP_OUT")?);
    // Без временного каталога prefs сцены перезаписали бы настоящий launcher-prefs.json.
    if std::env::var_os("HLSR_DATA_DIR").is_none_or(|dir| dir.is_empty()) {
        return Err("нет HLSR_DATA_DIR: prefs сцены пишутся только во временный каталог".into());
    }
    if variant.scale != 1.0 {
        // shortcut: deviceScaleFactor эталона окну GPUI не задать; сцены пока на scales [1].
        return Err(format!(
            "масштаб варианта {} не поддержан: только 1",
            variant.scale
        ));
    }
    // Вид одного экрана (поле `view`): у эталона остальное спрятано полем `css` сцены.
    if let Some(view) = scene.view.as_deref().filter(|view| *view != "background") {
        return Err(format!(
            "вид «{view}» не поддержан: окно лаунчера или background"
        ));
    }
    let probe = scene
        .view
        .is_some()
        .then(|| (scene.clone(), scene.prefs_of(&variant)));
    let mut prefs = LauncherPrefs::open(paths);
    if !prefs.save(Value::Object(scene.prefs_of(&variant))) {
        return Err("launcher-prefs.json не записан".into());
    }
    // `localStorage.wasLaunched` эталона — в GPUI ключ prefs (заставка: «С возвращением»).
    let was_launched = scene.local_storage.get("wasLaunched");
    prefs.set("wasLaunched", Value::Bool(was_launched.is_some_and(truthy)));
    // Маршрут `#/` — заставка, остальные — сразу оболочка.
    let boot = scene.route == "/";
    // `scene.parallax` (как `snapCss` эталона): без него фон не следит за курсором.
    let still_parallax = !scene.parallax;
    let theme_id = prefs.theme_id();
    let accent = prefs
        .get("paperAccent")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let fixtures = Arc::new(FixtureServices::new(&scene, &variant)?);
    // Картинки `https` сцены — файлы каталога (аватары, скриншоты новостей).
    let images = hlsr_ui::images::FixtureImages::new(scene.https_files(&dir));
    let services: Services = fixtures.clone();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("hlsr-snap")
        .enable_all()
        .build()
        .map_err(|e| format!("рантайм tokio: {e}"))?;
    let (width, height) = scene.window_of(&variant);
    let zoom = scene.zoom_of(&variant);
    let plan = Plan {
        steps: scene.steps.clone(),
        open_settle: OPEN_SETTLE,
        step_settle: STEP_SETTLE,
        shot_settle: SHOT_SETTLE + Duration::from_millis(scene.settle_ms),
        png: beside(&out, "png"),
        emit: {
            let fixtures = fixtures.clone();
            Box::new(move |channel, payload| fixtures.emit(channel, payload))
        },
    };
    let app_paths = paths.clone();
    let result: Rc<RefCell<Option<Result<Outcome, String>>>> = Rc::default();
    let finished = result.clone();
    let handle = runtime.handle().clone();
    let app = gpui::Application::with_platform(hlsr_gpui::app::platform()?)
        .with_assets(hlsr_gpui::assets::Assets)
        .with_quit_mode(QuitMode::Explicit);
    let _runtime = runtime.enter();
    app.run(move |cx| {
        hlsr_gpui::init(cx);
        hlsr_gpui::bridge::init(cx, handle);
        cx.global_mut::<Theme>()
            .set(&theme_id, accent.as_deref(), false, Instant::now());
        cx.set_global(hlsr_gpui::shell::root::AppPaths(app_paths));
        if still_parallax {
            cx.set_global(hlsr_gpui::shell::background::StillParallax);
        }
        let env = hlsr_gpui::env::GpuiEnv::new(cx);
        cx.set_global(hlsr_gpui::image::Pictures::new(
            hlsr_ui::images::ImageLoader::new(Rc::new(images), &*env),
        ));
        // На мониторе `HLSR_LIVE_DISPLAY`: замеры частоты кадров по монитору окна.
        let display = crate::window::live_display(cx).map(|(id, _)| id);
        let bounds = Bounds::centered(display, size(px(width as f32), px(height as f32)), cx);
        let options = gpui::WindowOptions {
            display_id: display,
            ..hlsr_gpui::window::launcher_options(bounds)
        };
        let opened = cx.open_window(options, |window, cx| {
            window.set_rem_size(hlsr_gpui::scale::rem_size(zoom));
            let root: AnyView = match &probe {
                Some((scene, prefs)) => cx
                    .new(|cx| BackgroundProbe::new(&services, scene, prefs, cx))
                    .into(),
                None if boot => cx.new(|cx| LauncherRoot::new(services, cx)).into(),
                None => cx.new(|cx| LauncherRoot::launcher(services, cx)).into(),
            };
            cx.new(|_| Root(root))
        });
        let window = match opened {
            Ok(window) => window,
            Err(error) => {
                *finished.borrow_mut() = Some(Err(format!("окно не открылось: {error:#}")));
                cx.quit();
                return;
            }
        };
        let done = finished.clone();
        let started = window.update(cx, move |_, window, cx| {
            hlsr_gpui::snap::run(plan, window, cx, move |outcome, cx| {
                *done.borrow_mut() = Some(Ok(outcome));
                cx.quit();
            });
        });
        if let Err(error) = started {
            *finished.borrow_mut() = Some(Err(format!("окно закрылось: {error:#}")));
            cx.quit();
        }
    });
    // Итог пишется и при сбое: причина — в run.json, а не только в stderr.
    let outcome = match result.take() {
        Some(Ok(outcome)) => outcome,
        failed => Outcome {
            error: Some(
                failed
                    .and_then(Result::err)
                    .unwrap_or_else(|| "окно закрылось до снимка".into()),
            ),
            ..Outcome::default()
        },
    };
    write_json(&beside(&out, "ipc-log.json"), &Value::Array(fixtures.log()))?;
    let frames = hlsr_gpui::live::frame_times();
    write_json(
        &beside(&out, "run.json"),
        &json!({
            "window": [width, height],
            "zoom": zoom,
            "steps": outcome.steps,
            "problems": fixtures.problems(),
            "error": outcome.error,
            // Сколько кадров окно нарисовало за прогон и начала последних (мс до конца прогона):
            // замеры частоты кадров и их ровности.
            "frames": frames["frame"],
            "frameAgo": frames["ago"],
        }),
    )?;
    match outcome.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
