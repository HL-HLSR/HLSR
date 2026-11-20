//! Предпросмотр паков HL1 (`shared/hl1-pack-preview.js`): чистая раскладка сцены WON и
//! выбор фрагмента спрайта прицела. Рисует сцену окно (подпроект 5); здесь только координаты.
//!
//! Сцена: здоровье 100, броня 100, MP5 25/67 и две гранаты. Размеры берутся из настоящих
//! `hud.txt` и спрайтов паков. Координаты HUD — по `cl_dll/{health,battery,ammo,flashlight}.cpp`
//! (в WON броня ещё на width/5).

use crate::errors::RawError;
use crate::js_value::prop;
use serde_json::Value;
use std::collections::HashMap;

/// Ширина и высота сцены по умолчанию.
pub const DEFAULT_WIDTH: f64 = 1600.0;
pub const DEFAULT_HEIGHT: f64 = 900.0;

/// Прямоугольники спрайтов `[x, y, ширина, высота]` по имени (`packs.json`, поле `hud`).
pub type Rects = HashMap<String, [f64; 4]>;

/// Команда рисования сцены.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Спрайт `name` из прямоугольника `rect` атласа в точку `at`. Прямоугольника может не быть
    /// (необязательный спрайт): у 0.4.7 `box` тогда `undefined`.
    Sprite {
        name: String,
        rect: Option<[f64; 4]>,
        at: [f64; 2],
    },
    /// Закрашенный прямоугольник `size` в точке `at` (разделитель между числами).
    Fill { at: [f64; 2], size: [f64; 2] },
}

/// `rects[name][index]`: нет прямоугольника — `TypeError` V8 при чтении индекса.
fn dimension(rects: &Rects, name: &str, index: usize) -> Result<f64, RawError> {
    rects.get(name).map(|rect| rect[index]).ok_or_else(|| {
        RawError::new(format!(
            "Cannot read properties of undefined (reading '{index}')"
        ))
    })
}

struct Scene<'a> {
    rects: &'a Rects,
    commands: Vec<Command>,
    /// Ширина цифры.
    digit_width: f64,
}

impl Scene<'_> {
    fn sprite(&mut self, name: &str, x: f64, y: f64) {
        self.commands.push(Command::Sprite {
            name: name.to_owned(),
            rect: self.rects.get(name).copied(),
            at: [x, y],
        });
    }

    /// Число из спрайтов цифр, выровненное вправо по трём знакам; возвращает x после него.
    fn number(&mut self, value: u32, mut x: f64, y: f64) -> f64 {
        for char in format!("{value:>3}").chars() {
            if char != ' ' {
                self.sprite(&format!("number_{char}"), x, y);
            }
            x += self.digit_width;
        }
        x
    }

    fn divider(&mut self, x: f64, y: f64, height: f64) {
        self.commands.push(Command::Fill {
            at: [x, y],
            size: [(self.digit_width / 10.0).floor(), height],
        });
    }
}

/// `hudCommands(rects, width, height)`: команды сцены в порядке рисования. Нет обязательного
/// прямоугольника (`number_0`, `cross`, `suit_empty`, `ammo`, `ammo2`, `flash_full`,
/// `flash_empty`) — ошибка с текстом `TypeError` Node; нет остальных — спрайт без `rect`.
pub fn hud_commands(rects: &Rects, width: f64, height: f64) -> Result<Vec<Command>, RawError> {
    let dw = dimension(rects, "number_0", 2)?;
    let dh = dimension(rects, "number_0", 3)?;
    let baseline = height - dh - (dh / 2.0).floor();
    let number_y = baseline + (dh * 0.2).floor();
    let mut scene = Scene {
        rects,
        commands: Vec::new(),
        digit_width: dw,
    };

    scene.sprite(
        "cross",
        (dimension(rects, "cross", 2)? / 2.0).floor(),
        baseline,
    );
    let x = scene.number(
        100,
        dimension(rects, "cross", 2)? + (dw / 2.0).floor(),
        number_y,
    );
    scene.divider(x + (dw / 2.0).floor(), number_y, dh);
    let suit_x = (width / 5.0).floor();
    let suit_y = baseline - (dimension(rects, "suit_empty", 3)? / 6.0).floor();
    scene.sprite("suit_empty", suit_x, suit_y);
    scene.sprite("suit_full", suit_x, suit_y);
    scene.number(100, suit_x + dimension(rects, "suit_empty", 2)?, number_y);

    let mut x = scene.number(
        25,
        width - 8.0 * dw - dimension(rects, "ammo", 2)?,
        number_y,
    );
    x += (dw / 2.0).floor();
    scene.divider(x, number_y, dh);
    x = scene.number(67, x + (dw / 10.0).floor() + (dw / 2.0).floor(), number_y);
    scene.sprite(
        "ammo",
        x,
        number_y - (dimension(rects, "ammo", 3)? / 8.0).floor(),
    );
    let secondary_y = number_y - dh - (dh / 4.0).floor();
    x = scene.number(
        2,
        width - 4.0 * dw - dimension(rects, "ammo2", 2)?,
        secondary_y,
    );
    scene.sprite(
        "ammo2",
        x,
        secondary_y - (dimension(rects, "ammo2", 3)? / 8.0).floor(),
    );

    let flash_x = width
        - dimension(rects, "flash_full", 2)?
        - (dimension(rects, "flash_full", 2)? / 2.0).floor();
    let flash_y = (dimension(rects, "flash_empty", 3)? / 2.0).floor();
    scene.sprite("flash_empty", flash_x, flash_y);
    scene.sprite("flash_full", flash_x, flash_y);
    Ok(scene.commands)
}

/// Метаданные атласа прицела Crosshair Changer (`packs.json`).
#[derive(Debug, Clone, PartialEq)]
pub struct PackMeta {
    pub shapes: Vec<String>,
    pub colors: Vec<String>,
    /// Размер клетки атласа, отступ внутри клетки и размер самого прицела.
    pub changer_cell: f64,
    pub changer_inset: f64,
    pub changer_size: f64,
}

/// Слой прицела: картинка и прямоугольник в ней.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub image: &'static str,
    pub rect: [f64; 4],
}

/// Позиция значения в списке; нет в списке — первая клетка (`Math.max(0, indexOf(…))`).
fn index_in(list: &[String], value: Option<&Value>) -> f64 {
    value
        .and_then(Value::as_str)
        .and_then(|text| list.iter().position(|item| item == text))
        .unwrap_or(0) as f64
}

/// `crosshairLayer(values, metadata)`: прицел выбранного пака или `None` (стандартный).
pub fn crosshair_layer(values: &Value, meta: &PackMeta) -> Option<Layer> {
    match prop(values, "packCrosshair").and_then(Value::as_str) {
        Some("execut4ble-crosshair") => Some(Layer {
            image: "cross-execut4ble",
            rect: [0.0, 0.0, 128.0, 128.0],
        }),
        Some("crosshair-changer") => {
            let shape = index_in(&meta.shapes, prop(values, "changerShape"));
            let color = index_in(&meta.colors, prop(values, "changerColor"));
            Some(Layer {
                image: "cross-changer",
                rect: [
                    color * meta.changer_cell + meta.changer_inset,
                    shape * meta.changer_cell + meta.changer_inset,
                    meta.changer_size,
                    meta.changer_size,
                ],
            })
        }
        _ => None,
    }
}
