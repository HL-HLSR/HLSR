//! Ассеты превью персонализации — те же файлы, что `src/renderer/assets/launcher/{hl1,ghosting}-preview`
//! 0.4.9 (карта §6): байты в сборке; декодирует их рендер превью при первом обращении (лениво, как
//! `layer()` 0.4.7), а не при запуске.

macro_rules! file {
    ($dir:literal, $name:literal) => {
        include_bytes!(concat!("../../assets/", $dir, "/", $name)).as_slice()
    };
}

/// `hl1-preview/meta.json`.
pub const HL1_META: &str = include_str!("../../assets/hl1-preview/meta.json");
/// `hl1-preview/meta-weapons.json`: шаги сдвига вперёд, поля зрения и места слоёв оружия.
pub const HL1_WEAPONS: &str = include_str!("../../assets/hl1-preview/meta-weapons.json");
/// `ghosting-preview/scene/ghost.json`: где готовый призрак в кадре 1920×1080.
pub const GHOST_SPRITE: &str = include_str!("../../assets/ghosting-preview/scene/ghost.json");
/// `hl1-preview/packs.json`.
pub const HL1_PACKS: &str = include_str!("../../assets/hl1-preview/packs.json");
/// `ghosting-preview/meta.json`.
pub const GHOSTING_META: &str = include_str!("../../assets/ghosting-preview/meta.json");

/// Слой превью HL1 по имени `FILES` 0.4.7 (`scene-90`, `weapon-50`, `hud`, `hud-ag`, …).
pub fn hl1(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "cross-changer" => file!("hl1-preview", "cross-changer-atlas.png"),
        "cross-execut4ble" => file!("hl1-preview", "cross-execut4ble.png"),
        "crosshair" => file!("hl1-preview", "crosshair.png"),
        "hud-ag" => file!("hl1-preview", "hud-ag-atlas.png"),
        "hud-stock" => file!("hl1-preview", "hud-stock.png"),
        "hud" => file!("hl1-preview", "hud.png"),
        "panels" => file!("hl1-preview", "panels.png"),
        "scene-100" => file!("hl1-preview", "scene-100.webp"),
        "scene-110" => file!("hl1-preview", "scene-110.webp"),
        "scene-120" => file!("hl1-preview", "scene-120.webp"),
        "scene-130" => file!("hl1-preview", "scene-130.webp"),
        "scene-140" => file!("hl1-preview", "scene-140.webp"),
        "scene-70" => file!("hl1-preview", "scene-70.webp"),
        "scene-80" => file!("hl1-preview", "scene-80.webp"),
        "scene-90" => file!("hl1-preview", "scene-90.webp"),
        "viewangles" => file!("hl1-preview", "viewangles.png"),
        _ => return None,
    })
}

