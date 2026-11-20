//! QA-режим доставки (`qa-mode.js`): своя библиотека, копии манифестов с временным источником,
//! патчи и файловые манифесты из переменных окружения. Только вне «production» и при включённой
//! удалённой доставке.

use crate::environment::Environment;
use crate::errors::{Code, DeliveryError};
use crate::feature_flag;
use crate::js_value as jv;
use hlsr_core::js::Platform;
use hlsr_core::js::path::NodePath;
use serde_json::{Map, Value, json};

pub const QA_FLAG: &str = "HLSR_GAME_DELIVERY_QA";
pub const QA_LIBRARY_FLAG: &str = "HLSR_GAME_DELIVERY_QA_LIBRARY";
pub const QA_PATCHES_FLAG: &str = "HLSR_GAME_DELIVERY_QA_PATCHES";
pub const QA_MANIFEST_FLAG: &str = "HLSR_GAME_DELIVERY_QA_MANIFEST";
pub const QA_FILES_FLAG: &str = "HLSR_GAME_DELIVERY_QA_FILES";
pub const QA_VERSION: &str = "1.0.0";
pub const QA_PROVIDER_ID: &str = "yandex-public-temp";

/// `DEFAULT_QA_LIBRARY`: `C:\HLSR-Yandex-QA` в Windows, `~/Games/HLSR-Yandex-QA` в Linux.
pub fn default_qa_library() -> String {
    match Platform::CURRENT {
        Platform::Win32 => "C:\\HLSR-Yandex-QA".to_owned(),
        Platform::Linux => {
            let home = std::env::home_dir().unwrap_or_default();
            NodePath::native().join(&[&home.to_string_lossy(), "Games", "HLSR-Yandex-QA"])
        }
    }
}

/// Итог `evaluate`: включён ли режим и почему нет.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation {
    pub enabled: bool,
    pub reason: Option<&'static str>,
}

/// `evaluate({productionBuild, env})`.
pub fn evaluate(env: &Environment) -> Evaluation {
    let reason = if env.is_production() {
        Some("production-build")
    } else if env.var(QA_FLAG).as_deref() != Some("true") {
        Some("qa-flag-off")
    } else if !feature_flag::is_remote_delivery_enabled(env) {
        Some("remote-disabled")
    } else {
        None
    };
    Evaluation {
        enabled: reason.is_none(),
        reason,
    }
}

/// `isEnabled(env)`.
pub fn is_enabled(env: &Environment) -> bool {
    evaluate(env).enabled
}

/// `describe(env)`: `{flag, productionBuild, enabled, reason, libraryPath, version}`. Путь
/// библиотеки, который не проходит проверку, в 0.4.7 бросает исключение из `describe`; здесь —
/// `Err`.
pub fn describe(env: &Environment) -> Result<Value, DeliveryError> {
    let result = evaluate(env);
    let library = if result.enabled {
        Value::String(library_path(env)?)
    } else {
        Value::Null
    };
    Ok(json!({
        "flag": QA_FLAG,
        "productionBuild": env.is_production(),
        "enabled": result.enabled,
        "reason": result.reason,
        "libraryPath": library,
        "version": QA_VERSION,
    }))
}

/// `libraryPath(env)`: папка QA-библиотеки, абсолютная и не корень диска.
pub fn library_path(env: &Environment) -> Result<String, DeliveryError> {
    let raw = env
        .var(QA_LIBRARY_FLAG)
        .filter(|raw| !raw.is_empty())
        .unwrap_or_else(default_qa_library);
    let path = NodePath::native();
    let resolved = path.resolve(&[&raw]);
    let absolute = match Platform::CURRENT {
        Platform::Win32 => {
            let bytes = resolved.as_bytes();
            bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'\\' | b'/')
        }
        Platform::Linux => path.is_absolute(&raw),
    };
    if !absolute || path.dirname(&resolved) == resolved {
        return Err(DeliveryError::new(
            Code::PathInvalid,
            format!("Недопустимый путь QA-библиотеки: {raw}"),
        ));
    }
    Ok(resolved)
}

/// `JSON.parse(fs.readFileSync(file, "utf8"))`; ошибка — её текст.
fn read_json(file: &str) -> Result<Value, String> {
    let text = std::fs::read(file).map_err(|error| {
        crate::errors::RawError::fs(&error, "open", std::path::Path::new(file)).message
    })?;
    serde_json::from_slice(&text).map_err(|error| error.to_string())
}

