//! Коды внешнего вида (`personalization-code.js` и `src/shared/personalization-code.js`): строку
//! `HLSR-P1.<игра>.<сжатый снимок>.<сумма>` можно отправить другому игроку, и его лаунчер применит
//! цвета, расположение, прицел, паки и поле зрения — но не запуск, бинды и тему.
//!
//! Снимок — JSON без пробелов (`deflateRaw` уровня 9, base64url), сумма — первые 16 знаков hex
//! `sha256("HLSR-P1.<игра>." + сжатые байты)`. Импорт ничего не записывает: окно показывает
//! вид, а сохраняет его обычным путём. Даже код с верной суммой проходит ту же проверку, что
//! настройки, и отклоняется, если хоть что-то в нём пришлось бы подменить или отбросить.
//!
//! Байты сжатия у zlib Node и у Rust различаются, но чтение совместимо: код Node разбирается,
//! а код этого лаунчера разбирает Node.
//!
//! 0.4.9: моды игры (`custom_packs`) едут после JSON пробельными символами ([`hide`]): 0.4.8
//! отклоняет код с любым незнакомым полем, а пробелы после JSON его `JSON.parse` пропускает.

use crate::custom_packs;
use crate::ghosting_personalization::{Personalization, sanitize_personalization};
use crate::ghosting_settings;
use crate::goldsrc_settings::{self, Flat};
use crate::js_value::{number, prop};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use flate2::write::DeflateEncoder;
use flate2::{Compression, Decompress, FlushDecompress, Status};
use hlsr_core::js;
use hlsr_core::json_text::{json_parse, stringify};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::io::Write as _;

/// `PACKS_GAME`: игра кода → игра каталога модов.
fn packs_game(id: &str) -> Option<&'static str> {
    match id {
        "218" => Some("ghosting-renewed"),
        "70" => Some("hl1-won"),
        _ => None,
    }
}

/// Байт — четыре символа по 2 бита (старшие первыми).
const SPACES: [char; 4] = [' ', '\t', '\n', '\r'];

/// `hide(value)`: JSON значения пробельными символами.
pub fn hide(value: &Value) -> String {
    stringify(value, "")
        .bytes()
        .flat_map(|byte| [6, 4, 2, 0].map(|shift| SPACES[usize::from((byte >> shift) & 3)]))
        .collect()
}

/// `reveal(text)`: обратное [`hide`]; объект или `INVALID`.
fn reveal(text: &str) -> Result<Value, CodeError> {
    let digits: Vec<u8> = text
        .chars()
        .map(|c| SPACES.iter().position(|space| *space == c).map(|i| i as u8))
        .collect::<Option<_>>()
        .ok_or(CodeError::Invalid)?;
    if !digits.len().is_multiple_of(4) {
        return Err(CodeError::Invalid);
    }
    let bytes: Vec<u8> = digits
        .chunks(4)
        .map(|four| four.iter().fold(0u8, |byte, digit| (byte << 2) | digit))
        .collect();
    let value = json_parse(&js::utf8_string(&bytes)).map_err(|_| CodeError::Invalid)?;
    if !value.is_object() {
        return Err(CodeError::Invalid);
    }
    Ok(value)
}

/// `cleanMods(id, value)`: моды этой игры (id → включён), незнакомые id пропускаются.
fn clean_mods(id: &str, value: Option<&Value>) -> Option<Map<String, Value>> {
    let value = value?.as_object()?;
    let game = packs_game(id);
    let mods: Map<String, Value> = custom_packs::PACKS
        .iter()
        .filter(|pack| Some(pack.game) == game)
        .filter_map(|pack| match value.get(pack.id) {
            Some(Value::Bool(enabled)) => Some((pack.id.to_owned(), Value::Bool(*enabled))),
            _ => None,
        })
        .collect();
    (!mods.is_empty()).then_some(mods)
}

// ---- src/shared/personalization-code.js: версия и пределы одинаковы в форме и в обработчиках main.

