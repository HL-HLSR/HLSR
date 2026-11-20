//! Моды игр (`custom-packs.js` 0.4.9): модели, HUD, звуки с переключателем в персонализации.
//! Файлы мода приходят патчем игры в `<мод>/hlsr-packs/<id>` и лежат там, движок их не видит.
//! Включение копирует их туда, где движок ищет файлы раньше оригиналов; выключение удаляет копии.
//!
//! HL1 (GoldSrc): копии ложатся в папку мода `valve_WON`, её движок читает раньше `valve`.
//! Если там на этом месте уже был чужой файл, он уходит в `.hlsr/custom-packs/backup` и потом
//! возвращается. Ghosting (Source): мод целиком ложится папкой в `ghostingmod/custom/<id>`.
//!
//! Перед любой правкой файлов в `state.json` пишется журнал (`pending`). Сбой посередине
//! доделывает следующий вызов: включение откатывается, выключение доводится до конца. Оба
//! случая — одно и то же «вернуть как было» ([`undo`]).

use crate::atomic::write_json_atomic;
use crate::errors::{Code, DeliveryError, Failure, RawError};
use crate::file_ops;
use crate::files_manifest;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hlsr_core::logger::Logger;
use serde_json::{Map, Value, json};
use std::fs;
use std::path::{Path, PathBuf};

const TEMP_SUFFIX: &str = ".hlsr-tmp";
const MAX_SOUND_BYTES: u64 = 2 * 1024 * 1024;

/// `GAMES[gameId]`: где у игры лежат моды и куда ложатся копии.
#[derive(Debug, Clone)]
pub struct Game {
    pub id: &'static str,
    /// Откуда копировать (`sourceDir`): `<мод>/hlsr-packs`.
    pub source_dir: &'static str,
    /// Куда ложатся копии (`targetDir(pack)`), от папки игры.
    target_dir: fn(&Pack) -> String,
    /// Докуда подчищать пустые папки после выключения.
    pub prune_root: &'static str,
    /// Папка с оригиналами игры (`valve` у HL1), где их искать для звука «оригинал».
    pub base_dir: Option<&'static str>,
}

impl Game {
    /// `targetDir(pack)`.
    pub fn target_dir(&self, pack: &Pack) -> String {
        (self.target_dir)(pack)
    }
}

fn hl1_target(_pack: &Pack) -> String {
    "Half-Life/valve_WON".to_owned()
}

fn ghosting_target(pack: &Pack) -> String {
    format!("ghostingmod/custom/{}", pack.id)
}

/// `GAMES`.
pub static GAMES: [Game; 2] = [
    Game {
        id: "hl1-won",
        source_dir: "Half-Life/valve_WON/hlsr-packs",
        target_dir: hl1_target,
        prune_root: "Half-Life/valve_WON",
        base_dir: Some("Half-Life/valve"),
    },
    Game {
        id: "ghosting-renewed",
        source_dir: "ghostingmod/hlsr-packs",
        target_dir: ghosting_target,
        prune_root: "ghostingmod",
        base_dir: None,
    },
];

/// `STATE_DIR`: журнал и включённые моды, от папки игры.
pub const STATE_DIR: &str = ".hlsr/custom-packs";

/// Мод: файлы — пути внутри мода, имена как в игре (WON — строчные; Source регистр не различает).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pack {
    pub id: &'static str,
    pub game: &'static str,
    pub kind: &'static str,
    pub author: Option<&'static str>,
    pub files: &'static [&'static str],
}

/// `GAMES[gameId]`.
pub fn game_of(game_id: &str) -> Option<&'static Game> {
    GAMES.iter().find(|game| game.id == game_id)
}

