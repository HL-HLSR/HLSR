//! Стекло темы Liquid Glass (`launcher.css` 0.4.9, `[data-theme="liquid-glass"]`): окна, всплывающие
//! панели и «мягкие» кнопки над кадром размывают то, что под ними, и гнут его линзой кромки
//! (`backdrop-filter: blur() <линза> saturate() brightness()`; линза — эталон 21st.dev вместо шума
//! `#lg-refract` 0.4.9, `hlsr_ui::render::liquid`), поверх — подкраска `--lg-tint`, блик
//! `--lg-sheen`, блик кромки, кромка `--lg-rim` и тень `--lg-shadow`.
//!
//! Под стеклом на Windows — снимок окна под блоком (патч `vendor/gpui-pre-windows`):
//! первая тень блока — метка, отрисовка копирует место блока перед ней (кадр, затемнение,
//! содержимое страницы, окна ниже), не чаще 20 раз в секунду, и читает копию на следующих кадрах.
//! Пока снимка нет (первые кадры блока, Linux, тесты) — то, что известно без него: кадр игры с
//! cover и параллаксом (без кадра — `--l-bg-fallback`), затемнение `--l-bg-scrim`, передний план и
//! подложки окон-диалогов (`--l-modal-scrim`, под панелью в окне — и заливка окна); содержимого
//! страницы там нет (отчёты `docs/gpui/reports/liquid.md`, `polish-backdrop.md`).
//!
//! Кусок стекла считает `hlsr_ui::render::liquid::piece_at`: блок, его скругление, подложка и
//! фильтр → картинка в CSS px окна (шире блока на две клетки). [`Surface`] — первый ребёнок блока: первый кусок
//! считает сам в кадре, где блок появился, дальше — в фоновом потоке не чаще [`PACE`]; пока новый
//! считается, рисует прежний (он едет вместе с блоком) с обрезкой блоком и скруглением.
//! Одна задача на блок: параллакс не копит очередь. Куски живут в глобале [`LiquidState`] по
//! глобальному id элемента; блок, которого нет в кадре дольше [`KEEP`], отпускает картинку.

use crate::layer::Layers;
use crate::paint::{scrim_alpha, shadow};
use crate::scale::{zoom, zpx};
use crate::theme::{Gradient, Theme};
use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Bounds, BoxShadow, Corners, Element,
    ElementId, Global, GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId,
    ParentElement, Pixels, Position, Rems, RenderImage, Style, Styled, Window, div, fill, point,
    px, size,
};
use hlsr_ui::render::liquid::{
    BLUR_8, LG_PANEL, LG_SOFT, LG_TINT, LG_WINDOW, LiquidFilter, Paint, SIGMA, SIGMA_COARSE,
    piece_at, sample,
};
use hlsr_ui::render::rail_glass::{Placement, over};
use hlsr_ui::{Rgba, Rgba8Image};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Окна: `background-color: rgba(18, 22, 30, .58)`.
pub const WINDOW_FILL: Rgba = Rgba::new(18, 22, 30, 148);
/// Всплывающие панели: `rgba(24, 28, 38, .5)`.
pub const PANEL_FILL: Rgba = Rgba::new(24, 28, 38, 128);
/// Меню скорости и подсказка плеера «Демо»: `rgba(18, 22, 30, .78)`.
pub const DEMO_FILL: Rgba = Rgba::new(18, 22, 30, 199);
/// Неактивные вкладки окна настроек игры, её подразделы и меню редактора лежат на преломлённом кадре:
/// текст `#e3e8ee`, значок вкладки `#c3c9d2` (контраст не ниже 4,5:1, launcher.css 0.4.9).
pub const NAV_TEXT: u32 = 0xe3e8ee;
pub const NAV_ICON: u32 = 0xc3c9d2;
/// Запасной интервал обновления куска стекла, если частота монитора окна неизвестна (тесты).
/// При известной частоте монитора стекло догоняет подложку в такт кадру монитора (`pace_for`),
/// а `slot.busy` не даёт фоновым задачам копиться в очередь.
pub const PACE: Duration = Duration::from_millis(33);
/// Первые куски в потоке окна — не дольше этого за кадр: на 240 Гц бюджет кадра 4,17 мс, так что
/// смена страницы или темы укладывается в кадр, а остальные и крупные блоки считает фон.
const SYNC_BUDGET: Duration = Duration::from_micros(1500);
/// Крупные поверхности (модальные окна, встроенные панели настроек) считаются в фоне сразу:
/// даже грубый синхронный расчёт такой площади превышает бюджет кадра 240 Гц.
const SYNC_MAX_AREA: f32 = 120_000.;

fn pace_for(window: &Window, cx: &App) -> Duration {
    // Размытие и преломление на ЦП + загрузка текстур в атлас GPU: чаще ~30 Гц (`PACE`)
    // сдвиг размытой подложки не виден, а 240 Гц забивают очередь текстур при смене игр.
    // Все анимации интерфейса, переходы и параллакс при этом идут на полной частоте монитора.
    window
        .refresh_interval(cx)
        .filter(|d| !d.is_zero())
        .map(|d| d.max(PACE))
        .unwrap_or(PACE)
}
/// Блок, которого нет в кадре дольше этого, отпускает кусок (открытое снова окно берёт его сразу).
pub const KEEP: Duration = Duration::from_secs(10);
/// Подложка стекла — снимок окна под блоком (Windows); со стендом (dev-hooks)
/// `HLSR_LIQUID_BACKDROP=0` его снимает (только известные слои [`Under`]) — для замеров.
fn backdrop_on() -> bool {
    static OFF: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
        cfg!(feature = "dev-hooks")
            && std::env::var_os("HLSR_LIQUID_BACKDROP").is_some_and(|v| v == "0")
    });
    !*OFF
}

/// Группа правил стекла launcher.css 0.4.9.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// Окна (`.settings-modal__window`, `.profile__window`, `.confirm__window`…): `--lg-filter`.
    Window,
    /// Всплывающие панели (меню, списки, уведомления, подсказка колонки): `--lg-filter-panel`.
    Panel,
    /// Меню скорости и подсказка плеера «Демо»: панель с подкраской .78.
    DemoMenu,
    /// Кнопки, тосты и поле поиска над кадром: только `--lg-filter-soft`, заливка своя.
    Soft,
    /// Окно настроек игры на странице «Персонализация» (embedded): только `--lg-filter` — заливку,
    /// блик, кромку и тень правила embedded снимают, `backdrop-filter` не трогают.
    Embedded,
    /// Подложка окна-диалога с `backdrop-filter: blur(8px)` (`.news-viewer`) — во всех темах:
    /// только размытие, сверху своя заливка ([`crate::layer::Modal::blur`]).
    Scrim,
}

impl Kind {
    fn filter(self) -> &'static LiquidFilter {
        match self {
            Kind::Window | Kind::Embedded => &LG_WINDOW,
            Kind::Panel | Kind::DemoMenu => &LG_PANEL,
            Kind::Soft => &LG_SOFT,
            Kind::Scrim => &BLUR_8,
        }
    }

    /// Подкраска, блик и кромка есть у окон и панелей.
    fn rimmed(self) -> bool {
        !matches!(self, Kind::Soft | Kind::Embedded | Kind::Scrim)
    }

    /// Что блок рисует поверх подложки; `own` — заливка «мягкого» блока.
    fn paint(self, own: Rgba) -> Paint {
        let fill = match self {
            Kind::Window => WINDOW_FILL,
            Kind::Panel => PANEL_FILL,
            Kind::DemoMenu => DEMO_FILL,
            Kind::Soft | Kind::Scrim => own,
            Kind::Embedded => Rgba::TRANSPARENT,
        };
        let glass = self.rimmed();
        Paint {
            fill,
            tint: if glass { LG_TINT } else { Rgba::TRANSPARENT },
            sheen: glass,
        }
    }
}

