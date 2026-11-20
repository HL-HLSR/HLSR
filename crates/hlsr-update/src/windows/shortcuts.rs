//! Ярлыки HLSR в меню «Пуск» и на рабочем столе (`windows-install/shortcuts.js`) — те же файлы
//! `HLSR.lnk`, что создавал прежний установщик, поэтому обновление их подхватывает. В 0.4.7
//! ярлык писал `shell.writeShortcutLink` Electron; здесь — `IShellLinkW` с `IPropertyStore`
//! (AUMID) через COM ([`SystemShellLinks`]), за трейтом [`ShellLinks`] ради тестов. Тот же COM
//! пишет и читает ярлыки игр на рабочем столе (`AppShell` приложения, `GameActions.js`).

use super::known_folders::KnownFolders;
use super::layout::{self, Identity, win_path};
use serde_json::{Value, json};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Что записать в ярлык (поля `options` `shell.writeShortcutLink`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutSpec {
    pub target: String,
    /// Аргументы командной строки (`--select-game=<id>` у ярлыка игры, пусто у ярлыков установки).
    pub args: String,
    pub cwd: String,
    pub icon: String,
    pub icon_index: i32,
    /// `appUserModelId`: группировка окон на панели задач; пусто — не задаётся (ключа нет в
    /// `options`).
    pub app_user_model_id: String,
    pub description: String,
}

/// Что прочитано из ярлыка (`shell.readShortcutLink`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShortcutInfo {
    pub target: String,
    pub cwd: String,
    pub args: String,
    pub description: String,
    pub icon: String,
    pub icon_index: i32,
    pub app_user_model_id: String,
}

/// Ярлык не создался или не прочитался.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ShortcutError(pub String);

/// Файлы `.lnk`.
pub trait ShellLinks: Send + Sync {
    /// Создать ярлык или заменить существующий. Каталог файла должен существовать.
    fn write(&self, file: &Path, spec: &ShortcutSpec) -> Result<(), ShortcutError>;
    /// Прочитать ярлык.
    fn read(&self, file: &Path) -> Result<ShortcutInfo, ShortcutError>;
}

/// Подделка: «ярлык» — JSON-файл с теми же полями, чтобы работали `exists` и удаление файла.
/// Тело — байт в байт как у подделки `tools/reference/update/windows-install.js` (размер и хеш
/// файла — в фикстуре), поэтому без `args`: у ярлыков установки их нет.
#[derive(Debug, Default)]
pub struct FakeShellLinks {
    written: Mutex<Vec<PathBuf>>,
    fail_writes: Mutex<bool>,
}

impl FakeShellLinks {
    pub fn new() -> FakeShellLinks {
        FakeShellLinks::default()
    }

    /// Следующие записи отказывают.
    pub fn set_fail_writes(&self, fail: bool) {
        *self.fail_writes.lock().unwrap_or_else(|p| p.into_inner()) = fail;
    }

    /// Файлы, записанные через подделку, по порядку.
    pub fn written(&self) -> Vec<PathBuf> {
        self.written
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

impl ShellLinks for FakeShellLinks {
    fn write(&self, file: &Path, spec: &ShortcutSpec) -> Result<(), ShortcutError> {
        if *self.fail_writes.lock().unwrap_or_else(|p| p.into_inner()) {
            return Err(ShortcutError("отказано в доступе".to_owned()));
        }
        let body = json!({
            "target": spec.target,
            "cwd": spec.cwd,
            "icon": spec.icon,
            "iconIndex": spec.icon_index,
            "appUserModelId": spec.app_user_model_id,
            "description": spec.description,
        });
        std::fs::write(file, body.to_string())
            .map_err(|error| ShortcutError(format!("{}: {error}", file.display())))?;
        self.written
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(file.to_owned());
        Ok(())
    }

    fn read(&self, file: &Path) -> Result<ShortcutInfo, ShortcutError> {
        let text = std::fs::read_to_string(file)
            .map_err(|error| ShortcutError(format!("{}: {error}", file.display())))?;
        let body: Value =
            serde_json::from_str(&text).map_err(|error| ShortcutError(error.to_string()))?;
        let field = |name: &str| body[name].as_str().unwrap_or("").to_owned();
        Ok(ShortcutInfo {
            target: field("target"),
            cwd: field("cwd"),
            args: String::new(),
            description: field("description"),
            icon: field("icon"),
            icon_index: body["iconIndex"].as_i64().unwrap_or(0) as i32,
            app_user_model_id: field("appUserModelId"),
        })
    }
}

/// Какой из двух ярлыков.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutKind {
    StartMenu,
    Desktop,
}

/// `wantsDesktopShortcut`: при установке человеком — его выбор; при обновлении (или без выбора) —
/// значение `HLSRDesktopShortcut` из реестра: ярлык нужен, если там не `"0"`.
pub fn wants_desktop_shortcut(
    registry_value: Option<&str>,
    update: bool,
    choice: Option<bool>,
) -> bool {
    if !update && let Some(choice) = choice {
        return choice;
    }
    registry_value != Some("0")
}

/// Ярлыки установки: пути и операции над ними.
pub struct Shortcuts {
    links: Arc<dyn ShellLinks>,
    /// `<Меню «Пуск»>\Programs\<имя>.lnk`.
    pub start_menu: PathBuf,
    /// `<Рабочий стол>\<имя>.lnk`.
    pub desktop: PathBuf,
    app_id: String,
    product_name: String,
}

impl Shortcuts {
    /// Пути ярлыков берутся у известных папок; не определились — ошибка (как `app.getPath`).
    pub fn new(
        links: Arc<dyn ShellLinks>,
        folders: &dyn KnownFolders,
        identity: &Identity,
    ) -> io::Result<Shortcuts> {
        let name = format!("{}.lnk", identity.shortcut_name);
        Ok(Shortcuts {
            links,
            start_menu: folders.start_menu_programs()?.join(&name),
            desktop: folders.desktop()?.join(&name),
            app_id: identity.app_id.clone(),
            product_name: identity.product_name.clone(),
        })
    }

