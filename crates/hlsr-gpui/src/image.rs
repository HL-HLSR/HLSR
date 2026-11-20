//! Картинки вида. GPUI рисует `RenderImage` (BGRA, прямая альфа); у каждой свой `ImageId` и
//! место в атласе видеокарты, а атлас сам ничего не освобождает: картинку, ушедшую из показа,
//! убирает `drop_image` (01-gpui §4.8).
//!
//! - [`render_image`] — растр логики (`hlsr_ui::Rgba8Image`) в `RenderImage`;
//! - [`Raster`] — место под растр, который перерисовывается (превью, ник, рамка, цветной SVG
//!   через `hlsr_ui::safe_svg::load_svg`): прежний кадр уходит из атласа при следующей отрисовке;
//! - [`Pictures`] — картинки загрузчика `hlsr_ui::images` (сеть, файлы сборки, `data:`, диск):
//!   одна `RenderImage` на готовую копию загрузчика (перевод в BGRA — в фоне), живёт, пока
//!   копия в его кеше или на экране;
//! - [`decode_live`] — живые WebP и GIF: все кадры с задержками, `img(..).id(..)` листает их сам.
//!
//! Размер растра — физические px области: [`device_size`].

use crate::scale::zoom;
use gpui::{App, AppContext, Context, EntityId, Global, RenderImage, Window};
use hlsr_ui::Rgba8Image;
use hlsr_ui::images::{self, Fit, ImageError, ImageLoader, ImageRequest};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::{Arc, Weak};

