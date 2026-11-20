//! Единственный экземпляр лаунчера и признаки запущенного HLSR 0.4.x.
//!
//! Первый экземпляр держит блокировку (Linux — `flock` на файле, Windows — именованный
//! мьютекс) и канал (Linux — Unix-сокет, Windows — именованный pipe). Канал создаётся
//! сразу при захвате блокировки: запрос, пришедший до [`PrimaryInstance::listen`],
//! ждёт в очереди канала. Следующий запуск передаёт первому свои аргументы строкой
//! JSON `{"argv":[…]}` и выходит.

use crate::error::CoreError;
use crate::paths::Paths;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

/// Сколько второй экземпляр ждёт ответа первого.
const FORWARD_TIMEOUT: Duration = Duration::from_secs(3);
/// Сколько первый экземпляр ждёт строку запроса.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_REQUEST: u64 = 64 * 1024;
const RETRY: Duration = Duration::from_millis(50);

/// Итог [`acquire`].
pub enum Acquired {
    /// Этот процесс — первый экземпляр.
    Primary(PrimaryInstance),
    /// Лаунчер уже запущен; аргументы переданы ему (или не переданы — это в журнале).
    Forwarded,
}

/// Блокировка первого экземпляра и его канал. Drop снимает блокировку и закрывает канал.
pub struct PrimaryInstance {
    endpoint: Option<String>,
    channel: Option<platform::Channel>,
    task: Option<tokio::task::JoinHandle<()>>,
    lock: Option<platform::Lock>,
    place: platform::Place,
    /// Куда канал отдаёт аргументы следующих запусков ([`PrimaryInstance::listen`]).
    sender: Option<mpsc::UnboundedSender<Vec<String>>>,
}

/// Захватить блокировку или передать `argv` уже запущенному лаунчеру.
pub fn acquire(paths: &Paths, argv: Vec<String>) -> Result<Acquired, CoreError> {
    let id = instance_id(&paths.data_dir);
    let place = platform::Place::prepare(&paths.data_dir, &id).map_err(CoreError::Instance)?;
    let Some(lock) = platform::Lock::try_acquire(&place).map_err(CoreError::Instance)? else {
        forward(&place.endpoint(), &argv);
        return Ok(Acquired::Forwarded);
    };
    let channel = open_channel(&place);
    Ok(Acquired::Primary(PrimaryInstance {
        endpoint: channel.as_ref().map(|_| place.endpoint()),
        channel,
        task: None,
        lock: Some(lock),
        place,
        sender: None,
    }))
}

/// Только блокировка первого экземпляра, без передачи аргументов и без канала: так её берут
/// установщик и удаление Windows (`requestSingleInstanceLock` в `setup.js` 0.4.7), пока работают с
/// файлами лаунчера. `None` — лаунчер запущен.
pub fn try_lock(paths: &Paths) -> Result<Option<PrimaryInstance>, CoreError> {
    let id = instance_id(&paths.data_dir);
    let place = platform::Place::prepare(&paths.data_dir, &id).map_err(CoreError::Instance)?;
    let Some(lock) = platform::Lock::try_acquire(&place).map_err(CoreError::Instance)? else {
        return Ok(None);
    };
    Ok(Some(PrimaryInstance {
        endpoint: None,
        channel: None,
        task: None,
        lock: Some(lock),
        place,
        sender: None,
    }))
}

fn open_channel(place: &platform::Place) -> Option<platform::Channel> {
    match platform::Channel::open(place) {
        Ok(channel) => Some(channel),
        Err(err) => {
            tracing::warn!("[instance] канал для второго экземпляра не открыт: {err}");
            None
        }
    }
}

impl PrimaryInstance {
    /// Принимать аргументы следующих запусков. Вызывается внутри рантайма tokio один раз;
    /// без канала (или при повторном вызове) приёмник сразу закрыт.
    pub fn listen(&mut self) -> mpsc::UnboundedReceiver<Vec<String>> {
        let (tx, rx) = mpsc::unbounded_channel();
        if self.sender.is_none() {
            self.sender = Some(tx.clone());
        }
        self.serve(tx);
        rx
    }

    fn serve(&mut self, tx: mpsc::UnboundedSender<Vec<String>>) {
        if let Some(channel) = self.channel.take() {
            match channel.serve(tx) {
                Ok(task) => self.task = Some(task),
                Err(err) => {
                    tracing::warn!("[instance] канал для второго экземпляра не слушается: {err}")
                }
            }
        }
    }

