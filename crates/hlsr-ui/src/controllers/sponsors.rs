//! Окно «Спонсорство» (`SponsorsModal.vue` 0.4.9) и «Спасибо за поддержку»
//! (`SupportThanksModal.vue`). Без UI.
//!
//! Ответ `public-sponsors-get` бывает двух видов (`shared/sponsor-sections.js`):
//! - новый сервер (`version: 2`): `{links, active, donors, past}` — три раздела окна и адрес
//!   бота для кнопок «Подписка» и «Донат»;
//! - старый сервер на PHP (`version: 1`): `{sponsors}` — окно как в 0.4.7: «царь горы» (трон,
//!   пьедестал, сетка из шести дорожек), суммы в рублях (полная — в подсказке, от миллиона —
//!   сокращённая), буква-заглушка под аватаром.
//!
//! Клик по записи: профиль HLSR (`link.steamId64`) или свой адрес — его main берёт из своего
//! последнего списка по разделу и номеру (`sponsor-open-link`); кнопки оплаты — `support-open`.
//! Обработчики обоих каналов (проверка номера, только https/`t.me`) — в `hlsr-online`, задача
//! O-3 (волна 15); до неё роутер отвечает `no_handler`, окно молчит, как `.catch(() => null)`.

use super::bridge::Tr;
use super::jsv::{field, prop, str_of, truthy};
use crate::intl;
use chrono::TimeZone;
use hlsr_core::language::Language;
use serde_json::{Value, json};

/// Разделы окна по порядку (`SECTIONS`).
pub const SECTIONS: [&str; 3] = ["active", "donors", "past"];
/// Кнопки оплаты (`PAY_KEYS`): обе ведут в бота.
pub const PAY_KEYS: [&str; 2] = ["subscribe", "donate"];

/// Что показывает окно.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Ждём ответа: только загрузка, чтобы при старом сервере не мелькнула шапка нового окна.
    Loading,
    /// Старый сервер (`version: 1`): окно 0.4.7 без кнопок и разделов.
    Legacy,
    /// Новое окно (в том числе с ошибкой или пустым списком).
    Window,
}

/// Что делает клик по записи или кнопке.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// `$emit("open-profile", steamId64)`: оболочка закрывает окно и открывает профиль.
    Profile(String),
    /// `ipcRenderer.invoke(channel, ...args)`.
    Invoke(&'static str, Vec<Value>),
}

/// Карточка спонсора старого окна, готовая к показу.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SponsorCard {
    pub id: String,
    pub name: String,
    pub note: String,
    /// `amountShort`: сумма в карточке.
    pub amount: String,
    /// `amountText`: полная сумма (подсказка).
    pub amount_full: String,
    /// `king`, `podium`, `rest`.
    pub tier: &'static str,
    /// Клетка сетки из шести дорожек: трон — ряд целиком, пьедестал — по три (один — целиком),
    /// остальные — по две.
    pub row: i32,
    pub col: i32,
    pub colspan: i32,
    /// Место (с 1).
    pub place: i32,
    /// `aria-label` номера места.
    pub place_label: String,
    /// «Царь горы» над ником; у остальных пусто.
    pub king_title: String,
    /// Подпись под троном: «Чтобы занять трон, нужно больше …».
    pub dethrone: String,
    pub initial: String,
    pub avatar: String,
    /// Есть профиль HLSR: карточка — кнопка, открывает профиль.
    pub steam_id: String,
    pub nick_style: String,
    pub nick_css: String,
    pub frame_css: String,
    /// Значок подписчика у ника (`profile.subscriber === true`).
    pub subscriber: bool,
}

