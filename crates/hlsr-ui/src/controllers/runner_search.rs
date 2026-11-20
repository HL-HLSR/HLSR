//! Поиск игроков в шапке (`RunnerSearch.vue` 0.4.9): запрос с задержкой 350 мс, список
//! результатов, выбор стрелками и Enter, аватары из кеша Steam для тех, у кого нет ссылки или
//! она не открылась, значок подписчика у ника. Машина без интерфейса и без часов: побочные
//! действия возвращает списком ([`Effect`]), ответы сервисов и срабатывание таймера
//! возвращаются в неё методами; вид (`hlsr-gpui` `header::search`) выполняет вызовы и держит
//! таймер.

use super::bridge::Tr;
use super::jsv::{field, prop, str_of, truthy};
use crate::services::Reply;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::time::Duration;

/// Задержка перед запросом (`setTimeout(…, 350)`).
pub const SEARCH_DELAY: Duration = Duration::from_millis(350);

/// Короче запроса не ищут: `query.trim().length >= 2` (длина — в кодовых единицах UTF-16).
const MIN_QUERY: usize = 2;

/// Побочное действие машины.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// `clearTimeout` и новый `setTimeout(search, 350)` с номером запроса.
    StartTimer(u64),
    StopTimer,
    /// `ipcRenderer.invoke(channel, ...args)`; ответ вернуть методом по `tag`.
    Invoke {
        tag: Tag,
        channel: &'static str,
        args: Vec<Value>,
    },
    /// `$emit("open-profile", steamId64)`.
    OpenProfile(String),
    /// Поле ввода очищено программно (вышел из аккаунта): сбросить текст поля.
    ResetText,
}

/// Чему ответ.
#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    State,
    Search(u64),
    Avatars(u64, Vec<String>),
}

/// Строка результата, готовая к показу.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchRow {
    pub id: String,
    pub name: String,
    /// SteamID или «скрыт».
    pub id_text: String,
    pub letter: String,
    /// Адрес или data URI аватара; пусто — буква.
    pub avatar: String,
    pub nick_style: String,
    pub nick_css: String,
    pub frame_css: String,
    /// `profile.subscriber === true`: значок подписчика у ника.
    pub subscriber: bool,
}

/// Что показывает поле и список.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchView {
    pub signed_in: bool,
    pub open: bool,
    pub loading: bool,
    pub error: bool,
    pub selected: i32,
    pub rows: Vec<SearchRow>,
}

#[derive(Debug, Default)]
pub struct SearchMachine {
    query: String,
    results: Vec<Value>,
    signed_in: bool,
    open: bool,
    loading: bool,
    error: bool,
    request_id: u64,
    selected: usize,
    /// `avatars[id]`: `Some(None)` — ссылка не открылась, `Some(Some(uri))` — из кеша Steam.
    avatars: HashMap<String, Option<String>>,
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

impl SearchMachine {
    pub fn new() -> SearchMachine {
        SearchMachine::default()
    }

    /// `mounted`: слушатель `runner-profile-state` ставит адаптер; здесь — `runner-state`.
    pub fn mounted(&self) -> Vec<Effect> {
        vec![Effect::Invoke {
            tag: Tag::State,
            channel: "runner-state",
            args: vec![],
        }]
    }

    /// `beforeUnmount`: таймер сброшен, ответы прежних запросов отбрасываются.
    pub fn unmounted(&mut self) -> Vec<Effect> {
        self.request_id += 1;
        vec![Effect::StopTimer]
    }

    /// `onProfile(event, data)`: событие `runner-profile-state` или ответ `runner-state`.
    pub fn profile_state(&mut self, data: &Value) -> Vec<Effect> {
        self.signed_in = truthy(field(field(Some(data), "account"), "signedIn"));
        if self.signed_in {
            return vec![];
        }
        self.query.clear();
        self.open = false;
        self.results.clear();
        self.request_id += 1;
        vec![Effect::ResetText]
    }

    /// `@input`: текст изменён (`v-model` и `schedule`).
    pub fn input(&mut self, text: &str) -> Vec<Effect> {
        self.query = text.to_owned();
        self.schedule()
    }

    fn schedule(&mut self) -> Vec<Effect> {
        let mut effects = vec![Effect::StopTimer];
        self.request_id += 1;
        if utf16_len(self.query.trim()) < MIN_QUERY {
            self.open = false;
            self.results.clear();
            return effects;
        }
        self.open = true;
        self.loading = true;
        self.error = false;
        effects.push(Effect::StartTimer(self.request_id));
        effects
    }

    /// Крестик очистки: запрос пуст, фокус в поле.
    pub fn clear(&mut self) -> Vec<Effect> {
        self.query.clear();
        self.schedule()
    }

