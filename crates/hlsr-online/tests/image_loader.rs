//! Загрузчик картинок с кешем (карта подпроекта 4, §0.8; в 0.4.7 это делал Chromium для `<img>`):
//! формат и размеры по заголовку PNG, JPEG и WebP, пределы, редиректы, кеш на диске во временной
//! папке (запись, срок жизни по поддельным часам, повреждённый файл, вытеснение), один запрос на
//! одинаковые адреса, отказ при зависании. Сеть — двойник сервера на `127.0.0.1`.
#![cfg(any(debug_assertions, feature = "dev-hooks"))]

mod support;

use hlsr_online::image_loader::{
    ImageError, ImageFormat, ImageLoader, ImageLoaderOptions, MAX_IMAGE_BYTES, sniff,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use support::FakeClock;
use support::server::{Journal, Server, start_server};

// ------------------------------------------------------------------ файлы-образцы (только заголовки)

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// PNG: подпись, `IHDR` и `IEND`; контрольные суммы не нужны — читается только заголовок.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
    out.extend_from_slice(b"IHDR");
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(&[8, 2, 0, 0, 0, 0xAA, 0xBB, 0xCC, 0xDD]);
    out.extend_from_slice(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]);
    out
}

/// JPEG: `APP0`, комментарий, таблица Хаффмана, кадр `sof` (0xC0 — базовый, 0xC2 — прогрессивный).
fn jpeg(width: u16, height: u16, sof: u8) -> Vec<u8> {
    let mut out = vec![0xFF, 0xD8];
    out.extend_from_slice(&[0xFF, 0xE0, 0, 16]);
    out.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
    out.extend_from_slice(&[0xFF, 0xFE, 0, 5, b'a', b'b', b'c']);
    // Набивка 0xFF перед маркером и DHT (0xC4 — не кадр).
    out.extend_from_slice(&[0xFF, 0xFF, 0xC4, 0, 6, 1, 2, 3, 4]);
    out.extend_from_slice(&[0xFF, sof, 0, 17, 8]);
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&[3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}

fn riff(chunk: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut body = b"WEBP".to_vec();
    body.extend_from_slice(chunk);
    body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    body.extend_from_slice(payload);
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// WebP с потерями (`VP8 `).
fn webp_lossy(width: u16, height: u16) -> Vec<u8> {
    let mut payload = vec![0x30, 0x01, 0x00, 0x9D, 0x01, 0x2A];
    payload.extend_from_slice(&width.to_le_bytes());
    payload.extend_from_slice(&height.to_le_bytes());
    payload.extend_from_slice(&[0; 8]);
    riff(b"VP8 ", &payload)
}

/// WebP без потерь (`VP8L`): размеры хранятся без единицы.
fn webp_lossless(width: u32, height: u32) -> Vec<u8> {
    let bits = (width - 1) | ((height - 1) << 14);
    let mut payload = vec![0x2F];
    payload.extend_from_slice(&bits.to_le_bytes());
    payload.extend_from_slice(&[0; 8]);
    riff(b"VP8L", &payload)
}

/// WebP расширенный (`VP8X`): холст без единицы, по три байта.
fn webp_extended(width: u32, height: u32) -> Vec<u8> {
    let mut payload = vec![0x10, 0, 0, 0];
    payload.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
    payload.extend_from_slice(&(height - 1).to_le_bytes()[..3]);
    riff(b"VP8X", &payload)
}

// ------------------------------------------------------------------ заголовок

#[test]
fn sniff_reads_format_and_size_from_the_header() {
    let info = |bytes: &[u8]| sniff(bytes).map(|i| (i.format, i.width, i.height));
    assert_eq!(info(&png(640, 480)), Some((ImageFormat::Png, 640, 480)));
    assert_eq!(info(&png(1, 1)), Some((ImageFormat::Png, 1, 1)));
    assert_eq!(
        info(&jpeg(1920, 1080, 0xC0)),
        Some((ImageFormat::Jpeg, 1920, 1080))
    );
    assert_eq!(
        info(&jpeg(800, 600, 0xC2)),
        Some((ImageFormat::Jpeg, 800, 600))
    );
    assert_eq!(
        info(&webp_lossy(320, 200)),
        Some((ImageFormat::Webp, 320, 200))
    );
    assert_eq!(info(&webp_lossless(1, 1)), Some((ImageFormat::Webp, 1, 1)));
    assert_eq!(
        info(&webp_lossless(16384, 16384)),
        Some((ImageFormat::Webp, 16384, 16384))
    );
    assert_eq!(
        info(&webp_extended(8000, 4000)),
        Some((ImageFormat::Webp, 8000, 4000))
    );
}

#[test]
fn sniff_refuses_everything_else() {
    let mut bad_signature = png(10, 10);
    bad_signature[1] = b'Q';
    let mut bad_chunk = png(10, 10);
    bad_chunk[12..16].copy_from_slice(b"IDAT");
    let mut no_frame = jpeg(10, 10, 0xC0);
    // Вместо кадра — таблица квантования той же длины, дальше конец изображения.
    let at = no_frame.iter().position(|&b| b == 0xC0).unwrap();
    no_frame[at] = 0xDB;
    let mut wave = webp_lossy(10, 10);
    wave[8..12].copy_from_slice(b"WAVE");
    let mut start_code = webp_lossy(10, 10);
    start_code[23] = 0;
    let mut lossless_mark = webp_lossless(10, 10);
    lossless_mark[20] = 0x2E;
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("пусто", Vec::new()),
        ("GIF", b"GIF89a\x01\x00\x01\x00\x00\x00\x00;".to_vec()),
        (
            "SVG",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_vec(),
        ),
        ("HTML", b"<!doctype html><html></html>".to_vec()),
        ("текст", "картинка не нашлась".as_bytes().to_vec()),
        ("подпись PNG неверна", bad_signature),
        ("первый блок PNG не IHDR", bad_chunk),
        ("PNG обрезан", png(10, 10)[..23].to_vec()),
        ("PNG без ширины", png(0, 10)),
        ("PNG без высоты", png(10, 0)),
        ("PNG шире i32", png(0x8000_0000, 1)),
        ("JPEG без кадра", no_frame),
        ("JPEG обрезан до кадра", jpeg(10, 10, 0xC0)[..24].to_vec()),
        ("JPEG только начало", vec![0xFF, 0xD8, 0xFF]),
        ("JPEG без ширины", jpeg(0, 10, 0xC0)),
        ("RIFF не WebP", wave),
        ("WebP без кода начала", start_code),
        ("WebP без потерь: подпись", lossless_mark),
        ("WebP обрезан", webp_lossy(10, 10)[..29].to_vec()),
        ("WebP: неизвестный блок", riff(b"ANIM", &[0; 20])),
        ("WebP с потерями без ширины", webp_lossy(0, 10)),
    ];
    for (what, bytes) in cases {
        assert_eq!(sniff(&bytes), None, "{what}");
    }
}

// ------------------------------------------------------------------ загрузка

struct Env {
    server: Server,
    journal: Journal,
    dir: tempfile::TempDir,
    clock: FakeClock,
}

/// Время часов загрузчика: срок жизни кеша считается от него.
const NOW: i64 = 1_760_000_000_000;

async fn env(routes: Value) -> Env {
    let journal: Journal = Arc::new(Mutex::new(Vec::new()));
    let server = start_server("cdn", journal.clone()).await;
    server.set_routes(&routes);
    Env {
        server,
        journal,
        dir: tempfile::tempdir().unwrap(),
        clock: FakeClock::new(NOW),
    }
}

impl Env {
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.server.authority)
    }

    fn loader(&self, tweak: impl FnOnce(&mut ImageLoaderOptions)) -> ImageLoader {
        let mut options = ImageLoaderOptions {
            clock: self.clock.clock(),
            ..ImageLoaderOptions::default()
        };
        tweak(&mut options);
        ImageLoader::new(self.dir.path().join("images"), options)
    }

    fn requests(&self) -> usize {
        self.journal.lock().unwrap().len()
    }

    fn cache_files(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.dir.path().join("images"))
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

