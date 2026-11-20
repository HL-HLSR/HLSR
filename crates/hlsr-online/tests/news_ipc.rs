//! Каналы ленты, меняющие состояние (`telemetry/news-ipc.js`): тесты `scripts/news-selftest.js`
//! `:211-247` с теми же случаями — реакции передают только вид и пост, ответ и свои реакции
//! нормализуются, коды отказов сохраняются, ссылка открывается только после подтверждения
//! настоящего HTTPS-хоста. Профиль, диалог и браузер — подставные.

use hlsr_delivery::providers::BoxFuture;
use hlsr_online::telemetry::news_ipc::{NewsError, NewsIpc, NewsLinks, NewsProfile, NewsReactions};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

type Action = Box<dyn Fn(&str, &Value) -> Result<Value, NewsError> + Send + Sync>;

struct Profile {
    action: Action,
    calls: Mutex<Vec<(String, Value)>>,
    achieved: Mutex<Vec<String>>,
}

impl NewsProfile for Profile {
    fn action<'a>(&'a self, path: &'a str, body: Value) -> BoxFuture<'a, Result<Value, NewsError>> {
        Box::pin(async move {
            let answer = (self.action)(path, &body);
            self.calls.lock().unwrap().push((path.to_owned(), body));
            answer
        })
    }

    fn achieve(&self, id: &str) {
        self.achieved.lock().unwrap().push(id.to_owned());
    }
}

#[derive(Default)]
struct Reactions(Mutex<Vec<(String, Value)>>);

impl NewsReactions for Reactions {
    fn update_reactions(&self, id: &str, raw: &Value) {
        self.0.lock().unwrap().push((id.to_owned(), raw.clone()));
    }
}

struct Links {
    confirmed: bool,
    asked: Mutex<Vec<(String, String)>>,
    opened: Mutex<Vec<String>>,
}

impl NewsLinks for Links {
    fn confirm_link<'a>(&'a self, host: &'a str, url: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            self.asked
                .lock()
                .unwrap()
                .push((host.to_owned(), url.to_owned()));
            self.confirmed
        })
    }

    fn open_external<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<(), NewsError>> {
        Box::pin(async move {
            self.opened.lock().unwrap().push(url.to_owned());
            Ok(())
        })
    }
}

struct Fixture {
    ipc: NewsIpc,
    profile: Arc<Profile>,
    reactions: Arc<Reactions>,
    links: Arc<Links>,
}

fn fixture(action: Action, confirmed: bool) -> Fixture {
    let profile = Arc::new(Profile {
        action,
        calls: Mutex::default(),
        achieved: Mutex::default(),
    });
    let reactions = Arc::new(Reactions::default());
    let links = Arc::new(Links {
        confirmed,
        asked: Mutex::default(),
        opened: Mutex::default(),
    });
    Fixture {
        ipc: NewsIpc::new(reactions.clone(), profile.clone(), links.clone()),
        profile,
        reactions,
        links,
    }
}

fn never(_: &str, _: &Value) -> Result<Value, NewsError> {
    panic!("профиль не должен вызываться");
}

