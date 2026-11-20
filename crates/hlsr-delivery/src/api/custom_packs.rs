//! Моды игры в персонализации (`customPacks`, `customPackSound` из `index.js` 0.4.9): список,
//! включение и выключение, звук для прослушивания. Файлы меняются только при закрытой игре и без
//! другой работы с играми (установка, проверка, починка).

use super::{DeliveryApi, error_code};
use crate::custom_packs;
use crate::errors::Failure;
use crate::js_value as jv;
use serde_json::{Value, json};

/// `error.code` (у ошибки fs — её код; нет кода — пусто).
fn code_of(error: &Failure) -> String {
    match error {
        Failure::Delivery(error) => error.code.as_str().to_owned(),
        Failure::Raw(error) => error.code.clone().unwrap_or_default(),
    }
}

/// Код ошибки для окна: `error.code || "CUSTOM_PACK_FAILED"`.
fn failure_code(error: &Failure) -> String {
    Some(code_of(error))
        .filter(|code| !code.is_empty())
        .unwrap_or_else(|| "CUSTOM_PACK_FAILED".to_owned())
}

impl DeliveryApi {
    /// `custom-packs-get` (без `change`) и `custom-packs-set` (`change` = `(packId, enabled)`).
    fn custom_packs(&self, id: &Value, change: Option<(&Value, &Value)>) -> Value {
        let game_id = self.resolve_game_id(id).unwrap_or_default();
        if !custom_packs::has_packs(&game_id) {
            return error_code("UNKNOWN_GAME");
        }
        let install = self.installed_path_of(&game_id);
        let service = self.ensure_service();
        // Слот — до проверки игры: запуск, установка и починка не начнутся посреди копий.
        let slot = service
            .as_ref()
            .and_then(|service| service.reserve_custom_packs(&game_id));
        let blocked = match &service {
            None => Some("NOT_READY"),
            Some(service) if service.is_game_running(&game_id) => Some("GAME_RUNNING"),
            Some(_) if slot.is_none() => Some("BUSY"),
            Some(_) => None,
        };
        let logger = self.logger();
        let result = (|| -> Result<Value, Failure> {
            if let (Some((pack_id, enabled)), Some(install), None) = (change, &install, blocked) {
                let pack_id = jv::template(Some(pack_id));
                if !custom_packs::PACKS
                    .iter()
                    .any(|pack| pack.id == pack_id && pack.game == game_id)
                {
                    return Ok(error_code("CUSTOM_PACK_UNKNOWN"));
                }
                let packs = custom_packs::set_enabled(
                    install,
                    &pack_id,
                    jv::truthy(Some(enabled)),
                    logger,
                    None,
                )?;
                return Ok(json!({"status": "ok", "packs": packs}));
            }
            let packs =
                custom_packs::list(install.as_deref(), &game_id, blocked.is_none(), logger)?;
            if change.is_none() {
                return Ok(json!({"status": "ok", "installed": install.is_some(), "packs": packs}));
            }
            let code = if install.is_some() {
                blocked.map_or(Value::Null, Value::from)
            } else {
                Value::from("NOT_INSTALLED")
            };
            Ok(json!({"status": "error", "code": code, "packs": packs}))
        })();
        result.unwrap_or_else(|error| {
            let what = match change {
                Some((pack_id, _)) => format!("пак {}", jv::template(Some(pack_id))),
                None => "список".to_owned(),
            };
            logger.warn(&format!(
                "[custom-packs] {what}: {} {error}",
                code_of(&error)
            ));
            let code = failure_code(&error);
            let packs = custom_packs::list(install.as_deref(), &game_id, false, logger)
                .map_or(Value::Null, Value::from);
            json!({"status": "error", "code": code, "packs": packs})
        })
    }

    /// [`Self::custom_packs`] вне потоков tokio: хеши и копии файлов паков — блокирующий ввод-вывод.
    async fn custom_packs_blocking(&self, id: &Value, change: Option<(&Value, &Value)>) -> Value {
        let Some(api) = self.weak.upgrade() else {
            return error_code("NOT_READY");
        };
        let id = id.clone();
        let change = change.map(|(pack_id, enabled)| (pack_id.clone(), enabled.clone()));
        tokio::task::spawn_blocking(move || {
            api.custom_packs(
                &id,
                change.as_ref().map(|(pack_id, enabled)| (pack_id, enabled)),
            )
        })
        .await
        .unwrap_or_else(|_| error_code("CUSTOM_PACK_FAILED"))
    }

    /// `custom-packs-get` (id): `{status, installed, packs}`.
    pub async fn custom_packs_get(&self, id: &Value) -> Value {
        self.custom_packs_blocking(id, None).await
    }

    /// `custom-packs-set` (id, packId, enabled): `{status, packs}`.
    pub async fn custom_packs_set(&self, id: &Value, pack_id: &Value, enabled: &Value) -> Value {
        self.custom_packs_blocking(id, Some((pack_id, enabled)))
            .await
    }

    /// `custom-packs-sound` (id, packId, file, which `pack|original`): `{status, dataUrl}`.
    pub fn custom_packs_sound(
        &self,
        id: &Value,
        pack_id: &Value,
        file: &Value,
        which: &Value,
    ) -> Value {
        let game_id = self.resolve_game_id(id).unwrap_or_default();
        let install = custom_packs::has_packs(&game_id)
            .then(|| self.installed_path_of(&game_id))
            .flatten();
        let Some(install) = install else {
            return error_code("NOT_INSTALLED");
        };
        let pack_id = jv::template(Some(pack_id));
        if !custom_packs::PACKS
            .iter()
            .any(|pack| pack.id == pack_id && pack.game == game_id)
        {
            return error_code("CUSTOM_PACK_UNKNOWN");
        }
        match custom_packs::read_sound(
            &install,
            &pack_id,
            &jv::template(Some(file)),
            which.as_str(),
        ) {
            Ok(data_url) => json!({"status": "ok", "dataUrl": data_url}),
            Err(Failure::Raw(error)) if error.code.as_deref() == Some("ENOENT") => {
                error_code("CUSTOM_PACK_NOT_AVAILABLE")
            }
            Err(error) => error_code(&failure_code(&error)),
        }
    }
}
