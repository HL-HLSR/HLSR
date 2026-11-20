//! Мост статуса игры для Discord под Proton (`linux/PresenceRelay.js`). `client.dll` Ghosting
//! шлёт карту, FPS и таймер забега в именованный канал из `HLSR_PRESENCE_PIPE`, а под Wine такой
//! канал живёт внутри wineserver и с хоста не виден. Поэтому в префикс игры запускается
//! `hlsr-presence-relay.exe`: он создаёт канал и пересылает байты на `127.0.0.1:<порт>`, где их
//! принимает мост лаунчера. Токен помощнику не нужен: рукопожатие проверяет лаунчер.

use crate::linux::prefix_command::ProtonError;
use crate::linux::python_runtime::io_error;
use crate::livesplit_overlay::{WaitOptions, wait_for_status};
use crate::spawn::Env;
use hlsr_core::js;
use hlsr_core::paths::Resources;
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

/// `RESOURCE_DIRECTORY`.
pub const RESOURCE_DIRECTORY: &str = "presence-relay";
/// `EXECUTABLE`.
pub const EXECUTABLE: &str = "hlsr-presence-relay.exe";

// `/^\\\\\.\\pipe\\hlsr-presence-[0-9a-f]{8,64}$/`.
static PIPE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\\\\\.\\pipe\\hlsr-presence-[0-9a-f]{8,64}$").expect("шаблон канала")
});

/// Куда слушать и что отдать игре (`targetOf`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresenceTarget {
    pub pipe: String,
    pub port: u16,
    /// Канал и токен для игры (порт ей не нужен).
    pub game_env: Env,
}

impl PresenceTarget {
    pub fn to_json(&self) -> Value {
        json!({
            "pipe": self.pipe,
            "port": self.port,
            "gameEnv": {
                "HLSR_PRESENCE_PIPE": self.game_env.get("HLSR_PRESENCE_PIPE"),
                "HLSR_PRESENCE_TOKEN": self.game_env.get("HLSR_PRESENCE_TOKEN"),
            },
        })
    }
}

/// `targetOf(environment)`: окружение моста лаунчера → цель помощника; `None` — мост на хосте не
/// поднят (канал, порт или токен не те).
pub fn target_of(environment: Option<&Env>) -> Option<PresenceTarget> {
    let empty = Env::new();
    let env = environment.unwrap_or(&empty);
    let pipe = env
        .get("HLSR_PRESENCE_PIPE")
        .map(String::as_str)
        .unwrap_or("");
    let port = js::to_number(
        env.get("HLSR_PRESENCE_PORT")
            .map(String::as_str)
            .unwrap_or("NaN"),
    );
    let token = env
        .get("HLSR_PRESENCE_TOKEN")
        .filter(|token| !token.is_empty())?;
    if !PIPE.is_match(pipe) || port.fract() != 0.0 || port <= 0.0 || port > 65535.0 {
        return None;
    }
    let mut game_env = Env::new();
    game_env.insert("HLSR_PRESENCE_PIPE".to_owned(), pipe.to_owned());
    game_env.insert("HLSR_PRESENCE_TOKEN".to_owned(), token.clone());
    Some(PresenceTarget {
        pipe: pipe.to_owned(),
        port: port as u16,
        game_env,
    })
}

/// `relayArguments(target, processName, statusPath)`.
pub fn relay_arguments(
    target: &PresenceTarget,
    process_name: &str,
    status_path: &str,
) -> Vec<String> {
    [
        "--pipe",
        &target.pipe,
        "--port",
        &target.port.to_string(),
        "--name",
        process_name,
        "--status",
        status_path,
    ]
    .iter()
    .map(|arg| (*arg).to_owned())
    .collect()
}

/// Папка помощника (`prepare()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedRelay {
    pub dir: PathBuf,
    pub executable: PathBuf,
    pub status: PathBuf,
}

/// `PresenceRelay`.
#[derive(Debug, Clone)]
pub struct PresenceRelay {
    base_dir: Option<PathBuf>,
    resources_path: PathBuf,
}

fn sha256_of(file: &Path) -> std::io::Result<String> {
    let bytes = js::fs::read_file(file)?;
    Ok(hlsr_delivery::file_ops::hex(&Sha256::digest(bytes)))
}

impl PresenceRelay {
    /// `baseDir` — папка данных лаунчера; `resourcesPath` по умолчанию — ресурсы лаунчера.
    pub fn new(base_dir: Option<PathBuf>, resources_path: Option<PathBuf>) -> PresenceRelay {
        PresenceRelay {
            base_dir,
            resources_path: resources_path
                .unwrap_or_else(|| Resources::detect().dir().to_path_buf()),
        }
    }

    /// `prepare()`: копия помощника в папке данных (ресурсы AppImage смонтированы во временную
    /// папку, которая живёт, только пока работает лаунчер); прежний файл статуса убирается.
    pub fn prepare(&self) -> Result<PreparedRelay, ProtonError> {
        let source = self
            .resources_path
            .join(RESOURCE_DIRECTORY)
            .join(EXECUTABLE);
        let Some(base_dir) = self
            .base_dir
            .as_ref()
            .filter(|dir| !dir.as_os_str().is_empty())
        else {
            return Err(ProtonError::new("папка данных лаунчера не задана"));
        };
        if !source.exists() {
            return Err(ProtonError::new(format!(
                "{EXECUTABLE} не найден в ресурсах лаунчера"
            )));
        }
        let dir = base_dir.join(RESOURCE_DIRECTORY);
        std::fs::create_dir_all(&dir).map_err(io_error)?;
        let executable = dir.join(EXECUTABLE);
        // Копии ещё нет — её хеш не совпадёт.
        let current = sha256_of(&executable).ok();
        let wanted = sha256_of(&source).map_err(io_error)?;
        if current.as_deref() != Some(wanted.as_str()) {
            let temporary = PathBuf::from(format!(
                "{}.{}.tmp",
                executable.to_string_lossy(),
                std::process::id()
            ));
            std::fs::copy(&source, &temporary).map_err(io_error)?;
            std::fs::rename(&temporary, &executable).map_err(io_error)?;
        }
        let status = dir.join("relay.status");
        match std::fs::remove_file(&status) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(io_error(error));
            }
            _ => {}
        }
        Ok(PreparedRelay {
            dir,
            executable,
            status,
        })
    }

    /// `waitForStatus(file, timeoutMs, options)`: код из файла статуса помощника.
    pub async fn wait_for_status(
        &self,
        file: &Path,
        timeout: Duration,
        options: &WaitOptions<'_>,
    ) -> Option<u64> {
        wait_for_status(file, timeout, options).await
    }
}
