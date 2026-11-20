//! Модуль `path` Node 24 (как в Electron 0.4.7): `resolve`, `normalize`, `join`,
//! `basename`, `extname` и `dirname` для `path.posix` и `path.win32`, строка в строку.
//! Строки обрабатываются по единицам UTF-16, как в JS.

use super::Platform;

const SLASH: u16 = b'/' as u16;
const BACKSLASH: u16 = b'\\' as u16;
const DOT: u16 = b'.' as u16;
const COLON: u16 = b':' as u16;

/// `path` одной ОС (`path.posix` или `path.win32`) и текущий каталог процесса,
/// от которого `resolve` строит относительные пути.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodePath {
    platform: Platform,
    cwd: String,
}

impl NodePath {
    /// `path` ОС `platform` с текущим каталогом `cwd` (как `process.cwd()`).
    pub fn new(platform: Platform, cwd: impl Into<String>) -> NodePath {
        NodePath {
            platform,
            cwd: cwd.into(),
        }
    }

    /// `path` этой ОС и текущий каталог процесса. Если каталог не прочитался —
    /// пустая строка: `resolve` тогда оставляет относительный путь относительным.
    pub fn native() -> NodePath {
        let cwd = std::env::current_dir()
            .map(|dir| dir.to_string_lossy().into_owned())
            .unwrap_or_default();
        NodePath::new(Platform::CURRENT, cwd)
    }

    pub fn platform(&self) -> Platform {
        self.platform
    }

