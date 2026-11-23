//! Запасные данные игры в оболочке (`launcher-data.js` 0.4.7: `demoState`, `heroSubtitles`;
//! `LauncherShell.vue`: `splitTitle`, `getPlayTime`, `fallbackHours`): пока статус доставки и
//! статистика ещё не пришли, герой и нижняя панель показывают их.

use super::game_availability::GAME_LIST;
use hlsr_core::js;
use serde_json::Value;

/// `demo.download`: сколько скачано и сколько всего, МБ.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Download {
    pub done: f64,
    pub total: f64,
}

/// `launcherData.demoState[id]`; для игры без записи — пустой (`{}`): сборок нет.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GameDemo {
    /// `lastLaunchDays`: `None` — «Никогда».
    pub last_launch_days: Option<u32>,
    /// `buildUpdated`: готовая строка даты; `None` — пустое место.
    pub build_updated: Option<&'static str>,
    pub builds: &'static [&'static str],
    /// Сборки, которые появятся позже (видны в списке, не выбираются).
    pub upcoming: &'static [&'static str],
    pub download: Option<Download>,
    /// `episodes` 0.4.9: сборка «Версии сборки» → эпизод Ghosting (`"ep1" | "ep2"`).
    pub episodes: &'static [(&'static str, &'static str)],
}

/// `GHOSTING_EPISODES` 0.4.9: эпизоды Ghosting — дополнения её установки; выбор хранит
/// лаунчер (`prefs.ghostingBuild`).
pub const GHOSTING_EPISODES: &[(&str, &str)] = &[("Episode One", "ep1"), ("Episode Two", "ep2")];

impl GameDemo {
    /// `(demo.episodes || {})[build] || null`: эпизод выбранной сборки.
    pub fn episode_of(&self, build: &str) -> Option<&'static str> {
        self.episodes
            .iter()
            .find(|(name, _)| *name == build)
            .map(|(_, id)| *id)
    }
}

/// `launcherData.demoState[id] || {}`.
pub fn demo_state(id: &str) -> GameDemo {
    match id {
        "70" => GameDemo {
            builds: &["WON 2005"],
            ..GameDemo::default()
        },
        "50" => GameDemo {
            last_launch_days: Some(6),
            build_updated: Some("02.02.2025 19:05"),
            builds: &["GoldSrc Package", "WON", "Steam Legacy"],
            download: Some(Download {
                done: 513.0,
                total: 513.0,
            }),
            ..GameDemo::default()
        },
        "130" => GameDemo {
            build_updated: Some("17.01.2025 12:44"),
            builds: &["GoldSrc Package", "Steam Legacy"],
            download: Some(Download {
                done: 285.0,
                total: 285.0,
            }),
            ..GameDemo::default()
        },
        "220" => GameDemo {
            builds: &["4260", "2707", "5288"],
            ..GameDemo::default()
        },
        "218" => GameDemo {
            builds: &["HL1 MOVEMENT", "Episode One", "Episode Two"],
            episodes: GHOSTING_EPISODES,
            ..GameDemo::default()
        },
        _ => GameDemo::default(),
    }
}

/// `launcherData.heroSubtitles[id]`.
pub fn hero_subtitle_override(id: &str) -> Option<&'static str> {
    (id == "218").then_some("Ghosting Mod: Renewed")
}

/// `fallbackHours[id] || "0"`.
pub fn fallback_hours(id: &str) -> &'static str {
    match id {
        "70" => "36,5",
        "50" => "8,2",
        "220" => "124",
        _ => "0",
    }
}

/// `getPlayTime(gameId)`: часы игры из `library.json` (`config[id].inGameTime`, мс) с одним
/// знаком и запятой, иначе запасные.
pub fn play_time(id: &str, in_game_time_ms: Option<&Value>) -> String {
    match in_game_time_ms {
        Some(value) if js::truthy(value) => {
            let ms = js::value::number(value).unwrap_or(f64::NAN);
            js::to_fixed(ms / 3_600_000.0, 1).replace('.', ",")
        }
        _ => fallback_hours(id).to_owned(),
    }
}

/// `splitTitle(name)`: до первого «:» и после него (вторая часть пустая — `null`); лишнее
/// после второго «:» отбрасывается, как у `split(":")[1]`.
pub fn split_title(name: &str) -> (String, Option<String>) {
    let mut parts = name.split(':');
    let main = js::trim(parts.next().unwrap_or_default()).to_owned();
    let sub = parts
        .next()
        .filter(|part| !part.is_empty())
        .map(|part| js::trim(part).to_owned());
    (main, sub)
}

