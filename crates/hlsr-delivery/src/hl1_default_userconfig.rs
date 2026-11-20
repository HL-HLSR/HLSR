//! Стандартный userconfig.cfg Half-Life WON, который ставит лаунчер вместо стокового из пакета
//! Half-Life 2005 WON (bxt.rs) (`hl1-default-userconfig.js`). Строки стокового сохранены,
//! добавлены пояснения и обязательная по правилам speedrun.com переменная BXT.
//!
//! Источники: стоковый userconfig.cfg пакета; правила категории WON и гайд «FAQ & Setup» на
//! speedrun.com/hl1; белый список команд категорий (fps_max не выше 100,
//! `_bxt_save_runtime_data_in_demos 1`, счётчик FPS, сложность — до старта рана).
//!
//! Комментарии только на английском и только ASCII, как в стоковом файле: русский текст в cfg игре
//! не нужен. Точки с запятой в комментариях нет нарочно: GoldSrc делит по ним команды и внутри
//! комментария.

use crate::bind_presets::HL1_RUN_ALIAS;
use std::sync::LazyLock;

/// Какой userconfig.cfg лаунчер заменяет: SHA-256 байт файла без строк exec лаунчера и без пустых
/// строк в конце. Стоковый из пакета и стандартные прошлых версий лаунчера. Изменённый игроком
/// файл не совпадёт и останется.
pub const REPLACEABLE_SHA256: [&str; 3] = [
    "04aec02c6aa9461ba16d2f71087fcc79e2baccf2a31c9518dbd2e29a8a894994", // стоковый
    "928b569e280853c4309433527798e838324f0227730a9e5d86366ff811625fb0", // лаунчер, без cl_showfps
    "a83cd5ee65241940916093e93a8db34ba96844d07f6a267203c1c6e403a209d3", // лаунчер 0.4.0, русские комментарии
];

static STOCK: LazyLock<String> = LazyLock::new(|| {
    [
        "exec WON.cfg // Don't remove this".to_owned(),
        String::new(),
        "// Use this to write all your personal binds and aliases".to_owned(),
        "// The alias below helps you start/reset runs".to_owned(),
        String::new(),
        format!("alias run \"{HL1_RUN_ALIAS}\""),
        "bind p \"run\"".to_owned(),
        "bxt_hud_incorrect_fps_indicator 0".to_owned(),
    ]
    .join("\n")
});

static TEXT: LazyLock<String> = LazyLock::new(|| {
    let lines = [
        "exec WON.cfg // Don't remove this: fps_max 100 and clockwindow 0 as in the original engine, runs without them are rejected".to_owned(),
        String::new(),
        "// Required by the speedrun.com rules".to_owned(),
        "_bxt_save_runtime_data_in_demos 1 // BXT data in demos, moderators verify runs with it".to_owned(),
        "cl_showfps 1 // FPS counter in the corner of the screen, must stay on".to_owned(),
        String::new(),
        "// Run start: difficulty, first map, BXT timer and demo recording run_1, run_2 and so on".to_owned(),
        "// Difficulty can't be changed during a run, so it is set here, before the start".to_owned(),
        format!("alias run \"{HL1_RUN_ALIAS}\""),
        "bind p \"run\" // Restart the run".to_owned(),
        String::new(),
        "// After the finish, type stop in the console to stop demo recording".to_owned(),
        "// Demos are submitted with the run, keep the demos of your record".to_owned(),
        String::new(),
        "bxt_hud_incorrect_fps_indicator 0".to_owned(),
        String::new(),
        "// Write your own binds and aliases below. Game settings are easier to change in the launcher:".to_owned(),
        "// Game settings, Configs tab".to_owned(),
        String::new(),
    ];
    lines.join("\r\n") + "\r\n"
});

/// `TEXT`: стандартный файл лаунчера (CRLF, после последней строки пустая).
pub fn text() -> &'static str {
    &TEXT
}

/// `STOCK`: стоковый файл пакета без строк exec лаунчера (LF).
pub fn stock() -> &'static str {
    &STOCK
}