    /// `app.releaseSingleInstanceLock()`: блокировка и канал отпущены — следующий запуск
    /// (например, от администратора) станет первым экземпляром.
    pub fn release(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.channel = None;
        self.endpoint = None;
        self.lock = None;
    }

    /// `app.requestSingleInstanceLock()` после [`release`](Self::release): блокировка взята
    /// снова, канал принимает аргументы в тот же приёмник [`listen`](Self::listen). `false` —
    /// блокировку держит другой процесс.
    pub fn reacquire(&mut self) -> bool {
        if self.lock.is_some() {
            return true;
        }
        match platform::Lock::try_acquire(&self.place) {
            Ok(Some(lock)) => self.lock = Some(lock),
            Ok(None) => return false,
            Err(err) => {
                tracing::warn!("[instance] блокировка не взята снова: {err}");
                return false;
            }
        }
        self.channel = open_channel(&self.place);
        self.endpoint = self.channel.as_ref().map(|_| self.place.endpoint());
        if let Some(sender) = self.sender.clone() {
            self.serve(sender);
        }
        true
    }

    /// Держит ли процесс блокировку первого экземпляра.
    pub fn is_held(&self) -> bool {
        self.lock.is_some()
    }

    /// Путь сокета (Linux) или имя pipe (Windows); `None`, если канал не открылся.
    pub fn endpoint(&self) -> Option<String> {
        self.endpoint.clone()
    }
}

impl Drop for PrimaryInstance {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        // Сокет удаляется в Lock::drop, пока блокировка ещё наша.
    }
}

/// Первые 16 hex-символов SHA-256 от пути папки данных (на Windows — в нижнем регистре).
fn instance_id(data_dir: &Path) -> String {
    use sha2::{Digest, Sha256};
    #[cfg(windows)]
    let bytes = data_dir.to_string_lossy().to_lowercase().into_bytes();
    #[cfg(not(windows))]
    let bytes = std::os::unix::ffi::OsStrExt::as_bytes(data_dir.as_os_str()).to_vec();
    Sha256::digest(&bytes)[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn forward(endpoint: &str, argv: &[String]) {
    let request = serde_json::json!({ "argv": argv }).to_string() + "\n";
    // Свой поток: block_on внутри чужого рантайма tokio — паника.
    let reply = std::thread::scope(|scope| {
        scope
            .spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .and_then(|rt| {
                        rt.block_on(send_raw(endpoint, request.as_bytes(), FORWARD_TIMEOUT))
                    })
            })
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("поток передачи упал")))
    });
    match reply {
        Ok(reply) if reply == "ok\n" => {}
        Ok(reply) => tracing::warn!("[instance] первый экземпляр ответил {reply:?}"),
        Err(err) => {
            tracing::warn!(
                "[instance] аргументы не переданы первому экземпляру ({endpoint}): {err}"
            )
        }
    }
}

/// Отправить запрос в канал первого экземпляра и прочитать ответ (до перевода строки
/// или закрытия). Пока канала нет или он занят, подключение повторяется до `timeout`.
#[doc(hidden)]
pub async fn send_raw(endpoint: &str, request: &[u8], timeout: Duration) -> io::Result<String> {
    let exchange = async {
        let mut stream = platform::connect(endpoint).await?;
        stream.write_all(request).await?;
        stream.flush().await?;
        let mut reply = Vec::new();
        let mut buf = [0u8; 64];
        loop {
            match stream.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    reply.extend_from_slice(&buf[..n]);
                    if reply.ends_with(b"\n") || reply.len() > 64 {
                        break;
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::BrokenPipe => break,
                Err(err) => return Err(err),
            }
        }
        Ok(String::from_utf8_lossy(&reply).into_owned())
    };
    tokio::time::timeout(timeout, exchange).await.map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!("нет ответа за {} с", timeout.as_secs_f64()),
        )
    })?
}

/// Один запрос второго экземпляра: строка JSON → `tx`, ответ `ok\n`.
async fn handle<S>(stream: S, tx: mpsc::UnboundedSender<Vec<String>>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (read, mut write) = tokio::io::split(stream);
    let mut reader = BufReader::new(read.take(MAX_REQUEST));
    let mut line = Vec::new();
    let read = tokio::time::timeout(REQUEST_TIMEOUT, reader.read_until(b'\n', &mut line)).await;
    if !matches!(read, Ok(Ok(_))) || !line.ends_with(b"\n") {
        tracing::warn!("[instance] запрос второго экземпляра не дочитан");
        return;
    }
    let Some(argv) = parse_request(&line) else {
        tracing::warn!("[instance] запрос второго экземпляра отброшен: не {{\"argv\":[строки]}}");
        return;
    };
    if tx.send(argv).is_err() {
        return;
    }
    let _ = tokio::time::timeout(REQUEST_TIMEOUT, async {
        write.write_all(b"ok\n").await?;
        write.flush().await?;
        // Ждать, пока второй экземпляр прочитает ответ и закроет канал: на Windows
        // закрытый сервером pipe теряет непрочитанное.
        reader.read(&mut [0u8; 1]).await
    })
    .await;
}