/// Слой оружия HL1 0.4.9 (`weapons/<weapon|semi>-<fov>-f<сдвиг>.webp`, места — `HL1_WEAPONS`).
pub fn hl1_weapon(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "semi-100-f-10" => file!("hl1-preview/weapons", "semi-100-f-10.webp"),
        "semi-100-f0" => file!("hl1-preview/weapons", "semi-100-f0.webp"),
        "semi-100-f10" => file!("hl1-preview/weapons", "semi-100-f10.webp"),
        "semi-100-f20" => file!("hl1-preview/weapons", "semi-100-f20.webp"),
        "semi-110-f-10" => file!("hl1-preview/weapons", "semi-110-f-10.webp"),
        "semi-110-f-20" => file!("hl1-preview/weapons", "semi-110-f-20.webp"),
        "semi-110-f0" => file!("hl1-preview/weapons", "semi-110-f0.webp"),
        "semi-110-f10" => file!("hl1-preview/weapons", "semi-110-f10.webp"),
        "semi-110-f20" => file!("hl1-preview/weapons", "semi-110-f20.webp"),
        "semi-120-f-10" => file!("hl1-preview/weapons", "semi-120-f-10.webp"),
        "semi-120-f-20" => file!("hl1-preview/weapons", "semi-120-f-20.webp"),
        "semi-120-f0" => file!("hl1-preview/weapons", "semi-120-f0.webp"),
        "semi-120-f10" => file!("hl1-preview/weapons", "semi-120-f10.webp"),
        "semi-120-f20" => file!("hl1-preview/weapons", "semi-120-f20.webp"),
        "semi-130-f-10" => file!("hl1-preview/weapons", "semi-130-f-10.webp"),
        "semi-130-f-20" => file!("hl1-preview/weapons", "semi-130-f-20.webp"),
        "semi-130-f0" => file!("hl1-preview/weapons", "semi-130-f0.webp"),
        "semi-130-f10" => file!("hl1-preview/weapons", "semi-130-f10.webp"),
        "semi-130-f20" => file!("hl1-preview/weapons", "semi-130-f20.webp"),
        "semi-140-f-10" => file!("hl1-preview/weapons", "semi-140-f-10.webp"),
        "semi-140-f-20" => file!("hl1-preview/weapons", "semi-140-f-20.webp"),
        "semi-140-f0" => file!("hl1-preview/weapons", "semi-140-f0.webp"),
        "semi-140-f10" => file!("hl1-preview/weapons", "semi-140-f10.webp"),
        "semi-140-f20" => file!("hl1-preview/weapons", "semi-140-f20.webp"),
        "semi-50-f-10" => file!("hl1-preview/weapons", "semi-50-f-10.webp"),
        "semi-50-f0" => file!("hl1-preview/weapons", "semi-50-f0.webp"),
        "semi-50-f10" => file!("hl1-preview/weapons", "semi-50-f10.webp"),
        "semi-50-f20" => file!("hl1-preview/weapons", "semi-50-f20.webp"),
        "semi-70-f-10" => file!("hl1-preview/weapons", "semi-70-f-10.webp"),
        "semi-70-f0" => file!("hl1-preview/weapons", "semi-70-f0.webp"),
        "semi-70-f10" => file!("hl1-preview/weapons", "semi-70-f10.webp"),
        "semi-70-f20" => file!("hl1-preview/weapons", "semi-70-f20.webp"),
        "semi-80-f-10" => file!("hl1-preview/weapons", "semi-80-f-10.webp"),
        "semi-80-f0" => file!("hl1-preview/weapons", "semi-80-f0.webp"),
        "semi-80-f10" => file!("hl1-preview/weapons", "semi-80-f10.webp"),
        "semi-80-f20" => file!("hl1-preview/weapons", "semi-80-f20.webp"),
        "semi-90-f-10" => file!("hl1-preview/weapons", "semi-90-f-10.webp"),
        "semi-90-f0" => file!("hl1-preview/weapons", "semi-90-f0.webp"),
        "semi-90-f10" => file!("hl1-preview/weapons", "semi-90-f10.webp"),
        "semi-90-f20" => file!("hl1-preview/weapons", "semi-90-f20.webp"),
        "weapon-100-f-10" => file!("hl1-preview/weapons", "weapon-100-f-10.webp"),
        "weapon-100-f0" => file!("hl1-preview/weapons", "weapon-100-f0.webp"),
        "weapon-100-f10" => file!("hl1-preview/weapons", "weapon-100-f10.webp"),
        "weapon-100-f20" => file!("hl1-preview/weapons", "weapon-100-f20.webp"),
        "weapon-110-f-10" => file!("hl1-preview/weapons", "weapon-110-f-10.webp"),
        "weapon-110-f-20" => file!("hl1-preview/weapons", "weapon-110-f-20.webp"),
        "weapon-110-f0" => file!("hl1-preview/weapons", "weapon-110-f0.webp"),
        "weapon-110-f10" => file!("hl1-preview/weapons", "weapon-110-f10.webp"),
        "weapon-110-f20" => file!("hl1-preview/weapons", "weapon-110-f20.webp"),
        "weapon-120-f-10" => file!("hl1-preview/weapons", "weapon-120-f-10.webp"),
        "weapon-120-f-20" => file!("hl1-preview/weapons", "weapon-120-f-20.webp"),
        "weapon-120-f0" => file!("hl1-preview/weapons", "weapon-120-f0.webp"),
        "weapon-120-f10" => file!("hl1-preview/weapons", "weapon-120-f10.webp"),
        "weapon-120-f20" => file!("hl1-preview/weapons", "weapon-120-f20.webp"),
        "weapon-130-f-10" => file!("hl1-preview/weapons", "weapon-130-f-10.webp"),
        "weapon-130-f-20" => file!("hl1-preview/weapons", "weapon-130-f-20.webp"),
        "weapon-130-f0" => file!("hl1-preview/weapons", "weapon-130-f0.webp"),
        "weapon-130-f10" => file!("hl1-preview/weapons", "weapon-130-f10.webp"),
        "weapon-130-f20" => file!("hl1-preview/weapons", "weapon-130-f20.webp"),
        "weapon-140-f-10" => file!("hl1-preview/weapons", "weapon-140-f-10.webp"),
        "weapon-140-f-20" => file!("hl1-preview/weapons", "weapon-140-f-20.webp"),
        "weapon-140-f0" => file!("hl1-preview/weapons", "weapon-140-f0.webp"),
        "weapon-140-f10" => file!("hl1-preview/weapons", "weapon-140-f10.webp"),
        "weapon-140-f20" => file!("hl1-preview/weapons", "weapon-140-f20.webp"),
        "weapon-50-f-10" => file!("hl1-preview/weapons", "weapon-50-f-10.webp"),
        "weapon-50-f0" => file!("hl1-preview/weapons", "weapon-50-f0.webp"),
        "weapon-50-f10" => file!("hl1-preview/weapons", "weapon-50-f10.webp"),
        "weapon-50-f20" => file!("hl1-preview/weapons", "weapon-50-f20.webp"),
        "weapon-70-f-10" => file!("hl1-preview/weapons", "weapon-70-f-10.webp"),
        "weapon-70-f0" => file!("hl1-preview/weapons", "weapon-70-f0.webp"),
        "weapon-70-f10" => file!("hl1-preview/weapons", "weapon-70-f10.webp"),
        "weapon-70-f20" => file!("hl1-preview/weapons", "weapon-70-f20.webp"),
        "weapon-80-f-10" => file!("hl1-preview/weapons", "weapon-80-f-10.webp"),
        "weapon-80-f0" => file!("hl1-preview/weapons", "weapon-80-f0.webp"),
        "weapon-80-f10" => file!("hl1-preview/weapons", "weapon-80-f10.webp"),
        "weapon-80-f20" => file!("hl1-preview/weapons", "weapon-80-f20.webp"),
        "weapon-90-f-10" => file!("hl1-preview/weapons", "weapon-90-f-10.webp"),
        "weapon-90-f0" => file!("hl1-preview/weapons", "weapon-90-f0.webp"),
        "weapon-90-f10" => file!("hl1-preview/weapons", "weapon-90-f10.webp"),
        "weapon-90-f20" => file!("hl1-preview/weapons", "weapon-90-f20.webp"),
        _ => return None,
    })
}