/// `targets()`: куда ложатся копии (пути от папки игры). Для проверки файлов это слой
/// кастомизации (`mutable-paths.js`).
pub fn targets() -> Vec<String> {
    PACKS
        .iter()
        .flat_map(|pack| {
            let dir = game_of(pack.game).map_or_else(String::new, |game| game.target_dir(pack));
            pack.files.iter().map(move |file| format!("{dir}/{file}"))
        })
        .collect()
}

/// `PACKS` — выгружено из `custom-packs.js` 0.4.9 (`under`/`each` раскрыты).
pub static PACKS: [Pack; 8] = [
    Pack {
        id: "arianon-viewmodels",
        game: "hl1-won",
        kind: "models",
        author: Some("Arianon"),
        files: &[
            "models/v_357.mdl",
            "models/v_9mmar.mdl",
            "models/v_9mmhandgun.mdl",
            "models/v_crossbow.mdl",
            "models/v_crowbar.mdl",
            "models/v_egon.mdl",
            "models/v_gauss.mdl",
            "models/v_grenade.mdl",
            "models/v_hgun.mdl",
            "models/v_rpg.mdl",
            "models/v_satchel.mdl",
            "models/v_satchel_radio.mdl",
            "models/v_shotgun.mdl",
            "models/v_squeak.mdl",
            "models/v_tripmine.mdl",
        ],
    },
    Pack {
        id: "item-sprites",
        game: "hl1-won",
        kind: "sprites",
        author: None,
        files: &[
            "models/w_357.mdl",
            "models/w_357ammobox.mdl",
            "models/w_9mmar.mdl",
            "models/w_9mmarclip.mdl",
            "models/w_9mmclip.mdl",
            "models/w_9mmhandgun.mdl",
            "models/w_argrenade.mdl",
            "models/w_battery.mdl",
            "models/w_crossbow.mdl",
            "models/w_crossbow_clip.mdl",
            "models/w_egon.mdl",
            "models/w_gauss.mdl",
            "models/w_gaussammo.mdl",
            "models/w_grenade.mdl",
            "models/w_hgun.mdl",
            "models/w_longjump.mdl",
            "models/w_medkit.mdl",
            "models/w_rpg.mdl",
            "models/w_rpgammo.mdl",
            "models/w_satchel.mdl",
            "models/w_shotbox.mdl",
            "models/w_shotgun.mdl",
            "models/w_sqknest.mdl",
        ],
    },
    Pack {
        id: "oxcjae-soundpack-lite",
        game: "hl1-won",
        kind: "sounds",
        author: Some("oxcjae"),
        files: &[
            "sound/barney/ba_die3.wav",
            "sound/barney/ba_dontmake.wav",
            "sound/barney/ba_endline.wav",
            "sound/barney/ba_iwish.wav",
            "sound/barney/ba_somuch.wav",
            "sound/barney/ba_stepoff.wav",
            "sound/barney/ba_tomb.wav",
            "sound/barney/ba_uwish.wav",
            "sound/barney/c1a0_ba_hevyes.wav",
            "sound/barney/c1a4_ba_octo3.wav",
            "sound/barney/c2a2_ba_launch.wav",
            "sound/barney/c2a3_ba_assn.wav",
            "sound/barney/c2a4_ba_5tau.wav",
            "sound/barney/c2a4_ba_alive.wav",
            "sound/hgrunt/c2a3_ambush_vox.wav",
            "sound/hgrunt/c2a3_hg_1drag.wav",
            "sound/hgrunt/c2a3_hg_2drag.wav",
            "sound/hgrunt/c2a3_hg_3drag.wav",
            "sound/hgrunt/c2a3_hg_4drag.wav",
            "sound/hgrunt/c2a3_hg_5drag.wav",
            "sound/hgrunt/c2a3_hg_laugh.wav",
            "sound/hgrunt/c2a5_hg_abandon.wav",
            "sound/hgrunt/c2a5_hg_lebuz.wav",
            "sound/hgrunt/hg_civvies.wav",
            "sound/scientist/afellowsci.wav",
            "sound/scientist/ahfreeman.wav",
            "sound/scientist/c1a0_sci_crit1a.wav",
            "sound/scientist/c1a0_sci_ctrl1a.wav",
            "sound/scientist/c1a0_sci_ctrl2a.wav",
            "sound/scientist/c1a0_sci_dis1a.wav",
            "sound/scientist/c1a0_sci_dis9a.wav",
            "sound/scientist/c1a0_sci_disa.wav",
            "sound/scientist/c1a0_sci_lock4a.wav",
            "sound/scientist/c1a0_sci_lock6a.wav",
            "sound/scientist/c1a2_sci_dangling.wav",
            "sound/scientist/c1a3_sci_silo1a.wav",
            "sound/scientist/c1a3_sci_silo2a.wav",
            "sound/scientist/c1a4_sci_gener.wav",
            "sound/scientist/c2a3_sci_track.wav",
            "sound/scientist/c2a4_sci_letout.wav",
            "sound/scientist/c2a5_sci_boobie.wav",
            "sound/scientist/c3a2_sci_fool.wav",
            "sound/scientist/canttakemore.wav",
            "sound/scientist/freeman.wav",
            "sound/scientist/freemanalive.wav",
            "sound/scientist/goodtoseeyou.wav",
            "sound/scientist/greetings.wav",
            "sound/scientist/greetings2.wav",
            "sound/scientist/heal1.wav",
            "sound/scientist/heal2.wav",
            "sound/scientist/heal3.wav",
            "sound/scientist/heal4.wav",
            "sound/scientist/heal5.wav",
            "sound/scientist/hello.wav",
            "sound/scientist/hello2.wav",
            "sound/scientist/hellofreeman.wav",
            "sound/scientist/hellofromlab.wav",
            "sound/scientist/hellothere.wav",
            "sound/scientist/holdstill.wav",
            "sound/scientist/inmesstoo.wav",
            "sound/scientist/iwounded.wav",
            "sound/scientist/iwounded2.wav",
            "sound/scientist/iwoundedbad.wav",
            "sound/scientist/letstrythis.wav",
            "sound/scientist/newhevsuit.wav",
            "sound/scientist/sci_pain1.wav",
            "sound/scientist/sci_pain2.wav",
            "sound/scientist/sci_pain3.wav",
            "sound/scientist/sci_pain4.wav",
            "sound/scientist/sci_pain5.wav",
            "sound/scientist/scream01.wav",
            "sound/scientist/scream3.wav",
            "sound/scientist/scream4.wav",
            "sound/scientist/youlookbad.wav",
            "sound/scientist/youneedmedic.wav",
            "sound/scientist/youwounded.wav",
        ],
    },
    Pack {
        id: "condensed-hud",
        game: "ghosting-renewed",
        kind: "hud",
        author: None,
        files: &[
            "materials/vgui/hud/icon_acid.vmt",
            "materials/vgui/hud/icon_acid.vtf",
            "materials/vgui/hud/icon_blank.vmt",
            "materials/vgui/hud/icon_blank.vtf",
            "materials/vgui/hud/icon_blast.vmt",
            "materials/vgui/hud/icon_blast.vtf",
            "materials/vgui/hud/icon_energy.vmt",
            "materials/vgui/hud/icon_energy.vtf",
            "materials/vgui/hud/icon_heat.vmt",
            "materials/vgui/hud/icon_heat.vtf",
            "materials/vgui/hud/icon_oxygen.vmt",
            "materials/vgui/hud/icon_oxygen.vtf",
            "materials/vgui/hud/icon_poison.vmt",
            "materials/vgui/hud/icon_poison.vtf",
            "materials/vgui/hud/icon_radiation.vmt",
            "materials/vgui/hud/icon_radiation.vtf",
            "materials/vgui/hud/icon_shock.vmt",
            "materials/vgui/hud/icon_shock.vtf",
            "resource/clientscheme.res",
            "resource/valve_english.txt",
            "resource/valve_russian.txt",
            "resource/synergy_english.txt",
            "resource/Hype font.txt",
            "resource/LICENSE_OFL.txt",
            "resource/din1451alt.ttf",
            "resource/din1451alt2.ttf",
            "resource/FacileSans.ttf",
            "resource/halflife2.ttf",
            "resource/hl2crosshairs.ttf",
            "resource/HL2EP2.ttf",
            "resource/HL2MMod.ttf",
            "resource/HL2MP.ttf",
            "resource/HYPE.ttf",
            "resource/K12HL2.ttf",
            "resource/lostcoast.ttf",
            "resource/nokia-pure-text.ttf",
            "resource/RealBeta's Weapon Icons.ttf",
            "resource/SynFont.ttf",
            "resource/NotoSans-Bold.ttf",
            "resource/NotoSans-BoldItalic.ttf",
            "resource/NotoSans-Italic.ttf",
            "resource/NotoSans-Regular.ttf",
            "resource/Roboto-Black.ttf",
            "resource/Roboto-BlackItalic.ttf",
            "resource/Roboto-Bold.ttf",
            "resource/Roboto-BoldItalic.ttf",
            "resource/Roboto-Italic.ttf",
            "resource/Roboto-Light.ttf",
            "resource/Roboto-LightItalic.ttf",
            "resource/Roboto-Medium.ttf",
            "resource/Roboto-MediumItalic.ttf",
            "resource/Roboto-Regular.ttf",
            "resource/Roboto-Thin.ttf",
            "resource/Roboto-ThinItalic.ttf",
            "resource/RobotoCondensed-Bold.ttf",
            "resource/RobotoCondensed-BoldItalic.ttf",
            "resource/RobotoCondensed-Italic.ttf",
            "resource/RobotoCondensed-Light.ttf",
            "resource/RobotoCondensed-LightItalic.ttf",
            "resource/RobotoCondensed-Regular.ttf",
            "scripts/hudanimations.txt",
            "scripts/hudanimations_manifest.txt",
            "scripts/hudanimations_syn.txt",
            "scripts/hudlayout.res",
        ],
    },
    Pack {
        id: "m9-bayonet",
        game: "ghosting-renewed",
        kind: "models",
        author: None,
        files: &[
            "models/weapons/v_crowbar.mdl",
            "models/weapons/v_crowbar.ani",
            "models/weapons/v_crowbar.vvd",
            "models/weapons/v_crowbar.dx80.vtx",
            "models/weapons/v_crowbar.dx90.vtx",
            "models/weapons/v_crowbar.sw.vtx",
            "materials/models/csgo_knife/knife_m9_bay.vmt",
            "materials/models/csgo_knife/knife_m9_bay.vtf",
            "materials/models/csgo_knife/knife_m9_bay_ao.vtf",
            "materials/models/csgo_knife/knife_m9_bay_exponent.vtf",
        ],
    },
    Pack {
        id: "signed-smg",
        game: "ghosting-renewed",
        kind: "skins",
        author: Some("Eold"),
        files: &["materials/models/weapons/v_smg1/v_smg1_sheet.vtf"],
    },
    Pack {
        id: "signed-shotgun",
        game: "ghosting-renewed",
        kind: "skins",
        author: Some("Eold"),
        files: &["materials/models/weapons/v_shotgun/vshotgun_albedo.vtf"],
    },
    Pack {
        id: "signed-rpg",
        game: "ghosting-renewed",
        kind: "skins",
        author: Some("Eold"),
        files: &["materials/models/weapons/v_rocket_launcher/rocket launcher sheet.vtf"],
    },
];