/// Переход или анимация ухода стеклянной панели: в Liquid Glass панели уходят так же плавно,
/// как появляются (`l-pop-lg`, `gm-lg`, `toast-out-lg` launcher.css 0.4.9), — иначе стекло
/// с размытием пропадает с фона рывком. Без анимаций (`.no-anim`) уход мгновенный и так.
pub fn motion(theme: &Theme, name: &'static str) -> &'static str {
    match (theme.liquid(), name) {
        (true, "l-pop") => "l-pop-lg",
        (true, "gm") => "gm-lg",
        (true, "toast-out") => "toast-out-lg",
        _ => name,
    }
}

/// Цвет GPUI → байты (прямая альфа).
pub fn bytes(color: impl Into<gpui::Rgba>) -> Rgba {
    let c: gpui::Rgba = color.into();
    let b = |v: f32| (v * 255.).round().clamp(0., 255.) as u8;
    Rgba::new(b(c.r), b(c.g), b(c.b), b(c.a))
}

fn hsla(color: Rgba) -> Hsla {
    gpui::Rgba {
        r: f32::from(color.r) / 255.,
        g: f32::from(color.g) / 255.,
        b: f32::from(color.b) / 255.,
        a: f32::from(color.a) / 255.,
    }
    .into()
}

/// `--lg-shadow`: `0 6px 6px rgba(0,0,0,.2), 0 0 20px rgba(0,0,0,.1), 0 24px 60px rgba(0,0,0,.28)`.
pub fn lg_shadow(zoom: f32) -> Vec<BoxShadow> {
    let black = |a: f32| gpui::black().opacity(a);
    vec![
        shadow(zoom, 0., 6., 6., 0., black(0.2)),
        shadow(zoom, 0., 0., 20., 0., black(0.1)),
        shadow(zoom, 0., 24., 60., 0., black(0.28)),
    ]
}

/// `--lg-rim`: `inset 2px 2px 1px 0 rgba(255,255,255,.28), inset -1px -1px 1px 1px
/// rgba(255,255,255,.14)`.
pub fn lg_rim(zoom: f32) -> Vec<BoxShadow> {
    let white = |a: f32| gpui::white().opacity(a);
    [
        shadow(zoom, 2., 2., 1., 0., white(0.28)),
        shadow(zoom, -1., -1., 1., 1., white(0.14)),
    ]
    .into_iter()
    .map(|s| BoxShadow { inset: true, ..s })
    .collect()
}

/// Что лежит под стеклом окна: кадр и его место (CSS px окна, cover и параллакс), передний план,
/// цвет затемнения `--l-shade-rgb` и CSS-окно, по которому лежит затемнение.
#[derive(Clone)]
pub struct Under {
    pub frame: Arc<Rgba8Image>,
    pub place: Placement,
    pub front: Option<(Arc<Rgba8Image>, Placement)>,
    /// Шарики фона LiveSplit (под затемнением).
    pub orbs: Vec<Orb>,
    pub shade: Rgba,
    pub view: (f32, f32),
    /// `--l-footer-fill` нижних панелей в кадре (при смене «Игры» ↔ LiveSplit — обеих).
    pub footers: Vec<Footer>,
    /// Смена кадра (`launcher-bg-fade`, .5 с): `frame` — уходящий, над ним проявляется новый.
    pub fade: Option<Fade>,
}

/// Новый кадр при смене игры над уходящим: слои фона гаснут и проявляются вместе, поэтому над
/// уходящим кадром (прозрачность `o`) лежат запасной фон с `1 − o` и новый кадр с `alpha` — то же,
/// что окно рисует под стеклом в этот миг (иначе стекло берёт новый кадр одним скачком).
#[derive(Clone)]
pub struct Fade {
    frame: Arc<Rgba8Image>,
    place: Placement,
    alpha: f32,
    /// Запасной фон без кадра (`gradient.jpg`, `--l-app-scrim`, `--l-bg-fallback`) во всё окно.
    base: Arc<Rgba8Image>,
    base_alpha: f32,
}

impl Fade {
    fn same(&self, other: &Fade) -> bool {
        Arc::ptr_eq(&self.frame, &other.frame)
            && self.place == other.place
            && self.alpha == other.alpha
            && Arc::ptr_eq(&self.base, &other.base)
            && self.base_alpha == other.base_alpha
    }
}

/// Цвет с прозрачностью, умноженной на `k`.
fn faint(color: Rgba, k: f32) -> Rgba {
    Rgba {
        a: (f32::from(color.a) * k).round() as u8,
        ..color
    }
}

/// Шарик фона LiveSplit под стеклом: круг цвета (CSS px окна).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Orb {
    pub center: (f32, f32),
    pub radius: f32,
    pub color: Rgba,
}

/// Слой страницы, известный без снимка: `::before` нижней панели «Игр» и LiveSplit —
/// `--l-footer-fill` снизу вверх, слева нарастает на `--l-footer-lead-in` (`mask-image`). Под
/// кнопкой меню игры и выбором сборки он тёмный: без него стекло там светлее 0.4.9 на 20–45 уровней.
#[derive(Debug, Clone, PartialEq)]
pub struct Footer {
    /// Коробка `::before` (CSS px окна).
    pub rect: Placement,
    /// Стопы градиента снизу вверх: (доля, цвет).
    pub stops: Vec<(f32, Rgba)>,
    pub lead: f32,
}

impl Footer {
    pub(crate) fn new(rect: Placement, gradient: &Gradient, lead: f32) -> Self {
        // Угол 0deg — снизу вверх; все темы используют одинаковое направление.
        debug_assert_eq!(gradient.angle, 0.);
        Self {
            rect,
            stops: gradient
                .stops
                .iter()
                .map(|stop| (stop.pos, bytes(stop.color)))
                .collect(),
            lead,
        }
    }

    /// Цвет слоя в точке окна (CSS px), прямая альфа.
    pub(crate) fn at(&self, x: f32, y: f32) -> Rgba {
        let r = self.rect;
        let (Some(first), Some(last)) = (self.stops.first(), self.stops.last()) else {
            return Rgba::TRANSPARENT;
        };
        if x < r.x || x >= r.x + r.width || y < r.y || y >= r.y + r.height {
            return Rgba::TRANSPARENT;
        }
        let t = (r.y + r.height - y) / r.height.max(1.);
        let mut color = if t <= first.0 { first.1 } else { last.1 };
        for pair in self.stops.windows(2) {
            let ((p0, c0), (p1, c1)) = (pair[0], pair[1]);
            if t >= p0 && t <= p1 && p1 > p0 {
                color = premix(c0, c1, (t - p0) / (p1 - p0));
                break;
            }
        }
        let mask = ((x - r.x) / self.lead.max(1.)).clamp(0., 1.);
        Rgba {
            a: (f32::from(color.a) * mask).round() as u8,
            ..color
        }
    }
}

/// Нижняя панель в кадре: её `::before` (`bounds` — px окна) идёт под стекло этого кадра.
pub(crate) fn footer(cx: &mut App, bounds: Bounds<Pixels>, zoom: f32, fill: &Gradient, lead: f32) {
    // Слой ставится в каждом кадре: панель без кэша видов.
    crate::cache::glass();
    cx.default_global::<LiquidState>().footers.push(Footer::new(
        css_rect(bounds, zoom),
        fill,
        lead,
    ));
}

impl Under {
    fn same(&self, other: &Under) -> bool {
        let front = match (&self.front, &other.front) {
            (Some((a, pa)), Some((b, pb))) => Arc::ptr_eq(a, b) && pa == pb,
            (None, None) => true,
            _ => false,
        };
        let fade = match (&self.fade, &other.fade) {
            (Some(a), Some(b)) => a.same(b),
            (None, None) => true,
            _ => false,
        };
        Arc::ptr_eq(&self.frame, &other.frame)
            && self.place == other.place
            && front
            && self.orbs == other.orbs
            && self.shade == other.shade
            && self.view == other.view
            && self.footers == other.footers
            && fade
    }
}

