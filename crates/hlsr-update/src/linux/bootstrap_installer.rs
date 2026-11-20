//! Установщик AppImage (режим `Bootstrap`; `launcher-update/BootstrapInstaller.js` и
//! `installer/bootstrap-main.js` 0.4.9): ставит вшитый архив без сети, предлагает обновить более
//! старую установку, удаляет установку (игры и данные лаунчера — только по согласию; чужое в библиотеке
//! остаётся, окно говорит «Оставлено в …»), запускает `current/hlsr
//! --no-sandbox` без переменных AppImage. Окно — общее [`InstallerWindow`] (формы карты §4.2), действия
//! — [`BootstrapBackend`].
//!
//! Отличия от 0.4.7: путь библиотеки читается из `library.json` каталога данных лаунчера (у Electron
//! установщик жил в соседнем профиле `…/hlsr/installer` и читал `../library.json` — тот же файл);
//! копия на рабочий стол и её удаление — по `XDG_DESKTOP_DIR` (решение 17).

use super::archive_install::{self, ArchiveInstall};
use super::desktop_integration::{self, AppImageDesktop, MANAGED_MARKER};
use super::embedded_payload::{self, Payload};
use super::launcher_uninstall;
use super::{EnvVars, LinuxDirs, SyncRunner};
use crate::channel::compare_semver;
use crate::installer_window::{
    BackendError, Ctx, InstallerBackend, InstallerState, InstallerWindow, Phase, StatePatch,
    UninstallPlan,
};
use crate::update_manager::AppControl;
use crate::updater::processes::{ProcessLauncher, Spawn, child_env};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// `launchEnvironment(env)`: без `APPIMAGE`, `APPDIR`, `ARGV0`, `OWD`; из `LD_LIBRARY_PATH` убираются
/// каталоги смонтированного AppImage.
pub fn launch_environment(source: &[(String, String)]) -> EnvVars {
    let appdir = super::env_get(source, "APPDIR").map(str::to_owned);
    let mut env: EnvVars = source
        .iter()
        .filter(|(name, _)| !["APPIMAGE", "APPDIR", "ARGV0", "OWD"].contains(&name.as_str()))
        .cloned()
        .collect();
    if let Some(appdir) = appdir.filter(|appdir| !appdir.is_empty())
        && let Some(index) = env
            .iter()
            .position(|(name, value)| name == "LD_LIBRARY_PATH" && !value.is_empty())
    {
        let kept: Vec<&str> = env[index]
            .1
            .split(':')
            .filter(|entry| !entry.is_empty() && !entry.starts_with(appdir.as_str()))
            .collect();
        if kept.is_empty() {
            env.remove(index);
        } else {
            env[index].1 = kept.join(":");
        }
    }
    env
}

/// `rendererLibraryPath`: `libraryPath` из `library.json` лаунчера, иначе `~/Games/HLSR`.
pub fn renderer_library_path(dirs: &LinuxDirs) -> String {
    std::fs::read(dirs.config.join("library.json"))
        .ok()
        .and_then(|bytes| hlsr_core::json_text::json_parse(&String::from_utf8_lossy(&bytes)).ok())
        .and_then(|data| {
            data.get("libraryPath")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .map(|path| hlsr_core::js::trim(&path).to_owned())
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| {
            dirs.home
                .join("Games")
                .join("HLSR")
                .to_string_lossy()
                .into_owned()
        })
}

/// `versionOf(current)`: версия из `versions/<версия>-<sha>`.
pub fn version_of(current: Option<&str>) -> Option<String> {
    let rest = current?.strip_prefix("versions/")?;
    let (version, _) = rest.split_once('-')?;
    let parts: Vec<&str> = version.split('.').collect();
    (parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())))
    .then(|| version.to_owned())
}

fn is_newer(payload: &Payload, installed: Option<&str>) -> bool {
    installed.is_some_and(|installed| {
        compare_semver(&payload.version(), installed).is_ok_and(|order| order.is_gt())
    })
}