fn parse_request(line: &[u8]) -> Option<Vec<String>> {
    let value: serde_json::Value = serde_json::from_slice(line).ok()?;
    value
        .get("argv")?
        .as_array()?
        .iter()
        .map(|arg| arg.as_str().map(str::to_owned))
        .collect()
}

/// Запущен ли HLSR 0.4.x (Electron) с этой папкой данных.
#[cfg(not(windows))]
pub fn electron_running(data_dir: &Path) -> bool {
    electron_running_with(data_dir, process_is_hlsr)
}

/// [`electron_running`] с подменённой проверкой процесса: `SingletonLock` Chromium —
/// символьная ссылка `<хост>-<pid>`; запущена, если хост наш и `is_alive(pid)`.
#[cfg(not(windows))]
pub fn electron_running_with(data_dir: &Path, is_alive: impl Fn(u32) -> bool) -> bool {
    let Ok(target) = std::fs::read_link(data_dir.join("SingletonLock")) else {
        return false;
    };
    let Some((host, pid)) = target.to_str().and_then(|text| text.rsplit_once('-')) else {
        return false;
    };
    let Ok(pid) = pid.parse::<u32>() else {
        return false;
    };
    host == hostname() && is_alive(pid)
}

#[cfg(not(windows))]
fn hostname() -> String {
    rustix::system::uname()
        .nodename()
        .to_string_lossy()
        .into_owned()
}

/// Процесс жив, и это HLSR (`hlsr`) или Electron из исходников (`electron`).
#[cfg(not(windows))]
fn process_is_hlsr(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|comm| {
        let comm = comm.trim_end().to_ascii_lowercase();
        comm.starts_with("hlsr") || comm.starts_with("electron")
    })
}

/// Запущен ли HLSR 0.4.x (Electron) с этой папкой данных: Chromium держит
/// `<данные>\lockfile` открытым без общего доступа на запись, файл удаляется при закрытии.
#[cfg(windows)]
pub fn electron_running(data_dir: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    match std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(data_dir.join("lockfile"))
    {
        Ok(_) => false,
        Err(err) => err.raw_os_error() == Some(ERROR_SHARING_VIOLATION),
    }
}

/// `true`, если 0.4.x не запущена или завершилась за `timeout`.
pub fn wait_for_electron_exit(data_dir: &Path, timeout: Duration, poll: Duration) -> bool {
    wait_for_electron_exit_with(data_dir, timeout, poll, electron_running)
}

/// [`wait_for_electron_exit`] с подменённой проверкой (тесты).
pub fn wait_for_electron_exit_with(
    data_dir: &Path,
    timeout: Duration,
    poll: Duration,
    running: impl Fn(&Path) -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if !running(data_dir) {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        std::thread::sleep(poll.min(deadline - now));
    }
}

#[cfg(not(windows))]
mod platform {
    use super::{RETRY, handle};
    use std::fs::File;
    use std::io;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use tokio::sync::mpsc;

    /// Длина пути сокета без завершающего нуля: `sun_path` — 108 байт.
    const MAX_SOCKET_PATH: usize = 107;

    /// Каталог блокировки и сокета: `$XDG_RUNTIME_DIR/hlsr`, без него — папка данных,
    /// а если путь сокета не помещается в `sun_path` — `/tmp/hlsr-<uid>`.
    pub(super) struct Place {
        lock: PathBuf,
        socket: PathBuf,
    }

    impl Place {
        pub(super) fn prepare(data_dir: &Path, id: &str) -> io::Result<Place> {
            let uid = rustix::process::getuid().as_raw();
            let runtime = std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .filter(|dir| dir.is_absolute());
            let socket_name = format!("instance-{id}.sock");
            let (mut dir, mut private) = match runtime {
                Some(runtime) => (runtime.join("hlsr"), true),
                None => (data_dir.to_owned(), false),
            };
            if dir.join(&socket_name).as_os_str().len() > MAX_SOCKET_PATH {
                dir = PathBuf::from(format!("/tmp/hlsr-{uid}"));
                private = true;
            }
            if private {
                prepare_private_dir(&dir, uid)?;
            } else {
                std::fs::create_dir_all(&dir)?;
            }
            Ok(Place {
                lock: dir.join(format!("instance-{id}.lock")),
                socket: dir.join(socket_name),
            })
        }

