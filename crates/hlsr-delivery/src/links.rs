//! Внешние ссылки лаунчера (`shared/links.js`): окно открывает их по ключу, адрес знает только
//! main.

/// `LINKS`: ключ → адрес, в порядке объекта 0.4.9.
pub const LINKS: [(&str, &str); 18] = [
    ("site", "https://hlsr.pro"),
    ("privacy", "https://hlsr.pro/privacy/"),
    ("terms", "https://hlsr.pro/terms/"),
    // Отступление от 0.4.9 (решение владельца, 0.5.0): исходники и выпуски — HL-HLSR/HLSR.
    // Обновления по-прежнему качаются из HLSR-Releases (delivery_endpoints, hlsr-update::channel).
    ("github", "https://github.com/HL-HLSR/HLSR"),
    ("releases", "https://github.com/HL-HLSR/HLSR/releases"),
    ("discord", "https://discord.com/invite/8Np6brWBuz"),
    (
        "steamHalfLife",
        "https://store.steampowered.com/app/70/HalfLife/",
    ),
    (
        "steamHalfLife2",
        "https://store.steampowered.com/app/220/HalfLife_2/",
    ),
    // Откуда лаунчер берёт паки HUD и прицелов HL1 (goldsrc_packs.rs).
    ("hl1Resources", "https://www.speedrun.com/hl1/resources"),
    // Исходник bxt-rs (GPL-3), которую «Демо» скачивает для рендера HL1 (config/tools/bxt-rs.manifest.json).
    (
        "bxtrsSource",
        "https://hlsr.pro/downloads/tools/bxt-rs/won-2623912/bxt-rs-won-2623912-src.tar.gz",
    ),
    // Её лицензия и уведомления крейтов (те же файлы ставятся рядом с DLL).
    (
        "bxtrsLicense",
        "https://hlsr.pro/downloads/tools/bxt-rs/won-2623912/THIRD-PARTY-NOTICES.txt",
    ),
    ("speedrun-hl1", "https://www.speedrun.com/hl1"),
    ("speedrun-hl2", "https://www.speedrun.com/hl2"),
    ("google-drive", "https://drive.google.com/drive/my-drive"),
    ("authorInfiniteCore", "https://github.com/InfiniteC0re"),
    ("authorProlert", "https://github.com/prolert"),
    ("authorKanoyo", "https://github.com/kanoyo-git"),
    ("authorScriptedSnark", "https://github.com/ScriptedSnark"),
];

/// `urlFor(key)`: адрес по ключу; не строка или неизвестный ключ — `None`.
pub fn url_for(key: &serde_json::Value) -> Option<&'static str> {
    let key = key.as_str()?;
    LINKS
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, url)| *url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_own_keys_resolve() {
        assert_eq!(
            url_for(&json!("discord")),
            Some("https://discord.com/invite/8Np6brWBuz")
        );
        assert_eq!(url_for(&json!("constructor")), None);
        assert_eq!(url_for(&json!(5)), None);
        assert_eq!(url_for(&json!("Discord")), None);
    }
}
