//! Запросы Proton и Python к GitHub (`request` и `readText` из `ProtonManager.js`,
//! `request` из `PythonRuntime.js`, повтор метаданных при TLS-ошибке из `net-fallback.js`) на
//! локальном сервере: проверенный адрес отправляется на `127.0.0.1` по HTTP, а хост, схема и
//! редиректы проверяются по исходному адресу, как в 0.4.7. Интернет не нужен.

use hlsr_delivery::providers::BoxFuture;
use hlsr_platform::linux::abort::Abort;
use hlsr_platform::linux::prefix_command::ProtonError;
use hlsr_platform::linux::proton_manager::{
    Body, Fetch, GithubFetch, GithubText, MemoryBody, ReadText, proton_rules, read_limited,
};
use hlsr_platform::linux::python_runtime;
use hlsr_platform::net_fallback::{FallbackGet, FallbackResponse};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

/// Ответ сервера: код, заголовки, тело; `None` — не отвечать вовсе.
type Reply = Option<(u16, Vec<(String, String)>, Vec<u8>)>;

/// Запрос, который увидел сервер: путь (с хостом исходного адреса) и заголовки.
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    headers: HashMap<String, String>,
}

struct Server {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Server {
    async fn start(handler: impl Fn(&str) -> Reply + Send + Sync + 'static) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let handler = Arc::new(handler);
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let handler = handler.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut byte = [0u8; 1];
                    while !head.ends_with(b"\r\n\r\n") {
                        if socket.read(&mut byte).await.unwrap_or(0) == 0 {
                            return;
                        }
                        head.push(byte[0]);
                    }
                    let text = String::from_utf8_lossy(&head).into_owned();
                    let mut lines = text.split("\r\n");
                    let path = lines.next().unwrap().split(' ').nth(1).unwrap().to_owned();
                    let headers = lines
                        .filter_map(|line| line.split_once(": "))
                        .map(|(k, v)| (k.to_ascii_lowercase(), v.to_owned()))
                        .collect();
                    log.lock().unwrap().push(Seen {
                        path: path.clone(),
                        headers,
                    });
                    let Some((status, headers, body)) = handler(&path) else {
                        // Не отвечать: соединение висит, пока клиент не бросит его.
                        let mut sink = [0u8; 16];
                        let _ = socket.read(&mut sink).await;
                        std::future::pending::<()>().await;
                        return;
                    };
                    let mut out = format!(
                        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n",
                        body.len()
                    );
                    for (name, value) in headers {
                        out.push_str(&format!("{name}: {value}\r\n"));
                    }
                    out.push_str("\r\n");
                    let _ = socket.write_all(out.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Server { port, seen }
    }

    /// Проверенный адрес → `http://127.0.0.1:<порт>/<хост><путь>`.
    fn fetch(&self, rules: hlsr_platform::linux::proton_manager::FetchRules) -> GithubFetch {
        let port = self.port;
        GithubFetch::new(rules).with_route(Arc::new(move |url: &url::Url| {
            let query = url.query().map(|q| format!("?{q}")).unwrap_or_default();
            url::Url::parse(&format!(
                "http://127.0.0.1:{port}/{}{}{query}",
                url.host_str().unwrap(),
                url.path()
            ))
            .unwrap()
        }))
    }

    fn paths(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|s| s.path.clone())
            .collect()
    }
}

fn ok(body: &str) -> Reply {
    Some((200, Vec::new(), body.as_bytes().to_vec()))
}

fn redirect(location: &str) -> Reply {
    Some((
        302,
        vec![("Location".to_owned(), location.to_owned())],
        Vec::new(),
    ))
}

async fn text_of(fetch: &GithubFetch, url: &str) -> Result<String, ProtonError> {
    let mut body = fetch.request(url, None).await?;
    let bytes = read_limited(body.as_mut(), 1 << 20, "слишком велик", None).await?;
    Ok(String::from_utf8(bytes).unwrap())
}

#[tokio::test]
async fn github_request_sends_launcher_headers_and_reads_the_body() {
    let server = Server::start(|_| ok("[]")).await;
    let fetch = server.fetch(proton_rules());
    let text = text_of(
        &fetch,
        "https://api.github.com/repos/x/y/releases?per_page=20",
    )
    .await;
    assert_eq!(text.unwrap(), "[]");
    let seen = server.seen.lock().unwrap()[0].clone();
    assert_eq!(seen.path, "/api.github.com/repos/x/y/releases?per_page=20");
    assert_eq!(seen.headers["user-agent"], "HLSR-Linux");
    assert_eq!(seen.headers["accept"], "application/vnd.github+json");
}

#[tokio::test]
async fn redirects_are_followed_from_the_current_address_and_checked() {
    let server = Server::start(|path| match path.split('?').next().unwrap() {
        "/github.com/a" => redirect("/b?x=1"),
        "/github.com/b" => redirect("https://release-assets.githubusercontent.com/c"),
        "/release-assets.githubusercontent.com/c" => ok("archive"),
        "/github.com/evil" => redirect("https://evil.example/x"),
        "/github.com/plain" => redirect("http://github.com/x"),
        "/github.com/no-location" => Some((302, Vec::new(), Vec::new())),
        "/github.com/missing" => Some((404, Vec::new(), b"nope".to_vec())),
        other if other.starts_with("/github.com/loop") => {
            let n: u32 = other
                .trim_start_matches("/github.com/loop")
                .parse()
                .unwrap();
            if n == 0 {
                ok("end")
            } else {
                redirect(&format!("/loop{}", n - 1))
            }
        }
        _ => Some((500, Vec::new(), Vec::new())),
    })
    .await;
    let fetch = server.fetch(proton_rules());
    assert_eq!(
        text_of(&fetch, "https://github.com/a").await.unwrap(),
        "archive"
    );
    assert_eq!(
        server.paths(),
        [
            "/github.com/a",
            "/github.com/b?x=1",
            "/release-assets.githubusercontent.com/c"
        ]
    );
    let message = |result: Result<String, ProtonError>| result.unwrap_err().message;
    assert_eq!(
        message(text_of(&fetch, "https://github.com/evil").await),
        "Недоверенный адрес загрузки Proton"
    );
    assert_eq!(
        message(text_of(&fetch, "https://github.com/plain").await),
        "Недоверенный адрес загрузки Proton"
    );
    assert_eq!(
        message(text_of(&fetch, "https://github.com/no-location").await),
        "GitHub вернул перенаправление без адреса"
    );
    assert_eq!(
        message(text_of(&fetch, "https://github.com/missing").await),
        "GitHub ответил 404"
    );
    // Пять редиректов — можно, шестой — нет.
    assert_eq!(
        text_of(&fetch, "https://github.com/loop5").await.unwrap(),
        "end"
    );
    assert_eq!(
        message(text_of(&fetch, "https://github.com/loop6").await),
        "Недоверенный адрес загрузки Proton"
    );

    let python = server.fetch(python_runtime::fetch_rules());
    assert_eq!(
        message(text_of(&python, "https://github.com/no-location").await),
        "Загрузка Python: HTTP 302"
    );
    assert_eq!(
        message(text_of(&python, "https://github.com/missing").await),
        "Загрузка Python: HTTP 404"
    );
    assert_eq!(
        message(text_of(&python, "https://api.github.com/x").await),
        "Недоверенный адрес загрузки Python",
        "API GitHub Python не нужен"
    );
}

#[tokio::test]
async fn untrusted_addresses_are_refused_without_a_request() {
    let server = Server::start(|_| ok("x")).await;
    let fetch = server.fetch(proton_rules());
    for url in [
        "http://github.com/x",
        "https://example.com/x",
        "https://github.com.evil/x",
        "https://127.0.0.1/x",
    ] {
        let error = text_of(&fetch, url).await.unwrap_err();
        assert_eq!(error.message, "Недоверенный адрес загрузки Proton", "{url}");
    }
    let error = text_of(&fetch, "not a url").await.unwrap_err();
    assert_eq!(error.code.as_deref(), Some("ERR_INVALID_URL"));
    assert!(server.paths().is_empty());
}

#[tokio::test]
async fn silent_server_times_out_and_abort_stops_the_wait() {
    let server = Server::start(|_| None).await;
    let fetch = server
        .fetch(proton_rules())
        .with_timeout(Duration::from_millis(200));
    let error = text_of(&fetch, "https://github.com/x").await.unwrap_err();
    assert_eq!(error.message, "Тайм-аут загрузки Proton");

    let slow = server.fetch(proton_rules());
    let abort = Abort::new();
    let stop = abort.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        stop.abort();
    });
    let error = match slow.request("https://github.com/x", Some(&abort)).await {
        Err(error) => error,
        Ok(_) => panic!("ответа не было"),
    };
    assert!(error.is_abort());
}