    /// `path.sep`.
    pub fn sep(&self) -> &'static str {
        match self.platform {
            Platform::Win32 => "\\",
            Platform::Linux => "/",
        }
    }

    /// `path.resolve(...paths)`.
    pub fn resolve(&self, paths: &[&str]) -> String {
        let paths: Vec<Vec<u16>> = paths.iter().map(|p| units(p)).collect();
        let cwd = units(&self.cwd);
        text(&match self.platform {
            Platform::Win32 => win32_resolve(&paths, &cwd),
            Platform::Linux => posix_resolve(&paths, &cwd),
        })
    }

    /// `path.normalize(path)`.
    pub fn normalize(&self, path: &str) -> String {
        text(&match self.platform {
            Platform::Win32 => win32_normalize(&units(path)),
            Platform::Linux => posix_normalize(&units(path)),
        })
    }

    /// `path.join(...paths)`.
    pub fn join(&self, paths: &[&str]) -> String {
        let paths: Vec<Vec<u16>> = paths.iter().map(|p| units(p)).collect();
        text(&match self.platform {
            Platform::Win32 => win32_join(&paths),
            Platform::Linux => posix_join(&paths),
        })
    }

    /// `path.toNamespacedPath(path)`: в Windows — длинный путь `\\?\C:\…` или
    /// `\\?\UNC\сервер\…` после `resolve`; в Linux — путь как есть.
    pub fn to_namespaced_path(&self, path: &str) -> String {
        if self.platform == Platform::Linux || path.is_empty() {
            return path.to_owned();
        }
        let resolved = self.resolve(&[path]);
        let units = units(&resolved);
        if units.len() <= 2 {
            return path.to_owned();
        }
        if units[0] == BACKSLASH {
            if units[1] == BACKSLASH && units[2] != u16::from(b'?') && units[2] != DOT {
                return format!("\\\\?\\UNC\\{}", text(&units[2..]));
            }
        } else if is_device_root(units[0]) && units[1] == COLON && units[2] == BACKSLASH {
            return format!("\\\\?\\{resolved}");
        }
        resolved
    }

    /// `path.basename(path)` (без второго аргумента).
    pub fn basename(&self, path: &str) -> String {
        let path = units(path);
        let (start, is_sep): (usize, fn(u16) -> bool) = match self.platform {
            // Буква диска не считается частью имени.
            Platform::Win32 if path.len() >= 2 && is_device_root(path[0]) && path[1] == COLON => {
                (2, is_win32_sep)
            }
            Platform::Win32 => (0, is_win32_sep),
            Platform::Linux => (0, is_posix_sep),
        };
        let mut begin = start;
        let mut end: Option<usize> = None;
        for i in (start..path.len()).rev() {
            if is_sep(path[i]) {
                if end.is_some() {
                    begin = i + 1;
                    break;
                }
            } else if end.is_none() {
                end = Some(i + 1);
            }
        }
        match end {
            Some(end) => text(&path[begin..end]),
            None => String::new(),
        }
    }

    /// `path.extname(path)`: от последней точки имени до его конца; имя из одних точек
    /// (`..`) и «скрытый» файл (`.run`) расширения не имеют.
    pub fn extname(&self, path: &str) -> String {
        let path = units(path);
        let (start, is_sep): (usize, fn(u16) -> bool) = match self.platform {
            // Буква диска не мешает: после неё разделитель — не «лишний» в конце.
            Platform::Win32 if path.len() >= 2 && path[1] == COLON && is_device_root(path[0]) => {
                (2, is_win32_sep)
            }
            Platform::Win32 => (0, is_win32_sep),
            Platform::Linux => (0, is_posix_sep),
        };
        let mut start_dot: Option<usize> = None;
        let mut start_part = start;
        let mut end: Option<usize> = None;
        let mut matched_slash = true;
        // Что видели до первой точки справа: 0 — ничего, 1 — точку, -1 — другой символ.
        let mut pre_dot_state = 0;
        for i in (start..path.len()).rev() {
            let code = path[i];
            if is_sep(code) {
                if !matched_slash {
                    start_part = i + 1;
                    break;
                }
                continue;
            }
            if end.is_none() {
                matched_slash = false;
                end = Some(i + 1);
            }
            if code == DOT {
                if start_dot.is_none() {
                    start_dot = Some(i);
                } else if pre_dot_state != 1 {
                    pre_dot_state = 1;
                }
            } else if start_dot.is_some() {
                pre_dot_state = -1;
            }
        }
        match (start_dot, end) {
            (Some(dot), Some(end))
                if pre_dot_state != 0
                    && !(pre_dot_state == 1 && dot == end - 1 && dot == start_part + 1) =>
            {
                text(&path[dot..end])
            }
            _ => String::new(),
        }
    }

    /// `path.isAbsolute(path)`: в Linux — начинается с `/`; в Windows — начинается с
    /// разделителя или с буквы диска, двоеточия и разделителя (`C:\\x`, но не `C:x`).
    pub fn is_absolute(&self, path: &str) -> bool {
        let path = units(path);
        match self.platform {
            Platform::Linux => path.first().is_some_and(|&code| is_posix_sep(code)),
            Platform::Win32 => match path.as_slice() {
                [] => false,
                [first, ..] if is_win32_sep(*first) => true,
                [letter, colon, sep, ..] => {
                    is_device_root(*letter) && *colon == COLON && is_win32_sep(*sep)
                }
                _ => false,
            },
        }
    }

    /// `path.dirname(path)`.
    pub fn dirname(&self, path: &str) -> String {
        let path = units(path);
        text(&match self.platform {
            Platform::Win32 => win32_dirname(&path),
            Platform::Linux => posix_dirname(&path),
        })
    }
}