/// Запись раздела нового окна, готовая к показу.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entry {
    /// Номер записи в разделе ответа: по нему main открывает её ссылку.
    pub index: usize,
    /// `nameOf`: имя, иначе ник профиля, иначе «Аноним».
    pub name: String,
    /// Скрытый ник: курсив и цвет `--l-text-faint`, без подсветки и значка.
    pub anonymous: bool,
    pub subscriber: bool,
    pub initial: String,
    pub avatar: String,
    /// Подсветка ника: только у видимого ника с профилем.
    pub nick_style: String,
    pub nick_css: String,
    pub frame_css: String,
    /// Донаты: сумма в карточке и полная (подсказка).
    pub amount: String,
    pub amount_full: String,
    pub note: String,
    /// `king`, `podium`, `rest` (донаты).
    pub tier: &'static str,
    /// `aria-label` номера места (донаты).
    pub place_label: String,
    /// Подпись под троном (донаты, первое место).
    pub dethrone: String,
    /// Ник кликается (`entryAction`).
    pub link: bool,
    /// `linkTitle`: «Открыть профиль HLSR», «Открыть ссылку: хост» или пусто.
    pub title: String,
}

/// Раздел нового окна.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    /// `active`, `donors`, `past`.
    pub key: &'static str,
    pub entries: Vec<Entry>,
}

/// Состояние окна: ждёт ответа или ответ (`null` — отказ).
#[derive(Debug, Clone)]
pub struct SponsorsState {
    result: Option<Value>,
    loading: bool,
}

impl Default for SponsorsState {
    fn default() -> Self {
        SponsorsState::new()
    }
}

/// `sponsorTier(index)`: 0 — трон, 1–2 — пьедестал, дальше — сетка.
pub fn tier(index: usize) -> &'static str {
    match index {
        0 => "king",
        1 | 2 => "podium",
        _ => "rest",
    }
}

impl SponsorsState {
    pub fn new() -> SponsorsState {
        SponsorsState {
            result: None,
            loading: true,
        }
    }

    /// `created()`: список запрашивается один раз, при открытии окна.
    pub fn load() -> Action {
        Action::Invoke("public-sponsors-get", Vec::new())
    }

    /// Ответ `public-sponsors-get` (отказ — `null`).
    pub fn answer(&mut self, reply: Option<Value>) {
        self.result = reply;
        self.loading = false;
    }

    fn result(&self) -> Option<&Value> {
        self.result.as_ref().filter(|result| truthy(Some(result)))
    }

    /// `isLegacy(result)`.
    pub fn legacy(&self) -> bool {
        self.result()
            .and_then(|result| prop(result, "version"))
            .and_then(Value::as_f64)
            == Some(1.0)
    }

    pub fn view(&self) -> View {
        if self.loading {
            View::Loading
        } else if self.legacy() {
            View::Legacy
        } else {
            View::Window
        }
    }

    /// `failed`: ответа нет или `status !== "ok"`.
    pub fn failed(&self) -> bool {
        !self.loading
            && !self
                .result()
                .and_then(|result| prop(result, "status"))
                .is_some_and(|status| status == "ok")
    }