#[tokio::test]
async fn read_text_limits_the_answer() {
    let server = Server::start(|_| ok(&"x".repeat(100))).await;
    let text = GithubText::new(Arc::new(server.fetch(proton_rules())), Arc::new(NoFallback));
    let url = "https://api.github.com/x";
    assert_eq!(
        text.read_text(url, Some(100), None).await.unwrap().len(),
        100
    );
    let error = text.read_text(url, Some(99), None).await.unwrap_err();
    assert_eq!(error.message, "Ответ GitHub слишком велик");
}

// ---------------------------------------------------------------- повтор при TLS-ошибке

struct NoFallback;

impl FallbackGet for NoFallback {
    fn get<'a>(
        &'a self,
        _url: &'a str,
        _headers: &'a [(&'a str, &'a str)],
        _limit: usize,
        _abort: Option<&'a Abort>,
    ) -> BoxFuture<'a, Result<FallbackResponse, ProtonError>> {
        panic!("повтор не нужен");
    }
}

/// Основной путь всегда падает заданной ошибкой.
struct Failing(ProtonError);

impl Fetch for Failing {
    fn request<'a>(
        &'a self,
        _url: &'a str,
        _abort: Option<&'a Abort>,
    ) -> BoxFuture<'a, Result<Box<dyn Body>, ProtonError>> {
        let error = self.0.clone();
        Box::pin(async move { Err::<Box<dyn Body>, _>(error) })
    }
}