fn step(bytes: &[u8]) -> Value {
    json!({"bodyHex": hex(bytes), "headers": [["Content-Type", "image/png"]]})
}

fn key_of(url: &str) -> String {
    let normalized = url::Url::parse(url).unwrap();
    Sha256::digest(normalized.as_str().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn loads_png_jpeg_and_webp_into_the_cache() {
    let env = env(json!({
        "/a.png": [step(&png(640, 480))],
        "/b": [step(&jpeg(1920, 1080, 0xC0))],
        "/c.webp": [step(&webp_lossless(32, 16))],
    }))
    .await;
    let loader = env.loader(|_| {});

    let a = loader.load(&env.url("/a.png")).await.unwrap();
    assert_eq!(
        (a.format, a.width, a.height, a.cached),
        (ImageFormat::Png, 640, 480, false)
    );
    assert_eq!(a.bytes, png(640, 480));
    assert_eq!(std::fs::read(&a.path).unwrap(), a.bytes);
    assert_eq!(a.format.mime(), "image/png");

    let b = loader.load(&env.url("/b")).await.unwrap();
    assert_eq!(
        (b.format, b.width, b.height),
        (ImageFormat::Jpeg, 1920, 1080)
    );
    let c = loader.load(&env.url("/c.webp")).await.unwrap();
    assert_eq!((c.format, c.width, c.height), (ImageFormat::Webp, 32, 16));

    // Файлы кеша: SHA-256 адреса и расширение по содержимому, а не по адресу; временных нет.
    let mut expected = vec![
        format!("{}.png", key_of(&env.url("/a.png"))),
        format!("{}.jpg", key_of(&env.url("/b"))),
        format!("{}.webp", key_of(&env.url("/c.webp"))),
    ];
    expected.sort();
    assert_eq!(env.cache_files(), expected);

    // Запрос: GET с UA лаунчера, без referer и учётных данных.
    let journal = env.journal.lock().unwrap();
    assert_eq!(journal.len(), 3);
    assert_eq!(journal[0]["method"], "GET");
    assert_eq!(journal[0]["headers"]["user-agent"], "HLSR-Launcher");
    assert!(journal[0]["headers"].get("referer").is_none());
    assert!(journal[0]["headers"].get("cookie").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_second_load_comes_from_the_cache() {
    let env = env(json!({"/a.png": [step(&png(10, 20))]})).await;
    let loader = env.loader(|_| {});
    let url = env.url("/a.png");

    let first = loader.load(&url).await.unwrap();
    let second = loader.load(&url).await.unwrap();
    assert!(!first.cached && second.cached);
    assert_eq!(first.bytes, second.bytes);
    assert_eq!(first.path, second.path);
    assert_eq!(env.requests(), 1);

    // Новый загрузчик на той же папке (перезапуск лаунчера) кеш видит.
    let again = env.loader(|_| {}).load(&url).await.unwrap();
    assert!(again.cached);
    assert_eq!(env.requests(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_cache_expires_by_the_loader_clock() {
    let env = env(json!({"/a.png": [step(&png(10, 20))]})).await;
    let loader = env.loader(|options| options.ttl_ms = 10_000);
    let url = env.url("/a.png");

    loader.load(&url).await.unwrap();
    env.clock.advance(10_000);
    assert!(
        loader.load(&url).await.unwrap().cached,
        "ровно срок — ещё свежий"
    );
    assert_eq!(env.requests(), 1);
    env.clock.advance(1);
    assert!(!loader.load(&url).await.unwrap().cached, "срок вышел");
    assert_eq!(env.requests(), 2);
    // Файл снова свежий: время изменения — по часам загрузчика.
    assert!(loader.load(&url).await.unwrap().cached);
    assert_eq!(env.requests(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_damaged_cache_file_is_loaded_again() {
    let env = env(json!({"/a.png": [step(&png(10, 20))]})).await;
    let loader = env.loader(|_| {});
    let url = env.url("/a.png");
    let first = loader.load(&url).await.unwrap();

    std::fs::write(&first.path, b"not an image").unwrap();
    let healed = loader.load(&url).await.unwrap();
    assert!(!healed.cached);
    assert_eq!(healed.bytes, png(10, 20));
    assert_eq!(std::fs::read(&healed.path).unwrap(), png(10, 20));
    assert_eq!(env.requests(), 2);

    // Файл кеша с другим форматом, чем в имени, — тоже повреждён.
    std::fs::write(&first.path, jpeg(5, 5, 0xC0)).unwrap();
    assert!(!loader.load(&url).await.unwrap().cached);
    assert_eq!(env.requests(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_wrong_answers_and_keeps_them_out_of_the_cache() {
    let env = env(json!({
        "/missing": [{"status": 404}],
        "/broken": [{"status": 500, "body": "boom"}],
        "/empty-ok": [{"status": 204, "contentLength": 0}],
        "/html": [{"body": "<!doctype html><html></html>"}],
        "/svg": [{"body": "<svg xmlns=\"http://www.w3.org/2000/svg\"/>", "headers": [["Content-Type", "image/svg+xml"]]}],
        "/gif": [{"bodyHex": hex(b"GIF89a\x01\x00\x01\x00\x00\x00\x00;")}],
        "/nothing": [{"body": ""}],
        "/fake.png": [{"body": "this is text with a png extension"}],
        "/wide": [step(&png(8001, 10))],
        "/tall": [step(&jpeg(10, 8001, 0xC0))],
        "/huge": [step(&png(5657, 5657))],
        "/limit": [step(&png(5656, 5656))],
        "/hang-up": [{"hangUp": true}],
    }))
    .await;
    let loader = env.loader(|_| {});
    let load = |path: &'static str| {
        let url = env.url(path);
        let loader = &loader;
        async move { loader.load(&url).await }
    };

    assert_eq!(load("/missing").await.unwrap_err(), ImageError::Http(404));
    assert_eq!(load("/broken").await.unwrap_err(), ImageError::Http(500));
    assert_eq!(load("/empty-ok").await.unwrap_err(), ImageError::Http(204));
    for path in ["/html", "/svg", "/gif", "/nothing", "/fake.png"] {
        assert_eq!(
            load(path).await.unwrap_err(),
            ImageError::NotAnImage,
            "{path}"
        );
    }
    for path in ["/wide", "/tall", "/huge"] {
        assert_eq!(
            load(path).await.unwrap_err(),
            ImageError::Dimensions,
            "{path}"
        );
    }
    let limit = load("/limit").await.unwrap();
    assert_eq!((limit.width, limit.height), (5656, 5656));
    assert_eq!(load("/hang-up").await.unwrap_err(), ImageError::Network);
    assert_eq!(
        load("/not-found-on-server").await.unwrap_err(),
        ImageError::Http(404)
    );
    // В кеше только годная картинка.
    assert_eq!(
        env.cache_files(),
        vec![format!("{}.png", key_of(&env.url("/limit")))]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn limits_the_size_of_the_file_without_hanging() {
    let big = {
        let mut bytes = png(10, 10);
        bytes.extend(std::iter::repeat_n(0u8, 300));
        bytes
    };
    let env = env(json!({
        "/known": [step(&big)],
        "/chunked": [{"bodyHex": hex(&big), "contentLength": null}],
        "/slow-overflow": [{"bodySize": 20000, "pauseAfter": 500, "pauseMs": 400, "contentLength": null}],
    }))
    .await;
    let loader = env.loader(|options| options.max_bytes = 256);
    let started = Instant::now();
    // Размер известен из заголовка — тело не читается; неизвестен — чтение обрывается на пределе.
    assert_eq!(
        loader.load(&env.url("/known")).await.unwrap_err(),
        ImageError::TooLarge
    );
    assert_eq!(
        loader.load(&env.url("/chunked")).await.unwrap_err(),
        ImageError::TooLarge
    );
    assert_eq!(
        loader.load(&env.url("/slow-overflow")).await.unwrap_err(),
        ImageError::TooLarge
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "ответ больше предела не должен висеть"
    );
    assert!(env.cache_files().is_empty());

    // Предел по умолчанию — 8 МиБ.
    assert_eq!(MAX_IMAGE_BYTES, 8 * 1024 * 1024);
}

#[tokio::test(flavor = "multi_thread")]
async fn follows_redirects_within_the_limit() {
    let env = env(json!({
        "/old.png": [{"status": 302, "headers": [["Location", "/new.png"]]}],
        "/new.png": [step(&png(7, 9))],
        "/loop-a": [{"status": 302, "headers": [["Location", "/loop-b"]]}],
        "/loop-b": [{"status": 302, "headers": [["Location", "/loop-a"]]}],
        "/to-ftp": [{"status": 302, "headers": [["Location", "ftp://127.0.0.1/x.png"]]}],
    }))
    .await;
    let loader = env.loader(|_| {});

    let image = loader.load(&env.url("/old.png")).await.unwrap();
    assert_eq!((image.width, image.height), (7, 9));
    assert_eq!(
        loader.load(&env.url("/loop-a")).await.unwrap_err(),
        ImageError::Network
    );
    assert_eq!(
        loader.load(&env.url("/to-ftp")).await.unwrap_err(),
        ImageError::BadScheme
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn checks_the_address_before_any_request() {
    let env = env(json!({"/a.png": [step(&png(1, 1))]})).await;
    let loader = env.loader(|options| options.hosts = Some(vec!["127.0.0.1".to_owned()]));
    let long = format!("https://127.0.0.1/{}", "x".repeat(2100));
    for (url, expected) in [
        ("not a url", ImageError::InvalidUrl),
        ("", ImageError::InvalidUrl),
        ("/a.png", ImageError::InvalidUrl),
        ("https://", ImageError::InvalidUrl),
        ("ftp://127.0.0.1/a.png", ImageError::BadScheme),
        ("file:///etc/passwd", ImageError::BadScheme),
        ("data:image/png;base64,AA==", ImageError::BadScheme),
        ("javascript:alert(1)", ImageError::BadScheme),
        ("http://example.com/a.png", ImageError::BadScheme),
        ("http://localhost.example/a.png", ImageError::BadScheme),
        ("https://user@127.0.0.1/a.png", ImageError::Credentials),
        ("https://user:pw@127.0.0.1/a.png", ImageError::Credentials),
        ("https://other.example/a.png", ImageError::HostNotAllowed),
        (
            "https://127.0.0.1.evil.example/a.png",
            ImageError::HostNotAllowed,
        ),
        ("https://127.0.0.1/a\nb.png", ImageError::InvalidUrl),
        (long.as_str(), ImageError::InvalidUrl),
    ] {
        assert_eq!(loader.load(url).await.unwrap_err(), expected, "{url:?}");
    }
    assert_eq!(env.requests(), 0);
    // Список хостов не задан — любой хост (CSP `img-src https:` 0.4.7); loopback по http — в тестах.
    assert!(env.loader(|_| {}).load(&env.url("/a.png")).await.is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn identical_addresses_share_one_request() {
    let env = env(json!({"/a.png": [{"bodyHex": hex(&png(3, 3)), "delayMs": 300}]})).await;
    let loader = Arc::new(env.loader(|_| {}));
    let url = env.url("/a.png");

    let tasks: Vec<_> = (0..4)
        .map(|_| {
            let (loader, url) = (loader.clone(), url.clone());
            tokio::spawn(async move { loader.load(&url).await })
        })
        .collect();
    let mut cached = 0;
    for task in tasks {
        let image = task.await.unwrap().unwrap();
        assert_eq!((image.width, image.height), (3, 3));
        cached += usize::from(image.cached);
    }
    assert_eq!(env.requests(), 1);
    assert_eq!(cached, 3, "остальные взяли файл, записанный первым");
}

#[tokio::test(flavor = "multi_thread")]
async fn trims_the_cache_from_the_oldest_file() {
    let routes: Vec<(String, Value)> = (1..=4u32)
        .map(|n| (format!("/{n}.png"), json!([step(&png(n, n))])))
        .collect();
    let env = env(Value::Object(routes.into_iter().collect())).await;
    let size = png(1, 1).len() as u64;
    // Влезает три картинки.
    let loader = env.loader(|options| options.cache_max_bytes = size * 3);

    for n in 1..=4u32 {
        loader.load(&env.url(&format!("/{n}.png"))).await.unwrap();
        env.clock.advance(1000);
    }
    let names = env.cache_files();
    assert_eq!(names.len(), 3, "{names:?}");
    assert!(
        !names.contains(&format!("{}.png", key_of(&env.url("/1.png")))),
        "самая старая вытеснена"
    );
    for n in 2..=4 {
        assert!(names.contains(&format!("{}.png", key_of(&env.url(&format!("/{n}.png"))))));
    }
    // Вытесненная грузится заново.
    assert!(!loader.load(&env.url("/1.png")).await.unwrap().cached);
    // Только что записанная файл не теряет, даже если одна не влезает в предел.
    let tiny = env.loader(|options| options.cache_max_bytes = 1);
    let kept = tiny.load(&env.url("/2.png")).await.unwrap();
    assert!(kept.path.exists());
}

/// Файл в папке кеша с заданным временем изменения (мс по часам загрузчика).
fn put(env: &Env, name: &str, bytes: &[u8], modified_ms: i64) {
    let dir = env.dir.path().join("images");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + Duration::from_millis(modified_ms as u64))
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn eviction_leaves_foreign_files_alone() {
    let routes: Vec<(String, Value)> = (1..=4u32)
        .map(|n| (format!("/{n}.png"), json!([step(&png(n, n))])))
        .collect();
    let env = env(Value::Object(routes.into_iter().collect())).await;
    let size = png(1, 1).len() as u64;
    // Чужие файлы в папке: картинка с «чужим» именем, большой файл, текст и вложенная папка. Они
    // старше всего кеша, не считаются в его размер и вытесняться не должны.
    put(&env, "avatar.png", &png(300, 300), NOW - 1_000_000);
    put(&env, "photo.webp", &vec![0u8; 100_000], NOW - 1_000_000);
    put(&env, "notes.txt", b"mine", NOW - 1_000_000);
    put(
        &env,
        &format!("{}.PNG", "a".repeat(64)),
        b"upper",
        NOW - 1_000_000,
    );
    std::fs::create_dir_all(env.dir.path().join("images").join("subdir.png")).unwrap();
    let loader = env.loader(|options| options.cache_max_bytes = size * 3);

    for n in 1..=4u32 {
        loader.load(&env.url(&format!("/{n}.png"))).await.unwrap();
        env.clock.advance(1000);
    }
    let names = env.cache_files();
    for foreign in ["avatar.png", "photo.webp", "notes.txt", "subdir.png"] {
        assert!(names.contains(&foreign.to_owned()), "{foreign}: {names:?}");
    }
    assert!(names.contains(&format!("{}.PNG", "a".repeat(64))));
    let ours = names
        .iter()
        .filter(|name| {
            name.len() == 68
                && name.ends_with(".png")
                && name[..64].bytes().all(|b| b.is_ascii_hexdigit())
        })
        .count();
    assert_eq!(ours, 3, "своих файлов ровно по пределу: {names:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_temp_files_are_swept_but_a_running_write_is_not() {
    let env = env(json!({"/a.png": [step(&png(10, 20))]})).await;
    let key = key_of(&env.url("/a.png"));
    let other = format!(
        "{}.{}.0.tmp",
        "b".repeat(64),
        std::process::id().wrapping_add(1)
    );
    let crashed = format!("{key}.{}.7.tmp", std::process::id().wrapping_add(1));
    let running = format!("{}.{}.99.tmp", "c".repeat(64), std::process::id());
    let foreign_tmp = "download.tmp";
    for name in [&other, &crashed, &running] {
        put(&env, name, b"half-written", NOW);
    }
    put(&env, foreign_tmp, b"mine", NOW);

    env.loader(|_| {}).load(&env.url("/a.png")).await.unwrap();
    let names = env.cache_files();
    assert!(
        !names.contains(&other) && !names.contains(&crashed),
        "{names:?}"
    );
    assert!(
        names.contains(&running),
        "запись этого процесса идёт: {names:?}"
    );
    assert!(names.contains(&foreign_tmp.to_owned()), "{names:?}");
    assert!(names.contains(&format!("{key}.png")));
}

#[tokio::test(flavor = "multi_thread")]
async fn expired_files_are_swept_on_the_next_write() {
    let env = env(json!({"/new.png": [step(&png(3, 3))]})).await;
    let ttl = 10_000;
    let old_key = "d".repeat(64);
    put(&env, &format!("{old_key}.png"), &png(5, 5), NOW - ttl - 1);
    let fresh_key = "e".repeat(64);
    put(
        &env,
        &format!("{fresh_key}.jpg"),
        &jpeg(5, 5, 0xC0),
        NOW - ttl,
    );

    env.loader(|options| options.ttl_ms = ttl)
        .load(&env.url("/new.png"))
        .await
        .unwrap();
    let names = env.cache_files();
    assert!(
        !names.contains(&format!("{old_key}.png")),
        "просрочен: {names:?}"
    );
    assert!(
        names.contains(&format!("{fresh_key}.jpg")),
        "ровно срок — ещё свеж: {names:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_format_replaces_the_old_file_of_the_same_address() {
    let env = env(json!({"/a": [{"bodyHex": hex(&webp_lossless(32, 16))}]})).await;
    let url = env.url("/a");
    let key = key_of(&url);
    let ttl = 10_000;
    // Прошлый ответ был PNG и устарел: файл другого формата под тем же ключом.
    put(&env, &format!("{key}.png"), &png(8, 8), NOW - ttl - 1);

    let loader = env.loader(|options| options.ttl_ms = ttl);
    let loaded = loader.load(&url).await.unwrap();
    assert_eq!((loaded.format, loaded.cached), (ImageFormat::Webp, false));
    assert_eq!(env.cache_files(), [format!("{key}.webp")]);
    assert_eq!(env.requests(), 1);

    // Устаревший PNG, битый JPEG и свежий WebP одного адреса: годен WebP, лишнее уходит, запросов
    // нет (раньше первый негодный файл прерывал чтение, и картинка грузилась заново).
    put(&env, &format!("{key}.png"), &png(8, 8), NOW - ttl - 1);
    put(&env, &format!("{key}.jpg"), b"damaged", NOW);
    let again = loader.load(&url).await.unwrap();
    assert!(again.cached);
    assert_eq!(again.format, ImageFormat::Webp);
    assert_eq!(env.requests(), 1);
    assert_eq!(env.cache_files(), [format!("{key}.webp")]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_silent_server_ends_with_a_timeout_not_a_hang() {
    let env = env(json!({
        "/silent": [{"delayMs": 30000, "bodyHex": hex(&png(1, 1))}],
        "/stalls": [{"bodyHex": hex(&png(1, 1)), "pauseAfter": 10, "pauseMs": 30000, "contentLength": null}],
    }))
    .await;
    let loader = env.loader(|options| options.timeout = Duration::from_millis(300));
    let started = Instant::now();
    assert_eq!(
        loader.load(&env.url("/silent")).await.unwrap_err(),
        ImageError::Timeout
    );
    assert_eq!(
        loader.load(&env.url("/stalls")).await.unwrap_err(),
        ImageError::Network
    );
    // Сервер молчит 30 с: тайм-аут (0,3 с) обязан сработать намного раньше.
    assert!(started.elapsed() < Duration::from_secs(15));
    assert!(env.cache_files().is_empty());
}