pub const PREFIX: &str = "HLSR-P1";
pub const MAX_CODE_LENGTH: usize = 32768;
pub const MAX_PAYLOAD_BYTES: usize = 32768;
/// Внешний идентификатор игры (лаунчера или `gameId`) → идентификатор в коде.
pub const GAME_IDS: [(&str, &str); 4] = [
    ("218", "218"),
    ("ghosting-renewed", "218"),
    ("70", "70"),
    ("hl1-won", "70"),
];

/// Размер куска, которым Node читает вывод `zlib` (`Z_DEFAULT_CHUNK`).
const NODE_CHUNK: usize = 16 * 1024;

/// Отказ кодов: `PersonalizationCodeError` с полем `code`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CodeError {
    #[error("PERSONALIZATION_CODE_GAME_UNSUPPORTED")]
    GameUnsupported,
    #[error("PERSONALIZATION_CODE_INVALID")]
    Invalid,
    #[error("PERSONALIZATION_CODE_TOO_LARGE")]
    TooLarge,
    #[error("PERSONALIZATION_CODE_VERSION_UNSUPPORTED")]
    VersionUnsupported,
    #[error("PERSONALIZATION_CODE_WRONG_GAME")]
    WrongGame,
}

impl CodeError {
    /// Код ошибки, который видит окно.
    pub fn code(self) -> &'static str {
        match self {
            CodeError::GameUnsupported => "PERSONALIZATION_CODE_GAME_UNSUPPORTED",
            CodeError::Invalid => "PERSONALIZATION_CODE_INVALID",
            CodeError::TooLarge => "PERSONALIZATION_CODE_TOO_LARGE",
            CodeError::VersionUnsupported => "PERSONALIZATION_CODE_VERSION_UNSUPPORTED",
            CodeError::WrongGame => "PERSONALIZATION_CODE_WRONG_GAME",
        }
    }
}

/// Снимок внешнего вида: `{ personalization }` у Ghosting, `{ personalization, packs, bxt }` у HL1.
#[derive(Debug, Clone)]
pub struct Look {
    pub personalization: Personalization,
    /// Паки и поля BXT (только HL1).
    pub hl1: Option<Flat>,
    /// Моды из кода (id → включён); только у [`decode`].
    pub mods: Option<Map<String, Value>>,
}

impl Look {
    /// Объект снимка в порядке ключей 0.4.7 (`mods` 0.4.9 — последним).
    pub fn to_value(&self) -> Value {
        let mut object = serde_json::Map::new();
        object.insert("personalization".into(), self.personalization.to_value());
        if let Some(flat) = &self.hl1 {
            object.insert("packs".into(), flat.packs.to_value());
            object.insert("bxt".into(), flat.bxt.to_value());
        }
        if let Some(mods) = &self.mods {
            object.insert("mods".into(), Value::Object(mods.clone()));
        }
        Value::Object(object)
    }
}

/// `gameIdOf(id)`: идентификатор игры в коде.
pub fn game_id_of(id: &str) -> Result<&'static str, CodeError> {
    GAME_IDS
        .iter()
        .find(|(known, _)| *known == id)
        .map(|(_, game)| *game)
        .ok_or(CodeError::GameUnsupported)
}

/// `snapshotLook(id, input)`: снимок внешнего вида, а не всех настроек игры или профиля
/// лаунчера. Значения по умолчанию включены явно: импорт должен убрать и те элементы HUD
/// получателя, которые у автора кода были выключены. `id` — идентификатор из [`game_id_of`].
pub fn snapshot_look(id: &str, input: &Value) -> Look {
    let source = if input.is_object() {
        input
    } else {
        &Value::Null
    };
    let null = &Value::Null;
    let mut personalization =
        sanitize_personalization(prop(source, "personalization").unwrap_or(null));
    if id == "218" {
        let mut extra = ghosting_settings::defaults();
        extra.assign(&personalization.extra);
        personalization.extra = extra;
        Look {
            personalization,
            hl1: None,
            mods: None,
        }
    } else {
        personalization.extra = ghosting_settings::Extras::new();
        let flat = goldsrc_settings::to_flat(
            prop(source, "packs").unwrap_or(null),
            prop(source, "bxt").unwrap_or(null),
        );
        Look {
            personalization,
            hl1: Some(goldsrc_settings::from_flat(&flat.to_value())),
            mods: None,
        }
    }
}

