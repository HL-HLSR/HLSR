//! Превью персонализации (`GhostingSettingsPreview.vue` 0.4.9, задачи 33, 34 и G9): кадр
//! Ghosting или HL1 с HUD, прицелом и оверлеем LiveSplit, подписи под ним, окно во весь
//! лаунчер, «движение», ширина кадра по разрешению игры (`ghosting-screen-get`) и файлы
//! игрока для шрифтов HUD и спрея (`game-personalization-preview-asset`). С 0.4.9 у разделов
//! «Призраки и запись» и «Эффекты и спреи» своя сцена (призрак на пляже в портретном окне,
//! спрей на стене), своя модель призрака (`kind: model`) и HUD-мод Condensed HUD.
//!
//! Живёт в потоке интерфейса; кадры рисует [`RenderWorker`] в своём потоке (растр на ЦП,
//! решение 19), последний запрос вытесняет прежние. Вызовы main — через [`Invoke`]
//! (в приложении — `DeliveryApi::call`, в тестах и снимках — фикстуры с журналом).

use crate::render::ghost_scene::{self, Kind as SceneKind, SceneRenderer, SprayInfo};
use crate::render::ghosting_overlay::{Assets, OverlayRenderer};
use crate::render::hl1_overlay::Hl1Renderer;
use crate::render::{canvas_text, preview, raster};
use hlsr_core::js;
use hlsr_delivery::livesplit_overlay::{self, Overlay};
use hlsr_platform::livesplit_overlay_shared::{self as shared, KeyedImage, PixelOrder};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Instant;
/// Холст кадра (tiny-skia): вид получает его в `on_frame`, не завися от tiny-skia сам.
pub use tiny_skia::Pixmap;

/// Ответ канала main.
pub type Reply = Box<dyn FnOnce(Value)>;
/// `ipcRenderer.invoke(channel, ...args)`: ответ приходит в потоке интерфейса.
pub type Invoke = Rc<dyn Fn(&'static str, Vec<Value>, Reply)>;

/// Какие кадры: Ghosting (HL2) или Half-Life WON + BXT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Ghosting,
    Hl1,
}

/// Разрешение игры (`ghosting-screen-get`) или монитора (`source: "display"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub width: u32,
    pub height: u32,
    pub source: String,
}

/// Снимок окна LiveSplit (`livesplit-overlay-capture`): B, G, R, X сверху вниз.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub pixels: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

/// Что показывает превью (для глобала `GamePreview`). Строки — ключи локализации:
/// разметка переводит их сама (смена языка не ждёт нового кадра).
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub frame_width: u32,
    pub ready: bool,
    pub loading: bool,
    /// `statusText`: ключ (загрузка или ошибка кадра), пусто — нет.
    pub message: &'static str,
    /// `caption`: ключ подписи, пусто — нет.
    pub caption: &'static str,
    pub caption_error: bool,
    /// `screenText`: ключ `#L_GS_X_PREVIEW_SCREEN_*` и размер `W×H`.
    pub screen: Option<(String, String)>,
    /// «Движение» есть (`canAnimate`).
    pub motion: bool,
    pub animating: bool,
    /// Ручка оверлея (`overlayHandle`): прямоугольник в пикселях кадра.
    pub handle: Option<crate::controllers::preview_view::Handle>,
    pub expanded: bool,
    /// Сцена раздела вместо кадра с HUD (`scene`).
    pub scene: Option<SceneKind>,
    /// Портретное окно (`portrait`): кадр занимает всю высоту, ширина — от окна.
    pub portrait: bool,
    /// Строки сцены под кадром (`sceneLines`).
    pub scene_lines: Vec<SceneLine>,
}

/// Строка сцены под кадром (`sceneLines` 0.4.9).
#[derive(Debug, Clone, PartialEq)]
pub enum SceneLine {
    /// `#L_GS_X_SCENE_GHOST`: расстояние до призрака, юниты.
    Ghost(i64),
    /// Строка без чисел (ключ локализации).
    Key(&'static str),
    /// `#L_GS_X_SCENE_SPRAY_SIZE`: сторона на стене (юниты, `toFixed(2)`) и квадрат, px.
    SpraySize { units: f64, texels: u32 },
}

/// Своя модель призрака (`ghostModelState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelState {
    /// Стандартная `models/player.mdl` (готовый `ghost.webp`).
    Stock,
    Loading,
    Shown,
    /// Нет в файлах игры.
    Missing,
    /// Не прочиталась.
    Failed,
}

/// Готовый кадр и подпись спрея (у сцены «Эффекты и спреи»).
pub struct Frame {
    pub pixmap: Pixmap,
    pub spray: Option<SprayInfo>,
}

impl From<Pixmap> for Frame {
    fn from(pixmap: Pixmap) -> Frame {
        Frame {
            pixmap,
            spray: None,
        }
    }
}

/// Запрос кадра для потока рисования.
#[derive(Clone)]
pub struct Job {
    pub serial: u64,
    pub variant: Variant,
    pub width: u32,
    pub value: Value,
    pub section: String,
    pub assets: Assets,
    pub time: f64,
    pub overlay: Option<(Overlay, Arc<Pixmap>, shared::Placement)>,
    /// Сцена раздела (`scene`): рисует [`SceneRenderer`], оверлея на ней нет.
    pub scene: Option<SceneKind>,
}

