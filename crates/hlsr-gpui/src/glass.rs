//! Стекло как `backdrop-filter` 0.4.9. Удалена только тема Liquid Glass (решение владельца
//! 17.11); в обычных темах матовое стекло над кадром — колонка значков, `.game-rail`
//! с `backdrop-filter: blur(var(--l-rail-blur))` (GameSidebar.vue).
//!
//! [`RailGlass`] — слой стекла колонки: кусок кадра под коробкой колонки, размытый как у
//! Chromium (`hlsr_ui::render::rail_glass`), в фоновом потоке. Пока новый кусок считается
//! (смена кадра, параллакс, масштаб), виден прежний: интерфейс не ждёт. Кусок нового кадра
//! (смена игры) проявляется над прежним за .5 с `--l-ease-fade` от своего прихода, а не
//! подменяет его разом. Затемнение кадра
//! (`--l-bg-scrim`) размывается вместе с кадром, как у Chromium; заливку колонки
//! (`--l-rail-fill` → `--l-rail-open-fill`) колонка рисует поверх слоя.
//!
//! Слой лежит под колонкой отдельным видом оболочки, а не внутри неё: колонка закэширована
//! ([`crate::cache`]), а слой при параллаксе меняется каждый кадр движения — внутри колонки он
//! рисовал бы заново и её.

use crate::layer::Layers;
use crate::motion::{AnimGate, Timing, monitor_notify};
use crate::paint::{scrim_alpha, shadow, strips};
use crate::scale::{u, viewport, zoom};
use crate::shell::background::{Background, scrims};
use crate::theme::Theme;
use gpui::{Context, ObjectFit, Render, RenderImage, WeakEntity, Window, div, img, prelude::*};
use hlsr_ui::render::rail_glass::{Placement, box_backdrop};
use hlsr_ui::{Rgba, Rgba8Image};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Не чаще раза в столько — новый кусок, пока кадр под колонкой движется (параллакс): Chromium
/// размывает подложку на видеокарте каждый кадр, здесь это размытие на ЦП в фоновом потоке и
/// лишняя перерисовка окна по его концу (замер на «Играх» при параллаксе на невидимом столе:
/// поток размытия — 20–25 % ядра). Сдвиг кадра за 50 мс под размытием 28 px не виден.
const PACE: Duration = Duration::from_millis(50);

/// Кадр под стеклом: пиксели и место в окне (CSS px, с учётом cover и параллакса), цвет
/// затемнения `--l-shade-rgb` и окно `view` (CSS px), по которому лежит затемнение.
#[derive(Clone)]
pub struct Backdrop {
    pub pixels: Arc<Rgba8Image>,
    pub frame: Placement,
    pub shade: gpui::Rgba,
    pub view: (f32, f32),
}

impl Backdrop {
    pub(crate) fn same(&self, other: &Backdrop) -> bool {
        Arc::ptr_eq(&self.pixels, &other.pixels)
            && self.frame == other.frame
            && self.shade == other.shade
            && self.view == other.view
    }
}

/// Что считать: кусок кадра под коробкой `size` (CSS px) с размытием `blur`, растянутый до
/// `device` (физические px коробки).
#[derive(Clone)]
struct Job {
    backdrop: Backdrop,
    size: (f32, f32),
    blur: f32,
    device: (u32, u32),
}

impl PartialEq for Job {
    fn eq(&self, other: &Job) -> bool {
        self.backdrop.same(&other.backdrop)
            && self.size == other.size
            && self.blur == other.blur
            && self.device == other.device
    }
}

impl Job {
    fn run(&self) -> Arc<RenderImage> {
        let (width, height) = self.size;
        let (shade, view) = (self.backdrop.shade, self.backdrop.view);
        let byte = |v: f32| (v * 255.).round() as u8;
        let small = box_backdrop(
            &self.backdrop.pixels,
            self.backdrop.frame,
            width,
            height,
            self.blur,
            |x, y| {
                let a = scrim_alpha(x, y, view);
                Rgba::new(byte(shade.r), byte(shade.g), byte(shade.b), byte(a))
            },
        );
        render_image(&small, self.device)
    }
}

