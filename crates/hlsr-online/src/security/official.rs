//! «Официальная сборка» для окна (спонсорство, Telegram, вход и профили, «Главная», счётчики
//! онлайна): ключ аттестации вшит (`BuildKey::available`) и сервер хотя бы раз принял билет.
//! Отказ `CLIENT_MODIFIED`, `ATTEST_REQUIRED` или `CLIENT_UNVERIFIED` — сборка не официальная, пока
//! сервер снова не примет билет.
//!
//! Публичные запросы (онлайн, лента, спонсоры, заготовки подсветки) идут через
//! [`AttestedTransport`]: те же `X-HLSR-Build` и `X-HLSR-Ticket`, что у профиля
//! (`build_header_from`, `ticket_provider_from` — одна аттестация на оба), и по их ответам он ведёт
//! [`Official`].

use crate::online_profile::online_profile_service::{BuildHeader, TicketProvider};
use crate::telemetry::transport::{RequestOptions, Response, Transport, TransportError};
use hlsr_core::json_text::json_parse;
use hlsr_delivery::providers::BoxFuture;
use serde_json::Value;
use std::sync::{Arc, Mutex};

/// Отказы сервера, после которых сборка не официальная.
const REFUSALS: [&str; 3] = ["CLIENT_MODIFIED", "ATTEST_REQUIRED", "CLIENT_UNVERIFIED"];

/// Что сервер сказал о билете публичного запроса.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Ответа с билетом ещё не было.
    Unknown,
    Accepted,
    Refused,
}

/// Сборка для окна.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// Ключ есть, сервер ещё не ответил: ничего не показывать, но и заглушку тоже.
    Pending,
    Official,
    /// Ключа нет или сервер отказал: заглушка «в официальном лаунчере».
    Unofficial,
}

impl Standing {
    /// Значение события `build-official`.
    pub fn as_str(self) -> &'static str {
        match self {
            Standing::Pending => "pending",
            Standing::Official => "official",
            Standing::Unofficial => "unofficial",
        }
    }

    pub fn parse(text: &str) -> Option<Standing> {
        [Standing::Pending, Standing::Official, Standing::Unofficial]
            .into_iter()
            .find(|standing| standing.as_str() == text)
    }
}

/// Правило: официальная — только ключ есть и билет принят.
pub fn standing(key: bool, verdict: Verdict) -> Standing {
    match (key, verdict) {
        (true, Verdict::Accepted) => Standing::Official,
        (true, Verdict::Unknown) => Standing::Pending,
        _ => Standing::Unofficial,
    }
}

/// `error` отказа в теле ответа.
fn refusal(response: &Response) -> Option<String> {
    match json_parse(&response.body).ok()?.get("error")? {
        Value::String(code) => Some(code.clone()),
        _ => None,
    }
}

/// Вердикт по ответу; `ticketed` — запрос ушёл с билетом. Успех без билета ничего не говорит.
pub fn verdict_of(response: &Response, ticketed: bool) -> Option<Verdict> {
    if (200..300).contains(&response.status) || response.status == 304 {
        return ticketed.then_some(Verdict::Accepted);
    }
    refusal(response)
        .is_some_and(|code| REFUSALS.contains(&code.as_str()))
        .then_some(Verdict::Refused)
}

/// Колбэк перемены: событие окна `build-official`.
pub type OnStanding = Arc<dyn Fn(Standing) + Send + Sync>;

struct Inner {
    key: bool,
    pretend: bool,
    verdict: Mutex<Verdict>,
    on_change: Option<OnStanding>,
}

/// Официальность сборки; копии делят состояние.
#[derive(Clone)]
pub struct Official(Arc<Inner>);

impl Official {
    /// `key` — `BuildKey::available()`; `pretend` — стенд (`dev-hooks`, тесты) считается
    /// официальной сборкой без ключа.
    pub fn new(key: bool, pretend: bool, on_change: Option<OnStanding>) -> Official {
        Official(Arc::new(Inner {
            key,
            pretend,
            verdict: Mutex::new(Verdict::Unknown),
            on_change,
        }))
    }

