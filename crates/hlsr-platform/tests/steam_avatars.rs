//! Аватары Steam против 0.4.7: корпуса чистых функций и сценарии `fetchAvatar`
//! (`tools/reference/platform/steam-avatars.js` → `steam-avatars.json`). Сценарии идут по
//! настоящему коду запросов: адрес `https://<хост>/<путь>` уходит на локальный сервер 127.0.0.1
//! (`WebGet::with_remap` — подмена хостов только здесь), а проверки хоста, редиректы, пределы
//! размера и разбор XML выполняет сам модуль.

#[path = "support/blobs.rs"]
mod blobs;
mod support;

use hlsr_platform::profile::steam_avatars::{
    HttpGet, IMAGE_HOST_SUFFIX, IMAGE_HOSTS, MAX_PROFILE_BYTES, PROFILE_HOST, WebGet,
    avatar_url_from, image_type_of, is_allowed_image_host,
};
use serde_json::json;
use support::{Mismatches, fixture, items, str_of};

fn from_hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
        .collect()
}

#[test]
fn constants_like_0_4_7() {
    let fixture = fixture("steam-avatars.json");
    assert_eq!(str_of(&fixture["constants"]["PROFILE_HOST"]), PROFILE_HOST);
    assert_eq!(fixture["constants"]["IMAGE_HOSTS"], json!(IMAGE_HOSTS));
    assert_eq!(
        str_of(&fixture["constants"]["IMAGE_HOST_SUFFIX"]),
        IMAGE_HOST_SUFFIX
    );
    assert_eq!(MAX_PROFILE_BYTES, 256 * 1024);
}

#[test]
fn avatar_url_from_like_0_4_7() {
    let fixture = fixture("steam-avatars.json");
    let cases = items(&fixture["avatarUrlFrom"]);
    assert!(cases.len() >= 30);
    let mut mismatches = Mismatches::default();
    for case in cases {
        // `String(xml || "")`: не строка — ничего похожего на XML.
        let xml = case["input"].as_str().unwrap_or("");
        mismatches.check(
            &json!(avatar_url_from(xml)),
            &case["result"],
            format!("avatarUrlFrom({:?})", case["input"]),
        );
    }
    mismatches.finish("avatarUrlFrom");
}

#[test]
fn image_type_of_like_0_4_7() {
    let fixture = fixture("steam-avatars.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["imageTypeOf"]) {
        let hex = str_of(&case["hex"]);
        mismatches.check(
            &json!(image_type_of(&from_hex(hex))),
            &case["result"],
            format!("imageTypeOf({hex})"),
        );
    }
    mismatches.finish("imageTypeOf");
}

#[test]
fn is_allowed_image_host_like_0_4_7() {
    let fixture = fixture("steam-avatars.json");
    let mut mismatches = Mismatches::default();
    for case in items(&fixture["isAllowedImageHost"]) {
        let host = case["host"].as_str().unwrap_or("");
        mismatches.check_eq(
            is_allowed_image_host(host),
            case["result"].as_bool().unwrap(),
            format!("isAllowedImageHost({:?})", case["host"]),
        );
    }
    mismatches.finish("isAllowedImageHost");
}

// --- локальный сервер сценария: подмена адреса (`WebGet::with_remap`) есть только в отладочной
// сборке и с `dev-hooks` — в выпуске без них эти сценарии не собираются.

#[cfg(any(debug_assertions, feature = "dev-hooks"))]
mod served {
    use super::*;
    use blobs::{bytes_of, digest, sha256_hex};
    use hlsr_core::js::Platform;
    use hlsr_platform::profile::steam_avatars::{FetchOptions, fetch_avatar};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;
    use url::Url;

    #[derive(Debug, Clone, PartialEq)]
    struct Seen {
        host: String,
        path: String,
        accept: Option<String>,
        user_agent: Option<String>,
    }

    struct Server {
        port: u16,
        seen: Arc<Mutex<Vec<Seen>>>,
    }