/// Снимок окна под блоком (патч `gpui-pre-windows`, `backdrop::Capture`); вне Windows его нет.
#[derive(Clone)]
#[cfg_attr(not(windows), allow(dead_code))]
struct Captured {
    /// Где снимок в окне, CSS px.
    place: Placement,
    width: u32,
    height: u32,
    /// BGRA с предумноженной альфой; тот же снимок — тот же указатель.
    bgra: Arc<[u8]>,
}

impl Captured {
    fn image(&self) -> Rgba8Image {
        let mut image = Rgba8Image::new(self.width, self.height);
        for (out, &[b, g, r, a]) in image
            .make_mut_slice()
            .iter_mut()
            .zip(self.bgra.as_chunks::<4>().0)
        {
            *out = Rgba::new(r, g, b, a);
        }
        image.premultiplied()
    }
}

/// Из чего кусок: снимок окна под блоком или известные слои с подложками окон-диалогов `fills`.
#[derive(Clone)]
#[cfg_attr(not(windows), allow(dead_code))]
enum Source {
    Captured(Captured),
    Known { under: Under, fills: Vec<Rgba> },
}

impl PartialEq for Source {
    fn eq(&self, other: &Source) -> bool {
        match (self, other) {
            (Source::Captured(a), Source::Captured(b)) => {
                a.place == b.place && Arc::ptr_eq(&a.bgra, &b.bgra)
            }
            (
                Source::Known { under, fills },
                Source::Known {
                    under: other_under,
                    fills: other_fills,
                },
            ) => under.same(other_under) && fills == other_fills,
            _ => false,
        }
    }
}

/// Что считать: блок `rect` (CSS px окна, скругление `radius` CSS px) стекла `kind` над `source`.
#[derive(Clone, PartialEq)]
struct Job {
    source: Source,
    rect: Placement,
    radius: f32,
    kind: Kind,
    paint: Paint,
    /// Клеток сетки на σ размытия: `SIGMA`, у первого куска блока — `SIGMA_COARSE`.
    cells: f32,
}

impl Job {
    /// Кусок и где он лежит (CSS px окна).
    fn run(&self) -> Option<(Arc<RenderImage>, Placement)> {
        let (under, fills) = match &self.source {
            Source::Known { under, fills } => (under, fills),
            Source::Captured(captured) => {
                let image = captured.image();
                let none = |_: f32, _: f32| Rgba::TRANSPARENT;
                let piece = piece_at(
                    &image,
                    captured.place,
                    self.rect,
                    self.radius,
                    self.kind.filter(),
                    self.cells,
                    none,
                    &self.paint,
                )?;
                return render_image(piece);
            }
        };
        let top = |x: f32, y: f32| {
            let shade = Rgba {
                a: (scrim_alpha(x, y, under.view) * 255.).round() as u8,
                ..under.shade
            };
            let mut color = Rgba::TRANSPARENT;
            if let Some(fade) = &under.fade {
                let window = Placement {
                    x: 0.,
                    y: 0.,
                    width: under.view.0,
                    height: under.view.1,
                };
                color = faint(sample(&fade.base, window, x, y), fade.base_alpha);
                color = over(
                    faint(sample(&fade.frame, fade.place, x, y), fade.alpha),
                    color,
                );
            }
            for orb in &under.orbs {
                let (dx, dy) = (x - orb.center.0, y - orb.center.1);
                if dx * dx + dy * dy <= orb.radius * orb.radius {
                    color = over(orb.color, color);
                }
            }
            color = over(shade, color);
            if let Some((front, place)) = &under.front {
                color = over(sample(front, *place, x, y), color);
            }
            for footer in &under.footers {
                color = over(footer.at(x, y), color);
            }
            for fill in fills {
                color = over(*fill, color);
            }
            color
        };
        let piece = piece_at(
            &under.frame,
            under.place,
            self.rect,
            self.radius,
            self.kind.filter(),
            self.cells,
            top,
            &self.paint,
        )?;
        render_image(piece)
    }
}

/// Кусок → картинка GPUI (BGRA) и где она лежит (CSS px окна).
fn render_image(piece: hlsr_ui::render::liquid::Piece) -> Option<(Arc<RenderImage>, Placement)> {
    let (width, height) = (piece.image.width(), piece.image.height());
    let mut bgra = piece.image.to_bytes();
    for pixel in bgra.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    let frame = image::RgbaImage::from_raw(width, height, bgra)?;
    Some((
        Arc::new(RenderImage::new([image::Frame::new(frame)])),
        piece.place,
    ))
}

/// Кусок одного блока: показанный, считается ли новый, ждёт ли кадр срока [`PACE`] и когда
/// начат последний.
struct Slot {
    kind: Kind,
    shown: Option<(Job, Arc<RenderImage>, Placement)>,
    busy: bool,
    armed: bool,
    started: Option<Instant>,
    seen: Instant,
    /// Последний снимок окна под блоком: держится, пока нет нового (кадр без метки блока) и блок
    /// на его месте.
    captured: Option<Captured>,
}

/// Что блоку нужно после кадра: посчитать новый кусок в фоне или кадр к сроку [`PACE`].
enum Next {
    Spawn(Job),
    Wake(Instant),
}

/// Кадр окна, когда готов кусок или подошёл срок нового: с ближайшим кадром движения
/// окна (`motion::monitor_notify`), а не отдельным кадром между ними — параллакс и так рисует, и
/// лишний полный кадр на каждый кусок каждого стекла не нужен. Перерисовывается ближайший
/// закэшированный вид вокруг блока (`cache::cached`; с ним — его предки): он со стеклом в кэш не
/// берётся, а закэшированные соседи повторяются. Вне таких видов — корневой вид.
struct Wake(AnyWindowHandle, Option<gpui::EntityId>);

impl Wake {
    fn new(window: &Window, owner: Option<gpui::EntityId>) -> Wake {
        Wake(window.window_handle(), owner)
    }

    fn call(&self, cx: &mut App) {
        let owner = self.1;
        self.0
            .update(cx, |root, window, _| {
                crate::motion::monitor_notify(window, owner.unwrap_or(root.entity_id()))
            })
            .ok();
    }
}

/// Где кусок блока `from` (CSS px, место куска `place`) лежит под блоком, который сейчас в `to`:
/// кусок едет и тянется вместе с блоком, пока не посчитан новый.
fn fit(from: Placement, to: Placement, place: Placement) -> Placement {
    let (kx, ky) = (to.width / from.width, to.height / from.height);
    Placement {
        x: to.x + (place.x - from.x) * kx,
        y: to.y + (place.y - from.y) * ky,
        width: place.width * kx,
        height: place.height * ky,
    }
}

/// Стопы `--l-app-scrim` и `--l-bg-fallback`, пропорции окна в сотых.
type FallbackKey = ([Vec<Rgba>; 2], u32);

/// Подложка стекла и куски блоков окна лаунчера.
#[derive(Default)]
pub struct LiquidState {
    under: Option<Under>,
    /// Без кадра игры: подложка `.launcher-root` и `--l-bg-fallback` (стопы, пропорции окна в
    /// сотых) и картинка.
    fallback: Option<(FallbackKey, Arc<Rgba8Image>)>,
    /// Слои нижних панелей этого кадра (ставит их `prepaint`, снимаются в начале кадра).
    footers: Vec<Footer>,
    /// Слои нижних панелей прошлого кадра (сохраняются, пока нижняя панель закэширована под
    /// открытым модальным окном верхнего слоя).
    prev_footers: Vec<Footer>,
    slots: HashMap<GlobalElementId, Slot>,
    /// Картинки, ушедшие из показа: освобождаются в атласе на следующем кадре.
    retired: Vec<Arc<RenderImage>>,
    /// Сколько первые куски этого кадра считались в потоке окна ([`SYNC_BUDGET`]).
    sync: Duration,
}