/// `loadQaPatches(env)`: список патчей из файла `HLSR_GAME_DELIVERY_QA_PATCHES` (массив или
/// `{patches}`); переменная не задана — `None`.
pub fn load_qa_patches(env: &Environment) -> Result<Option<Value>, DeliveryError> {
    let Some(file) = env.var(QA_PATCHES_FLAG).filter(|file| !file.is_empty()) else {
        return Ok(None);
    };
    let data = read_json(&file).map_err(|message| {
        DeliveryError::new(
            Code::ManifestInvalid,
            format!("Не удалось прочитать QA-патчи из {file}: {message}"),
        )
    })?;
    if data.is_array() {
        return Ok(Some(data));
    }
    Ok(jv::prop(&data, "patches")
        .filter(|patches| hlsr_core::js::truthy(patches))
        .cloned())
}

/// `manifestFor(base, now, env)`: копия манифеста для QA — канал `test`, только временный
/// источник, патчи из файла.
pub fn manifest_for(
    base: &Value,
    now_iso: &str,
    env: &Environment,
) -> Result<Value, DeliveryError> {
    let mut copy = base.clone();
    let version = jv::prop(base, "version")
        .filter(|version| hlsr_core::js::truthy(version))
        .cloned()
        .unwrap_or_else(|| json!(QA_VERSION));
    let Some(object) = copy.as_object_mut() else {
        return Err(DeliveryError::new(
            Code::ManifestInvalid,
            "Манифест для QA-режима — не объект",
        ));
    };
    object.insert("channel".into(), json!("test"));
    object.insert("version".into(), version);
    object.insert("versionStatus".into(), json!("assigned"));
    object.insert("publishedAt".into(), json!(now_iso));
    object.insert("enabled".into(), json!(true));
    object.insert(
        "notes".into(),
        json!(format!(
            "RUNTIME-КОПИЯ ДЛЯ ЛОКАЛЬНОГО QA. Не публиковать. Создаётся в памяти при \
             {QA_FLAG}=true и не сохраняется на диск; файл манифеста в репозитории \
             остаётся выключенным черновиком."
        )),
    );

    let providers = object
        .get_mut("baseArchive")
        .and_then(|archive| archive.get_mut("providers"))
        .and_then(Value::as_array_mut);
    let Some(providers) = providers else {
        return Err(DeliveryError::new(
            Code::ProviderUnsupported,
            "В манифесте нет ни одного источника для QA-режима",
        ));
    };
    let temporary: Vec<Value> = providers
        .iter()
        .filter(|provider| jv::is_str(jv::prop(provider, "id"), QA_PROVIDER_ID))
        .cloned()
        .collect();
    if !temporary.is_empty() {
        *providers = temporary;
    }
    if providers.is_empty() {
        return Err(DeliveryError::new(
            Code::ProviderUnsupported,
            "В манифесте нет ни одного источника для QA-режима",
        ));
    }

    if let Some(patches) = load_qa_patches(env)? {
        let last = patches
            .as_array()
            .and_then(|list| list.last())
            .and_then(|last| jv::prop(last, "toVersion"))
            .filter(|version| hlsr_core::js::truthy(version))
            .cloned();
        object.insert("patches".into(), patches);
        if let Some(version) = last {
            object.insert("version".into(), version);
        }
    }

    Ok(copy)
}

/// `manifestOverrides(env)`: манифесты игр из файлов, перечисленных в
/// `HLSR_GAME_DELIVERY_QA_MANIFEST` (`{gameId: file}`).
pub fn manifest_overrides(env: &Environment) -> Result<Option<Map<String, Value>>, DeliveryError> {
    let Some(raw) = env.var(QA_MANIFEST_FLAG).filter(|raw| !raw.is_empty()) else {
        return Ok(None);
    };
    let map: Value = serde_json::from_str(&raw).map_err(|error| {
        DeliveryError::new(
            Code::ManifestInvalid,
            format!("{QA_MANIFEST_FLAG} не является JSON: {error}"),
        )
    })?;
    let mut result = Map::new();
    for (game_id, file) in map.as_object().into_iter().flatten() {
        let file = jv::template(Some(file));
        let manifest = read_json(&file).map_err(|message| {
            DeliveryError::new(
                Code::ManifestInvalid,
                format!("Не удалось прочитать QA-манифест {game_id} из {file}: {message}"),
            )
        })?;
        result.insert(game_id.clone(), manifest);
    }
    Ok(Some(result))
}

