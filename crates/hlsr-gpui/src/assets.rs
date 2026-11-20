//! Картинки и SVG `crates/hlsr-ui/assets/**` (список собирает `build.rs`) и свои SVG вида
//! (`hlsr/…`, их пишет код). GPUI берёт их по имени: `img("launcher/…")`, `svg().path("glyphs/…")`.

use gpui::{AssetSource, Result, SharedString};
use std::borrow::Cow;

const FILES: &[(&str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/assets.rs"));

/// SVG, нарисованные здесь, а не взятые из 0.4.9.
const OWN: &[(&str, &[u8])] = &[
    ("hlsr/spin-track.svg", crate::icons::SPIN_TRACK),
    ("hlsr/spin-arc.svg", crate::icons::SPIN_ARC),
    ("hlsr/chevron-down.svg", crate::icons::CHEVRON_DOWN),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(FILES
            .iter()
            .chain(OWN)
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(FILES
            .iter()
            .chain(OWN)
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| (*name).into())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_hlsr_ui_assets_by_relative_name() {
        for name in [
            "launcher/hl2-ghosting-bg.png",
            "logos/livesplit.svg",
            "glyphs/discord.svg",
            "hlsr/spin-arc.svg",
        ] {
            assert!(Assets.load(name).unwrap().is_some(), "{name}");
        }
        assert!(Assets.load("нет.svg").unwrap().is_none());
    }
}