/// Только для самотестов: «сбой» после файла (`options.hooks.afterFile`).
pub type AfterFile<'a> = &'a mut dyn FnMut(&str) -> Result<(), Failure>;

/// `hasPacks(gameId)`.
pub fn has_packs(game_id: &str) -> bool {
    PACKS.iter().any(|pack| pack.game == game_id)
}

/// `packOf(id)`.
fn pack_of(id: &str) -> Result<&'static Pack, DeliveryError> {
    PACKS.iter().find(|pack| pack.id == id).ok_or_else(|| {
        DeliveryError::new(Code::CustomPackUnknown, format!("Неизвестный пак: {id}"))
    })
}

/// `sha256Of(file)`: нет файла (или это папка) — `None`. Читается блоками, не целиком.
fn sha256_of(file: &Path) -> Result<Option<String>, RawError> {
    if fs::metadata(file).is_ok_and(|meta| meta.is_dir()) {
        return Ok(None);
    }
    match files_manifest::hash_file_sync(file) {
        Ok(hash) => Ok(Some(hash)),
        Err(error) if matches!(error.code.as_deref(), Some("ENOENT" | "EISDIR")) => Ok(None),
        Err(error) => Err(error),
    }
}

/// `sizeOf(file)`: размер файла; нет файла или это не файл — `None` (в JS −1).
fn size_of(file: &Path) -> Option<u64> {
    fs::metadata(file)
        .ok()
        .filter(fs::Metadata::is_file)
        .map(|meta| meta.len())
}