fn units(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// Обратно в строку. Разрезы идут только по ASCII-разделителям, суррогатные
/// пары остаются целыми.
fn text(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

fn lower(units: &[u16]) -> String {
    text(units).to_lowercase()
}

fn is_posix_sep(code: u16) -> bool {
    code == SLASH
}

fn is_win32_sep(code: u16) -> bool {
    code == SLASH || code == BACKSLASH
}

fn is_device_root(code: u16) -> bool {
    (u16::from(b'A')..=u16::from(b'Z')).contains(&code)
        || (u16::from(b'a')..=u16::from(b'z')).contains(&code)
}

fn concat(parts: &[&[u16]]) -> Vec<u16> {
    parts.concat()
}

fn index_of(haystack: &[u16], needle: u16, from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .iter()
        .position(|&c| c == needle)
        .map(|at| at + from)
}

/// `String.prototype.slice(start, end)` с отрицательными индексами, как в JS.
fn js_slice(s: &[u16], start: isize, end: isize) -> &[u16] {
    let len = s.len() as isize;
    let clamp = |i: isize| if i < 0 { (len + i).max(0) } else { i.min(len) } as usize;
    let (start, end) = (clamp(start), clamp(end));
    if start >= end { &[] } else { &s[start..end] }
}

const WINDOWS_RESERVED_NAMES: [&str; 28] = [
    "CON",
    "PRN",
    "AUX",
    "NUL",
    "COM1",
    "COM2",
    "COM3",
    "COM4",
    "COM5",
    "COM6",
    "COM7",
    "COM8",
    "COM9",
    "LPT1",
    "LPT2",
    "LPT3",
    "LPT4",
    "LPT5",
    "LPT6",
    "LPT7",
    "LPT8",
    "LPT9",
    "COM\u{b9}",
    "COM\u{b2}",
    "COM\u{b3}",
    "LPT\u{b9}",
    "LPT\u{b2}",
    "LPT\u{b3}",
];

/// `isWindowsReservedName(path, colonIndex)`: `colonIndex = -1` режет последний символ, как `slice`.
fn is_windows_reserved_name(path: &[u16], colon_index: isize) -> bool {
    let device = text(js_slice(path, 0, colon_index)).to_uppercase();
    WINDOWS_RESERVED_NAMES.contains(&device.as_str())
}

/// `normalizeString`: убирает `.` и `..` в пути с одинаковыми разделителями.
fn normalize_string(
    path: &[u16],
    allow_above_root: bool,
    separator: u16,
    is_sep: fn(u16) -> bool,
) -> Vec<u16> {
    let mut res: Vec<u16> = Vec::new();
    let mut last_segment_length: isize = 0;
    let mut last_slash: isize = -1;
    let mut dots: i32 = 0;
    let mut code: u16 = 0;
    for i in 0..=path.len() {
        if i < path.len() {
            code = path[i];
        } else if is_sep(code) {
            break;
        } else {
            code = SLASH;
        }
        let at = i as isize;
        if is_sep(code) {
            if last_slash == at - 1 || dots == 1 {
                // Пустой сегмент или `.`.
            } else if dots == 2 {
                let ends_with_dots = res.len() >= 2
                    && last_segment_length == 2
                    && res[res.len() - 1] == DOT
                    && res[res.len() - 2] == DOT;
                if !ends_with_dots {
                    if res.len() > 2 {
                        let last_slash_index = res.len() as isize - last_segment_length - 1;
                        if last_slash_index == -1 {
                            res.clear();
                            last_segment_length = 0;
                        } else {
                            res.truncate(last_slash_index as usize);
                            let previous = res.iter().rposition(|&c| c == separator);
                            last_segment_length =
                                res.len() as isize - 1 - previous.map_or(-1, |p| p as isize);
                        }
                        last_slash = at;
                        dots = 0;
                        continue;
                    } else if !res.is_empty() {
                        res.clear();
                        last_segment_length = 0;
                        last_slash = at;
                        dots = 0;
                        continue;
                    }
                }
                if allow_above_root {
                    if !res.is_empty() {
                        res.push(separator);
                    }
                    res.extend([DOT, DOT]);
                    last_segment_length = 2;
                }
            } else {
                if !res.is_empty() {
                    res.push(separator);
                }
                res.extend_from_slice(&path[(last_slash + 1) as usize..i]);
                last_segment_length = at - last_slash - 1;
            }
            last_slash = at;
            dots = 0;
        } else if code == DOT && dots != -1 {
            dots += 1;
        } else {
            dots = -1;
        }
    }
    res
}

fn posix_resolve(paths: &[Vec<u16>], cwd: &[u16]) -> Vec<u16> {
    let only_here =
        paths.is_empty() || (paths.len() == 1 && (paths[0].is_empty() || paths[0] == [DOT]));
    if only_here && cwd.first() == Some(&SLASH) {
        return cwd.to_vec();
    }
    let mut resolved: Vec<u16> = Vec::new();
    let mut absolute = false;
    for path in paths.iter().rev() {
        if absolute {
            break;
        }
        if path.is_empty() {
            continue;
        }
        resolved = concat(&[path, &[SLASH], &resolved]);
        absolute = path[0] == SLASH;
    }
    if !absolute {
        resolved = concat(&[cwd, &[SLASH], &resolved]);
        absolute = cwd.first() == Some(&SLASH);
    }
    let resolved = normalize_string(&resolved, !absolute, SLASH, is_posix_sep);
    if absolute {
        concat(&[&[SLASH], &resolved])
    } else if resolved.is_empty() {
        vec![DOT]
    } else {
        resolved
    }
}

fn posix_normalize(path: &[u16]) -> Vec<u16> {
    if path.is_empty() {
        return vec![DOT];
    }
    let absolute = path[0] == SLASH;
    let trailing = path[path.len() - 1] == SLASH;
    let mut out = normalize_string(path, !absolute, SLASH, is_posix_sep);
    if out.is_empty() {
        return if absolute {
            vec![SLASH]
        } else if trailing {
            vec![DOT, SLASH]
        } else {
            vec![DOT]
        };
    }
    if trailing {
        out.push(SLASH);
    }
    if absolute {
        concat(&[&[SLASH], &out])
    } else {
        out
    }
}

fn posix_join(paths: &[Vec<u16>]) -> Vec<u16> {
    let parts: Vec<&[u16]> = paths
        .iter()
        .filter(|p| !p.is_empty())
        .map(Vec::as_slice)
        .collect();
    if parts.is_empty() {
        return vec![DOT];
    }
    posix_normalize(&parts.join(&SLASH))
}

/// Корень пути Windows: `\\сервер\ресурс`, `\\?\…`, `C:`, `\`.
struct Win32Root {
    device: Vec<u16>,
    root_end: usize,
    is_absolute: bool,
}

/// Разбор корня, как в `win32.resolve` (без особых случаев `normalize`).
fn win32_resolve_root(path: &[u16]) -> Win32Root {
    let len = path.len();
    let mut root = Win32Root {
        device: Vec::new(),
        root_end: 0,
        is_absolute: false,
    };
    let Some(&code) = path.first() else {
        return root;
    };
    if len == 1 {
        if is_win32_sep(code) {
            root.root_end = 1;
            root.is_absolute = true;
        }
    } else if is_win32_sep(code) {
        root.is_absolute = true;
        if is_win32_sep(path[1]) {
            if let Some(unc) = match_unc(path) {
                if unc.first_part != [DOT] && unc.first_part != [u16::from(b'?')] {
                    root.device = concat(&[
                        &[BACKSLASH, BACKSLASH],
                        unc.first_part,
                        &[BACKSLASH],
                        &path[unc.share_start..unc.share_end],
                    ]);
                    root.root_end = unc.share_end;
                } else {
                    root.device = concat(&[&[BACKSLASH, BACKSLASH], unc.first_part]);
                    root.root_end = 4;
                }
            }
        } else {
            root.root_end = 1;
        }
    } else if is_device_root(code) && path[1] == COLON {
        root.device = path[..2].to_vec();
        root.root_end = 2;
        if len > 2 && is_win32_sep(path[2]) {
            root.is_absolute = true;
            root.root_end = 3;
        }
    }
    root
}

/// `\\сервер\ресурс…`: имя сервера и границы имени ресурса (если оба есть).
struct Unc<'a> {
    first_part: &'a [u16],
    share_start: usize,
    share_end: usize,
}

fn match_unc(path: &[u16]) -> Option<Unc<'_>> {
    let len = path.len();
    let mut j = 2;
    let mut last = j;
    while j < len && !is_win32_sep(path[j]) {
        j += 1;
    }
    if j >= len || j == last {
        return None;
    }
    let first_part = &path[last..j];
    last = j;
    while j < len && is_win32_sep(path[j]) {
        j += 1;
    }
    if j >= len || j == last {
        return None;
    }
    last = j;
    while j < len && !is_win32_sep(path[j]) {
        j += 1;
    }
    (j == len || j != last).then_some(Unc {
        first_part,
        share_start: last,
        share_end: j,
    })
}

