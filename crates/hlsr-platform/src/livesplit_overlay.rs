//! Оверлей LiveSplit внутри игры (`game-delivery/livesplit-overlay.js` 0.4.9; ресурсы —
//! `resources/livesplit-overlay`, исходники в `scripts/livesplit-overlay`). Лаунчер кладёт в папку
//! игры прокси: `d3d9.dll` для Source (HL2, Ghosting) и `opengl32.dll` для GoldSrc (HL1). Игра
//! грузит его сама, как обёртки вроде ReShade; прокси передаёт вызовы настоящей DLL и рисует снимок
//! окна LiveSplit поверх каждого кадра, поэтому оверлей виден и в полноэкранном режиме. В другие
//! процессы лаунчер ничего не внедряет.
//!
//! Прокси читает настройки (`overlay.ini`) и пишет журнал в папку данных игры, путь к которой лежит
//! в `hlsr-overlay.path` рядом с ним. Сохранение персонализации переписывает `overlay.ini`, и
//! запущенная игра подхватывает его на лету. Папка данных своя у каждой игры.
//!
//! Под Wine окно другого процесса не снимается, поэтому там прокси просит 64-битный помощник
//! внедрить в LiveSplit `hlsr-share64.dll`, и кадр идёт через общую память. Эти два файла есть
//! только в Linux-сборке лаунчера. Игре под Wine нужна ещё замена DLL (`WINEDLLOVERRIDES=d3d9=n,b`).
//!
//! LiveSplit, запущенный из лаунчера, открыт для одной игры (на Linux — в её префиксе Proton). В
//! других играх оверлей тогда выключен в `overlay.ini` и включается снова, когда LiveSplit закроют
//! или откроют для этой игры.
//!
//! На Windows окно LiveSplit для предпросмотра снимает сам лаунчер
//! (`livesplit_window_capture`), под Wine — помощник изнутри LiveSplit.

use crate::livesplit_overlay_shared::to_ini;
use hlsr_core::js::path::NodePath;
use hlsr_core::js::{self, Platform};
use hlsr_core::logger::{Logger, TracingLogger};
use hlsr_core::paths::Resources;
use hlsr_delivery::atomic::write_file_atomic;
use hlsr_delivery::errors::RawError;
use hlsr_delivery::livesplit_overlay::{Overlay, sanitize_overlay};
use hlsr_delivery::manifests::LaunchConfig;
use hlsr_delivery::ports::OverlayConfig;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

pub const RESOURCE_DIRECTORY: &str = "livesplit-overlay";

/// Прокси: имя в ресурсах и имя, под которым игра загрузит его из своей папки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Proxy {
    pub resource: &'static str,
    pub name: &'static str,
}

pub const PROXY_D3D9: Proxy = Proxy {
    resource: "proxy-d3d9.dll",
    name: "d3d9.dll",
};
pub const PROXY_OPENGL32: Proxy = Proxy {
    resource: "proxy-opengl32.dll",
    name: "opengl32.dll",
};
/// 64-битные, как LiveSplit: снимок окна изнутри LiveSplit под Wine.
pub const HELPER64: &str = "hlsr-overlay64.exe";
pub const SHARE_DLL: &str = "hlsr-share64.dll";
pub const FILES_WINDOWS: [&str; 2] = [PROXY_D3D9.resource, PROXY_OPENGL32.resource];
pub const FILES_LINUX: [&str; 4] = [
    PROXY_D3D9.resource,
    PROXY_OPENGL32.resource,
    HELPER64,
    SHARE_DLL,
];
/// Файлы ресурсов этой системы.
pub const FILES: &[&str] = if cfg!(windows) {
    &FILES_WINDOWS
} else {
    &FILES_LINUX
};
/// До 0.4.9 снимок делал этот exe, и его копия лежала в папке данных.
pub const LEGACY_CAPTURE: &str = "hlsr-capture.exe";
pub const SIDECAR: &str = "hlsr-overlay.path";
/// Строка внутри прокси (`scripts/livesplit-overlay/proxy.h`): по ней своя `d3d9.dll` отличается от
/// DXVK или ReShade пользователя.
pub const PROXY_MARKER: &[u8] = b"HLSR-OVERLAY-PROXY-1";
pub const CONFIG: &str = "overlay.ini";
pub const SNAPSHOT_MAGIC: &[u8; 8] = b"HLSRLS1\0";
/// Окно LiveSplit больше 2048 пикселей DLL не снимает (`MAX_SIDE` в `common.h`).
pub const MAX_SIDE: u32 = 2048;

