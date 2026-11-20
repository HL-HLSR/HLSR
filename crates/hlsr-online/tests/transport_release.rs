//! Выпуск без `dev-hooks` (спецификация, решение 2): `http:` не принимается даже на loopback —
//! в 0.4.7 такой ход был QA-подменой. Запуск: `cargo test -p hlsr-online --test transport_release
//! --config profile.dev.debug-assertions=false` (в отладочной сборке файл пуст).
#![cfg(not(any(debug_assertions, feature = "dev-hooks")))]

use hlsr_online::telemetry::transport::{HttpTransport, RequestOptions, TransportError};

#[tokio::test]
async fn plain_http_is_refused_even_on_loopback() {
    let transport = HttpTransport::new();
    let options = RequestOptions::default();
    for url in [
        "http://127.0.0.1:9/",
        "http://localhost:9/",
        "http://example.test/",
    ] {
        assert_eq!(
            transport.request(url, &options).await.unwrap_err(),
            TransportError::Insecure,
            "{url}"
        );
    }
    // https на loopback схему и адрес проходит: дальше обычный отказ соединения.
    assert_eq!(
        transport
            .request("https://127.0.0.1:9/", &options)
            .await
            .unwrap_err(),
        TransportError::Network
    );
    assert_eq!(
        transport
            .request("ftp://127.0.0.1/", &options)
            .await
            .unwrap_err(),
        TransportError::BadScheme
    );
}
