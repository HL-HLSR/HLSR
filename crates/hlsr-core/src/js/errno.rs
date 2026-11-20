//! Коды и тексты ошибок ОС, как их показывает Node 24 (libuv): `error.code`
//! (`EBUSY`, `UNKNOWN`, `Unknown system error -61`…) и `error.message` ошибок `fs`.

use std::borrow::Cow;
use std::io;

/// Имя кода libuv (`uv_err_name`) и его текст (`uv_strerror`), с номером errno Linux,
/// если код приходит от ядра. Таблица — `util.getSystemErrorMap()` Node 24 из
/// Electron 0.4.7 (libuv 1.52) на Linux без `EAI_*`; сверяется в обе стороны с
/// `tests/fixtures/node-semantics.json`.
const UV_ERRORS: [(Option<i32>, &str, &str); 70] = [
    (Some(1), "EPERM", "operation not permitted"),
    (Some(2), "ENOENT", "no such file or directory"),
    (Some(3), "ESRCH", "no such process"),
    (Some(4), "EINTR", "interrupted system call"),
    (Some(5), "EIO", "i/o error"),
    (Some(6), "ENXIO", "no such device or address"),
    (Some(7), "E2BIG", "argument list too long"),
    (Some(8), "ENOEXEC", "exec format error"),
    (Some(9), "EBADF", "bad file descriptor"),
    (Some(11), "EAGAIN", "resource temporarily unavailable"),
    (Some(12), "ENOMEM", "not enough memory"),
    (Some(13), "EACCES", "permission denied"),
    (Some(14), "EFAULT", "bad address in system call argument"),
    (Some(16), "EBUSY", "resource busy or locked"),
    (Some(17), "EEXIST", "file already exists"),
    (Some(18), "EXDEV", "cross-device link not permitted"),
    (Some(19), "ENODEV", "no such device"),
    (Some(20), "ENOTDIR", "not a directory"),
    (Some(21), "EISDIR", "illegal operation on a directory"),
    (Some(22), "EINVAL", "invalid argument"),
    (Some(23), "ENFILE", "file table overflow"),
    (Some(24), "EMFILE", "too many open files"),
    (Some(25), "ENOTTY", "inappropriate ioctl for device"),
    (Some(26), "ETXTBSY", "text file is busy"),
    (Some(27), "EFBIG", "file too large"),
    (Some(28), "ENOSPC", "no space left on device"),
    (Some(29), "ESPIPE", "invalid seek"),
    (Some(30), "EROFS", "read-only file system"),
    (Some(31), "EMLINK", "too many links"),
    (Some(32), "EPIPE", "broken pipe"),
    (Some(34), "ERANGE", "result too large"),
    (Some(36), "ENAMETOOLONG", "name too long"),
    (Some(38), "ENOSYS", "function not implemented"),
    (Some(39), "ENOTEMPTY", "directory not empty"),
    (Some(40), "ELOOP", "too many symbolic links encountered"),
    (Some(49), "EUNATCH", "protocol driver not attached"),
    (Some(64), "ENONET", "machine is not on the network"),
    (Some(71), "EPROTO", "protocol error"),
    (
        Some(75),
        "EOVERFLOW",
        "value too large for defined data type",
    ),
    (Some(84), "EILSEQ", "illegal byte sequence"),
    (Some(88), "ENOTSOCK", "socket operation on non-socket"),
    (Some(89), "EDESTADDRREQ", "destination address required"),
    (Some(90), "EMSGSIZE", "message too long"),
    (Some(91), "EPROTOTYPE", "protocol wrong type for socket"),
    (Some(92), "ENOPROTOOPT", "protocol not available"),
    (Some(93), "EPROTONOSUPPORT", "protocol not supported"),
    (Some(94), "ESOCKTNOSUPPORT", "socket type not supported"),
    (Some(95), "ENOTSUP", "operation not supported on socket"),
    (Some(97), "EAFNOSUPPORT", "address family not supported"),
    (Some(98), "EADDRINUSE", "address already in use"),
    (Some(99), "EADDRNOTAVAIL", "address not available"),
    (Some(100), "ENETDOWN", "network is down"),
    (Some(101), "ENETUNREACH", "network is unreachable"),
    (
        Some(103),
        "ECONNABORTED",
        "software caused connection abort",
    ),
    (Some(104), "ECONNRESET", "connection reset by peer"),
    (Some(105), "ENOBUFS", "no buffer space available"),
    (Some(106), "EISCONN", "socket is already connected"),
    (Some(107), "ENOTCONN", "socket is not connected"),
    (
        Some(108),
        "ESHUTDOWN",
        "cannot send after transport endpoint shutdown",
    ),
    (Some(110), "ETIMEDOUT", "connection timed out"),
    (Some(111), "ECONNREFUSED", "connection refused"),
    (Some(112), "EHOSTDOWN", "host is down"),
    (Some(113), "EHOSTUNREACH", "host is unreachable"),
    (Some(114), "EALREADY", "connection already in progress"),
    (Some(121), "EREMOTEIO", "remote I/O error"),
    (Some(125), "ECANCELED", "operation canceled"),
    (None, "EFTYPE", "inappropriate file type or format"),
    (None, "ECHARSET", "invalid Unicode character"),
    (None, "UNKNOWN", "unknown error"),
    (None, "EOF", "end of file"),
];