impl Global for LiquidState {}

impl LiquidState {
    /// Сколько блоков держат кусок (замеры, тесты).
    pub fn pieces(cx: &App) -> usize {
        cx.try_global::<LiquidState>().map_or(0, |state| {
            state
                .slots
                .values()
                .filter(|slot| slot.shown.is_some())
                .count()
        })
    }
}

/// Новый кадр при смене игры: картинка, место и прозрачности нового и уходящего слоя.
pub type Incoming = (Arc<Rgba8Image>, Placement, f32, f32);

/// Хозяин фона: что сейчас под стеклом. `frame` — кадр и его место; `None` — кадра нет, под
/// стеклом `--l-bg-fallback` темы. `incoming` — новый кадр, пока он проявляется над `frame`.
pub fn set_under(
    cx: &mut App,
    frame: Option<(Arc<Rgba8Image>, Placement)>,
    incoming: Option<Incoming>,
    front: Option<(Arc<Rgba8Image>, Placement)>,
    orbs: Vec<Orb>,
    view: (f32, f32),
) {
    let theme = cx.global::<Theme>();
    let shade = Rgba {
        a: 255,
        ..bytes(theme.color("l-shade-rgb"))
    };
    let column = |key: &str| -> Vec<Rgba> {
        let gradient = theme.gradient(key);
        // Оба градиента — сверху вниз (180deg) с ровными долями стопов.
        debug_assert_eq!(gradient.angle, 180.);
        gradient
            .stops
            .iter()
            .map(|stop| bytes(stop.color))
            .collect()
    };
    let layers = [column("l-app-scrim"), column("l-bg-fallback")];
    let state = cx.default_global::<LiquidState>();
    let mut base = || {
        let aspect = (view.0 / view.1.max(1.) * 100.).round() as u32;
        let fresh = state
            .fallback
            .as_ref()
            .is_none_or(|(key, _)| key.0 != layers || key.1 != aspect);
        if fresh {
            let image = Arc::new(fallback_image(&layers, view));
            state.fallback = Some(((layers.clone(), aspect), image));
        }
        state.fallback.as_ref().expect("только что").1.clone()
    };
    let fade = incoming.map(|(frame, place, alpha, old)| Fade {
        frame,
        place,
        alpha,
        base: base(),
        base_alpha: 1. - old,
    });
    let (frame, place) = match frame {
        Some(frame) => frame,
        None => {
            let window = Placement {
                x: 0.,
                y: 0.,
                width: view.0,
                height: view.1,
            };
            (base(), window)
        }
    };
    state.under = Some(Under {
        frame,
        place,
        front,
        orbs,
        shade,
        view,
        footers: Vec::new(),
        fade,
    });
}

/// `assets/gradient.jpg` (`--l-app-image` 0.4.9, подложка `.launcher-root`), один раз на процесс.
pub const APP_IMAGE: &str = "gradient.jpg";

fn app_image() -> Option<&'static image::RgbaImage> {
    static IMAGE: std::sync::OnceLock<Option<image::RgbaImage>> = std::sync::OnceLock::new();
    IMAGE
        .get_or_init(|| {
            let bytes = gpui::AssetSource::load(&crate::assets::Assets, APP_IMAGE).ok()??;
            Some(image::load_from_memory(&bytes).ok()?.to_rgba8())
        })
        .as_ref()
}

/// Цвет градиента сверху вниз с ровными долями стопов в доле `t` высоты.
fn column_at(stops: &[Rgba], t: f32) -> Rgba {
    match stops {
        [] => Rgba::TRANSPARENT,
        [one] => *one,
        _ => {
            let at = t.clamp(0., 1.) * (stops.len() - 1) as f32;
            let i = (at.floor() as usize).min(stops.len() - 2);
            premix(stops[i], stops[i + 1], at - i as f32)
        }
    }
}

/// Цвет между стопами `a` и `b` (прямая альфа) в доле `f`, как у CSS — в предумноженных цветах.
fn premix(a: Rgba, b: Rgba, f: f32) -> Rgba {
    let (wa, wb) = (f32::from(a.a), f32::from(b.a));
    let alpha = wa + (wb - wa) * f;
    let mix = |x: u8, y: u8| {
        let v = f32::from(x) * wa + (f32::from(y) * wb - f32::from(x) * wa) * f;
        if alpha > 0. {
            (v / alpha).round().clamp(0., 255.) as u8
        } else {
            0
        }
    };
    Rgba::new(
        mix(a.r, b.r),
        mix(a.g, b.g),
        mix(a.b, b.b),
        alpha.round() as u8,
    )
}

/// Окно без кадра игры, 160 × 90 (рисунок гладкий, стекло его размывает): `gradient.jpg`
/// (`cover`, по центру) под `--l-app-scrim`, сверху `--l-bg-fallback`. В 0.4.9 картинку видно
/// только в Liquid Glass: у прочих тем запасной фон непрозрачен.
fn fallback_image([scrim, fallback]: &[Vec<Rgba>; 2], view: (f32, f32)) -> Rgba8Image {
    let (w, h) = (160u32, 90u32);
    let app = app_image();
    let mut out = Rgba8Image::new(w, h);
    for (index, pixel) in out.make_mut_slice().iter_mut().enumerate() {
        let (i, j) = (index as u32 % w, index as u32 / w);
        let (x, y) = ((i as f32 + 0.5) / w as f32, (j as f32 + 0.5) / h as f32);
        let base = app.map_or(Rgba::rgb(0, 0, 0), |app| {
            // cover: картинка заполняет окно, лишнее срезано поровну.
            let (aw, ah) = (app.width() as f32, app.height() as f32);
            let k = (view.0 / aw).max(view.1 / ah);
            let (dw, dh) = (aw * k, ah * k);
            let u = ((x * view.0 - (view.0 - dw) / 2.) / k).clamp(0., aw - 1.);
            let v = ((y * view.1 - (view.1 - dh) / 2.) / k).clamp(0., ah - 1.);
            let [r, g, b, _] = app.get_pixel(u as u32, v as u32).0;
            Rgba::rgb(r, g, b)
        });
        *pixel = over(column_at(fallback, y), over(column_at(scrim, y), base));
    }
    out
}
/// Начало кадра окна лаунчера: ушедшие картинки освобождаются; блоки, которых давно нет в
/// кадре, отпускают кусок; вне темы Liquid Glass отпускается всё, кроме размытых подложек окон
/// ([`Kind::Scrim`]: они есть во всех темах).
pub fn frame(window: &mut Window, cx: &mut App) {
    let liquid = active(cx.global::<Theme>());
    let under_modal = crate::motion::AnimGate::get(cx).top_layer > 0;
    if !cx.has_global::<LiquidState>() {
        return;
    }
    let state = cx.global_mut::<LiquidState>();
    if !state.footers.is_empty() {
        state.prev_footers = std::mem::take(&mut state.footers);
    } else if !under_modal {
        state.prev_footers.clear();
    }
    state.sync = Duration::ZERO;
    if state.retired.is_empty() && state.slots.is_empty() && (liquid || state.under.is_none()) {
        return;
    }
    let now = Instant::now();
    let mut gone = std::mem::take(&mut state.retired);
    state.slots.retain(|_, slot| {
        let keep = (liquid || slot.kind == Kind::Scrim)
            && (under_modal || now.saturating_duration_since(slot.seen) < KEEP);
        if !keep {
            gone.extend(slot.shown.take().map(|(_, image, _)| image));
        }
        keep
    });
    if !liquid {
        state.under = None;
        state.fallback = None;
        state.prev_footers.clear();
    }
    let dropped = !gone.is_empty();
    for image in gone {
        cx.drop_image(image, Some(window));
    }
    if dropped && !liquid {
        crate::memory::trim_heap_async(cx);
    }
}