    /// `supportKeys(result.links)`: кнопки оплаты, если сервер прислал адрес бота.
    pub fn support_keys(&self) -> Vec<&'static str> {
        let links = match self.result() {
            Some(result) if !self.legacy() => prop(result, "links"),
            _ => None,
        };
        let bot = links
            .filter(|links| links.is_object())
            .and_then(|links| prop(links, "bot"));
        match bot {
            Some(Value::String(url)) if !url.is_empty() => PAY_KEYS.to_vec(),
            _ => Vec::new(),
        }
    }

    /// Записи раздела `key` ответа (`result[key]`).
    fn raw(&self, key: &str) -> &[Value] {
        self.result()
            .filter(|result| result.is_object())
            .and_then(|result| prop(result, key))
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice)
    }

    /// `sectionsOf(result)` нового окна: `active`, `donors`, `past` без пустых.
    pub fn sections(&self, lang: Language, tr: &Tr) -> Vec<Section> {
        let v2 = self
            .result()
            .and_then(|result| prop(result, "version"))
            .and_then(Value::as_f64)
            == Some(2.0);
        if !v2 {
            return Vec::new();
        }
        SECTIONS
            .iter()
            .filter(|key| !self.raw(key).is_empty())
            .map(|&key| Section {
                key,
                entries: self
                    .raw(key)
                    .iter()
                    .enumerate()
                    .map(|(index, entry)| entry_view(key, index, entry, lang, tr))
                    .collect(),
            })
            .collect()
    }

    /// Список 0.4.7 (`version: 1`): `{id, name, note, amount, profile}`.
    pub fn legacy_cards(&self, lang: Language, tr: &Tr) -> Vec<SponsorCard> {
        if !self.legacy() {
            return Vec::new();
        }
        let sponsors = self.raw("sponsors");
        sponsors
            .iter()
            .enumerate()
            .map(|(index, sponsor)| card(index, sponsors.len(), sponsor, lang, tr))
            .collect()
    }

    /// `activate(section, index)`: профиль или ссылка через main; остальное не кликается.
    pub fn activate(&self, section: &str, index: usize) -> Option<Action> {
        if self.legacy() || !SECTIONS.contains(&section) {
            return None;
        }
        let entry = self.raw(section).get(index)?;
        match entry_action(entry)? {
            Link::Profile(steam_id) => Some(Action::Profile(steam_id)),
            Link::Url(_) => Some(Action::Invoke(
                "sponsor-open-link",
                vec![json!({"section": section, "index": index})],
            )),
        }
    }

    /// `support(key)`: кнопки Tribute и бота — адрес знает только main.
    pub fn support(&self, key: &str) -> Option<Action> {
        let key = self.support_keys().into_iter().find(|k| *k == key)?;
        Some(Action::Invoke("support-open", vec![json!(key)]))
    }
}

/// Куда ведёт запись (`entryAction`).
#[derive(Debug, Clone, PartialEq)]
enum Link {
    Profile(String),
    Url(String),
}

fn entry_action(entry: &Value) -> Option<Link> {
    let link = prop(entry, "link").filter(|link| link.is_object())?;
    let text = |key| str_of(prop(link, key)).filter(|value| !value.is_empty());
    match str_of(prop(link, "mode")) {
        Some("profile") => text("steamId64").map(|id| Link::Profile(id.to_owned())),
        Some("url") => text("url").map(|url| Link::Url(url.to_owned())),
        _ => None,
    }
}

/// `entryName(entry)`: своё имя, иначе ник профиля; `""` — скрытый ник.
pub fn entry_name(entry: &Value) -> String {
    if !entry.is_object() {
        return String::new();
    }
    let own = str_of(prop(entry, "name")).unwrap_or("").trim();
    if !own.is_empty() {
        return own.to_owned();
    }
    str_of(field(prop(entry, "profile"), "personaName"))
        .unwrap_or("")
        .trim()
        .to_owned()
}

/// `new URL(href).hostname`: без учётки и порта, хост в нижнем регистре (WHATWG, как в 0.4.9).
fn hostname(href: &str) -> Option<String> {
    url::Url::parse(href).ok()?.host_str().map(str::to_owned)
}