fn win32_resolve(paths: &[Vec<u16>], cwd: &[u16]) -> Vec<u16> {
    let mut resolved_device: Vec<u16> = Vec::new();
    let mut resolved_tail: Vec<u16> = Vec::new();
    let mut resolved_absolute = false;

    let mut i = paths.len() as isize - 1;
    while i >= -1 {
        let path: Vec<u16> = if i >= 0 {
            let path = &paths[i as usize];
            if path.is_empty() {
                i -= 1;
                continue;
            }
            path.clone()
        } else if resolved_device.is_empty() {
            let only_here = paths.is_empty()
                || (paths.len() == 1
                    && (paths[0].is_empty() || paths[0] == [DOT])
                    && cwd.first().copied().is_some_and(is_win32_sep));
            if only_here {
                // Не на Windows текущий каталог — путь POSIX: `/` меняются на `\`.
                return cwd
                    .iter()
                    .map(|&c| if c == SLASH { BACKSLASH } else { c })
                    .collect();
            }
            cwd.to_vec()
        } else {
            // Node берёт текущий каталог диска из `process.env["=C:"]`. Rust не читает
            // переменные с `=` в имени; без неё Node, как и здесь, берёт cwd процесса
            // или корень диска. Касается только путей вида `D:foo` на чужой диск.
            let other_drive = lower(js_slice(cwd, 0, 2)) != lower(&resolved_device)
                && cwd.get(2) == Some(&BACKSLASH);
            if other_drive {
                concat(&[&resolved_device, &[BACKSLASH]])
            } else {
                cwd.to_vec()
            }
        };

        let root = win32_resolve_root(&path);
        if !root.device.is_empty() {
            if !resolved_device.is_empty() {
                if lower(&root.device) != lower(&resolved_device) {
                    // Путь на другом устройстве — не к месту.
                    i -= 1;
                    continue;
                }
            } else {
                resolved_device = root.device.clone();
            }
        }

        if resolved_absolute {
            if !resolved_device.is_empty() {
                break;
            }
        } else {
            resolved_tail = concat(&[&path[root.root_end..], &[BACKSLASH], &resolved_tail]);
            resolved_absolute = root.is_absolute;
            if root.is_absolute && !resolved_device.is_empty() {
                break;
            }
        }
        i -= 1;
    }

    let tail = normalize_string(&resolved_tail, !resolved_absolute, BACKSLASH, is_win32_sep);
    if resolved_absolute {
        concat(&[&resolved_device, &[BACKSLASH], &tail])
    } else {
        let out = concat(&[&resolved_device, &tail]);
        if out.is_empty() { vec![DOT] } else { out }
    }
}

