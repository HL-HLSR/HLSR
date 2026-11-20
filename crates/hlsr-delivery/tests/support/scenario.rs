//! Сценарии сверки с деревьями файлов: раскладка из данных, снимок дерева с текстом latin1,
//! нормализация путей в ответах. Зеркало `tools/reference/lib/scenario-fs.js`; те же сценарии
//! выполняют Node 24 (модули 0.4.7) и Rust.
//!
//! Элемент `setup`: `{file, latin1 | utf8 | hex | repeat: [кусок, раз], tail, mtime}`, `{dir}`,
//! `{symlink, target}`, `{generate: {dir, name, count, pad, latin1 | repeat, mtime, step}}`.

use super::{items, opt_str, str_of};
use hlsr_core::js;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

/// Время записи файлов без своего (секунды), как в сценарии.
pub const DEFAULT_MTIME: f64 = 1_600_000_000.0;
/// Текст файла попадает в снимок, если файл не больше.
pub const TEXT_LIMIT: u64 = 6000;

/// Содержимое файла из описания в сценарии.
pub fn bytes_of(item: &Value) -> Vec<u8> {
    if let Some(text) = opt_str(&item["latin1"]) {
        return js::latin1_bytes(text);
    }
    if let Some(text) = opt_str(&item["utf8"]) {
        return text.as_bytes().to_vec();
    }
    if let Some(hex) = opt_str(&item["hex"]) {
        return (0..hex.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
            .collect();
    }
    if let Some(repeat) = item["repeat"].as_array() {
        let text = str_of(&repeat[0]).repeat(repeat[1].as_u64().unwrap() as usize)
            + opt_str(&item["tail"]).unwrap_or_default();
        return js::latin1_bytes(&text);
    }
    Vec::new()
}

/// Крупный текст в шаге: строка или `{repeat: [кусок, раз], tail}`.
pub fn expand(text: &Value) -> String {
    match text {
        Value::String(text) => text.clone(),
        spec => {
            str_of(&spec["repeat"][0]).repeat(spec["repeat"][1].as_u64().unwrap() as usize)
                + opt_str(&spec["tail"]).unwrap_or_default()
        }
    }
}

/// `fs.utimesSync(file, seconds, seconds)` Node в Linux: libuv переводит секунды в
/// `timespec` — целая часть, дробная в наносекундах с точностью до микросекунды.
pub fn set_mtime(file: &Path, seconds: f64) {
    let whole = seconds.trunc();
    let mut nanos = ((seconds - whole) * 1e9) as i64;
    nanos -= nanos % 1000;
    let time = UNIX_EPOCH + Duration::new(whole as u64, nanos as u32);
    let handle = std::fs::OpenOptions::new()
        .write(true)
        .open(file)
        .unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    handle
        .set_times(std::fs::FileTimes::new().set_modified(time))
        .unwrap_or_else(|e| panic!("{}: {e}", file.display()));
}

/// Путь `a/b/c` от корня; `.` и пустые части пропускаются, `..` снимает часть — как `path.join`.
pub fn path_in(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |mut acc, part| {
            match part {
                "" | "." => {}
                ".." => {
                    acc.pop();
                }
                part => acc.push(part),
            }
            acc
        })
}

pub fn put_file(root: &Path, relative: &str, item: &Value) {
    let file = path_in(root, relative);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, bytes_of(item)).unwrap();
    set_mtime(&file, item["mtime"].as_f64().unwrap_or(DEFAULT_MTIME));
}

/// Разложить файлы `setup` сценария.
pub fn setup(root: &Path, list: &Value) {
    for item in list.as_array().into_iter().flatten() {
        if let Some(dir) = opt_str(&item["dir"]) {
            let dir = path_in(root, dir);
            std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        } else if let Some(link) = opt_str(&item["symlink"]) {
            make_symlink(root, link, str_of(&item["target"]));
        } else if let Some(generate) = item.get("generate") {
            for n in 0..generate["count"].as_u64().unwrap() {
                let pad = generate["pad"].as_u64().unwrap_or(1) as usize;
                let name = str_of(&generate["name"]).replace("{n}", &format!("{n:0pad$}"));
                let dir = str_of(&generate["dir"]);
                let step = generate["step"].as_f64().unwrap_or(0.0);
                let mut file = match generate.get("latin1") {
                    Some(latin1) => json!({"latin1": latin1}),
                    None => json!({"repeat": generate["repeat"]}),
                };
                if let Some(base) = generate["mtime"].as_f64() {
                    file["mtime"] = json!(base + n as f64 * step);
                }
                put_file(root, &format!("{dir}/{name}"), &file);
            }
        } else {
            put_file(root, str_of(&item["file"]), item);
        }
    }
}

