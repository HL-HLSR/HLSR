//! `game-presence/endpoint-store.js`: канал связи с игрой (имя канала, токен, порт на Linux) живёт
//! в окружении уже запущенной игры. Если лаунчер закрыли посреди игры, после перезапуска он
//! поднимает тот же канал, иначе игра и мост под Proton стучатся в старый адрес. Файл
//! `presence-endpoint.json` хранится, пока игра идёт; при выходе без запущенных игр он удаляется,
//! и токен меняется.

use hlsr_core::js;
use hlsr_core::json_text::{json_parse, stringify};
use hlsr_delivery::atomic::write_file_atomic;
use hlsr_delivery::errors::RawError;
use serde_json::{Map, Value};
use std::path::Path;

/// Имя файла в папке данных.
pub const FILE_NAME: &str = "presence-endpoint.json";

/// Начало имени канала Windows: `\\.\pipe\hlsr-presence-`.
pub const PIPE_PREFIX: &str = "\\\\.\\pipe\\hlsr-presence-";

/// Канал связи: имя канала Windows, токен (64 шестнадцатеричных знака) и порт на Linux
/// (`None` — порта нет: Windows или сервер не слушает).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub pipe_name: String,
    pub token: String,
    pub port: Option<u16>,
}

impl Endpoint {
    /// Объект JS `{pipeName, token, port}` в этом порядке ключей.
    pub fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert("pipeName".into(), self.pipe_name.clone().into());
        object.insert("token".into(), self.token.clone().into());
        object.insert("port".into(), self.port.map_or(Value::Null, Value::from));
        Value::Object(object)
    }
}

fn is_lower_hex(text: &str, length: usize) -> bool {
    text.len() == length
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `PIPE_RE`: `^\\\\.\\pipe\\hlsr-presence-[0-9a-f]{16}$`.
pub fn is_pipe_name(text: &str) -> bool {
    text.strip_prefix(PIPE_PREFIX)
        .is_some_and(|rest| is_lower_hex(rest, 16))
}

/// `TOKEN_RE`: `^[0-9a-f]{64}$`.
pub fn is_token(text: &str) -> bool {
    is_lower_hex(text, 64)
}

/// `normalize(value)`: канал, если имя и токен верны; порт — целое от 1 до 65535, иначе `None`.
pub fn normalize(value: &Value) -> Option<Endpoint> {
    let object = value.as_object()?;
    // `String(value.pipeName || "")`: годится только строка.
    let pipe_name = object.get("pipeName").and_then(Value::as_str).unwrap_or("");
    let token = object.get("token").and_then(Value::as_str).unwrap_or("");
    if !is_pipe_name(pipe_name) || !is_token(token) {
        return None;
    }
    let port = js::value::to_number(object.get("port"));
    let port = (port.fract() == 0.0 && port > 0.0 && port <= 65535.0).then_some(port as u16);
    Some(Endpoint {
        pipe_name: pipe_name.to_owned(),
        token: token.to_owned(),
        port,
    })
}

/// `load(file)`: прочитанный канал или `None` (нет файла, не JSON, не тот вид).
pub fn load(file: &Path) -> Option<Endpoint> {
    let bytes = std::fs::read(file).ok()?;
    let value = json_parse(&String::from_utf8_lossy(&bytes)).ok()?;
    normalize(&value)
}

/// `save(file, endpoint)`: записать канал (`JSON.stringify` без отступов, атомарно, права 0600
/// на Unix). `Ok(false)` — значение не канал, файл не тронут; ошибка — сбой записи.
pub fn save(file: &Path, endpoint: &Value) -> Result<bool, RawError> {
    let Some(endpoint) = normalize(endpoint) else {
        return Ok(false);
    };
    write_file_atomic(file, stringify(&endpoint.to_value(), "").as_bytes())?;
    restrict(file);
    Ok(true)
}

/// То же для готового [`Endpoint`] (канал моста).
pub fn save_endpoint(file: &Path, endpoint: &Endpoint) -> Result<bool, RawError> {
    save(file, &endpoint.to_value())
}

/// `chmod 0600`; на Windows права задаёт профиль пользователя.
fn restrict(file: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = file;
}

/// `clear(file)`: удалить файл; нет файла или не удалось — не ошибка.
pub fn clear(file: &Path) {
    let _ = std::fs::remove_file(file);
}
