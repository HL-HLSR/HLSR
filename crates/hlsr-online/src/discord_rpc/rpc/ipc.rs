//! `discord-rpc/src/transports/ipc.js`: адреса IPC Discord, кадры и подключение.
//!
//! * Адрес: Windows — именованный канал `\\?\pipe\discord-ipc-<n>`, Unix — сокет
//!   `${XDG_RUNTIME_DIR || TMPDIR || TMP || TEMP || /tmp}/discord-ipc-<n>` (одна косая в конце
//!   каталога срезается); `n` — 0…10, берётся первый, к которому подключились (Flatpak и Snap
//!   0.4.7 не видит, и здесь тоже).
//! * Кадр: `int32 LE op`, `int32 LE длина`, JSON в UTF-8. `op`: 0 рукопожатие, 1 кадр, 2
//!   закрытие, 3 ping (в ответ pong с тем же телом), 4 pong.
//!
//! Отступление от 0.4.7: разбор кадров настоящий потоковый. Модуль Node разбирает один кадр
//! на одно чтение сокета и теряет второй кадр, пришедший в том же чтении; здесь кадры
//! собираются из любого дробления потока.

use hlsr_core::json_text::stringify;
use serde_json::Value;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};

/// `OPCodes`.
pub mod op {
    pub const HANDSHAKE: i32 = 0;
    pub const FRAME: i32 = 1;
    pub const CLOSE: i32 = 2;
    pub const PING: i32 = 3;
    pub const PONG: i32 = 4;
}

/// Самый большой адрес IPC: `getIPC` пробует 0…10.
pub const LAST_ID: u32 = 10;

/// `RPC_CONNECTION_TIMEOUT` клиента: 10 с на подключение и `READY`.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Выше этого размера тела кадр считается мусором, а соединение — разорванным. Discord шлёт
/// кадры до 64 КиБ; 0.4.7 предела не знает, но и кадра в 16 МиБ от Discord не бывает.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// `encode(op, data)`: заголовок и тело `JSON.stringify(data)`.
pub fn encode(op: i32, data: &Value) -> Vec<u8> {
    let body = stringify(data, "");
    let mut packet = Vec::with_capacity(8 + body.len());
    packet.extend_from_slice(&op.to_le_bytes());
    packet.extend_from_slice(&(body.len() as i32).to_le_bytes());
    packet.extend_from_slice(body.as_bytes());
    packet
}

/// Кадр из потока: `op` и тело (сырые байты JSON).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub op: i32,
    pub body: Vec<u8>,
}

/// Кадр нельзя разобрать: отрицательная или чрезмерная длина.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadFrame;

/// Сборщик кадров из потока байт.
#[derive(Debug, Default)]
pub struct Decoder {
    buffer: Vec<u8>,
}

impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Следующий целый кадр; `Ok(None)` — данных не хватает.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, BadFrame> {
        if self.buffer.len() < 8 {
            return Ok(None);
        }
        let op = i32::from_le_bytes([
            self.buffer[0],
            self.buffer[1],
            self.buffer[2],
            self.buffer[3],
        ]);
        let length = i32::from_le_bytes([
            self.buffer[4],
            self.buffer[5],
            self.buffer[6],
            self.buffer[7],
        ]);
        let length = usize::try_from(length).map_err(|_| BadFrame)?;
        if length > MAX_FRAME_BYTES {
            return Err(BadFrame);
        }
        if self.buffer.len() < 8 + length {
            return Ok(None);
        }
        let body = self.buffer[8..8 + length].to_vec();
        self.buffer.drain(..8 + length);
        Ok(Some(Frame { op, body }))
    }
}

/// ОС, чей адрес строится.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Unix,
}

impl Platform {
    pub const CURRENT: Platform = if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::Unix
    };
}

/// `getIPCPath(id)`: `env` — окружение процесса (имя → значение).
pub fn ipc_path(platform: Platform, env: &dyn Fn(&str) -> Option<String>, id: u32) -> String {
    match platform {
        Platform::Windows => format!("\\\\?\\pipe\\discord-ipc-{id}"),
        Platform::Unix => {
            // `XDG_RUNTIME_DIR || TMPDIR || TMP || TEMP || '/tmp'`: пустая строка ложна.
            let prefix = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
                .iter()
                .filter_map(|name| env(name))
                .find(|value| !value.is_empty())
                .unwrap_or_else(|| "/tmp".to_owned());
            let prefix = prefix.strip_suffix('/').unwrap_or(&prefix);
            format!("{prefix}/discord-ipc-{id}")
        }
    }
}

/// Адреса и тайм-аут подключения. Настоящие — [`IpcConfig::system`]; тесты подставляют свои
/// адреса, чтобы не коснуться настоящего Discord.
#[derive(Clone)]
pub struct IpcConfig {
    /// Адрес `discord-ipc-<id>`.
    pub path: Arc<dyn Fn(u32) -> String + Send + Sync>,
    /// `RPC_CONNECTION_TIMEOUT`.
    pub connect_timeout: Duration,
}

