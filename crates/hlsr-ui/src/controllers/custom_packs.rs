//! Моды игры (`CustomPacks.vue` 0.4.9): плитки с кадрами «до / после», переключатель (сразу
//! кладёт файлы мода в игру или убирает их — `custom-packs-set`, «Сохранить» не нужен), автор,
//! описание, тип, список файлов, у звуков — прослушать оригинал или мод (`custom-packs-sound`).
//! Без окна: состояние и чистые функции; каналы и звук ведёт вид.

use super::personalization_panel::GOLDSRC_GAME_IDS;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;

/// Снимок превью: ассет, подпись, фокус (`object-position`, доли ширины и высоты).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shot {
    pub src: &'static str,
    pub label: Option<&'static str>,
    pub focus: (f32, f32),
}

const fn shot(src: &'static str, label: Option<&'static str>, x: f32, y: f32) -> Shot {
    Shot {
        src,
        label,
        focus: (x / 100., y / 100.),
    }
}

const ITEM_SPRITES: [Shot; 2] = [
    shot("item-sprites-off.jpg", Some("#L_CPK_SHOT_OFF"), 50., 62.),
    shot("item-sprites-on.jpg", Some("#L_CPK_SHOT_ON"), 50., 62.),
];
const CONDENSED_HUD: [Shot; 2] = [
    shot(
        "condensed-hud-off.jpg",
        Some("#L_CPK_SHOT_MOD_OFF"),
        0.,
        100.,
    ),
    shot("condensed-hud-on.jpg", Some("#L_CPK_SHOT_MOD_ON"), 0., 100.),
];
const M9_BAYONET: [Shot; 1] = [shot("m9-bayonet.jpg", None, 64., 35.)];
const SIGNED_SMG: [Shot; 1] = [shot("signed-smg.jpg", None, 55., 45.)];
const SIGNED_SHOTGUN: [Shot; 1] = [shot("signed-shotgun.jpg", None, 40., 50.)];
const SIGNED_RPG: [Shot; 1] = [shot("signed-rpg.jpg", None, 55., 40.)];

/// `SHOTS`: снимки «без мода / с модом» (у ножа и перекрасок — один кадр без подписи); `src` —
/// имя в `assets/launcher/custom-packs`.
pub fn shots(id: &str) -> &'static [Shot] {
    match id {
        "item-sprites" => &ITEM_SPRITES,
        "condensed-hud" => &CONDENSED_HUD,
        "m9-bayonet" => &M9_BAYONET,
        "signed-smg" => &SIGNED_SMG,
        "signed-shotgun" => &SIGNED_SHOTGUN,
        "signed-rpg" => &SIGNED_RPG,
        _ => &[],
    }
}

/// `KIND_ICONS`.
pub fn kind_icon(kind: &str) -> &'static str {
    match kind {
        "models" => "fa-crosshairs",
        "skins" => "fa-spray-can",
        "sprites" => "fa-shapes",
        "sounds" => "fa-volume-up",
        "hud" => "fa-gauge-high",
        _ => "",
    }
}

/// `ERRORS[code] || "#L_CPK_ERR_FAILED"`: ответ `custom-packs-set` → строка (её же показывает код
/// настроек, когда переключает моды из кода).
pub fn error_key(code: &str) -> &'static str {
    match code {
        "GAME_RUNNING" => "#L_CPK_ERR_RUNNING",
        "BUSY" => "#L_CPK_ERR_BUSY",
        "FILE_LOCKED" => "#L_CPK_ERR_LOCKED",
        "CUSTOM_PACK_NOT_AVAILABLE" => "#L_CPK_NOT_AVAILABLE",
        "NOT_INSTALLED" => "#L_CPK_NOT_INSTALLED",
        _ => "#L_CPK_ERR_FAILED",
    }
}

/// Мод из ответа `custom-packs-get/-set`.
#[derive(Debug, Clone, PartialEq)]
pub struct Pack {
    pub id: String,
    pub kind: String,
    pub author: Option<String>,
    pub files: Vec<String>,
    pub available: bool,
    pub enabled: bool,
}

