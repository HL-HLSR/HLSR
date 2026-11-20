//! Герой игры (`HeroPanel.vue`, в 0.4.9 как в 0.4.7): «HALF-LIFE» общий у всех игр и при смене
//! игры стоит на месте, а окончание названия, подзаголовок и значения статистики перетекают на
//! месте (переход `l-value`, вид — `hlsr-gpui/src/games/hero.rs`). Здесь — разбор названия и
//! данные героя.

use super::game_demo::GameHeader;
use super::game_stats::{self, StatsRow};
use chrono::TimeZone;
use hlsr_core::language::Language;
use serde_json::Value;

/// Название игры по частям: общая часть серии и окончание.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeroTitle {
    pub base: String,
    pub rest: String,
}

/// `title`: `/^(Half-Life)(.*)$/i` — «Half-Life» и всё после него («Half-Life 2» →
/// «Half-Life» и « 2»); любое другое название — целиком в `base`.
pub fn title_parts(hero_title: &str) -> HeroTitle {
    const SERIES: &str = "half-life";
    let whole = HeroTitle {
        base: hero_title.to_owned(),
        rest: String::new(),
    };
    let Some(head) = hero_title.get(..SERIES.len()) else {
        return whole;
    };
    let rest = &hero_title[SERIES.len()..];
    // `.` не принимает перевод строки, `$` без флага `m` — только конец текста.
    let single_line = !rest.contains(['\n', '\r', '\u{2028}', '\u{2029}']);
    if head.eq_ignore_ascii_case(SERIES) && single_line {
        HeroTitle {
            base: head.to_owned(),
            rest: rest.to_owned(),
        }
    } else {
        whole
    }
}

/// Всё, что показывает герой.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeroView {
    pub title: HeroTitle,
    /// `game.heroSubtitle || ""`: строка есть всегда (иначе у игр без подзаголовка
    /// статистика подпрыгивала бы вверх).
    pub subtitle: String,
    pub rows: [StatsRow; 3],
}

/// Герой игры `game` по ответам main (`stats` — `game-stats-get`, `delivery` — `DeliveryStatus`).
pub fn view<Tz: TimeZone>(
    lang: Language,
    game: &GameHeader,
    stats: Option<&Value>,
    delivery: Option<&Value>,
    tz: &Tz,
    now_ms: i64,
) -> HeroView {
    HeroView {
        title: title_parts(&game.hero_title),
        subtitle: game.hero_subtitle.clone().unwrap_or_default(),
        rows: game_stats::rows(&game_stats::Input {
            lang,
            demo: &game.demo,
            playtime_hours: &game.playtime_hours,
            stats,
            delivery,
            tz,
            now_ms,
        }),
    }
}

/// [`view`] по часам и поясу системы (`new Date()` героя).
pub fn view_local(
    lang: Language,
    game: &GameHeader,
    stats: Option<&Value>,
    delivery: Option<&Value>,
) -> HeroView {
    let now = chrono::Utc::now().timestamp_millis();
    view(lang, game, stats, delivery, &chrono::Local, now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controllers::game_demo::header;
    use chrono::Utc;
    use serde_json::json;

    #[test]
    fn the_series_name_stays_and_the_ending_swaps() {
        let hl = title_parts("Half-Life");
        assert_eq!((hl.base.as_str(), hl.rest.as_str()), ("Half-Life", ""));
        let hl2 = title_parts("Half-Life 2");
        assert_eq!((hl2.base.as_str(), hl2.rest.as_str()), ("Half-Life", " 2"));
        // Регистр совпадения сохраняется (его потом поднимает text-transform).
        let loud = title_parts("HALF-LIFE 2");
        assert_eq!(
            (loud.base.as_str(), loud.rest.as_str()),
            ("HALF-LIFE", " 2")
        );
        // Другие названия и короткие строки — целиком.
        let other = title_parts("Portal");
        assert_eq!((other.base.as_str(), other.rest.as_str()), ("Portal", ""));
        assert_eq!(title_parts("Half").base, "Half");
        assert_eq!(title_parts("").base, "");
        // Название в середине не в счёт; перевод строки ломает `(.*)$`.
        assert_eq!(title_parts("My Half-Life").base, "My Half-Life");
        assert_eq!(title_parts("Half-Life\n2").rest, "");
        // Не-ASCII после серии не режется посреди символа.
        assert_eq!(title_parts("Half-Life — 2").rest, " — 2");
        assert_eq!(title_parts("Half-Lif\u{e9}").base, "Half-Lif\u{e9}");
    }

    #[test]
    fn hero_of_each_game_has_a_subtitle_slot() {
        let now = 1_792_152_000_000;
        let hl = header("70", None).unwrap();
        let view = view(Language::En, &hl, None, None, &Utc, now);
        assert_eq!(view.title.base, "Half-Life");
        assert_eq!(view.subtitle, "");
        assert_eq!(view.rows[1].value, "36,5 h");

        let ghosting = header("218", Some(&json!(7_200_000))).unwrap();
        let view = view_with_stats(&ghosting, now);
        assert_eq!(view.title.rest, " 2");
        assert_eq!(view.subtitle, "Ghosting Mod: Renewed");
        assert_eq!(view.rows[0].value, "Today");
        assert_eq!(view.rows[1].value, "2 h 0 min");
    }

    fn view_with_stats(game: &GameHeader, now: i64) -> HeroView {
        let stats = json!({
            "available": true,
            "lastLaunchAt": "2026-10-16T08:00:00.000Z",
            "totalPlaySeconds": 7200,
        });
        view(Language::En, game, Some(&stats), None, &Utc, now)
    }
}