/// Слой стекла: первый ребёнок блока (`absolute` во весь блок). Рисует кусок подложки со
/// скруглением блока (пока его нет — заливку и подкраску), сверху — кромку `--lg-rim` и
/// `insets` блока. Вне темы Liquid Glass не нужен: звать через [`glass`] / [`soft`]; во всех
/// темах — только размытая подложка окна ([`Kind::Scrim`], [`crate::layer::Modal::blur`]).
pub struct Surface {
    kind: Kind,
    own: Rgba,
    radius: Rems,
    /// Рамка блока: слой ложится и под неё (`absolute` отсчитывается внутри рамки).
    border: Pixels,
    insets: Vec<BoxShadow>,
    /// Ближайший закэшированный вид вокруг ([`Wake`]).
    owner: Option<gpui::EntityId>,
}

impl Surface {
    pub fn new(kind: Kind, radius: Rems) -> Surface {
        Surface {
            kind,
            own: Rgba::TRANSPARENT,
            radius,
            border: Pixels::ZERO,
            insets: Vec::new(),
            owner: None,
        }
    }

    /// Своя заливка «мягкого» блока.
    pub fn own(mut self, color: Hsla) -> Surface {
        self.own = bytes(color);
        self
    }

    /// Ширина рамки блока.
    pub fn border(mut self, width: Pixels) -> Surface {
        self.border = width;
        self
    }

    /// Тени `inset` блока поверх стекла (у окон и панелей — ещё кромка).
    pub fn insets(mut self, insets: Vec<BoxShadow>) -> Surface {
        self.insets = insets;
        self
    }

    /// Слой в блок. Во время смены темы стекло проявляется или гаснет по доле Liquid Glass
    /// (`Theme::liquid_share`) поверх того, что блок рисует сам: снимок старой темы растворяется
    /// в новом (`startViewTransition` 0.4.9), а не меняется на стекло в первом кадре смены.
    pub fn layer(self, share: f32) -> AnyElement {
        if share >= 1. {
            return self.into_any_element();
        }
        // Обёртка без id: путь элемента и кусок блока те же.
        div()
            .absolute()
            .inset_0()
            .opacity(share)
            .child(self)
            .into_any_element()
    }

    /// Подложки окон-диалогов над затемнением кадра (стекло окна — его заливка с подкраской):
    /// - окно слоя k лежит на `--l-modal-scrim` и на окнах слоёв ниже (слой — у `prepaint`
    ///   окна, `motion::layer`; окно вне слоя — панель друзей — ни на чём);
    /// - всплывающая панель рисуется `deferred` вне слоя: она в верхнем окне, если оно открыто
    ///   (страница под окном недоступна, её панели закрыты);
    /// - мягкое стекло и окно «Персонализации» — на странице, под окнами.
    fn fills(&self, cx: &App) -> Vec<Rgba> {
        let scrim = bytes(cx.global::<Theme>().color("l-modal-scrim"));
        let window = [scrim, WINDOW_FILL, LG_TINT];
        let (under, own_scrim) = match self.kind {
            Kind::Window => {
                let layer = crate::motion::current_layer();
                (layer.saturating_sub(1), layer > 0)
            }
            Kind::Panel | Kind::DemoMenu => (Layers::open_count(cx), false),
            Kind::Soft | Kind::Embedded | Kind::Scrim => (0, false),
        };
        let mut fills: Vec<Rgba> = (0..under.min(2)).flat_map(|_| window).collect();
        if own_scrim {
            fills.push(scrim);
        }
        fills
    }
}

/// Метка подложки — первая тень блока (`gpui_windows::backdrop::MARKER`, прозрачная): отрисовка
/// снимает место блока перед ней, до его теней, заливки и детей. `.shadow(…)` блока после неё
/// метку снимает — тогда под стеклом известные слои.
fn mark<E: Styled>(mut el: E) -> E {
    if let Some(color) = MARKER.filter(|_| backdrop_on()) {
        let marker = BoxShadow {
            color,
            offset: point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(0.),
            inset: false,
        };
        el.style()
            .box_shadow
            .get_or_insert_default()
            .insert(0, marker);
    }
    el
}

#[cfg(windows)]
const MARKER: Option<Hsla> = Some(gpui_windows::backdrop::MARKER);
/// Снимка нет — метка не нужна.
#[cfg(not(windows))]
const MARKER: Option<Hsla> = None;

/// Стекло блока `el`: метка подложки и слой [`Surface`] (или его обёртка [`Surface::layer`])
/// первым ребёнком.
pub fn with_surface<E: Styled + ParentElement>(el: E, surface: impl IntoElement) -> E {
    mark(el).child(surface)
}

/// Снимок окна под блоком `bounds` (px окна) из последних снимков отрисовки: тот, чья коробка
/// больше всех совпадает с блоком (не меньше половины). Блок сдвинулся с прошлого снимка — снимок
/// всё равно годится: пиксели лежат на своих местах окна.
#[cfg(windows)]
fn backdrop(window: &Window, bounds: Bounds<Pixels>, zoom: f32) -> Option<Captured> {
    if !backdrop_on() {
        return None;
    }
    let captures = gpui_windows::backdrop::latest(crate::win32::hwnd(window)?)?;
    let scale = window.scale_factor();
    let own = [
        f32::from(bounds.origin.x) * scale,
        f32::from(bounds.origin.y) * scale,
        f32::from(bounds.size.width) * scale,
        f32::from(bounds.size.height) * scale,
    ];
    let best = captures
        .iter()
        // Байты не того размера не берутся: `Captured::image` их не сверяет.
        .filter(|capture| {
            let [_, _, width, height] = capture.rect;
            capture.bgra.len() == width as usize * height as usize * 4
        })
        .map(|capture| (overlap(own, capture.shape), capture))
        .filter(|(share, _)| *share >= 0.5)
        .max_by(|a, b| a.0.total_cmp(&b.0))?
        .1;
    let [x, y, width, height] = best.rect;
    let k = scale * zoom;
    Some(Captured {
        place: Placement {
            x: x as f32 / k,
            y: y as f32 / k,
            width: width as f32 / k,
            height: height as f32 / k,
        },
        width,
        height,
        bgra: best.bgra.clone(),
    })
}

#[cfg(not(windows))]
fn backdrop(_: &Window, _: Bounds<Pixels>, _: f32) -> Option<Captured> {
    None
}

/// Доля совпадения двух прямоугольников `[x, y, ширина, высота]`: пересечение / объединение.
#[cfg_attr(not(windows), allow(dead_code))]
fn overlap(a: [f32; 4], b: [f32; 4]) -> f32 {
    let w = (a[0] + a[2]).min(b[0] + b[2]) - a[0].max(b[0]);
    let h = (a[1] + a[3]).min(b[1] + b[3]) - a[1].max(b[1]);
    let common = w.max(0.) * h.max(0.);
    let union = a[2] * a[3] + b[2] * b[3] - common;
    if union > 0. { common / union } else { 0. }
}

/// Место в CSS px до половины пикселя: дробный хвост раскладки не плодит новых кусков.
fn css_rect(bounds: Bounds<Pixels>, zoom: f32) -> Placement {
    let q = |v: Pixels| (f32::from(v) / zoom * 2.).round() / 2.;
    Placement {
        x: q(bounds.origin.x),
        y: q(bounds.origin.y),
        width: q(bounds.size.width),
        height: q(bounds.size.height),
    }
}

impl Element for Surface {
    type RequestLayoutState = ();
    type PrepaintState = Option<(Arc<RenderImage>, Bounds<Pixels>)>;