/// Внешний мир установщика.
pub struct BootstrapServices {
    pub dirs: LinuxDirs,
    /// `process.env` (дочерним процессам — без `SLINT_*`/`WINIT_*`).
    pub env: EnvVars,
    pub runner: Arc<dyn SyncRunner>,
    pub launcher: Arc<dyn ProcessLauncher>,
    pub app: Arc<dyn AppControl>,
    /// Среда tokio для проверки архива (методы окна вызываются вне её потоков).
    pub runtime: tokio::runtime::Handle,
    /// Выход перед полным удалением (`SessionStore::clear` ПП4, аргумент — каталог данных): секрет сессии
    /// 0.5 лежит в Secret Service/KWallet и пережил бы удаление `~/.config/hlsr` («…настройками и входом»).
    pub forget_session: Arc<dyn Fn(&Path) + Send + Sync>,
}

/// Что открыть.
#[derive(Debug, Clone)]
pub struct BootstrapOptions {
    /// Корень установки; `None` — `~/.local/opt/hlsr`.
    pub root: Option<PathBuf>,
    /// Каталог вшитого архива (`<resources>/payload`).
    pub payload_dir: PathBuf,
    /// `ru` или `en` (`app.getLocale()`).
    pub language: String,
}

/// Действия окна установщика AppImage.
pub struct BootstrapBackend {
    services: BootstrapServices,
    root: Mutex<PathBuf>,
    payload: Result<Payload, String>,
}

impl BootstrapBackend {
    fn root(&self) -> PathBuf {
        self.root
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn library_root(&self) -> String {
        renderer_library_path(&self.services.dirs)
    }

    fn payload_error(&self) -> Option<String> {
        self.payload.as_ref().err().cloned()
    }

    fn desktop_shortcut(&self, root: &Path) -> Result<(), String> {
        let options = AppImageDesktop {
            executable: root.join("start").to_string_lossy().into_owned(),
            icon_source: root
                .join("current")
                .join("resources")
                .join("icons")
                .join("hlsr.png"),
            data_home: self.services.dirs.data_home.clone(),
            refresh: true,
        };
        if !desktop_integration::ensure_app_image_desktop(
            &options,
            self.services.runner.as_ref(),
            &self.services.env,
        )? {
            return Err("Ярлык в меню принадлежит другому приложению".to_owned());
        }
        Ok(())
    }

    fn desktop_copy(&self) -> Result<(), String> {
        let desktop = &self.services.dirs.desktop;
        super::make_dirs(desktop, 0o777)
            .map_err(|error| super::fs_error(&error, "mkdir", desktop))?;
        let target = desktop.join(desktop_integration::DESKTOP_FILE);
        if target.exists()
            && !std::fs::read(&target)
                .map(|bytes| String::from_utf8_lossy(&bytes).contains(MANAGED_MARKER))
                .unwrap_or(false)
        {
            return Err("Существующий ярлык рабочего стола сохранён".to_owned());
        }
        let source = desktop_integration::desktop_path(&self.services.dirs.data_home);
        std::fs::copy(&source, &target)
            .map_err(|error| super::fs_error2(&error, "copyfile", &source, &target))?;
        super::chmod(&target, 0o755).map_err(|error| super::fs_error(&error, "chmod", &target))
    }
}

impl InstallerBackend for BootstrapBackend {
    fn can_select_directory(&self) -> bool {
        true
    }

