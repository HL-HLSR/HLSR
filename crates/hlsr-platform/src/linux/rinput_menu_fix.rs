//! Linux: патч курсора RInput в меню (`linux/rinput-menu-fix.js`) через `/proc/<pid>/mem`. Wine
//! грузит DLL как обычные отображения файлов, поэтому базы модулей видны в `/proc/<pid>/maps`.
//!
//! Логика (разбор карты памяти, обход процессов, память поверх файла) не делает системных вызовов
//! сама: файлы `/proc` даёт [`ProcFs`] (`fs` 0.4.7), поэтому она собирается и проверяется на всех
//! ОС. Настоящая ФС ([`SystemProcFs`]) — только под Linux.

use crate::rinput_menu_fix::{self, PatchStatus};

/// Запись карты памяти: начало отображения, смещение в файле, путь файла.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapEntry {
    pub start: u64,
    pub offset: u64,
    pub file: String,
}

/// `parseMaps(text)`: строки `/proc/<pid>/maps` → записи. Пустые строки пропускаются;
/// `start`/`offset` — шестнадцатеричные, путь — всё с шестого поля через пробел.
pub fn parse_maps(text: &str) -> Vec<MapEntry> {
    text.split('\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let start = parts
                .first()
                .and_then(|range| range.split('-').next())
                .and_then(|value| u64::from_str_radix(value, 16).ok())
                .unwrap_or(0);
            let offset = parts
                .get(2)
                .and_then(|value| u64::from_str_radix(value, 16).ok())
                .unwrap_or(0);
            let file = parts
                .get(5..)
                .map(|rest| rest.join(" "))
                .unwrap_or_default();
            MapEntry {
                start,
                offset,
                file,
            }
        })
        .collect()
}

/// `imageBase(maps, name)`: начало отображения модуля `name` (смещение 0, путь кончается на
/// `/name` или `\name`, без учёта регистра); `None` — нет такого.
pub fn image_base(maps: &[MapEntry], name: &str) -> Option<u64> {
    let lower = name.to_lowercase();
    maps.iter()
        .find(|entry| entry.offset == 0 && ends_with_separator(&entry.file.to_lowercase(), &lower))
        .map(|entry| entry.start)
}

/// `[\\/]<name>$`: путь кончается на имя с разделителем перед ним.
fn ends_with_separator(file: &str, name: &str) -> bool {
    file.strip_suffix(name)
        .is_some_and(|prefix| prefix.ends_with('/') || prefix.ends_with('\\'))
}

/// Файл памяти процесса (`fd` от `openSync(…, "r+")`): чтение и запись по смещению.
pub trait MemFile {
    /// `readSync(fd, buf, 0, length, position)`.
    fn read_at(&mut self, buf: &mut [u8], position: u64) -> Result<(), String>;
    /// `writeSync(fd, buf, 0, length, position)`.
    fn write_at(&mut self, buf: &[u8], position: u64) -> Result<(), String>;
}

/// Вызовы `fs`, которыми патч ходит в `/proc`.
pub trait ProcFs {
    /// `readdirSync(dir)`.
    fn read_dir(&self, dir: &str) -> Result<Vec<String>, String>;
    /// `readFileSync(file, "utf8")`.
    fn read_to_string(&self, file: &str) -> Result<String, String>;
    /// `openSync(file, "r+")`; файл закрывается при удалении (`closeSync`).
    fn open_mem<'a>(&'a self, file: &str) -> Result<Box<dyn MemFile + 'a>, String>;
}

/// `ProcessMemory`: память процесса через `/proc/<pid>/mem`. Файл открывается при первом чтении:
/// пока RInput не загружен, процесс трогать незачем.
pub struct ProcMemory<'a> {
    pid: String,
    fs: &'a dyn ProcFs,
    maps: Vec<MapEntry>,
    file: Option<Box<dyn MemFile + 'a>>,
}