    fn verdict(&self) -> std::sync::MutexGuard<'_, Verdict> {
        self.0.verdict.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn standing(&self) -> Standing {
        self.standing_with(*self.verdict())
    }

    fn standing_with(&self, verdict: Verdict) -> Standing {
        if self.0.pretend {
            Standing::Official
        } else {
            standing(self.0.key, verdict)
        }
    }

    /// Публичным запросам есть смысл уходить: без ключа билета не будет, и сборка не станет
    /// официальной (самосборка ничего не спрашивает у сервера).
    pub fn possible(&self) -> bool {
        self.0.pretend || self.0.key
    }

    /// Новый вердикт; сборка для окна сменилась — `on_change`. Колбэк зовётся под замком:
    /// одновременные ответы не обгонят друг друга в событиях окна (он только шлёт событие).
    pub fn record(&self, verdict: Verdict) {
        let mut current = self.verdict();
        let before = self.standing_with(*current);
        *current = verdict;
        let after = self.standing_with(verdict);
        if before != after
            && let Some(on_change) = &self.0.on_change
        {
            on_change(after);
        }
    }
}

/// Транспорт публичных запросов: отпечаток сборки и билет аттестации профиля поверх общего
/// транспорта; ответы ведут [`Official`].
pub struct AttestedTransport {
    inner: Arc<dyn Transport>,
    build_header: BuildHeader,
    ticket_provider: TicketProvider,
    official: Official,
}

impl AttestedTransport {
    pub fn new(
        inner: Arc<dyn Transport>,
        build_header: BuildHeader,
        ticket_provider: TicketProvider,
        official: Official,
    ) -> AttestedTransport {
        AttestedTransport {
            inner,
            build_header,
            ticket_provider,
            official,
        }
    }

    /// Один запрос с `x-hlsr-build` и `x-hlsr-ticket` (если есть); второе — ушёл ли билет.
    async fn send(
        &self,
        url: &str,
        options: &RequestOptions,
        force_ticket: bool,
    ) -> Result<(Response, bool), TransportError> {
        let mut options = options.clone();
        if let Some(build) = (self.build_header)().await.filter(|b| !b.is_empty()) {
            options.headers.push(("x-hlsr-build".to_owned(), build));
        }
        let ticket = match (self.ticket_provider)(force_ticket).await {
            Ok(ticket) => ticket.filter(|ticket| !ticket.is_empty()),
            // Сервер признал сборку изменённой: билета не будет, запрос уходит без него.
            Err(_) => {
                self.official.record(Verdict::Refused);
                None
            }
        };
        let ticketed = ticket.is_some();
        if let Some(ticket) = ticket {
            options.headers.push(("x-hlsr-ticket".to_owned(), ticket));
        }
        Ok((self.inner.request(url, &options).await?, ticketed))
    }
}