/// Сцены разделов «Призраки и запись» и «Эффекты и спреи» 0.4.9 (`ghosting-preview/scene`).
pub fn ghosting_scene(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "beach" => file!("ghosting-preview/scene", "beach.webp"),
        "wall" => file!("ghosting-preview/scene", "wall.webp"),
        "ghost" => file!("ghosting-preview/scene", "ghost.webp"),
        "spray" => file!("ghosting-preview/scene", "spray-default.webp"),
        _ => return None,
    })
}

/// Слой превью Ghosting: `scene-<fov>`, `view-<i>`, `depth-<i>`, `hud`.
pub fn ghosting(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "hud" => file!("ghosting-preview", "hud.png"),
        "scene-100" => file!("ghosting-preview", "scene-100.webp"),
        "scene-110" => file!("ghosting-preview", "scene-110.webp"),
        "scene-120" => file!("ghosting-preview", "scene-120.webp"),
        "scene-130" => file!("ghosting-preview", "scene-130.webp"),
        "scene-140" => file!("ghosting-preview", "scene-140.webp"),
        "scene-150" => file!("ghosting-preview", "scene-150.webp"),
        "scene-75" => file!("ghosting-preview", "scene-75.webp"),
        "scene-80" => file!("ghosting-preview", "scene-80.webp"),
        "scene-90" => file!("ghosting-preview", "scene-90.webp"),
        "depth-0" => file!("ghosting-preview", "views/0-depth.png"),
        "view-0" => file!("ghosting-preview", "views/0.webp"),
        "depth-1" => file!("ghosting-preview", "views/1-depth.png"),
        "view-1" => file!("ghosting-preview", "views/1.webp"),
        "depth-2" => file!("ghosting-preview", "views/2-depth.png"),
        "view-2" => file!("ghosting-preview", "views/2.webp"),
        "depth-3" => file!("ghosting-preview", "views/3-depth.png"),
        "view-3" => file!("ghosting-preview", "views/3.webp"),
        "depth-4" => file!("ghosting-preview", "views/4-depth.png"),
        "view-4" => file!("ghosting-preview", "views/4.webp"),
        "depth-5" => file!("ghosting-preview", "views/5-depth.png"),
        "view-5" => file!("ghosting-preview", "views/5.webp"),
        "depth-6" => file!("ghosting-preview", "views/6-depth.png"),
        "view-6" => file!("ghosting-preview", "views/6.webp"),
        "depth-7" => file!("ghosting-preview", "views/7-depth.png"),
        "view-7" => file!("ghosting-preview", "views/7.webp"),
        "depth-8" => file!("ghosting-preview", "views/8-depth.png"),
        "view-8" => file!("ghosting-preview", "views/8.webp"),
        "depth-9" => file!("ghosting-preview", "views/9-depth.png"),
        "view-9" => file!("ghosting-preview", "views/9.webp"),
        "depth-10" => file!("ghosting-preview", "views/10-depth.png"),
        "view-10" => file!("ghosting-preview", "views/10.webp"),
        "depth-11" => file!("ghosting-preview", "views/11-depth.png"),
        "view-11" => file!("ghosting-preview", "views/11.webp"),
        "depth-12" => file!("ghosting-preview", "views/12-depth.png"),
        "view-12" => file!("ghosting-preview", "views/12.webp"),
        "depth-13" => file!("ghosting-preview", "views/13-depth.png"),
        "view-13" => file!("ghosting-preview", "views/13.webp"),
        "depth-14" => file!("ghosting-preview", "views/14-depth.png"),
        "view-14" => file!("ghosting-preview", "views/14.webp"),
        "depth-15" => file!("ghosting-preview", "views/15-depth.png"),
        "view-15" => file!("ghosting-preview", "views/15.webp"),
        "depth-16" => file!("ghosting-preview", "views/16-depth.png"),
        "view-16" => file!("ghosting-preview", "views/16.webp"),
        "depth-17" => file!("ghosting-preview", "views/17-depth.png"),
        "view-17" => file!("ghosting-preview", "views/17.webp"),
        "depth-18" => file!("ghosting-preview", "views/18-depth.png"),
        "view-18" => file!("ghosting-preview", "views/18.webp"),
        "depth-19" => file!("ghosting-preview", "views/19-depth.png"),
        "view-19" => file!("ghosting-preview", "views/19.webp"),
        "depth-20" => file!("ghosting-preview", "views/20-depth.png"),
        "view-20" => file!("ghosting-preview", "views/20.webp"),
        _ => return None,
    })
}