    fn reason(status: u64) -> &'static str {
        match status {
            200 => "OK",
            204 => "No Content",
            300 => "Multiple Choices",
            301 => "Moved Permanently",
            302 => "Found",
            303 => "See Other",
            307 => "Temporary Redirect",
            308 => "Permanent Redirect",
            404 => "Not Found",
            500 => "Internal Server Error",
            _ => "Status",
        }
    }

    /// Сервер отвечает по таблице маршрутов сценария. Адрес запроса — `/@<хост><путь>`.
    async fn serve(routes: Vec<Value>) -> Server {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let routes = routes.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    let (read, mut write) = stream.into_split();
                    let mut lines = BufReader::new(read).lines();
                    let Ok(Some(request)) = lines.next_line().await else {
                        return;
                    };
                    let target = request.split(' ').nth(1).unwrap_or("/").to_owned();
                    let (mut accept, mut user_agent) = (None, None);
                    while let Ok(Some(line)) = lines.next_line().await {
                        if line.is_empty() {
                            break;
                        }
                        if let Some((name, value)) = line.split_once(':') {
                            match name.to_ascii_lowercase().as_str() {
                                "accept" => accept = Some(value.trim().to_owned()),
                                "user-agent" => user_agent = Some(value.trim().to_owned()),
                                _ => {}
                            }
                        }
                    }
                    let rest = target.strip_prefix("/@").unwrap_or(&target);
                    let (host, path) = match rest.find('/') {
                        Some(at) => (rest[..at].to_owned(), rest[at..].to_owned()),
                        None => (rest.to_owned(), "/".to_owned()),
                    };
                    log.lock().unwrap().push(Seen {
                        host: host.clone(),
                        path: path.clone(),
                        accept,
                        user_agent,
                    });
                    let route = routes
                        .iter()
                        .find(|route| route["host"] == json!(host) && route["path"] == json!(path));
                    let (status, headers, body) = match route {
                        Some(route) => (
                            route["status"].as_u64().unwrap(),
                            route["headers"].as_object().cloned().unwrap_or_default(),
                            bytes_of(&route["body"]),
                        ),
                        None => (599, Default::default(), Vec::new()),
                    };
                    let mut head = format!("HTTP/1.1 {status} {}\r\n", reason(status));
                    for (name, value) in headers {
                        head.push_str(&format!("{name}: {}\r\n", value.as_str().unwrap()));
                    }
                    head.push_str(&format!(
                        "Content-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    ));
                    let _ = write.write_all(head.as_bytes()).await;
                    let _ = write.write_all(&body).await;
                    let _ = write.shutdown().await;
                });
            }
        });
        Server { port, seen }
    }

    fn web_get(port: u16) -> Arc<dyn HttpGet> {
        Arc::new(WebGet::new().with_remap(Arc::new(move |url: &Url| {
            let mut mapped = Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap();
            let host = match url.port() {
                Some(port) => format!("{}:{port}", url.host_str().unwrap_or("")),
                None => url.host_str().unwrap_or("").to_owned(),
            };
            mapped.set_path(&format!("/@{host}{}", url.path()));
            mapped.set_query(url.query());
            mapped
        })))
    }

    fn seen_value(seen: &[Seen]) -> Value {
        Value::Array(
            seen.iter()
                .map(|item| {
                    json!({
                        "host": item.host,
                        "path": item.path,
                        "accept": item.accept,
                        "userAgent": item.user_agent,
                    })
                })
                .collect(),
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fetch_avatar_like_0_4_7() {
        let fixture = fixture("steam-avatars.json");
        let scenarios = items(&fixture["fetch"]);
        assert!(scenarios.len() >= 54, "сценариев: {}", scenarios.len());
        let mut mismatches = Mismatches::default();

        for scenario in scenarios {
            let name = str_of(&scenario["name"]);
            let id = match &scenario["id"] {
                Value::String(text) => text.clone(),
                other => other["number"].to_string(),
            };
            let root = tempfile::tempdir().unwrap();
            let mut cache_dir = root.path().join("avatar-cache");
            if scenario["cacheDirIsFile"] == json!(true) {
                cache_dir = root.path().join("blocker");
                std::fs::write(&cache_dir, "x").unwrap();
            } else if scenario["cacheAsDirectory"] == json!(true) {
                std::fs::create_dir_all(cache_dir.join(format!("{id}.bin"))).unwrap();
            } else if !scenario["cache"].is_null() {
                std::fs::create_dir_all(&cache_dir).unwrap();
                std::fs::write(
                    cache_dir.join(format!("{id}.bin")),
                    bytes_of(&scenario["cache"]),
                )
                .unwrap();
            }

            let server = serve(items(&scenario["routes"]).clone()).await;
            let options = FetchOptions {
                cache_dir: (scenario["noCacheDir"] != json!(true))
                    .then(|| cache_dir.to_string_lossy().into_owned()),
                get: Some(web_get(server.port)),
                platform: Platform::CURRENT,
            };
            let result = fetch_avatar(&id, &options).await;
            let seen = server.seen.lock().unwrap().clone();

            mismatches.check(
                &digest(result.as_deref()),
                &scenario["result"],
                format!("«{name}»: результат"),
            );
            mismatches.check(
                &seen_value(&seen),
                &scenario["requests"],
                format!("«{name}»: запросы"),
            );

            let mut files = serde_json::Map::new();
            if scenario["noCacheDir"] != json!(true)
                && scenario["cacheDirIsFile"] != json!(true)
                && cache_dir.exists()
            {
                let mut names: Vec<_> = std::fs::read_dir(&cache_dir)
                    .unwrap()
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect();
                names.sort();
                for file in names {
                    let full = cache_dir.join(&file);
                    files.insert(
                        file,
                        if full.is_dir() {
                            Value::Null
                        } else {
                            let bytes = std::fs::read(&full).unwrap();
                            json!({"length": bytes.len(), "sha256": sha256_hex(&bytes)})
                        },
                    );
                }
            }
            mismatches.check(
                &Value::Object(files),
                &scenario["cacheFiles"],
                format!("«{name}»: кеш"),
            );
        }
        mismatches.finish("fetchAvatar");
    }

    // --- вне сценариев Node

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_silent_server_hits_the_idle_timeout() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                // Принимаем и молчим.
                held.push(stream);
            }
        });
        let get = Arc::new(
            WebGet::new()
                .with_timeout(Duration::from_millis(300))
                .with_remap(Arc::new(move |_: &Url| {
                    Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap()
                })),
        );
        let options = FetchOptions {
            cache_dir: None,
            get: Some(get),
            platform: Platform::CURRENT,
        };
        let started = std::time::Instant::now();
        assert_eq!(fetch_avatar("76561197960265828", &options).await, None);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_body_that_stalls_after_the_headers_is_dropped() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buffer = [0u8; 2048];
                    let _ = stream.read(&mut buffer).await;
                    let _ = stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n<profile>")
                        .await;
                    tokio::time::sleep(Duration::from_secs(30)).await;
                });
            }
        });
        let get = Arc::new(
            WebGet::new()
                .with_timeout(Duration::from_millis(300))
                .with_remap(Arc::new(move |_: &Url| {
                    Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap()
                })),
        );
        let options = FetchOptions {
            cache_dir: None,
            get: Some(get),
            platform: Platform::CURRENT,
        };
        assert_eq!(fetch_avatar("76561197960265828", &options).await, None);
    }
}

#[tokio::test]
async fn a_real_web_get_refuses_http_and_foreign_hosts_without_a_request() {
    // Без подмены: адрес не https или хост чужой — отказ до всякого запроса (сети здесь нет).
    let get = WebGet::new();
    let options = hlsr_platform::profile::steam_avatars::GetOptions {
        allow_host: Some(is_allowed_image_host),
        ..Default::default()
    };
    assert_eq!(
        get.get("http://steamcdn-a.akamaihd.net/a.jpg", options)
            .await,
        None
    );
    assert_eq!(
        get.get("https://evil.example.com/a.jpg", options).await,
        None
    );
    assert_eq!(get.get("not a url", options).await, None);
    assert_eq!(
        get.get("ftp://steamcdn-a.akamaihd.net/a.jpg", options)
            .await,
        None
    );
}