impl IpcConfig {
    pub fn system() -> IpcConfig {
        IpcConfig {
            path: Arc::new(|id| ipc_path(Platform::CURRENT, &|name| std::env::var(name).ok(), id)),
            connect_timeout: CONNECT_TIMEOUT,
        }
    }
}

impl std::fmt::Debug for IpcConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IpcConfig")
            .field("connect_timeout", &self.connect_timeout)
            .finish_non_exhaustive()
    }
}

/// Канал или сокет, по которому идут кадры.
pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

pub type IpcStream = Box<dyn Stream>;

/// Ожидание занятого канала Windows (`ERROR_PIPE_BUSY`): libuv ждёт его до 30 с.
#[cfg(windows)]
const PIPE_BUSY_WAIT: Duration = Duration::from_secs(30);

#[cfg(windows)]
async fn open(path: &str) -> io::Result<IpcStream> {
    use tokio::net::windows::named_pipe::ClientOptions;
    const ERROR_PIPE_BUSY: i32 = 231;
    let started = std::time::Instant::now();
    loop {
        match ClientOptions::new().open(path) {
            Ok(client) => return Ok(Box::new(client)),
            Err(error)
                if error.raw_os_error() == Some(ERROR_PIPE_BUSY)
                    && started.elapsed() < PIPE_BUSY_WAIT =>
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(unix)]
async fn open(path: &str) -> io::Result<IpcStream> {
    Ok(Box::new(tokio::net::UnixStream::connect(path).await?))
}

#[cfg(not(any(windows, unix)))]
async fn open(_path: &str) -> io::Result<IpcStream> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

/// `getIPC()`: первый отозвавшийся из `discord-ipc-0…10`; ни один — `None`
/// (`Error('Could not connect')`).
pub async fn get_ipc(config: &IpcConfig) -> Option<IpcStream> {
    for id in 0..=LAST_ID {
        if let Ok(stream) = open(&(config.path)(id)).await {
            return Some(stream);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn encode_header_and_body() {
        let packet = encode(op::HANDSHAKE, &json!({"v": 1, "client_id": "15"}));
        assert_eq!(&packet[..4], &[0, 0, 0, 0]);
        let body = br#"{"v":1,"client_id":"15"}"#;
        assert_eq!(&packet[4..8], &(body.len() as i32).to_le_bytes());
        assert_eq!(&packet[8..], body);
    }

    #[test]
    fn decoder_assembles_frames_from_any_split() {
        let mut stream = encode(op::FRAME, &json!({"cmd": "A"}));
        stream.extend(encode(op::PING, &json!("x")));
        stream.extend(encode(op::CLOSE, &json!({})));
        for step in [1, 2, 3, 7, 8, 9, stream.len()] {
            let mut decoder = Decoder::default();
            let mut frames = Vec::new();
            for chunk in stream.chunks(step) {
                decoder.push(chunk);
                while let Some(frame) = decoder.next_frame().unwrap() {
                    frames.push(frame);
                }
            }
            let ops: Vec<i32> = frames.iter().map(|frame| frame.op).collect();
            assert_eq!(ops, [op::FRAME, op::PING, op::CLOSE], "по {step} байт");
            assert_eq!(frames[0].body, br#"{"cmd":"A"}"#);
            assert_eq!(frames[1].body, br#""x""#);
        }
    }

    #[test]
    fn decoder_rejects_negative_and_huge_lengths() {
        let mut decoder = Decoder::default();
        decoder.push(&[1, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]);
        assert_eq!(decoder.next_frame(), Err(BadFrame));
        let mut decoder = Decoder::default();
        decoder.push(&1i32.to_le_bytes());
        decoder.push(&((MAX_FRAME_BYTES + 1) as i32).to_le_bytes());
        assert_eq!(decoder.next_frame(), Err(BadFrame));
    }

    #[test]
    fn paths_follow_environment_order() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_owned())
            }
        };
        let path = |pairs, id| ipc_path(Platform::Unix, &env(pairs), id);
        assert_eq!(path(&[], 0), "/tmp/discord-ipc-0");
        assert_eq!(
            path(&[("XDG_RUNTIME_DIR", "/run/user/1000")], 3),
            "/run/user/1000/discord-ipc-3"
        );
        assert_eq!(
            path(&[("XDG_RUNTIME_DIR", "/run/user/1000/")], 1),
            "/run/user/1000/discord-ipc-1"
        );
        assert_eq!(path(&[("XDG_RUNTIME_DIR", "/a//")], 1), "/a//discord-ipc-1");
        assert_eq!(
            path(&[("XDG_RUNTIME_DIR", ""), ("TMPDIR", "/var/tmp")], 2),
            "/var/tmp/discord-ipc-2"
        );
        assert_eq!(
            path(&[("TMP", "/x"), ("TEMP", "/y")], 2),
            "/x/discord-ipc-2"
        );
        assert_eq!(path(&[("TEMP", "/y/")], 10), "/y/discord-ipc-10");
        assert_eq!(path(&[("XDG_RUNTIME_DIR", "/")], 0), "/discord-ipc-0");
        assert_eq!(
            ipc_path(Platform::Windows, &env(&[("TMPDIR", "/x")]), 7),
            "\\\\?\\pipe\\discord-ipc-7"
        );
    }
}