    fn id(&self) -> Option<ElementId> {
        Some("liquid-glass".into())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        // Кусок ищет `prepaint` каждого кадра: виды вокруг блока — без кэша. Вид вокруг —
        // с раскладки: `prepaint` блока в `deferred` идёт уже вне его.
        crate::cache::glass();
        self.owner = crate::cache::stack().last().copied();
        let mut style = Style {
            position: Position::Absolute,
            ..Style::default()
        };
        let edge = (-self.border).into();
        style.inset.top = edge;
        style.inset.left = edge;
        style.inset.right = edge;
        style.inset.bottom = edge;
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let id = id?.clone();
        let zoom = zoom(window);
        let pace = pace_for(window, cx);
        let fills = self.fills(cx);
        let state = cx.default_global::<LiquidState>();
        for image in std::mem::take(&mut state.retired) {
            cx.drop_image(image, Some(window));
        }
        // Часы исполнителя: в выпуске — `Instant::now()`, в тестах — управляемые.
        let now = cx.background_executor().now();
        let under_modal = crate::motion::AnimGate::get(cx).top_layer > 0;
        let state = cx.global_mut::<LiquidState>();
        let footers = if !state.footers.is_empty() {
            state.footers.clone()
        } else if under_modal {
            state.prev_footers.clone()
        } else {
            Vec::new()
        };
        let under = state.under.clone().map(|under| Under { footers, ..under });
        let slot = state.slots.entry(id.clone()).or_insert(Slot {
            kind: self.kind,
            shown: None,
            busy: false,
            armed: false,
            started: None,
            seen: Instant::now(),
            captured: None,
        });
        slot.seen = Instant::now();
        let rect = css_rect(bounds, zoom);
        // Скругление в CSS px до четверти пикселя: форма линзы кромки.
        let radius = (f32::from(self.radius.to_pixels(window.rem_size())) / zoom * 4.).round() / 4.;
        // Блок ушёл с места прежнего снимка (прокрутка, другое место) — до нового известные слои.
        let corners = |p: Placement| [p.x, p.y, p.width, p.height];
        slot.captured = backdrop(window, bounds, zoom).or_else(|| {
            slot.captured
                .take()
                .filter(|old| overlap(corners(old.place), corners(rect)) >= 0.5)
        });
        let source = match slot.captured.clone() {
            Some(captured) => Some(Source::Captured(captured)),
            None => under.map(|under| Source::Known { under, fills }),
        };
        // Блок меньше CSS px (схлопнутый тост) куска не даёт: не просить, иначе задача на кадр.
        let job = source
            .filter(|_| rect.width >= 1. && rect.height >= 1.)
            .map(|source| Job {
                source,
                rect,
                radius,
                kind: self.kind,
                paint: self.kind.paint(self.own),
                cells: SIGMA,
            });
        let mut next = None;
        if let Some(job) = job
            && !slot.busy
        {
            let fresh = slot
                .shown
                .as_ref()
                .is_some_and(|(shown, _, _)| *shown == job);
            let due = slot.started.map_or(now, |started| started + pace);
            if slot.shown.is_none() {
                // Первый кусок небольшого блока — в этом же кадре, в потоке окна: меню, кнопки и
                // тосты появляются сразу со стеклом. Крупные панели и всё сверх SYNC_BUDGET —
                // сразу в фоне, чтобы не срывать кадр на 144–240 Гц.
                if state.sync < SYNC_BUDGET
                    && self.kind != Kind::Scrim
                    && rect.width * rect.height <= SYNC_MAX_AREA
                {
                    let start = Instant::now();
                    let coarse = Job {
                        cells: SIGMA_COARSE,
                        ..job.clone()
                    };
                    if let Some((image, place)) = coarse.run() {
                        slot.shown = Some((coarse, image, place));
                    }
                    state.sync += start.elapsed();
                }
                slot.busy = true;
                slot.started = Some(now);
                next = Some(Next::Spawn(job));
            } else if !fresh && now >= due {
                slot.busy = true;
                slot.started = Some(now);
                next = Some(Next::Spawn(job));
            } else if !fresh && !slot.armed {
                slot.armed = true;
                next = Some(Next::Wake(due));
            }
        }
        // Пока новый кусок считается, прежний едет и тянется вместе с блоком (вход панели со
        // сдвигом и масштабом): под блоком всегда кусок, а не заливка над резким кадром.
        let shown = slot.shown.as_ref().map(|(job, image, place)| {
            let at = fit(job.rect, rect, *place);
            let at = Bounds::new(
                point(zpx(zoom, at.x), zpx(zoom, at.y)),
                size(zpx(zoom, at.width), zpx(zoom, at.height)),
            );
            (image.clone(), at)
        });
        let wake = Wake::new(window, self.owner);
        match next {
            Some(Next::Spawn(job)) => {
                let task = cx.background_spawn({
                    let job = job.clone();
                    async move { job.run() }
                });
                cx.spawn(async move |cx| {
                    let done = task.await;
                    cx.update(|cx| {
                        let state = cx.default_global::<LiquidState>();
                        match state.slots.get_mut(&id) {
                            Some(slot) => {
                                slot.busy = false;
                                if let Some((image, place)) = done
                                    && let Some((_, old, _)) =
                                        slot.shown.replace((job, image, place))
                                {
                                    state.retired.push(old);
                                }
                            }
                            None => state.retired.extend(done.map(|(image, _)| image)),
                        }
                        wake.call(cx);
                    });
                })
                .detach();
            }
            // Подложка сменилась раньше срока: кадр — к сроку, если сам он не придёт.
            Some(Next::Wake(due)) => {
                cx.spawn(async move |cx| {
                    let executor = cx.background_executor().clone();
                    executor
                        .timer(due.saturating_duration_since(executor.now()))
                        .await;
                    cx.update(|cx| {
                        if let Some(slot) = cx.default_global::<LiquidState>().slots.get_mut(&id) {
                            slot.armed = false;
                        }
                        wake.call(cx);
                    });
                })
                .detach();
            }
            None => {}
        }
        shown
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        shown: &mut Self::PrepaintState,
        window: &mut Window,
        _cx: &mut App,
    ) {
        let radii = Corners::all(self.radius.to_pixels(window.rem_size()))
            .clamp_radii_for_quad_size(bounds.size);
        match shown.take() {
            Some((image, at)) => {
                window.paint_image(bounds, at, radii, image, 0, false).ok();
            }
            None => {
                // Подложки нет (до первого кадра фона): заливка и подкраска.
                let paint = self.kind.paint(self.own);
                let flat = over(paint.tint, paint.fill);
                window.paint_quad(fill(bounds, hsla(flat)).corner_radii(radii));
            }
        }
        let mut insets = if self.kind.rimmed() {
            lg_rim(zoom(window))
        } else {
            Vec::new()
        };
        insets.extend(self.insets.iter().cloned());
        window.paint_inset_shadows(bounds, radii, &insets);
    }
}

/// Окно или панель `el` в теме Liquid Glass (иначе — как есть): своя заливка и тени заменяются
/// стеклом `kind` и `--lg-shadow`, рамка остаётся. Звать до `.child(…)`: слой стекла — первый
/// ребёнок. `border` — ширина рамки блока (0 — без рамки).
pub fn glass<E: Styled + ParentElement>(
    el: E,
    kind: Kind,
    radius: Rems,
    border: Pixels,
    theme: &Theme,
    zoom: f32,
) -> E {
    glass_with(el, kind, radius, border, Vec::new(), theme, zoom)
}

