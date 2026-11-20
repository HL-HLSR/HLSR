//! Помощники сверки Proton с 0.4.7 (`tools/reference/platform/proton.js`): нормализация путей
//! как у скрипта (временный корень → `<root>`, `\` → `/`), снимок дерева файлов в формате
//! `tools/reference/lib/tree.js`, временные корни, подделки сети.

#![allow(dead_code)]

use hlsr_core::js::path::NodePath;
use hlsr_delivery::providers::BoxFuture;
use hlsr_platform::linux::abort::Abort;
use hlsr_platform::linux::prefix_command::ProtonError;
use hlsr_platform::linux::proton_manager::{Body, Fetch, MemoryBody, ReadText, real_path};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Временный корень сценария (`workRoot`): путь после разворота ссылок, удаляется в конце.
pub struct Root {
    _dir: tempfile::TempDir,
    pub path: PathBuf,
    pub text: String,
}

impl Root {
    pub fn new(prefix: &str) -> Root {
        let dir = tempfile::Builder::new().prefix(prefix).tempdir().unwrap();
        let path = real_path(dir.path()).unwrap();
        let text = path.to_string_lossy().into_owned();
        Root {
            _dir: dir,
            path,
            text,
        }
    }

    /// `path.join(root, p)`.
    pub fn at(&self, relative: &str) -> String {
        NodePath::native().join(&[&self.text, relative])
    }

    /// `norm(value)` скрипта с этим корнем.
    pub fn norm(&self, value: &Value) -> Value {
        norm(value, &[self.text.as_str()])
    }

    pub fn norm_text(&self, text: &str) -> String {
        norm_text(text, &[self.text.as_str()])
    }
}

/// `writeFile(file, content, mode)`.
pub fn write_file(file: &Path, content: impl AsRef<[u8]>, mode: Option<u32>) {
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, content).unwrap();
    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = mode;
}

/// Корень в трёх записях (как есть, через `/`, внутри строки JSON) → `<root>`, затем `\` → `/`.
pub fn norm_text(text: &str, roots: &[&str]) -> String {
    let mut text = text.to_owned();
    for root in roots {
        let variants = [
            root.replace('\\', "\\\\"),
            (*root).to_owned(),
            root.replace('/', "\\"),
            root.replace('\\', "/"),
        ];
        for variant in variants {
            text = text.replace(&variant, "<root>");
        }
    }
    text.replace("\\\\", "/").replace('\\', "/")
}