fn with_suffix(file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(TEMP_SUFFIX);
    PathBuf::from(name)
}

/// `layout(installPath, pack)`: где лежат журнал, копии, резервные копии и сам мод.
struct Layout {
    install: PathBuf,
    state_root: PathBuf,
    state_file: PathBuf,
    prune_root: PathBuf,
    backup_root: PathBuf,
    source_dir: String,
    target_dir: String,
}

impl Layout {
    fn of(install: &Path, pack: &Pack) -> Result<Layout, DeliveryError> {
        let game = game_of(pack.game).expect("игра пака в GAMES");
        let state_root = file_ops::resolve_inside(install, STATE_DIR, None)?;
        Ok(Layout {
            install: install.to_path_buf(),
            state_file: state_root.join("state.json"),
            backup_root: state_root.join("backup"),
            state_root,
            prune_root: file_ops::resolve_inside(install, game.prune_root, None)?,
            source_dir: format!("{}/{}", game.source_dir, pack.id),
            target_dir: game.target_dir(pack),
        })
    }

    fn source(&self, file: &str) -> Result<PathBuf, DeliveryError> {
        file_ops::resolve_inside(&self.install, &format!("{}/{file}", self.source_dir), None)
    }

    fn target(&self, file: &str) -> Result<PathBuf, DeliveryError> {
        file_ops::resolve_inside(&self.install, &format!("{}/{file}", self.target_dir), None)
    }

