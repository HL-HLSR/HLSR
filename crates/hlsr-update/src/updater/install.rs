//! Установка обновления (`updater/core/install.js`): `HLSR-Setup-<v>.exe /S --updated /D=…`
//! (0.4.9 опускала `/D=`, если в пути пробел или `"`: NSIS не принимает `/D=` в кавычках, а Node
//! кавычит такой аргумент; см. [`InstallOptions::installer`]), окно установщика не скрыто, 10 минут,
//! нужен код 0. Собранная из чанков
//! версия ставится своим же `HLSR.exe` (`/D=` всегда, `PORTABLE_EXECUTABLE_FILE`, скрытое окно).
//! Проверка установленного: первые три числа `ProductVersion` у `HLSR.exe` (`GetFileVersionInfoW`
//! вместо PowerShell — решение 14); не прочиталось — принимается. Затем запуск лаунчера без
//! аргументов из каталога установки.

use super::processes::{ProcessLauncher, RunExit, Spawn, set_env};
use crate::errors::{UpdCode, UpdaterError};
use hlsr_core::js;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// `INSTALLER_ARGS`.
pub const INSTALLER_ARGS: [&str; 2] = ["/S", "--updated"];
/// `DEFAULT_INSTALL_TIMEOUT_MS`.
pub const DEFAULT_INSTALL_TIMEOUT: Duration = Duration::from_millis(10 * 60 * 1000);

/// `nsisRejectsDSwitchQuotedBySpawn(dir)`: `/[\s"]/`.
pub fn nsis_rejects_d_switch_quoted_by_spawn(dir: &str) -> bool {
    dir.chars().any(|c| c == '"' || js::is_whitespace(c))
}

/// `installerArgsFor(installDir)`: `/D=<каталог>` последним — только без пробелов и кавычек.
/// `None` — каталог не строка.
pub fn installer_args_for(install_dir: Option<&str>) -> Vec<String> {
    let dir = install_dir.map(js::trim).unwrap_or("");
    let mut args: Vec<String> = INSTALLER_ARGS.iter().map(|arg| (*arg).to_owned()).collect();
    if !dir.is_empty() && !nsis_rejects_d_switch_quoted_by_spawn(dir) {
        args.push(format!("/D={dir}"));
    }
    args
}

/// Как запускать установщик (`options` `runInstaller`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOptions {
    pub args: Vec<String>,
    /// Окружение установщика целиком.
    pub env: Vec<(String, String)>,
    /// `windowsHide`.
    pub hidden: bool,
    pub timeout: Duration,
}

impl InstallOptions {
    /// Полный установщик: `/S --updated /D=<каталог>`, окно не скрыто. Отступление от 0.4.9
    /// ([`installer_args_for`]): `/D=` передаётся и с пробелом в пути. Апдейтер Rust ставит только
    /// 0.5+, а её установщик — свой `HLSR.exe`, он принимает `/D=` в кавычках, как у собранной из
    /// чанков версии. Без `/D=` каталог брался из `InstallLocation`, и `D:\HLSR Тест\Launcher` при
    /// устаревшем реестре обновлялся не там (или ставился в `C:\HLSR\Launcher`).
    pub fn installer(install_dir: &str, env: Vec<(String, String)>) -> InstallOptions {
        let dir = js::trim(install_dir);
        let mut args: Vec<String> = INSTALLER_ARGS.iter().map(|arg| (*arg).to_owned()).collect();
        if !dir.is_empty() {
            args.push(format!("/D={dir}"));
        }
        InstallOptions {
            args,
            env,
            hidden: false,
            timeout: DEFAULT_INSTALL_TIMEOUT,
        }
    }
}

