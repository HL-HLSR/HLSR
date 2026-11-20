//! Импорт конфига HL1 и настройки лаунчера (`shared/goldsrc-import-adopt.js`).
//!
//! hlsr-personalization.cfg выполняется после импорта (и ещё раз в его хвосте, см.
//! `game_binds_install`) и пишет прицел BXT, цвет HUD, панели BXT и поле зрения явно, даже по
//! умолчанию. Без переноса значения игрока из его cfg молча заменялись бы значениями
//! лаунчера. Поэтому при импорте они становятся настройками лаунчера: окно показывает их,
//! лаунчер их и пишет.
//!
//! Берутся только команды верхнего уровня (их выполнит игра при запуске), последнее значение
//! главнее. Значения внутри alias и биндов — это то, что игрок переключает клавишей, их
//! лаунчер не трогает.

use crate::game_binds;
use hlsr_core::js::object::JsObject;
use hlsr_core::js::{self, math_round};
use hlsr_core::json_text::js_number;
use serde_json::{Map, Value, json};

const PANELS: [&str; 4] = ["timer", "speedometer", "jumpspeed", "viewangles"];

/// Последние значения переменных верхнего уровня: имя (нижний регистр) → значение. Порядок
/// ключей — как у объекта JS.
pub fn cvars_of(text: &str) -> JsObject<String> {
    let mut result = JsObject::new();
    for args in game_binds::parse_cfg(text) {
        if args.len() < 2 {
            continue;
        }
        let verb = args[0].to_lowercase();
        if matches!(verb.as_str(), "bind" | "alias" | "exec" | "unbind") {
            continue;
        }
        result.insert(verb, args[1].clone());
    }
    result
}

fn number_of(value: &str) -> Option<f64> {
    let text = js::trim(value);
    if text.is_empty() {
        return None;
    }
    let n = js::to_number(text);
    n.is_finite().then_some(n)
}

/// Цвет «R G B»: три числа от 0 до 255, округляются.
fn color_of(value: &str) -> Option<Value> {
    let text = js::trim(value);
    let parts: Vec<&str> = if text.is_empty() {
        vec![""]
    } else {
        text.split(js::is_whitespace)
            .filter(|part| !part.is_empty())
            .collect()
    };
    if parts.len() != 3 {
        return None;
    }
    let numbers: Vec<f64> = parts.iter().map(|part| js::to_number(part)).collect();
    if !numbers
        .iter()
        .all(|&n| n.is_finite() && (0.0..=255.0).contains(&n))
    {
        return None;
    }
    Some(json!({
        "r": js_number(math_round(numbers[0])),
        "g": js_number(math_round(numbers[1])),
        "b": js_number(math_round(numbers[2])),
    }))
}

/// Итог переноса.
#[derive(Debug, Clone, PartialEq)]
pub struct Adopted {
    /// Копия настроек персонализации с перенесёнными значениями.
    pub personalization: Map<String, Value>,
    /// Копия настроек BXT с перенесёнными значениями.
    pub bxt: Map<String, Value>,
    /// Перенесённые переменные.
    pub adopted: Vec<String>,
    /// Те, что лаунчер записать так не может (значение вне его пределов или сочетание, которого
    /// нет в настройках); их значение из импорта лаунчер перекроет.
    pub skipped: Vec<String>,
}

impl Adopted {
    /// Как объект JS: `{ personalization, bxt, adopted, skipped }`.
    pub fn to_value(&self) -> Value {
        json!({
            "personalization": self.personalization,
            "bxt": self.bxt,
            "adopted": self.adopted,
            "skipped": self.skipped,
        })
    }
}

struct Taker<'a> {
    cvars: &'a JsObject<String>,
    adopted: Vec<String>,
    skipped: Vec<String>,
}

impl Taker<'_> {
    /// `take(name, apply)`: переменная есть в cfg — `apply` либо переносит её, либо отказывает.
    fn take(&mut self, name: &str, apply: impl FnOnce(&str) -> bool) {
        let Some(value) = self.cvars.get(name) else {
            return;
        };
        if apply(value) {
            self.adopted.push(name.to_owned());
        } else {
            self.skipped.push(format!("{name} {value}"));
        }
    }
}

/// Булево значение: число, не равное нулю.
fn flag(set: impl FnOnce(bool)) -> impl FnOnce(&str) -> bool {
    move |value| match number_of(value) {
        Some(n) => {
            set(n != 0.0);
            true
        }
        None => false,
    }
}

/// Число в пределах лаунчера; иначе отказ.
fn ranged(min: f64, max: f64, set: impl FnOnce(f64)) -> impl FnOnce(&str) -> bool {
    move |value| match number_of(value) {
        Some(n) if n >= min && n <= max => {
            set(math_round(n));
            true
        }
        _ => false,
    }
}