fn win32_normalize(path: &[u16]) -> Vec<u16> {
    let len = path.len();
    if len == 0 {
        return vec![DOT];
    }
    let code = path[0];
    if len == 1 {
        return if is_posix_sep(code) {
            vec![BACKSLASH]
        } else {
            path.to_vec()
        };
    }
    let mut root_end = 0;
    let mut device: Option<Vec<u16>> = None;
    let mut is_absolute = false;

    if is_win32_sep(code) {
        is_absolute = true;
        if is_win32_sep(path[1]) {
            if let Some(unc) = match_unc(path) {
                if unc.first_part == [DOT] || unc.first_part == [u16::from(b'?')] {
                    // Корень устройства (`\\.\PHYSICALDRIVE0`), особо — `\\?\COM1:`.
                    let mut dev = concat(&[&[BACKSLASH, BACKSLASH], unc.first_part]);
                    root_end = 4;
                    let colon_index = index_of(path, COLON, 0).map_or(-1, |c| c as isize);
                    let possible = js_slice(path, 4, colon_index + 1);
                    if is_windows_reserved_name(possible, possible.len() as isize - 1) {
                        dev = concat(&[&units("\\\\?\\"), possible]);
                        root_end = 4 + possible.len();
                    }
                    device = Some(dev);
                } else if unc.share_end == len {
                    // Только корень UNC: дальше разбирать нечего.
                    return concat(&[
                        &[BACKSLASH, BACKSLASH],
                        unc.first_part,
                        &[BACKSLASH],
                        &path[unc.share_start..],
                        &[BACKSLASH],
                    ]);
                } else {
                    device = Some(concat(&[
                        &[BACKSLASH, BACKSLASH],
                        unc.first_part,
                        &[BACKSLASH],
                        &path[unc.share_start..unc.share_end],
                    ]));
                    root_end = unc.share_end;
                }
            }
        } else {
            root_end = 1;
        }
    } else if let Some(colon_index) = index_of(path, COLON, 0).filter(|&c| c > 0) {
        if is_device_root(code) && colon_index == 1 {
            device = Some(path[..2].to_vec());
            root_end = 2;
            if len > 2 && is_win32_sep(path[2]) {
                is_absolute = true;
                root_end = 3;
            }
        } else if is_windows_reserved_name(path, colon_index as isize) {
            device = Some(path[..=colon_index].to_vec());
            root_end = colon_index + 1;
        }
    }

    let mut tail = if root_end < len {
        normalize_string(&path[root_end..], !is_absolute, BACKSLASH, is_win32_sep)
    } else {
        Vec::new()
    };
    if tail.is_empty() && !is_absolute {
        tail = vec![DOT];
    }
    if !tail.is_empty() && is_win32_sep(path[len - 1]) {
        tail.push(BACKSLASH);
    }
    let dot_tail = || concat(&[&[DOT, BACKSLASH], &tail]);
    if !is_absolute && device.is_none() && path.contains(&COLON) {
        // Относительный путь не должен стать похожим на абсолютный (CVE-2024-36139).
        if tail.len() >= 2 && is_device_root(tail[0]) && tail[1] == COLON {
            return dot_tail();
        }
        let mut index = index_of(path, COLON, 0);
        while let Some(at) = index {
            if at == len - 1 || is_win32_sep(path[at + 1]) {
                return dot_tail();
            }
            index = index_of(path, COLON, at + 1);
        }
    }
    let colon_index = index_of(path, COLON, 0).map_or(-1, |c| c as isize);
    if is_windows_reserved_name(path, colon_index) {
        let device = device.unwrap_or_default();
        return concat(&[&[DOT, BACKSLASH], &device, &tail]);
    }
    match device {
        None if is_absolute => concat(&[&[BACKSLASH], &tail]),
        None => tail,
        Some(device) if is_absolute => concat(&[&device, &[BACKSLASH], &tail]),
        Some(device) => concat(&[&device, &tail]),
    }
}

