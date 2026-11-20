//! Что из репозитория уходит в `resources/` выпуска. Списки повторяют `extraResources` из
//! `package.json` 0.4.7 (общие, `win`, `linux`) с ресурсами, которые ждёт Rust-лаунчер
//! (`hlsr_core::paths::Resources`: `7zip/<ОС>/x64/7za`, оверлей и компоненты LiveSplit, шрифты и
//! заплатки для Linux, лицензии). Раскладка Windows — решение Р6 спецификации:
//! `resources\{install-files.json, 7zip\…, livesplit-overlay\…, livesplit-components\…, licenses\…}`.

use crate::error::{At, Result, fail};
use crate::tree;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Linux,
}

impl Platform {
    pub fn parse(text: &str) -> Option<Platform> {
        match text {
            "windows" | "win32" => Some(Platform::Windows),
            "linux" => Some(Platform::Linux),
            _ => None,
        }
    }
}

/// Откуда (от корня репозитория), куда (от `resources/` выпуска), обязателен ли источник.
struct Rule {
    from: &'static str,
    to: &'static str,
    optional: bool,
}

const fn rule(from: &'static str, to: &'static str) -> Rule {
    Rule {
        from,
        to,
        optional: false,
    }
}

const COMMON: &[Rule] = &[
    rule("LICENSE", "licenses/HLSR-LICENSE.txt"),
    rule("build/license-en.txt", "licenses/LICENSE_en.txt"),
    rule("build/license-ru.txt", "licenses/LICENSE_ru.txt"),
    rule(
        "build/THIRD-PARTY-NOTICES.txt",
        "licenses/THIRD-PARTY-NOTICES.txt",
    ),
    rule(
        "build/SOURCE-SDK-LICENSE.txt",
        "licenses/SOURCE-SDK-LICENSE.txt",
    ),
    rule(
        "build/thirdpartylegalnotices.txt",
        "licenses/thirdpartylegalnotices.txt",
    ),
    // Шрифты, вшитые в лаунчер (`hlsr-ui/fonts`, картинки эмодзи `hlsr-ui/assets/emoji`): OFL требует
    // отдавать текст лицензии со шрифтом. 0.4.7 клала LICENSE.txt Font Awesome в app.asar
    // (`**/node_modules/**/LICENSE*`).
    rule(
        "build/licenses/CascadiaCode-OFL.txt",
        "licenses/CascadiaCode-OFL.txt",
    ),
    rule("build/licenses/Rubik-OFL.txt", "licenses/Rubik-OFL.txt"),
    rule(
        "build/licenses/FontAwesome-LICENSE.txt",
        "licenses/FontAwesome-LICENSE.txt",
    ),
    rule(
        "build/licenses/HLSRSymbols-OFL.txt",
        "licenses/HLSRSymbols-OFL.txt",
    ),
    rule(
        "build/licenses/NotoColorEmoji-OFL.txt",
        "licenses/NotoColorEmoji-OFL.txt",
    ),
    // Оверлей LiveSplit (0.4.9): прокси в обеих системах; помощник и DLL для снимка под Wine — только
    // в Linux (extraResources с ilter).
    rule(
        "resources/livesplit-overlay/proxy-d3d9.dll",
        "livesplit-overlay/proxy-d3d9.dll",
    ),
    rule(
        "resources/livesplit-overlay/proxy-opengl32.dll",
        "livesplit-overlay/proxy-opengl32.dll",
    ),
    rule("resources/livesplit-components", "livesplit-components"),
];

const WINDOWS: &[Rule] = &[
    rule("resources/7zip/LICENSE.txt", "7zip/LICENSE.txt"),
    rule("resources/7zip/win", "7zip/win"),
];

const LINUX: &[Rule] = &[
    rule("resources/7zip/LICENSE.txt", "7zip/LICENSE.txt"),
    rule("resources/7zip/linux", "7zip/linux"),
    rule("build/icons/256x256.png", "icons/hlsr.png"),
    rule(
        "resources/livesplit-overlay/hlsr-overlay64.exe",
        "livesplit-overlay/hlsr-overlay64.exe",
    ),
    rule(
        "resources/livesplit-overlay/hlsr-share64.dll",
        "livesplit-overlay/hlsr-share64.dll",
    ),
    rule("resources/livesplit-fonts", "livesplit-fonts"),
    rule("resources/livesplit-fixes", "livesplit-fixes"),
    rule("resources/presence-relay", "presence-relay"),
    rule(
        "resources/linux/install-userns-profile.sh",
        "linux/install-userns-profile.sh",
    ),
    // umu-launcher (GPL-3.0): текст лицензии — рядом с остальными; в архиве выпуска umu его нет.
    rule(
        "build/licenses/umu-launcher-LICENSE.txt",
        "licenses/umu-launcher-LICENSE.txt",
    ),
    // Скачивается `scripts/prepare-linux-umu.js` на Linux-машине; без него выпуск Linux неполон.
    Rule {
        from: "build/umu/umu-run",
        to: "umu/umu-run",
        optional: true,
    },
];

/// Итог раскладки.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Staged {
    pub copied: Vec<String>,
    /// Необязательные источники, которых нет.
    pub skipped: Vec<String>,
}

/// Разложить ресурсы платформы из репозитория `repo` в пустой каталог `out`.
pub fn run(platform: Platform, repo: &Path, out: &Path) -> Result<Staged> {
    if !tree::is_empty_or_missing(out)? {
        return fail(format!("Каталог ресурсов не пуст: {}", out.display()));
    }
    let own = match platform {
        Platform::Windows => WINDOWS,
        Platform::Linux => LINUX,
    };
    let rules: Vec<&Rule> = COMMON.iter().chain(own).collect();
    let missing: Vec<&str> = rules
        .iter()
        .filter(|rule| !rule.optional && !repo.join(rule.from).exists())
        .map(|rule| rule.from)
        .collect();
    if !missing.is_empty() {
        return fail(format!(
            "В репозитории {} нет файлов выпуска: {}",
            repo.display(),
            missing.join(", ")
        ));
    }
    fs::create_dir_all(out).at(out)?;
    let mut staged = Staged::default();
    for rule in rules {
        let from = repo.join(rule.from);
        if !from.exists() {
            staged.skipped.push(rule.from.to_owned());
            continue;
        }
        let to = rule
            .to
            .split('/')
            .fold(out.to_owned(), |path, part| path.join(part));
        if from.is_dir() {
            tree::copy_tree(&from, &to)?;
        } else {
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent).at(parent)?;
            }
            fs::copy(&from, &to).at(&from)?;
        }
        staged.copied.push(rule.to.to_owned());
    }
    Ok(staged)
}