/// Игра, в папку которой кладётся прокси (`target` — `{installPath, config}`).
#[derive(Debug, Clone, Copy)]
pub struct Target<'a> {
    pub install_path: &'a Path,
    pub executable: &'a str,
    /// `goldsrc` у HL1; у остальных — Source.
    pub engine: Option<&'a str>,
}

impl<'a> Target<'a> {
    pub fn of(install_path: &'a Path, config: &'a LaunchConfig) -> Target<'a> {
        Target {
            install_path,
            executable: config.executable,
            engine: config.engine,
        }
    }
}

/// Итог `prepare`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub dir: PathBuf,
    pub log: PathBuf,
    /// Имя прокси в папке игры: `d3d9.dll` или `opengl32.dll`.
    pub proxy: &'static str,
    /// Папки игры, где прокси встал.
    pub placed: Vec<PathBuf>,
    /// Замена для `WINEDLLOVERRIDES` под Wine.
    pub dll_override: Option<String>,
}

/// Папка снимка окна LiveSplit (`captureTool`). Linux: ещё помощник и DLL изнутри LiveSplit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureTool {
    pub dir: PathBuf,
    pub snapshot: PathBuf,
    pub status: PathBuf,
    pub injector64: Option<PathBuf>,
    pub share_dll: Option<PathBuf>,
}

/// Снимок окна LiveSplit: строки BGRX сверху вниз.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// `processNameOf(config)`: имя процесса игры: `hl2.exe`, `hl.exe`.
pub fn process_name_of(executable: &str) -> String {
    NodePath::new(Platform::Win32, "")
        .basename(executable)
        .to_lowercase()
}

/// `parseSnapshot(buffer)`: `HLSRLS1\0`, ширина, высота (`uint32` LE), затем строки BGRX сверху
/// вниз; не тот размер — `None`.
pub fn parse_snapshot(buffer: &[u8]) -> Option<Snapshot> {
    if buffer.len() < 16 || &buffer[..8] != SNAPSHOT_MAGIC {
        return None;
    }
    let width = u32::from_le_bytes(buffer[8..12].try_into().ok()?);
    let height = u32::from_le_bytes(buffer[12..16].try_into().ok()?);
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return None;
    }
    if buffer.len() as u64 != 16 + u64::from(width) * u64::from(height) * 4 {
        return None;
    }
    Some(Snapshot {
        width,
        height,
        pixels: buffer[16..].to_vec(),
    })
}

/// `mergeOverrides(existing, addition)`: значение `WINEDLLOVERRIDES` с добавленной заменой; уже
/// заданные пользователем замены остаются. Для того же имени (`d3d9=b` пользователя) действует
/// наше правило, дописанное последним: Wine (`add_load_order` в `dlls/ntdll/unix/loadorder.c`) заменяет
/// прежнее правило для имени новым.
pub fn merge_overrides(existing: Option<&str>, addition: &str) -> String {
    let current = js::trim(existing.unwrap_or(""));
    if current.is_empty() {
        addition.to_owned()
    } else {
        format!("{current};{addition}")
    }
}

/// Параметры ожидания файла статуса (`options` `waitForStatus`).
#[derive(Default)]
pub struct WaitOptions<'a> {
    /// Пауза между проверками; по умолчанию 200 мс.
    pub interval: Option<Duration>,
    /// Ожидание отменено (процесс помощника вышел).
    pub cancelled: Option<&'a (dyn Fn() -> bool + Send + Sync)>,
}