/// Строгое равенство примитивов JS (`raw === clean`).
fn strictly_equal(raw: &Value, clean: &Value) -> bool {
    match (raw, clean) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Number(_), Value::Number(_)) => number(raw) == number(clean),
        (Value::String(a), Value::String(b)) => a == b,
        _ => false,
    }
}

/// `matchesClean(raw, clean)`: порядок ключей не важен, но каждое переданное значение должно
/// пройти обычную проверку настроек без подмены или отбрасывания. Новые поля с умолчаниями не
/// мешают читать код предыдущей версии редактора. Массивы никогда не совпадают.
fn matches_clean(raw: &Value, clean: &Value) -> bool {
    match raw {
        Value::Object(raw) => match clean {
            Value::Object(clean) => raw.iter().all(|(key, value)| {
                clean
                    .get(key)
                    .is_some_and(|expected| matches_clean(value, expected))
            }),
            _ => false,
        },
        _ => strictly_equal(raw, clean),
    }
}

fn checksum(id: &str, bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(format!("{PREFIX}.{id}.").as_bytes());
    hash.update(bytes);
    hash.finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Полезная нагрузка кода до сжатия: `JSON.stringify(snapshotLook(id, input))` в UTF-8 и моды
/// `input.mods` после него ([`hide`]).
pub fn payload(id: &str, input: &Value) -> Result<Vec<u8>, CodeError> {
    let game = game_id_of(id)?;
    let mut text = stringify(&snapshot_look(game, input).to_value(), "");
    if let Some(mods) = clean_mods(game, input.as_object().and_then(|input| input.get("mods"))) {
        let mut wrapped = Map::new();
        wrapped.insert("mods".into(), Value::Object(mods));
        text.push_str(&hide(&Value::Object(wrapped)));
    }
    Ok(text.into_bytes())
}

/// `encode(id, input)`: код внешнего вида.
pub fn encode(id: &str, input: &Value) -> Result<String, CodeError> {
    let game = game_id_of(id)?;
    let bytes = payload(id, input)?;
    if bytes.len() > MAX_PAYLOAD_BYTES {
        return Err(CodeError::TooLarge);
    }
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::new(9));
    encoder.write_all(&bytes).expect("запись в память");
    let packed = encoder.finish().expect("запись в память");
    let code = format!(
        "{PREFIX}.{game}.{}.{}",
        URL_SAFE_NO_PAD.encode(&packed),
        checksum(game, &packed)
    );
    if code.len() > MAX_CODE_LENGTH {
        return Err(CodeError::TooLarge);
    }
    Ok(code)
}

