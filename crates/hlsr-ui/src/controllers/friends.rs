//! Панель друзей Steam (`FriendsPanel.vue` 0.4.7): список из памяти лаунчера (сразу) и свежий
//! (тихо заменяет прежний), поиск при списке длиннее восьми, группы «в HLSR» и «Steam»,
//! аватары, которых нет в ответе, догружаются пачками по 40. Машина без Slint: методы возвращают
//! вызовы каналов ([`Effect`]), адаптер (`adapters::profile`) выполняет их.

use super::bridge::Tr;
use super::jsv::{prop, str_of, truthy};
use serde_json::{Value, json};

/// Размер пачки аватаров.
pub const AVATAR_BATCH: usize = 40;

/// Поиск виден, когда друзей больше стольки.
pub const SEARCH_FROM: usize = 8;

/// Чему ответ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tag {
    /// Первый `friends-list` (из памяти лаунчера).
    First,
    /// `friends-list({force: true})`.
    Fresh,
    /// Пачка аватаров: цепочка (после каждого списка — своя) и номер пачки.
    Avatars { chain: u64, at: usize },
}

/// Вызов канала.
#[derive(Debug, Clone, PartialEq)]
pub struct Effect {
    pub tag: Tag,
    pub channel: &'static str,
    pub args: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq)]
struct Friend {
    steam_id: String,
    name: String,
    avatar: Option<String>,
    in_hlsr: bool,
}

/// Строка списка панели.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EntryView {
    /// `title`, `friend`, `note`.
    pub kind: &'static str,
    /// Заголовок группы, подпись друга или текст примечания.
    pub text: String,
    /// Число друзей группы (у заголовка).
    pub count: i32,
    pub id: String,
    pub letter: String,
    pub avatar: String,
    pub in_hlsr: bool,
    /// `title` значка «в HLSR».
    pub hlsr_hint: String,
    /// Строка друга после другой строки той же группы (`li + li`: margin-top 2).
    pub gap: bool,
}

/// Что показывает панель.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FriendsView {
    /// Счётчик в шапке: число друзей или пусто.
    pub counter: String,
    pub search: bool,
    pub loading: bool,
    pub entries: Vec<EntryView>,
}

/// Состояние панели (`data()`).
#[derive(Debug, Clone)]
pub struct FriendsState {
    signed_in: bool,
    loading: bool,
    friends: Vec<Friend>,
    query: String,
    destroyed: bool,
    chain: u64,
    chains: Vec<(u64, Vec<String>)>,
}

impl FriendsState {
    pub fn new() -> FriendsState {
        FriendsState {
            signed_in: false,
            loading: true,
            friends: Vec::new(),
            query: String::new(),
            destroyed: false,
            chain: 0,
            chains: Vec::new(),
        }
    }

    /// `mounted`: `friends-list`.
    pub fn mounted(&self) -> Vec<Effect> {
        vec![Effect {
            tag: Tag::First,
            channel: "friends-list",
            args: Vec::new(),
        }]
    }

    /// `beforeUnmount`: ответы, которые ещё идут, отбрасываются.
    pub fn unmounted(&mut self) {
        self.destroyed = true;
    }

    pub fn set_query(&mut self, query: &str) {
        self.query = query.to_owned();
    }

    /// Ответ на вызов `tag`; `Err` — отказ.
    pub fn answer(&mut self, tag: Tag, reply: Result<Value, String>) -> Vec<Effect> {
        match tag {
            Tag::First => {
                self.loading = false;
                let Ok(data) = reply else {
                    return Vec::new();
                };
                if !truthy(Some(&data)) {
                    return Vec::new();
                }
                let mut effects = self.apply(&data);
                if !truthy(prop(&data, "cached")) || prop(&data, "signedIn") == Some(&json!(false))
                {
                    return effects;
                }
                effects.push(Effect {
                    tag: Tag::Fresh,
                    channel: "friends-list",
                    args: vec![json!({"force": true})],
                });
                effects
            }
            Tag::Fresh => match reply {
                Ok(fresh) if truthy(Some(&fresh)) && !self.destroyed => self.apply(&fresh),
                _ => Vec::new(),
            },
            Tag::Avatars { chain, at } => self.avatars(chain, at, reply),
        }
    }