#[tokio::test]
async fn reactions_pass_only_the_kind_and_the_post_and_normalize_the_reply() {
    let f = fixture(
        Box::new(|_, _| {
            Ok(json!({"postId": "12", "reactions": {"like": 1}, "mine": ["like", "evil", "like"]}))
        }),
        true,
    );
    let reply = f.ipc.react("12", "like").await;
    assert_eq!(reply["status"], "ok");
    assert_eq!(reply["mine"], json!(["like"]));
    assert_eq!(reply["postId"], "12");
    assert_eq!(
        reply["reactions"],
        json!({"like": 1, "love": 0, "fire": 0, "wow": 0, "celebrate": 0})
    );
    assert_eq!(
        *f.profile.calls.lock().unwrap(),
        vec![(
            "news/react".to_owned(),
            json!({"postId": "12", "reaction": "like"})
        )]
    );
    assert_eq!(f.reactions.0.lock().unwrap()[0].0, "12");
    assert_eq!(f.reactions.0.lock().unwrap()[0].1["like"], 1);
    assert_eq!(*f.profile.achieved.lock().unwrap(), vec!["community-voice"]);

    // Неверный пост и неизвестная реакция до профиля не доходят.
    assert_eq!(f.ipc.react("999 OR 1", "like").await["status"], "error");
    assert_eq!(f.ipc.react("12", "evil").await["status"], "error");
    assert_eq!(f.ipc.react("12", "constructor").await["status"], "error");
    assert_eq!(f.profile.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn reaction_without_my_kinds_does_not_unlock_the_achievement() {
    let f = fixture(
        Box::new(|_, _| Ok(json!({"postId": "12", "reactions": {"like": 0}, "mine": []}))),
        true,
    );
    assert_eq!(f.ipc.react("12", "like").await["status"], "ok");
    assert!(f.profile.achieved.lock().unwrap().is_empty());
}

#[tokio::test]
async fn ipc_keeps_the_codes_of_missing_sign_in_and_rate_limit_and_ignores_a_foreign_post() {
    for code in ["RUNNER_AUTH_REQUIRED", "RUNNER_HTTP_429"] {
        let f = fixture(
            Box::new(move |_, _| Err(NewsError::with_code(code, code))),
            true,
        );
        assert_eq!(
            f.ipc.react("12", "like").await,
            json!({"status": "error", "code": code})
        );
        assert!(f.reactions.0.lock().unwrap().is_empty());
    }
    // Ответ про другой пост кеш не меняет; ошибка без кода — NEWS_FAILED.
    let f = fixture(
        Box::new(|_, _| Ok(json!({"postId": "13", "reactions": {"like": 1}, "mine": ["like"]}))),
        true,
    );
    assert_eq!(
        f.ipc.react("12", "like").await,
        json!({"status": "error", "code": "NEWS_FAILED"})
    );
    assert!(f.reactions.0.lock().unwrap().is_empty());
    for broken in [
        json!({"postId": "12", "mine": []}),
        json!({"postId": "12", "reactions": null, "mine": []}),
        json!({"postId": "12", "reactions": {"like": 1}}),
        json!({"postId": "12", "reactions": {"like": 1}, "mine": {}}),
        json!({"postId": 12, "reactions": {"like": 1}, "mine": []}),
        json!(null),
    ] {
        let f = fixture(Box::new(move |_, _| Ok(broken.clone())), true);
        assert_eq!(f.ipc.react("12", "like").await["status"], "error");
        assert!(f.reactions.0.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn mine_lists_my_reactions_per_post() {
    let f = fixture(
        Box::new(|_, _| {
            Ok(json!({"mine": {"12": ["like", "evil", "like", "fire"], "14": "like"}}))
        }),
        true,
    );
    let ids: Vec<String> = ["12", "13", "14", "12"]
        .iter()
        .map(|id| (*id).to_owned())
        .collect();
    let reply = f.ipc.mine(&ids).await;
    assert_eq!(
        reply,
        json!({"status": "ok", "mine": {"12": ["like", "fire"], "13": [], "14": []}})
    );
    // Повторы в запрос не уходят.
    assert_eq!(
        *f.profile.calls.lock().unwrap(),
        vec![(
            "news/mine".to_owned(),
            json!({"postIds": ["12", "13", "14"]})
        )]
    );

    let f = fixture(Box::new(never), true);
    let too_many: Vec<String> = (1..=41).map(|id| id.to_string()).collect();
    assert_eq!(f.ipc.mine(&too_many).await["status"], "error");
    assert_eq!(f.ipc.mine(&["1 OR 1".to_owned()]).await["status"], "error");
    let forty: Vec<String> = (1..=40).map(|id| id.to_string()).collect();
    let f = fixture(Box::new(|_, _| Ok(json!({"mine": {}}))), true);
    assert_eq!(f.ipc.mine(&forty).await["status"], "ok");
}

#[tokio::test]
async fn news_links_open_only_after_confirming_a_real_https_host() {
    let f = fixture(Box::new(never), true);
    for url in [
        "javascript:alert(1)",
        "http://example.com",
        "https://user@example.com",
        "https://example.com:8443",
        "https://example.com\n",
    ] {
        assert_eq!(f.ipc.open_link(url).await["status"], "error", "{url}");
    }
    assert!(f.links.asked.lock().unwrap().is_empty());
    assert!(f.links.opened.lock().unwrap().is_empty());

    assert_eq!(
        f.ipc.open_link("https://example.com/path").await["status"],
        "ok"
    );
    assert_eq!(
        *f.links.asked.lock().unwrap(),
        vec![(
            "example.com".to_owned(),
            "https://example.com/path".to_owned()
        )]
    );
    assert_eq!(
        *f.links.opened.lock().unwrap(),
        vec!["https://example.com/path"]
    );

    let declined = fixture(Box::new(never), false);
    assert_eq!(
        declined.ipc.open_link("https://example.com").await,
        json!({"status": "cancelled"})
    );
    assert!(declined.links.opened.lock().unwrap().is_empty());
}
