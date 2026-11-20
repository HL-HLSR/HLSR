//! Меню игры без запуска (`GameActions.js`): открыть папку игры или скриншотов, ярлык на рабочем
//! столе (Windows — `.lnk` через оболочку, Linux — `.desktop`), удаление своего ярлыка, игра из
//! аргумента `--select-game=`.

use crate::errors::{Code, DeliveryError, Failure, RawError};
use crate::manifests;
use crate::paths::is_game_id as is_game_id_shape;
use crate::paths::{LibraryPaths, assert_inside, game_install_path};
use crate::ports::{Logger, Shell, ShortcutLink};
use hlsr_core::js::Platform;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// `SELECT_GAME_ARG`.
pub const SELECT_GAME_ARG: &str = "--select-game=";

/// Чем запускается лаунчер: для ярлыков.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LauncherIdentity {
    /// `process.execPath`.
    pub exec_path: PathBuf,
    /// `process.env.APPIMAGE`.
    pub app_image: Option<String>,
    /// `app.isPackaged`.
    pub packaged: bool,
    /// `app.getAppPath()` сборки разработки: идёт аргументом перед `--no-sandbox`. У лаунчера на
    /// Rust отдельной папки приложения нет — `None`.
    pub app_path: Option<PathBuf>,
}

/// `desktopExecPath(file)` (`linux/DesktopIntegration.js`): путь в кавычках для `Exec=`
/// (`\`, `"`, `$`, `` ` `` экранируются, `%` удваивается); не абсолютный или с переводом строки —
/// `None`.
pub fn desktop_exec_path(file: &str) -> Option<String> {
    if !file.starts_with('/') || file.contains(['\r', '\n', '\0']) {
        return None;
    }
    let mut escaped = String::with_capacity(file.len() + 2);
    for c in file.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '$' | '`' => {
                escaped.push('\\');
                escaped.push(c);
            }
            '%' => escaped.push_str("%%"),
            _ => escaped.push(c),
        }
    }
    Some(format!("\"{escaped}\""))
}

/// `ArchiveInstall.managedRoot(executable)`: корень управляемой установки Linux
/// (`<root>/versions/<v>/hlsr` с `.hlsr-install.json` и `.hlsr-release.json`) или `None`.
pub fn managed_root(executable: &Path) -> Option<PathBuf> {
    let real = std::fs::canonicalize(executable).ok()?;
    let dir = real.parent()?;
    let root = dir.parent()?.parent()?.to_path_buf();
    let read = |file: PathBuf| -> Option<Value> {
        serde_json::from_slice(&std::fs::read(file).ok()?).ok()
    };
    let marker = read(root.join(".hlsr-install.json"))?;
    let versions = dir.parent()?.file_name()?;
    if marker.get("schema") != Some(&json!(1)) || versions != "versions" {
        return None;
    }
    let release = read(dir.join(".hlsr-release.json"))?;
    let ours = release.get("platform") == Some(&json!("linux"))
        && release.get("arch") == Some(&json!("x64"))
        && real.file_name().is_some_and(|name| name == "hlsr");
    ours.then_some(root)
}

/// `GameActions.parseSelectGameArg(argv)`: игра из первого аргумента `--select-game=`.
pub fn parse_select_game_arg(argv: &[String]) -> Option<String> {
    let value = argv
        .iter()
        .find_map(|arg| arg.strip_prefix(SELECT_GAME_ARG))?;
    is_game_id_shape(value).then(|| value.to_owned())
}

/// Действия меню игры.
pub struct GameActions {
    paths: LibraryPaths,
    shell: Arc<dyn Shell>,
    /// `app.getPath("desktop")`.
    desktop: PathBuf,
    launcher: LauncherIdentity,
    logger: Arc<dyn Logger>,
    platform: Platform,
}

impl GameActions {
    pub fn new(
        paths: LibraryPaths,
        shell: Arc<dyn Shell>,
        desktop: PathBuf,
        launcher: LauncherIdentity,
        logger: Arc<dyn Logger>,
    ) -> GameActions {
        GameActions {
            paths,
            shell,
            desktop,
            launcher,
            logger,
            platform: Platform::CURRENT,
        }
    }