fn entry_view(
    section: &'static str,
    index: usize,
    entry: &Value,
    lang: Language,
    tr: &Tr,
) -> Entry {
    let name = entry_name(entry);
    let profile = prop(entry, "profile").filter(|profile| truthy(Some(profile)));
    let text =
        |value: Option<&Value>, key: &str| str_of(field(value, key)).unwrap_or("").to_owned();
    // Подсветка и значок — только у видимого ника.
    let styled = (!name.is_empty()).then_some(profile).flatten();
    let action = entry_action(entry);
    let title = match &action {
        Some(Link::Profile(_)) => tr("#L_SPONSORS_OPEN_PROFILE", &[]),
        Some(Link::Url(url)) => hostname(url)
            .map(|host| tr("#L_SPONSORS_OPEN_LINK", &[host.as_str()]))
            .unwrap_or_default(),
        None => String::new(),
    };
    let donor = section == "donors";
    let amount = prop(entry, "amount")
        .and_then(hlsr_core::js::value::number)
        .unwrap_or(0.0);
    let short = amount_short(lang, amount);
    let place = (index + 1).to_string();
    Entry {
        index,
        anonymous: name.is_empty(),
        subscriber: styled.is_some_and(|p| prop(p, "subscriber") == Some(&Value::Bool(true))),
        initial: initial(Some(&Value::String(name.clone()))),
        avatar: text(profile, "avatarUrl"),
        nick_style: text(field(styled, "nick"), "style"),
        nick_css: text(field(styled, "nick"), "css"),
        frame_css: text(field(profile, "avatarFrame"), "css"),
        amount: if donor { short.clone() } else { String::new() },
        amount_full: if donor {
            amount_text(lang, amount)
        } else {
            String::new()
        },
        note: str_of(prop(entry, "note")).unwrap_or("").to_owned(),
        tier: if donor { tier(index) } else { "" },
        place_label: if donor {
            tr("#L_SPONSORS_PLACE", &[place.as_str()])
        } else {
            String::new()
        },
        dethrone: if donor && index == 0 {
            tr("#L_SPONSORS_DETHRONE", &[short.as_str()])
        } else {
            String::new()
        },
        link: action.is_some(),
        title,
        name: if name.is_empty() {
            tr("#L_SPONSORS_ANONYMOUS", &[])
        } else {
            name
        },
    }
}

/// `amountText`: `amount.toLocaleString(ru-RU | en-GB) + " ₽"`.
pub fn amount_text(lang: Language, amount: f64) -> String {
    format!("{} ₽", intl::format_number(lang, amount))
}

/// `amountShort`: от миллиона — `compact` («100 млн ₽»), иначе полная сумма.
pub fn amount_short(lang: Language, amount: f64) -> String {
    if amount < 1_000_000.0 {
        return amount_text(lang, amount);
    }
    format!("{} ₽", intl::format_compact(lang, amount))
}

/// `initial(name)`: `String(name || "?").trim()`, первая буква заглавной (пусто — «?»).
pub fn initial(name: Option<&Value>) -> String {
    let text = match name {
        Some(value) if truthy(Some(value)) => hlsr_core::js::to_string(value),
        _ => "?".to_owned(),
    };
    let trimmed = text.trim();
    trimmed
        .chars()
        .next()
        .map(|first| first.to_uppercase().collect())
        .unwrap_or_else(|| "?".to_owned())
}

fn card(index: usize, total: usize, sponsor: &Value, lang: Language, tr: &Tr) -> SponsorCard {
    let amount = prop(sponsor, "amount")
        .and_then(hlsr_core::js::value::number)
        .unwrap_or(0.0);
    let profile = prop(sponsor, "profile").filter(|profile| truthy(Some(profile)));
    let text =
        |value: Option<&Value>, key: &str| str_of(field(value, key)).unwrap_or_default().to_owned();
    let short = amount_short(lang, amount);
    let place = (index + 1).to_string();
    SponsorCard {
        id: match prop(sponsor, "id") {
            Some(Value::String(id)) => id.clone(),
            Some(other) => other.to_string(),
            None => String::new(),
        },
        name: text(Some(sponsor), "name"),
        note: text(Some(sponsor), "note"),
        amount: short.clone(),
        amount_full: amount_text(lang, amount),
        tier: tier(index),
        row: match index {
            0 => 0,
            1 | 2 => 1,
            _ => 2 + (index as i32 - 3) / 3,
        },
        col: match index {
            0 | 1 => 0,
            2 => 3,
            _ => ((index as i32 - 3) % 3) * 2,
        },
        // Один на пьедестале — на всю ширину ряда.
        colspan: match index {
            0 => 6,
            1 if total == 2 => 6,
            1 | 2 => 3,
            _ => 2,
        },
        place: index as i32 + 1,
        place_label: tr("#L_SPONSORS_PLACE", &[place.as_str()]),
        king_title: if index == 0 {
            tr("#L_SPONSORS_KING", &[])
        } else {
            String::new()
        },
        dethrone: if index == 0 {
            tr("#L_SPONSORS_DETHRONE", &[short.as_str()])
        } else {
            String::new()
        },
        initial: initial(prop(sponsor, "name")),
        avatar: text(profile, "avatarUrl"),
        steam_id: text(profile, "steamId64"),
        nick_style: text(field(profile, "nick"), "style"),
        nick_css: text(field(profile, "nick"), "css"),
        frame_css: text(field(profile, "avatarFrame"), "css"),
        subscriber: profile.is_some_and(|p| prop(p, "subscriber") == Some(&Value::Bool(true))),
    }
}