/// `norm(value)`: строки и ключи объектов.
pub fn norm(value: &Value, roots: &[&str]) -> Value {
    match value {
        Value::String(text) => Value::String(norm_text(text, roots)),
        Value::Array(items) => Value::Array(items.iter().map(|item| norm(item, roots)).collect()),
        Value::Object(object) => {
            let mut out = Map::new();
            for (key, item) in object {
                out.insert(norm_text(key, roots), norm(item, roots));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// Снимок дерева (`snapshot` из `tools/reference/lib/tree.js`) с подменой корня в файлах UTF-8;
/// нет каталога — `null` (`treeOf`).
pub fn tree_of(dir: &Path, roots: &[&str]) -> Value {
    if !dir.exists() {
        return Value::Null;
    }
    let mut entries = Vec::new();
    walk(dir, "", roots, &mut entries);
    entries.sort_by(|a: &Value, b: &Value| {
        hlsr_core::js::cmp_utf16(a["path"].as_str().unwrap(), b["path"].as_str().unwrap())
    });
    norm(&Value::Array(entries), roots)
}

fn walk(dir: &Path, prefix: &str, roots: &[&str], out: &mut Vec<Value>) {
    for item in std::fs::read_dir(dir).unwrap() {
        let item = item.unwrap();
        let name = item.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let kind = item.file_type().unwrap();
        if kind.is_symlink() {
            let target = std::fs::read_link(item.path()).unwrap();
            out.push(json!({"path": rel, "kind": "symlink", "target": target.to_string_lossy()}));
        } else if kind.is_dir() {
            out.push(json!({"path": rel, "kind": "dir"}));
            walk(&item.path(), &rel, roots, out);
        } else {
            let mut bytes = std::fs::read(item.path()).unwrap();
            if let Ok(text) = String::from_utf8(bytes.clone()) {
                bytes = norm_text(&text, roots).into_bytes();
            }
            let sha256 = hlsr_delivery::file_ops::hex(&Sha256::digest(&bytes));
            out.push(json!({"path": rel, "kind": "file", "size": bytes.len(), "sha256": sha256}));
        }
    }
}

/// Ошибка как в фикстуре: `{code, message}`.
pub fn error_value(error: &ProtonError) -> Value {
    json!({"code": error.code, "message": error.message})
}

/// `attempt(fn)`: `{ok}` или `{error: {code, message}}`.
pub fn attempt<T>(result: Result<T, ProtonError>, ok: impl FnOnce(T) -> Value) -> Value {
    match result {
        Ok(value) => json!({"ok": ok(value)}),
        Err(error) => json!({"error": error_value(&error)}),
    }
}

/// Подделка сети менеджера: тексты по адресам, архив на любой запрос загрузки, журналы
/// запросов.
#[derive(Default)]
pub struct FakeNet {
    pub texts: Mutex<HashMap<String, String>>,
    pub body: Mutex<Vec<u8>>,
    pub reads: Mutex<Vec<Value>>,
    pub requests: Mutex<Vec<String>>,
}

impl FakeNet {
    pub fn reads(&self) -> Vec<Value> {
        self.reads.lock().unwrap().clone()
    }

    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

/// `readText` подделки: нет текста — «GitHub ответил 404».
pub struct FakeText(pub Arc<FakeNet>);

impl ReadText for FakeText {
    fn read_text<'a>(
        &'a self,
        url: &'a str,
        limit: Option<usize>,
        _abort: Option<&'a Abort>,
    ) -> BoxFuture<'a, Result<String, ProtonError>> {
        let net = self.0.clone();
        Box::pin(async move {
            net.reads
                .lock()
                .unwrap()
                .push(json!({"url": url, "limit": limit}));
            net.texts
                .lock()
                .unwrap()
                .get(url)
                .cloned()
                .ok_or_else(|| ProtonError::new("GitHub ответил 404"))
        })
    }
}

/// `request` подделки: тело одним куском.
pub struct FakeRequest(pub Arc<FakeNet>);

impl Fetch for FakeRequest {
    fn request<'a>(
        &'a self,
        url: &'a str,
        _abort: Option<&'a Abort>,
    ) -> BoxFuture<'a, Result<Box<dyn Body>, ProtonError>> {
        let net = self.0.clone();
        Box::pin(async move {
            net.requests.lock().unwrap().push(url.to_owned());
            let body = net.body.lock().unwrap().clone();
            Ok(Box::new(MemoryBody::new([body])) as Box<dyn Body>)
        })
    }
}

/// Архив обновления из фикстур (`tests/fixtures/proton/*.tar.xz`).
pub fn update_archive() -> Vec<u8> {
    std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/proton/proton-wineland-11.0-20991231-hlsr99-x86_64.tar.xz"),
    )
    .unwrap()
}

/// SHA-256 и SHA-512 в hex.
pub fn digests(bytes: &[u8]) -> (String, String) {
    (
        hlsr_delivery::file_ops::hex(&Sha256::digest(bytes)),
        hlsr_delivery::file_ops::hex(&sha2::Sha512::digest(bytes)),
    )
}

// ---------------------------------------------------------------- поддельный запуск процессов

use hlsr_platform::spawn::{
    Child, ChildControl, ChildController, Env, PATH_DELIMITER, SpawnOptions, Spawner, Stdio,
};

/// Вызов `spawn`: команда, аргументы, папка, `detached`, вид `stdio`, окружение.
#[derive(Debug, Clone)]
pub struct Call {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub detached: bool,
    /// `ignore`, `pipe` или `log`.
    pub stdio: &'static str,
    pub env: Env,
}