    fn select_directory(&self, directory: &str, ctx: &Ctx<'_>) -> Result<(), String> {
        let selected = super::resolve(Path::new(directory));
        let home = &self.services.dirs.home;
        let current = archive_install::read_current(&selected).ok().flatten();
        if current.is_none()
            && !launcher_uninstall::is_managed_root(&selected, home)
            && selected.exists()
        {
            let names = hlsr_core::js::fs::read_dir_names(&selected)
                .map_err(|error| super::fs_error(&error, "scandir", &selected))?;
            if !names.is_empty() {
                return Err("Выберите пустую папку или каталог установленного HLSR".to_owned());
            }
        }
        *self
            .root
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = selected.clone();
        let current_version = version_of(current.as_deref());
        let update = self
            .payload
            .as_ref()
            .is_ok_and(|payload| is_newer(payload, current_version.as_deref()));
        let error = self.payload_error();
        ctx.send(StatePatch {
            directory: Some(selected.to_string_lossy().into_owned()),
            phase: Some(if error.is_some() {
                Phase::Error
            } else if current.is_some() && !update {
                Phase::Done
            } else {
                Phase::Idle
            }),
            installed_version: Some(current_version),
            update: Some(update),
            can_uninstall: Some(
                current.is_some() || launcher_uninstall::is_managed_root(&selected, home),
            ),
            error: Some(error),
            warning: Some(None),
            ..StatePatch::default()
        });
        Ok(())
    }

    fn install(&self, desktop_shortcut: bool, ctx: &Ctx<'_>) -> Result<bool, BackendError> {
        let payload = match &self.payload {
            Ok(payload) => payload.clone(),
            Err(error) => {
                ctx.send(StatePatch::phase(Phase::Error).with_error(Some(error.clone())));
                return Ok(false);
            }
        };
        let root = self.root();
        let mut installer = ArchiveInstall::new(&root);
        let result = (|| -> Result<bool, String> {
            ctx.send(StatePatch::phase(Phase::Verifying).with_error(None));
            let cancel = ctx.cancel_token().cloned();
            self.services.runtime.block_on(installer.prepare(
                &payload.archive,
                &payload.manifest,
                cancel.as_ref(),
            ))?;
            if ctx.closed() {
                return Ok(false);
            }
            if cancel
                .as_ref()
                .is_some_and(crate::cancel::CancelToken::is_cancelled)
            {
                return Err("This operation was aborted".to_owned());
            }
            ctx.commit();
            ctx.send(StatePatch::phase(Phase::Installing));
            installer.commit()?;
            let mut warning: Option<String> = None;
            match desktop_integration::ensure_managed_cli(&root, &self.services.dirs.home) {
                Ok(true) => {}
                Ok(false) => {
                    warning = Some("Существующая команда hlsr сохранена без изменений".to_owned())
                }
                Err(error) => warning = Some(error),
            }
            let desktop = self.desktop_shortcut(&root).and_then(|()| {
                if desktop_shortcut {
                    self.desktop_copy()
                } else {
                    Ok(())
                }
            });
            if let Err(error) = desktop {
                warning = Some(
                    [warning.as_deref(), Some(error.as_str())]
                        .into_iter()
                        .flatten()
                        .filter(|text| !text.is_empty())
                        .collect::<Vec<_>>()
                        .join(". "),
                );
            }
            ctx.send(StatePatch {
                phase: Some(Phase::Done),
                warning: Some(warning),
                can_uninstall: Some(false),
                update: Some(false),
                installed_version: Some(Some(payload.version())),
                ..StatePatch::default()
            });
            Ok(true)
        })();
        installer.discard();
        Ok(result?)
    }

    fn launch(&self, _ctx: &Ctx<'_>) -> Result<bool, String> {
        let root = self.root();
        let current = archive_install::read_current(&root)?.ok_or_else(|| {
            "The \"path\" argument must be of type string. Received null".to_owned()
        })?;
        let target = root.join(&current).join("hlsr");
        let command = Spawn {
            file: target,
            args: vec!["--no-sandbox".to_owned()],
            cwd: None,
            env: child_env(launch_environment(&self.services.env)),
            hidden: false,
            detached: true,
        };
        self.services.launcher.spawn(&command)?;
        self.services.app.quit();
        Ok(true)
    }