/// `manifestsFor(registry, now, env)`: QA-копии всех манифестов реестра.
pub fn manifests_for(
    registry: &[(String, Value)],
    now_iso: &str,
    env: &Environment,
) -> Result<Vec<(String, Value)>, DeliveryError> {
    let overrides = manifest_overrides(env)?.unwrap_or_default();
    registry
        .iter()
        .map(|(game_id, manifest)| {
            let base = overrides.get(game_id).unwrap_or(manifest);
            Ok((game_id.clone(), manifest_for(base, now_iso, env)?))
        })
        .collect()
}

/// Файловые манифесты игры: `(gameId, [(версия, манифест)])`.
pub type GameFilesManifests = (String, Vec<(String, Value)>);

/// `filesManifestsFor(registry, env)`: файловые манифесты реестра и дополнительные версии из
/// `HLSR_GAME_DELIVERY_QA_FILES` (`{gameId: {version: file}}`).
pub fn files_manifests_for(
    registry: &[GameFilesManifests],
    env: &Environment,
) -> Result<Vec<GameFilesManifests>, DeliveryError> {
    let mut result: Vec<GameFilesManifests> = registry.to_vec();
    let Some(raw) = env.var(QA_FILES_FLAG).filter(|raw| !raw.is_empty()) else {
        return Ok(result);
    };
    let extra: Value = serde_json::from_str(&raw).map_err(|error| {
        DeliveryError::new(
            Code::ManifestInvalid,
            format!("{QA_FILES_FLAG} не является JSON: {error}"),
        )
    })?;
    for (game_id, by_version) in extra.as_object().into_iter().flatten() {
        let index = match result.iter().position(|(id, _)| id == game_id) {
            Some(index) => index,
            None => {
                result.push((game_id.clone(), Vec::new()));
                result.len() - 1
            }
        };
        for (version, file) in by_version.as_object().into_iter().flatten() {
            let file = jv::template(Some(file));
            let manifest = read_json(&file).map_err(|message| {
                DeliveryError::new(
                    Code::ManifestInvalid,
                    format!(
                        "Не удалось прочитать файловый манифест {game_id} {version} из {file}: {message}"
                    ),
                )
            })?;
            let versions = &mut result[index].1;
            match versions.iter_mut().find(|(known, _)| known == version) {
                Some(entry) => entry.1 = manifest,
                None => versions.push((version.clone(), manifest)),
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &[(&str, &str)], production: bool) -> Environment {
        Environment::new(vars.iter().copied(), production)
    }

    #[test]
    fn qa_mode_needs_flag_remote_delivery_and_dev_build() {
        assert_eq!(
            evaluate(&env(&[(QA_FLAG, "true")], true)).reason,
            Some("production-build")
        );
        assert_eq!(evaluate(&env(&[], false)).reason, Some("qa-flag-off"));
        assert_eq!(
            evaluate(&env(&[(QA_FLAG, "1")], false)).reason,
            Some("qa-flag-off")
        );
        let off = describe(&env(&[], false)).unwrap();
        assert_eq!(off["enabled"], false);
        assert_eq!(off["libraryPath"], Value::Null);
        assert_eq!(off["version"], "1.0.0");
    }

    #[test]
    fn manifest_copy_keeps_only_temporary_source() {
        let base = json!({
            "gameId": "g", "version": "", "channel": "stable",
            "baseArchive": {"providers": [{"id": "drive"}, {"id": QA_PROVIDER_ID}]},
        });
        let copy = manifest_for(&base, "2026-01-01T00:00:00.000Z", &env(&[], false)).unwrap();
        assert_eq!(copy["channel"], "test");
        assert_eq!(copy["version"], "1.0.0");
        assert_eq!(
            copy["baseArchive"]["providers"],
            json!([{"id": QA_PROVIDER_ID}])
        );
        let none = json!({"baseArchive": {"providers": []}});
        assert_eq!(
            manifest_for(&none, "x", &env(&[], false)).unwrap_err().code,
            Code::ProviderUnsupported
        );
    }
}