/// `waitForStatus(fs, file, timeoutMs, options)`: код из файла статуса помощника под Proton
/// (`--status`); `None` — не дождались. Сессия `umu` не завершается, пока в префиксе работает
/// LiveSplit или игра, так что выход процесса ждать бесполезно. Число за пределами `u64` —
/// `u64::MAX` (код не 0 и не 2 в любом случае).
pub async fn wait_for_status(
    file: &Path,
    timeout: Duration,
    options: &WaitOptions<'_>,
) -> Option<u64> {
    let deadline = Instant::now() + timeout;
    let interval = options.interval.unwrap_or(Duration::from_millis(200));
    loop {
        if options.cancelled.is_some_and(|cancelled| cancelled()) {
            return None;
        }
        // Файла ещё нет — ждём.
        if let Ok(bytes) = std::fs::read(file) {
            let text = String::from_utf8_lossy(&bytes);
            let text = js::trim(&text);
            if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) {
                return Some(text.parse().unwrap_or(u64::MAX));
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(interval).await;
    }
}

fn sha256_of(file: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(file)?;
    Ok(hlsr_delivery::file_ops::hex(&Sha256::digest(bytes)))
}

/// Параметры конструктора (`options`).
#[derive(Default)]
pub struct OverlayOptions {
    /// Папка данных лаунчера: оверлей кладёт файлы в `<baseDir>/livesplit-overlay`.
    pub base_dir: Option<PathBuf>,
    /// `resourcesPath`; по умолчанию — каталог ресурсов лаунчера.
    pub resources_path: Option<PathBuf>,
    pub logger: Option<Arc<dyn Logger>>,
    /// Система, под которую готовятся файлы; по умолчанию — текущая.
    pub platform: Option<Platform>,
}

/// `LiveSplitOverlay`.
pub struct LiveSplitOverlay {
    base_dir: Option<PathBuf>,
    resources_path: PathBuf,
    logger: Arc<dyn Logger>,
    platform: Platform,
    /// Для какой игры запущен LiveSplit из лаунчера; `None` — не запущен.
    bound_game_id: Mutex<Option<String>>,
}

impl LiveSplitOverlay {
    pub fn new(options: OverlayOptions) -> LiveSplitOverlay {
        LiveSplitOverlay {
            base_dir: options.base_dir,
            resources_path: options
                .resources_path
                .unwrap_or_else(|| Resources::detect().dir().to_path_buf()),
            logger: options.logger.unwrap_or_else(|| Arc::new(TracingLogger)),
            platform: options.platform.unwrap_or(Platform::CURRENT),
            bound_game_id: Mutex::new(None),
        }
    }

    fn bound(&self) -> Option<String> {
        self.bound_game_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn windows(&self) -> bool {
        self.platform == Platform::Win32
    }

    /// `boundGameId`: игра, для которой открыт LiveSplit.
    pub fn bound_game_id(&self) -> Option<String> {
        self.bound()
    }

    /// `files()`: файлы ресурсов для этой системы.
    pub fn files(&self) -> &'static [&'static str] {
        if self.windows() {
            &FILES_WINDOWS
        } else {
            &FILES_LINUX
        }
    }

    /// `gamePath(file)`: путь так, как его видит игра; под Wine корень Linux — диск `Z:`.
    pub fn game_path(&self, file: &Path) -> String {
        let resolved = NodePath::native().resolve(&[&file.to_string_lossy()]);
        if self.windows() {
            resolved
        } else {
            format!("Z:{}", resolved.replace('/', "\\"))
        }
    }

    /// `iniFor(gameId, overlay)`: `overlay.ini` игры; оверлей выключен, если LiveSplit открыт для
    /// другой игры.
    pub fn ini_for(&self, game_id: &str, overlay: &Overlay) -> String {
        let bound = self.bound().is_none_or(|bound| bound == game_id);
        let mut effective = overlay.clone();
        effective.enabled = overlay.enabled && bound;
        to_ini(&effective)
    }

    /// `dirFor(gameId)`: `<baseDir>/livesplit-overlay/<gameId>`; недопустимое имя — `None`.
    pub fn dir_for(&self, game_id: &str) -> Option<PathBuf> {
        let base = self
            .base_dir
            .as_ref()
            .filter(|base| !base.as_os_str().is_empty())?;
        let valid = !game_id.is_empty()
            && game_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
        valid.then(|| base.join(RESOURCE_DIRECTORY).join(game_id))
    }

    /// `resource(name)`: файл оверлея в ресурсах лаунчера.
    pub fn resource(&self, name: &str) -> PathBuf {
        self.resources_path.join(RESOURCE_DIRECTORY).join(name)
    }

    /// `available()`: все файлы оверлея этой системы на месте.
    pub fn available(&self) -> bool {
        self.files().iter().all(|name| self.resource(name).exists())
    }

    /// `proxyOf(config)`: GoldSrc рисует через OpenGL, Source — через Direct3D 9.
    pub fn proxy_of(engine: Option<&str>) -> Proxy {
        if engine == Some("goldsrc") {
            PROXY_OPENGL32
        } else {
            PROXY_D3D9
        }
    }

    /// `proxyFolders(installPath, config)`: куда игра смотрит за DLL. Source грузит
    /// `shaderapidx9.dll` из `bin\` и ищет его зависимости там, но не у всех сборок это так: прокси
    /// лежит и в `bin\`, и рядом с `hl2.exe`. GoldSrc грузит `hw.dll` из папки `hl.exe`. Папки вне
    /// игры отбрасываются.
    pub fn proxy_folders(target: &Target) -> Vec<PathBuf> {
        let path = NodePath::native();
        let root = path.resolve(&[&target.install_path.to_string_lossy()]);
        let executable = path.resolve(&[&root, target.executable]);
        let folders = if target.engine == Some("goldsrc") {
            vec![path.dirname(&executable)]
        } else {
            vec![path.join(&[&root, "bin"]), path.dirname(&executable)]
        };
        let inside = format!("{root}{}", path.sep());
        let mut out: Vec<String> = Vec::new();
        for folder in folders {
            if !out.contains(&folder) && (folder == root || folder.starts_with(&inside)) {
                out.push(folder);
            }
        }
        out.into_iter().map(PathBuf::from).collect()
    }

    /// `copyTo(source, target)`: копия файла, если её ещё нет или она устарела. Файл запущенной
    /// игры на Windows заблокирован: тогда остаётся прежняя копия.
    pub fn copy_to(&self, source: &Path, target: &Path) -> Result<PathBuf, RawError> {
        let wanted = sha256_of(source).map_err(|error| RawError::fs(&error, "open", source))?;
        let current = sha256_of(target).ok();
        if current.as_deref() == Some(wanted.as_str()) {
            return Ok(target.to_path_buf());
        }
        let temporary = PathBuf::from(format!("{}.{}.tmp", target.display(), std::process::id()));
        let copied = std::fs::copy(source, &temporary)
            .map(drop)
            .map_err(|error| RawError::fs_with_dest(&error, "copyfile", source, Some(&temporary)))
            .and_then(|()| {
                std::fs::rename(&temporary, target).map_err(|error| {
                    RawError::fs_with_dest(&error, "rename", &temporary, Some(target))
                })
            });
        if let Err(error) = copied {
            let _ = std::fs::remove_file(&temporary);
            if current.is_none() {
                return Err(error);
            }
            let name = target
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            self.logger.warn(&format!(
                "[livesplit-overlay] {name} не обновлён: {}",
                error.message
            ));
        }
        Ok(target.to_path_buf())
    }

    /// `syncFile(name, dir)`: копия файла ресурсов в папку.
    pub fn sync_file(&self, name: &str, dir: &Path) -> Result<PathBuf, RawError> {
        self.copy_to(&self.resource(name), &dir.join(name))
    }

    /// `isOwnProxy(file)`: файл по этому пути — наш прокси (а не DXVK или ReShade пользователя).
    pub fn is_own_proxy(file: &Path) -> bool {
        std::fs::read(file).is_ok_and(|bytes| {
            bytes
                .windows(PROXY_MARKER.len())
                .any(|window| window == PROXY_MARKER)
        })
    }

    /// `placeProxies(target, dataDir)`: кладёт прокси и `hlsr-overlay.path` в папки игры.
    /// Возвращает папки, где прокси встал. Чужую `d3d9.dll` или `opengl32.dll` не трогает.
    pub fn place_proxies(&self, target: &Target, data_dir: &Path) -> Vec<PathBuf> {
        let kind = Self::proxy_of(target.engine);
        let source = self.resource(kind.resource);
        let mut placed = Vec::new();
        for folder in Self::proxy_folders(target) {
            if !folder.exists() {
                continue;
            }
            let file = folder.join(kind.name);
            if file.exists() && !Self::is_own_proxy(&file) {
                self.logger.warn(&format!(
                    "[livesplit-overlay] {} не от лаунчера, оверлей в эту папку не ставится",
                    file.display()
                ));
                continue;
            }
            let put = self.copy_to(&source, &file).and_then(|_| {
                let sidecar = folder.join(SIDECAR);
                let text = format!("{}\n", self.game_path(data_dir));
                match std::fs::read(&sidecar) {
                    Ok(current) if current == text.as_bytes() => Ok(()),
                    _ => write_file_atomic(&sidecar, text.as_bytes()),
                }
            });
            match put {
                Ok(()) => placed.push(folder),
                Err(error) => self.logger.warn(&format!(
                    "[livesplit-overlay] прокси {} не положен: {}",
                    file.display(),
                    error.message
                )),
            }
        }
        placed
    }

    /// `removeProxies(target)`: убирает свой прокси и `hlsr-overlay.path` из папок игры. Чужие
    /// файлы и файл запущенной игры остаются.
    pub fn remove_proxies(&self, target: &Target) {
        let kind = Self::proxy_of(target.engine);
        for folder in Self::proxy_folders(target) {
            let file = folder.join(kind.name);
            let sidecar = folder.join(SIDECAR);
            let removed = (|| -> Result<(), RawError> {
                if file.exists() && Self::is_own_proxy(&file) {
                    remove_if_exists(&file)?;
                }
                if !file.exists() {
                    remove_if_exists(&sidecar)?;
                }
                Ok(())
            })();
            if let Err(error) = removed {
                self.logger.warn(&format!(
                    "[livesplit-overlay] {} не убран: {}",
                    file.display(),
                    error.message
                ));
            }
        }
    }

    /// `prepare(gameId, settings, target, {proxy})`: готовит запуск игры — `overlay.ini` и
    /// помощники в папке данных, прокси в папках игры. Зовётся до запуска игры: прокси
    /// подхватывается при старте. `None` — оверлей выключен (прокси убран). `proxy: true` — прокси
    /// нужен и без оверлея (Vulkan, `vulkan_backend`): он ставится, а оверлей в `overlay.ini`
    /// выключен.
    pub fn prepare(
        &self,
        game_id: &str,
        settings: &Value,
        target: Option<&Target>,
        proxy: bool,
    ) -> Result<Option<Prepared>, RawError> {
        let overlay = sanitize_overlay(overlay_of(settings));
        let Some(dir) = self.dir_for(game_id) else {
            return Ok(None);
        };
        if !overlay.enabled && !proxy {
            if let Some(target) = target {
                self.remove_proxies(target);
            }
            return Ok(None);
        }
        if !self.available() {
            return Err(RawError::new(
                "файлы оверлея LiveSplit не найдены в ресурсах лаунчера",
            ));
        }
        std::fs::create_dir_all(&dir).map_err(|error| RawError::fs(&error, "mkdir", &dir))?;
        // Под Wine прокси запускает помощник из папки данных.
        if !self.windows() {
            self.sync_file(HELPER64, &dir)?;
            self.sync_file(SHARE_DLL, &dir)?;
        }
        write_file_atomic(
            &dir.join(CONFIG),
            self.ini_for(game_id, &overlay).as_bytes(),
        )?;
        let kind = Self::proxy_of(target.and_then(|target| target.engine));
        let placed = target.map_or_else(Vec::new, |target| self.place_proxies(target, &dir));
        let dll_override = (!self.windows() && !placed.is_empty()).then(|| {
            format!(
                "{}=n,b",
                kind.name.strip_suffix(".dll").unwrap_or(kind.name)
            )
        });
        Ok(Some(Prepared {
            log: dir.join("overlay.log"),
            dir,
            proxy: kind.name,
            placed,
            dll_override,
        }))
    }

    /// `writeConfig(gameId, settings)`: настройки из окна персонализации. Если папка игры уже есть
    /// (игра запускалась с оверлеем), прокси в запущенной игре подхватит их сразу.
    pub fn write_config(&self, game_id: &str, settings: &Value) -> Result<bool, RawError> {
        let Some(dir) = self.dir_for(game_id).filter(|dir| dir.exists()) else {
            return Ok(false);
        };
        let overlay = sanitize_overlay(overlay_of(settings));
        write_file_atomic(
            &dir.join(CONFIG),
            self.ini_for(game_id, &overlay).as_bytes(),
        )?;
        Ok(true)
    }

    /// `bindTo(gameId, settingsOf)`: LiveSplit запущен для `game_id` (или закрыт — `None`):
    /// `overlay.ini` всех игр, которые уже запускались с оверлеем, переписывается. `settings_of`
    /// — настройки игры из `GameSettingsStore`. `true` — привязка сменилась.
    pub fn bind_to(
        &self,
        game_id: Option<&str>,
        settings_of: &dyn Fn(&str) -> Option<Value>,
    ) -> bool {
        let next = game_id.filter(|id| !id.is_empty()).map(str::to_owned);
        {
            let mut bound = self
                .bound_game_id
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if *bound == next {
                return false;
            }
            *bound = next;
        }
        let Some(root) = self
            .base_dir
            .as_ref()
            .filter(|base| !base.as_os_str().is_empty())
            .map(|base| base.join(RESOURCE_DIRECTORY))
        else {
            return true;
        };
        let Ok(entries) = std::fs::read_dir(&root) else {
            return true;
        };
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();
        names.sort();
        for name in names {
            if name == "capture" || self.dir_for(&name).is_none() {
                continue;
            }
            let settings = settings_of(&name).unwrap_or(Value::Null);
            if let Err(error) = self.write_config(&name, &settings) {
                self.logger.warn(&format!(
                    "[livesplit-overlay] overlay.ini {name} не переписан: {}",
                    error.message
                ));
            }
        }
        true
    }

    /// `captureTool()`: папка снимка окна LiveSplit (предпросмотр), отдельная, чтобы не зависеть от
    /// игр. Linux: ещё помощник и DLL изнутри LiveSplit; на Windows туда ничего не копируется.
    pub fn capture_tool(&self) -> Result<Option<CaptureTool>, RawError> {
        let Some(dir) = self.dir_for("capture").filter(|_| self.available()) else {
            return Ok(None);
        };
        std::fs::create_dir_all(&dir).map_err(|error| RawError::fs(&error, "mkdir", &dir))?;
        let (injector64, share_dll) = if self.windows() {
            (None, None)
        } else {
            (
                Some(self.sync_file(HELPER64, &dir)?),
                Some(self.sync_file(SHARE_DLL, &dir)?),
            )
        };
        Ok(Some(CaptureTool {
            snapshot: dir.join("snapshot.bin"),
            status: dir.join("capture.status"),
            injector64,
            share_dll,
            dir,
        }))
    }

    /// `removeLegacyCapture()`: копия `hlsr-capture.exe` от прежней версии — антивирус продолжал
    /// бы находить её в папке данных и после обновления.
    pub fn remove_legacy_capture(&self) {
        let Some(dir) = self.dir_for("capture") else {
            return;
        };
        if let Err(error) = remove_if_exists(&dir.join(LEGACY_CAPTURE)) {
            self.logger.warn(&format!(
                "[livesplit-overlay] {LEGACY_CAPTURE} не убран: {}",
                error.message
            ));
        }
    }

    /// `waitForStatus(file, timeoutMs, options)`.
    pub async fn wait_for_status(
        &self,
        file: &Path,
        timeout: Duration,
        options: &WaitOptions<'_>,
    ) -> Option<u64> {
        wait_for_status(file, timeout, options).await
    }

    /// `readSnapshot(file)`: нет файла или он не снимок — `None`.
    pub fn read_snapshot(&self, file: &Path) -> Option<Snapshot> {
        std::fs::read(file)
            .ok()
            .and_then(|bytes| parse_snapshot(&bytes))
    }
}

/// `settings && settings.liveSplitOverlay`: остальное `sanitizeOverlay` заменит стандартным.
fn overlay_of(settings: &Value) -> &Value {
    settings
        .as_object()
        .and_then(|object| object.get("liveSplitOverlay"))
        .unwrap_or(&Value::Null)
}

/// `fs.rmSync(file, {force: true})`: нет файла — не ошибка.
fn remove_if_exists(file: &Path) -> Result<(), RawError> {
    match std::fs::remove_file(file) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(RawError::fs(&error, "rm", file))
        }
        _ => Ok(()),
    }
}

impl OverlayConfig for LiveSplitOverlay {
    fn write_config(&self, game_id: &str, settings: &Value) -> Result<bool, String> {
        LiveSplitOverlay::write_config(self, game_id, settings).map_err(|error| error.message)
    }

    fn bind_to(&self, game_id: Option<&str>, settings_of: &dyn Fn(&str) -> Option<Value>) -> bool {
        LiveSplitOverlay::bind_to(self, game_id, settings_of)
    }
}