impl Call {
    /// Как вызов записывает скрипт фикстуры (`PATH` — списком частей).
    pub fn to_json(&self) -> Value {
        json!({
            "command": self.command,
            "args": self.args,
            "cwd": self.cwd.as_ref().map(|cwd| cwd.to_string_lossy().into_owned()),
            "detached": self.detached,
            "stdio": self.stdio,
            "env": env_json(&self.env),
        })
    }
}

/// `envOf(env)`: ключи по порядку, `PATH` — списком частей.
pub fn env_json(env: &Env) -> Value {
    let mut out = Map::new();
    for (key, value) in env {
        let item = if key == "PATH" {
            json!(value.split(PATH_DELIMITER).collect::<Vec<_>>())
        } else {
            json!(value)
        };
        out.insert(key.clone(), item);
    }
    Value::Object(out)
}

/// Сигналы, которые получили поддельные процессы.
#[derive(Default)]
pub struct Signals(pub Mutex<Vec<String>>);

impl ChildControl for Signals {
    fn kill(&self, signal: &str) -> bool {
        self.0.lock().unwrap().push(signal.to_owned());
        true
    }
    fn kill_group(&self, _signal: &str) -> bool {
        false
    }
}

/// Что делает поддельный процесс на следующем шаге цикла событий (`setImmediate`): номер вызова,
/// вызов, управление процессом и файл журнала (`stdio` в файл).
pub type Behaviour =
    Arc<dyn Fn(usize, &Call, Arc<ChildController>, Option<std::fs::File>) + Send + Sync>;

/// Поддельный `spawnFn`: помнит вызовы и держит процессы живыми, пока их не завершат.
pub struct FakeSpawner {
    pub calls: Mutex<Vec<Call>>,
    pub controllers: Mutex<Vec<Arc<ChildController>>>,
    pub children: Mutex<Vec<Child>>,
    pub signals: Arc<Signals>,
    behaviour: Behaviour,
    pid: Box<dyn Fn(usize) -> Option<u32> + Send + Sync>,
}

impl FakeSpawner {
    /// PID — 1000 + число вызовов с прошлого [`take_calls`](Self::take_calls) (`1000 +
    /// calls.length` скрипта).
    pub fn new(behaviour: Behaviour) -> Arc<FakeSpawner> {
        FakeSpawner::with_pid(behaviour, |count| Some(1000 + count as u32))
    }

    pub fn with_pid(
        behaviour: Behaviour,
        pid: impl Fn(usize) -> Option<u32> + Send + Sync + 'static,
    ) -> Arc<FakeSpawner> {
        Arc::new(FakeSpawner {
            calls: Mutex::new(Vec::new()),
            controllers: Mutex::new(Vec::new()),
            children: Mutex::new(Vec::new()),
            signals: Arc::default(),
            behaviour,
            pid: Box::new(pid),
        })
    }

    /// Вызовы с прошлого раза.
    pub fn take_calls(&self) -> Vec<Call> {
        std::mem::take(&mut *self.calls.lock().unwrap())
    }

    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    pub fn controller(&self, index: usize) -> Arc<ChildController> {
        self.controllers.lock().unwrap()[index].clone()
    }
}

impl Spawner for FakeSpawner {
    fn spawn(&self, command: &str, args: &[String], options: SpawnOptions) -> Child {
        let (stdio, log) = match options.stdio {
            Stdio::Ignore => ("ignore", None),
            Stdio::Pipe => ("pipe", None),
            Stdio::Log(file) => ("log", Some(file)),
        };
        let call = Call {
            command: command.to_owned(),
            args: args.to_vec(),
            cwd: options.cwd,
            detached: options.detached,
            stdio,
            env: options.env,
        };
        let (index, count) = {
            let mut calls = self.calls.lock().unwrap();
            calls.push(call.clone());
            (self.controllers.lock().unwrap().len(), calls.len())
        };
        let (child, controller) = Child::new((self.pid)(count), self.signals.clone());
        let controller = Arc::new(controller);
        self.controllers.lock().unwrap().push(controller.clone());
        self.children.lock().unwrap().push(child.clone());
        let behaviour = self.behaviour.clone();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            behaviour(index, &call, controller, log);
        });
        child
    }
}