    fn uninstall_plan(&self, _ctx: &Ctx<'_>) -> Option<UninstallPlan> {
        launcher_uninstall::plan_launcher_uninstall(
            &self.root(),
            &self.library_root(),
            &self.services.dirs,
        )
        .ok()
    }

    fn uninstall(&self, remove_games: bool, ctx: &Ctx<'_>) -> Result<bool, String> {
        ctx.commit();
        ctx.send(StatePatch::phase(Phase::Uninstalling).with_error(None));
        let services = &self.services;
        // Данные лаунчера: ~/.config/hlsr (`dirs.config`) и журналы ~/.local/state/hlsr; уходят только
        // при полном удалении.
        let data_dirs = [
            services.dirs.config.clone(),
            services.dirs.home.join(".local").join("state").join("hlsr"),
        ];
        if remove_games {
            (services.forget_session)(&services.dirs.config);
        }
        let result = launcher_uninstall::uninstall_launcher(
            &self.root(),
            &self.library_root(),
            remove_games,
            &data_dirs,
            &services.dirs,
        )
        .map_err(|error| error.message)?;
        desktop_integration::refresh_menu_caches(
            &services.dirs.data_home,
            services.runner.as_ref(),
            &services.env,
        );
        if result.desktop_removed {
            desktop_integration::refresh_kde_menu_cache(
                &services.dirs.data_home,
                services.runner.as_ref(),
                &services.env,
            );
        }
        let failed = (result.status == "partial").then(|| {
            let shown: Vec<&str> = result
                .failed
                .iter()
                .take(3)
                .map(|item| item.path.as_str())
                .collect();
            format!(
                "Часть файлов осталась ({}): {}",
                result.failed.len(),
                shown.join(", ")
            )
        });
        let left = (!result.left.is_empty()).then(|| {
            format!(
                "Оставлено в {} ({}), удаление это не трогает: {}",
                result.library_root,
                result.left.len(),
                result
                    .left
                    .iter()
                    .take(5)
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        });
        let warning = [failed, left].into_iter().flatten().collect::<Vec<_>>();
        let warning = (!warning.is_empty()).then(|| warning.join(". "));
        ctx.send(StatePatch {
            phase: Some(Phase::Uninstalled),
            can_uninstall: Some(false),
            warning: Some(warning),
            ..StatePatch::default()
        });
        Ok(true)
    }
}

/// `openInstaller(electron, options)`: окно установщика с начальным состоянием — фаза `error`, если
/// вшитый архив не читается; `done`, если стоит та же или более новая версия; иначе `idle`.
pub fn open_installer(
    options: BootstrapOptions,
    services: BootstrapServices,
) -> Arc<InstallerWindow> {
    let root = options
        .root
        .clone()
        .unwrap_or_else(|| archive_install::default_root(&services.dirs.home));
    let installed = archive_install::read_current(&root).ok().flatten();
    let preexisting =
        installed.is_some() || launcher_uninstall::is_managed_root(&root, &services.dirs.home);
    let payload = embedded_payload::read(&options.payload_dir)
        .map_err(|error| format!("{error}. Скачайте установщик заново."));
    let installed_version = version_of(installed.as_deref());
    let update = payload
        .as_ref()
        .is_ok_and(|payload| is_newer(payload, installed_version.as_deref()));
    let state = InstallerState {
        phase: if payload.is_err() {
            Phase::Error
        } else if installed.is_some() && !update {
            Phase::Done
        } else {
            Phase::Idle
        },
        directory: root.to_string_lossy().into_owned(),
        language: options.language.clone(),
        can_uninstall: preexisting,
        version: payload.as_ref().ok().map(Payload::version),
        installed_version,
        update,
        error: payload.as_ref().err().cloned(),
        ..InstallerState::default()
    };
    let backend = Arc::new(BootstrapBackend {
        services,
        root: Mutex::new(root),
        payload,
    });
    Arc::new(InstallerWindow::new(backend, state))
}