/// Ссылка `link` (от корня) на `target`; `<root>` в цели — корень. В Windows ссылок нет.
pub fn make_symlink(root: &Path, link: &str, target: &str) {
    #[cfg(unix)]
    {
        let link = path_in(root, link);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(target.replace("<root>", root.to_str().unwrap()), link).unwrap();
    }
    #[cfg(not(unix))]
    let _ = (root, link, target);
}

/// Снимок дерева; у файлов до [`TEXT_LIMIT`] байт — ещё и текст latin1.
pub fn tree_with_text(root: &Path) -> Value {
    let mut tree = super::tree::snapshot(root, None);
    for entry in tree.as_array_mut().unwrap() {
        let relative = str_of(&entry["path"]).to_owned();
        match str_of(&entry["kind"]) {
            "symlink" => {
                let target = str_of(&entry["target"]).replace(root.to_str().unwrap(), "<root>");
                entry["target"] = json!(target);
            }
            "file" if entry["size"].as_u64().unwrap() <= TEXT_LIMIT => {
                let bytes = std::fs::read(path_in(root, &relative)).unwrap();
                entry["latin1"] = json!(js::latin1_string(&bytes));
            }
            _ => {}
        }
    }
    tree
}

/// Путь в тексте ответа: `<root>` вместо временной папки, `<tmp>` вместо её родителя, прямые косые.
pub fn normalize_text(root: &Path, text: &str) -> String {
    let parent = root.parent().unwrap().to_str().unwrap();
    text.replace(root.to_str().unwrap(), "<root>")
        .replace(parent, "<tmp>")
        .replace('\\', "/")
}

/// Путь внутри временной папки — от неё через `/`; прочие — нормализованным текстом.
pub fn rel(root: &Path, path: &Path) -> String {
    if path == root {
        return ".".to_owned();
    }
    match path.strip_prefix(root) {
        Ok(rest) => rest
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => normalize_text(root, &path.to_string_lossy()),
    }
}

/// Временная папка сценария. В Windows `canonicalize` даёт путь с префиксом `\\?\` — в текстах
/// ошибок он лишний, поэтому там берётся путь как есть.
pub fn temp_root() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = if cfg!(windows) {
        temp.path().to_path_buf()
    } else {
        std::fs::canonicalize(temp.path()).unwrap()
    };
    (temp, root)
}

/// Папки без прав мешают удалению временного каталога: вернуть права.
pub fn open_up(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755));
        if let Ok(read) = std::fs::read_dir(dir) {
            for entry in read.flatten() {
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    open_up(&entry.path());
                }
            }
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// Права доступа не действуют на root.
pub fn is_root_user() -> bool {
    #[cfg(unix)]
    {
        rustix::process::geteuid().is_root()
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// `chmod` шага сценария (в Windows не делается).
pub fn chmod(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
}

/// Шаги сценария над самой папкой, общие для всех модулей; `None` — шаг не общий.
pub fn common_step(root: &Path, step: &Value) -> Option<Value> {
    Some(match str_of(&step["op"]) {
        "tree" => json!({"tree": tree_with_text(root)}),
        "write" => {
            put_file(root, str_of(&step["file"]), step);
            Value::Null
        }
        "remove" => {
            let path = path_in(root, str_of(&step["file"]));
            let _ = std::fs::remove_dir_all(&path).or_else(|_| std::fs::remove_file(&path));
            Value::Null
        }
        "mkdir" => {
            std::fs::create_dir_all(path_in(root, str_of(&step["dir"]))).unwrap();
            Value::Null
        }
        "utime" => {
            set_mtime(
                &path_in(root, str_of(&step["file"])),
                step["mtime"].as_f64().unwrap(),
            );
            Value::Null
        }
        "chmod" => {
            chmod(
                &path_in(root, str_of(&step["file"])),
                step["mode"].as_u64().unwrap() as u32,
            );
            Value::Null
        }
        "symlink" => {
            make_symlink(root, str_of(&step["symlink"]), str_of(&step["target"]));
            Value::Null
        }
        _ => return None,
    })
}

/// Шаги списка (`items`) как строки.
pub fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|list| list.iter().map(|s| str_of(s).to_owned()).collect())
        .unwrap_or_default()
}

/// Сценарии группы фикстуры.
pub fn scenarios(value: &Value) -> &Vec<Value> {
    items(value)
}