/// Растр логики → картинка GPUI: BGRA, прямая альфа (предумноженный делится на альфу).
pub fn render_image(image: &Rgba8Image) -> Arc<RenderImage> {
    let premultiplied = image.is_premultiplied();
    let mut bytes = Vec::with_capacity(image.byte_len());
    for p in image.as_slice() {
        let (r, g, b) = if premultiplied && p.a != 255 {
            let straight = |c: u8| match p.a {
                0 => 0,
                a => ((u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8,
            };
            (straight(p.r), straight(p.g), straight(p.b))
        } else {
            (p.r, p.g, p.b)
        };
        bytes.extend_from_slice(&[b, g, r, p.a]);
    }
    let buffer = ::image::RgbaImage::from_raw(image.width(), image.height(), bytes)
        .expect("размер картинки совпадает с байтами");
    Arc::new(RenderImage::new([::image::Frame::new(buffer)]))
}

/// Физические px области `width × height` CSS px макета (зум окна × масштаб экрана): растры
/// рисуются сразу в этом размере, без второго уменьшения на видеокарте.
pub fn device_size(window: &Window, width: f32, height: f32) -> (u32, u32) {
    let scale = zoom(window) * window.scale_factor();
    (
        (width * scale).round().max(1.) as u32,
        (height * scale).round().max(1.) as u32,
    )
}

/// Место под растр, который перерисовывается. Новый кадр — [`Raster::set`]; рисовать то, что
/// отдаёт [`Raster::image`] (она же убирает из атласа прежние кадры). Когда место уходит
/// насовсем — [`Raster::release`].
#[derive(Default)]
pub struct Raster {
    shown: Option<Arc<RenderImage>>,
    /// Ушли из показа: убираются из атласа при следующей отрисовке.
    retired: Vec<Arc<RenderImage>>,
}

impl Raster {
    /// Новый кадр; пустой растр — то же, что [`Raster::clear`].
    pub fn set(&mut self, image: &Rgba8Image) {
        if image.width() == 0 || image.height() == 0 {
            self.clear();
        } else {
            self.set_render(render_image(image));
        }
    }

    /// Новый кадр, уже готовый для GPUI (например, из [`decode_live`]).
    pub fn set_render(&mut self, image: Arc<RenderImage>) {
        if let Some(old) = self.shown.replace(image.clone())
            && !Arc::ptr_eq(&old, &image)
        {
            self.retired.push(old);
        }
    }

    pub fn clear(&mut self) {
        self.retired.extend(self.shown.take());
    }

    /// Кадр для отрисовки; прежние кадры уходят из атласа всех окон.
    pub fn image(&mut self, window: &mut Window, cx: &mut App) -> Option<Arc<RenderImage>> {
        for old in self.retired.drain(..) {
            cx.drop_image(old, Some(window));
        }
        self.shown.clone()
    }

    /// Место ушло насовсем (вид закрыт): всё из атласа. Звать вне отрисовки окна.
    pub fn release(&mut self, cx: &mut App) {
        for old in self.retired.drain(..).chain(self.shown.take()) {
            cx.drop_image(old, None);
        }
    }
}

/// `RenderImage` готовых копий загрузчика, по адресу копии. Копия ушла из кеша загрузчика и
/// с экрана (слабая ссылка мертва) — её `RenderImage` уходит из атласа.
#[derive(Default)]
struct Converted(HashMap<usize, (Weak<Rgba8Image>, Arc<RenderImage>)>);

fn address(image: &Arc<Rgba8Image>) -> usize {
    Arc::as_ptr(image) as usize
}

impl Converted {
    /// Готовая картинка копии. Звать после [`Converted::sweep`]: тогда живая запись по адресу —
    /// эта же копия.
    fn get(&self, image: &Arc<Rgba8Image>) -> Option<Arc<RenderImage>> {
        self.0
            .get(&address(image))
            .map(|(_, render)| render.clone())
    }

    /// Картинка копии готова. Отдаёт запись умершей копии с тем же адресом: её — из атласа.
    fn insert(
        &mut self,
        image: &Arc<Rgba8Image>,
        render: Arc<RenderImage>,
    ) -> Option<Arc<RenderImage>> {
        self.0
            .insert(address(image), (Arc::downgrade(image), render))
            .map(|(_, old)| old)
    }

    /// Записи копий, которых больше нет: их `RenderImage` — убрать из атласа.
    fn sweep(&mut self) -> Vec<Arc<RenderImage>> {
        let mut gone = Vec::new();
        self.0.retain(|_, (copy, image)| {
            let alive = copy.strong_count() > 0;
            if !alive {
                gone.push(image.clone());
            }
            alive
        });
        gone
    }
}

struct PicturesInner {
    loader: ImageLoader,
    converted: Converted,
    /// Вид и запрос, чья загрузка уже идёт: второй раз не просим.
    waiting: HashSet<(EntityId, String)>,
    /// Копии, которые переводятся в фоне, и виды, которые их ждут.
    converting: HashMap<usize, Vec<EntityId>>,
}

/// Картинки загрузчика `hlsr_ui::images` в атласе GPUI (Global приложения; клон — тот же набор).
/// Окно держит картинку, пока копия загрузчика в его кеше (предел 64 МиБ, освобождение по
/// области — [`Pictures::release_scope`]) или пока вид её показывает.
#[derive(Clone)]
pub struct Pictures(Rc<RefCell<PicturesInner>>);

impl Global for Pictures {}

impl Pictures {
    pub fn new(loader: ImageLoader) -> Pictures {
        Pictures(Rc::new(RefCell::new(PicturesInner {
            loader,
            converted: Converted::default(),
            waiting: HashSet::new(),
            converting: HashMap::new(),
        })))
    }

    pub fn loader(&self) -> ImageLoader {
        self.0.borrow().loader.clone()
    }

    /// Идёт загрузка (и в обход набора: фон грузит кадры загрузчиком сам) или перевод в BGRA
    /// (стенд ждёт картинки перед снимком).
    pub fn busy(&self) -> bool {
        let inner = self.0.borrow();
        !inner.waiting.is_empty()
            || !inner.converting.is_empty()
            || inner.loader.stats().pending > 0
    }

    /// Освободить картинки области (`ImageLoader::release_scope`) и убрать из атласа копии,
    /// которых больше нет, не дожидаясь следующего [`Pictures::get`].
    pub fn release_scope(&self, scope: &str, cx: &mut App) {
        self.loader().release_scope(scope);
        let gone = self.0.borrow_mut().converted.sweep();
        drop_later(gone, cx);
    }

    /// Картинка запроса для вида `cx`: готова — сразу; `None` — загрузка или перевод в BGRA
    /// идёт, вид перерисуется, когда они кончатся; ошибка загрузки — `Some(Err)` (загрузчик её
    /// помнит до освобождения области).
    pub fn get<V: 'static>(
        &self,
        request: ImageRequest,
        _window: &mut Window,
        cx: &mut Context<V>,
    ) -> Option<Result<Arc<RenderImage>, ImageError>> {
        let gone = self.0.borrow_mut().converted.sweep();
        drop_later(gone, cx);
        let loader = self.loader();
        if let Some(image) = loader.cached(&request) {
            return self.ready(image, cx).map(Ok);
        }
        let key = (cx.entity_id(), format!("{request:?}"));
        if !self.0.borrow_mut().waiting.insert(key.clone()) {
            return None;
        }
        // Из кеша или известная ошибка — колбэк зовётся сразу, внутри `request`.
        let now: Rc<RefCell<Option<images::ImageResult>>> = Rc::default();
        let inside = Rc::new(Cell::new(true));
        let app = cx.to_async();
        let this = self.clone();
        let (got, sync) = (now.clone(), inside.clone());
        loader.request(request, move |result| {
            this.0.borrow_mut().waiting.remove(&key);
            if sync.get() {
                *got.borrow_mut() = Some(result);
            } else {
                // Готово позже: вид перерисуется и возьмёт картинку из кеша загрузчика.
                app.update(|cx| cx.notify(key.0));
            }
        });
        inside.set(false);
        let result = now.borrow_mut().take()?;
        match result {
            Ok(image) => self.ready(image, cx).map(Ok),
            Err(error) => Some(Err(error)),
        }
    }

    /// `RenderImage` копии; нет — перевод в фоне (до `MAX_PIXELS` — не в потоке интерфейса), вид
    /// `cx` перерисуется, когда он кончится.
    fn ready<V: 'static>(
        &self,
        image: Arc<Rgba8Image>,
        cx: &mut Context<V>,
    ) -> Option<Arc<RenderImage>> {
        let mut inner = self.0.borrow_mut();
        if let Some(render) = inner.converted.get(&image) {
            return Some(render);
        }
        let views = inner.converting.entry(address(&image)).or_default();
        let first = views.is_empty();
        if !views.contains(&cx.entity_id()) {
            views.push(cx.entity_id());
        }
        drop(inner);
        if first {
            let this = self.clone();
            let task = cx.background_spawn(async move {
                let render = render_image(&image);
                (image, render)
            });
            cx.spawn(async move |_, cx| {
                let (image, render) = task.await;
                let (old, views) = {
                    let mut inner = this.0.borrow_mut();
                    let views = inner.converting.remove(&address(&image));
                    (inner.converted.insert(&image, render), views)
                };
                cx.update(|cx| {
                    drop_later(old.into_iter().collect(), cx);
                    for view in views.into_iter().flatten() {
                        cx.notify(view);
                    }
                });
            })
            .detach();
        }
        None
    }
}