    /// `@focus`: список открывается, если запрос достаточно длинный.
    pub fn focus(&mut self) {
        self.open = utf16_len(self.query.trim()) >= MIN_QUERY;
    }

    /// Esc в поле и `mousedown` вне поиска.
    pub fn close(&mut self) {
        self.open = false;
    }

    /// ↓ / ↑.
    pub fn move_selection(&mut self, step: i32) {
        let count = self.results.len() as i32;
        if count > 0 {
            self.selected = (self.selected as i32 + step).rem_euclid(count) as usize;
        }
    }

    /// Enter: `choose(results[selected])`.
    pub fn enter(&mut self) -> Vec<Effect> {
        self.choose(self.selected)
    }

    /// Щелчок по строке или Enter: список закрыт, открыт профиль.
    pub fn choose(&mut self, index: usize) -> Vec<Effect> {
        let Some(id) = self
            .results
            .get(index)
            .and_then(|profile| str_of(prop(profile, "steamId64")))
            .map(str::to_owned)
        else {
            return vec![];
        };
        self.open = false;
        vec![Effect::OpenProfile(id)]
    }

    /// Сработал таймер запроса `request_id`.
    pub fn timer_fired(&mut self, request_id: u64) -> Vec<Effect> {
        if request_id != self.request_id {
            return vec![];
        }
        vec![Effect::Invoke {
            tag: Tag::Search(request_id),
            channel: "runner-search",
            args: vec![json!(self.query.trim())],
        }]
    }

    /// Ответ `runner-search`.
    pub fn search_reply(&mut self, request_id: u64, reply: Reply) -> Vec<Effect> {
        if request_id != self.request_id {
            return vec![];
        }
        let mut effects = vec![];
        match reply {
            Ok(value) => {
                self.error = !value.is_array();
                self.results = value.as_array().cloned().unwrap_or_default();
                self.selected = 0;
                effects.extend(self.load_avatars(None));
            }
            Err(_) => self.error = true,
        }
        self.loading = false;
        effects
    }

    /// `loadAvatars(requestId, only)`: аватары из кеша Steam тем, у кого ссылки нет.
    fn load_avatars(&self, only: Option<Vec<String>>) -> Vec<Effect> {
        let ids = only.unwrap_or_else(|| {
            self.results
                .iter()
                .filter(|profile| self.avatar_of(profile).is_none())
                .filter_map(|profile| str_of(prop(profile, "steamId64")).map(str::to_owned))
                .collect()
        });
        if ids.is_empty() {
            return vec![];
        }
        vec![Effect::Invoke {
            tag: Tag::Avatars(self.request_id, ids.clone()),
            channel: "friends-avatars",
            args: vec![json!(ids)],
        }]
    }

    /// Ответ `friends-avatars` на запрос `request_id` для `ids`.
    pub fn avatars_reply(&mut self, request_id: u64, ids: &[String], reply: Reply) {
        let Ok(found) = reply else { return };
        if request_id != self.request_id || !truthy(Some(&found)) {
            return;
        }
        for id in ids {
            if let Some(uri) = str_of(prop(&found, id)).filter(|uri| !uri.is_empty()) {
                self.avatars.insert(id.clone(), Some(uri.to_owned()));
            }
        }
    }

    /// `avatarFailed(profile)`: ссылка аватара не открылась — буква, пока кеш Steam не ответит.
    /// Второй раз не просит: иначе картинка кеша Steam, что тоже не открылась, запрашивалась бы
    /// по кругу на каждом кадре.
    pub fn avatar_failed(&mut self, id: &str) -> Vec<Effect> {
        if self.avatars.contains_key(id) {
            return vec![];
        }
        self.avatars.insert(id.to_owned(), None);
        self.load_avatars(Some(vec![id.to_owned()]))
    }

    fn avatar_of(&self, profile: &Value) -> Option<String> {
        let id = str_of(prop(profile, "steamId64")).unwrap_or_default();
        match self.avatars.get(id) {
            Some(own) => own.clone(),
            None => str_of(field(prop(profile, "details"), "avatarUrl"))
                .filter(|url| !url.is_empty())
                .map(str::to_owned),
        }
    }

