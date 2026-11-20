//! Загрузчик картинок: форматы, уменьшение, кеш с вытеснением, освобождение по области, порт
//! сети. Результат из потока декодирования приходит в очередь, которую тест разбирает сам.

use hlsr_ui::Rgba8Image;
use hlsr_ui::images::{
    self, CacheStats, Dispatch, Fit, FixtureImages, ImageError, ImageLoader, ImageRequest,
    ImageSource, NoRemote, RemoteImages, data_uri_bytes, decode, fitted_size,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Job = Box<dyn FnOnce() + Send>;

/// Очередь «потока интерфейса»: тест сам выполняет то, что вернул поток декодирования.
fn queue() -> (Dispatch, Receiver<Job>) {
    let (tx, rx) = channel::<Job>();
    let tx = Mutex::new(tx);
    (
        Arc::new(move |job| {
            let _ = tx.lock().unwrap().send(job);
        }),
        rx,
    )
}

/// Выполнять пришедшее, пока не станет `done` (не дольше 10 с).
fn pump_until(rx: &Receiver<Job>, done: impl Fn() -> bool) {
    while !done() {
        let job = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("картинка не пришла за 10 с");
        job();
    }
}

fn encode(width: u32, height: u32, format: image::ImageFormat) -> Vec<u8> {
    encode_tinted(width, height, format, 0)
}

/// Картинка: левая половина прозрачная красная, правая — непрозрачная синяя; `tint` меняет
/// зелёный канал (разные байты — разные адреса).
fn encode_tinted(width: u32, height: u32, format: image::ImageFormat, tint: u8) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(width, height, |x, _| {
        if x < width / 2 {
            image::Rgba([255, 0, 0, 0])
        } else {
            image::Rgba([0, tint, 255, 255])
        }
    });
    let mut out = std::io::Cursor::new(Vec::new());
    match format {
        image::ImageFormat::Jpeg => image::DynamicImage::ImageRgba8(img)
            .to_rgb8()
            .write_to(&mut out, format)
            .unwrap(),
        _ => img.write_to(&mut out, format).unwrap(),
    }
    out.into_inner()
}