    fn backup(&self, file: &str) -> Result<PathBuf, DeliveryError> {
        file_ops::resolve_inside(
            &self.backup_root,
            &format!("{}/{file}", self.target_dir),
            None,
        )
    }
}

/// Журнал: `{packs, pending}` (`readState`).
struct State {
    packs: Map<String, Value>,
    pending: Option<Map<String, Value>>,
}

impl State {
    fn read(file: &Path) -> State {
        let state = hlsr_core::json_store::read_json_safe(file);
        State {
            packs: state
                .get("packs")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
            pending: state.get("pending").and_then(Value::as_object).cloned(),
        }
    }

    fn write(&self, file: &Path) -> Result<(), RawError> {
        let pending = self.pending.clone().map_or(Value::Null, Value::Object);
        write_json_atomic(file, &json!({"packs": self.packs, "pending": pending}))
    }
}

fn state_file_of(install: &Path) -> Result<PathBuf, DeliveryError> {
    Ok(file_ops::resolve_inside(install, STATE_DIR, None)?.join("state.json"))
}

/// Копия через временный файл рядом: на месте цели всегда либо прежний файл, либо новый
/// целиком, недописанного не бывает.
fn copy_atomic(source: &Path, target: &Path, relative: &str) -> Result<(), Failure> {
    let temp = with_suffix(target);
    file_ops::copy_file_sync(source, &temp, Some(relative))?;
    file_ops::make_writable(&temp);
    file_ops::make_writable(target);
    fs::rename(&temp, target).map_err(|error| {
        let _ = fs::remove_file(&temp);
        let raw = RawError::fs_with_dest(&error, "rename", &temp, Some(target));
        match file_ops::wrap_lock_error(&raw, relative) {
            Some(locked) => Failure::Delivery(locked),
            None => Failure::Raw(raw),
        }
    })
}