/// Рисование кадра (в потоке рисования).
#[derive(Default)]
pub struct Renderers {
    hl1: Hl1Renderer,
    ghosting: OverlayRenderer,
    scenes: SceneRenderer,
}

impl Renderers {
    pub fn render(&mut self, job: &Job) -> Result<Frame, raster::RasterError> {
        if let Some(kind) = job.scene {
            let (pixmap, spray) = self
                .scenes
                .render(job.width, &job.value, kind, &job.assets)?;
            return Ok(Frame { pixmap, spray });
        }
        let mut frame = match job.variant {
            Variant::Hl1 => self.hl1.render(&job.value)?,
            Variant::Ghosting => {
                self.ghosting
                    .render(job.width, &job.value, &job.section, &job.assets, job.time)?
            }
        };
        if let Some((overlay, image, rect)) = &job.overlay {
            preview::draw_overlay(&mut frame, overlay, image, *rect);
        }
        Ok(frame.into())
    }
}

/// Наибольшая частота кадров «движения» (оружие и клавиши Ghosting).
pub const MOTION_FPS: u32 = 30;

/// Готовый кадр или ошибка.
pub type Rendered = (u64, Result<Frame, String>);

/// Потоки рисования: кадр с HUD и сцена раздела — каждый в своём (как `layerRenderer` и
/// `sceneRenderer` 0.4.9): долгий кадр HUD не держит сцену раздела, куда перешли. Поток берёт
/// последний запрос (прежние вытесняются), рисует, отдаёт кадр в `done` (в его потоке —
/// вызывающий передаёт в поток интерфейса). Уничтожение закрывает очереди и не ждёт потоки:
/// начатый кадр (2133 px, первый декод) дорисуется и уйдёт в никуда, а окно не замрёт на уходе
/// с экрана.
pub struct RenderWorker {
    hud: Option<mpsc::Sender<Job>>,
    scene: Option<mpsc::Sender<Job>>,
}

impl RenderWorker {
    pub fn spawn(done: impl Fn(Rendered) + Send + Sync + 'static) -> RenderWorker {
        let done = Arc::new(done);
        RenderWorker {
            hud: Self::thread("hlsr-preview", done.clone()),
            scene: Self::thread("hlsr-preview-scene", done),
        }
    }

    fn thread(
        name: &str,
        done: Arc<impl Fn(Rendered) + Send + Sync + 'static>,
    ) -> Option<mpsc::Sender<Job>> {
        let (tx, rx) = mpsc::channel::<Job>();
        let spawned = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let mut renderers = Renderers::default();
                while let Ok(mut job) = rx.recv() {
                    while let Ok(newer) = rx.try_recv() {
                        job = newer;
                    }
                    let started = Instant::now();
                    let result = renderers.render(&job).map_err(|e| e.to_string());
                    // «Движение» (кадр со временем) — не чаще MOTION_FPS: rAF 0.4.9 шёл бы
                    // с частотой монитора и занял бы поток рисования целиком.
                    if job.time > 0.0 {
                        let frame = std::time::Duration::from_secs(1) / MOTION_FPS;
                        std::thread::sleep(frame.saturating_sub(started.elapsed()));
                    }
                    done((job.serial, result));
                }
            });
        match spawned {
            Ok(_) => Some(tx),
            Err(error) => {
                tracing::warn!("[preview] поток рисования {name} не запущен: {error}");
                None
            }
        }
    }

    pub fn send(&self, job: Job) {
        (self.sender())(job);
    }

    /// Отправка запросов (для [`PreviewController::set_sender`]): сцена — в свой поток.
    pub fn sender(&self) -> impl Fn(Job) + 'static {
        let (hud, scene) = (self.hud.clone(), self.scene.clone());
        move |job| {
            let tx = if job.scene.is_some() { &scene } else { &hud };
            if let Some(tx) = tx {
                let _ = tx.send(job);
            }
        }
    }
}

/// Кадры сняты в 16:9 при высоте 900: другое соотношение меняет только ширину.
pub const FRAME_HEIGHT: u32 = preview::FRAME_HEIGHT;

/// Файл игрока по пути (а не имя системного шрифта): его нет — «файл недоступен».
fn looks_like_file(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    value.contains(['\\', '/'])
        || [".ttf", ".otf", ".png", ".bmp", ".tga"]
            .iter()
            .any(|ext| lower.ends_with(ext))
}

/// Шрифт цифр HUD-мода в его папке `custom` (читается тем же путём, что шрифт игрока).
const HUD_MOD_FONT: &str = "resource/din1451alt2.ttf";

const ASSET_KEYS: [&str; 4] = [
    "gh_hud_font",
    "gh_showfps_font",
    "gh_showpos_font",
    "gh_spray_file",
];