fn data_uri(bytes: &[u8], mime: &str) -> String {
    use base64::Engine as _;
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

#[test]
fn decodes_png_jpeg_webp_and_rejects_others() {
    for format in [
        image::ImageFormat::Png,
        image::ImageFormat::Jpeg,
        image::ImageFormat::WebP,
    ] {
        let decoded = decode(&encode(40, 20, format), None, Fit::Contain).unwrap();
        assert_eq!((decoded.width, decoded.height), (40, 20), "{format:?}");
        assert_eq!(decoded.natural, (40, 20));
    }
    // GIF и BMP по сигнатуре — не наши форматы (как у загрузчика ПП4).
    let gif = b"GIF89a\x01\x00\x01\x00\x00\x00\x00;";
    assert_eq!(
        decode(gif, None, Fit::Contain).err(),
        Some(ImageError::NotAnImage)
    );
    let bmp = b"BM\x3a\x00\x00\x00\x00\x00\x00\x00\x36\x00\x00\x00";
    assert_eq!(
        decode(bmp, None, Fit::Contain).err(),
        Some(ImageError::NotAnImage)
    );
    assert_eq!(
        decode(b"<svg/>", None, Fit::Contain).err(),
        Some(ImageError::NotAnImage)
    );
    // Обрезанный файл.
    let png = encode(40, 20, image::ImageFormat::Png);
    assert_eq!(
        decode(&png[..40], None, Fit::Contain).err(),
        Some(ImageError::NotAnImage)
    );
}

#[test]
fn premultiplies_alpha() {
    let decoded = decode(&encode(4, 2, image::ImageFormat::Png), None, Fit::Contain).unwrap();
    let bytes = decoded.pixels.to_bytes();
    // Прозрачный красный → (0, 0, 0, 0); непрозрачный синий остаётся.
    assert_eq!(&bytes[..4], &[0, 0, 0, 0]);
    assert_eq!(&bytes[8..12], &[0, 0, 255, 255]);
}

#[test]
fn fitted_size_never_upscales() {
    // Обложка: заполнить обе стороны; вписывание: целиком внутри.
    assert_eq!(
        fitted_size(400, 200, Some((100, 100)), Fit::Cover),
        (200, 100)
    );
    assert_eq!(
        fitted_size(400, 200, Some((100, 100)), Fit::Contain),
        (100, 50)
    );
    assert_eq!(
        fitted_size(400, 200, Some((800, 800)), Fit::Cover),
        (400, 200)
    );
    assert_eq!(fitted_size(400, 200, None, Fit::Cover), (400, 200));
    assert_eq!(
        fitted_size(400, 200, Some((0, 100)), Fit::Cover),
        (400, 200)
    );
    // Арт 2560×1440 в окно 1600×1000 (1280×800 при 1,25).
    assert_eq!(
        fitted_size(2560, 1440, Some((1600, 1000)), Fit::Cover),
        (1778, 1000)
    );
    let decoded = decode(
        &encode(400, 200, image::ImageFormat::WebP),
        Some((100, 100)),
        Fit::Contain,
    )
    .unwrap();
    assert_eq!((decoded.width, decoded.height), (100, 50));
    assert_eq!(decoded.natural, (400, 200));
}

#[test]
fn parses_sources() {
    assert_eq!(
        ImageSource::parse("https://cdn.example/a.png"),
        Some(ImageSource::Remote("https://cdn.example/a.png".into()))
    );
    assert!(matches!(
        ImageSource::parse("DATA:image/png;base64,AA=="),
        Some(ImageSource::DataUri(_))
    ));
    assert_eq!(
        ImageSource::parse("bundle:launcher/hero-hl2.jpg"),
        Some(ImageSource::Bundle("launcher/hero-hl2.jpg".into()))
    );
    assert_eq!(ImageSource::parse("bundle:"), None);
    assert_eq!(ImageSource::parse(""), None);
    assert_eq!(ImageSource::parse("relative/a.png"), None);
    let dir = std::env::temp_dir();
    assert_eq!(
        ImageSource::parse(dir.to_str().unwrap()),
        Some(ImageSource::File(dir.clone()))
    );
    #[cfg(windows)]
    assert_eq!(
        ImageSource::parse("file:///C:/a%20b/c.png"),
        Some(ImageSource::File("C:/a b/c.png".into()))
    );
    #[cfg(not(windows))]
    assert_eq!(
        ImageSource::parse("file:///home/a%20b.png"),
        Some(ImageSource::File("/home/a b.png".into()))
    );
}

#[test]
fn data_uris() {
    assert_eq!(
        data_uri_bytes("data:image/png;base64,AAEC").unwrap(),
        [0, 1, 2]
    );
    assert_eq!(
        data_uri_bytes("data:image/png;base64,AAE=").unwrap(),
        [0, 1]
    );
    // Перевод строки внутри base64 пропускается.
    assert_eq!(
        data_uri_bytes("data:image/png;base64,AA\nEC").unwrap(),
        [0, 1, 2]
    );
    assert_eq!(data_uri_bytes("data:text/plain,a%20b").unwrap(), b"a b");
    assert_eq!(
        data_uri_bytes("data:image/png;base64").err(),
        Some(ImageError::InvalidSource)
    );
    assert_eq!(
        data_uri_bytes("data:image/png;base64,!!!").err(),
        Some(ImageError::InvalidSource)
    );
}

struct Harness {
    loader: ImageLoader,
    rx: Receiver<Job>,
}

impl Harness {
    fn new(remote: Rc<dyn RemoteImages>) -> Harness {
        let (dispatch, rx) = queue();
        Harness {
            loader: ImageLoader::with_dispatch(remote, dispatch),
            rx,
        }
    }

    /// Загрузить и дождаться ответа.
    fn load(&self, request: ImageRequest) -> Result<Arc<Rgba8Image>, ImageError> {
        let slot = Rc::new(RefCell::new(None));
        let out = slot.clone();
        self.loader
            .request(request, move |result| *out.borrow_mut() = Some(result));
        pump_until(&self.rx, || slot.borrow().is_some());
        slot.take().unwrap()
    }

    fn stats(&self) -> CacheStats {
        self.loader.stats()
    }
}

fn size(image: &Rgba8Image) -> (u32, u32) {
    (image.width(), image.height())
}

#[test]
fn loads_lazily_and_caches() {
    let h = Harness::new(Rc::new(NoRemote));
    assert_eq!(
        h.stats(),
        CacheStats::default(),
        "ничего не декодировано до запроса"
    );
    let uri = data_uri(&encode(64, 32, image::ImageFormat::Png), "image/png");
    let request = ImageRequest::new(ImageSource::DataUri(uri.clone()), "home");
    let image = h.load(request.clone()).unwrap();
    assert_eq!(size(&image), (64, 32));
    assert_eq!(h.stats().decodes, 1);
    assert_eq!(h.stats().bytes, 64 * 32 * 4);
    // Второй раз — из кеша, без декодирования и сразу.
    let hit = Rc::new(RefCell::new(false));
    let flag = hit.clone();
    h.loader.request(request.clone(), move |result| {
        assert!(result.is_ok());
        *flag.borrow_mut() = true;
    });
    assert!(*hit.borrow());
    assert_eq!(h.stats().decodes, 1);
    assert!(h.loader.cached(&request).is_some());
}

#[test]
fn shares_pending_load_and_reuses_larger_copy() {
    let h = Harness::new(Rc::new(NoRemote));
    let bytes: &'static [u8] =
        Box::leak(encode(400, 200, image::ImageFormat::WebP).into_boxed_slice());
    images::register_bundle("test/shared.webp", bytes);
    let source = ImageSource::Bundle("test/shared.webp".into());
    let big = ImageRequest::new(source.clone(), "games/218").with_target(200, 200, Fit::Cover);
    let answers = Rc::new(RefCell::new(Vec::new()));
    for _ in 0..3 {
        let answers = answers.clone();
        h.loader.request(big.clone(), move |result| {
            answers.borrow_mut().push(size(&result.unwrap()))
        });
    }
    assert_eq!(
        h.stats().pending,
        1,
        "одинаковые запросы делят одну загрузку"
    );
    pump_until(&h.rx, || answers.borrow().len() == 3);
    // Обложка 200×200 у картинки 2:1 — 400×200 (шаг области 32 px: 224×224 → без уменьшения).
    assert_eq!(*answers.borrow(), vec![(400, 200); 3]);
    assert_eq!(h.stats().decodes, 1);
    // Меньшая область берёт готовую крупную копию.
    let small = ImageRequest::new(source.clone(), "games/218").with_target(64, 64, Fit::Cover);
    assert!(h.loader.cached(&small).is_some());
    // Своё вписывание — своя копия.
    let contain = ImageRequest::new(source, "games/218").with_target(64, 64, Fit::Contain);
    assert!(h.loader.cached(&contain).is_none());
    assert_eq!(size(&h.load(contain).unwrap()), (64, 32));
    assert_eq!(h.stats().decodes, 2);
}

#[test]
fn evicts_least_recently_used_over_budget() {
    let h = Harness::new(Rc::new(NoRemote));
    // Картинка 32×32 = 4 КиБ; предел — две.
    h.loader.set_budget(2 * 32 * 32 * 4);
    let request = |n: u8| {
        let bytes = encode_tinted(32, 32, image::ImageFormat::Png, n);
        ImageRequest::new(ImageSource::DataUri(data_uri(&bytes, "image/png")), "home")
    };
    h.load(request(1)).unwrap();
    h.load(request(2)).unwrap();
    // Первая снова нужна — вытеснится вторая.
    assert!(h.loader.cached(&request(1)).is_some());
    h.load(request(3)).unwrap();
    assert_eq!(h.stats().entries, 2);
    assert_eq!(h.stats().bytes, 2 * 32 * 32 * 4);
    assert!(h.loader.cached(&request(1)).is_some());
    assert!(h.loader.cached(&request(2)).is_none());
    assert!(h.loader.cached(&request(3)).is_some());
    // Картинка больше всего предела остаётся одна.
    h.loader.set_budget(100);
    assert_eq!(h.stats().entries, 0);
    h.load(request(4)).unwrap();
    assert_eq!(h.stats().entries, 1);
}

#[test]
fn release_scope_frees_and_drops_late_results() {
    let h = Harness::new(Rc::new(NoRemote));
    let in_scope = |scope: &str, tint: u8| {
        let bytes = encode_tinted(16, 16, image::ImageFormat::Png, tint);
        ImageRequest::new(ImageSource::DataUri(data_uri(&bytes, "image/png")), scope)
    };
    h.load(in_scope("games/218", 1)).unwrap();
    h.load(in_scope("games/220", 2)).unwrap();
    assert_eq!(h.stats().entries, 2);
    h.loader.release_scope("games/218");
    assert_eq!(h.stats().entries, 1);
    assert_eq!(h.stats().bytes, 16 * 16 * 4);
    assert!(h.loader.cached(&in_scope("games/218", 1)).is_none());
    assert!(h.loader.cached(&in_scope("games/220", 2)).is_some());

    // Область ушла, пока картинка грузилась: ответ получают, но в кеш он не попадает.
    let done = Rc::new(RefCell::new(false));
    let flag = done.clone();
    h.loader
        .request(in_scope("personalization", 3), move |result| {
            assert!(result.is_ok());
            *flag.borrow_mut() = true;
        });
    h.loader.release_scope("personalization");
    pump_until(&h.rx, || *done.borrow());
    assert_eq!(h.stats().entries, 1);
    h.loader.clear();
    assert_eq!(
        h.stats(),
        CacheStats {
            decodes: 3,
            ..CacheStats::default()
        }
    );
}

#[test]
fn remote_images_through_port_and_errors_not_repeated() {
    let dir = tempfile_dir();
    let file = dir.join("avatar.jpg");
    std::fs::write(&file, encode(48, 48, image::ImageFormat::Jpeg)).unwrap();
    let mut files = HashMap::new();
    files.insert("https://avatars.example/a.jpg".to_owned(), file);
    let h = Harness::new(Rc::new(FixtureImages::new(files)));
    let avatar = ImageRequest::new(
        ImageSource::Remote("https://avatars.example/a.jpg".into()),
        "profile",
    )
    .with_target(32, 32, Fit::Cover);
    assert_eq!(
        size(&h.load(avatar).unwrap()),
        (32, 32),
        "уменьшено под область"
    );

    let missing = ImageRequest::new(
        ImageSource::Remote("https://avatars.example/b.jpg".into()),
        "profile",
    );
    assert_eq!(
        h.load(missing.clone()).err(),
        Some(ImageError::Remote("IMAGE_HTTP_404".into()))
    );
    // Неудачный адрес не запрашивается снова, пока область жива: ответ сразу.
    let answered = Rc::new(RefCell::new(None));
    let out = answered.clone();
    h.loader.request(missing.clone(), move |result| {
        *out.borrow_mut() = Some(result.err())
    });
    assert_eq!(
        answered.take(),
        Some(Some(ImageError::Remote("IMAGE_HTTP_404".into())))
    );
    h.loader.release_scope("profile");
    assert_eq!(h.stats().pending, 0);
    h.loader.request(missing, |_| {});
    assert_eq!(
        h.stats().pending,
        1,
        "после освобождения области — новая попытка"
    );
    pump_until(&h.rx, || h.stats().pending == 0);

    // Без сети и с неизвестным файлом сборки — ошибки, не паника.
    let offline = Harness::new(Rc::new(NoRemote));
    assert_eq!(
        offline
            .load(ImageRequest::new(
                ImageSource::Remote("https://x.example/a.png".into()),
                "home"
            ))
            .err(),
        Some(ImageError::Remote("IMAGE_NETWORK".into()))
    );
    assert_eq!(
        offline
            .load(ImageRequest::new(
                ImageSource::Bundle("nope".into()),
                "home"
            ))
            .err(),
        Some(ImageError::NoBundle)
    );
    assert_eq!(
        offline
            .load(ImageRequest::new(
                ImageSource::File(dir.join("nope.png")),
                "home"
            ))
            .err(),
        Some(ImageError::Read)
    );
    let _ = std::fs::remove_dir_all(dir);
}

fn tempfile_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "hlsr-ui-images-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Замер: арт трёх игр 0.4.7 в окне 1280×800 при 1,25 (1600×1000 физ.), переключение
/// 218 → 220 → 70 → 218 с освобождением прежней игры. Не тест — число в отчёт:
/// `cargo test -p hlsr-ui --release --test images -- --ignored --nocapture games_art_memory`.
#[test]
#[ignore = "замер памяти на арте 0.4.7 (нужен ~/HLSR-electron)"]
fn games_art_memory() {
    let root = std::env::var_os("HLSR_ELECTRON")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .map(|home| std::path::PathBuf::from(home).join("HLSR-electron"))
        })
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|home| std::path::PathBuf::from(home).join("HLSR-electron"))
        })
        .unwrap()
        .join("src/renderer/assets/launcher");
    let h = Harness::new(Rc::new(NoRemote));
    let window = (1600, 1000);
    // Фон — обложка окна с запасом параллакса 5 %; передний план — по высоте окна (HL1 —
    // 170 % высоты: `auto 170cqh`).
    let bleed = |(w, h): (u32, u32)| ((w as f64 * 1.05) as u32, (h as f64 * 1.05) as u32);
    let games: [(&str, &str, &str, f64); 3] = [
        (
            "218",
            "hl2-ghosting-bg.png",
            "hl2-ghosting-front.webp",
            1.05,
        ),
        ("220", "hero-hl2.jpg", "hl2-front.webp", 1.05),
        ("70", "hl1-bg.jpg", "hl1-front.png", 1.7),
    ];
    let mut peak = 0;
    let order = [0, 1, 2, 0];
    for (step, &index) in order.iter().enumerate() {
        let (id, hero, front, front_height) = games[index];
        if step > 0 {
            h.loader
                .release_scope(&format!("games/{}", games[order[step - 1]].0));
        }
        let scope = format!("games/{id}");
        let (bw, bh) = bleed(window);
        let hero = h
            .load(
                ImageRequest::new(ImageSource::File(root.join(hero)), &scope).with_target(
                    bw,
                    bh,
                    Fit::Cover,
                ),
            )
            .unwrap();
        let front_h = (f64::from(window.1) * front_height) as u32;
        let front = h
            .load(
                ImageRequest::new(ImageSource::File(root.join(front)), &scope).with_target(
                    u32::MAX / 4,
                    front_h,
                    Fit::Contain,
                ),
            )
            .unwrap();
        let stats = h.stats();
        peak = peak.max(stats.bytes);
        println!(
            "{id}: фон {:?}, передний план {:?}; кеш {} картинок, {:.1} МБ",
            size(&hero),
            size(&front),
            stats.entries,
            stats.bytes as f64 / 1048576.0
        );
    }
    println!("пик кеша {:.1} МБ", peak as f64 / 1048576.0);
    if cfg!(windows) {
        let pid = std::process::id();
        let script = format!(
            "$p = Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -Filter 'IDProcess={pid}'; \
             $g = Get-Process -Id {pid}; \
             '{{0:N1}} {{1:N1}} {{2:N1}}' -f ($p.WorkingSetPrivate/1MB), ($g.WorkingSet64/1MB), ($g.PrivateMemorySize64/1MB)"
        );
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .unwrap();
        println!(
            "процесс: Private Working Set, Working Set, Private Bytes (МБ): {}",
            String::from_utf8_lossy(&out.stdout).trim()
        );
    }
}