/// `/^HLSR-P\d+$/`.
fn is_version_tag(text: &str) -> bool {
    text.strip_prefix("HLSR-P")
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

/// `zlib.inflateRawSync(packed, { maxOutputLength: MAX_PAYLOAD_BYTES, info: true })` плюс проверка
/// `engine.bytesWritten === packed.length` (после конца потока данных быть не должно).
///
/// Node отдаёт вывод кусками по 16 КиБ и после каждого куска сверяет накопленное с пределом; ошибка
/// потока внутри куска бросается раньше этой сверки. Поэтому `TOO_LARGE` получает поток, вывод
/// которого закончился больше предела или дошёл до трёх кусков (49152 байта) без ошибки, а
/// оборванный или испорченный поток, не выдавший трёх кусков, — `INVALID`, даже если он уже вышел
/// за 32768. Здесь распаковка идёт одним вызовом в буфер из трёх кусков. Точное место ошибки на
/// кратных 16384 байтам zlib определяет по внутреннему состоянию; такой поток считается дошедшим
/// до следующего куска (случай бывает только у поддельного кода).
fn inflate(packed: &[u8]) -> Result<Vec<u8>, CodeError> {
    // Три куска: столько нужно, чтобы вывод вышел за предел 32768.
    let mut out = vec![0u8; 3 * NODE_CHUNK];
    let mut inflater = Decompress::new(false);
    let status = inflater.decompress(packed, &mut out, FlushDecompress::Finish);
    let produced = usize::try_from(inflater.total_out()).unwrap_or(usize::MAX);
    match status {
        Ok(Status::StreamEnd) => {
            if produced > MAX_PAYLOAD_BYTES {
                return Err(CodeError::TooLarge);
            }
            // Данные после конца потока: Node считает записанными только прочитанные байты.
            if usize::try_from(inflater.total_in()).ok() != Some(packed.len()) {
                return Err(CodeError::Invalid);
            }
            out.truncate(produced);
            Ok(out)
        }
        _ if produced >= out.len() => Err(CodeError::TooLarge),
        _ => Err(CodeError::Invalid),
    }
}

/// `decode(id, input)`: снимок из кода; `input` — то, что прислало окно (не строка — `INVALID`).
pub fn decode(id: &str, input: &Value) -> Result<Look, CodeError> {
    let game = game_id_of(id)?;
    let Value::String(text) = input else {
        return Err(CodeError::Invalid);
    };
    if js::trim(text).is_empty() {
        return Err(CodeError::Invalid);
    }
    if text.encode_utf16().count() > MAX_CODE_LENGTH {
        return Err(CodeError::TooLarge);
    }
    // Код можно вставить с переносами строк из сообщения.
    let code: String = text.chars().filter(|c| !js::is_whitespace(*c)).collect();
    let parts: Vec<&str> = code.split('.').collect();
    if parts.len() != 4 || !is_version_tag(parts[0]) {
        return Err(CodeError::Invalid);
    }
    if parts[0] != PREFIX {
        return Err(CodeError::VersionUnsupported);
    }
    if !GAME_IDS
        .iter()
        .any(|(known, game)| *known == parts[1] && *game == parts[1])
    {
        return Err(CodeError::Invalid);
    }
    if parts[1] != game {
        return Err(CodeError::WrongGame);
    }
    let body_ok = !parts[2].is_empty()
        && parts[2]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    let sum_ok = parts[3].len() == 16
        && parts[3]
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !body_ok || !sum_ok {
        return Err(CodeError::Invalid);
    }
    let packed = js::buffer_from_base64(parts[2]);
    if URL_SAFE_NO_PAD.encode(&packed) != parts[2] || checksum(game, &packed) != parts[3] {
        return Err(CodeError::Invalid);
    }

    let inflated = inflate(&packed)?;
    let text = js::utf8_string(&inflated);
    let raw = json_parse(&text).map_err(|_| CodeError::Invalid)?;
    let is_object = |value: Option<&Value>| value.is_some_and(Value::is_object);
    let personalization = prop(&raw, "personalization");
    if !raw.is_object()
        || !is_object(personalization)
        || !is_object(personalization.and_then(|value| prop(value, "extra")))
    {
        return Err(CodeError::Invalid);
    }
    let mut look = snapshot_look(game, &raw);
    if !matches_clean(&raw, &look.to_value()) {
        return Err(CodeError::Invalid);
    }
    // Чужой код не указывает на файлы этого компьютера: спрей лаунчера — только свой, по хешу.
    let extra = &mut look.personalization.extra;
    if let Some(ghosting_settings::ExtraValue::Text(spray)) = extra.get("gh_spray_file")
        && !spray.is_empty()
        && !ghosting_settings::is_managed_spray_path(spray)
    {
        extra.insert(
            "gh_spray_file",
            ghosting_settings::ExtraValue::Text(String::new()),
        );
    }
    // После JSON — только пробельные символы (их пропустил разбор): там моды.
    let after = &text[text.rfind('}').map_or(0, |at| at + 1)..];
    if !after.is_empty() {
        look.mods = clean_mods(game, reveal(after)?.get("mods"));
    }
    Ok(look)
}