struct Inner {
    variant: Variant,
    game_id: String,
    section: String,
    value: Value,
    active: bool,
    overlay: Option<Value>,
    /// Общий с видом снимок: байты окна LiveSplit не копируются.
    snapshot: Option<Rc<Snapshot>>,
    overlay_editing: bool,
    screen: Option<Screen>,
    display: Option<(u32, u32)>,
    ready: bool,
    error: bool,
    asset_error: Option<&'static str>,
    animate: bool,
    reduce_motion: bool,
    expanded: bool,
    asset_signature: String,
    asset_revision: u64,
    assets: Assets,
    serial: u64,
    /// Запрошен ли кадр, который ещё не пришёл (`frameRequest`).
    pending: bool,
    /// Запрос пришёл, пока ждали прежний: нарисовать ещё раз.
    again: bool,
    keyed: Option<(String, KeyedImage, Arc<Pixmap>)>,
    /// HUD-мод Condensed HUD включён (`hudMod`).
    hud_mod: bool,
    scene_ready: bool,
    scene_error: bool,
    /// Сцена запрошенного кадра (кадр в работе один).
    pending_scene: Option<SceneKind>,
    /// Размер окна вида (`stageBox`, событие `box`): по нему ширина портретного кадра.
    stage_box: Option<(f64, f64)>,
    spray_info: Option<SprayInfo>,
    model_state: ModelState,
}

/// Подписчик события контроллера (один).
type Handler<F> = RefCell<Option<Box<F>>>;

/// Превью персонализации одной игры.
pub struct PreviewController {
    inner: RefCell<Inner>,
    invoke: Invoke,
    started: Instant,
    /// Отправить кадр в поток рисования.
    send: RefCell<Box<dyn Fn(Job)>>,
    /// Состояние изменилось.
    on_state: Handler<dyn Fn(&State)>,
    /// Пришёл кадр и его сцена (отдаётся владельцу: копия 5,8 МБ на кадр не нужна).
    on_frame: Handler<dyn Fn(Pixmap, Option<SceneKind>)>,
    /// `emit('overlay', value)`: оверлей перетащили.
    on_overlay: Handler<dyn Fn(Value)>,
    weak: RefCell<std::rc::Weak<PreviewController>>,
    /// Номер контроллера у потока рисования и возврат в поток интерфейса ([`with_worker`]):
    /// файлы игрока разбираются в фоне. Без него (тесты) — на месте.
    offload: RefCell<Option<(u64, crate::env::Dispatch)>>,
}

/// Разобранный файл игрока: шрифт (семейство, байты) или спрей.
type Decoded = Result<(String, Vec<u8>), Arc<Pixmap>>;

/// Файл игрока из `dataUrl` ответа main (вне потока интерфейса: спрей до 16 МБ).
fn decode_asset(key: &str, path: &str, url: &str) -> Option<Decoded> {
    let bytes = crate::images::data_uri_bytes(url).ok()?;
    if key == "gh_spray_file" {
        raster::decode(path, &bytes).ok().map(|p| Err(Arc::new(p)))
    } else {
        canvas_text::family_of(&bytes).map(|family| Ok((family, bytes)))
    }
}

impl PreviewController {
    /// Превью `variant` игры `game_id`. Кадры уходят в [`PreviewController::set_sender`]
    /// (поток рисования, [`with_worker`]); готовый кадр возвращается в
    /// [`PreviewController::frame_ready`].
    pub fn new(variant: Variant, game_id: &str, invoke: Invoke) -> Rc<PreviewController> {
        let section = if variant == Variant::Hl1 {
            "personalization"
        } else {
            "hud"
        };
        let controller = Rc::new(PreviewController {
            inner: RefCell::new(Inner {
                variant,
                game_id: game_id.to_owned(),
                section: section.to_owned(),
                value: hlsr_delivery::ghosting_personalization::default_personalization()
                    .to_value(),
                active: true,
                overlay: None,
                snapshot: None,
                overlay_editing: false,
                screen: None,
                display: None,
                ready: false,
                error: false,
                asset_error: None,
                animate: false,
                reduce_motion: false,
                expanded: false,
                asset_signature: String::new(),
                asset_revision: 0,
                assets: Assets::default(),
                serial: 0,
                pending: false,
                again: false,
                keyed: None,
                hud_mod: false,
                scene_ready: false,
                scene_error: false,
                pending_scene: None,
                stage_box: None,
                spray_info: None,
                model_state: ModelState::Stock,
            }),
            invoke,
            started: Instant::now(),
            send: RefCell::new(Box::new(|_| {})),
            on_state: RefCell::new(None),
            on_frame: RefCell::new(None),
            on_overlay: RefCell::new(None),
            weak: RefCell::new(std::rc::Weak::new()),
            offload: RefCell::new(None),
        });
        *controller.weak.borrow_mut() = Rc::downgrade(&controller);
        controller
    }

    /// Куда отправлять запросы кадров.
    pub fn set_sender(&self, send: impl Fn(Job) + 'static) {
        *self.send.borrow_mut() = Box::new(send);
    }

    pub fn on_state(&self, f: impl Fn(&State) + 'static) {
        *self.on_state.borrow_mut() = Some(Box::new(f));
    }

    pub fn on_frame(&self, f: impl Fn(Pixmap, Option<SceneKind>) + 'static) {
        *self.on_frame.borrow_mut() = Some(Box::new(f));
    }

    pub fn on_overlay(&self, f: impl Fn(Value) + 'static) {
        *self.on_overlay.borrow_mut() = Some(Box::new(f));
    }