    pub fn view(&self, tr: &Tr) -> SearchView {
        let rows = self
            .results
            .iter()
            .map(|profile| {
                let name = str_of(prop(profile, "personaName")).filter(|name| !name.is_empty());
                let id = str_of(prop(profile, "steamId64")).unwrap_or_default();
                let shown = name.unwrap_or(id);
                let hidden = prop(profile, "details")
                    .and_then(|details| prop(details, "hideSteamId"))
                    .is_some_and(|hidden| hidden == &Value::Bool(true));
                let text = |value: Option<&Value>, key: &str| {
                    str_of(field(value, key)).unwrap_or_default().to_owned()
                };
                SearchRow {
                    id: id.to_owned(),
                    name: shown.to_owned(),
                    id_text: if hidden {
                        tr("#L_RUNNER_ID_HIDDEN", &[])
                    } else {
                        id.to_owned()
                    },
                    letter: letter_of(name),
                    avatar: self.avatar_of(profile).unwrap_or_default(),
                    nick_style: text(prop(profile, "nick"), "style"),
                    nick_css: text(prop(profile, "nick"), "css"),
                    frame_css: text(prop(profile, "avatarFrame"), "css"),
                    subscriber: prop(profile, "subscriber") == Some(&Value::Bool(true)),
                }
            })
            .collect();
        SearchView {
            signed_in: self.signed_in,
            open: self.open,
            loading: self.loading,
            error: self.error,
            selected: self.selected as i32,
            rows,
        }
    }