impl<'a> ProcMemory<'a> {
    /// Читает карту памяти процесса (`/proc/<pid>/maps`).
    pub fn open(pid: &str, fs: &'a dyn ProcFs) -> Result<ProcMemory<'a>, String> {
        let maps = parse_maps(&fs.read_to_string(&format!("/proc/{pid}/maps"))?);
        Ok(ProcMemory {
            pid: pid.to_owned(),
            fs,
            maps,
            file: None,
        })
    }

    fn file(&mut self) -> Result<&mut Box<dyn MemFile + 'a>, String> {
        if self.file.is_none() {
            self.file = Some(self.fs.open_mem(&format!("/proc/{}/mem", self.pid))?);
        }
        Ok(self.file.as_mut().expect("файл памяти открыт"))
    }
}

impl rinput_menu_fix::ProcessMemory for ProcMemory<'_> {
    fn module_base(&mut self, name: &str) -> Result<Option<u64>, String> {
        Ok(image_base(&self.maps, name))
    }

    fn read(&mut self, address: u64, length: usize) -> Result<Vec<u8>, String> {
        let mut buffer = vec![0u8; length];
        self.file()?.read_at(&mut buffer, address)?;
        Ok(buffer)
    }

    fn write(&mut self, address: u64, buf: &[u8]) -> Result<(), String> {
        self.file()?.write_at(buf, address)
    }
}

/// `patchProcess(pid, fs)`.
pub fn patch_process(pid: &str, fs: &dyn ProcFs) -> Result<PatchStatus, String> {
    let mut memory = ProcMemory::open(pid, fs)?;
    rinput_menu_fix::patch_memory(&mut memory)
}

/// `patchRunningGame(processName, fs)`: патчит все запущенные процессы игры с таким именем (Wine
/// называет их по exe; `comm` сравнивается без учёта регистра).
pub fn patch_running_game(process_name: &str, fs: &dyn ProcFs) -> Result<PatchStatus, String> {
    let target = process_name.to_lowercase();
    let mut result = PatchStatus::Waiting;
    for entry in fs.read_dir("/proc")? {
        if entry.is_empty() || !entry.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let Ok(comm) = fs.read_to_string(&format!("/proc/{entry}/comm")) else {
            continue;
        };
        if hlsr_core::js::trim(&comm).to_lowercase() != target {
            continue;
        }
        result = rinput_menu_fix::merge_status(result, patch_process(&entry, fs)?);
    }
    Ok(result)
}

#[cfg(target_os = "linux")]
pub use system::SystemProcFs;

#[cfg(target_os = "linux")]
mod system {
    use super::{MemFile, ProcFs};
    use std::fs::{File, OpenOptions};
    use std::os::unix::fs::FileExt as _;

    /// Настоящая ФС (`require("fs")`).
    #[derive(Debug, Clone, Copy, Default)]
    pub struct SystemProcFs;

    struct Mem(File);

    impl MemFile for Mem {
        fn read_at(&mut self, buf: &mut [u8], position: u64) -> Result<(), String> {
            self.0
                .read_exact_at(buf, position)
                .map_err(|error| format!("чтение памяти по {position:#x}: {error}"))
        }

        fn write_at(&mut self, buf: &[u8], position: u64) -> Result<(), String> {
            self.0
                .write_all_at(buf, position)
                .map_err(|error| format!("запись памяти по {position:#x}: {error}"))
        }
    }

    impl ProcFs for SystemProcFs {
        fn read_dir(&self, dir: &str) -> Result<Vec<String>, String> {
            let entries = std::fs::read_dir(dir).map_err(|error| format!("{dir}: {error}"))?;
            Ok(entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect())
        }

        fn read_to_string(&self, file: &str) -> Result<String, String> {
            std::fs::read_to_string(file).map_err(|error| format!("{file}: {error}"))
        }

        fn open_mem<'a>(&'a self, file: &str) -> Result<Box<dyn MemFile + 'a>, String> {
            let opened = OpenOptions::new()
                .read(true)
                .write(true)
                .open(file)
                .map_err(|error| format!("{file}: {error}"))?;
            Ok(Box::new(Mem(opened)))
        }
    }
}