impl Transport for AttestedTransport {
    fn request<'a>(
        &'a self,
        url: &'a str,
        options: &'a RequestOptions,
    ) -> BoxFuture<'a, Result<Response, TransportError>> {
        Box::pin(async move {
            let (mut response, mut ticketed) = self.send(url, options, false).await?;
            // Билет истёк или ещё не получен: один повтор с новым, как у профиля.
            if ticketed
                && response.status == 428
                && refusal(&response).as_deref() == Some("ATTEST_REQUIRED")
            {
                (response, ticketed) = self.send(url, options, true).await?;
            }
            if let Some(verdict) = verdict_of(&response, ticketed) {
                self.official.record(verdict);
            }
            Ok(response)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::attestation::ClientModified;
    use crate::telemetry::transport::Headers;

    /// Транспорт-заглушка: ответы по очереди, запросы — в журнал.
    #[derive(Default)]
    struct Replay {
        replies: Mutex<Vec<(u16, &'static str)>>,
        sent: Mutex<Vec<Vec<(String, String)>>>,
    }

    impl Transport for Replay {
        fn request<'a>(
            &'a self,
            _url: &'a str,
            options: &'a RequestOptions,
        ) -> BoxFuture<'a, Result<Response, TransportError>> {
            self.sent.lock().unwrap().push(options.headers.clone());
            let (status, body) = self.replies.lock().unwrap().remove(0);
            Box::pin(async move {
                Ok(Response {
                    status,
                    headers: Headers::default(),
                    body: body.to_owned(),
                })
            })
        }
    }

    const BUILD: &str =
        "win32/0.5.0/0000000000000000000000000000000000000000000000000000000000000000";

    fn build_header() -> BuildHeader {
        Arc::new(|| Box::pin(async { Some(BUILD.to_owned()) }))
    }

    /// Билеты `t0`, `t1`, … (`force` — следующий); `None` — ключа нет.
    fn tickets(key: bool) -> (TicketProvider, Arc<Mutex<Vec<bool>>>) {
        let asked: Arc<Mutex<Vec<bool>>> = Arc::default();
        let log = asked.clone();
        let provider: TicketProvider = Arc::new(move |force| {
            let mut asked = log.lock().unwrap();
            asked.push(force);
            let ticket = key.then(|| format!("t{}", asked.len() - 1));
            Box::pin(async move { Ok(ticket) })
        });
        (provider, asked)
    }

    fn transport(
        replies: &[(u16, &'static str)],
        key: bool,
        provider: TicketProvider,
    ) -> (Arc<Replay>, AttestedTransport, Arc<Mutex<Vec<Standing>>>) {
        let replay = Arc::new(Replay::default());
        *replay.replies.lock().unwrap() = replies.to_vec();
        let changes: Arc<Mutex<Vec<Standing>>> = Arc::default();
        let log = changes.clone();
        let official = Official::new(
            key,
            false,
            Some(Arc::new(move |s| log.lock().unwrap().push(s))),
        );
        let attested = AttestedTransport::new(replay.clone(), build_header(), provider, official);
        (replay, attested, changes)
    }

    fn get() -> RequestOptions {
        RequestOptions {
            method: Some("GET".to_owned()),
            headers: vec![("if-none-match".to_owned(), "\"e1\"".to_owned())],
            ..RequestOptions::default()
        }
    }

    fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
        headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn official_only_with_a_key_and_an_accepted_ticket() {
        assert_eq!(standing(true, Verdict::Accepted), Standing::Official);
        assert_eq!(standing(true, Verdict::Unknown), Standing::Pending);
        assert_eq!(standing(true, Verdict::Refused), Standing::Unofficial);
        for verdict in [Verdict::Unknown, Verdict::Accepted, Verdict::Refused] {
            assert_eq!(standing(false, verdict), Standing::Unofficial);
        }
        // Стенд — официальный всегда; самосборка ничего не спрашивает.
        let stand = Official::new(false, true, None);
        stand.record(Verdict::Refused);
        assert_eq!(stand.standing(), Standing::Official);
        assert!(stand.possible());
        assert!(!Official::new(false, false, None).possible());
        for standing in [Standing::Pending, Standing::Official, Standing::Unofficial] {
            assert_eq!(Standing::parse(standing.as_str()), Some(standing));
        }
        assert_eq!(Standing::parse("yes"), None);
    }

    #[test]
    fn verdicts_follow_the_server() {
        let reply = |status, body: &str| Response {
            status,
            headers: Headers::default(),
            body: body.to_owned(),
        };
        assert_eq!(verdict_of(&reply(200, "{}"), true), Some(Verdict::Accepted));
        assert_eq!(verdict_of(&reply(304, ""), true), Some(Verdict::Accepted));
        // Ответ без билета о сборке ничего не говорит.
        assert_eq!(verdict_of(&reply(200, "{}"), false), None);
        for (status, code) in [
            (403, "CLIENT_MODIFIED"),
            (428, "ATTEST_REQUIRED"),
            (503, "CLIENT_UNVERIFIED"),
        ] {
            let body = format!("{{\"error\":\"{code}\"}}");
            assert_eq!(
                verdict_of(&reply(status, &body), true),
                Some(Verdict::Refused)
            );
            assert_eq!(
                verdict_of(&reply(status, &body), false),
                Some(Verdict::Refused)
            );
        }
        assert_eq!(verdict_of(&reply(500, "{}"), true), None);
        assert_eq!(
            verdict_of(&reply(403, "{\"error\":\"BANNED\"}"), true),
            None
        );
        assert_eq!(verdict_of(&reply(503, "<html>"), true), None);
    }

    #[tokio::test]
    async fn public_requests_carry_the_build_and_the_ticket() {
        let (provider, asked) = tickets(true);
        let (replay, attested, changes) = transport(&[(200, "{}")], true, provider);
        assert_eq!(attested.official.standing(), Standing::Pending);
        let response = attested.request("https://api/x", &get()).await.unwrap();
        assert_eq!(response.status, 200);
        let sent = replay.sent.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        // Свои заголовки клиента остаются.
        assert_eq!(header(&sent[0], "if-none-match"), Some("\"e1\""));
        assert_eq!(header(&sent[0], "x-hlsr-build"), Some(BUILD));
        assert_eq!(header(&sent[0], "x-hlsr-ticket"), Some("t0"));
        assert_eq!(*asked.lock().unwrap(), [false]);
        assert_eq!(*changes.lock().unwrap(), [Standing::Official]);
    }

    #[tokio::test]
    async fn an_expired_ticket_is_renewed_once_then_refused() {
        let required = "{\"error\":\"ATTEST_REQUIRED\"}";
        let (provider, asked) = tickets(true);
        let (replay, attested, changes) =
            transport(&[(428, required), (200, "{}")], true, provider);
        attested.request("https://api/x", &get()).await.unwrap();
        let sent = replay.sent.lock().unwrap().clone();
        assert_eq!(header(&sent[1], "x-hlsr-ticket"), Some("t1"));
        assert_eq!(*asked.lock().unwrap(), [false, true]);
        assert_eq!(*changes.lock().unwrap(), [Standing::Official]);

        // Повтор тоже отказан: не официальная, третьего запроса нет.
        *replay.replies.lock().unwrap() = vec![(428, required), (428, required)];
        let response = attested.request("https://api/x", &get()).await.unwrap();
        assert_eq!(response.status, 428);
        assert_eq!(replay.sent.lock().unwrap().len(), 4);
        assert_eq!(
            *changes.lock().unwrap(),
            [Standing::Official, Standing::Unofficial]
        );
    }

    #[tokio::test]
    async fn without_a_key_there_is_no_ticket_and_no_official_build() {
        let (provider, _) = tickets(false);
        let (replay, attested, changes) = transport(&[(200, "{}")], false, provider);
        assert_eq!(attested.official.standing(), Standing::Unofficial);
        attested.request("https://api/x", &get()).await.unwrap();
        let sent = replay.sent.lock().unwrap().clone();
        assert_eq!(header(&sent[0], "x-hlsr-build"), Some(BUILD));
        assert_eq!(header(&sent[0], "x-hlsr-ticket"), None);
        assert!(changes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_modified_client_loses_the_official_build() {
        let modified: TicketProvider = Arc::new(|_| Box::pin(async { Err(ClientModified) }));
        let (replay, attested, changes) =
            transport(&[(403, "{\"error\":\"CLIENT_MODIFIED\"}")], true, modified);
        attested.request("https://api/x", &get()).await.unwrap();
        let sent = replay.sent.lock().unwrap().clone();
        assert_eq!(header(&sent[0], "x-hlsr-ticket"), None);
        assert_eq!(*changes.lock().unwrap(), [Standing::Unofficial]);
        assert_eq!(attested.official.standing(), Standing::Unofficial);
    }

    #[tokio::test]
    async fn an_unverified_version_is_not_official_until_accepted_again() {
        let (provider, _) = tickets(true);
        let (_, attested, changes) = transport(
            &[(503, "{\"error\":\"CLIENT_UNVERIFIED\"}"), (200, "{}")],
            true,
            provider,
        );
        attested.request("https://api/x", &get()).await.unwrap();
        assert_eq!(attested.official.standing(), Standing::Unofficial);
        attested.request("https://api/x", &get()).await.unwrap();
        assert_eq!(attested.official.standing(), Standing::Official);
        assert_eq!(
            *changes.lock().unwrap(),
            [Standing::Unofficial, Standing::Official]
        );
    }
}
