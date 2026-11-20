//! Обучение по вкладке «Персонализация» (`config-tour.js` и расчёт места `ConfigTour.vue` 0.4.9):
//! какие шаги есть у игры и в каком порядке, где встаёт подсветка и карточка. Якорь шага — элемент
//! с именем `anchor` (`data-tour` 0.4.9); шаг, чьего якоря нет на экране, обучение пропускает.

/// `GHOSTING_ID`.
pub const GHOSTING_ID: &str = "ghosting-renewed";

/// Поля подсветки вокруг элемента (`PAD`), CSS px.
pub const PAD: f32 = 6.;
/// Отступ карточки от подсветки (`GAP`).
pub const GAP: f32 = 12.;
/// Отступ карточки от края окна (`EDGE`).
pub const EDGE: f32 = 12.;
/// Ширина карточки (`.tour__card`, не шире окна − 24).
pub const CARD_WIDTH: f32 = 340.;
/// Сколько ждать якорь, когда раздел не менялся (`WAIT_MS`).
pub const WAIT_MS: u64 = 400;
/// После смены раздела вкладки окна появляются не сразу (`WAIT_SWITCH_MS`).
pub const WAIT_SWITCH_MS: u64 = 1200;
/// Шаг ожидания якоря.
pub const POLL_MS: u64 = 40;
/// Автозапуск после обновления ждёт столько (`configTourDue`: за приветствием идёт другое окно).
pub const DUE_DELAY_MS: u64 = 1200;

/// Шаг обучения.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TourStep {
    pub id: &'static str,
    /// Имя якоря (`data-tour`).
    pub anchor: &'static str,
    /// Раздел шапки шага (`None` — не трогать: первый шаг подсвечивает саму вкладку).
    pub section: Option<&'static str>,
    /// Раздел списка слева, который страница открывает на шаге (`pageSection`).
    pub page_section: Option<&'static str>,
    pub title: String,
    pub text: String,
}

/// `PAGE_SECTION`: у выбора игры и импорта с экспортом это «Конфиг», где стоят их кнопки.
fn page_section(id: &str) -> Option<&'static str> {
    Some(match id {
        "gs-personal" => "personal",
        "gs-configs" | "configs-import" | "configs-export" => "configs",
        "gs-keyboard" => "keyboard",
        "gs-scripts" => "scripts",
        "gs-launch" => "launch",
        "gs-demos" => "demos",
        "gs-ghosts" => "ghosts",
        _ => return None,
    })
}

/// `stepsFor(engine, gameId)`: `engine` — «goldsrc» | «source» | «source-old», `game_id` — id
/// сборки («hl1-won»…).
pub fn steps_for(engine: &str, game_id: &str) -> Vec<TourStep> {
    let mut ids = vec!["configs-tab", "configs-games"];
    // Внешний вид и демо: HL1 и Ghosting.
    let has_look = engine == "goldsrc" || game_id == GHOSTING_ID;
    if has_look {
        ids.push("gs-personal");
    }
    ids.extend([
        "gs-configs",
        "configs-import",
        "configs-export",
        "gs-keyboard",
    ]);
    // Скрипты гайда есть только у HL1.
    if engine == "goldsrc" {
        ids.push("gs-scripts");
    }
    ids.push("gs-launch");
    if has_look {
        ids.push("gs-demos");
    }
    if game_id == GHOSTING_ID {
        ids.push("gs-ghosts");
    }
    ids.into_iter()
        .map(|id| {
            let key = id.to_uppercase().replace('-', "_");
            TourStep {
                id,
                anchor: id,
                section: (id != "configs-tab").then_some("configs"),
                page_section: page_section(id),
                title: format!("#L_TOUR_{key}_TITLE"),
                text: format!("#L_TOUR_{key}_TEXT"),
            }
        })
        .collect()
}

/// Прямоугольник в CSS px окна.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

/// Подсветка и левый верхний угол карточки.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub ring: Rect,
    pub card: (f32, f32),
}

