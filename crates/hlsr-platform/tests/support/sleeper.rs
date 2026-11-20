//! Настоящий процесс для проверок: временная копия системной программы, которая «спит» заданное
//! время (Windows — `ping`, Linux — `sleep`). У копии свой путь и своё имя образа, поэтому поиск
//! по имени находит только её. Процесс закрывается при выходе из области видимости.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

pub struct Sleeper {
    pub child: Option<Child>,
    /// Путь копии программы, как его отдаёт система (без `\\?\`).
    pub exe: PathBuf,
    /// Имя образа: `hlsr-t-<pid>-<номер>.exe` (Linux — без расширения, до 15 знаков).
    pub image: String,
    pub dir: tempfile::TempDir,
    /// Часы машины до запуска и после того, как запуск вернулся: время старта процесса по часам
    /// системы лежит между ними (под нагрузкой запуск может вернуться через секунды).
    pub started: std::time::SystemTime,
    pub spawned: std::time::SystemTime,
}

fn plain(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    PathBuf::from(text.strip_prefix("\\\\?\\").unwrap_or(&text).to_owned())
}

impl Sleeper {
    /// Копия программы в новой временной папке (`<папка>/Games/hl1-won`, как папка игры в
    /// библиотеке) и её запуск на `seconds` секунд.
    pub fn start(seconds: u32) -> Sleeper {
        let dir = tempfile::tempdir().expect("временная папка");
        let root = plain(&std::fs::canonicalize(dir.path()).expect("путь временной папки"))
            .join("Games")
            .join("hl1-won");
        std::fs::create_dir_all(&root).expect("папка игры");
        let number = COUNTER.fetch_add(1, Ordering::SeqCst);

        #[cfg(windows)]
        let (source, image, args) = {
            let system = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_owned());
            (
                PathBuf::from(system).join("System32").join("PING.EXE"),
                format!("hlsr-t-{}-{number}.exe", std::process::id()),
                vec!["-n".to_owned(), seconds.to_string(), "127.0.0.1".to_owned()],
            )
        };
        #[cfg(not(windows))]
        let (source, image, args) = (
            PathBuf::from("/bin/sleep"),
            format!("hlsr-{}-{number}", std::process::id() % 100_000),
            vec![seconds.to_string()],
        );

        let exe = root.join(&image);
        std::fs::copy(&source, &exe).expect("копия системной программы");
        let started = std::time::SystemTime::now();
        let child = Command::new(&exe)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("запуск копии");
        let spawned = std::time::SystemTime::now();
        Sleeper {
            child: Some(child),
            exe,
            image,
            dir,
            started,
            spawned,
        }
    }

    pub fn pid(&self) -> u32 {
        self.child.as_ref().expect("процесс запущен").id()
    }

    /// Папка с копией (она же «папка игры» для проверки пути внутри неё).
    pub fn root(&self) -> PathBuf {
        self.exe.parent().expect("папка копии").to_path_buf()
    }

    /// `Games` библиотеки (`paths.games`): папка игры `hl1-won` — [`Sleeper::root`].
    #[allow(dead_code)]
    pub fn games(&self) -> PathBuf {
        self.root().parent().expect("папка игр").to_path_buf()
    }

    pub fn is_exited(&mut self) -> bool {
        self.child
            .as_mut()
            .is_some_and(|child| matches!(child.try_wait(), Ok(Some(_))))
    }

    /// Закрыть и дождаться (чтобы процесс не остался «зомби»).
    pub fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Дождаться выхода, который устроил кто-то другой (например, остановка по PID).
    pub fn reap(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.wait();
        }
    }
}

impl Drop for Sleeper {
    fn drop(&mut self) {
        self.kill();
    }
}
