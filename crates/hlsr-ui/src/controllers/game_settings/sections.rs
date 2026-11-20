//! Разделы страницы «Персонализация» (`GameSettingsModal.vue` embedded 0.4.9: `allSections`,
//! `sections`, `activeSection`, `footerSaves`, `direction`). Окна клавиатуры поверх настроек
//! больше нет: «Клавиатура» — обычный раздел.

use super::draft::{GHOSTING_GAME_ID, GOLDSRC_GAME_IDS};

/// Раздел списка слева.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section {
    pub id: &'static str,
    pub icon: &'static str,
    pub title: &'static str,
    only: Only,
}

/// У каких игр есть раздел.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Only {
    All,
    /// Внешний вид игры (`hasPersonal`) и «Демо» (`hasDemos`): HL1 и Ghosting.
    GoldSrcOrGhosting,
    Ghosting,
    Keyboard,
    Scripts,
}

const fn section(id: &'static str, icon: &'static str, title: &'static str, only: Only) -> Section {
    Section {
        id,
        icon,
        title,
        only,
    }
}

/// `allSections`: порядок списка.
pub const ALL: [Section; 8] = [
    section(
        "personal",
        "fas fa-palette",
        "#L_GS_S_PERSONAL",
        Only::GoldSrcOrGhosting,
    ),
    section("configs", "fas fa-file-code", "#L_GS_S_CONFIGS", Only::All),
    section(
        "keyboard",
        "fas fa-keyboard",
        "#L_GS_S_KEYBOARD",
        Only::Keyboard,
    ),
    section("scripts", "fas fa-scroll", "#L_GS_S_SCRIPTS", Only::Scripts),
    section("launch", "fas fa-play", "#L_GS_S_LAUNCH", Only::All),
    section(
        "demos",
        "fas fa-film",
        "#L_GS_S_DEMOS",
        Only::GoldSrcOrGhosting,
    ),
    section("ghosts", "fas fa-ghost", "#L_GS_S_GHOSTS", Only::Ghosting),
    section(
        "version",
        "fas fa-code-branch",
        "#L_GS_S_VERSION",
        Only::Ghosting,
    ),
];

/// От чего зависят разделы.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Context<'a> {
    /// Идентификатор игры в лаунчере («70», «220», «218»).
    pub game_id: &'a str,
    /// Есть заготовки биндов для игры или выбранной сборки (`hasKeyboard`).
    pub has_keyboard: bool,
    /// Скрипты гайда есть только у Half-Life 1 (`hasScripts`).
    pub has_scripts: bool,
}

impl Context<'_> {
    pub fn goldsrc(&self) -> bool {
        GOLDSRC_GAME_IDS.contains(&self.game_id)
    }

    pub fn ghosting(&self) -> bool {
        self.game_id == GHOSTING_GAME_ID
    }
}

/// `sections`.
pub fn sections(context: &Context<'_>) -> Vec<Section> {
    ALL.iter()
        .filter(|item| match item.only {
            Only::All => true,
            Only::GoldSrcOrGhosting => context.goldsrc() || context.ghosting(),
            Only::Ghosting => context.ghosting(),
            Only::Keyboard => context.has_keyboard,
            Only::Scripts => context.has_scripts,
        })
        .copied()
        .collect()
}

/// `activeSection`: выбранный раздел или первый, если такого у игры нет.
pub fn active_section(sections: &[Section], current: &str) -> &'static str {
    sections
        .iter()
        .find(|item| item.id == current)
        .map_or_else(|| sections[0].id, |item| item.id)
}

/// `footerSaves`: «Демо», «Призраки» и «Версия» ничего не сохраняют через подвал.
pub fn footer_saves(id: &str) -> bool {
    !matches!(id, "demos" | "ghosts" | "version")
}

/// `selectSection`: переход между «Персонализацией» (своё «Сохранить») и прочими разделами
/// (общее «Сохранить») спрашивает о несохранённом там, откуда уходят — `"personal"` или
/// `"settings"`; `None` — спрашивать не о чем.
pub fn leave_part(from: &str, to: &str) -> Option<&'static str> {
    if from == to {
        None
    } else if from == "personal" {
        Some("personal")
    } else if to == "personal" {
        Some("settings")
    } else {
        None
    }
}

/// Куда уезжает содержимое при смене раздела (`direction` `tab-ink.js`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Next,
    Prev,
}

/// «next», если новый раздел ниже прежнего или тот же, иначе «prev». Нет раздела в списке —
/// `indexOf` даёт −1.
pub fn direction(ids: &[&str], from: &str, to: &str) -> Direction {
    let index = |id: &str| {
        ids.iter()
            .position(|known| *known == id)
            .map_or(-1, |i| i as i32)
    };
    if index(to) >= index(from) {
        Direction::Next
    } else {
        Direction::Prev
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(context: Context<'_>) -> Vec<&'static str> {
        sections(&context).iter().map(|item| item.id).collect()
    }

    #[test]
    fn sections_depend_on_the_game_and_the_build() {
        let ghosting = Context {
            game_id: "218",
            has_keyboard: true,
            has_scripts: false,
        };
        assert_eq!(
            ids(ghosting),
            [
                "personal", "configs", "keyboard", "launch", "demos", "ghosts", "version"
            ]
        );
        let hl1 = Context {
            game_id: "70",
            has_keyboard: true,
            has_scripts: true,
        };
        assert_eq!(
            ids(hl1),
            [
                "personal", "configs", "keyboard", "scripts", "launch", "demos"
            ]
        );
        // HL2: сборка без заготовок биндов — без клавиатуры (`hasKeyboard`).
        let hl2 = Context {
            game_id: "220",
            has_keyboard: false,
            has_scripts: false,
        };
        assert_eq!(ids(hl2), ["configs", "launch"]);
        assert_eq!(
            ids(Context {
                has_keyboard: true,
                ..hl2
            }),
            ["configs", "keyboard", "launch"]
        );
    }

    #[test]
    fn an_unknown_section_falls_back_to_the_first_and_some_have_no_footer() {
        let list = sections(&Context {
            game_id: "218",
            has_keyboard: false,
            has_scripts: false,
        });
        assert_eq!(active_section(&list, "launch"), "launch");
        assert_eq!(active_section(&list, "scripts"), "personal");
        assert_eq!(active_section(&list, ""), "personal");
        assert!(footer_saves("launch") && footer_saves("configs"));
        assert!(!footer_saves("demos") && !footer_saves("ghosts") && !footer_saves("version"));
    }

    #[test]
    fn only_crossing_the_personal_border_asks_about_unsaved_changes() {
        assert_eq!(leave_part("personal", "launch"), Some("personal"));
        assert_eq!(leave_part("configs", "personal"), Some("settings"));
        assert_eq!(leave_part("configs", "launch"), None);
        assert_eq!(leave_part("personal", "personal"), None);
    }

    #[test]
    fn slide_goes_down_to_a_section_below() {
        let ids = ["configs", "launch", "ghosts"];
        assert_eq!(direction(&ids, "configs", "launch"), Direction::Next);
        assert_eq!(direction(&ids, "ghosts", "launch"), Direction::Prev);
        assert_eq!(direction(&ids, "launch", "launch"), Direction::Next);
    }
}