/// `place()`: подсветка — якорь с полями [`PAD`]; карточка под ней, не влезает — над ней, нигде —
/// у нижнего края окна; по горизонтали от левого края подсветки, в пределах окна.
pub fn place(anchor: Rect, card: (f32, f32), base: (f32, f32)) -> Placement {
    let ring = Rect {
        left: anchor.left - PAD,
        top: anchor.top - PAD,
        width: anchor.width + PAD * 2.,
        height: anchor.height + PAD * 2.,
    };
    let (cw, ch) = card;
    let below = ring.top + ring.height + GAP;
    let above = ring.top - GAP - ch;
    let top = if below + ch <= base.1 - EDGE {
        below
    } else if above >= EDGE {
        above
    } else {
        base.1 - ch - EDGE
    };
    // Math.max(EDGE, Math.min(ring.left, base.width - cw - EDGE)).
    let left = EDGE.max(ring.left.min(base.0 - cw - EDGE));
    Placement {
        ring,
        card: (left, EDGE.max(top)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tr;
    use hlsr_core::language::Language;

    fn ids(list: &[TourStep]) -> Vec<&str> {
        list.iter().map(|step| step.id).collect()
    }

    #[test]
    fn steps_follow_the_engine_and_the_game_like_049() {
        assert_eq!(
            ids(&steps_for("goldsrc", "hl1-won")),
            [
                "configs-tab",
                "configs-games",
                "gs-personal",
                "gs-configs",
                "configs-import",
                "configs-export",
                "gs-keyboard",
                "gs-scripts",
                "gs-launch",
                "gs-demos"
            ]
        );
        // У HL2 нет внешнего вида, демо и призраков; у Ghosting есть призраки.
        assert_eq!(
            ids(&steps_for("source", "hl2-4260")),
            [
                "configs-tab",
                "configs-games",
                "gs-configs",
                "configs-import",
                "configs-export",
                "gs-keyboard",
                "gs-launch"
            ]
        );
        assert_eq!(
            ids(&steps_for("source", "ghosting-renewed")),
            [
                "configs-tab",
                "configs-games",
                "gs-personal",
                "gs-configs",
                "configs-import",
                "configs-export",
                "gs-keyboard",
                "gs-launch",
                "gs-demos",
                "gs-ghosts"
            ]
        );
        assert!(!ids(&steps_for("source-old", "hl2-2707")).contains(&"gs-scripts"));
        assert_eq!(
            ids(&steps_for("unknown", "x"))[..2],
            ["configs-tab", "configs-games"]
        );
    }

    #[test]
    fn every_step_has_its_page_section_anchor_and_texts() {
        let want = |id: &str| match id {
            "configs-tab" | "configs-games" => None,
            "gs-personal" => Some("personal"),
            "gs-configs" | "configs-import" | "configs-export" => Some("configs"),
            "gs-keyboard" => Some("keyboard"),
            "gs-scripts" => Some("scripts"),
            "gs-launch" => Some("launch"),
            "gs-demos" => Some("demos"),
            "gs-ghosts" => Some("ghosts"),
            other => panic!("{other}"),
        };
        let mut keys = vec![
            "#L_TOUR_BACK",
            "#L_TOUR_NEXT",
            "#L_TOUR_SKIP",
            "#L_TOUR_DONE",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        for (engine, game) in [
            ("goldsrc", "hl1-won"),
            ("source", "ghosting-renewed"),
            ("source", "hl2-4260"),
        ] {
            for step in steps_for(engine, game) {
                assert_eq!(step.page_section, want(step.id), "{}", step.id);
                assert_eq!(step.anchor, step.id);
                // Первый шаг подсвечивает вкладку в текущем разделе.
                let section = (step.id != "configs-tab").then_some("configs");
                assert_eq!(step.section, section);
                let key = step.id.to_uppercase().replace('-', "_");
                assert_eq!(step.title, format!("#L_TOUR_{key}_TITLE"));
                assert_eq!(step.text, format!("#L_TOUR_{key}_TEXT"));
                keys.extend([step.title, step.text]);
            }
        }
        // Тексты шагов и кнопок есть в ru и en, без тире.
        for lang in [Language::Ru, Language::En] {
            for key in &keys {
                let text = tr(lang, key, &[]);
                assert!(!text.is_empty() && text != *key, "{key} {lang:?}");
                assert!(!text.contains(['—', '–']), "{key} {lang:?}: {text}");
            }
        }
    }

    #[test]
    fn card_goes_below_above_or_inside_like_049() {
        let base = (1600., 900.);
        let card = (340., 150.);
        // Под подсветкой; слева — от края подсветки.
        let a = place(
            Rect {
                left: 300.,
                top: 100.,
                width: 120.,
                height: 40.,
            },
            card,
            base,
        );
        let ring = Rect {
            left: 294.,
            top: 94.,
            width: 132.,
            height: 52.,
        };
        assert_eq!(a.ring, ring);
        assert_eq!(a.card, (294., 158.));
        // Внизу окна: над подсветкой; справа карточка прижата к краю.
        let b = place(
            Rect {
                left: 1500.,
                top: 820.,
                width: 60.,
                height: 40.,
            },
            card,
            base,
        );
        assert_eq!(b.card, (1600. - 340. - 12., 814. - 12. - 150.));
        // Подсветка почти на всё окно: карточка внутри, у нижнего края; не выше EDGE.
        let c = place(
            Rect {
                left: 0.,
                top: 20.,
                width: 1600.,
                height: 860.,
            },
            card,
            base,
        );
        assert_eq!(c.card, (12., 900. - 150. - 12.));
        let d = place(c.ring, (340., 2000.), base);
        assert_eq!(d.card.1, 12.);
    }
}
