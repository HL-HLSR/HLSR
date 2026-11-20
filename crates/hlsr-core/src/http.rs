//! Общая настройка HTTP и TLS 0.4.7 (Р7 подпроекта 6): один построитель клиента `reqwest` для
//! доставки (`hlsr-delivery`), онлайна, платформы и обновления (`hlsr-update`), чтобы у них не
//! расходились проверка сертификатов и тайм-ауты соединения. `hlsr-update` от `hlsr-delivery` не
//! зависит, поэтому построитель живёт здесь; `hlsr_delivery::http_client` его реэкспортирует.

use std::sync::Arc;
use std::time::Duration;

/// Соединение (DNS и TCP): `timeout: 5000` у `http.globalAgent` и `https.globalAgent` Node 24.
/// Пока сокет соединяется, действует он, а `request.setTimeout(timeout)` — только после
/// соединения; сработав, он тоже даёт `NETWORK_TIMEOUT`. В reqwest этот тайм-аут накрывает и
/// рукопожатие TLS (у Node оно идёт под тайм-аутом запроса).
pub const CONNECT_TIMEOUT_MS: u64 = 5_000;

/// `isLoopback(hostname)`; `hostname` — как у `URL` (IPv6 в скобках: `[::1]` — не loopback).
pub fn is_loopback(hostname: &str) -> bool {
    matches!(hostname, "127.0.0.1" | "localhost" | "::1")
}

/// Построитель клиента `reqwest` с общей настройкой HTTP и TLS 0.4.7: HTTP/1.1 без сжатия и
/// системного прокси, без редиректов (их ведёт вызывающий), соединение не дольше 5 с
/// ([`CONNECT_TIMEOUT_MS`]), TLS — rustls с ring и корнями Mozilla.
pub fn client_builder() -> reqwest::ClientBuilder {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring поддерживает TLS 1.2 и 1.3")
        .with_root_certificates(roots)
        .with_no_client_auth();
    reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_millis(CONNECT_TIMEOUT_MS))
        .redirect(reqwest::redirect::Policy::none())
        .http1_only()
        .tls_backend_preconfigured(tls)
}