/// «Спасибо за поддержку» (`thanksNotice` LauncherShell 0.4.9): одно окно на все пришедшие
/// оплаты, первая — самая свежая.
#[derive(Debug, Clone, PartialEq)]
pub struct ThanksNotice {
    /// `subscription` или `donation`.
    pub kind: String,
    /// Конец подписки, unix-секунды (`Number.isSafeInteger`).
    pub until: Option<i64>,
    /// Строка про дизайнерский ник и подсветку.
    pub styles: bool,
}

impl ThanksNotice {
    /// `onSupportThanks(e, notice)`: событие `support-thanks`; чужое — `None`.
    pub fn from_event(notice: &Value) -> Option<ThanksNotice> {
        let kind =
            str_of(prop(notice, "kind")).filter(|k| *k == "subscription" || *k == "donation")?;
        str_of(prop(notice, "id"))?;
        const SAFE: f64 = 9_007_199_254_740_991.0;
        let until = prop(notice, "until")
            .and_then(Value::as_f64)
            .filter(|x| x.fract() == 0.0 && x.abs() <= SAFE)
            .map(|x| x as i64);
        Some(ThanksNotice {
            kind: kind.to_owned(),
            until,
            styles: prop(notice, "styles") == Some(&Value::Bool(true)),
        })
    }

    /// `until`: дата конца подписки «ДД.ММ.ГГГГ» по часам `tz` (у доната и без даты — пусто).
    pub fn until_in<Tz: TimeZone>(&self, tz: &Tz) -> String
    where
        Tz::Offset: std::fmt::Display,
    {
        let Some(until) = self
            .until
            .filter(|u| *u != 0 && self.kind == "subscription")
        else {
            return String::new();
        };
        until
            .checked_mul(1000)
            .and_then(|ms| tz.timestamp_millis_opt(ms).single())
            .map(|day| day.format("%d.%m.%Y").to_string())
            .unwrap_or_default()
    }

    /// Абзацы окна: «Спасибо…», строка про стили, «Подписка активна до …» (местное время).
    pub fn lines(&self, lang: Language) -> Vec<String> {
        self.lines_in(lang, &chrono::Local)
    }