/// `treeSetupOptions(treeDir, installDir, version)`: собранная из чанков версия ставится её же
/// `HLSR.exe` как установщиком (`PORTABLE_EXECUTABLE_FILE` включает режим `Setup`).
pub fn tree_setup_options(
    tree_dir: &Path,
    install_dir: &str,
    version: &str,
    mut env: Vec<(String, String)>,
) -> InstallOptions {
    let setup = tree_dir.join(format!("HLSR-Setup-{version}.exe"));
    set_env(
        &mut env,
        hlsr_core::args::PORTABLE_EXECUTABLE_FILE,
        &setup.to_string_lossy(),
    );
    InstallOptions {
        args: vec![
            "/S".to_owned(),
            "--updated".to_owned(),
            format!("/D={install_dir}"),
        ],
        env,
        hidden: true,
        timeout: DEFAULT_INSTALL_TIMEOUT,
    }
}

/// `runInstaller(installerPath, options)`: нужен код выхода 0.
pub async fn run_installer(
    launcher: &dyn ProcessLauncher,
    installer: &Path,
    options: &InstallOptions,
) -> Result<(), UpdaterError> {
    let command = Spawn {
        file: installer.to_owned(),
        args: options.args.clone(),
        cwd: None,
        env: options.env.clone(),
        hidden: options.hidden,
        detached: false,
    };
    match launcher.run(&command, options.timeout).await {
        Err(message) => Err(UpdaterError::new(UpdCode::InstallFailed, message)),
        Ok(RunExit::TimedOut) => Err(UpdaterError::new(
            UpdCode::InstallFailed,
            format!(
                "установщик не завершился за {} мс",
                options.timeout.as_millis()
            ),
        )),
        Ok(RunExit::Exited(Some(0))) => Ok(()),
        Ok(RunExit::Exited(code)) => {
            let shown = code.map_or_else(|| "null".to_owned(), |code| code.to_string());
            Err(UpdaterError::new(
                UpdCode::InstallExitCode,
                format!("установщик вернул код {shown}"),
            )
            .with_details(json!({"exitCode": code})))
        }
    }
}

/// `versionTriple(value)`: первые три числа (`/^(\d+)\.(\d+)\.(\d+)/` после `trim`).
pub fn version_triple(value: &str) -> Option<String> {
    let text = js::trim(value);
    let mut parts = Vec::with_capacity(3);
    let mut rest = text;
    for index in 0..3 {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        parts.push(&rest[..digits]);
        rest = &rest[digits..];
        if index < 2 {
            rest = rest.strip_prefix('.')?;
        }
    }
    Some(parts.join("."))
}

/// Вывод `(Get-Item …).VersionInfo.ProductVersion` 0.4.7: после `trim` начинается с `X.Y.Z` —
/// первое слово, иначе `None`. То же правило — к строке `ProductVersion` из ресурсов exe.
pub fn product_version_from_text(text: &str) -> Option<String> {
    let value = js::trim(text);
    version_triple(value)?;
    value.split(js::is_whitespace).next().map(str::to_owned)
}

/// `ProductVersion` из ресурса `VERSIONINFO` (`GetFileVersionInfoW`): сначала язык из
/// `\VarFileInfo\Translation`, затем английский с кодовыми страницами 1200, 1252 и 0, как
/// `FileVersionInfo` .NET. Ресурса нет или строка не версия — `None`.
#[cfg(windows)]
pub fn read_exe_product_version(path: &Path) -> Option<String> {
    use windows::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    };
    use windows::core::HSTRING;

    let file = HSTRING::from(path);
    // SAFETY: буфер нужной длины живёт, пока из него читают VerQueryValueW; указатели,
    // которые она отдаёт, указывают внутрь буфера, длины — в единицах из документации.
    unsafe {
        let size = GetFileVersionInfoSizeW(&file, None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(&file, None, size, data.as_mut_ptr().cast()).ok()?;

        let query = |sub: &str| -> Option<(*const u8, usize)> {
            let mut pointer: *mut core::ffi::c_void = std::ptr::null_mut();
            let mut len = 0u32;
            let ok = VerQueryValueW(
                data.as_ptr().cast(),
                &HSTRING::from(sub),
                &mut pointer,
                &mut len,
            );
            (ok.as_bool() && !pointer.is_null()).then_some((pointer as *const u8, len as usize))
        };

        let mut blocks: Vec<String> = Vec::new();
        if let Some((pointer, len)) = query("\\VarFileInfo\\Translation") {
            let pairs = std::slice::from_raw_parts(pointer.cast::<u16>(), len / 2);
            for pair in pairs.as_chunks::<2>().0 {
                blocks.push(format!("{:04X}{:04X}", pair[0], pair[1]));
            }
        }
        blocks.extend(["040904B0", "040904E4", "04090000"].map(str::to_owned));

        for block in blocks {
            let Some((pointer, len)) = query(&format!("\\StringFileInfo\\{block}\\ProductVersion"))
            else {
                continue;
            };
            let units = std::slice::from_raw_parts(pointer.cast::<u16>(), len);
            let end = units
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(units.len());
            let text = String::from_utf16_lossy(&units[..end]);
            if !text.is_empty() {
                return product_version_from_text(&text);
            }
        }
        None
    }
}