    /// Создать ярлык на `exe`: цель, рабочий каталог — каталог `exe`, значок 0, AUMID `appId`,
    /// описание — имя продукта.
    pub fn write(&self, file: &Path, exe: &str) -> Result<(), ShortcutError> {
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| ShortcutError(format!("{}: {error}", parent.display())))?;
        }
        let spec = ShortcutSpec {
            target: exe.to_owned(),
            args: String::new(),
            cwd: win_path().dirname(exe),
            icon: exe.to_owned(),
            icon_index: 0,
            app_user_model_id: self.app_id.clone(),
            description: self.product_name.clone(),
        };
        self.links
            .write(file, &spec)
            .map_err(|_| ShortcutError(format!("Не удалось создать ярлык {}", file.display())))
    }

    /// Ярлык ведёт на наш `exe`. Чужой `HLSR.lnk` остаётся на месте: удаляем только свои.
    pub fn points_to(&self, file: &Path, exe: &str) -> bool {
        self.links
            .read(file)
            .is_ok_and(|info| layout::is_same_dir(&info.target, exe))
    }

    pub fn exists(&self, file: &Path) -> bool {
        file.exists()
    }

    /// Ярлыки при установке: при установке человеком — по его выбору, при обновлении недостающие
    /// восстанавливаются, существующие не трогаются. Возвращает предупреждения (`install.js`).
    pub fn install(&self, exe: &str, update: bool, create_desktop: bool) -> Vec<String> {
        let wanted = [(&self.start_menu, true), (&self.desktop, create_desktop)];
        let mut warnings = Vec::new();
        for (file, want) in wanted {
            if !want || (update && self.exists(file)) {
                continue;
            }
            if let Err(error) = self.write(file, exe) {
                warnings.push(error.to_string());
            }
        }
        warnings
    }

    /// Удаление: свои ярлыки (оба), чужие не трогаем. Предупреждения — «Ярлык не удалён: …».
    pub fn remove_own(&self, exe: &str) -> Vec<String> {
        let mut warnings = Vec::new();
        for file in [&self.start_menu, &self.desktop] {
            if !self.points_to(file, exe) {
                continue;
            }
            if std::fs::remove_file(file).is_err() {
                warnings.push(format!("Ярлык не удалён: {}", file.display()));
            }
        }
        warnings
    }
}

/// Ярлыки ОС: `IShellLinkW` + `IPropertyStore` (AUMID) — как `shell.writeShortcutLink(file,
/// "create", options)` и `shell.readShortcutLink(file)` Electron (`base::win::
/// CreateOrUpdateShortcutLink`, `ResolveShortcutProperties`): файл создаётся или заменяется,
/// оболочка получает `SHChangeNotify`, цель читается с `SLGP_UNCPRIORITY`.
#[cfg(windows)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemShellLinks;