/// `error.code` ошибки ОС, как у Node. Код, которого libuv не знает: в Linux —
/// `Unknown system error -N` (`uv_err_name`; так fs Node и пишет), в Windows — `UNKNOWN`
/// (libuv сводит незнакомое к UV_UNKNOWN; так приходит, например, «файл содержит вирус»).
/// `None` — ошибка не от ОС.
pub fn code(error: &io::Error) -> Option<Cow<'static, str>> {
    error.raw_os_error().map(os_code)
}

/// Текст libuv для кода (`uv_strerror`): у незнакомого кода Linux — то же
/// `Unknown system error -N`, у прочих незнакомых — `unknown error`.
pub fn description(code: &str) -> Cow<'static, str> {
    if let Some((_, _, text)) = UV_ERRORS.iter().find(|(_, name, _)| *name == code) {
        return Cow::Borrowed(text);
    }
    if code.starts_with(UNKNOWN_SYSTEM_ERROR) {
        return Cow::Owned(code.to_owned());
    }
    Cow::Borrowed("unknown error")
}

/// `error.message` ошибки `fs` Node: `EBUSY: resource busy or locked, open '<путь>'`,
/// для двух путей — `… copyfile '<откуда>' -> '<куда>'`. Ошибка не от ОС — её текст.
pub fn fs_message(error: &io::Error, syscall: &str, path: &str, dest: Option<&str>) -> String {
    let Some(code) = code(error) else {
        return error.to_string();
    };
    let mut message = format!("{code}: {}, {syscall} '{path}'", description(&code));
    if let Some(dest) = dest {
        message.push_str(&format!(" -> '{dest}'"));
    }
    message
}

const UNKNOWN_SYSTEM_ERROR: &str = "Unknown system error ";

#[cfg(unix)]
fn os_code(errno: i32) -> Cow<'static, str> {
    UV_ERRORS
        .iter()
        .find(|(number, _, _)| *number == Some(errno))
        .map_or_else(
            || Cow::Owned(format!("{UNKNOWN_SYSTEM_ERROR}-{errno}")),
            |(_, name, _)| Cow::Borrowed(*name),
        )
}

#[cfg(windows)]
fn os_code(error: i32) -> Cow<'static, str> {
    Cow::Borrowed(win32_code(error))
}

/// Перевод кода Win32 или WinSock в имя libuv (`uv_translate_sys_error`): таблица
/// снята с самого electron.exe 0.4.7 под Wine (`tools/reference/node-semantics-win32.js`)
/// и сверяется с `tests/fixtures/node-semantics-win32.json` для кодов 1..=16000.
pub fn win32_code(error: i32) -> &'static str {
    match error {
        208 => "E2BIG",
        740 | 1920 | 10013 => "EACCES",
        1227 | 10048 => "EADDRINUSE",
        10049 => "EADDRNOTAVAIL",
        10047 => "EAFNOSUPPORT",
        232 | 10035 => "EAGAIN",
        10037 => "EALREADY",
        6 | 1004 => "EBADF",
        32 | 33 | 231 => "EBUSY",
        995 | 10004 => "ECANCELED",
        1113 => "ECHARSET",
        1236 | 10053 => "ECONNABORTED",
        1225 | 10061 => "ECONNREFUSED",
        64 | 10054 => "ECONNRESET",
        80 | 183 => "EEXIST",
        998 | 10014 => "EFAULT",
        193 => "EFTYPE",
        1232 | 10065 => "EHOSTUNREACH",
        13 | 87 | 122 | 1464 | 10022 | 10046 => "EINVAL",
        23 | 31 | 110 | 156 | 205 | 1101 | 1102 | 1103 | 1104 | 1106 | 1111 | 1117 | 1129
        | 1165 | 1166 | 1393 => "EIO",
        10056 => "EISCONN",
        1 => "EISDIR",
        1921 => "ELOOP",
        4 | 10024 => "EMFILE",
        10040 => "EMSGSIZE",
        111 | 206 => "ENAMETOOLONG",
        1231 | 10051 => "ENETUNREACH",
        10055 => "ENOBUFS",
        2 | 3 | 15 | 123 | 126 | 161 | 203 | 267 | 4392 | 11001 | 11004 => "ENOENT",
        8 | 14 => "ENOMEM",
        39 | 82 | 112 | 277 | 1100 => "ENOSPC",
        2250 | 10057 => "ENOTCONN",
        145 => "ENOTEMPTY",
        10038 => "ENOTSOCK",
        50 => "ENOTSUP",
        109 => "EOF",
        5 | 1314 => "EPERM",
        230 | 233 | 10058 => "EPIPE",
        10043 => "EPROTONOSUPPORT",
        19 => "EROFS",
        10044 => "ESOCKTNOSUPPORT",
        121 | 10060 => "ETIMEDOUT",
        17 => "EXDEV",
        _ => "UNKNOWN",
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn codes_and_messages_follow_node() {
        let busy = io::Error::from_raw_os_error(16);
        assert_eq!(code(&busy).as_deref(), Some("EBUSY"));
        assert_eq!(
            fs_message(&busy, "open", "/x", None),
            "EBUSY: resource busy or locked, open '/x'"
        );
        assert_eq!(
            fs_message(
                &io::Error::from_raw_os_error(13),
                "copyfile",
                "/a",
                Some("/b")
            ),
            "EACCES: permission denied, copyfile '/a' -> '/b'"
        );
        assert_eq!(
            code(&io::Error::from_raw_os_error(200)).as_deref(),
            Some("Unknown system error -200")
        );
        assert_eq!(code(&io::Error::other("x")), None);
    }
}