/// Растянуть до `device` и отдать GPUI (BGRA, прямая альфа). Растягивает ЦП, а не видеокарта:
/// атлас GPUI кладёт картинки вплотную, и линейная выборка маленькой картинки подмешала бы к
/// краю колонки соседей по атласу.
fn render_image(small: &Rgba8Image, (width, height): (u32, u32)) -> Arc<RenderImage> {
    let small = image::RgbaImage::from_raw(small.width(), small.height(), small.to_bytes())
        .expect("размер картинки совпадает с байтами");
    let mut big = image::imageops::resize(
        &small,
        width.max(1),
        height.max(1),
        image::imageops::FilterType::Triangle,
    );
    for pixel in big.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new([image::Frame::new(big)]))
}

/// Кусок под одной из коробок колонки: показанный и считается ли новый (или ещё не прошёл
/// [`PACE`]); `under` — кусок прежнего кадра, который гаснет под новым.
#[derive(Default)]
struct Piece {
    shown: Option<(Job, Arc<RenderImage>)>,
    busy: bool,
    under: Option<Arc<RenderImage>>,
}

/// Слой стекла колонки (`absolute` от угла строки оболочки, шириной с колонку сейчас): под
/// стеклом — то же, что у фона без кадра (`--l-bg-fallback` и затемнение), сверху — кусок кадра.
/// Кусок считается под свёрнутой (`--l-rail-w`) и под выдвинутой (`--l-rail-open-w`) коробкой,
/// между ними — кроссфейд по выдвижению: в покое ровно то, что у Chromium.
pub struct RailGlass {
    /// Фон, чей кадр под стеклом ([`Background::backdrop`]): слой читает его сам в `render`.
    background: WeakEntity<Background>,
    backdrop: Option<Backdrop>,
    /// Выдвижение: 0 — свёрнута, 1 — выдвинута.
    open: f32,
    pieces: [Piece; 2],
    /// Картинки, ушедшие из показа: освобождаются в атласе при следующей отрисовке.
    retired: Vec<Arc<RenderImage>>,
    /// Пришёл первый кусок нового кадра (смена игры): с этого мгновения новый кусок проявляется
    /// над прежним, как слой фона (`.launcher-bg-fade`, .5 с).
    fade_since: Option<Instant>,
}

impl RailGlass {
    pub fn new(background: WeakEntity<Background>) -> RailGlass {
        RailGlass {
            background,
            backdrop: None,
            open: 0.,
            pieces: Default::default(),
            retired: Vec::new(),
            fade_since: None,
        }
    }

    /// Выдвижение колонки в этом кадре (0…1): оболочка ставит его до отрисовки слоя.
    pub fn set_open(&mut self, open: f32) {
        self.open = open;
    }

    /// Кадр под колонкой, взятый последней отрисовкой.
    #[cfg(test)]
    pub(crate) fn backdrop(&self) -> Option<&Backdrop> {
        self.backdrop.as_ref()
    }

    /// Кадр под колонкой сейчас (`None` — кадра нет): ушёл — куски уходят из атласа.
    fn take_backdrop(&mut self, backdrop: Option<Backdrop>) {
        if backdrop.is_none() {
            for piece in &mut self.pieces {
                self.retired
                    .extend(piece.shown.take().map(|(_, image)| image));
                self.retired.extend(piece.under.take());
            }
        }
        self.backdrop = backdrop;
    }

    /// Посчитать кусок `index`, если показан другой. Одна задача на кусок: готовый кусок виден
    /// сразу, а через [`PACE`] от начала задачи отрисовка спросит снова и получит самое свежее
    /// место кадра (параллакс не копит очередь). Перерисовка по ответу — с кадром движения окна
    /// ([`monitor_notify`]): свой кадр на каждый ответ добавлял до 40 кадров в секунду к параллаксу.
    fn request(&mut self, index: usize, job: Job, window: &mut Window, cx: &mut Context<Self>) {
        let piece = &mut self.pieces[index];
        if piece.busy || piece.shown.as_ref().is_some_and(|(shown, _)| *shown == job) {
            return;
        }
        piece.busy = true;
        // Размытие 28 px на ЦП + загрузка текстуры колонки в атлас GPU: чаще 30 Гц сдвиг под
        // таким размытием не виден, но 240 Гц забивают очередь атласа GPU и дают срывы кадров.
        // Само проявление (fade) и выдвижение колонки при этом идут на полной герцовке монитора.
        const MIN_BLUR_PACE: Duration = Duration::from_millis(33);
        let pace = window
            .refresh_interval(cx)
            .filter(|d| !d.is_zero())
            .map(|d| d.max(MIN_BLUR_PACE))
            .unwrap_or(PACE);
        let task = cx.background_spawn({
            let job = job.clone();
            async move { job.run() }
        });
        cx.spawn_in(window, async move |this, cx| {
            let started = Instant::now();
            let image = task.await;
            this.update_in(cx, |glass, window, cx| {
                let pixels = job.backdrop.pixels.clone();
                if glass.backdrop.is_none() {
                    glass.retired.push(image);
                } else if let Some((old_job, old)) = glass.pieces[index].shown.replace((job, image))
                {
                    // Кусок другого кадра — гаснет под новым; сдвиг параллакса того же кадра —
                    // просто замена. Отсчёт проявления — с прихода куска, а не со смены кадра:
                    // иначе медленное размытие съело бы проявление. Новая смена посреди
                    // проявления начинает его заново.
                    let piece = &mut glass.pieces[index];
                    if !Arc::ptr_eq(&old_job.backdrop.pixels, &pixels) {
                        if glass.fade_since.is_none() || piece.under.is_some() {
                            glass.fade_since = Some(cx.background_executor().now());
                        }
                        glass.retired.extend(piece.under.replace(old));
                    } else {
                        glass.retired.push(old);
                    }
                }
                monitor_notify(window, cx.entity_id());
            })
            .ok();
            let rest = pace.saturating_sub(started.elapsed());
            if !rest.is_zero() {
                cx.background_executor().timer(rest).await;
            }
            this.update_in(cx, |glass, window, cx| {
                glass.pieces[index].busy = false;
                monitor_notify(window, cx.entity_id());
            })
            .ok();
        })
        .detach();
    }
}

