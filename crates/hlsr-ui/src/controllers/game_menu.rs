//! Меню игры (`GameMenu.vue` 0.4.9): десять пунктов, доступность и причина недоступности из
//! ответа `game-menu-state`; при выбранном эпизоде Ghosting «Удалить» снимает только эпизод.
//! Меню над кнопкой у её правого края; в низком окне пункты сжимаются по ступеням
//! ([`Density`], место — `overlay::fit_game_menu`).
//! Вид — `hlsr-gpui/src/games/game_menu.rs`.

use super::jsv;
use crate::tr::tr;
use hlsr_core::js;
use hlsr_core::language::Language;
use serde_json::Value;

/// Описание пункта (`ITEMS`).
pub struct ItemDef {
    pub id: &'static str,
    pub icon: &'static str,
    pub label: &'static str,
    pub danger: bool,
    pub separated: bool,
}

/// `ITEMS`: порядок как в меню.
pub const ITEMS: [ItemDef; 10] = [
    ItemDef {
        id: "settings",
        icon: "fa-sliders-h",
        label: "#L_GM_SETTINGS",
        danger: false,
        separated: false,
    },
    ItemDef {
        id: "shortcut",
        icon: "fa-external-link-alt",
        label: "#L_GM_SHORTCUT",
        danger: false,
        separated: false,
    },
    ItemDef {
        id: "screenshots",
        icon: "fa-image",
        label: "#L_GM_SCREENSHOTS",
        danger: false,
        separated: false,
    },
    ItemDef {
        id: "folder",
        icon: "fa-folder-open",
        label: "#L_GM_FOLDER",
        danger: false,
        separated: false,
    },
    ItemDef {
        id: "verify",
        icon: "fa-check-double",
        label: "#L_GM_VERIFY",
        danger: false,
        separated: false,
    },
    ItemDef {
        id: "repair",
        icon: "fa-wrench",
        label: "#L_GM_REPAIR",
        danger: false,
        separated: false,
    },
    ItemDef {
        id: "exportBuild",
        icon: "fa-file-archive",
        label: "#L_GM_EXPORT_BUILD",
        danger: false,
        separated: true,
    },
    ItemDef {
        id: "exportSettings",
        icon: "fa-file-export",
        label: "#L_GM_EXPORT_SETTINGS",
        danger: false,
        separated: false,
    },
    ItemDef {
        id: "importBuild",
        icon: "fa-file-import",
        label: "#L_GM_IMPORT_BUILD",
        danger: false,
        separated: false,
    },
    ItemDef {
        id: "uninstall",
        icon: "fa-trash-alt",
        label: "#L_GM_UNINSTALL",
        danger: true,
        separated: true,
    },
];

/// `REASONS`: причина недоступности → ключ подсказки.
fn reason_key(reason: &str) -> &'static str {
    match reason {
        "not-installed" => "#L_GM_DISABLED_NOT_INSTALLED",
        "busy" => "#L_GM_DISABLED_BUSY",
        "game-running" => "#L_GM_DISABLED_GAME_RUNNING",
        "steam-managed" => "#L_GM_DISABLED_STEAM_MANAGED",
        "in-development" => "#L_GM_DISABLED_IN_DEVELOPMENT",
        "episode-not-installed" => "#L_GM_DISABLED_EPISODE",
        // Незнакомая причина — как «занято».
        _ => "#L_GM_DISABLED_BUSY",
    }
}

/// Пункт меню: подпись и состояние.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub id: &'static str,
    pub icon: &'static str,
    pub label: String,
    pub enabled: bool,
    pub danger: bool,
    pub separated: bool,
    /// Причина недоступности (подсказка недоступного пункта); пусто, если причины нет.
    pub hint: String,
}