/// [`glass`] со своими тенями блока поверх `--lg-rim` и `--lg-shadow` (рамка подписчика профиля,
/// свечение «Спонсорства»): `inset` ложатся на стекло, внешние — перед `--lg-shadow`.
pub fn glass_with<E: Styled + ParentElement>(
    el: E,
    kind: Kind,
    radius: Rems,
    border: Pixels,
    extra: Vec<BoxShadow>,
    theme: &Theme,
    zoom: f32,
) -> E {
    let share = theme.liquid_share();
    if share <= 0. {
        return el;
    }
    let (insets, mut outer): (Vec<BoxShadow>, Vec<BoxShadow>) =
        extra.into_iter().partition(|s| s.inset);
    outer.extend(lg_shadow(zoom));
    let mut el = el;
    // Смена темы: своя тень блока гаснет, тень стекла проявляется.
    let own = el.style().box_shadow.take().unwrap_or_default();
    let shadows: Vec<BoxShadow> = own
        .into_iter()
        .map(|s| faded(s, 1. - share))
        .chain(outer.into_iter().map(|s| faded(s, share)))
        .filter(|s| s.color.a > 0.)
        .collect();
    // Тени `inset` ложатся на стекло и гаснут вместе с ним.
    let surface = Surface::new(kind, radius).border(border).insets(insets);
    with_surface(
        opaque(clear(el, share), share).shadow(shadows),
        surface.layer(share),
    )
}

/// Окно и панель во время смены темы: своя заливка под стеклом непрозрачная. В 0.4.9 снимок
/// старой темы растворяется в снимке новой, и в обоих окно закрывает кадр: непрозрачное (hlsr)
/// или стекло. Смесь `--l-modal-bg` (#12161D → #181C2480) под проявляющимся стеклом (и заливка
/// без подложки, пока первый кусок считается в фоне) пропускала резкий кадр героя. У всех тем,
/// кроме Liquid Glass, `--l-modal-bg` и `--l-surface-solid` без прозрачности: шаг альфы при доле
/// 0 → ε не виден, а альфа по доле снова пропустила бы кадр сквозь окно.
fn opaque<E: Styled>(mut el: E, share: f32) -> E {
    if share > 0.
        && share < 1.
        && let Some(gpui::Fill::Color(bg)) = el.style().background.as_mut()
        && let Some(color) = bg.as_solid()
    {
        *bg = Hsla { a: 1., ..color }.into();
    }
    el
}

/// «Мягкое» стекло (кнопки, тосты, поле поиска над кадром) в теме Liquid Glass: своя заливка
/// `own` и тени `inset` блока ложатся поверх подложки, остальное как есть.
pub fn soft<E: Styled + ParentElement>(
    el: E,
    own: Hsla,
    radius: Rems,
    insets: Vec<BoxShadow>,
    theme: &Theme,
) -> E {
    let share = theme.liquid_share();
    if share <= 0. {
        return el;
    }
    let surface = Surface::new(Kind::Soft, radius).own(own).insets(insets);
    with_surface(clear(el, share), surface.layer(share))
}

/// Стекло в кадре: тема Liquid Glass или смена темы туда или обратно ([`Theme::liquid_share`]).
pub fn active(theme: &Theme) -> bool {
    theme.liquid_share() > 0.
}

/// Своя заливка блока лежит под стеклом, пока оно проявляется или гаснет (смена темы); в самой
/// теме её нет — стекло рисует заливку Liquid Glass.
fn clear<E: Styled>(el: E, share: f32) -> E {
    if share >= 1. {
        el.bg(gpui::transparent_black())
    } else {
        el
    }
}