    /// Другая ОС (ярлык `.desktop` или `.lnk`) — для тестов.
    pub fn with_platform(mut self, platform: Platform) -> GameActions {
        self.platform = platform;
        self
    }

    /// `resolveGameId(rawGameId)`: известная игра с правильным id, иначе `PATH_INVALID`.
    pub fn resolve_game_id(&self, raw: &Value) -> Result<String, DeliveryError> {
        let game_id = if hlsr_core::js::truthy(raw) {
            hlsr_core::js::to_string(raw)
        } else {
            String::new()
        };
        if !is_game_id_shape(&game_id) || !manifests::is_known_game(&game_id) {
            return Err(DeliveryError::new(
                Code::PathInvalid,
                format!("Неизвестная игра: {}", crate::js_value::template(Some(raw))),
            ));
        }
        Ok(game_id)
    }

    /// `installPathOf(gameId)`: папка игры, если в ней есть `installed.json`.
    pub fn install_path_of(&self, game_id: &str) -> Result<PathBuf, DeliveryError> {
        let install_path = game_install_path(&self.paths, game_id)?;
        if !install_path.join("installed.json").exists() {
            return Err(DeliveryError::new(
                Code::NotInstalled,
                "Игра не установлена",
            ));
        }
        Ok(install_path)
    }

    /// `resolveOpenTarget(gameId, relativePath, {create})`.
    fn resolve_open_target(
        &self,
        game_id: &str,
        relative: Option<&str>,
        create: bool,
    ) -> Result<PathBuf, Failure> {
        let install_path = self.install_path_of(game_id)?;
        let Some(relative) = relative.filter(|relative| !relative.is_empty()) else {
            return Ok(install_path);
        };
        let native = if self.platform == Platform::Win32 {
            relative.replace('/', "\\")
        } else {
            relative.to_owned()
        };
        let candidate = PathBuf::from(
            hlsr_core::js::path::NodePath::native()
                .join(&[&install_path.to_string_lossy(), &native]),
        );
        let target = assert_inside(
            &install_path,
            &candidate,
            Some(&format!("Папка \"{relative}\"")),
        )?;
        assert_inside(&self.paths.root, &target, Some("Путь библиотеки"))?;
        if !target.exists() {
            if !create {
                return Err(DeliveryError::new(
                    Code::PathInvalid,
                    format!("Папка не найдена: {relative}"),
                )
                .into());
            }
            // Ошибка mkdir уходит наверх как есть: фасад даёт ей код INSTALL_FAILED.
            std::fs::create_dir_all(&target)
                .map_err(|error| RawError::fs(&error, "mkdir", &target))?;
            self.logger.info(&format!(
                "[game-delivery] создана папка {}",
                target.display()
            ));
        }
        Ok(target)
    }

    /// `openPath(target)`.
    async fn open_path(&self, target: &Path) -> Result<(), DeliveryError> {
        let error = self.shell.open_path(target).await;
        if !error.is_empty() {
            return Err(DeliveryError::new(
                Code::PathInvalid,
                format!("Не удалось открыть папку: {error}"),
            )
            .with_details(json!({"path": target.to_string_lossy()})));
        }
        Ok(())
    }

    /// `openGameFolder(rawGameId)`: `{opened}`.
    pub async fn open_game_folder(&self, raw: &Value) -> Result<Value, Failure> {
        let game_id = self.resolve_game_id(raw)?;
        let target = self.resolve_open_target(&game_id, None, false)?;
        self.open_path(&target).await?;
        Ok(json!({"opened": target.to_string_lossy()}))
    }

    /// `openScreenshotsFolder(rawGameId)`: `{opened, created: true}`; папки нет — создаётся.
    pub async fn open_screenshots_folder(&self, raw: &Value) -> Result<Value, Failure> {
        let game_id = self.resolve_game_id(raw)?;
        let screenshots = manifests::local_config_of(&game_id)
            .map(|config| config.screenshots_path)
            .filter(|path| !path.is_empty());
        let Some(screenshots) = screenshots else {
            return Err(DeliveryError::new(
                Code::PathInvalid,
                "Для этой сборки не задана папка скриншотов",
            )
            .into());
        };
        let target = self.resolve_open_target(&game_id, Some(screenshots), true)?;
        self.open_path(&target).await?;
        Ok(json!({"opened": target.to_string_lossy(), "created": true}))
    }