/// `items`: пункты по ответу `game-menu-state` (`None` — ответа ещё нет). Пункт, о котором
/// main не сказал, недоступен: «доступно после установки сборки». `episode` — выбран эпизод
/// Ghosting (`Some(installed)`): «Удалить эпизод» с теми же запретами, что у игры, и только
/// скачанный.
pub fn items(lang: Language, state: Option<&Value>, episode: Option<bool>) -> Vec<MenuItem> {
    let states = jsv::field(state, "items").filter(|items| js::truthy(items));
    ITEMS
        .iter()
        .map(|item| {
            let info = states
                .and_then(|states| jsv::prop(states, item.id))
                .filter(|info| js::truthy(info));
            let mut enabled = info.is_some_and(|info| jsv::truthy(jsv::prop(info, "enabled")));
            let reason = match info {
                Some(info) => jsv::prop(info, "reason").filter(|reason| js::truthy(reason)),
                None => None,
            };
            let mut hint = match (info, reason) {
                // `{enabled: false, reason: "not-installed"}` для пункта без ответа.
                (None, _) => tr(lang, reason_key("not-installed"), &[]),
                (Some(_), Some(reason)) => tr(
                    lang,
                    reason_key(jsv::str_of(Some(reason)).unwrap_or_default()),
                    &[],
                ),
                (Some(_), None) => String::new(),
            };
            let episode = episode.filter(|_| item.id == "uninstall");
            if enabled && episode == Some(false) {
                enabled = false;
                hint = tr(lang, reason_key("episode-not-installed"), &[]);
            }
            let label = if episode.is_some() {
                "#L_GM_UNINSTALL_EPISODE"
            } else {
                item.label
            };
            MenuItem {
                id: item.id,
                icon: item.icon,
                label: tr(lang, label, &[]),
                enabled,
                danger: item.danger,
                separated: item.separated,
                hint,
            }
        })
        .collect()
}

/// Ступень плотности меню (`fit.size`): `""`, `compact`, `tight`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    Normal,
    Compact,
    Tight,
}

/// Ступени по порядку (`step` у `overlay::fit_game_menu`).
pub const DENSITIES: [Density; 3] = [Density::Normal, Density::Compact, Density::Tight];

impl Density {
    /// Поле пункта сверху и снизу, сторона плашки значка, зазор между пунктами, отступ над
    /// пунктом с линией и линия над ним (CSS px).
    pub fn metrics(self) -> (f32, f32, f32, f32, f32) {
        match self {
            Density::Normal => (8., 32., 4., 8., -7.),
            Density::Compact => (6., 30., 3., 7., -5.),
            Density::Tight => (3., 28., 2., 6., -5.),
        }
    }

    /// Высота меню с полями 8 (`scrollHeight`): пункт — плашка значка (строка 20 ниже её);
    /// подписи в одну строку (`width: max-content`).
    pub fn height(self) -> f64 {
        let (pad, icon, gap, separated, _) = self.metrics();
        let count = ITEMS.len() as f32;
        let lines = ITEMS.iter().filter(|item| item.separated).count() as f32;
        f64::from(16. + count * (2. * pad + icon) + (count - 1.) * gap + lines * separated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state(overrides: Value) -> Value {
        let mut items = json!({
            "settings": {"enabled": true, "reason": null},
            "shortcut": {"enabled": true, "reason": null},
            "screenshots": {"enabled": true, "reason": null},
            "folder": {"enabled": true, "reason": null},
            "updates": {"enabled": true, "reason": null},
            "verify": {"enabled": true, "reason": null},
            "repair": {"enabled": false, "reason": "in-development"},
            "exportBuild": {"enabled": true, "reason": null},
            "exportSettings": {"enabled": true, "reason": null},
            "importBuild": {"enabled": true, "reason": null},
            "uninstall": {"enabled": true, "reason": null},
        });
        for (key, value) in overrides.as_object().unwrap() {
            items[key] = value.clone();
        }
        json!({"available": true, "gameId": "ghosting-renewed", "installed": true, "items": items})
    }

    fn row(items: &[MenuItem], id: &str) -> MenuItem {
        items.iter().find(|item| item.id == id).unwrap().clone()
    }

    #[test]
    fn ten_items_in_menu_order_with_verify() {
        let items = items(Language::En, Some(&state(json!({}))), None);
        let ids: Vec<_> = items.iter().map(|item| item.id).collect();
        assert_eq!(
            ids,
            [
                "settings",
                "shortcut",
                "screenshots",
                "folder",
                "verify",
                "repair",
                "exportBuild",
                "exportSettings",
                "importBuild",
                "uninstall"
            ]
        );
        let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Game settings",
                "Add desktop shortcut",
                "Open screenshots folder",
                "Open game folder",
                "Verify files",
                "Repair game",
                "Export build",
                "Export settings and cfg",
                "Import build",
                "Delete game",
            ]
        );
        // Линии над «Экспорт сборки» и «Удалить игру»; красный — только удаление.
        let separated: Vec<_> = items
            .iter()
            .filter(|item| item.separated)
            .map(|item| item.id)
            .collect();
        assert_eq!(separated, ["exportBuild", "uninstall"]);
        let danger: Vec<_> = items
            .iter()
            .filter(|item| item.danger)
            .map(|item| item.id)
            .collect();
        assert_eq!(danger, ["uninstall"]);
        // Значки — как в ITEMS (без «fas»: дописывает слой).
        assert_eq!(row(&items, "uninstall").icon, "fa-trash-alt");
        assert_eq!(row(&items, "settings").icon, "fa-sliders-h");
        assert_eq!(row(&items, "verify").icon, "fa-check-double");
    }