/// `flag` с записью в поле `field` настроек.
fn flag_into<'a>(
    b: &'a mut Map<String, Value>,
    field: &'static str,
) -> impl FnOnce(&str) -> bool + 'a {
    flag(move |v| {
        b.insert(field.into(), json!(v));
    })
}

/// `ranged` с записью в поле `field` настроек.
fn ranged_into<'a>(
    b: &'a mut Map<String, Value>,
    min: f64,
    max: f64,
    field: &'static str,
) -> impl FnOnce(&str) -> bool + 'a {
    ranged(min, max, move |n| {
        b.insert(field.into(), js_number(n));
    })
}

/// Переносит значения из `text` в копии `personalization` и `bxt`.
pub fn adopt_imported(
    text: &str,
    personalization: &Map<String, Value>,
    bxt: &Map<String, Value>,
) -> Adopted {
    let cvars = cvars_of(text);
    let mut p = personalization.clone();
    let mut b = bxt.clone();
    let mut taker = Taker {
        cvars: &cvars,
        adopted: Vec::new(),
        skipped: Vec::new(),
    };

    for key in PANELS {
        taker.take(
            &format!("bxt_hud_{key}"),
            flag(|v| {
                b.insert(key.to_owned(), json!(v));
            }),
        );
    }
    taker.take(
        "default_fov",
        ranged(70.0, 120.0, |n| {
            b.insert("fov".into(), js_number(n));
        }),
    );
    taker.take(
        "bxt_fix_widescreen_fov",
        flag(|v| {
            b.insert("fixWidescreenFov".into(), json!(v));
        }),
    );
    taker.take(
        "bxt_viewmodel_fov",
        ranged(0.0, 130.0, |n| {
            b.insert("viewmodelFov".into(), js_number(n));
        }),
    );
    taker.take(
        "bxt_viewmodel_semitransparent",
        flag(|v| {
            b.insert("viewmodelSemitransparent".into(), json!(v));
        }),
    );
    // 0.4.9: модель оружия и панели BXT (порядок — как у take в 0.4.9).
    taker.take("bxt_remove_viewmodel", flag_into(&mut b, "removeViewmodel"));
    // Смещение модели оружия: -20..20, как в окне настроек.
    taker.take(
        "bxt_viewmodel_ofs_forward",
        ranged_into(&mut b, -20.0, 20.0, "viewmodelForward"),
    );
    taker.take(
        "bxt_viewmodel_ofs_right",
        ranged_into(&mut b, -20.0, 20.0, "viewmodelRight"),
    );
    taker.take(
        "bxt_viewmodel_ofs_up",
        ranged_into(&mut b, -20.0, 20.0, "viewmodelUp"),
    );
    taker.take(
        "bxt_viewmodel_bob_angled",
        flag_into(&mut b, "viewmodelBobAngled"),
    );
    taker.take(
        "bxt_viewmodel_disable_idle",
        flag_into(&mut b, "viewmodelNoIdle"),
    );
    taker.take(
        "bxt_viewmodel_disable_equip",
        flag_into(&mut b, "viewmodelNoEquip"),
    );
    taker.take("bxt_hud_jumpdistance", flag_into(&mut b, "jumpdistance"));
    taker.take(
        "bxt_hud_precision",
        ranged_into(&mut b, 0.0, 16.0, "precision"),
    );
    taker.take(
        "bxt_hud_incorrect_fps_indicator",
        flag_into(&mut b, "incorrectFps"),
    );
    // Положение панели: "x y" — доли экрана, в окне проценты 0..100.
    for key in PANELS {
        taker.take(&format!("bxt_hud_{key}_anchor"), |value| {
            let text = js::trim(value);
            let parts: Vec<Option<f64>> = if text.is_empty() {
                vec![None]
            } else {
                text.split(js::is_whitespace)
                    .filter(|part| !part.is_empty())
                    .map(number_of)
                    .collect()
            };
            let (Some(Some(x)), Some(Some(y))) = (parts.first(), parts.get(1)) else {
                return false;
            };
            if !(0.0..=1.0).contains(x) || !(0.0..=1.0).contains(y) {
                return false;
            }
            b.insert(format!("{key}X"), js_number(math_round(x * 100.0)));
            b.insert(format!("{key}Y"), js_number(math_round(y * 100.0)));
            true
        });
    }

    // Цвет HUD: лаунчер пишет один цвет в bxt_hud_color и bxt_hud_game_color.
    let hud_name = if cvars.contains_key("bxt_hud_color") {
        "bxt_hud_color"
    } else {
        "bxt_hud_game_color"
    };
    taker.take(hud_name, |value| {
        if js::trim(value).is_empty() {
            p.insert("hudEnabled".into(), json!(false));
            return true;
        }
        let Some(color) = color_of(value) else {
            return false;
        };
        p.insert("hudEnabled".into(), json!(true));
        p.insert("hudColor".into(), color);
        p.insert("hudAlpha".into(), json!(-1));
        p.insert("hudAffectsCrosshair".into(), json!(false));
        true
    });
    if hud_name == "bxt_hud_color"
        && let (Some(game), Some(own)) =
            (cvars.get("bxt_hud_game_color"), cvars.get("bxt_hud_color"))
        && game != own
    {
        taker.skipped.push(format!("bxt_hud_game_color {game}"));
    }
    // Прозрачность без своего цвета лаунчер не пишет (он всегда пишет 0).
    if let Some(alpha) = cvars.get("bxt_hud_game_alpha")
        && number_of(alpha) != Some(0.0)
    {
        taker.skipped.push(format!("bxt_hud_game_alpha {alpha}"));
    }

    // Прицел BXT. Любая его настройка из cfg делает прицел «своим».
    let cross_before = taker.adopted.len() + taker.skipped.len();
    taker.take(
        "bxt_cross",
        flag(|v| {
            p.insert("crossEnabled".into(), json!(v));
        }),
    );
    taker.take("bxt_cross_color", |value| {
        let Some(color) = color_of(value) else {
            return false;
        };
        p.insert("crossColor".into(), color);
        p.insert("hudAffectsCrosshair".into(), json!(false));
        true
    });
    taker.take(
        "bxt_cross_alpha",
        ranged(0.0, 255.0, |n| {
            p.insert("crossAlpha".into(), js_number(n));
        }),
    );
    taker.take(
        "bxt_cross_thickness",
        ranged(1.0, 10.0, |n| {
            p.insert("crossThickness".into(), js_number(n));
        }),
    );
    taker.take(
        "bxt_cross_size",
        ranged(0.0, 40.0, |n| {
            p.insert("crossSize".into(), js_number(n));
        }),
    );
    taker.take(
        "bxt_cross_gap",
        ranged(-30.0, 30.0, |n| {
            p.insert("crossGap".into(), js_number(n));
        }),
    );
    taker.take(
        "bxt_cross_outline",
        ranged(0.0, 4.0, |n| {
            p.insert("crossOutline".into(), json!(n > 0.0));
            if n > 0.0 {
                p.insert("crossOutlineThickness".into(), js_number(n));
            }
        }),
    );
    taker.take(
        "bxt_cross_circle_radius",
        ranged(0.0, 40.0, |n| {
            p.insert("crossCircleRadius".into(), js_number(n));
        }),
    );
    taker.take("bxt_cross_dot_color", |value| {
        if js::trim(value).is_empty() {
            p.insert("crossDotColor".into(), Value::Null);
            return true;
        }
        let Some(color) = color_of(value) else {
            return false;
        };
        p.insert("crossDotColor".into(), color);
        true
    });
    taker.take(
        "bxt_cross_dot_size",
        ranged(0.0, 12.0, |n| {
            p.insert("crossDotSize".into(), js_number(n));
        }),
    );
    for (name, field) in [
        ("bxt_cross_top_line", "crossTopLine"),
        ("bxt_cross_bottom_line", "crossBottomLine"),
        ("bxt_cross_left_line", "crossLeftLine"),
        ("bxt_cross_right_line", "crossRightLine"),
    ] {
        taker.take(
            name,
            flag(|v| {
                p.insert(field.into(), json!(v));
            }),
        );
    }
    let cross_enabled = |p: &Map<String, Value>| p.get("crossEnabled").is_some_and(js::truthy);
    if taker.adopted.len() + taker.skipped.len() > cross_before && cross_enabled(&p) {
        p.insert("crossPreset".into(), json!("custom"));
    } else if cvars.contains_key("bxt_cross") && !cross_enabled(&p) {
        p.insert("crossPreset".into(), json!("standard"));
    }

    // Штатный прицел: с прицелом BXT его скрывает настройка прицела, без него — отдельный
    // флаг bxt.hideCrosshair.
    taker.take("crosshair", |value| {
        let Some(on) = number_of(value).map(|n| n != 0.0) else {
            return false;
        };
        let own = cross_enabled(&p);
        b.insert("hideCrosshair".into(), json!(!on && !own));
        if own {
            p.insert("crossHideDefault".into(), json!(!on));
        }
        true
    });

    Adopted {
        personalization: p,
        bxt: b,
        adopted: taker.adopted,
        skipped: taker.skipped,
    }
}