    fn shortcut_name(game_id: &str) -> String {
        manifests::local_config_of(game_id)
            .map(|config| config.shortcut_name)
            .filter(|name| !name.is_empty())
            .map_or_else(|| format!("HLSR - {game_id}"), str::to_owned)
    }

    /// `createDesktopShortcut(rawGameId)`: `{shortcut, args}`.
    pub fn create_desktop_shortcut(&self, raw: &Value) -> Result<Value, DeliveryError> {
        if self.platform == Platform::Linux {
            return self.create_desktop_entry(raw);
        }
        let game_id = self.resolve_game_id(raw)?;
        let name = Self::shortcut_name(&game_id);
        let link_path = self.desktop.join(format!("{name}.lnk"));
        let target = self.launcher.exec_path.to_string_lossy().into_owned();
        let args = format!("{SELECT_GAME_ARG}{game_id}");
        let cwd = self
            .launcher
            .exec_path
            .parent()
            .map(|dir| dir.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ok = self.shell.write_shortcut_link(
            &link_path,
            &ShortcutLink {
                target: target.clone(),
                args: args.clone(),
                description: name,
                cwd,
                icon: target,
                icon_index: 0,
            },
        );
        if !ok {
            return Err(DeliveryError::new(
                Code::PathNotWritable,
                "Не удалось создать ярлык на рабочем столе",
            ));
        }
        self.logger.info(&format!(
            "[game-delivery] создан ярлык {} ({args})",
            link_path.display()
        ));
        Ok(json!({"shortcut": link_path.to_string_lossy(), "args": args}))
    }

    /// `createDesktopEntry(rawGameId)` (Linux): `.desktop`, который запускает лаунчер через
    /// `start` управляемой установки, AppImage или текущий файл.
    fn create_desktop_entry(&self, raw: &Value) -> Result<Value, DeliveryError> {
        let game_id = self.resolve_game_id(raw)?;
        let name = Self::shortcut_name(&game_id);
        let link_path = self.shortcut_path_of(&game_id);
        let target = match managed_root(&self.launcher.exec_path) {
            Some(root) => root.join("start").to_string_lossy().into_owned(),
            None => self
                .launcher
                .app_image
                .clone()
                .filter(|image| !image.is_empty())
                .unwrap_or_else(|| self.launcher.exec_path.to_string_lossy().into_owned()),
        };
        let args = format!("{SELECT_GAME_ARG}{game_id}");
        let Some(quoted_target) = desktop_exec_path(&target) else {
            return Err(DeliveryError::new(
                Code::PathInvalid,
                "Некорректный путь лаунчера",
            ));
        };
        let no_image = self.launcher.app_image.as_deref().is_none_or(str::is_empty);
        let app_arg = (no_image && !self.launcher.packaged)
            .then_some(self.launcher.app_path.as_ref())
            .flatten()
            .and_then(|path| desktop_exec_path(&path.to_string_lossy()));
        let exec_args = [
            Some(quoted_target),
            app_arg,
            Some("--no-sandbox".to_owned()),
            Some(args.clone()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
        let content = [
            "[Desktop Entry]".to_owned(),
            "Type=Application".to_owned(),
            format!("Name={}", name.replace(['\r', '\n'], " ")),
            format!("Exec={exec_args}"),
            "Icon=applications-games".to_owned(),
            "Categories=Game;".to_owned(),
            format!("X-HLSR-GameId={game_id}"),
            String::new(),
        ]
        .join("\n");

        write_desktop_entry(&self.desktop, &link_path, &content).map_err(|message| {
            DeliveryError::new(
                Code::PathNotWritable,
                format!("Не удалось создать ярлык: {message}"),
            )
        })?;

        self.logger.info(&format!(
            "[game-delivery] создан ярлык {} ({args})",
            link_path.display()
        ));
        Ok(json!({"shortcut": link_path.to_string_lossy(), "args": args}))
    }

    /// `shortcutPathOf(gameId)`.
    pub fn shortcut_path_of(&self, game_id: &str) -> PathBuf {
        if self.platform == Platform::Linux {
            return self.desktop.join(format!("hlsr-{game_id}.desktop"));
        }
        self.desktop
            .join(format!("{}.lnk", Self::shortcut_name(game_id)))
    }

    /// `removeDesktopShortcut(rawGameId)`: `{removed, reason}` или `{removed: true, path}`.
    pub fn remove_desktop_shortcut(&self, raw: &Value) -> Result<Value, DeliveryError> {
        let game_id = self.resolve_game_id(raw)?;
        let link_path = self.shortcut_path_of(&game_id);
        if !link_path.exists() {
            return Ok(json!({"removed": false, "reason": "absent"}));
        }
        match self.is_own_shortcut(&link_path, &game_id) {
            Err(()) => return Ok(json!({"removed": false, "reason": "unreadable"})),
            Ok(false) => return Ok(json!({"removed": false, "reason": "foreign"})),
            Ok(true) => {}
        }
        if let Err(error) = std::fs::remove_file(&link_path) {
            let message = RawError::fs(&error, "unlink", &link_path).message;
            self.logger.warn(&format!(
                "[game-delivery] не удалось удалить ярлык {}: {message}",
                link_path.display()
            ));
            return Ok(json!({"removed": false, "reason": "failed"}));
        }
        self.logger.info(&format!(
            "[game-delivery] удалён ярлык {}",
            link_path.display()
        ));
        Ok(json!({"removed": true, "path": link_path.to_string_lossy()}))
    }

    /// `isOwnShortcut(linkPath, gameId)`; `Err` — не прочитан.
    fn is_own_shortcut(&self, link_path: &Path, game_id: &str) -> Result<bool, ()> {
        let arg = format!("{SELECT_GAME_ARG}{game_id}");
        if self.platform == Platform::Linux {
            let content = std::fs::read(link_path).map_err(|_| ())?;
            let content = String::from_utf8_lossy(&content);
            return Ok(
                content.contains(&format!("X-HLSR-GameId={game_id}\n")) && content.contains(&arg)
            );
        }
        let link = self.shell.read_shortcut_link(link_path).map_err(|_| ())?;
        let exec = self.launcher.exec_path.to_string_lossy().to_lowercase();
        Ok(link.target.to_lowercase() == exec && link.args.contains(&arg))
    }
}

/// `mkdirSync(desktop)`, `writeFileSync(linkPath, content, {mode: 0o755})`, `chmodSync(0o755)`.
fn write_desktop_entry(desktop: &Path, link_path: &Path, content: &str) -> Result<(), String> {
    std::fs::create_dir_all(desktop)
        .map_err(|error| RawError::fs(&error, "mkdir", desktop).message)?;
    std::fs::write(link_path, content)
        .map_err(|error| RawError::fs(&error, "open", link_path).message)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(link_path, std::fs::Permissions::from_mode(0o755))
            .map_err(|error| RawError::fs(&error, "chmod", link_path).message)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_path_is_quoted_like_desktop_integration() {
        assert_eq!(
            desktop_exec_path("/opt/HLSR 100%.AppImage").as_deref(),
            Some("\"/opt/HLSR 100%%.AppImage\"")
        );
        assert_eq!(
            desktop_exec_path("/a\"b$c`d\\e").as_deref(),
            Some("\"/a\\\"b\\$c\\`d\\\\e\"")
        );
        assert_eq!(desktop_exec_path("relative"), None);
        assert_eq!(desktop_exec_path("/a\nb"), None);
    }

    #[test]
    fn select_game_arg_takes_the_first_one() {
        let args = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            parse_select_game_arg(&args(&["x", "--select-game=hl1-won"])).as_deref(),
            Some("hl1-won")
        );
        assert_eq!(
            parse_select_game_arg(&args(&["--select-game=Bad", "--select-game=hl1-won"])),
            None
        );
        assert_eq!(parse_select_game_arg(&args(&["--select-game="])), None);
    }
}
