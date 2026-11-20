//! Выпуск без `dev-hooks`: загрузчик картинок принимает только `https:` — `http:` не проходит даже
//! на loopback (как транспорт API, спецификация, решение 2). Запуск: `cargo test -p hlsr-online
//! --test image_loader_release --config profile.dev.debug-assertions=false` (в отладочной сборке
//! файл пуст).
#![cfg(not(any(debug_assertions, feature = "dev-hooks")))]

use hlsr_online::image_loader::{ImageError, ImageLoader, ImageLoaderOptions};

#[tokio::test]
async fn plain_http_is_refused_even_on_loopback() {
    let dir = tempfile::tempdir().unwrap();
    let loader = ImageLoader::new(dir.path().join("images"), ImageLoaderOptions::default());
    for url in [
        "http://127.0.0.1:9/a.png",
        "http://localhost:9/a.png",
        "http://example.test/a.png",
    ] {
        assert_eq!(
            loader.load(url).await.unwrap_err(),
            ImageError::BadScheme,
            "{url}"
        );
    }
    // https на loopback схему и адрес проходит: дальше обычный отказ соединения.
    assert_eq!(
        loader.load("https://127.0.0.1:9/a.png").await.unwrap_err(),
        ImageError::Network
    );
    assert!(!dir.path().join("images").exists());
}