fn posix_dirname(path: &[u16]) -> Vec<u16> {
    if path.is_empty() {
        return vec![DOT];
    }
    let has_root = path[0] == SLASH;
    let mut end = None;
    let mut matched_slash = true;
    for i in (1..path.len()).rev() {
        if path[i] == SLASH {
            if !matched_slash {
                end = Some(i);
                break;
            }
        } else {
            matched_slash = false;
        }
    }
    match end {
        None if has_root => vec![SLASH],
        None => vec![DOT],
        Some(1) if has_root => vec![SLASH, SLASH],
        Some(end) => path[..end].to_vec(),
    }
}

fn win32_dirname(path: &[u16]) -> Vec<u16> {
    let len = path.len();
    if len == 0 {
        return vec![DOT];
    }
    let code = path[0];
    if len == 1 {
        return if is_win32_sep(code) {
            path.to_vec()
        } else {
            vec![DOT]
        };
    }
    let mut root_end: Option<usize> = None;
    let mut offset = 0;
    if is_win32_sep(code) {
        // Возможный корень UNC.
        root_end = Some(1);
        offset = 1;
        if is_win32_sep(path[1]) {
            let mut j = 2;
            let mut last = j;
            while j < len && !is_win32_sep(path[j]) {
                j += 1;
            }
            if j < len && j != last {
                last = j;
                while j < len && is_win32_sep(path[j]) {
                    j += 1;
                }
                if j < len && j != last {
                    last = j;
                    while j < len && !is_win32_sep(path[j]) {
                        j += 1;
                    }
                    if j == len {
                        // Только корень UNC.
                        return path.to_vec();
                    }
                    if j != last {
                        // Корень UNC с продолжением: разделитель после него — часть корня.
                        root_end = Some(j + 1);
                        offset = j + 1;
                    }
                }
            }
        }
    } else if is_device_root(code) && path[1] == COLON {
        // Возможный корень с буквой диска.
        let end = if len > 2 && is_win32_sep(path[2]) {
            3
        } else {
            2
        };
        root_end = Some(end);
        offset = end;
    }
    let mut end = None;
    let mut matched_slash = true;
    for i in (offset..len).rev() {
        if is_win32_sep(path[i]) {
            if !matched_slash {
                end = Some(i);
                break;
            }
        } else {
            matched_slash = false;
        }
    }
    let end = match (end, root_end) {
        (Some(end), _) => end,
        (None, None) => return vec![DOT],
        (None, Some(root_end)) => root_end,
    };
    path[..end].to_vec()
}