/// Игра, как её видит герой и нижняя панель (`games` оболочки).
#[derive(Debug, Clone, PartialEq)]
pub struct GameHeader {
    pub id: &'static str,
    pub name: &'static str,
    pub hero_title: String,
    pub hero_subtitle: Option<String>,
    pub demo: GameDemo,
    /// `playtimeHours`: готовая строка («36,5»).
    pub playtime_hours: String,
}

impl GameHeader {
    /// `currentGameFullTitle` оболочки: «Заголовок: подзаголовок» героя; название в окнах настроек
    /// игры и заметок.
    pub fn full_title(&self) -> String {
        match &self.hero_subtitle {
            Some(subtitle) => format!("{}: {subtitle}", self.hero_title),
            None => self.hero_title.clone(),
        }
    }
}

/// Заголовок игры списка `GAME_LIST`; `in_game_time_ms` — `library.config[id].inGameTime`.
pub fn header(id: &str, in_game_time_ms: Option<&Value>) -> Option<GameHeader> {
    let game = GAME_LIST.iter().find(|game| game.id == id)?;
    let (main, sub) = split_title(game.name);
    Some(GameHeader {
        id: game.id,
        name: game.name,
        hero_title: main,
        hero_subtitle: hero_subtitle_override(game.id).map(str::to_owned).or(sub),
        demo: demo_state(game.id),
        playtime_hours: play_time(game.id, in_game_time_ms),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn titles_split_at_the_first_colon_and_ghosting_has_its_own_subtitle() {
        assert_eq!(split_title("Half-Life"), ("Half-Life".into(), None));
        assert_eq!(
            split_title("Half-Life: Opposing Force"),
            ("Half-Life".into(), Some("Opposing Force".into()))
        );
        assert_eq!(split_title("A: B: C"), ("A".into(), Some("B".into())));
        assert_eq!(split_title("A:"), ("A".into(), None));
        assert_eq!(split_title(" A : B "), ("A".into(), Some("B".into())));

        let ghosting = header("218", None).unwrap();
        assert_eq!(ghosting.hero_title, "Half-Life 2");
        assert_eq!(
            ghosting.hero_subtitle.as_deref(),
            Some("Ghosting Mod: Renewed")
        );
        let hl2 = header("220", None).unwrap();
        assert_eq!(hl2.hero_title, "Half-Life 2");
        assert_eq!(hl2.hero_subtitle, None);
        let op4 = header("50", None).unwrap();
        assert_eq!(op4.hero_subtitle.as_deref(), Some("Opposing Force"));
        assert!(header("999", None).is_none());
    }

    #[test]
    fn play_time_is_hours_with_one_decimal_and_a_comma() {
        assert_eq!(play_time("70", Some(&json!(5_400_000))), "1,5");
        assert_eq!(play_time("70", Some(&json!(3_600_000 * 124))), "124,0");
        // inGameTime 0 или нет записи — запасные часы игры.
        assert_eq!(play_time("70", Some(&json!(0))), "36,5");
        assert_eq!(play_time("50", None), "8,2");
        assert_eq!(play_time("130", None), "0");
        assert_eq!(play_time("220", Some(&json!(null))), "124");
        assert_eq!(play_time("218", None), "0");
    }

    #[test]
    fn demo_state_matches_launcher_data() {
        let hl2 = demo_state("220");
        assert_eq!(hl2.builds, ["4260", "2707", "5288"]);
        assert!(hl2.upcoming.is_empty() && hl2.episodes.is_empty());
        // Эпизоды Ghosting — сборки её списка на Windows и Linux.
        let ghosting = demo_state("218");
        assert_eq!(
            ghosting.builds,
            ["HL1 MOVEMENT", "Episode One", "Episode Two"]
        );
        assert_eq!(ghosting.episode_of("Episode One"), Some("ep1"));
        assert_eq!(ghosting.episode_of("Episode Two"), Some("ep2"));
        assert_eq!(ghosting.episode_of("HL1 MOVEMENT"), None);
        assert_eq!(demo_state("50").last_launch_days, Some(6));
        assert_eq!(
            demo_state("130").download,
            Some(Download {
                done: 285.0,
                total: 285.0
            })
        );
        // Эпизоды без записи: пустое состояние.
        assert_eq!(demo_state("380"), GameDemo::default());
    }
}
