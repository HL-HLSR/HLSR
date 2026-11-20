//! Файловая система, как её видят `SteamLocator` и `SteamAccount` (`opts.fs` в 0.4.7: `existsSync`,
//! `readFileSync`, `statSync`, `readdirSync`, `realpathSync`). Настоящая — [`RealFs`]; для тестов
//! [`MemoryFs`] — дерево в памяти с путями Windows или Linux, без обращения к диску.

use hlsr_core::js::Platform;
use hlsr_core::js::path::NodePath;
use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::sync::Mutex;

/// `fs.Stats`: то, что читает лаунчер.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStat {
    pub is_file: bool,
    pub is_dir: bool,
    pub size: u64,
    /// Владелец (только Linux).
    pub uid: Option<u32>,
}

/// Файловые вызовы Steam-модулей; пути — строки `path` той ОС, чьи правила у модуля.
pub trait SteamFs: Send + Sync {
    /// `existsSync(path)`.
    fn exists(&self, path: &str) -> bool;
    /// `readFileSync(path)`.
    fn read(&self, path: &str) -> io::Result<Vec<u8>>;
    /// `statSync(path)`.
    fn stat(&self, path: &str) -> io::Result<FileStat>;
    /// `readdirSync(path)`.
    fn read_dir(&self, path: &str) -> io::Result<Vec<String>>;
    /// `realpathSync(path)`.
    fn real_path(&self, path: &str) -> io::Result<String>;
}

/// `readFileSync(path, "utf8")`: неверные байты — U+FFFD, BOM остаётся символом.
pub fn read_to_string(fs: &dyn SteamFs, path: &str) -> io::Result<String> {
    Ok(hlsr_core::js::utf8_string(&fs.read(path)?))
}

/// Настоящая файловая система.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealFs;

impl SteamFs for RealFs {
    fn exists(&self, path: &str) -> bool {
        std::path::Path::new(path).exists()
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        hlsr_core::js::fs::read_file(std::path::Path::new(path))
    }

    fn stat(&self, path: &str) -> io::Result<FileStat> {
        let meta = std::fs::metadata(path)?;
        #[cfg(unix)]
        let uid = {
            use std::os::unix::fs::MetadataExt as _;
            Some(meta.uid())
        };
        #[cfg(not(unix))]
        let uid = None;
        Ok(FileStat {
            is_file: meta.is_file(),
            is_dir: meta.is_dir(),
            size: meta.len(),
            uid,
        })
    }

    fn read_dir(&self, path: &str) -> io::Result<Vec<String>> {
        hlsr_core::js::fs::read_dir_names(std::path::Path::new(path))
    }

    fn real_path(&self, path: &str) -> io::Result<String> {
        let real = std::fs::canonicalize(path)?;
        let text = real.to_string_lossy().into_owned();
        // `realpathSync` Node не отдаёт префикс `\\?\`, который ставит canonicalize в Windows.
        Ok(match text.strip_prefix("\\\\?\\") {
            Some(rest) => rest.to_owned(),
            None => text,
        })
    }
}

#[derive(Debug, Clone)]
enum Node {
    File(Vec<u8>),
    Dir,
    Link(String),
}

#[derive(Debug, Clone)]
struct Entry {
    /// Имя последней части пути, как его задали (регистр сохраняется).
    name: String,
    node: Node,
    uid: Option<u32>,
}

/// Дерево в памяти. Windows — пути без учёта регистра; Linux — с учётом.
#[derive(Debug)]
pub struct MemoryFs {
    path: NodePath,
    entries: Mutex<BTreeMap<String, Entry>>,
}

impl MemoryFs {
    pub fn new(platform: Platform) -> MemoryFs {
        let root = match platform {
            Platform::Win32 => "C:\\",
            Platform::Linux => "/",
        };
        MemoryFs {
            path: NodePath::new(platform, root),
            entries: Mutex::new(BTreeMap::new()),
        }
    }

    /// Ключ пути: после `resolve`, у Windows ещё и без регистра.
    fn key(&self, path: &str) -> String {
        let resolved = self.path.resolve(&[path]);
        match self.path.platform() {
            Platform::Win32 => resolved.to_lowercase(),
            Platform::Linux => resolved,
        }
    }

