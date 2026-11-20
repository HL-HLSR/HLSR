//! Общее для тестов диагностики: сценарий из фикстуры `diagnostics.json` (`tools/reference/update/
//! diagnostics.js`) как подделка контекста, запроса и DNS. Сценарий — отличия от `healthy` своей ОС
//! (`merge`), токены `{root}` и `{drive}` в путях заменяются на временный каталог.

use super::{items, str_of};
use hlsr_core::js::Platform;
use hlsr_update::diagnostics::context::{
    BuildRuntime, ContextData, DiagnosticsContext, GameChannelError, GpuStatus, LaunchAvailability,
    ProfileSnapshot, SteamApp, SteamHealth, TelemetryInfo, TelemetryStatus,
};
use hlsr_update::diagnostics::net::{DiagnosticsRequest, DnsLookup, NetError, Response};
use hlsr_update::diagnostics::runner::Check;
use hlsr_update::ports::BoxFuture;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

pub fn platform_of(name: &str) -> Platform {
    match name {
        "win32" => Platform::Win32,
        "linux" => Platform::Linux,
        other => panic!("платформа {other}"),
    }
}

/// `healthy(platform)` + `patch`: верхние ключи заменяются, `{"$absent": true}` убирает ключ, `net`
/// сливается по проверкам.
pub fn merge(fixture: &Value, platform: &str, patch: &Value) -> Value {
    let mut spec = fixture["healthy"][platform].clone();
    let object = spec.as_object_mut().expect("сценарий — объект");
    for (key, value) in patch.as_object().expect("patch — объект") {
        if value.get("$absent") == Some(&Value::Bool(true)) {
            object.remove(key);
        } else if key == "net" {
            for (probe, answer) in value.as_object().expect("net") {
                object["net"][probe] = answer.clone();
            }
        } else {
            object.insert(key.clone(), value.clone());
        }
    }
    spec
}

/// Временный каталог сценария и подстановки в его строках.
pub struct Tokens {
    pub root: String,
    pub drive: String,
}

impl Tokens {
    pub fn new(root: &Path) -> Tokens {
        Tokens {
            root: root.to_string_lossy().into_owned(),
            drive: root
                .ancestors()
                .last()
                .map(|top| top.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }
    }

    pub fn fill(&self, text: &str) -> String {
        text.replace("{root}", &self.root)
            .replace("{drive}", &self.drive)
    }

    pub fn fill_deep(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.fill(text)),
            Value::Array(list) => Value::Array(list.iter().map(|v| self.fill_deep(v)).collect()),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), self.fill_deep(v)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// Корень временного каталога → `{root}`; строка, которая сама есть корень диска, → `{drive}`
    /// (в других строках `/` или `C:\` — обычный текст).
    pub fn mask(&self, text: &str) -> String {
        if text == self.drive {
            return "{drive}".to_owned();
        }
        text.replace(&self.root, "{root}")
    }

    pub fn mask_deep(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.mask(text)),
            Value::Array(list) => Value::Array(list.iter().map(|v| self.mask_deep(v)).collect()),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), self.mask_deep(v)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
}

/// Разложить `files` сценария: `{dir}` или `{path, text | json}`.
pub fn write_files(root: &Path, files: &Value) {
    let Some(files) = files.as_array() else {
        return;
    };
    for item in files {
        if let Some(dir) = item["dir"].as_str() {
            std::fs::create_dir_all(dir.split('/').fold(root.to_path_buf(), |p, s| p.join(s)))
                .unwrap();
            continue;
        }
        let file = str_of(&item["path"])
            .split('/')
            .fold(root.to_path_buf(), |p, s| p.join(s));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let text = match item["text"].as_str() {
            Some(text) => text.to_owned(),
            None => item["json"].to_string(),
        };
        std::fs::write(file, text).unwrap();
    }
}

fn number(value: &Value) -> f64 {
    match value {
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(text) => hlsr_core::js::to_number(text),
        Value::Null => 0.0,
        Value::Bool(flag) => f64::from(u8::from(*flag)),
        _ => f64::NAN,
    }
}