    /// Запрос в поле (тесты).
    pub fn query(&self) -> &str {
        &self.query
    }
}

/// `Array.from((personaName || '?').trim() || '?')[0].toUpperCase()`.
fn letter_of(name: Option<&str>) -> String {
    let trimmed = name.unwrap_or("?").trim();
    let text = if trimmed.is_empty() { "?" } else { trimmed };
    text.chars()
        .next()
        .map(|first| first.to_uppercase().collect())
        .unwrap_or_default()
}

/// Пустой объект ответа `friends-avatars` (тесты и сцены).
pub fn no_avatars() -> Value {
    Value::Object(Map::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hlsr_core::language::Language;
    use std::rc::Rc;

    fn en() -> Tr {
        Rc::new(|key, args| crate::tr(Language::En, key, args))
    }

    fn signed_in() -> SearchMachine {
        let mut machine = SearchMachine::new();
        machine.profile_state(&json!({"account": {"signedIn": true}}));
        machine
    }

    fn profile(id: &str, name: &str, avatar: Option<&str>) -> Value {
        json!({"steamId64": id, "personaName": name, "nick": {"style": "gold", "css": null},
               "avatarFrame": null, "details": {"avatarUrl": avatar, "hideSteamId": false}})
    }

    #[test]
    fn a_short_query_neither_opens_the_list_nor_asks() {
        let mut machine = signed_in();
        assert_eq!(machine.input("a"), vec![Effect::StopTimer]);
        assert!(!machine.view(&en()).open);
        // Пробелы вокруг не считаются.
        assert_eq!(machine.input("  b  "), vec![Effect::StopTimer]);
        assert!(!machine.view(&en()).open);
    }

    #[test]
    fn a_query_waits_350_ms_and_only_the_last_one_is_asked() {
        let mut machine = signed_in();
        let first = machine.input("ka");
        let id = machine.request_id;
        assert_eq!(first, vec![Effect::StopTimer, Effect::StartTimer(id)]);
        let second = machine.input("kan");
        assert_eq!(second, vec![Effect::StopTimer, Effect::StartTimer(id + 1)]);
        // Таймер прежнего запроса уже не нужен.
        assert!(machine.timer_fired(id).is_empty());
        let asked = machine.timer_fired(id + 1);
        assert_eq!(
            asked,
            vec![Effect::Invoke {
                tag: Tag::Search(id + 1),
                channel: "runner-search",
                args: vec![json!("kan")],
            }]
        );
        let view = machine.view(&en());
        assert!(view.open && view.loading && !view.error);
    }

    #[test]
    fn results_replace_the_loader_and_missing_avatars_are_asked_from_steam() {
        let mut machine = signed_in();
        machine.input("ka");
        let request = machine.request_id;
        let reply = json!([
            profile("1", "Kanoyo", Some("https://a/1.png")),
            profile("2", "Buster", None)
        ]);
        let effects = machine.search_reply(request, Ok(reply));
        assert_eq!(
            effects,
            vec![Effect::Invoke {
                tag: Tag::Avatars(request, vec!["2".into()]),
                channel: "friends-avatars",
                args: vec![json!(["2"])],
            }]
        );
        let view = machine.view(&en());
        assert!(!view.loading && !view.error);
        assert_eq!(view.rows[0].avatar, "https://a/1.png");
        assert_eq!(view.rows[1].avatar, "");
        assert_eq!(view.rows[1].letter, "B");
        machine.avatars_reply(
            request,
            &["2".into()],
            Ok(json!({"2": "data:image/png;base64,xx"})),
        );
        assert_eq!(
            machine.view(&en()).rows[1].avatar,
            "data:image/png;base64,xx"
        );
    }

    #[test]
    fn a_stale_reply_is_dropped() {
        let mut machine = signed_in();
        machine.input("ka");
        let old = machine.request_id;
        machine.input("kan");
        assert!(
            machine
                .search_reply(old, Ok(json!([profile("1", "K", None)])))
                .is_empty()
        );
        assert!(machine.view(&en()).rows.is_empty());
        assert!(machine.view(&en()).loading);
    }

    #[test]
    fn a_failed_search_shows_the_network_error() {
        let mut machine = signed_in();
        machine.input("ka");
        let request = machine.request_id;
        machine.search_reply(request, Err("boom".into()));
        let view = machine.view(&en());
        assert!(view.error && !view.loading);
        machine.input("kan");
        let request = machine.request_id;
        // Ответ не массив — тоже ошибка, но не отказ.
        machine.search_reply(request, Ok(json!({"status": "error"})));
        assert!(machine.view(&en()).error);
    }

    #[test]
    fn arrows_wrap_and_enter_opens_the_selected_profile() {
        let mut machine = signed_in();
        machine.input("ka");
        let request = machine.request_id;
        machine.search_reply(
            request,
            Ok(json!([
                profile("1", "A", Some("u")),
                profile("2", "B", Some("u"))
            ])),
        );
        machine.move_selection(1);
        assert_eq!(machine.view(&en()).selected, 1);
        machine.move_selection(1);
        assert_eq!(machine.view(&en()).selected, 0);
        machine.move_selection(-1);
        assert_eq!(machine.view(&en()).selected, 1);
        assert_eq!(machine.enter(), vec![Effect::OpenProfile("2".into())]);
        assert!(!machine.view(&en()).open);
        // Нет результатов — Enter ничего не делает.
        let mut empty = signed_in();
        assert!(empty.enter().is_empty());
    }

    #[test]
    fn focus_reopens_only_a_long_enough_query_and_escape_closes() {
        let mut machine = signed_in();
        machine.input("kan");
        machine.close();
        assert!(!machine.view(&en()).open);
        machine.focus();
        assert!(machine.view(&en()).open);
        machine.input("k");
        machine.focus();
        assert!(!machine.view(&en()).open);
    }

    #[test]
    fn signing_out_clears_the_query_results_and_stops_pending_answers() {
        let mut machine = signed_in();
        machine.input("kan");
        let request = machine.request_id;
        let effects = machine.profile_state(&json!({"account": {"signedIn": false}}));
        assert_eq!(effects, vec![Effect::ResetText]);
        assert!(
            machine
                .search_reply(request, Ok(json!([profile("1", "K", None)])))
                .is_empty()
        );
        let view = machine.view(&en());
        assert!(!view.signed_in && !view.open && view.rows.is_empty());
        assert_eq!(machine.query(), "");
    }

    #[test]
    fn hidden_steam_ids_and_nameless_profiles_use_their_fallbacks() {
        let mut machine = signed_in();
        machine.input("ka");
        let request = machine.request_id;
        let hidden = json!({"steamId64": "7", "personaName": "", "details": {"hideSteamId": true}});
        machine.search_reply(request, Ok(json!([hidden])));
        let view = machine.view(&en());
        assert_eq!(view.rows[0].name, "7");
        assert_eq!(
            view.rows[0].id_text,
            crate::tr(Language::En, "#L_RUNNER_ID_HIDDEN", &[])
        );
        assert_eq!(view.rows[0].letter, "?");
    }

    #[test]
    fn subscribers_get_the_badge_and_a_broken_avatar_link_falls_back_to_steam_once() {
        let mut machine = signed_in();
        machine.input("ka");
        let request = machine.request_id;
        let mut fan = profile("1", "Kanoyo", Some("https://a/1.png"));
        fan["subscriber"] = json!(true);
        let mut other = profile("2", "Buster", Some("https://a/2.png"));
        other["subscriber"] = json!("yes");
        machine.search_reply(request, Ok(json!([fan, other])));
        let view = machine.view(&en());
        assert!(view.rows[0].subscriber && !view.rows[1].subscriber);
        // Ссылка не открылась: буква и один запрос в кеш Steam.
        assert_eq!(
            machine.avatar_failed("1"),
            vec![Effect::Invoke {
                tag: Tag::Avatars(request, vec!["1".into()]),
                channel: "friends-avatars",
                args: vec![json!(["1"])],
            }]
        );
        assert_eq!(machine.view(&en()).rows[0].avatar, "");
        assert!(machine.avatar_failed("1").is_empty());
        machine.avatars_reply(request, &["1".into()], Ok(json!({"1": "data:x"})));
        assert_eq!(machine.view(&en()).rows[0].avatar, "data:x");
    }

    #[test]
    fn clearing_resets_the_query_and_closes_the_list() {
        let mut machine = signed_in();
        machine.input("kan");
        assert!(machine.view(&en()).open);
        machine.clear();
        assert_eq!(machine.query(), "");
        assert!(!machine.view(&en()).open);
    }
}