    /// `mounted()`: `prefers-reduced-motion`, первый кадр (`loadLayers` или `loadScene`),
    /// разрешение игры.
    pub fn mount(&self, reduce_motion: bool, display: Option<(u32, u32)>) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.reduce_motion = reduce_motion;
            inner.display = display;
        }
        self.request();
        self.load_screen();
        self.publish();
    }

    /// Персонализация изменилась (`watch value, deep`).
    pub fn set_value(&self, value: Value) {
        if self.inner.borrow().value == value {
            return;
        }
        self.inner.borrow_mut().value = value;
        self.queue_render();
        self.load_assets();
        self.publish();
    }

    /// Раздел настроек (`hud`, `crosshair`, `weapon`, `keys`, `misc`…).
    pub fn set_section(&self, section: &str) {
        {
            let mut inner = self.inner.borrow_mut();
            if inner.section == section {
                return;
            }
            inner.section = section.to_owned();
            inner.animate = false;
            // `loadScene` 0.4.9 при смене раздела: сцена не загрузилась — новая попытка.
            inner.scene_error = false;
        }
        self.queue_render();
        self.publish();
    }

    /// HUD-мод Condensed HUD включён или выключен (`watch hudMod`).
    pub fn set_hud_mod(&self, on: bool) {
        if self.inner.borrow().hud_mod == on {
            return;
        }
        self.inner.borrow_mut().hud_mod = on;
        self.load_assets();
        self.queue_render();
    }

    /// Размер окна вида (`@box`): портретный кадр берёт ширину от него.
    pub fn set_stage_box(&self, width: f64, height: f64) {
        let changed = {
            let mut inner = self.inner.borrow_mut();
            let before = self.frame_width_of(&inner);
            inner.stage_box = Some((width, height));
            self.frame_width_of(&inner) != before
        };
        if changed {
            self.queue_render();
            self.publish();
        }
    }

    pub fn set_game(&self, game_id: &str) {
        if self.inner.borrow().game_id == game_id {
            return;
        }
        self.inner.borrow_mut().game_id = game_id.to_owned();
        self.load_screen();
    }

    /// Окно открыто или закрыто (`active`): при открытии — разрешение заново и новая
    /// попытка файла, если его не было.
    pub fn set_active(&self, active: bool) {
        let retry = {
            let mut inner = self.inner.borrow_mut();
            // `watch active` срабатывает только при смене значения: переход между
            // разделами с превью разрешение заново не спрашивает.
            if inner.active == active {
                return;
            }
            inner.active = active;
            if !active {
                return;
            }
            // `loadScene` при возврате на страницу.
            inner.scene_error = false;
            let retry = inner.asset_error.is_some();
            if retry {
                inner.asset_signature.clear();
            }
            retry
        };
        self.load_screen();
        if retry {
            self.load_assets();
        }
        self.queue_render();
    }

    /// Оверлей LiveSplit: настройки, снимок окна (иначе пример) и можно ли таскать.
    pub fn set_overlay(
        &self,
        overlay: Option<Value>,
        snapshot: Option<Rc<Snapshot>>,
        editing: bool,
    ) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.overlay = overlay;
            // Ключ картинки различает снимки только по размеру: новый снимок — пересчитать.
            if inner.snapshot != snapshot {
                inner.keyed = None;
            }
            inner.snapshot = snapshot;
            inner.overlay_editing = editing;
        }
        self.queue_render();
        self.publish();
    }

    pub fn toggle_motion(&self) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.animate = !inner.animate;
        }
        self.queue_render();
        self.publish();
    }

    /// Развернуть во весь лаунчер (только когда кадр готов).
    pub fn open_expanded(&self) {
        {
            let mut inner = self.inner.borrow_mut();
            if !Self::shown(&inner) {
                return;
            }
            inner.expanded = true;
        }
        self.publish();
    }

    pub fn close_expanded(&self) {
        {
            let mut inner = self.inner.borrow_mut();
            if !inner.expanded {
                return;
            }
            inner.expanded = false;
        }
        self.publish();
    }

    /// Ручку оверлея перетащили: новый левый верхний угол (px кадра) → место в процентах.
    pub fn move_overlay(&self, left: f64, top: f64) {
        let next = {
            let inner = self.inner.borrow();
            let Some(overlay) = self.overlay_value(&inner) else {
                return;
            };
            let Some(rect) = self.overlay_rect(&inner) else {
                return;
            };
            let position = shared::position_of(
                left,
                top,
                f64::from(self.frame_width_of(&inner)),
                f64::from(FRAME_HEIGHT),
                rect.width as f64,
                rect.height as f64,
            );
            if position.x == overlay.x && position.y == overlay.y {
                return;
            }
            let mut value = overlay.to_value();
            value["x"] = json!(position.x);
            value["y"] = json!(position.y);
            value
        };
        if let Some(f) = self.on_overlay.borrow().as_ref() {
            f(next);
        }
    }

    /// Кадр ещё рисуется: первый не готов, в работе или ждёт «ещё раз» (снимки сцен ждут его, как
    /// картинки). До первого запроса и у закрытого превью — нет.
    pub fn rendering(&self) -> bool {
        let inner = self.inner.borrow();
        inner.active
            && !Self::failed(&inner)
            && inner.serial > 0
            && (!Self::shown(&inner) || inner.pending || inner.again)
    }

    /// Сцена раздела (`scene`): у Ghosting «Призраки» — призрак, «Эффекты и спреи» — спрей.
    fn scene_of(inner: &Inner) -> Option<SceneKind> {
        match (inner.variant, inner.section.as_str()) {
            (Variant::Ghosting, "ghosts") => Some(SceneKind::Ghost),
            (Variant::Ghosting, "misc") => Some(SceneKind::Spray),
            _ => None,
        }
    }

    /// Кадр раздела готов (`shown`).
    fn shown(inner: &Inner) -> bool {
        if Self::scene_of(inner).is_some() {
            inner.scene_ready
        } else {
            inner.ready
        }
    }

    /// Кадр раздела не нарисовался (`failed`).
    fn failed(inner: &Inner) -> bool {
        if Self::scene_of(inner).is_some() {
            inner.scene_error
        } else {
            inner.error
        }
    }

    /// `sceneLines`: расстояние до призрака и своя модель, размер спрея или «выключены».
    fn scene_lines(inner: &Inner) -> Vec<SceneLine> {
        let extra = inner.value.get("extra");
        match Self::scene_of(inner) {
            Some(SceneKind::Ghost) => {
                let mut lines = vec![SceneLine::Ghost(
                    js::math_round(ghost_scene::ghost_distance()) as i64,
                )];
                let caption = match inner.model_state {
                    ModelState::Shown => Some("#L_GS_X_SCENE_GHOST_MODEL_POSE"),
                    ModelState::Missing => Some("#L_GS_X_SCENE_GHOST_MODEL"),
                    ModelState::Failed => Some("#L_GS_X_SCENE_GHOST_MODEL_FAILED"),
                    ModelState::Stock | ModelState::Loading => None,
                };
                lines.extend(caption.map(SceneLine::Key));
                lines
            }
            Some(SceneKind::Spray) => {
                // `Object.assign({gh_sprays: true}, extra)`.
                let sprays = extra
                    .and_then(|e| e.get("gh_sprays"))
                    .is_none_or(js::truthy);
                if !sprays {
                    return vec![SceneLine::Key("#L_GS_X_SCENE_SPRAYS_OFF")];
                }
                let mut lines = Vec::new();
                if let Some(info) = inner.spray_info {
                    lines.push(SceneLine::SpraySize {
                        units: (info.units * 100.0).round() / 100.0,
                        texels: info.texels,
                    });
                }
                let picked = extra
                    .and_then(|e| e.get("gh_spray_file"))
                    .is_some_and(js::truthy);
                if !picked {
                    lines.push(SceneLine::Key("#L_GS_X_SCENE_SPRAY_EXAMPLE"));
                }
                lines
            }
            None => Vec::new(),
        }
    }

    pub fn state(&self) -> State {
        let inner = self.inner.borrow();
        let ready = Self::shown(&inner);
        let failed = Self::failed(&inner);
        let message = if failed {
            "#L_GS_X_OVERLAY_ERROR"
        } else if ready {
            ""
        } else {
            "#L_GS_X_OVERLAY_LOADING"
        };
        State {
            frame_width: self.frame_width_of(&inner),
            ready,
            loading: !ready && !failed,
            message,
            caption: self.caption_of(&inner),
            caption_error: inner.asset_error.is_some(),
            screen: self.screen_of(&inner),
            motion: self.can_animate(&inner),
            animating: inner.animate,
            handle: if inner.overlay_editing {
                self.overlay_rect(&inner)
                    .map(|r| crate::controllers::preview_view::Handle {
                        left: r.left as f64,
                        top: r.top as f64,
                        width: r.width as f64,
                        height: r.height as f64,
                    })
            } else {
                None
            },
            expanded: inner.expanded,
            scene: Self::scene_of(&inner),
            portrait: Self::scene_of(&inner) == Some(SceneKind::Ghost),
            scene_lines: Self::scene_lines(&inner),
        }
    }

    /// Разрешение, в котором запустится игра; неизвестно — монитор (`gameScreen`).
    fn game_screen(&self, inner: &Inner) -> Option<Screen> {
        if inner.variant == Variant::Hl1 {
            return None;
        }
        if let Some(screen) = &inner.screen {
            return Some(screen.clone());
        }
        inner.display.map(|(width, height)| Screen {
            width,
            height,
            source: "display".to_owned(),
        })
    }

    fn frame_width_of(&self, inner: &Inner) -> u32 {
        let scene = Self::scene_of(inner);
        // Портрет: ширина следует за окном вида, не уже 0,45:1 и не шире 1,2:1.
        if scene == Some(SceneKind::Ghost) {
            return match inner.stage_box {
                Some((w, h)) if w > 0.0 && h > 0.0 => {
                    js::math_round((w / h).clamp(0.45, 1.2) * f64::from(FRAME_HEIGHT)) as u32
                }
                _ => FRAME_HEIGHT * 3 / 4,
            };
        }
        if inner.variant == Variant::Hl1 || scene.is_some() {
            return preview::frame_width_of(None);
        }
        preview::frame_width_of(
            self.game_screen(inner)
                .map(|s| (f64::from(s.width), f64::from(s.height))),
        )
    }

    fn screen_of(&self, inner: &Inner) -> Option<(String, String)> {
        if Self::scene_of(inner).is_some() {
            return None;
        }
        self.game_screen(inner).map(|s| {
            (
                format!("#L_GS_X_PREVIEW_SCREEN_{}", s.source.to_uppercase()),
                format!("{}\u{d7}{}", s.width, s.height),
            )
        })
    }

    fn can_animate(&self, inner: &Inner) -> bool {
        inner.variant != Variant::Hl1 && (inner.section == "weapon" || inner.section == "keys")
    }

    /// Прицел пака подчиняется тому же переключателю BXT, что и в игре.
    fn pack_hidden(&self, inner: &Inner) -> bool {
        let v = &inner.value;
        let pack = v
            .get("extra")
            .and_then(|e| e.get("packCrosshair"))
            .filter(|p| js::truthy(p));
        inner.variant == Variant::Hl1
            && pack.is_some_and(|p| p.as_str() != Some("default"))
            && v.get("crossEnabled").is_some_and(js::truthy)
            && v.get("crossHideDefault").is_some_and(js::truthy)
    }

    fn caption_of(&self, inner: &Inner) -> &'static str {
        if let Some(error) = inner.asset_error {
            return if error == "decode" {
                "#L_GS_X_OVERLAY_ASSET_DECODE"
            } else {
                "#L_GS_X_OVERLAY_ASSET_ERROR"
            };
        }
        if self.pack_hidden(inner) {
            return "#L_GS_H_PREVIEW_PACK_HIDDEN";
        }
        if inner.section == "weapon" && inner.variant != Variant::Hl1 {
            return "#L_GS_X_OVERLAY_WEAPON";
        }
        ""
    }

    /// `overlayValue`, если оверлей виден: включён или открыт его раздел.
    fn overlay_value(&self, inner: &Inner) -> Option<Overlay> {
        let overlay = livesplit_overlay::sanitize_overlay(inner.overlay.as_ref()?);
        (overlay.enabled || inner.overlay_editing).then_some(overlay)
    }

    /// Картинка оверлея после обрезки и маски (`overlayImage`); пересчитывается, только
    /// когда меняется то, от чего она зависит (`overlayKeying`).
    fn keyed(&self, inner: &mut Inner) -> Option<(KeyedImage, Arc<Pixmap>)> {
        let overlay = self.overlay_value(inner)?;
        let crop = overlay.crop;
        let key = format!(
            "{}|{:?}|{}|{}|{}|{}|{}|{}|{}/{}/{}/{}|{}",
            overlay.key,
            (
                overlay.key_color.r,
                overlay.key_color.g,
                overlay.key_color.b
            ),
            overlay.similarity,
            overlay.smoothness,
            overlay.luma_min,
            overlay.luma_min_smooth,
            overlay.luma_max,
            overlay.luma_max_smooth,
            crop.top,
            crop.right,
            crop.bottom,
            crop.left,
            inner
                .snapshot
                .as_ref()
                .map_or(0, |s| s.pixels.len() ^ s.width.rotate_left(20) ^ s.height),
        );
        if let Some((have, keyed, image)) = &inner.keyed
            && *have == key
        {
            return Some((keyed.clone(), image.clone()));
        }
        let keyed = match &inner.snapshot {
            Some(s) if !s.pixels.is_empty() => {
                shared::key_image(&overlay, &s.pixels, s.width, s.height, PixelOrder::Bgrx)
            }
            _ => preview::keyed_sample(
                &overlay,
                if inner.variant == Variant::Hl1 {
                    "hl1"
                } else {
                    "ghosting"
                },
            )?,
        };
        let image = Arc::new(preview::keyed_pixmap(&keyed)?);
        inner.keyed = Some((key, keyed.clone(), image.clone()));
        Some((keyed, image))
    }

    fn overlay_rect(&self, inner: &Inner) -> Option<shared::Placement> {
        let overlay = self.overlay_value(inner)?;
        let (keyed, _) = inner
            .keyed
            .as_ref()
            .map(|(_, k, i)| (k.clone(), i.clone()))?;
        preview::overlay_rect(&overlay, self.frame_width_of(inner), &keyed)
    }

    /// `queueRender`: кадр, если превью открыто и готово. Пока рисуется первый кадр
    /// (снимок состояния на момент `mount`), изменение помечается «ещё раз»: в 0.4.7 первый
    /// `draw()` после `loadLayers` читает свойства в момент rAF, т. е. уже изменённые.
    /// Кадр раздела, которого ещё нет (сцена или кадр с HUD), запрашивается как первый:
    /// у 0.4.9 их рендеры грузятся отдельно (`loadLayers`, `loadScene`).
    pub fn queue_render(&self) {
        if self.inner.borrow().active {
            self.request();
        }
    }

    fn request(&self) {
        let job = {
            let mut inner = self.inner.borrow_mut();
            let scene = Self::scene_of(&inner);
            // Кадр той же сцены уже рисуется — «ещё раз» после него. Другой (HUD или сцена
            // раздела) рисует свой поток: запрос сразу, ответ прежнего отбросит номер кадра.
            if inner.pending && inner.pending_scene == scene {
                inner.again = true;
                return;
            }
            if Self::failed(&inner) {
                return;
            }
            inner.again = false;
            inner.pending = true;
            inner.pending_scene = scene;
            inner.serial += 1;
            let time = if inner.animate {
                if inner.reduce_motion {
                    0.25
                } else {
                    self.started.elapsed().as_secs_f64()
                }
            } else {
                0.0
            };
            let overlay = self.keyed(&mut inner).and_then(|(keyed, image)| {
                let overlay = self.overlay_value(&inner)?;
                let rect = preview::overlay_rect(&overlay, self.frame_width_of(&inner), &keyed)?;
                Some((overlay, image, rect))
            });
            let overlay = overlay.filter(|_| scene.is_none());
            Job {
                serial: inner.serial,
                variant: inner.variant,
                width: self.frame_width_of(&inner),
                value: inner.value.clone(),
                section: inner.section.clone(),
                assets: inner.assets.clone(),
                time,
                overlay,
                scene,
            }
        };
        (self.send.borrow())(job);
    }

    /// Кадр готов (из потока рисования, уже в потоке интерфейса).
    pub fn frame_ready(&self, rendered: Rendered) {
        let (serial, result) = rendered;
        let (first_ready, again, animate, kind) = {
            let mut inner = self.inner.borrow_mut();
            if serial != inner.serial {
                return;
            }
            inner.pending = false;
            let kind = inner.pending_scene;
            let scene = kind.is_some();
            let (ready, error) = if scene {
                (inner.scene_ready, &mut inner.scene_error)
            } else {
                (inner.ready, &mut inner.error)
            };
            let first = !ready && result.is_ok();
            if let Err(e) = &result {
                tracing::warn!("[preview] кадр не нарисован: {e}");
                if !ready {
                    *error = true;
                }
            }
            if result.is_ok() {
                if scene {
                    inner.scene_ready = true;
                } else {
                    inner.ready = true;
                }
            }
            if let Ok(frame) = &result
                && scene
            {
                inner.spray_info = frame.spray;
            }
            let again = std::mem::take(&mut inner.again);
            (
                first,
                again,
                inner.animate && !inner.reduce_motion && inner.active,
                kind,
            )
        };
        if let Ok(frame) = result
            && let Some(f) = self.on_frame.borrow().as_ref()
        {
            f(frame.pixmap, kind);
        }
        if first_ready {
            self.load_assets();
        }
        // `loadAssets` уже мог отправить кадр по текущему состоянию: в 0.4.7 его
        // `queueRender` и «ещё раз» сливаются в один rAF — второй кадр не нужен.
        if (again || animate) && !self.inner.borrow().pending {
            self.queue_render();
        }
        self.publish();
    }

    fn publish(&self) {
        let state = self.state();
        if let Some(f) = self.on_state.borrow().as_ref() {
            f(&state);
        }
    }

    /// `loadScreen`: разрешение Ghosting (`ghosting-screen-get`).
    fn load_screen(&self) {
        let game_id = {
            let inner = self.inner.borrow();
            if inner.variant == Variant::Hl1 {
                return;
            }
            inner.game_id.clone()
        };
        let weak = self.weak.borrow().clone();
        (self.invoke)(
            "ghosting-screen-get",
            vec![Value::String(game_id.clone())],
            Box::new(move |reply| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                {
                    let mut inner = this.inner.borrow_mut();
                    if inner.game_id != game_id {
                        return;
                    }
                    let width = reply.get("width").and_then(Value::as_u64).unwrap_or(0) as u32;
                    let height = reply.get("height").and_then(Value::as_u64).unwrap_or(0) as u32;
                    inner.screen = (width > 0).then(|| Screen {
                        width,
                        height,
                        source: reply
                            .get("source")
                            .and_then(Value::as_str)
                            .unwrap_or("display")
                            .to_owned(),
                    });
                }
                this.queue_render();
                this.publish();
            }),
        );
    }

    /// `loadAssets`: шрифты HUD и спрей из файлов игрока (только Ghosting).
    fn load_assets(&self) {
        let (game_id, revision, wanted) = {
            let mut inner = self.inner.borrow_mut();
            if inner.variant == Variant::Hl1 {
                return;
            }
            let extra = inner.value.get("extra").cloned().unwrap_or(Value::Null);
            let text = |key: &str| {
                extra
                    .get(key)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned()
            };
            let signature = format!(
                "{}|{}|{}|{}|{}|{}",
                text("gh_hud_font"),
                text("gh_spray_file"),
                text("gh_showfps_font"),
                text("gh_showpos_font"),
                text("gh_model"),
                inner.hud_mod
            );
            if signature == inner.asset_signature {
                return;
            }
            inner.asset_signature = signature;
            inner.asset_revision += 1;
            inner.assets = Assets {
                hud_mod: inner.hud_mod,
                ..Assets::default()
            };
            inner.asset_error = None;
            inner.model_state = ModelState::Stock;
            let mut wanted: Vec<(&'static str, String)> = Vec::new();
            // Шрифт цифр HUD-мода из его папки; не прочитался — похожий DIN из ОС, без подписи.
            if inner.hud_mod {
                wanted.push(("hudModFont", HUD_MOD_FONT.to_owned()));
            }
            // Своя модель призрака: геометрия из игры → серый силуэт. Стандартная
            // models/player.mdl — готовый ghost.webp.
            let model = text("gh_model");
            if !model.is_empty() && model.trim().to_lowercase() != "models/player.mdl" {
                inner.model_state = ModelState::Loading;
                wanted.push(("gh_model", model));
            }
            wanted.extend(
                ASSET_KEYS
                    .iter()
                    .map(|k| (*k, text(k)))
                    .filter(|(_, v)| !v.is_empty()),
            );
            (inner.game_id.clone(), inner.asset_revision, wanted)
        };
        for (key, value) in wanted {
            let kind = match key {
                "gh_spray_file" => "spray",
                "gh_model" => "model",
                _ => "font",
            };
            let weak = self.weak.borrow().clone();
            let path = value.clone();
            (self.invoke)(
                "game-personalization-preview-asset",
                vec![
                    Value::String(game_id.clone()),
                    json!({"kind": kind, "value": value}),
                ],
                Box::new(move |reply| {
                    if let Some(this) = weak.upgrade() {
                        this.asset_loaded(revision, key, &path, reply);
                    }
                }),
            );
        }
        self.queue_render();
    }

    fn asset_loaded(&self, revision: u64, key: &'static str, path: &str, mut reply: Value) {
        let url = {
            let mut inner = self.inner.borrow_mut();
            if inner.asset_revision != revision {
                return;
            }
            let ok = reply.get("status").and_then(Value::as_str) == Some("ok");
            if key == "gh_model" {
                let model = ok.then(|| ghost_scene::model_of(&reply)).flatten();
                let not_found =
                    reply.get("code").and_then(Value::as_str) == Some("ASSET_NOT_FOUND");
                if model.is_none() && not_found {
                    inner.asset_error = Some("missing");
                }
                inner.model_state = match (&model, not_found) {
                    (Some(_), _) => ModelState::Shown,
                    (None, true) => ModelState::Missing,
                    (None, false) => ModelState::Failed,
                };
                inner.assets.ghost_model = model;
                drop(inner);
                self.queue_render();
                self.publish();
                return;
            }
            match reply.get_mut("dataUrl").map(Value::take) {
                Some(Value::String(url)) if ok && !url.is_empty() => url,
                _ => {
                    if key != "hudModFont" && looks_like_file(path) {
                        inner.asset_error = Some("missing");
                    }
                    drop(inner);
                    self.publish();
                    return;
                }
            }
        };
        let path = path.to_owned();
        self.offload(
            move || decode_asset(key, &path, &url).map(|decoded| (url, decoded)),
            move |this, decoded| this.asset_decoded(revision, key, decoded),
        );
    }

    /// Файл игрока разобран (`url` — ключ шрифта): в кадр; не рисуется — в подпись.
    fn asset_decoded(&self, revision: u64, key: &str, decoded: Option<(String, Decoded)>) {
        {
            let mut inner = self.inner.borrow_mut();
            if inner.asset_revision != revision {
                return;
            }
            // Файл прочитан, но не рисуется: это не «файла нет».
            match decoded {
                Some((url, Ok((family, bytes)))) => {
                    inner.assets.fonts.push((url, Arc::new(bytes)));
                    match key {
                        "hudModFont" => inner.assets.hud_mod_font = Some(family),
                        "gh_hud_font" => inner.assets.font_family = Some(family),
                        "gh_showfps_font" => inner.assets.showfps = Some(family),
                        _ => inner.assets.showpos = Some(family),
                    }
                }
                Some((_, Err(spray))) => inner.assets.spray = Some(spray),
                // Шрифт мода не прочитался — похожий DIN из ОС, ошибки в подписи нет.
                None if key == "hudModFont" => {}
                None => inner.asset_error = Some("decode"),
            }
        }
        self.queue_render();
        self.publish();
    }

    /// `work` в фоновом потоке, его итог — в `then` в потоке интерфейса (если контроллер ещё
    /// жив). Без [`with_worker`] (тесты) — на месте.
    fn offload<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
        then: impl FnOnce(&PreviewController, T) + Send + 'static,
    ) {
        let Some((id, dispatch)) = self.offload.borrow().clone() else {
            return then(self, work());
        };
        let spawned = std::thread::Builder::new()
            .name("hlsr-preview-asset".into())
            .spawn(move || {
                let result = work();
                dispatch(Box::new(move || {
                    if let Some(controller) = controller_by_id(id) {
                        then(&controller, result);
                    }
                }));
            });
        if let Err(error) = spawned {
            tracing::warn!("[preview] файл игрока не разобран: {error}");
        }
    }
}

thread_local! {
    static CONTROLLERS: RefCell<std::collections::HashMap<u64, std::rc::Weak<PreviewController>>> =
        RefCell::new(std::collections::HashMap::new());
}

fn controller_by_id(id: u64) -> Option<Rc<PreviewController>> {
    CONTROLLERS.with(|map| map.borrow().get(&id).and_then(std::rc::Weak::upgrade))
}

/// Потоки рисования для контроллера: готовые кадры возвращаются в поток интерфейса через
/// `dispatch` среды. Потоки живут, пока жив [`RenderWorker`].
pub fn with_worker(
    controller: &Rc<PreviewController>,
    dispatch: crate::env::Dispatch,
) -> RenderWorker {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    CONTROLLERS.with(|map| {
        let mut map = map.borrow_mut();
        map.retain(|_, weak| weak.strong_count() > 0);
        map.insert(id, Rc::downgrade(controller));
    });
    *controller.offload.borrow_mut() = Some((id, dispatch.clone()));
    let worker = RenderWorker::spawn(move |rendered| {
        dispatch(Box::new(move || {
            if let Some(controller) = controller_by_id(id) {
                controller.frame_ready(rendered);
            }
        }));
    });
    controller.set_sender(worker.sender());
    worker
}