impl Render for RailGlass {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        let theme = cx.global::<Theme>().clone();
        let view = viewport(window);
        let shade = theme.color("l-shade-rgb").into();
        if let Some(background) = self.background.upgrade() {
            let backdrop = background.read(cx).backdrop(view, shade);
            self.take_backdrop(backdrop);
        }
        for image in self.retired.drain(..) {
            window.drop_image(image).ok();
        }
        let blur = theme.len("l-rail-blur");
        let widths = [theme.len("l-rail-w"), theme.len("l-rail-open-w")];
        let height = view.1;
        let device = zoom(window) * window.scale_factor();
        let open = self.open.clamp(0., 1.);
        // Смена кадра: доля проявления нового куска, по часам окна.
        let fade = Timing::fade();
        let now = cx.background_executor().now();
        let appear = match self.fade_since {
            Some(since) if !AnimGate::still(cx).any() => {
                fade.at(now.saturating_duration_since(since).as_secs_f64() * 1000.)
            }
            _ => 1.,
        };
        if appear >= 1. {
            self.fade_since = None;
            for piece in &mut self.pieces {
                self.retired.extend(piece.under.take());
            }
        } else {
            crate::motion::monitor_frame(window);
        }
        let radius = Layers::radius(cx);
        let mut layers = Vec::new();
        if let Some(backdrop) = self.backdrop.clone() {
            // Выдвинутая снизу, свёрнутая поверх с прозрачностью 1 − выдвижение.
            for index in [1, 0] {
                let needed = if index == 0 { open < 1. } else { open > 0. };
                if !needed {
                    continue;
                }
                let width = widths[index];
                let job = Job {
                    backdrop: backdrop.clone(),
                    size: (width, height),
                    blur,
                    device: (
                        (width * device).round() as u32,
                        (height * device).round() as u32,
                    ),
                };
                self.request(index, job, window, cx);
                let piece = &self.pieces[index];
                let Some((_, image)) = &piece.shown else {
                    continue;
                };
                let opacity = if index == 0 { 1. - open } else { 1. };
                let layer = |image: &Arc<RenderImage>, opacity: f32| {
                    img(image.clone())
                        .absolute()
                        .top_0()
                        .left_0()
                        .w(u(width))
                        .h(u(height))
                        .object_fit(ObjectFit::Fill)
                        .rounded_tl(radius)
                        .rounded_bl(radius)
                        .opacity(opacity)
                };
                match &piece.under {
                    Some(under) => {
                        layers.push(layer(under, opacity));
                        layers.push(layer(image, opacity * appear));
                    }
                    None => layers.push(layer(image, opacity)),
                }
            }
        }
        let width = widths[0] + (widths[1] - widths[0]) * open;
        // box-shadow выдвинутой колонки `24px 0 48px -32px rgba(0,0,0,.7)` переходит из `none`;
        // она под стеклом (рисуется до детей слоя).
        let rail_shadow = shadow(
            zoom(window),
            24. * open,
            0.,
            48. * open,
            -32. * open,
            gpui::black().opacity(0.7 * open),
        );
        div()
            .absolute()
            .top_0()
            .left_0()
            .h_full()
            .w(u(width))
            .rounded_tl(radius)
            .rounded_bl(radius)
            .when(open > 0., |layer| layer.shadow(vec![rail_shadow]))
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w(u(view.0))
                    .h(u(view.1))
                    .children(strips(&theme.gradient("l-bg-fallback"), radius))
                    .children(scrims(&theme, radius)),
            )
            .children(layers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_image_is_bgra_at_the_device_size() {
        let mut small = Rgba8Image::new(2, 2);
        small.make_mut_slice().fill(Rgba::rgb(10, 20, 30));
        let image = render_image(&small, (40, 300));
        let size = image.size(0);
        assert_eq!((size.width.0, size.height.0), (40, 300));
        let bytes = image.as_bytes(0).unwrap();
        assert_eq!(&bytes[..4], [30, 20, 10, 255]);
    }

    #[test]
    fn every_theme_blurs_the_rail_like_0_4_9() {
        use crate::theme::Table;
        let table = Table::embedded();
        for id in table.ids() {
            let mut theme = Theme::new(table.clone());
            theme.set(id, None, false, std::time::Instant::now());
            assert_eq!(theme.len("l-rail-blur"), 28., "{id}");
        }
    }

    /// Смена кадра под колонкой: кусок нового кадра проявляется над прежним за .5 с, потом
    /// прежний уходит; сдвиг того же кадра (параллакс) — просто замена.
    #[gpui::test]
    fn new_frame_fades_in_over_the_old_one(cx: &mut gpui::TestAppContext) {
        cx.update(crate::init);
        let (glass, cx) = cx.add_window_view(|_, _| RailGlass::new(WeakEntity::new_invalid()));
        let backdrop = |pixels: &Arc<Rgba8Image>, x: f32| Backdrop {
            pixels: pixels.clone(),
            frame: Placement {
                x,
                y: 0.,
                width: 4.,
                height: 4.,
            },
            shade: gpui::rgb(0x05080b),
            view: (4., 4.),
        };
        let step = |cx: &mut gpui::VisualTestContext, ms: u64| {
            cx.executor().advance_clock(Duration::from_millis(ms));
            cx.run_until_parked();
            cx.update(|window, cx| window.simulate_next_frame(cx));
            cx.run_until_parked();
        };
        let under = |cx: &mut gpui::VisualTestContext| {
            glass.read_with(cx, |glass, _| glass.pieces[0].under.is_some())
        };
        let (old, new) = (
            Arc::new(Rgba8Image::new(4, 4)),
            Arc::new(Rgba8Image::new(4, 4)),
        );
        // Без фона (слабая ссылка пуста) кадр под стеклом ставит тест.
        let set = |cx: &mut gpui::VisualTestContext, backdrop: Backdrop| {
            glass.update(cx, |glass, cx| {
                glass.take_backdrop(Some(backdrop));
                cx.notify();
            })
        };
        set(cx, backdrop(&old, 0.));
        step(cx, 0);
        set(cx, backdrop(&old, 1.));
        step(cx, 60);
        assert!(!under(cx), "параллакс того же кадра — замена");
        set(cx, backdrop(&new, 1.));
        step(cx, 60);
        assert!(under(cx), "прежний кадр гаснет под новым");
        step(cx, 300);
        assert!(under(cx), "ещё растворяется");
        step(cx, 200);
        assert!(!under(cx));
    }

    #[test]
    fn jobs_compare_by_frame_identity_not_pixels() {
        let pixels = Arc::new(Rgba8Image::new(4, 4));
        let frame = Placement {
            x: 0.,
            y: 0.,
            width: 4.,
            height: 4.,
        };
        let job = |pixels: &Arc<Rgba8Image>, x: f32| Job {
            backdrop: Backdrop {
                pixels: pixels.clone(),
                frame: Placement { x, ..frame },
                shade: gpui::rgb(0x05080b),
                view: (4., 4.),
            },
            size: (2., 4.),
            blur: 28.,
            device: (2, 4),
        };
        assert!(job(&pixels, 0.) == job(&pixels, 0.));
        // Тот же кадр сдвинут (параллакс) — новый кусок.
        assert!(job(&pixels, 0.) != job(&pixels, 1.));
        // Другой кадр с теми же пикселями (смена игры) — тоже новый.
        let other = Arc::new(Rgba8Image::new(4, 4));
        assert!(job(&pixels, 0.) != job(&other, 0.));
    }
}