impl Pack {
    fn of(value: &Value) -> Option<Pack> {
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        Some(Pack {
            id: text("id")?,
            kind: text("kind").unwrap_or_default(),
            author: text("author").filter(|author| !author.is_empty()),
            files: value
                .get("files")
                .and_then(Value::as_array)
                .map(|files| {
                    files
                        .iter()
                        .filter_map(|file| file.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            available: hlsr_core::js::truthy(value.get("available").unwrap_or(&Value::Null)),
            enabled: hlsr_core::js::truthy(value.get("enabled").unwrap_or(&Value::Null)),
        })
    }

    fn key(&self) -> String {
        self.id.replace('-', "_").to_uppercase()
    }

    pub fn title_key(&self) -> String {
        format!("#L_CPK_TITLE_{}", self.key())
    }

    pub fn desc_key(&self) -> String {
        format!("#L_CPK_DESC_{}", self.key())
    }

    /// `#L_CPK_KIND_<KIND>` (аргумент — число файлов).
    pub fn kind_key(&self) -> String {
        format!("#L_CPK_KIND_{}", self.kind.to_uppercase())
    }
}

/// Что слушают: файл из мода или из игры.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    Original,
    Pack,
}

impl Which {
    pub const BOTH: [Which; 2] = [Which::Original, Which::Pack];

    pub fn name(self) -> &'static str {
        match self {
            Which::Original => "original",
            Which::Pack => "pack",
        }
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Which::Original => "#L_CPK_PLAY_ORIGINAL",
            Which::Pack => "#L_CPK_PLAY_PACK",
        }
    }
}

/// Состояние `CustomPacks.vue` (`data()`).
#[derive(Debug, Clone)]
pub struct CustomPacks {
    pub game_id: String,
    /// `null` — список ещё грузится.
    pub packs: Option<Vec<Pack>>,
    pub installed: bool,
    /// Ключ строки ошибки; пусто — ошибки нет.
    pub error: &'static str,
    /// Мод, в карточке которого ошибка; пусто — над списком.
    pub error_pack: String,
    pub switching: String,
    /// `<мод>/<файл>/<original|pack>`, что играет.
    pub playing: String,
}

impl CustomPacks {
    pub fn new(game_id: &str) -> CustomPacks {
        CustomPacks {
            game_id: game_id.to_owned(),
            packs: None,
            installed: true,
            error: "",
            error_pack: String::new(),
            switching: String::new(),
            playing: String::new(),
        }
    }

    pub fn is_hl1(&self) -> bool {
        GOLDSRC_GAME_IDS.contains(&self.game_id.as_str())
    }