#[cfg(windows)]
mod system {
    use super::{ShellLinks, ShortcutError, ShortcutInfo, ShortcutSpec, SystemShellLinks};
    use ::windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
    use ::windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use ::windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize, IPersistFile, STGM_READ,
    };
    use ::windows::Win32::System::Variant::VT_LPWSTR;
    use ::windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
    use ::windows::Win32::UI::Shell::{
        IShellLinkW, SHCNE_CREATE, SHCNE_UPDATEITEM, SHCNF_FLUSH, SHCNF_PATHW, SHChangeNotify,
        SHStrDupW, SLGP_UNCPRIORITY, ShellLink,
    };
    use ::windows::core::{BSTR, Interface, PCWSTR};
    use hlsr_core::win32::wide;
    use std::path::Path;

    /// COM на потоке на время вызова; `S_FALSE` (уже запущен) тоже требует `CoUninitialize`.
    struct Com(bool);

    impl Com {
        fn new() -> Com {
            // SAFETY: инициализация COM текущего потока; пара CoUninitialize — в Drop.
            Com(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok())
        }
    }

    impl Drop for Com {
        fn drop(&mut self) {
            if self.0 {
                // SAFETY: парный вызов к успешному CoInitializeEx этого же потока.
                unsafe { CoUninitialize() };
            }
        }
    }

    /// `PROPVARIANT` типа `VT_LPWSTR` (то, что кладёт `InitPropVariantFromString`): хранилище
    /// свойств ярлыка принимает AUMID как строку, а не `VT_BSTR`, которую делает `From<&str>`.
    fn propvariant_string(text: &str) -> ::windows::core::Result<PROPVARIANT> {
        let wide = wide(text);
        // SAFETY: строка завершена нулём; копия выделена CoTaskMemAlloc, её освободит PropVariantClear.
        let copy = unsafe { SHStrDupW(PCWSTR(wide.as_ptr())) }?;
        let mut value = PROPVARIANT::default();
        // SAFETY: значение только что создано пустым; пишем тип и указатель в общий союз.
        unsafe {
            let inner = &mut *value.Anonymous.Anonymous;
            inner.vt = VT_LPWSTR;
            inner.Anonymous.pwszVal = copy;
        }
        Ok(value)
    }

    fn text_of(buffer: &[u16]) -> String {
        let end = buffer
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(buffer.len());
        String::from_utf16_lossy(&buffer[..end])
    }

    fn write_link(file: &Path, spec: &ShortcutSpec) -> ::windows::core::Result<()> {
        let _com = Com::new();
        let text = |value: &str| wide(value);
        let (target, args, cwd, description, icon) = (
            text(&spec.target),
            text(&spec.args),
            text(&spec.cwd),
            text(&spec.description),
            text(&spec.icon),
        );
        let path = wide(file.as_os_str());
        let existed = file.exists();
        // SAFETY: все строки завершены нулём и живут до конца вызова; интерфейсы освобождаются до CoUninitialize.
        unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.SetPath(PCWSTR(target.as_ptr()))?;
            link.SetWorkingDirectory(PCWSTR(cwd.as_ptr()))?;
            link.SetArguments(PCWSTR(args.as_ptr()))?;
            link.SetDescription(PCWSTR(description.as_ptr()))?;
            link.SetIconLocation(PCWSTR(icon.as_ptr()), spec.icon_index)?;
            if !spec.app_user_model_id.is_empty() {
                let store: IPropertyStore = link.cast()?;
                let id = propvariant_string(&spec.app_user_model_id)?;
                store.SetValue(&PKEY_AppUserModel_ID, &id)?;
                store.Commit()?;
            }
            let persist: IPersistFile = link.cast()?;
            persist.Save(PCWSTR(path.as_ptr()), true)?;
        }
        // Как `base::win::CreateOrUpdateShortcutLink` (единственный вызов `SHChangeNotify` в
        // electron.exe 44.5.1; сверено по коду exe и по уведомлениям оболочки): после удачного
        // `Save` и освобождения интерфейсов — `SHCNE_UPDATEITEM`, если файл был, иначе
        // `SHCNE_CREATE`; `SHCNF_PATH | SHCNF_FLUSH` — вызов ждёт, пока оболочка примет уведомление.
        let event = if existed {
            SHCNE_UPDATEITEM
        } else {
            SHCNE_CREATE
        };
        // SAFETY: путь — строка UTF-16 с нулём на конце (`SHCNF_PATHW`), живёт до конца вызова.
        unsafe {
            SHChangeNotify(
                event,
                SHCNF_PATHW | SHCNF_FLUSH,
                Some(path.as_ptr().cast()),
                None,
            )
        };
        Ok(())
    }

    fn read_link(file: &Path) -> ::windows::core::Result<ShortcutInfo> {
        let _com = Com::new();
        let path = wide(file.as_os_str());
        let mut info = ShortcutInfo::default();
        // SAFETY: буферы вмещают заявленные длины; интерфейсы освобождаются до CoUninitialize.
        unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            let persist: IPersistFile = link.cast()?;
            persist.Load(PCWSTR(path.as_ptr()), STGM_READ)?;
            let mut buffer = vec![0u16; 32768];
            link.GetPath(&mut buffer, std::ptr::null_mut(), SLGP_UNCPRIORITY.0 as u32)?;
            info.target = text_of(&buffer);
            link.GetWorkingDirectory(&mut buffer)?;
            info.cwd = text_of(&buffer);
            link.GetArguments(&mut buffer)?;
            info.args = text_of(&buffer);
            link.GetDescription(&mut buffer)?;
            info.description = text_of(&buffer);
            let mut index = 0i32;
            link.GetIconLocation(&mut buffer, &mut index)?;
            info.icon = text_of(&buffer);
            info.icon_index = index;
            let store: IPropertyStore = link.cast()?;
            let id = store.GetValue(&PKEY_AppUserModel_ID)?;
            if !id.is_empty() {
                info.app_user_model_id = BSTR::try_from(&id)?.to_string();
            }
        }
        Ok(info)
    }

    impl ShellLinks for SystemShellLinks {
        fn write(&self, file: &Path, spec: &ShortcutSpec) -> Result<(), ShortcutError> {
            write_link(file, spec)
                .map_err(|error| ShortcutError(format!("{}: {error}", file.display())))
        }

        fn read(&self, file: &Path) -> Result<ShortcutInfo, ShortcutError> {
            read_link(file).map_err(|error| ShortcutError(format!("{}: {error}", file.display())))
        }
    }
}