fn faded(shadow: BoxShadow, k: f32) -> BoxShadow {
    BoxShadow {
        color: shadow.color.opacity(k),
        ..shadow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scale::u;
    use crate::theme::LIQUID_GLASS;
    use gpui::{TestAppContext, div, px};

    const WINDOW: Placement = Placement {
        x: 0.,
        y: 0.,
        width: 800.,
        height: 600.,
    };

    fn liquid(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::init(cx);
            cx.global_mut::<Theme>()
                .set(LIQUID_GLASS, None, false, Instant::now());
            let mut frame = Rgba8Image::new(16, 12);
            for (i, p) in frame.make_mut_slice().iter_mut().enumerate() {
                *p = Rgba::rgb((i * 13) as u8, 90, 160);
            }
            set_under(
                cx,
                Some((Arc::new(frame), WINDOW)),
                None,
                None,
                Vec::new(),
                (800., 600.),
            );
        });
    }

    /// Окно 300×200 с рамкой 1 и полями 8 в точке (100, 50).
    fn draw(window: &mut gpui::VisualTestContext, left: f32) {
        window.draw(
            point(px(0.), px(0.)),
            size(zpx(1., 800.), zpx(1., 600.)),
            move |_, cx| {
                let theme = cx.global::<Theme>().clone();
                let el = div()
                    .absolute()
                    .left(u(left))
                    .top(u(50.))
                    .w(u(300.))
                    .h(u(200.))
                    .border_1()
                    .p(u(8.));
                glass(el, Kind::Window, u(18.), px(1.), &theme, 1.)
                    .child(div().size_full())
                    .into_any_element()
            },
        );
    }

    fn shown(window: &mut gpui::VisualTestContext) -> Vec<(Placement, Arc<RenderImage>)> {
        window.update(|_, cx| {
            cx.global::<LiquidState>()
                .slots
                .values()
                .filter_map(|slot| {
                    let (job, image, _) = slot.shown.as_ref()?;
                    Some((job.rect, image.clone()))
                })
                .collect()
        })
    }

    #[gpui::test]
    fn surface_lies_under_the_border_and_keeps_one_piece(cx: &mut TestAppContext) {
        liquid(cx);
        let window = cx.add_empty_window();
        draw(window, 100.);
        // Первый кусок — в том же кадре, без фоновой задачи. Слой — во всю коробку с рамкой (поля
        // блока его не сужают).
        let first = shown(window);
        let rect = Placement {
            x: 100.,
            y: 50.,
            width: 300.,
            height: 200.,
        };
        assert_eq!(first.iter().map(|(r, _)| *r).collect::<Vec<_>>(), [rect]);
        let atlas = |window: &mut gpui::VisualTestContext, image: &Arc<RenderImage>| {
            let image = image.clone();
            window.update(move |window, _| window.has_image_atlas_entry(&image))
        };
        assert!(atlas(window, &first[0].1));
        // Блок сдвинулся: новый кусок (после PACE), прежний уходит из атласа.
        window.executor().advance_clock(PACE);
        window.run_until_parked();
        draw(window, 140.);
        window.run_until_parked();
        draw(window, 140.);
        let second = shown(window);
        assert_eq!(second[0].0.x, 140.);
        assert!(atlas(window, &second[0].1));
        assert!(!atlas(window, &first[0].1));
        // Другая тема: куски отпускаются.
        window.update(|window, cx| {
            cx.global_mut::<Theme>()
                .set("hlsr", None, false, Instant::now());
            frame(window, cx);
        });
        assert!(shown(window).is_empty());
        assert!(!atlas(window, &second[0].1));
    }

    #[gpui::test]
    fn first_pieces_over_the_frame_budget_come_from_the_background(cx: &mut TestAppContext) {
        liquid(cx);
        let window = cx.add_empty_window();
        window.update(|_, cx| cx.global_mut::<LiquidState>().sync = SYNC_BUDGET);
        draw(window, 100.);
        assert!(shown(window).is_empty());
        window.run_until_parked();
        assert_eq!(shown(window).len(), 1);
        // Новый кадр — бюджет снова целый.
        window.update(|window, cx| {
            frame(window, cx);
            assert_eq!(cx.global::<LiquidState>().sync, Duration::ZERO);
        });
    }

    #[gpui::test]
    fn switch_fades_own_fill_and_shadow_over_the_glass(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::init(cx);
            let start = Instant::now();
            let theme = cx.global_mut::<Theme>();
            theme.set(LIQUID_GLASS, None, true, start);
            let plain = || {
                div().bg(gpui::red().opacity(0.5)).shadow(vec![shadow(
                    1.,
                    0.,
                    4.,
                    8.,
                    0.,
                    gpui::black(),
                )])
            };
            // Начало смены: стекла ещё нет, блок как в старой теме.
            let mut el = glass(plain(), Kind::Window, u(8.), px(0.), theme, 1.);
            assert_eq!(el.style().background, Some(gpui::red().opacity(0.5).into()));
            theme.tick(start + Duration::from_millis(100));
            let share = theme.liquid_share();
            assert!(share > 0. && share < 1.);
            let mut el = glass(plain(), Kind::Window, u(8.), px(0.), theme, 1.);
            let style = el.style();
            // Своя заливка — под проявляющимся стеклом, непрозрачная: кадр сквозь окно не виден.
            assert_eq!(style.background, Some(gpui::red().into()));
            // Метка подложки (первая тень, Windows) — не тень блока.
            let unmarked = |shadows: Option<Vec<BoxShadow>>| -> Vec<BoxShadow> {
                let mut shadows = shadows.unwrap();
                if MARKER.is_some_and(|m| shadows.first().is_some_and(|s| s.color == m)) {
                    shadows.remove(0);
                }
                shadows
            };
            let shadows = unmarked(style.box_shadow.clone());
            assert!((shadows[0].color.a - (1. - share)).abs() < 1e-6);
            assert_eq!(shadows.len(), 1 + lg_shadow(1.).len());
            // Конец смены: своей заливки и теней нет, только стекло.
            theme.tick(start + Duration::from_secs(1));
            let mut el = glass(plain(), Kind::Window, u(8.), px(0.), theme, 1.);
            let style = el.style();
            assert_eq!(unmarked(style.box_shadow.clone()), lg_shadow(1.));
            let Some(gpui::Fill::Color(bg)) = &style.background else {
                panic!("заливка блока пропала");
            };
            assert_eq!(bg.as_solid().unwrap().a, 0.);
        });
    }

    #[gpui::test]
    fn incoming_frame_lies_over_the_leaving_one(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::init(cx);
            cx.global_mut::<Theme>()
                .set(LIQUID_GLASS, None, false, Instant::now());
            let flat = |v: u8| {
                let mut frame = Rgba8Image::new(4, 4);
                frame.make_mut_slice().fill(Rgba::rgb(v, v, v));
                Arc::new(frame)
            };
            let dark = flat(20);
            let mean = |cx: &mut App, incoming: Option<Incoming>| {
                set_under(
                    cx,
                    Some((dark.clone(), WINDOW)),
                    incoming,
                    None,
                    Vec::new(),
                    (800., 600.),
                );
                let job = Job {
                    source: Source::Known {
                        under: cx.global::<LiquidState>().under.clone().unwrap(),
                        fills: Vec::new(),
                    },
                    rect: Placement {
                        x: 300.,
                        y: 200.,
                        width: 200.,
                        height: 150.,
                    },
                    radius: 12.,
                    kind: Kind::Soft,
                    paint: Kind::Soft.paint(Rgba::TRANSPARENT),
                    cells: SIGMA,
                };
                let (image, _) = job.run().unwrap();
                let bytes = image.as_bytes(0).unwrap();
                bytes.iter().map(|&b| f64::from(b)).sum::<f64>() / bytes.len() as f64
            };
            let bright = flat(220);
            let old = mean(cx, None);
            let half = mean(cx, Some((bright.clone(), WINDOW, 0.5, 0.5)));
            let new = mean(cx, Some((bright, WINDOW, 1., 0.)));
            // Стекло перетекает вместе с кадром, а не берёт новый одним скачком.
            assert!(old + 10. < half && half + 10. < new, "{old} {half} {new}");
        });
    }

    #[gpui::test]
    fn constants_match_the_theme_tokens_of_0_4_9(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::init(cx);
            let theme = cx.global_mut::<Theme>();
            theme.set(LIQUID_GLASS, None, false, Instant::now());
            assert_eq!(bytes(theme.color("lg-tint")), LG_TINT);
            let sheen = theme.gradient("lg-sheen");
            assert_eq!(sheen.angle, 150.);
            let stops: Vec<(f32, u8)> = sheen
                .stops
                .iter()
                .map(|stop| (stop.pos, bytes(stop.color).a))
                .collect();
            let want: Vec<(f32, u8)> = hlsr_ui::render::liquid::LG_SHEEN
                .iter()
                .map(|&(pos, a)| (pos, (a * 255.).round() as u8))
                .collect();
            assert_eq!(stops, want);
            // Размытие и цвет — из токенов 0.4.9; `url(#lg-refract…)` там — шум, вместо него линза.
            let filter = |f: &LiquidFilter, refract: &str| {
                let mut text = format!(
                    "blur({}px) url(#{refract}) saturate({}%)",
                    f.blur,
                    (f.saturate * 100.).round()
                );
                if f.brightness != 1. {
                    text += &format!(" brightness({})", f.brightness);
                }
                text
            };
            assert_eq!(
                theme.css("lg-filter"),
                Some(filter(&LG_WINDOW, "lg-refract").as_str())
            );
            assert_eq!(
                theme.css("lg-filter-panel"),
                Some(filter(&LG_PANEL, "lg-refract-strong").as_str())
            );
            assert_eq!(
                theme.css("lg-filter-soft"),
                Some(filter(&LG_SOFT, "lg-refract-strong").as_str())
            );
            assert_eq!(
                theme.css("lg-rim"),
                Some(
                    "inset 2px 2px 1px 0 rgba(255, 255, 255, 0.28), inset -1px -1px 1px 1px \
                     rgba(255, 255, 255, 0.14)"
                )
            );
        });
    }

    #[test]
    fn a_capture_is_matched_by_overlap_and_read_as_bgra() {
        let unit = [0., 0., 10., 10.];
        assert_eq!(overlap(unit, unit), 1.);
        assert!((overlap(unit, [5., 0., 10., 10.]) - 1. / 3.).abs() < 1e-6);
        assert_eq!(overlap(unit, [20., 0., 10., 10.]), 0.);
        let captured = Captured {
            place: WINDOW,
            width: 2,
            height: 1,
            bgra: Arc::from([1, 2, 3, 255, 0, 0, 0, 0]),
        };
        let image = captured.image();
        assert!(image.is_premultiplied());
        assert_eq!(
            image.as_slice(),
            [Rgba::new(3, 2, 1, 255), Rgba::TRANSPARENT]
        );
        // Тот же снимок — тот же кусок; новые байты (даже равные) — новый.
        let same = Source::Captured(captured.clone());
        assert!(same == Source::Captured(captured.clone()));
        let copy = Captured {
            bgra: Arc::from(&captured.bgra[..]),
            ..captured
        };
        assert!(same != Source::Captured(copy));
    }

    #[test]
    fn fallback_shows_the_app_image_under_a_translucent_fallback() {
        let clear = vec![Rgba::TRANSPARENT, Rgba::TRANSPARENT];
        let opaque = vec![Rgba::rgb(13, 17, 24), Rgba::rgb(13, 17, 24)];
        let app = fallback_image(&[clear.clone(), clear.clone()], (1600., 900.));
        // gradient.jpg — цветная картинка: каналы расходятся, всё непрозрачно.
        assert!(app.as_slice().iter().all(|p| p.a == 255));
        assert!(app.as_slice().iter().any(|p| p.r.abs_diff(p.g) > 8));
        // Непрозрачный запасной фон (все темы, кроме Liquid Glass) её закрывает.
        let covered = fallback_image(&[clear, opaque], (1600., 900.));
        assert!(
            covered
                .as_slice()
                .iter()
                .all(|p| *p == Rgba::rgb(13, 17, 24))
        );
        // Белый .07 → тёмный непрозрачный: середина тёмная, как у CSS.
        let mid = column_at(&[Rgba::new(255, 255, 255, 18), Rgba::rgb(13, 17, 24)], 0.5);
        assert!(mid.a == 137 && mid.r < 30, "{mid:?}");
    }
}