/// `sha256Of(target) === entry.sha256`: `null` в записи совпадает только с «нет файла».
fn same_sha(actual: Option<&str>, entry: &Value) -> bool {
    match (actual, entry.get("sha256")) {
        (Some(actual), Some(Value::String(expected))) => actual == expected,
        (None, Some(Value::Null)) => true,
        _ => false,
    }
}

/// `undo(where, files, hooks)`: возвращает всё, как было до включения. Трогает только файлы,
/// которые сейчас совпадают с копией пака: изменённый после включения файл уже чужой.
fn undo(
    at: &Layout,
    files: &Map<String, Value>,
    mut after_file: Option<AfterFile>,
) -> Result<(), Failure> {
    for (file, entry) in files {
        let target = at.target(file)?;
        let backup = at.backup(file)?;
        file_ops::remove_file_sync(&with_suffix(&target), Some(file))?;
        file_ops::remove_file_sync(&with_suffix(&backup), Some(file))?;

        if same_sha(sha256_of(&target)?.as_deref(), entry) {
            if !hlsr_core::js::truthy(entry.get("backup").unwrap_or(&Value::Null)) {
                file_ops::remove_file_sync(&target, Some(file))?;
            } else if size_of(&backup).is_some() {
                copy_atomic(&backup, &target, file)?;
            }
            // Копии чужого файла нет (сбой до неё) — значит, он не тронут.
        }
        // Резервная копия больше не нужна и тогда, когда на месте цели не копия пака: это либо
        // сам чужой файл (сбой до копирования), либо файл, который положили после включения, —
        // он новее резервной и остаётся (как в 0.4.9).
        file_ops::remove_file_sync(&backup, Some(file))?;
        if let Some(dir) = target.parent() {
            file_ops::prune_empty_dirs(&at.prune_root, dir);
        }
        if let Some(dir) = backup.parent() {
            file_ops::prune_empty_dirs(&at.state_root, dir);
        }
        if let Some(hook) = after_file.as_mut() {
            hook(file)?;
        }
    }
    Ok(())
}

/// `adoptedRecord(pack, where)`: копии на месте и совпадают с файлами пака, а записи о
/// включении нет — состояние потеряно (переустановка переносит `ghostingmod/custom`, но не
/// `.hlsr`). Такой пак включён; выключение уберёт копии, резервных копий у него нет.
fn adopted_record(pack: &Pack, at: &Layout) -> Result<Option<Value>, Failure> {
    for file in pack.files {
        let size = size_of(&at.source(file)?);
        if size.is_none() || size_of(&at.target(file)?) != size {
            return Ok(None);
        }
    }
    let mut files = Map::new();
    for file in pack.files {
        let sha256 = sha256_of(&at.source(file)?)?;
        if sha256_of(&at.target(file)?)? != sha256 {
            return Ok(None);
        }
        files.insert(
            (*file).to_owned(),
            json!({"sha256": sha256, "backup": false}),
        );
    }
    Ok(Some(json!({"files": files})))
}

/// `files` записи из `state.json`, только файлы пака: испорченный журнал не удалит чужой файл
/// в папке игры (в 0.4.9 ключи не проверялись).
fn own_files(pack: &Pack, files: Option<&Value>) -> Map<String, Value> {
    files
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(file, _)| pack.files.contains(&file.as_str()))
        .map(|(file, entry)| (file.clone(), entry.clone()))
        .collect()
}