    pub fn lines_in<Tz: TimeZone>(&self, lang: Language, tz: &Tz) -> Vec<String>
    where
        Tz::Offset: std::fmt::Display,
    {
        let tr = |key: &str, args: &[&str]| crate::tr(lang, key, args);
        let mut lines = vec![tr("#L_THANKS_TEXT", &[])];
        if self.styles {
            lines.push(tr("#L_THANKS_STYLES", &[]));
        }
        let until = self.until_in(tz);
        if !until.is_empty() {
            lines.push(tr("#L_THANKS_UNTIL", &[until.as_str()]));
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    fn en() -> Tr {
        Rc::new(|key, args| crate::tr(Language::En, key, args))
    }

    fn legacy() -> Value {
        json!({"status": "ok", "version": 1, "sponsors": [
            {"id": "a", "name": "kanoyo", "note": "спасибо", "amount": 150_000_000,
             "profile": {"steamId64": "1", "avatarUrl": "https://a/1.png", "subscriber": true,
                         "nick": {"style": "gold", "css": null}, "avatarFrame": null}},
            {"id": "b", "name": "Buster", "note": "", "amount": 5000, "profile": null},
            {"id": "c", "name": " zed", "note": "x", "amount": 1500, "profile": null},
            {"id": "d", "name": "", "note": "", "amount": 10, "profile": null},
        ]})
    }

    fn v2() -> Value {
        let card = |name: &str, subscriber: bool| {
            json!({"personaName": name, "avatarUrl": "https://a/p.png",
                   "nick": {"style": "gold", "css": null}, "avatarFrame": {"css": "border: 2px solid red"},
                   "subscriber": subscriber})
        };
        json!({"status": "ok", "version": 2,
            "links": {"bot": "https://t.me/hlsr_bot"},
            "active": [
                {"profile": card("Kanoyo", true), "name": "", "amount": 0, "note": "", "since": "",
                 "link": {"mode": "profile", "steamId64": "76561199155902676"}},
                {"profile": card("", true), "name": "", "amount": 0, "note": "", "since": "",
                 "link": {"mode": "none"}},
            ],
            "donors": [
                {"profile": card("Big", false), "name": "Большой", "amount": 1_500_000, "note": "привет",
                 "since": "", "link": {"mode": "url", "url": "https://example.com/x?y"}},
                {"profile": null, "name": "Two", "amount": 500, "note": "", "since": "",
                 "link": {"mode": "none"}},
            ],
            "past": []})
    }

    #[test]
    fn places_follow_the_king_of_the_hill_scheme() {
        assert_eq!(
            (0..5).map(tier).collect::<Vec<_>>(),
            ["king", "podium", "podium", "rest", "rest"]
        );
    }

    #[test]
    fn the_view_waits_for_the_answer_then_picks_the_legacy_or_the_new_window() {
        let mut state = SponsorsState::new();
        assert_eq!(state.view(), View::Loading);
        assert!(!state.failed());
        assert_eq!(
            SponsorsState::load(),
            Action::Invoke("public-sponsors-get", vec![])
        );
        state.answer(Some(legacy()));
        assert_eq!(state.view(), View::Legacy);
        assert!(state.support_keys().is_empty() && state.sections(Language::En, &en()).is_empty());
        let mut fresh = SponsorsState::new();
        fresh.answer(Some(v2()));
        assert_eq!(fresh.view(), View::Window);
        assert!(!fresh.failed());
        // Отказ (`null`) и ошибка — новое окно с «не удалось загрузить».
        let mut failed = SponsorsState::new();
        failed.answer(None);
        assert_eq!((failed.view(), failed.failed()), (View::Window, true));
        let mut error = SponsorsState::new();
        error.answer(Some(json!({"status": "error", "version": 2, "links": {},
                                 "active": [], "donors": [], "past": []})));
        assert!(error.failed() && error.sections(Language::En, &en()).is_empty());
        // Ответ без версии — ни разделов, ни старого окна.
        let mut odd = SponsorsState::new();
        odd.answer(Some(json!({"status": "ok", "sponsors": []})));
        assert_eq!(odd.view(), View::Window);
        assert!(odd.sections(Language::En, &en()).is_empty());
    }

    #[test]
    fn pay_buttons_need_the_bot_address() {
        let mut state = SponsorsState::new();
        state.answer(Some(v2()));
        assert_eq!(state.support_keys(), ["subscribe", "donate"]);
        assert_eq!(
            state.support("donate"),
            Some(Action::Invoke("support-open", vec![json!("donate")]))
        );
        assert_eq!(state.support("bot"), None);
        let mut no_bot = SponsorsState::new();
        no_bot.answer(Some(json!({"status": "ok", "version": 2,
            "links": {"subscribe": "https://t.me/tribute/app"}, "active": [], "donors": [], "past": []})));
        assert!(no_bot.support_keys().is_empty());
        assert_eq!(no_bot.support("subscribe"), None);
    }

    #[test]
    fn sections_skip_empty_ones_and_entries_keep_their_server_index() {
        let mut state = SponsorsState::new();
        state.answer(Some(v2()));
        let sections = state.sections(Language::En, &en());
        assert_eq!(
            sections.iter().map(|s| s.key).collect::<Vec<_>>(),
            ["active", "donors"]
        );
        let active = &sections[0].entries;
        // Имя — ник профиля; со ссылкой на профиль — подсказка и клик.
        assert_eq!(active[0].name, "Kanoyo");
        assert!(active[0].subscriber && active[0].link && !active[0].anonymous);
        assert_eq!(active[0].nick_style, "gold");
        assert_eq!(
            active[0].title,
            crate::tr(Language::En, "#L_SPONSORS_OPEN_PROFILE", &[])
        );
        // Скрытый ник: «Аноним», без подсветки и значка, рамка аватара остаётся.
        assert!(active[1].anonymous && !active[1].subscriber && !active[1].link);
        assert_eq!(
            active[1].name,
            crate::tr(Language::En, "#L_SPONSORS_ANONYMOUS", &[])
        );
        assert_eq!(
            (active[1].nick_style.as_str(), active[1].initial.as_str()),
            ("", "?")
        );
        assert_eq!(active[1].frame_css, "border: 2px solid red");
        let donors = &sections[1].entries;
        assert_eq!((donors[0].tier, donors[1].tier), ("king", "podium"));
        assert_eq!(donors[0].name, "Большой");
        assert_eq!(donors[0].amount, "1.5m ₽");
        assert_eq!(donors[0].amount_full, "1,500,000 ₽");
        assert_eq!(
            donors[0].dethrone,
            crate::tr(Language::En, "#L_SPONSORS_DETHRONE", &["1.5m ₽"])
        );
        assert_eq!(
            donors[0].title,
            crate::tr(Language::En, "#L_SPONSORS_OPEN_LINK", &["example.com"])
        );
        assert_eq!(
            donors[1].place_label,
            crate::tr(Language::En, "#L_SPONSORS_PLACE", &["2"])
        );
        assert!(donors[1].dethrone.is_empty() && !donors[1].link);
    }

    #[test]
    fn link_title_host_drops_credentials_and_port() {
        assert_eq!(
            hostname("https://user:pw@Example.COM:8080/x?y#z").as_deref(),
            Some("example.com")
        );
        assert_eq!(hostname("https://[::1]:443/").as_deref(), Some("[::1]"));
        assert_eq!(hostname("не адрес"), None);
    }

    #[test]
    fn clicks_open_the_profile_or_ask_main_for_the_link() {
        let mut state = SponsorsState::new();
        state.answer(Some(v2()));
        assert_eq!(
            state.activate("active", 0),
            Some(Action::Profile("76561199155902676".into()))
        );
        assert_eq!(state.activate("active", 1), None);
        assert_eq!(
            state.activate("donors", 0),
            Some(Action::Invoke(
                "sponsor-open-link",
                vec![json!({"section": "donors", "index": 0})]
            ))
        );
        assert_eq!(state.activate("donors", 9), None);
        assert_eq!(state.activate("legacy", 0), None);
        let mut old = SponsorsState::new();
        old.answer(Some(legacy()));
        assert_eq!(old.activate("donors", 0), None);
    }

    #[test]
    fn amounts_are_full_below_a_million_and_compact_above() {
        assert_eq!(amount_text(Language::Ru, 1500.0), "1\u{a0}500 ₽");
        assert_eq!(amount_text(Language::En, 1500.0), "1,500 ₽");
        assert_eq!(amount_short(Language::Ru, 999_999.0), "999\u{a0}999 ₽");
        let ru = amount_short(Language::Ru, 100_000_000.0);
        assert!(
            ru.starts_with("100") && ru.contains("млн") && ru.ends_with(" ₽"),
            "{ru}"
        );
        assert_eq!(amount_short(Language::En, 1_500_000.0), "1.5m ₽");
    }

    #[test]
    fn the_legacy_grid_places_the_podium_and_the_rest_in_rows_of_three() {
        let many: Vec<Value> = (0..9)
            .map(|i| json!({"id": i, "name": format!("n{i}"), "amount": 10 - i}))
            .collect();
        let mut state = SponsorsState::new();
        state.answer(Some(
            json!({"status": "ok", "version": 1, "sponsors": many}),
        ));
        let cells: Vec<_> = state
            .legacy_cards(Language::En, &en())
            .iter()
            .map(|c| (c.row, c.col, c.colspan))
            .collect();
        assert_eq!(
            cells,
            [
                (0, 0, 6),
                (1, 0, 3),
                (1, 3, 3),
                (2, 0, 2),
                (2, 2, 2),
                (2, 4, 2),
                (3, 0, 2),
                (3, 2, 2),
                (3, 4, 2)
            ]
        );
        // Два спонсора: второй занимает весь ряд пьедестала.
        let mut two = SponsorsState::new();
        two.answer(Some(json!({"status": "ok", "version": 1, "sponsors": [
            {"id": 1, "name": "a", "amount": 2}, {"id": 2, "name": "b", "amount": 1}]})));
        let cells: Vec<_> = two
            .legacy_cards(Language::En, &en())
            .iter()
            .map(|c| (c.row, c.col, c.colspan))
            .collect();
        assert_eq!(cells, [(0, 0, 6), (1, 0, 6)]);
    }

    #[test]
    fn legacy_cards_carry_tier_place_and_king_extras() {
        let mut state = SponsorsState::new();
        state.answer(Some(legacy()));
        let cards = state.legacy_cards(Language::En, &en());
        assert_eq!(cards.len(), 4);
        assert_eq!(cards[0].tier, "king");
        assert_eq!(cards[0].initial, "K");
        assert_eq!(cards[0].steam_id, "1");
        assert!(cards[0].subscriber && !cards[1].subscriber);
        assert_eq!(cards[0].avatar, "https://a/1.png");
        assert_eq!(cards[0].nick_style, "gold");
        assert_eq!(
            cards[0].king_title,
            crate::tr(Language::En, "#L_SPONSORS_KING", &[])
        );
        assert_eq!(
            cards[0].dethrone,
            crate::tr(Language::En, "#L_SPONSORS_DETHRONE", &["150m ₽"])
        );
        assert_eq!(cards[1].tier, "podium");
        assert_eq!(cards[1].steam_id, "");
        assert_eq!(cards[1].dethrone, "");
        assert_eq!(cards[2].initial, "Z");
        assert_eq!(cards[3].tier, "rest");
        assert_eq!(cards[3].initial, "?");
        assert_eq!(cards[3].place, 4);
        assert_eq!(
            cards[3].place_label,
            crate::tr(Language::En, "#L_SPONSORS_PLACE", &["4"])
        );
    }

    #[test]
    fn thanks_take_valid_notices_and_date_the_subscription() {
        let utc = chrono::Utc;
        let notice = ThanksNotice::from_event(&json!({"id": "7", "kind": "subscription",
            "until": 1_797_000_000, "styles": true}))
        .unwrap();
        assert_eq!(notice.until_in(&utc), "11.12.2026");
        let lines = notice.lines_in(Language::Ru, &utc);
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[2],
            crate::tr(Language::Ru, "#L_THANKS_UNTIL", &["11.12.2026"])
        );
        // Донат: без даты, без строки про стили.
        let donation = ThanksNotice::from_event(
            &json!({"id": "8", "kind": "donation", "until": 1_797_000_000}),
        )
        .unwrap();
        assert_eq!(donation.lines_in(Language::En, &utc).len(), 1);
        // Дробный или огромный `until` — без даты.
        let odd =
            ThanksNotice::from_event(&json!({"id": "9", "kind": "subscription", "until": 1.5}))
                .unwrap();
        assert_eq!(odd.until, None);
        assert!(ThanksNotice::from_event(&json!({"id": 7, "kind": "donation"})).is_none());
        assert!(ThanksNotice::from_event(&json!({"id": "7", "kind": "gift"})).is_none());
        assert!(ThanksNotice::from_event(&Value::Null).is_none());
    }
}