/// Убрать картинки из атласа после кадра и нарисовать окна заново: их ещё может показывать
/// закэшированный вид ([`crate::cache`]) — его повтор кадра ссылался бы на место в атласе, которое
/// отдано другой картинке. Заново нарисованный вид попросит картинку снова.
fn drop_later(gone: Vec<Arc<RenderImage>>, cx: &mut App) {
    if gone.is_empty() {
        return;
    }
    cx.defer(move |cx| {
        for image in gone {
            cx.drop_image(image, None);
        }
        cx.refresh_windows();
    });
}

/// Предел живой картинки: все кадры вместе (BGRA). Кадры сверх него не берутся.
const LIVE_BUDGET: usize = 64 * 1024 * 1024;

/// Живая WebP или GIF: все кадры с задержками (рисовать `img(..).id(..)`: без id GPUI кадры не
/// листает). Неподвижная WebP — один кадр. Не WebP и не GIF, размер вне пределов загрузчика
/// (`hlsr_ui::images::MAX_SIDE`, `MAX_PIXELS`) или ни одного кадра — `None`. Тяжёлая: звать в
/// фоне (`cx.background_spawn`).
pub fn decode_live(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    use ::image::codecs::{gif::GifDecoder, webp::WebPDecoder};
    use ::image::{AnimationDecoder, ImageDecoder, ImageFormat};
    let cursor = std::io::Cursor::new(bytes);
    let fits = |(width, height): (u32, u32)| {
        width > 0
            && height > 0
            && width <= images::MAX_SIDE
            && height <= images::MAX_SIDE
            && u64::from(width) * u64::from(height) <= images::MAX_PIXELS
    };
    let (size, frames) = match ::image::guess_format(bytes).ok()? {
        ImageFormat::Gif => {
            let decoder = GifDecoder::new(cursor).ok()?;
            (decoder.dimensions(), decoder.into_frames())
        }
        ImageFormat::WebP => {
            let mut decoder = WebPDecoder::new(cursor).ok()?;
            if !decoder.has_animation() {
                let still = images::decode(bytes, None, Fit::Contain).ok()?;
                return Some(render_image(&still.pixels));
            }
            decoder
                .set_background_color(::image::Rgba([0, 0, 0, 0]))
                .ok()?;
            (decoder.dimensions(), decoder.into_frames())
        }
        _ => return None,
    };
    if !fits(size) {
        return None;
    }
    let frame_bytes = size.0 as usize * size.1 as usize * 4;
    let mut out = Vec::new();
    for frame in frames {
        let Ok(mut frame) = frame else { break };
        if (out.len() + 1) * frame_bytes > LIVE_BUDGET {
            break;
        }
        for pixel in frame.buffer_mut().pixels_mut() {
            pixel.0.swap(0, 2);
        }
        out.push(frame);
    }
    (!out.is_empty()).then(|| Arc::new(RenderImage::new(out)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scale::zpx;
    use gpui::{IntoElement, Styled, TestAppContext, img, point, px, size};
    use hlsr_ui::Rgba;

    #[test]
    fn render_image_is_bgra_with_straight_alpha() {
        let mut image = Rgba8Image::new(2, 1);
        image.make_mut_slice()[0] = Rgba::new(10, 20, 30, 255);
        image.make_mut_slice()[1] = Rgba::new(100, 50, 0, 128);
        let straight = render_image(&image);
        assert_eq!(
            straight.as_bytes(0).unwrap(),
            [30, 20, 10, 255, 0, 50, 100, 128]
        );
        let premultiplied = render_image(&image.premultiplied());
        assert_eq!(
            premultiplied.as_bytes(0).unwrap(),
            [30, 20, 10, 255, 0, 100, 199, 128]
        );
    }

    #[test]
    fn one_render_image_per_loader_copy_until_the_copy_is_gone() {
        let mut converted = Converted::default();
        let copy = Arc::new(Rgba8Image::new(4, 4));
        let first = render_image(&copy);
        assert!(converted.insert(&copy, first.clone()).is_none());
        assert!(Arc::ptr_eq(&first, &converted.get(&copy).unwrap()));
        assert!(converted.sweep().is_empty());
        drop(copy);
        let gone = converted.sweep();
        assert_eq!(gone.len(), 1);
        assert!(Arc::ptr_eq(&gone[0], &first));
        assert!(converted.0.is_empty());
    }

    /// Копия загрузчика переводится в фоне: сначала `None`, потом вид перерисован и картинка одна.
    #[gpui::test]
    fn loader_copy_converts_in_background(cx: &mut TestAppContext) {
        let env = hlsr_ui::env::ManualEnv::new();
        let pictures = Pictures::new(ImageLoader::new(Rc::new(images::NoRemote), &*env));
        let copy = Arc::new(Rgba8Image::new(4, 4));
        let view = cx.new(|_| ());
        let notified = Rc::new(Cell::new(0));
        let count = notified.clone();
        cx.update(|cx| {
            cx.observe(&view, move |_, _| count.set(count.get() + 1))
                .detach()
        });
        let ready =
            |cx: &mut TestAppContext| view.update(cx, |_, cx| pictures.ready(copy.clone(), cx));
        assert!(ready(cx).is_none());
        assert!(ready(cx).is_none());
        cx.run_until_parked();
        assert_eq!(notified.get(), 1);
        let first = ready(cx).unwrap();
        assert!(Arc::ptr_eq(&first, &ready(cx).unwrap()));
        assert!(pictures.0.borrow().converting.is_empty());
    }

    #[test]
    fn decode_live_keeps_every_gif_frame_and_delay() {
        use ::image::codecs::gif::GifEncoder;
        use ::image::{Delay, Frame, RgbaImage};
        let mut bytes = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut bytes);
            for color in [[255, 0, 0, 255], [0, 0, 255, 255]] {
                let frame = Frame::from_parts(
                    RgbaImage::from_pixel(2, 2, ::image::Rgba(color)),
                    0,
                    0,
                    Delay::from_numer_denom_ms(80, 1),
                );
                encoder.encode_frame(frame).unwrap();
            }
        }
        let live = decode_live(&bytes).unwrap();
        assert_eq!(live.frame_count(), 2);
        assert_eq!(
            std::time::Duration::from(live.delay(1)),
            std::time::Duration::from_millis(80)
        );
        // BGRA: красный кадр — синий канал первым нулевой, красный третьим.
        assert_eq!(&live.as_bytes(0).unwrap()[..4], [0, 0, 255, 255]);
        assert!(decode_live(b"not an image").is_none());
    }

    struct Counted(Rc<Cell<usize>>);

    impl gpui::Render for Counted {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.0.set(self.0.get() + 1);
            gpui::div().size_full()
        }
    }

    struct Host(gpui::Entity<Counted>);

    impl gpui::Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            use gpui::ParentElement;
            let style = gpui::StyleRefinement::default().size_full();
            gpui::div()
                .size_full()
                .child(crate::cache::cached(self.0.clone(), style, true))
        }
    }

    /// Картинка ушла из атласа — закэшированные виды рисуются заново (попросят её снова).
    #[gpui::test]
    fn dropped_picture_redraws_cached_views(cx: &mut TestAppContext) {
        let count = Rc::new(Cell::new(0));
        let shared = count.clone();
        let (_, cx) = cx.add_window_view(|_, cx| Host(cx.new(|_| Counted(shared))));
        cx.run_until_parked();
        let drawn = count.get();
        cx.update(|_, cx| drop_later(Vec::new(), cx));
        cx.run_until_parked();
        assert_eq!(count.get(), drawn);
        let gone = vec![render_image(&Rgba8Image::new(2, 2))];
        cx.update(|_, cx| drop_later(gone, cx));
        cx.run_until_parked();
        assert_eq!(count.get(), drawn + 1);
    }

    /// Приёмка F5: превью перерисовывается 100 раз — в атласе живёт одна картинка.
    #[gpui::test]
    fn hundred_redraws_keep_one_image_in_the_atlas(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        let raster = Rc::new(RefCell::new(Raster::default()));
        let seen: Rc<RefCell<Vec<Arc<RenderImage>>>> = Rc::default();
        for step in 0..100u32 {
            let mut frame = Rgba8Image::new(8 + step % 5, 8);
            frame.make_mut_slice()[0] = Rgba::rgb(step as u8, 0, 0);
            raster.borrow_mut().set(&frame);
            let (place, log) = (raster.clone(), seen.clone());
            let area = size(zpx(1., 64.), zpx(1., 64.));
            window.draw(point(px(0.), px(0.)), area, move |window, cx| {
                let image = place.borrow_mut().image(window, cx).unwrap();
                log.borrow_mut().push(image.clone());
                img(image).size_full().into_any_element()
            });
            let live = window.update(|window, _| {
                seen.borrow()
                    .iter()
                    .filter(|image| window.has_image_atlas_entry(image))
                    .count()
            });
            assert_eq!(live, 1, "шаг {step}");
        }
        let ids: HashSet<_> = seen.borrow().iter().map(|image| image.id).collect();
        assert_eq!(ids.len(), 100);
        window.cx.update(|cx| raster.borrow_mut().release(cx));
        let live = window.update(|window, _| {
            seen.borrow()
                .iter()
                .filter(|image| window.has_image_atlas_entry(image))
                .count()
        });
        assert_eq!(live, 0);
    }
}