/// `recover(installPath)`: доделывает операцию, прерванную сбоем. `true` — было что доделать.
pub fn recover(install: &Path, logger: &dyn Logger) -> Result<bool, Failure> {
    let state_file = state_file_of(install)?;
    let mut state = State::read(&state_file);
    let Some(pending) = state.pending.take() else {
        return Ok(false);
    };
    let id = hlsr_core::js::to_string(pending.get("id").unwrap_or(&Value::Null));
    if let Some(pack) = PACKS
        .iter()
        .find(|pack| Some(pack.id) == pending.get("id").and_then(Value::as_str))
    {
        let files = own_files(pack, pending.get("files"));
        undo(&Layout::of(install, pack)?, &files, None)?;
    }
    state.packs.remove(&id);
    state.write(&state_file)?;
    let done = if pending.get("op").and_then(Value::as_str) == Some("enable") {
        "включение откачено"
    } else {
        "выключение доведено"
    };
    logger.warn(&format!("[custom-packs] пак {id}: прерванное {done}"));
    Ok(true)
}

fn is_available(install: Option<&Path>, pack: &Pack) -> Result<bool, Failure> {
    let Some(install) = install else {
        return Ok(false);
    };
    let at = Layout::of(install, pack)?;
    for file in pack.files {
        if size_of(&at.source(file)?).is_none() {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `list(installPath, gameId, {recover})`: моды игры `{id, kind, author, files, available,
/// enabled}`. `recover` — доделать прерванное (только когда игра не запущена).
pub fn list(
    install: Option<&Path>,
    game_id: &str,
    recover_first: bool,
    logger: &dyn Logger,
) -> Result<Vec<Value>, Failure> {
    if let (Some(install), true) = (install, recover_first) {
        recover(install, logger)?;
    }
    let packs = match install {
        Some(install) => State::read(&state_file_of(install)?).packs,
        None => Map::new(),
    };
    let mut out = Vec::new();
    for pack in PACKS.iter().filter(|pack| pack.game == game_id) {
        let available = is_available(install, pack)?;
        let enabled = hlsr_core::js::truthy(packs.get(pack.id).unwrap_or(&Value::Null))
            || match (available, install) {
                (true, Some(install)) => {
                    adopted_record(pack, &Layout::of(install, pack)?)?.is_some()
                }
                _ => false,
            };
        out.push(json!({
            "id": pack.id,
            "kind": pack.kind,
            "author": pack.author,
            "files": pack.files,
            "available": available,
            "enabled": enabled,
        }));
    }
    Ok(out)
}

fn enable(
    pack: &Pack,
    state: &mut State,
    at: &Layout,
    mut after_file: Option<AfterFile>,
) -> Result<(), Failure> {
    let mut files = Map::new();
    for file in pack.files {
        let sha256 = sha256_of(&at.source(file)?)?;
        let backup = size_of(&at.target(file)?).is_some();
        files.insert(
            (*file).to_owned(),
            json!({"sha256": sha256, "backup": backup}),
        );
    }
    state.pending = Some(json_object(
        json!({"id": pack.id, "op": "enable", "files": files}),
    ));
    state.write(&at.state_file)?;

    let copied = (|| -> Result<(), Failure> {
        for file in pack.files {
            let target = at.target(file)?;
            if files[*file]["backup"] == Value::Bool(true) {
                copy_atomic(&target, &at.backup(file)?, file)?;
            }
            copy_atomic(&at.source(file)?, &target, file)?;
            if let Some(hook) = after_file.as_mut() {
                hook(file)?;
            }
        }
        Ok(())
    })();
    if let Err(error) = copied {
        undo(at, &files, None)?;
        state.pending = None;
        state.write(&at.state_file)?;
        return Err(error);
    }

    state
        .packs
        .insert(pack.id.to_owned(), json!({"files": files}));
    state.pending = None;
    state.write(&at.state_file)?;
    Ok(())
}

fn disable(
    pack: &Pack,
    state: &mut State,
    at: &Layout,
    record: &Value,
    after_file: Option<AfterFile>,
) -> Result<(), Failure> {
    let files = own_files(pack, record.get("files"));
    state.pending = Some(json_object(
        json!({"id": pack.id, "op": "disable", "files": files}),
    ));
    state.write(&at.state_file)?;

    undo(at, &files, after_file)?;
    state.packs.remove(pack.id);
    state.pending = None;
    state.write(&at.state_file)?;
    Ok(())
}

fn json_object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// `setEnabled(installPath, id, enabled)`: включить или выключить мод; ответ — [`list`] игры.
/// Игра в это время не должна быть запущена (проверяет вызывающий).
pub fn set_enabled(
    install: &Path,
    id: &str,
    enabled: bool,
    logger: &dyn Logger,
    after_file: Option<AfterFile>,
) -> Result<Vec<Value>, Failure> {
    let pack = pack_of(id)?;
    recover(install, logger)?;

    let at = Layout::of(install, pack)?;
    let mut state = State::read(&at.state_file);
    let record = match state
        .packs
        .get(pack.id)
        .filter(|record| hlsr_core::js::truthy(record))
    {
        Some(record) => Some(record.clone()),
        None if is_available(Some(install), pack)? => adopted_record(pack, &at)?,
        None => None,
    };
    if record.is_some() != enabled {
        if enabled && !is_available(Some(install), pack)? {
            return Err(DeliveryError::new(
                Code::CustomPackNotAvailable,
                format!("Файлов пака {} нет в игре", pack.id),
            )
            .into());
        }
        match &record {
            None => enable(pack, &mut state, &at, after_file)?,
            Some(record) => disable(pack, &mut state, &at, record, after_file)?,
        }
        logger.info(&format!(
            "[custom-packs] пак {} {}, файлов {}",
            pack.id,
            if enabled {
                "включён"
            } else {
                "выключен"
            },
            pack.files.len()
        ));
    }
    list(Some(install), pack.game, false, logger)
}

/// `readSound(installPath, id, file, which)`: звук для прослушивания — замена из пака
/// (`pack`) или оригинал из папки игры (`original`), не больше 2 МиБ; `data:audio/wav;base64,…`.
pub fn read_sound(
    install: &Path,
    id: &str,
    file: &str,
    which: Option<&str>,
) -> Result<String, Failure> {
    let pack = pack_of(id)?;
    let base_dir = game_of(pack.game).and_then(|game| game.base_dir);
    let unknown = || {
        DeliveryError::new(
            Code::CustomPackFileUnknown,
            format!("Нет звука {file} в паке {}", pack.id),
        )
    };
    let (Some(base_dir), "sounds", true, Some(which @ ("pack" | "original"))) =
        (base_dir, pack.kind, pack.files.contains(&file), which)
    else {
        return Err(unknown().into());
    };
    let full = if which == "pack" {
        Layout::of(install, pack)?.source(file)?
    } else {
        file_ops::resolve_inside(install, &format!("{base_dir}/{file}"), None)?
    };
    let meta = fs::metadata(&full).map_err(|error| RawError::fs(&error, "stat", &full))?;
    if !meta.is_file() || meta.len() > MAX_SOUND_BYTES {
        return Err(DeliveryError::new(
            Code::CustomPackFileUnknown,
            format!("Звук {file} не прочитан"),
        )
        .into());
    }
    let bytes = fs::read(&full).map_err(|error| RawError::fs(&error, "open", &full))?;
    Ok(format!("data:audio/wav;base64,{}", STANDARD.encode(bytes)))
}
