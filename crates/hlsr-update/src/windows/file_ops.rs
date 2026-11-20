//! Операции с файлами, которые установка делает так же, как Node 0.4.7 на Windows: переименование
//! `MoveFileExW` (`fs.renameSync` libuv), junction для ссылки на каталог (`fs.symlinkSync(…,
//! "junction")`), случайные суффиксы и идентификатор установки (`crypto`).

use std::io;
use std::path::Path;

/// `fs.renameSync(from, to)`: на Windows — `MoveFileExW(…, MOVEFILE_REPLACE_EXISTING)`, как у libuv;
/// коды ошибок те же (`EBUSY`, `EPERM`, `EACCES` у занятого каталога).
pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use hlsr_core::win32::wide;
        use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MoveFileExW};
        let (from, to) = (wide(from), wide(to));
        // SAFETY: обе строки завершены нулём и живут до конца вызова.
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_REPLACE_EXISTING) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(from, to)
    }
}

/// `n` случайных байт.
fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).expect("случайные байты ОС");
    bytes
}

/// `crypto.randomBytes(4).toString("hex")`: суффикс `.hlsr-new-<8hex>`.
pub fn random_suffix() -> String {
    hlsr_core::js::text::hex(&random_bytes::<4>())
}

/// `{${crypto.randomUUID().toUpperCase()}}`: UUID версии 4 заглавными в фигурных скобках.
pub fn new_install_id() -> String {
    let mut bytes = random_bytes::<16>();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hlsr_core::js::text::hex(&bytes).to_uppercase();
    format!(
        "{{{}-{}-{}-{}-{}}}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// `fs.symlinkSync(target, link, directory && win32 ? "junction" : undefined)`: на Windows ссылка на
/// каталог — junction (прав не требует), на файл — символическая ссылка; на прочих ОС — ссылка.
pub fn symlink_like_node(target: &Path, link: &Path, directory: bool) -> io::Result<()> {
    #[cfg(windows)]
    {
        if directory {
            create_junction(target, link)
        } else {
            std::os::windows::fs::symlink_file(target, link)
        }
    }
    #[cfg(unix)]
    {
        let _ = directory;
        std::os::unix::fs::symlink(target, link)
    }
}

/// `fs.readlinkSync`: цель ссылки; у junction без префикса `\\?\` (как отдаёт Node).
pub fn read_link(link: &Path) -> io::Result<std::path::PathBuf> {
    let target = std::fs::read_link(link)?;
    let text = target.to_string_lossy();
    Ok(match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => rest.into(),
        _ => target,
    })
}

/// Junction `link` → `target` (точка подключения NTFS): каталог `link` создаётся, в него пишется
/// точка повторного анализа `IO_REPARSE_TAG_MOUNT_POINT`. Относительная цель считается от каталога
/// ссылки (как `path.resolve(link, "..", target)` у Node).
#[cfg(windows)]
pub fn create_junction(target: &Path, link: &Path) -> io::Result<()> {
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_REPARSE_POINT;
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;

    let base = link.parent().unwrap_or(Path::new(""));
    let absolute = std::path::absolute(base.join(target))?;
    let mut plain = absolute.to_string_lossy().into_owned();
    if let Some(rest) = plain.strip_prefix(r"\\?\") {
        plain = rest.to_owned();
    }
    let substitute: Vec<u16> = format!(r"\??\{plain}").encode_utf16().collect();
    let print: Vec<u16> = plain.encode_utf16().collect();

    // REPARSE_DATA_BUFFER (MountPointReparseBuffer): заголовок 8 байт, 4 смещения/длины по u16,
    // затем обе строки с нулями на конце.
    let sub_bytes = substitute.len() * 2;
    let print_bytes = print.len() * 2;
    let path_buffer = sub_bytes + 2 + print_bytes + 2;
    let data_len = 8 + path_buffer;
    let mut buffer: Vec<u8> = Vec::with_capacity(8 + data_len);
    buffer.extend_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
    buffer.extend_from_slice(&(data_len as u16).to_le_bytes());
    buffer.extend_from_slice(&0u16.to_le_bytes());
    buffer.extend_from_slice(&0u16.to_le_bytes());
    buffer.extend_from_slice(&(sub_bytes as u16).to_le_bytes());
    buffer.extend_from_slice(&((sub_bytes + 2) as u16).to_le_bytes());
    buffer.extend_from_slice(&(print_bytes as u16).to_le_bytes());
    for unit in substitute.iter().chain(&[0]).chain(&print).chain(&[0]) {
        buffer.extend_from_slice(&unit.to_le_bytes());
    }

    std::fs::create_dir(link)?;
    let wide = hlsr_core::win32::wide(link);
    // SAFETY: строка пути завершена нулём; дескриптор закрывается ниже.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let error = io::Error::last_os_error();
        let _ = std::fs::remove_dir(link);
        return Err(error);
    }
    let mut returned = 0u32;
    // SAFETY: буфер жив до конца вызова, длина указана верно; дескриптор открыт на запись.
    let ok = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_SET_REPARSE_POINT,
            buffer.as_ptr().cast(),
            buffer.len() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    let error = (ok == 0).then(io::Error::last_os_error);
    // SAFETY: дескриптор получен от CreateFileW и закрывается один раз.
    unsafe { CloseHandle(handle) };
    if let Some(error) = error {
        let _ = std::fs::remove_dir(link);
        return Err(error);
    }
    Ok(())
}