    fn entries(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Entry>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Корень и части пути (`resolved` уже после `resolve`).
    fn split(&self, resolved: &str) -> (String, Vec<String>) {
        let mut parts = Vec::new();
        let mut current = resolved.to_owned();
        loop {
            let parent = self.path.dirname(&current);
            if parent == current {
                break;
            }
            parts.push(self.path.basename(&current));
            current = parent;
        }
        parts.reverse();
        (current, parts)
    }

    /// Создать каталоги предков `path` (и сам `path`, если `include_self`).
    fn make_dirs(&self, path: &str, include_self: bool) {
        let resolved = self.path.resolve(&[path]);
        let (root, parts) = self.split(&resolved);
        let count = parts.len() - usize::from(!include_self && !parts.is_empty());
        let mut current = root;
        for part in parts.iter().take(count) {
            current = self.path.join(&[&current, part]);
            self.entries().entry(self.key(&current)).or_insert(Entry {
                name: part.clone(),
                node: Node::Dir,
                uid: None,
            });
        }
    }

    fn insert(&self, path: &str, node: Node) {
        self.make_dirs(path, false);
        let resolved = self.path.resolve(&[path]);
        let name = self.path.basename(&resolved);
        self.entries().insert(
            self.key(&resolved),
            Entry {
                name,
                node,
                uid: None,
            },
        );
    }

    /// Каталог (с предками).
    pub fn add_dir(&self, path: &str) {
        self.make_dirs(path, true);
    }

    /// Файл (предки создаются).
    pub fn add_file(&self, path: &str, content: impl AsRef<[u8]>) {
        self.insert(path, Node::File(content.as_ref().to_vec()));
    }

    /// Символическая ссылка `link` → `target` (предки ссылки создаются).
    pub fn add_link(&self, link: &str, target: &str) {
        self.insert(link, Node::Link(self.path.resolve(&[target])));
    }

    /// Убрать элемент (сам по себе, без содержимого).
    pub fn remove(&self, path: &str) {
        self.entries().remove(&self.key(path));
    }

    /// Владелец элемента (Linux, `statSync().uid`).
    pub fn set_uid(&self, path: &str, uid: u32) {
        if let Some(entry) = self.entries().get_mut(&self.key(path)) {
            entry.uid = Some(uid);
        }
    }

    /// Путь после раскрытия всех ссылок; нет элемента — `None`.
    fn canonical(&self, path: &str) -> Option<String> {
        let (root, parts) = self.split(&self.path.resolve(&[path]));
        let mut pending: VecDeque<String> = parts.into();
        let mut walked = root;
        let mut links = 0;
        while let Some(part) = pending.pop_front() {
            let next = self.path.join(&[&walked, &part]);
            let node = self.entries().get(&self.key(&next))?.node.clone();
            if let Node::Link(target) = node {
                links += 1;
                if links > 40 {
                    return None;
                }
                // Цель ссылки раскрывается заново, оставшиеся части пути идут за ней.
                let (target_root, target_parts) = self.split(&target);
                walked = target_root;
                for part in target_parts.into_iter().rev() {
                    pending.push_front(part);
                }
            } else {
                walked = next;
            }
        }
        Some(walked)
    }

    fn entry_of(&self, path: &str) -> Option<(String, Entry)> {
        let canonical = self.canonical(path)?;
        let found = self.entries().get(&self.key(&canonical)).cloned();
        // Корень диска существует всегда, даже если в дереве его не заводили.
        let entry = found.or_else(|| {
            (self.path.dirname(&canonical) == canonical).then(|| Entry {
                name: canonical.clone(),
                node: Node::Dir,
                uid: None,
            })
        })?;
        Some((canonical, entry))
    }
}

fn not_found(path: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("ENOENT: no such file or directory, '{path}'"),
    )
}

impl SteamFs for MemoryFs {
    fn exists(&self, path: &str) -> bool {
        self.entry_of(path).is_some()
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        match self.entry_of(path) {
            Some((
                _,
                Entry {
                    node: Node::File(bytes),
                    ..
                },
            )) => Ok(bytes),
            Some(_) => Err(io::Error::other(format!(
                "EISDIR: illegal operation on a directory, read '{path}'"
            ))),
            None => Err(not_found(path)),
        }
    }

    fn stat(&self, path: &str) -> io::Result<FileStat> {
        let Some((_, entry)) = self.entry_of(path) else {
            return Err(not_found(path));
        };
        Ok(FileStat {
            is_file: matches!(entry.node, Node::File(_)),
            is_dir: matches!(entry.node, Node::Dir),
            size: match &entry.node {
                Node::File(bytes) => bytes.len() as u64,
                _ => 0,
            },
            uid: entry.uid,
        })
    }

    fn read_dir(&self, path: &str) -> io::Result<Vec<String>> {
        let Some((canonical, entry)) = self.entry_of(path) else {
            return Err(not_found(path));
        };
        if !matches!(entry.node, Node::Dir) {
            return Err(io::Error::other(format!(
                "ENOTDIR: not a directory, scandir '{path}'"
            )));
        }
        let parent = self.key(&canonical);
        let mut names: Vec<String> = self
            .entries()
            .iter()
            .filter(|(key, _)| **key != parent && self.path.dirname(key) == parent)
            .map(|(_, entry)| entry.name.clone())
            .collect();
        // `scandir` Linux сортирует по байтам имён.
        names.sort();
        Ok(names)
    }

    fn real_path(&self, path: &str) -> io::Result<String> {
        self.canonical(path).ok_or_else(|| not_found(path))
    }
}