fn win32_join(paths: &[Vec<u16>]) -> Vec<u16> {
    let parts: Vec<&[u16]> = paths
        .iter()
        .filter(|p| !p.is_empty())
        .map(Vec::as_slice)
        .collect();
    let Some(first) = parts.first() else {
        return vec![DOT];
    };
    let mut joined = parts.join(&BACKSLASH);

    // Две косые черты в начале normalize принял бы за UNC: их сводят к одной,
    // если первая часть не похожа на UNC явно (`\\сервер`).
    let mut needs_replace = true;
    let mut slash_count = 0;
    if is_win32_sep(first[0]) {
        slash_count += 1;
        if first.len() > 1 && is_win32_sep(first[1]) {
            slash_count += 1;
            if first.len() > 2 {
                if is_win32_sep(first[2]) {
                    slash_count += 1;
                } else {
                    needs_replace = false;
                }
            }
        }
    }
    if needs_replace {
        while slash_count < joined.len() && is_win32_sep(joined[slash_count]) {
            slash_count += 1;
        }
        if slash_count >= 2 {
            joined = concat(&[&[BACKSLASH], &joined[slash_count..]]);
        }
    }

    // С зарезервированным именем устройства в части путь не нормализуется.
    let reserved = joined
        .split(|&c| c == BACKSLASH)
        .filter(|part| !part.is_empty())
        .any(|part| {
            index_of(part, COLON, 0)
                .is_some_and(|colon| is_windows_reserved_name(part, colon as isize))
        });
    if reserved {
        return joined
            .iter()
            .map(|&c| if c == SLASH { BACKSLASH } else { c })
            .collect();
    }
    win32_normalize(&joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn posix() -> NodePath {
        NodePath::new(Platform::Linux, "/home/u")
    }

    fn win32() -> NodePath {
        NodePath::new(Platform::Win32, "C:\\Users\\u")
    }

    #[test]
    fn resolve_collapses_dots_and_uses_cwd() {
        assert_eq!(posix().resolve(&["a/../b/./c/"]), "/home/u/b/c");
        assert_eq!(posix().resolve(&["/x", "../y"]), "/y");
        assert_eq!(
            win32().resolve(&["C:/HLSR/Games/../Tools/"]),
            "C:\\HLSR\\Tools"
        );
        assert_eq!(win32().resolve(&["Games"]), "C:\\Users\\u\\Games");
        assert_eq!(
            win32().resolve(&["\\\\srv\\share\\a\\..\\b"]),
            "\\\\srv\\share\\b"
        );
    }

    #[test]
    fn join_and_basename() {
        assert_eq!(posix().join(&["/a", "/etc/x.cfg"]), "/a/etc/x.cfg");
        assert_eq!(win32().join(&["C:\\a", "D:\\x"]), "C:\\a\\D:\\x");
        assert_eq!(
            win32().basename("C:\\Games\\hl1-won-1.0.0.7z"),
            "hl1-won-1.0.0.7z"
        );
        assert_eq!(win32().basename("C:"), "");
        assert_eq!(posix().basename("/a/b/"), "b");
    }
}
