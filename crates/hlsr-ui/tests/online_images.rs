//! Порт сети загрузчика картинок в приложении: загрузчик ПП4 (`hlsr_online::image_loader`) на
//! своём сервере `127.0.0.1` (в отладочной сборке `http:` на loopback разрешён). Картинка
//! приходит, уменьшается и попадает в кеш на диске; второй раз — без запроса; код ошибки
//! загрузчика доходит до запроса как есть.

use hlsr_online::image_loader::{ImageLoader as DiskLoader, ImageLoaderOptions};
use hlsr_ui::images::{
    Dispatch, Fit, ImageError, ImageLoader, ImageRequest, ImageSource, OnlineImages,
};
use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Job = Box<dyn FnOnce() + Send>;
/// Размер готовой картинки или ошибка загрузки.
type Got = Result<(u32, u32), ImageError>;

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

fn pump_until(rx: &Receiver<Job>, done: impl Fn() -> bool) {
    while !done() {
        let job = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("картинка не пришла за 10 с");
        job();
    }
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(width, height, image::Rgba([0, 128, 255, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// Сервер на 127.0.0.1: `/a.png` — картинка, остальное — 404. Считает запросы.
fn serve(body: Vec<u8>) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            counter.fetch_add(1, Ordering::SeqCst);
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            let _ = reader.read_line(&mut request);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            let response = if request.starts_with("GET /a.png ") {
                let mut head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .into_bytes();
                head.extend_from_slice(&body);
                head
            } else {
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
            };
            let _ = stream.write_all(&response);
        }
    });
    (base, hits)
}

#[test]
fn remote_images_come_through_the_online_loader_and_its_disk_cache() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let dir = std::env::temp_dir().join(format!("hlsr-ui-online-images-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (base, hits) = serve(png(64, 32));
    let disk = Arc::new(DiskLoader::new(&dir, ImageLoaderOptions::default()));
    let remote = Rc::new(OnlineImages::new(disk, runtime.handle().clone()));
    let (dispatch, rx) = queue();
    let loader = ImageLoader::with_dispatch(remote, dispatch);

    let url = format!("{base}/a.png");
    let request = ImageRequest::new(ImageSource::Remote(url.clone()), "home").with_target(
        32,
        16,
        Fit::Contain,
    );
    let got: Rc<RefCell<Option<Got>>> = Rc::default();
    let sink = got.clone();
    loader.request(request.clone(), move |result| {
        *sink.borrow_mut() = Some(result.map(|image| (image.width(), image.height())));
    });
    pump_until(&rx, || got.borrow().is_some());
    assert_eq!(got.borrow_mut().take(), Some(Ok((32, 16))));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    let cached: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(cached.len(), 1, "{cached:?}");
    assert!(cached[0].ends_with(".png"), "{cached:?}");

    // Кеш интерфейса пуст — картинка берётся из кеша на диске, без запроса.
    loader.clear();
    let sink = got.clone();
    loader.request(request, move |result| {
        *sink.borrow_mut() = Some(result.map(|image| (image.width(), image.height())));
    });
    pump_until(&rx, || got.borrow().is_some());
    assert_eq!(got.borrow_mut().take(), Some(Ok((32, 16))));
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    // Нет файла: код загрузчика ПП4.
    let missing = ImageRequest::new(ImageSource::Remote(format!("{base}/b.png")), "home");
    let sink = got.clone();
    loader.request(missing, move |result| {
        *sink.borrow_mut() = Some(result.map(|image| (image.width(), image.height())));
    });
    pump_until(&rx, || got.borrow().is_some());
    assert_eq!(
        got.borrow_mut().take(),
        Some(Err(ImageError::Remote("IMAGE_HTTP_404".into())))
    );
    drop(loader);
    let _ = std::fs::remove_dir_all(&dir);
}
