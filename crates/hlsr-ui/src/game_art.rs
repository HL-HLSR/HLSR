//! Арт игр для фона (`launcher-data.js` 0.4.7, `gameArt`): кадр, передний план и его раскладка;
//! с 0.4.9 — кадр эпизодов Ghosting (`episodesHero`).
//!
//! Картинки 0.4.7 подключал webpack; здесь они вшиты в программу и зарегистрированы в загрузчике
//! под именем «путь от `renderer/assets`» (адрес `bundle:launcher/hl1-bg.jpg`,
//! [`crate::images::register_bundle`]). Игры 50 и 130 скрыты (`shared/game-availability.js`),
//! их фоны (1,4 МБ) не вшиваются; эпизоды 380 и 420 берут арт HL2, как в 0.4.7.
//!
//! Раскладка переднего плана — переменные `--front-*` в `cqw`/`cqh` (доли ширины и высоты окна):
//! `background-size: auto <height>cqh` и `background-position: calc(<percent> × свободное место
//! + <cqw> + <cqh>) <y>cqh`. Рамка слоя шире окна на `--front-bleed-x` с каждой стороны и на
//! `--front-bleed-y` снизу ([`FRONT_BLEED_X_CQW`], [`FRONT_BLEED_Y_CQH`]).

use crate::images::register_bundle;

/// `--front-bleed-x`: запас слоя по ширине с каждой стороны, cqw.
pub const FRONT_BLEED_X_CQW: f64 = 12.0;
/// `--front-bleed-y`: запас слоя снизу, cqh.
pub const FRONT_BLEED_Y_CQH: f64 = 30.0;

/// Раскладка переднего плана одной игры (`heroFrontLayout`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrontLayout {
    /// `--front-size: auto <h>cqh`: высота картинки в долях высоты окна (cqh), ширина по пропорции.
    pub height_cqh: f64,
    /// `--front-pos` по x: `percent × (ширина рамки − ширина картинки) + cqw·W/100 + cqh·H/100`,
    /// где рамка — окно плюс запасы слева и справа.
    pub x_percent: f64,
    pub x_cqw: f64,
    pub x_cqh: f64,
    /// `--front-pos` по y от верха рамки (она начинается у верха окна), cqh.
    pub y_cqh: f64,
    /// `--front-origin-x/y`: центр поворота (наклон не переносится, решение 10), cqw/cqh.
    pub origin_x_cqw: f64,
    pub origin_y_cqh: f64,
}

impl FrontLayout {
    /// Значения `.launcher-bg__front` без своей раскладки (`LauncherBackground.vue`): подобраны под
    /// Гордона и Аликс из Ghosting.
    pub const DEFAULT: FrontLayout = FrontLayout {
        height_cqh: 195.0,
        x_percent: 0.5,
        x_cqw: 20.0,
        x_cqh: 0.0,
        y_cqh: 3.0,
        origin_x_cqw: 72.0,
        origin_y_cqh: 85.0,
    };

    /// Положение и размер картинки слоя в логических px окна `window` для картинки с
    /// пропорциями `aspect` (ширина / высота): `(x, y, ширина, высота)` от угла окна.
    pub fn place(&self, window: (f64, f64), aspect: f64) -> (f64, f64, f64, f64) {
        let (width, height) = window;
        let (cqw, cqh) = (width / 100.0, height / 100.0);
        let image_height = self.height_cqh * cqh;
        let image_width = image_height * aspect;
        // Рамка слоя: от −bleed-x до ширины окна + bleed-x.
        let box_left = -FRONT_BLEED_X_CQW * cqw;
        let box_width = width + 2.0 * FRONT_BLEED_X_CQW * cqw;
        let x = box_left
            + self.x_percent * (box_width - image_width)
            + self.x_cqw * cqw
            + self.x_cqh * cqh;
        (x, self.y_cqh * cqh, image_width, image_height)
    }
}

/// Арт игры.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GameArt {
    /// Кадр (`hero`): имя файла сборки.
    pub hero: &'static str,
    /// Передний план (`heroFront`), если есть.
    pub front: Option<&'static str>,
    /// Раскладка переднего плана (`heroFrontLayout` или умолчания).
    pub layout: FrontLayout,
    /// Кадр выбранного эпизода (`episodesHero` 0.4.9), без переднего плана.
    pub episodes_hero: Option<&'static str>,
}

const HL2_LAYOUT: FrontLayout = FrontLayout {
    height_cqh: 100.0,
    x_percent: 0.0,
    // calc(var(--front-bleed-x) + 106cqw - 178cqh)
    x_cqw: FRONT_BLEED_X_CQW + 106.0,
    x_cqh: -178.0,
    y_cqh: 9.0,
    origin_x_cqw: 70.0,
    origin_y_cqh: 70.0,
};

const HL2: GameArt = GameArt {
    hero: "launcher/hero-hl2.jpg",
    front: Some("launcher/hl2-front.webp"),
    layout: HL2_LAYOUT,
    episodes_hero: None,
};

impl GameArt {
    /// Арт игры по идентификатору лаунчера (`"70"`, `"218"`, `"220"`…).
    pub fn of(id: &str) -> Option<GameArt> {
        match id {
            "70" => Some(GameArt {
                hero: "launcher/hl1-bg.jpg",
                front: Some("launcher/hl1-front.png"),
                layout: FrontLayout {
                    height_cqh: 170.0,
                    x_percent: 0.5,
                    x_cqw: 22.0,
                    x_cqh: 0.0,
                    y_cqh: 10.0,
                    origin_x_cqw: 72.0,
                    origin_y_cqh: 70.0,
                },
                episodes_hero: None,
            }),
            "218" => Some(GameArt {
                hero: "launcher/hl2-ghosting-bg.png",
                front: Some("launcher/hl2-ghosting-front.webp"),
                layout: FrontLayout::DEFAULT,
                episodes_hero: Some("launcher/hl2-episodes-bg.jpg"),
            }),
            "220" | "380" | "420" => Some(HL2),
            _ => None,
        }
    }
}

/// Файлы арта, вшитые в программу.
const FILES: [(&str, &[u8]); 7] = [
    (
        "launcher/hl1-bg.jpg",
        include_bytes!("../assets/launcher/hl1-bg.jpg"),
    ),
    (
        "launcher/hl1-front.png",
        include_bytes!("../assets/launcher/hl1-front.png"),
    ),
    (
        "launcher/hero-hl2.jpg",
        include_bytes!("../assets/launcher/hero-hl2.jpg"),
    ),
    (
        "launcher/hl2-front.webp",
        include_bytes!("../assets/launcher/hl2-front.webp"),
    ),
    (
        "launcher/hl2-ghosting-bg.png",
        include_bytes!("../assets/launcher/hl2-ghosting-bg.png"),
    ),
    (
        "launcher/hl2-ghosting-front.webp",
        include_bytes!("../assets/launcher/hl2-ghosting-front.webp"),
    ),
    (
        "launcher/hl2-episodes-bg.jpg",
        include_bytes!("../assets/launcher/hl2-episodes-bg.jpg"),
    ),
];

/// Байты вшитого файла арта.
pub fn bytes(name: &str) -> Option<&'static [u8]> {
    FILES
        .iter()
        .find(|(file, _)| *file == name)
        .map(|(_, bytes)| *bytes)
}

/// Зарегистрировать арт в загрузчике картинок (`bundle:<имя>`); повторный вызов безвреден.
pub fn register_bundles() {
    for (name, bytes) in FILES {
        register_bundle(name, bytes);
    }
}