fn optional_text(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// Подделка контекста: значения берутся из сценария.
pub struct StubContext {
    pub spec: Value,
    pub root: PathBuf,
    pub game_verdicts: Vec<(String, Option<String>)>,
}

impl StubContext {
    pub fn new(spec: Value, root: PathBuf, fixture: &Value) -> StubContext {
        let game_verdicts = items(&fixture["gameVerdicts"])
            .iter()
            .map(|entry| {
                (
                    str_of(&entry["json"]).to_owned(),
                    entry["code"].as_str().map(str::to_owned),
                )
            })
            .collect();
        StubContext {
            spec,
            root,
            game_verdicts,
        }
    }

    fn tokens(&self) -> Tokens {
        Tokens::new(&self.root)
    }

    /// Функция сценария: `None` — функции нет, `Some(None)` — бросила или вернула пустое.
    fn function(&self, name: &str) -> Option<Option<Value>> {
        let field = self.spec.get(name)?;
        if field.get("throws").is_some() {
            return Some(None);
        }
        let value = field.get("value")?;
        Some((!value.is_null()).then(|| value.clone()))
    }
}

impl DiagnosticsContext for StubContext {
    fn data(&self) -> ContextData {
        let spec = &self.spec;
        let tokens = self.tokens();
        ContextData {
            app_version: optional_text(&spec["appVersion"]).unwrap_or_default(),
            packaged: spec["packaged"].as_bool().unwrap_or(false),
            platform: platform_of(str_of(&spec["platform"])),
            arch: str_of(&spec["os"]["arch"]).to_owned(),
            os_release: str_of(&spec["os"]["release"]).to_owned(),
            native_available: spec["nativeAvailable"].as_bool().unwrap_or(false),
            native_load_error: spec["nativeLoadError"].as_str().unwrap_or("").to_owned(),
            umu_available: spec["umuAvailable"].as_bool().unwrap_or(false),
            proton_selected: optional_text(&spec["protonSelected"]),
            user_data_path: match spec["userData"].as_str() {
                Some("root") => Some(self.root.clone()),
                Some("missing") => Some(self.root.join("no-such-dir")),
                _ => None,
            },
            update_state: optional_text(&spec["updateState"]).unwrap_or_default(),
            update_enabled: spec["updateEnabled"].as_bool(),
            library_path: spec["library"].as_str().map(|text| tokens.fill(text)),
            free_space_mb: spec["freeSpaceMb"].as_f64(),
            steam_health: spec.get("steamHealth").map(|health| SteamHealth {
                status: str_of(&health["status"]).to_owned(),
                code: optional_text(&health["code"]),
                detail: optional_text(&health["detail"]),
            }),
        }
    }

    fn gpu_status(&self) -> BoxFuture<'_, Option<GpuStatus>> {
        let gpu = self.function("gpu").flatten();
        Box::pin(async move {
            let gpu = gpu?;
            Some(GpuStatus {
                configured_enabled: gpu["configuredEnabled"].as_bool().unwrap_or(false),
                applied_enabled: gpu["appliedEnabled"].as_bool(),
                available: gpu["available"].as_bool().unwrap_or(false),
                hardware_enabled: gpu["hardwareEnabled"].as_bool().unwrap_or(false),
                software_renderer: gpu["softwareRenderer"].as_bool().unwrap_or(false),
                device: gpu["device"].as_str().unwrap_or("").to_owned(),
                driver: gpu["driver"].as_str().unwrap_or("").to_owned(),
            })
        })
    }

    fn download_limit_bps(&self) -> Option<f64> {
        let field = self.spec.get("downloadLimitBps")?;
        let value = field.get("fn").or_else(|| field.get("value"))?;
        value.as_f64().filter(|x| x.is_finite())
    }

    fn launch_availability(&self) -> BoxFuture<'_, Option<LaunchAvailability>> {
        let launch = self.function("launch").flatten();
        Box::pin(async move {
            let launch = launch?;
            Some(LaunchAvailability {
                ok: launch["ok"].as_bool().unwrap_or(false),
                code: optional_text(&launch["code"]),
            })
        })
    }

    fn required_steam_apps(&self) -> Vec<SteamApp> {
        // Без `steamOwnership` проверка владения не выполняется.
        if self.spec.get("ownership").is_none() {
            return Vec::new();
        }
        self.spec
            .get("requiredSteamApps")
            .and_then(Value::as_array)
            .map(|apps| {
                apps.iter()
                    .map(|app| SteamApp {
                        app_id: app["appId"].as_u64().unwrap(),
                        title: str_of(&app["title"]).to_owned(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn steam_ownership(&self, app_id: u64, _timeout: Duration) -> BoxFuture<'_, Option<bool>> {
        let answer = self.spec["ownership"]
            .get(app_id.to_string())
            .and_then(Value::as_bool);
        Box::pin(async move { answer })
    }

    fn profile(&self) -> Option<ProfileSnapshot> {
        let profile = self.function("profile").flatten()?;
        Some(ProfileSnapshot {
            signed_in: profile["signedIn"].as_bool().unwrap_or(false),
            play_seconds: number(&profile["playSeconds"]),
            launch_count: profile.get("launchCount").map(number),
            games_played: profile.get("gamesPlayed").map(number),
            top_game_id: optional_text(&profile["topGameId"]),
            achievements: profile.get("achievements").map(|a| {
                (
                    a["unlocked"].as_f64().unwrap_or(0.0),
                    a["total"].as_f64().unwrap_or(0.0),
                )
            }),
            installed_at: optional_text(&profile["installedAt"]),
        })
    }

    fn telemetry(&self) -> TelemetryInfo {
        if self.spec.get("telemetryStatus").is_some() {
            return TelemetryInfo::Status(self.function("telemetryStatus").flatten().map(
                |status| TelemetryStatus {
                    backend_configured: status["backendConfigured"].as_bool().unwrap_or(false),
                    enabled: status["enabled"].as_bool().unwrap_or(false),
                },
            ));
        }
        if self.spec.get("telemetryEnabled").is_some() {
            let enabled = self
                .function("telemetryEnabled")
                .flatten()
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            return TelemetryInfo::Enabled(enabled);
        }
        TelemetryInfo::Unavailable
    }

    fn build_runtime(&self, _installed: &Value, _launcher_version: &str) -> BuildRuntime {
        match &self.spec["buildRuntime"] {
            Value::String(_) => BuildRuntime {
                state: "repair-required".to_owned(),
                reason: Some("BUILD_METADATA_INVALID".to_owned()),
            },
            runtime => BuildRuntime {
                state: str_of(&runtime["state"]).to_owned(),
                reason: optional_text(&runtime["reason"]),
            },
        }
    }

    fn build_metadata_sha256(&self, _install_path: &str) -> Result<Option<String>, String> {
        match &self.spec["buildRead"] {
            Value::String(_) => Err("boom".to_owned()),
            read if read["present"].as_bool() == Some(true) => {
                Ok(read["sha256"].as_str().map(str::to_owned))
            }
            _ => Ok(None),
        }
    }

    fn validate_game_channel(&self, data: &Value) -> Result<(), GameChannelError> {
        let key = data.to_string();
        let verdict = self
            .game_verdicts
            .iter()
            .find(|(json, _)| *json == key)
            .unwrap_or_else(|| panic!("в фикстуре нет вердикта канала игры для {key}"));
        match &verdict.1 {
            None => Ok(()),
            Some(code) => Err(GameChannelError {
                code: Some(code.clone()),
            }),
        }
    }

    fn proton_release_ok(&self, release: &Value) -> bool {
        release["ok"].as_bool() == Some(true) || release["tag_name"].as_str() == Some("pinned")
    }
}

// ------------------------------------------------------------------- сеть

/// Тело ответа по ссылке `@имя` на документ фикстуры или как есть.
pub fn body_bytes(fixture: &Value, body: &Value) -> Option<Vec<u8>> {
    match body {
        Value::Null => None,
        Value::String(text) => match text.strip_prefix('@') {
            Some("docs-empty") => Some(Vec::new()),
            Some(name) => {
                let doc = &fixture["docs"][name];
                assert!(!doc.is_null(), "нет документа {name}");
                Some(match doc {
                    Value::String(text) => text.clone().into_bytes(),
                    other => other.to_string().into_bytes(),
                })
            }
            None => Some(text.clone().into_bytes()),
        },
        other => Some(other.to_string().into_bytes()),
    }
}

/// Подделка запроса: ответ по логической проверке сценария, адреса записываются.
pub struct StubRequest {
    answers: Vec<(String, Value)>,
    bodies: Value,
    pub urls: Mutex<Vec<String>>,
}

impl StubRequest {
    pub fn new(fixture: &Value, platform: &str, net: &Value) -> StubRequest {
        let by_key = fixture["urlsByKey"][platform].as_object().unwrap();
        let answers = by_key
            .iter()
            .filter_map(|(key, url)| Some((str_of(url).to_owned(), net.get(key)?.clone())))
            .collect();
        StubRequest {
            answers,
            bodies: fixture.clone(),
            urls: Mutex::default(),
        }
    }

    /// Адреса запросов в виде логических ключей (`launcher`, `third`…).
    pub fn keys(&self, fixture: &Value, platform: &str) -> Vec<String> {
        let by_key = fixture["urlsByKey"][platform].as_object().unwrap();
        self.urls
            .lock()
            .unwrap()
            .iter()
            .map(|url| {
                by_key
                    .iter()
                    .find(|(_, candidate)| candidate.as_str() == Some(url))
                    .map(|(key, _)| key.clone())
                    .unwrap_or_else(|| format!("?{url}"))
            })
            .collect()
    }
}

impl DiagnosticsRequest for StubRequest {
    fn get<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<Response, NetError>> {
        self.urls.lock().unwrap().push(url.to_owned());
        let answer = self
            .answers
            .iter()
            .find(|(candidate, _)| candidate == url)
            .unwrap_or_else(|| panic!("сценарий: нет ответа на {url}"))
            .1
            .clone();
        let bodies = self.bodies.clone();
        Box::pin(async move {
            if answer["hang"].as_bool() == Some(true) {
                std::future::pending::<()>().await;
            }
            if let Some(error) = answer.get("error") {
                return Err(NetError {
                    code: error["code"].as_str().map(str::to_owned),
                    message: error["message"].as_str().unwrap_or("").to_owned(),
                });
            }
            Ok(Response {
                status: answer["status"].as_u64().unwrap() as u16,
                body: body_bytes(&bodies, &answer["body"]),
                truncated: answer["truncated"].as_bool() == Some(true),
            })
        })
    }
}

/// Подделка DNS: `"ok"`, `"hang"` или `{error: {code, message}}`.
pub struct StubDns(pub Value);

impl DnsLookup for StubDns {
    fn lookup<'a>(&'a self, _host: &'a str) -> BoxFuture<'a, Result<(), NetError>> {
        let spec = self.0.clone();
        Box::pin(async move {
            match spec.as_str() {
                Some("ok") => Ok(()),
                Some("hang") => std::future::pending().await,
                _ => Err(NetError {
                    code: spec["error"]["code"].as_str().map(str::to_owned),
                    message: spec["error"]["message"]
                        .as_str()
                        .unwrap_or("dns")
                        .to_owned(),
                }),
            }
        })
    }
}

/// Строки раннера как JSON, с масками `{root}` и `{drive}`.
pub fn rows(checks: &[Check], tokens: &Tokens) -> Vec<Value> {
    checks
        .iter()
        .map(|check| tokens.mask_deep(&check.to_value()))
        .collect()
}

/// Объект из пар (для сборки спецификаций в тестах).
pub fn object(pairs: &[(&str, Value)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect::<Map<_, _>>(),
    )
}