    #[test]
    fn a_selected_episode_turns_delete_into_delete_episode() {
        let installed = items(Language::Ru, Some(&state(json!({}))), Some(true));
        let delete = row(&installed, "uninstall");
        assert_eq!(delete.label, "Удалить эпизод");
        assert!(delete.enabled && delete.danger);
        // Не скачан — недоступен со своей причиной; остальные пункты не трогаются.
        let missing = items(Language::Ru, Some(&state(json!({}))), Some(false));
        let delete = row(&missing, "uninstall");
        assert_eq!(delete.label, "Удалить эпизод");
        assert!(!delete.enabled);
        assert_eq!(delete.hint, "Эпизод ещё не скачан");
        assert_eq!(row(&missing, "settings").label, "Настройки игры");
        // Запреты игры важнее: игра запущена — её причина.
        let running = state(json!({"uninstall": {"enabled": false, "reason": "game-running"}}));
        let delete = row(
            &items(Language::Ru, Some(&running), Some(false)),
            "uninstall",
        );
        assert_eq!(delete.hint, "Сначала закрой игру");
    }

    #[test]
    fn the_menu_packs_tighter_in_a_low_window() {
        let heights = DENSITIES.map(Density::height);
        // 0.4.9: плотная ступень 3 + 28 + 3 — меню над кнопкой влезает и в 1152×648.
        assert_eq!(heights, [548., 477., 386.]);
        let fit = |top: f64| crate::overlay::fit_game_menu(&heights, top, 80.);
        // Место = верх кнопки − 12 − низ шапки − 8.
        assert_eq!(fit(824.).step, 0);
        assert_eq!(fit(600.).step, 1);
        let tight = fit(500.);
        assert_eq!((tight.step, tight.scrolls), (2, false));
        assert!(fit(400.).scrolls);
    }

    #[test]
    fn availability_and_reasons_come_from_game_menu_state() {
        let items = items(Language::En, Some(&state(json!({}))), None);
        assert!(row(&items, "settings").enabled);
        assert_eq!(row(&items, "settings").hint, "");
        // «Восстановить» main пока отключает «в разработке».
        let repair = row(&items, "repair");
        assert!(!repair.enabled);
        assert_eq!(repair.hint, "In development, coming back soon");

        let busy = state(json!({
            "uninstall": {"enabled": false, "reason": "busy"},
            "verify": {"enabled": false, "reason": "game-running"},
            "exportBuild": {"enabled": false, "reason": "steam-managed"},
            "importBuild": {"enabled": false, "reason": "not-installed"},
            "folder": {"enabled": false, "reason": "something-new"},
        }));
        let items = super::items(Language::En, Some(&busy), None);
        assert_eq!(
            row(&items, "uninstall").hint,
            "Not available while downloading or installing"
        );
        assert_eq!(row(&items, "verify").hint, "Close the game first");
        assert_eq!(
            row(&items, "exportBuild").hint,
            "This build is verified and updated by Steam"
        );
        assert_eq!(
            row(&items, "importBuild").hint,
            "Available after the build is installed"
        );
        // Незнакомая причина — как «занято».
        assert_eq!(
            row(&items, "folder").hint,
            "Not available while downloading or installing"
        );
        let russian = super::items(Language::Ru, Some(&busy), None);
        assert_eq!(row(&russian, "verify").hint, "Сначала закрой игру");
    }

    #[test]
    fn without_an_answer_everything_waits_for_the_install() {
        for answer in [
            None,
            Some(json!(null)),
            Some(json!({})),
            Some(json!({"items": null})),
        ] {
            let items = items(Language::En, answer.as_ref(), None);
            assert_eq!(items.len(), 10);
            assert!(items.iter().all(|item| !item.enabled));
            assert!(
                items
                    .iter()
                    .all(|item| item.hint == "Available after the build is installed")
            );
        }
        // Пункт, о котором main не сказал, — недоступен, остальные как есть.
        let partial = json!({"items": {"settings": {"enabled": true, "reason": null}}});
        let items = items(Language::En, Some(&partial), None);
        assert!(row(&items, "settings").enabled);
        assert!(!row(&items, "folder").enabled);
        assert_eq!(
            row(&items, "folder").hint,
            "Available after the build is installed"
        );
    }
}