    pub fn note_key(&self) -> &'static str {
        if self.is_hl1() {
            "#L_CPK_NOTE_HL1"
        } else {
            "#L_CPK_NOTE_GHOSTING"
        }
    }

    /// Почему мод недоступен (`.cpk-card__needs`).
    pub fn needs_key(&self) -> &'static str {
        if !self.installed {
            "#L_CPK_NOT_INSTALLED"
        } else if self.is_hl1() {
            "#L_CPK_NOT_AVAILABLE"
        } else {
            "#L_CPK_NOT_AVAILABLE_GHOSTING"
        }
    }

    fn take_packs(&mut self, list: Option<&Value>) -> Option<Value> {
        let list = list.filter(|list| list.is_array())?;
        self.packs = Some(
            list.as_array()
                .into_iter()
                .flatten()
                .filter_map(Pack::of)
                .collect(),
        );
        Some(list.clone())
    }

    /// Ответ `custom-packs-get`: список для `$emit("packs")` (предпросмотр знает о HUD-моде).
    pub fn loaded(&mut self, reply: Result<Value, String>) -> Option<Value> {
        let result = reply.unwrap_or(Value::Null);
        if result.get("status").and_then(Value::as_str) == Some("ok") {
            let emitted = self.take_packs(result.get("packs"));
            if self.packs.is_none() {
                self.packs = Some(Vec::new());
            }
            self.installed = result.get("installed") != Some(&Value::Bool(false));
            emitted
        } else {
            self.packs = Some(Vec::new());
            self.error = "#L_CPK_ERR_FAILED";
            None
        }
    }

    /// Начало переключения: `false` — уже идёт другое (переключатель остаётся как на диске).
    pub fn begin_toggle(&mut self, pack_id: &str) -> bool {
        if !self.switching.is_empty() {
            return false;
        }
        self.switching = pack_id.to_owned();
        self.error = "";
        self.error_pack = pack_id.to_owned();
        self.stop();
        true
    }

    /// Ответ `custom-packs-set`: новый список для `$emit("packs")`.
    pub fn toggled(&mut self, reply: Result<Value, String>) -> Option<Value> {
        let emitted = match reply {
            Ok(result) => {
                let emitted = self.take_packs(result.get("packs"));
                if result.get("status").and_then(Value::as_str) != Some("ok") {
                    let code = result.get("code").and_then(Value::as_str).unwrap_or("");
                    self.error = error_key(code);
                }
                emitted
            }
            Err(_) => {
                self.error = "#L_CPK_ERR_FAILED";
                None
            }
        };
        self.switching.clear();
        emitted
    }

    fn sound_id(pack: &str, file: &str, which: Which) -> String {
        format!("{pack}/{file}/{}", which.name())
    }

    pub fn is_playing(&self, pack: &str, file: &str, which: Which) -> bool {
        self.playing == Self::sound_id(pack, file, which)
    }

    pub fn stop(&mut self) {
        self.playing.clear();
    }

    /// Кнопка «оригинал / мод»: `Some(id)` — запросить звук; та же кнопка ещё раз — стоп.
    pub fn play(&mut self, pack: &str, file: &str, which: Which) -> Option<String> {
        let id = Self::sound_id(pack, file, which);
        let again = self.playing == id;
        self.stop();
        if again {
            return None;
        }
        self.playing = id.clone();
        Some(id)
    }

    /// Ответ `custom-packs-sound`: байты WAV, если звук всё ещё нужен (`playing === id`).
    pub fn sound_reply(
        &mut self,
        id: &str,
        pack: &str,
        reply: Result<Value, String>,
    ) -> Option<Vec<u8>> {
        if self.playing != id {
            return None;
        }
        let result = reply.unwrap_or(Value::Null);
        let data = result
            .get("dataUrl")
            .and_then(Value::as_str)
            .filter(|_| result.get("status").and_then(Value::as_str) == Some("ok"));
        let Some(data) = data else {
            self.playing.clear();
            self.error = "#L_CPK_ERR_SOUND";
            self.error_pack = pack.to_owned();
            return None;
        };
        // `audio.play().catch(...)`: не разобрался — тихо, без ошибки.
        let bytes = data
            .split_once(";base64,")
            .and_then(|(_, body)| STANDARD.decode(body).ok());
        if bytes.is_none() {
            self.playing.clear();
        }
        bytes
    }

    /// `audio.onended` (и сбой воспроизведения).
    pub fn ended(&mut self, id: &str) {
        if self.playing == id {
            self.playing.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pack(id: &str, kind: &str, enabled: bool, available: bool) -> Value {
        json!({"id": id, "kind": kind, "author": "a", "files": ["sound/x.wav", "sound/y.wav"],
            "available": available, "enabled": enabled})
    }

    #[test]
    fn the_list_loads_and_a_failure_leaves_an_empty_list() {
        let mut c = CustomPacks::new("70");
        assert!(c.is_hl1() && c.note_key() == "#L_CPK_NOTE_HL1");
        let list = json!([pack("oxcjae-soundpack-lite", "sounds", false, true)]);
        let reply = json!({"status": "ok", "installed": false, "packs": list});
        assert_eq!(c.loaded(Ok(reply)), Some(list));
        let packs = c.packs.as_ref().unwrap();
        assert_eq!(packs[0].title_key(), "#L_CPK_TITLE_OXCJAE_SOUNDPACK_LITE");
        assert_eq!(packs[0].kind_key(), "#L_CPK_KIND_SOUNDS");
        assert!(!c.installed && c.needs_key() == "#L_CPK_NOT_INSTALLED");

        let mut c = CustomPacks::new("218");
        assert_eq!(c.loaded(Err("x".into())), None);
        assert_eq!(
            (c.packs.as_deref(), c.error),
            (Some(&[][..]), "#L_CPK_ERR_FAILED")
        );
        assert!(c.error_pack.is_empty());
        assert_eq!(c.needs_key(), "#L_CPK_NOT_AVAILABLE_GHOSTING");
    }

    #[test]
    fn a_toggle_shows_the_disk_and_names_the_error_in_its_card() {
        let mut c = CustomPacks::new("218");
        c.loaded(Ok(
            json!({"status": "ok", "packs": [pack("m9-bayonet", "models", false, true)]}),
        ));
        c.playing = "x/y/pack".into();
        assert!(c.begin_toggle("m9-bayonet"));
        assert!(c.playing.is_empty(), "переключение глушит звук");
        assert!(
            !c.begin_toggle("signed-smg"),
            "второе — пока идёт первое — нет"
        );
        let reply = json!({"status": "error", "code": "GAME_RUNNING",
            "packs": [pack("m9-bayonet", "models", false, true)]});
        assert!(c.toggled(Ok(reply)).is_some());
        assert_eq!(
            (c.error, c.error_pack.as_str()),
            ("#L_CPK_ERR_RUNNING", "m9-bayonet")
        );
        assert!(c.switching.is_empty());
        assert!(c.begin_toggle("m9-bayonet"));
        assert_eq!(c.error, "");
        let on = json!({"status": "ok", "packs": [pack("m9-bayonet", "models", true, true)]});
        c.toggled(Ok(on));
        assert!(c.packs.as_ref().unwrap()[0].enabled);
        assert!(c.begin_toggle("m9-bayonet"));
        assert_eq!(
            c.toggled(Ok(json!({"status": "error", "code": "EPERM"}))),
            None
        );
        assert_eq!(c.error, "#L_CPK_ERR_FAILED");
        assert_eq!(error_key("BUSY"), "#L_CPK_ERR_BUSY");
    }

    #[test]
    fn the_same_button_stops_and_a_late_sound_is_dropped() {
        let mut c = CustomPacks::new("70");
        let id = c.play("p", "sound/x.wav", Which::Pack).unwrap();
        assert!(c.is_playing("p", "sound/x.wav", Which::Pack));
        // «Оригинал» того же файла сменил звук: ответ «мода» уже не нужен.
        let other = c.play("p", "sound/x.wav", Which::Original).unwrap();
        let ok = json!({"status": "ok", "dataUrl": "data:audio/wav;base64,UklGRg=="});
        assert_eq!(c.sound_reply(&id, "p", Ok(ok.clone())), None);
        assert_eq!(c.sound_reply(&other, "p", Ok(ok)), Some(b"RIFF".to_vec()));
        c.ended(&id);
        assert!(c.is_playing("p", "sound/x.wav", Which::Original));
        assert_eq!(c.play("p", "sound/x.wav", Which::Original), None);
        assert!(c.playing.is_empty());
        let id = c.play("p", "sound/y.wav", Which::Pack).unwrap();
        assert_eq!(
            c.sound_reply(&id, "p", Ok(json!({"status": "error"}))),
            None
        );
        assert_eq!((c.error, c.error_pack.as_str()), ("#L_CPK_ERR_SOUND", "p"));
        assert!(c.playing.is_empty());
    }

    #[test]
    fn shots_follow_the_049_table() {
        assert_eq!(shots("condensed-hud")[1].focus, (0., 1.));
        assert_eq!(shots("m9-bayonet")[0].src, "m9-bayonet.jpg");
        assert!(shots("oxcjae-soundpack-lite").is_empty());
        assert_eq!(kind_icon("hud"), "fa-gauge-high");
    }
}