        pub(super) fn endpoint(&self) -> String {
            self.socket.to_string_lossy().into_owned()
        }
    }

    /// Свой каталог с правами 0700; чужой или символьная ссылка — ошибка.
    fn prepare_private_dir(dir: &Path, uid: u32) -> io::Result<()> {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        let meta = std::fs::symlink_metadata(dir)?;
        if !meta.is_dir() || meta.uid() != uid {
            return Err(io::Error::other(format!(
                "{} — не свой каталог",
                dir.display()
            )));
        }
        if meta.mode() & 0o077 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    pub(super) struct Lock {
        file: Option<File>,
        lock: PathBuf,
        socket: PathBuf,
    }

    impl Lock {
        /// `None` — блокировку держит другой процесс.
        pub(super) fn try_acquire(place: &Place) -> io::Result<Option<Lock>> {
            // Владелец удаляет файл перед выходом, поэтому после захвата проверяется,
            // что под этим именем всё ещё наш файл, иначе захвачен уже удалённый.
            for _ in 0..20 {
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .mode(0o600)
                    .open(&place.lock)?;
                match file.try_lock() {
                    Ok(()) => {}
                    Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
                    Err(std::fs::TryLockError::Error(err)) => return Err(err),
                }
                let ours = file.metadata()?;
                match std::fs::metadata(&place.lock) {
                    Ok(now) if now.dev() == ours.dev() && now.ino() == ours.ino() => {
                        return Ok(Some(Lock {
                            file: Some(file),
                            lock: place.lock.clone(),
                            socket: place.socket.clone(),
                        }));
                    }
                    Ok(_) => {}
                    Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                    Err(err) => return Err(err),
                }
            }
            Err(io::Error::other("файл блокировки всё время пересоздаётся"))
        }
    }

    impl Drop for Lock {
        fn drop(&mut self) {
            // Пока блокировка наша, сокет и файл под этими именами — наши.
            let _ = std::fs::remove_file(&self.socket);
            let _ = std::fs::remove_file(&self.lock);
            drop(self.file.take());
        }
    }

    pub(super) struct Channel(std::os::unix::net::UnixListener);

    impl Channel {
        pub(super) fn open(place: &Place) -> io::Result<Channel> {
            // Сокет упавшего процесса: блокировка наша, значит, его никто не слушает.
            match std::fs::remove_file(&place.socket) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
            std::os::unix::net::UnixListener::bind(&place.socket).map(Channel)
        }

        pub(super) fn serve(
            self,
            tx: mpsc::UnboundedSender<Vec<String>>,
        ) -> io::Result<tokio::task::JoinHandle<()>> {
            self.0.set_nonblocking(true)?;
            let listener = tokio::net::UnixListener::from_std(self.0)?;
            Ok(crate::tasks::spawn_logged("instance", async move {
                loop {
                    match listener.accept().await {
                        Ok((stream, _)) => {
                            crate::tasks::spawn_logged(
                                "instance-request",
                                handle(stream, tx.clone()),
                            );
                        }
                        Err(err) => {
                            tracing::warn!("[instance] приём второго экземпляра: {err}");
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        }
                    }
                }
            }))
        }
    }

    pub(super) async fn connect(endpoint: &str) -> io::Result<tokio::net::UnixStream> {
        loop {
            match tokio::net::UnixStream::connect(endpoint).await {
                Ok(stream) => return Ok(stream),
                // Первый экземпляр ещё не открыл канал.
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                    ) =>
                {
                    tokio::time::sleep(RETRY).await;
                }
                Err(err) => return Err(err),
            }
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::{RETRY, handle};
    use crate::win32::wide;
    use std::io;
    use std::os::windows::io::{FromRawHandle, IntoRawHandle, OwnedHandle};
    use std::path::Path;
    use std::time::{Duration, Instant};
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};
    use tokio::sync::mpsc;
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_NO_DATA,
        ERROR_PIPE_BUSY, GetLastError, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX,
    };
    use windows_sys::Win32::System::Pipes::{
        CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
        PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };
    use windows_sys::Win32::System::Threading::CreateMutexW;

    const PIPE_BUFFER: u32 = 64 * 1024;

    /// Мьютекс `Local\hlsr-<h>` и pipe `\\.\pipe\hlsr-<h>`.
    pub(super) struct Place {
        mutex: String,
        pipe: String,
    }

