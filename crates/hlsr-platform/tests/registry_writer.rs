//! Запись `DXLevel_V1` в реестр Windows (`SystemRegistryWriter`, WinAPI вместо `reg.exe add`):
//! настоящая запись идёт в приватный куст приложения (`RegLoadAppKeyW` над временным файлом), так что
//! реестр пользователя не меняется. Куст `HKCU\Software\Valve\...` самого пользователя здесь не
//! трогается.

#![cfg(windows)]

use hlsr_core::win32::wide;
use hlsr_platform::depot_install::windows_registry::{set_dword_at, split_hive};
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS, KEY_READ, REG_DWORD, RegCloseKey,
    RegLoadAppKeyW, RegOpenKeyExW, RegQueryValueExW,
};

#[test]
fn hive_names_split_like_reg_exe_paths() {
    let (root, rest) = split_hive("HKCU\\Software\\Valve\\Source\\hl2\\Settings").unwrap();
    assert_eq!(root, HKEY_CURRENT_USER);
    assert_eq!(rest, "Software\\Valve\\Source\\hl2\\Settings");
    assert_eq!(split_hive("hkcu\\a").unwrap(), (HKEY_CURRENT_USER, "a"));
    assert_eq!(
        split_hive("HKEY_CURRENT_USER\\a\\b").unwrap(),
        (HKEY_CURRENT_USER, "a\\b")
    );
    assert_eq!(split_hive("HKLM\\a").unwrap(), (HKEY_LOCAL_MACHINE, "a"));
    assert_eq!(
        split_hive("HKEY_LOCAL_MACHINE\\a").unwrap(),
        (HKEY_LOCAL_MACHINE, "a")
    );
    assert_eq!(split_hive("HKCR\\a"), None);
    assert_eq!(split_hive("HKCU"), None);
    assert_eq!(split_hive(""), None);
}

fn read_dword(root: HKEY, subkey: &str, name: &str) -> Option<u32> {
    let wide_key = wide(subkey);
    let wide_name = wide(name);
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: строки завершены нулём, `key` — локальная переменная для дескриптора.
    let opened = unsafe { RegOpenKeyExW(root, wide_key.as_ptr(), 0, KEY_READ, &mut key) };
    if opened != ERROR_SUCCESS {
        return None;
    }
    let mut kind = 0u32;
    let mut data = [0u8; 4];
    let mut size = 4u32;
    // SAFETY: буфер на четыре байта, размер передан.
    let status = unsafe {
        RegQueryValueExW(
            key,
            wide_name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            data.as_mut_ptr(),
            &mut size,
        )
    };
    // SAFETY: дескриптор открыт выше.
    unsafe { RegCloseKey(key) };
    (status == ERROR_SUCCESS && kind == REG_DWORD && size == 4).then(|| u32::from_le_bytes(data))
}

#[test]
fn a_dword_is_created_and_overwritten_in_a_private_hive() {
    let dir = tempfile::tempdir().unwrap();
    let file = wide(&dir.path().join("test.hiv"));
    let mut root: HKEY = std::ptr::null_mut();
    // SAFETY: путь завершён нулём, `root` — локальная переменная для дескриптора.
    let status = unsafe { RegLoadAppKeyW(file.as_ptr(), &mut root, KEY_ALL_ACCESS, 0, 0) };
    assert_eq!(status, ERROR_SUCCESS, "RegLoadAppKeyW: код {status}");

    let subkey = "Software\\Valve\\Source\\hl2\\Settings";
    assert_eq!(read_dword(root, subkey, "DXLevel_V1"), None);

    // SAFETY: `root` — открытый приватный куст.
    unsafe { set_dword_at(root, subkey, "DXLevel_V1", 0x5f) }.unwrap();
    assert_eq!(read_dword(root, subkey, "DXLevel_V1"), Some(0x5f));

    // Повторная запись меняет значение, соседние значения не трогаются.
    unsafe { set_dword_at(root, subkey, "Other", 7) }.unwrap();
    unsafe { set_dword_at(root, subkey, "DXLevel_V1", 0x62) }.unwrap();
    assert_eq!(read_dword(root, subkey, "DXLevel_V1"), Some(0x62));
    assert_eq!(read_dword(root, subkey, "Other"), Some(7));

    // SAFETY: дескриптор открыт выше; куст выгружается вместе с ним.
    unsafe { RegCloseKey(root) };
}