/// Запрос повтора: адрес, заголовки, предел.
type FallbackCall = (String, Vec<(String, String)>, usize);

/// Повтор отвечает заданным ответом и помнит запросы.
struct Fallback {
    response: FallbackResponse,
    calls: Mutex<Vec<FallbackCall>>,
}

impl FallbackGet for Fallback {
    fn get<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(&'a str, &'a str)],
        limit: usize,
        _abort: Option<&'a Abort>,
    ) -> BoxFuture<'a, Result<FallbackResponse, ProtonError>> {
        self.calls.lock().unwrap().push((
            url.to_owned(),
            headers
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            limit,
        ));
        Box::pin(std::future::ready(Ok(self.response.clone())))
    }
}

fn fallback(status: u16, url: &str, body: Option<&str>) -> Arc<Fallback> {
    Arc::new(Fallback {
        response: FallbackResponse {
            status,
            url: url.to_owned(),
            body: body.map(|b| b.as_bytes().to_vec()),
            truncated: body.is_none(),
        },
        calls: Mutex::default(),
    })
}

#[tokio::test]
async fn tls_failure_of_metadata_is_retried_with_system_trust() {
    let url = "https://api.github.com/repos/x/releases?per_page=20";
    let tls = || {
        Arc::new(Failing(ProtonError::with_code(
            "UNABLE_TO_GET_ISSUER_CERT_LOCALLY",
            "unable to get local issuer certificate",
        )))
    };
    let good = fallback(200, url, Some("[1]"));
    let text = GithubText::new(tls(), good.clone());
    assert_eq!(text.read_text(url, None, None).await.unwrap(), "[1]");
    let calls = good.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, url);
    assert_eq!(
        calls[0].1,
        [
            ("User-Agent".to_owned(), "HLSR-Linux".to_owned()),
            (
                "Accept".to_owned(),
                "application/vnd.github+json".to_owned()
            )
        ]
    );
    assert_eq!(calls[0].2, 2 * 1024 * 1024);

    let cases = [
        (
            fallback(200, "https://evil.example/x", Some("[]")),
            "Недоверенный адрес загрузки Proton",
        ),
        (fallback(200, url, None), "Ответ GitHub слишком велик"),
        (fallback(500, url, Some("")), "GitHub ответил 500"),
    ];
    for (response, message) in cases {
        let error = GithubText::new(tls(), response)
            .read_text(url, Some(64 * 1024), None)
            .await
            .unwrap_err();
        assert_eq!(error.message, message);
    }

    // Не TLS — не повторяется.
    let reset = GithubText::new(
        Arc::new(Failing(ProtonError::with_code(
            "ECONNRESET",
            "socket hang up",
        ))),
        Arc::new(NoFallback),
    );
    assert_eq!(
        reset.read_text(url, None, None).await.unwrap_err().message,
        "socket hang up"
    );
    // Хост не GitHub — не повторяется.
    let foreign = GithubText::new(tls(), Arc::new(NoFallback));
    assert!(
        foreign
            .read_text("https://example.com/x", None, None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn memory_body_reads_in_chunks() {
    let mut body = MemoryBody::new([b"ab".to_vec(), b"cd".to_vec()]);
    let bytes = read_limited(&mut body, 4, "много", None).await.unwrap();
    assert_eq!(bytes, b"abcd");
    let mut body = MemoryBody::new([b"ab".to_vec(), b"cd".to_vec()]);
    assert_eq!(
        read_limited(&mut body, 3, "много", None)
            .await
            .unwrap_err()
            .message,
        "много"
    );
}