    /// `apply(data)`: список, уже загруженные аватары остаются, недостающие — пачками.
    fn apply(&mut self, data: &Value) -> Vec<Effect> {
        let known: Vec<(String, String)> = self
            .friends
            .iter()
            .filter_map(|friend| {
                friend
                    .avatar
                    .clone()
                    .map(|avatar| (friend.steam_id.clone(), avatar))
            })
            .collect();
        self.signed_in = prop(data, "signedIn") != Some(&json!(false));
        self.friends = prop(data, "friends")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .map(|friend| {
                        let steam_id = str_of(prop(friend, "steamId64"))
                            .unwrap_or_default()
                            .to_owned();
                        let avatar = str_of(prop(friend, "avatarDataUri"))
                            .filter(|avatar| !avatar.is_empty())
                            .map(str::to_owned)
                            .or_else(|| {
                                known
                                    .iter()
                                    .find(|(id, _)| *id == steam_id)
                                    .map(|(_, avatar)| avatar.clone())
                            });
                        Friend {
                            name: str_of(prop(friend, "personaName"))
                                .unwrap_or_default()
                                .to_owned(),
                            in_hlsr: truthy(prop(friend, "inHlsr")),
                            steam_id,
                            avatar,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.load_avatars()
    }

    /// `loadAvatars()`: первая пачка сразу, остальные — после ответов.
    fn load_avatars(&mut self) -> Vec<Effect> {
        let ids: Vec<String> = self
            .friends
            .iter()
            .filter(|friend| friend.avatar.is_none())
            .map(|friend| friend.steam_id.clone())
            .collect();
        if ids.is_empty() || self.destroyed {
            return Vec::new();
        }
        self.chain += 1;
        let chain = self.chain;
        let first = Effect {
            tag: Tag::Avatars { chain, at: 0 },
            channel: "friends-avatars",
            args: vec![json!(ids[..ids.len().min(AVATAR_BATCH)])],
        };
        self.chains.push((chain, ids));
        vec![first]
    }

    fn avatars(&mut self, chain: u64, at: usize, reply: Result<Value, String>) -> Vec<Effect> {
        let Some(index) = self.chains.iter().position(|(id, _)| *id == chain) else {
            return Vec::new();
        };
        let Ok(batch) = reply else {
            self.chains.remove(index);
            return Vec::new();
        };
        if truthy(Some(&batch)) {
            for friend in &mut self.friends {
                if let Some(found) =
                    str_of(prop(&batch, &friend.steam_id)).filter(|f| !f.is_empty())
                {
                    friend.avatar = Some(found.to_owned());
                }
            }
        }
        let next = at + AVATAR_BATCH;
        let ids = &self.chains[index].1;
        if self.destroyed || next >= ids.len() {
            self.chains.remove(index);
            return Vec::new();
        }
        vec![Effect {
            tag: Tag::Avatars { chain, at: next },
            channel: "friends-avatars",
            args: vec![json!(ids[next..ids.len().min(next + AVATAR_BATCH)])],
        }]
    }

    /// Друзья по запросу поиска (`visible`).
    fn visible(&self) -> Vec<&Friend> {
        let needle = self.query.trim().to_lowercase();
        if needle.is_empty() {
            return self.friends.iter().collect();
        }
        self.friends
            .iter()
            .filter(|friend| {
                friend.name.to_lowercase().contains(&needle) || friend.steam_id.contains(&needle)
            })
            .collect()
    }

    pub fn view(&self, tr: &Tr) -> FriendsView {
        let t = |key: &str| tr(key, &[]);
        let mut view = FriendsView {
            counter: if self.friends.is_empty() {
                String::new()
            } else {
                self.friends.len().to_string()
            },
            search: self.friends.len() > SEARCH_FROM,
            loading: self.loading,
            entries: Vec::new(),
        };
        let note = |text: String| EntryView {
            kind: "note",
            text,
            ..EntryView::default()
        };
        if self.loading {
            return view;
        }
        if !self.signed_in {
            view.entries.push(note(t("#L_SB_STEAM_SIGNED_OUT")));
            return view;
        }
        if self.friends.is_empty() {
            view.entries.push(note(t("#L_FRIENDS_EMPTY")));
            return view;
        }
        let visible = self.visible();
        for (title, in_hlsr) in [
            ("#L_RUNNER_FRIENDS_HLSR", true),
            ("#L_RUNNER_FRIENDS_STEAM", false),
        ] {
            let group: Vec<&&Friend> = visible.iter().filter(|f| f.in_hlsr == in_hlsr).collect();
            if group.is_empty() {
                continue;
            }
            view.entries.push(EntryView {
                kind: "title",
                text: t(title),
                count: group.len() as i32,
                ..EntryView::default()
            });
            for (index, friend) in group.into_iter().enumerate() {
                view.entries.push(EntryView {
                    gap: index > 0,
                    kind: "friend",
                    text: friend.name.clone(),
                    id: friend.steam_id.clone(),
                    letter: letter_of(&friend.name),
                    avatar: friend.avatar.clone().unwrap_or_default(),
                    in_hlsr: friend.in_hlsr,
                    hlsr_hint: if friend.in_hlsr {
                        t("#L_RUNNER_FRIENDS_HLSR")
                    } else {
                        String::new()
                    },
                    ..EntryView::default()
                });
            }
        }
        if visible.is_empty() {
            view.entries.push(note(t("#L_FRIENDS_NOT_FOUND")));
        }
        view
    }
}

impl Default for FriendsState {
    fn default() -> Self {
        FriendsState::new()
    }
}

/// `letterOf(friend)`: первая буква имени заглавной или «?».
pub fn letter_of(name: &str) -> String {
    let name = if name.trim().is_empty() {
        "?"
    } else {
        name.trim()
    };
    name.chars()
        .next()
        .map(|first| first.to_uppercase().collect())
        .unwrap_or_else(|| "?".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hlsr_core::language::Language;
    use std::rc::Rc;

    fn en() -> Tr {
        Rc::new(|key, args| crate::tr(Language::En, key, args))
    }

    fn friend(index: usize, avatar: Option<&str>, hlsr: bool) -> Value {
        json!({"steamId64": format!("7656119800000{:04}", index), "personaName": format!("Name{index}"),
               "avatarDataUri": avatar, "inHlsr": hlsr})
    }

    fn list(count: usize) -> Value {
        json!({"signedIn": true, "friends": (0..count).map(|i| friend(i, None, i % 4 == 1)).collect::<Vec<_>>()})
    }

    #[test]
    fn the_panel_asks_for_the_list_then_for_avatars_in_batches_of_forty() {
        let mut state = FriendsState::new();
        assert_eq!(state.mounted()[0].channel, "friends-list");
        let effects = state.answer(Tag::First, Ok(list(95)));
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].channel, "friends-avatars");
        assert_eq!(effects[0].args[0].as_array().unwrap().len(), 40);
        // Ответ без аватаров: следующая пачка 40, затем 15, затем конец.
        let second = state.answer(effects[0].tag.clone(), Ok(json!({})));
        assert_eq!(second[0].args[0].as_array().unwrap().len(), 40);
        let third = state.answer(second[0].tag.clone(), Ok(json!({})));
        assert_eq!(third[0].args[0].as_array().unwrap().len(), 15);
        assert!(state.answer(third[0].tag.clone(), Ok(json!({}))).is_empty());
    }

    #[test]
    fn the_cached_list_is_replaced_by_a_fresh_one_after_the_first_avatars_request() {
        let mut state = FriendsState::new();
        let mut cached = list(3);
        cached["cached"] = json!(true);
        let effects = state.answer(Tag::First, Ok(cached));
        let channels: Vec<_> = effects
            .iter()
            .map(|e| (e.channel, e.args.clone()))
            .collect();
        assert_eq!(channels[0].0, "friends-avatars");
        assert_eq!(channels[1], ("friends-list", vec![json!({"force": true})]));
        // Свежий список: загруженные аватары сохраняются.
        state.answer(
            effects[0].tag.clone(),
            Ok(json!({"76561198000000000": "data:image/png;base64,AA"})),
        );
        let after = state.answer(Tag::Fresh, Ok(list(3)));
        let ids = after[0].args[0].as_array().unwrap();
        assert_eq!(ids.len(), 2, "аватар первого друга уже есть");
        let view = state.view(&en());
        let first = view
            .entries
            .iter()
            .find(|e| e.id == "76561198000000000")
            .unwrap();
        assert_eq!(first.avatar, "data:image/png;base64,AA");
    }

    #[test]
    fn the_cached_flag_is_ignored_for_a_signed_out_list() {
        let mut state = FriendsState::new();
        let effects = state.answer(
            Tag::First,
            Ok(json!({"signedIn": false, "friends": [], "cached": true})),
        );
        assert!(effects.is_empty());
        let view = state.view(&en());
        assert_eq!(view.entries[0].text, "Not signed in to Steam");
    }

    #[test]
    fn groups_search_and_notes() {
        let mut state = FriendsState::new();
        assert!(state.view(&en()).loading);
        state.answer(Tag::First, Ok(list(0)));
        assert_eq!(
            state.view(&en()).entries[0].text,
            "Your Steam friends list is empty or unavailable."
        );
        state.answer(Tag::First, Ok(list(12)));
        let view = state.view(&en());
        assert!(view.search);
        assert_eq!(view.counter, "12");
        let kinds: Vec<_> = view.entries.iter().map(|e| (e.kind, e.count)).collect();
        assert_eq!(kinds[0], ("title", 3));
        assert_eq!(view.entries[0].text, "Friends in HLSR");
        assert_eq!(
            kinds.iter().filter(|(kind, _)| *kind == "friend").count(),
            12
        );
        // Поиск по имени без учёта регистра и по SteamID.
        state.set_query("  NAME1 ");
        let view = state.view(&en());
        let names: Vec<_> = view
            .entries
            .iter()
            .filter(|e| e.kind == "friend")
            .map(|e| e.text.clone())
            .collect();
        assert_eq!(names, ["Name1", "Name10", "Name11"]);
        state.set_query("zzz");
        assert_eq!(state.view(&en()).entries[0].text, "Nobody found.");
        state.set_query("0000");
        assert_eq!(
            state
                .view(&en())
                .entries
                .iter()
                .filter(|e| e.kind == "friend")
                .count(),
            12
        );
        state.set_query("000000007");
        assert_eq!(
            state
                .view(&en())
                .entries
                .iter()
                .filter(|e| e.kind == "friend")
                .count(),
            1
        );
    }

    #[test]
    fn a_rejected_list_stops_the_spinner() {
        let mut state = FriendsState::new();
        assert!(state.answer(Tag::First, Err("x".into())).is_empty());
        let view = state.view(&en());
        assert!(!view.loading);
        // Список пуст, вход по умолчанию — «не вошёл».
        assert_eq!(view.entries[0].text, "Not signed in to Steam");
    }

    #[test]
    fn letters_follow_the_name() {
        assert_eq!(letter_of("anna"), "A");
        assert_eq!(letter_of("  ёлка"), "Ё");
        assert_eq!(letter_of(""), "?");
        assert_eq!(letter_of("   "), "?");
    }
}