    impl Place {
        pub(super) fn prepare(_data_dir: &Path, id: &str) -> io::Result<Place> {
            Ok(Place {
                mutex: format!(r"Local\hlsr-{id}"),
                pipe: format!(r"\\.\pipe\hlsr-{id}"),
            })
        }

        pub(super) fn endpoint(&self) -> String {
            self.pipe.clone()
        }
    }

    pub(super) struct Lock {
        _mutex: OwnedHandle,
    }

    impl Lock {
        /// `None` — мьютекс уже создан другим процессом.
        pub(super) fn try_acquire(place: &Place) -> io::Result<Option<Lock>> {
            let name = wide(&place.mutex);
            // SAFETY: имя — строка с нулём на конце; атрибуты по умолчанию (не наследуется).
            let (handle, error) = unsafe {
                let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
                (handle, GetLastError())
            };
            if handle.is_null() {
                return Err(io::Error::from_raw_os_error(error as i32));
            }
            // SAFETY: дескриптор только что создан и больше нигде не закрывается.
            let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
            if error == ERROR_ALREADY_EXISTS {
                return Ok(None);
            }
            Ok(Some(Lock { _mutex: handle }))
        }
    }

    pub(super) struct Channel {
        first: OwnedHandle,
        pipe: String,
    }

    impl Channel {
        /// Первый экземпляр pipe. Если он ещё открыт у завершающегося прежнего процесса
        /// (перезапуск), создание повторяется до 2 с.
        pub(super) fn open(place: &Place) -> io::Result<Channel> {
            let name = wide(&place.pipe);
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                // SAFETY: имя — строка с нулём на конце; атрибуты по умолчанию.
                let handle = unsafe {
                    CreateNamedPipeW(
                        name.as_ptr(),
                        PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                        PIPE_TYPE_BYTE
                            | PIPE_READMODE_BYTE
                            | PIPE_WAIT
                            | PIPE_REJECT_REMOTE_CLIENTS,
                        PIPE_UNLIMITED_INSTANCES,
                        PIPE_BUFFER,
                        PIPE_BUFFER,
                        0,
                        std::ptr::null(),
                    )
                };
                if handle != INVALID_HANDLE_VALUE {
                    // SAFETY: дескриптор только что создан.
                    let first = unsafe { OwnedHandle::from_raw_handle(handle) };
                    return Ok(Channel {
                        first,
                        pipe: place.pipe.clone(),
                    });
                }
                let err = io::Error::last_os_error();
                if err.raw_os_error() != Some(ERROR_ACCESS_DENIED as i32)
                    || Instant::now() >= deadline
                {
                    return Err(err);
                }
                std::thread::sleep(RETRY);
            }
        }

        pub(super) fn serve(
            self,
            tx: mpsc::UnboundedSender<Vec<String>>,
        ) -> io::Result<tokio::task::JoinHandle<()>> {
            // SAFETY: pipe создан с FILE_FLAG_OVERLAPPED, владение передаётся tokio.
            let first = unsafe { NamedPipeServer::from_raw_handle(self.first.into_raw_handle())? };
            let pipe = self.pipe;
            Ok(crate::tasks::spawn_logged("instance", async move {
                let mut server = first;
                loop {
                    let connected = server.connect().await;
                    let next = loop {
                        match ServerOptions::new()
                            .reject_remote_clients(true)
                            .create(&pipe)
                        {
                            Ok(next) => break next,
                            Err(err) => {
                                tracing::warn!("[instance] новый экземпляр pipe: {err}");
                                tokio::time::sleep(Duration::from_secs(1)).await;
                            }
                        }
                    };
                    let current = std::mem::replace(&mut server, next);
                    match connected {
                        Ok(()) => {}
                        // Клиент записал запрос и ушёл до ConnectNamedPipe: данные могли остаться.
                        Err(err) if err.raw_os_error() == Some(ERROR_NO_DATA as i32) => {}
                        Err(err) => {
                            tracing::warn!("[instance] приём второго экземпляра: {err}");
                            continue;
                        }
                    }
                    crate::tasks::spawn_logged("instance-request", handle(current, tx.clone()));
                }
            }))
        }
    }

    pub(super) async fn connect(
        endpoint: &str,
    ) -> io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
        loop {
            match ClientOptions::new().open(endpoint) {
                Ok(client) => return Ok(client),
                // Первый экземпляр ещё не создал pipe или все экземпляры заняты.
                Err(err)
                    if err.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
                        || err.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) =>
                {
                    tokio::time::sleep(RETRY).await;
                }
                Err(err) => return Err(err),
            }
        }
    }
}
