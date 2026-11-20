//! Выбор сборки (`BuildSelector.vue` 0.4.9): список сборок игры (справа — размер нескачанного
//! эпизода, `badges`) и сборки «скоро» (видны, но не выбираются). Выпадающий список — `Dropdown`
//! набора (`hlsr-gpui/src/games/build_selector.rs`).

use crate::tr::tr;
use hlsr_core::language::Language;

/// Пункт списка сборок.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildItem {
    pub id: String,
    pub label: String,
    pub disabled: bool,
    /// Линия над первой из «скоро».
    pub separated: bool,
    pub badge: String,
    /// Подсказка пункта «скоро»: «Episode One — Появится позже».
    pub hint: String,
}

/// `items`: готовые сборки, затем «скоро»; `badges` — подпись справа по имени сборки.
pub fn items(
    lang: Language,
    builds: &[&str],
    upcoming: &[&str],
    badges: &[(&str, String)],
) -> Vec<BuildItem> {
    let ready = builds.iter().map(|name| BuildItem {
        id: (*name).to_owned(),
        label: (*name).to_owned(),
        disabled: false,
        separated: false,
        badge: badges
            .iter()
            .find(|(build, _)| build == name)
            .map(|(_, badge)| badge.clone())
            .unwrap_or_default(),
        hint: String::new(),
    });
    let soon = upcoming.iter().enumerate().map(|(index, name)| BuildItem {
        id: format!("upcoming:{name}"),
        label: (*name).to_owned(),
        disabled: true,
        separated: index == 0,
        badge: tr(lang, "#L_BUILD_SOON", &[]),
        hint: format!("{name} — {}", tr(lang, "#L_BUILD_SOON_HINT", &[])),
    });
    ready.chain(soon).collect()
}

/// `effectiveValue`: выбранная сборка, а если её в списке нет — первая; сборок нет — пусто.
pub fn effective_value(builds: &[&str], value: &str) -> String {
    if builds.contains(&value) {
        value.to_owned()
    } else {
        builds
            .first()
            .map(|name| (*name).to_owned())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_builds_come_first_then_the_soon_ones_behind_a_rule() {
        let items = items(
            Language::En,
            &["4260", "2707", "5288"],
            &["Episode One", "Episode Two"],
            &[],
        );
        let ids: Vec<_> = items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "4260",
                "2707",
                "5288",
                "upcoming:Episode One",
                "upcoming:Episode Two"
            ]
        );
        assert!(
            items[..3]
                .iter()
                .all(|item| !item.disabled && item.badge.is_empty())
        );
        assert!(items[3..].iter().all(|item| item.disabled));
        let separated: Vec<_> = items.iter().map(|item| item.separated).collect();
        assert_eq!(separated, [false, false, false, true, false]);
        assert_eq!(items[3].badge, "Soon");
        assert_eq!(items[3].hint, "Episode One — Coming later");
        assert_eq!(items[3].label, "Episode One");

        let russian = super::items(Language::Ru, &["4260"], &["Episode Two"], &[]);
        assert_eq!(russian[1].badge, "Скоро");
        assert_eq!(russian[1].hint, "Episode Two — Появится позже");
    }

    #[test]
    fn episodes_carry_their_size_on_the_right() {
        let badges = [("Episode One", "2,7\u{a0}ГБ".to_owned())];
        let items = items(
            Language::Ru,
            &["HL1 MOVEMENT", "Episode One", "Episode Two"],
            &[],
            &badges,
        );
        let got: Vec<_> = items.iter().map(|item| item.badge.as_str()).collect();
        assert_eq!(got, ["", "2,7\u{a0}ГБ", ""]);
        assert!(items.iter().all(|item| !item.disabled && !item.separated));
    }

    #[test]
    fn the_selection_falls_back_to_the_first_build() {
        let builds = ["GoldSrc Package", "WON", "Steam Legacy"];
        assert_eq!(effective_value(&builds, "WON"), "WON");
        assert_eq!(effective_value(&builds, "4260"), "GoldSrc Package");
        assert_eq!(effective_value(&builds, ""), "GoldSrc Package");
        assert_eq!(effective_value(&[], "WON"), "");
    }
}