/// Не Windows: ресурсов `VERSIONINFO` нет — версия не читается (и потому принимается).
#[cfg(not(windows))]
pub fn read_exe_product_version(_path: &Path) -> Option<String> {
    None
}

/// Итог `verifyInstalled`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub ok: bool,
    pub code: Option<UpdCode>,
    pub detail: Option<String>,
    /// Прочитанная версия: `Some(None)` — поле есть и равно `null`, `None` — поля нет.
    pub version: Option<Option<String>>,
    pub unverified: bool,
}

impl Installed {
    /// Форма 0.4.7 (только те поля, что были у объекта).
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("ok".into(), json!(self.ok));
        if let Some(code) = self.code {
            out.insert("code".into(), json!(code.as_str()));
        }
        if let Some(detail) = &self.detail {
            out.insert("detail".into(), json!(detail));
        }
        if let Some(version) = &self.version {
            out.insert("version".into(), json!(version));
        }
        if self.unverified {
            out.insert("unverified".into(), json!(true));
        }
        Value::Object(out)
    }
}

/// `verifyInstalled({launcherExe, targetVersion, readVersion})`.
pub fn verify_installed(
    launcher_exe: &Path,
    target_version: Option<&str>,
    read_version: &dyn Fn(&Path) -> Option<String>,
) -> Installed {
    if !launcher_exe.exists() {
        return Installed {
            ok: false,
            code: Some(UpdCode::InstallNotApplied),
            detail: Some("HLSR.exe отсутствует".to_owned()),
            version: None,
            unverified: false,
        };
    }
    let actual = read_version(launcher_exe).filter(|version| !version.is_empty());
    let Some(actual) = actual else {
        return Installed {
            ok: true,
            code: None,
            detail: None,
            version: Some(None),
            unverified: true,
        };
    };
    let Some(actual_triple) = version_triple(&actual) else {
        return Installed {
            ok: true,
            code: None,
            detail: None,
            version: Some(Some(actual)),
            unverified: true,
        };
    };
    let expected_triple = target_version.and_then(version_triple);
    if expected_triple.is_some_and(|expected| expected != actual_triple) {
        return Installed {
            ok: false,
            code: Some(UpdCode::InstallNotApplied),
            detail: Some(format!(
                "на диске {actual}, ожидалось {}",
                target_version.unwrap_or("undefined")
            )),
            version: Some(Some(actual)),
            unverified: false,
        };
    }
    Installed {
        ok: true,
        code: None,
        detail: None,
        version: Some(Some(actual)),
        unverified: false,
    }
}

/// `relaunch(launcherExe)`: лаунчер без аргументов из каталога установки, отвязанным процессом.
pub fn relaunch(
    launcher: &dyn ProcessLauncher,
    launcher_exe: &Path,
    env: Vec<(String, String)>,
) -> Result<u32, UpdaterError> {
    if !launcher_exe.exists() {
        return Err(UpdaterError::new(
            UpdCode::RelaunchFailed,
            "HLSR.exe не найден после установки",
        ));
    }
    let command = Spawn {
        file: launcher_exe.to_owned(),
        args: Vec::new(),
        cwd: launcher_exe.parent().map(PathBuf::from),
        env,
        hidden: false,
        detached: true,
    };
    launcher
        .spawn(&command)
        .map_err(|message| UpdaterError::new(UpdCode::RelaunchFailed, message))
}
