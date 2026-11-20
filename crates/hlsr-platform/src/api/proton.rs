//! Proton в фасаде (`AppManager.js:643-692`, только Linux): каналы `proton-*` и события
//! менеджера — уведомления `PROTON_*` и событие окна `proton-state`.

use super::{PlatformApi, channels};
use crate::linux::proton_manager::ProtonEvent;
use hlsr_delivery::ports::OpenDialog;
use serde_json::{Value, json};
use std::sync::Arc;

/// `String(tag).replace(/^wineland-/, "")`.
fn short(tag: &str) -> &str {
    tag.strip_prefix("wineland-").unwrap_or(tag)
}

fn text_of(payload: &Value, key: &str) -> String {
    hlsr_core::js::to_template_string(payload.get(key))
}

impl PlatformApi {
    /// Подписка на события менеджера Proton (конструктор фасада).
    pub(crate) fn attach_proton_events(&self) {
        let Some(proton) = &self.options.proton else {
            return;
        };
        let me = self.me.clone();
        proton
            .manager
            .subscribe(Arc::new(move |event: &ProtonEvent| {
                if let Some(api) = me.upgrade() {
                    api.on_proton_event(event);
                }
            }));
    }

    /// Событие менеджера Proton (`protonManager.on(…)` в `AppManager.js`): уведомления и
    /// `proton-state`. Вызывается подпиской конструктора; открыт для проверок.
    pub fn on_proton_event(&self, event: &ProtonEvent) {
        let payload = &event.payload;
        let add = |level: &str, code: &str, detail: &str, meta: Option<Value>| {
            if let Some(notifications) = &self.options.notifications
                && let Err(error) = notifications.add(level, code, Some(detail), meta.as_ref())
            {
                self.logger().warn(&format!(
                    "[notifications] уведомление {code} не записано: {error}"
                ));
            }
        };
        let remove_offers = || {
            if let Some(notifications) = &self.options.notifications {
                let _ = notifications.remove_where(|item| {
                    item.get("code").and_then(Value::as_str) == Some("PROTON_UPDATE_AVAILABLE")
                });
            }
        };
        match event.name {
            "install-started" => add(
                "info",
                "PROTON_INSTALLING",
                short(&text_of(payload, "version")),
                None,
            ),
            "installed" => add(
                "info",
                "PROTON_INSTALLED",
                short(&text_of(payload, "version")),
                None,
            ),
            "update-available" => {
                let to = text_of(payload, "to");
                if let Some(notifications) = &self.options.notifications {
                    let (items, _) = notifications.list();
                    let offered = items.iter().any(|item| {
                        item.get("code").and_then(Value::as_str) == Some("PROTON_UPDATE_AVAILABLE")
                            && item
                                .get("meta")
                                .and_then(|meta| meta.get("newVersion"))
                                .and_then(Value::as_str)
                                == Some(to.as_str())
                    });
                    if offered {
                        return;
                    }
                }
                remove_offers();
                add(
                    "info",
                    "PROTON_UPDATE_AVAILABLE",
                    &format!("{} → {}", short(&text_of(payload, "from")), short(&to)),
                    Some(json!({"target": "proton", "newVersion": payload.get("to")})),
                );
            }
            "updated" => {
                remove_offers();
                add(
                    "info",
                    "PROTON_UPDATED",
                    &format!(
                        "{} → {}",
                        short(&text_of(payload, "from")),
                        short(&text_of(payload, "to"))
                    ),
                    None,
                );
            }
            "state" => {
                let failed = payload.get("phase").and_then(Value::as_str) == Some("error");
                let first = {
                    let mut state = self.state();
                    let first = failed && !state.proton_failed;
                    state.proton_failed = failed;
                    first
                };
                if first {
                    add(
                        "error",
                        "PROTON_INSTALL_FAILED",
                        &text_of(payload, "error"),
                        None,
                    );
                }
                self.emit_to(channels::PROTON_STATE, payload.clone());
            }
            _ => {}
        }
    }

    /// Каналы `proton-*`; без Proton (Windows) их нет — `None`.
    pub(crate) async fn proton_call(
        &self,
        channel: &str,
        args: &[Value],
    ) -> Option<Result<Value, String>> {
        let proton = self.options.proton.clone()?;
        let manager = &proton.manager;
        let result = match channel {
            "proton-cancel" => Ok(json!(manager.cancel())),
            "proton-status" => Ok(manager.status()),
            "proton-select" => {
                let path = hlsr_core::js::to_template_string(args.first());
                manager
                    .select(&path)
                    .map(|entry| entry.to_json())
                    .map_err(|error| error.message)
            }
            "proton-add" => {
                let chosen = self
                    .options
                    .dialogs
                    .show_open_dialog(OpenDialog {
                        title: Some("Выберите папку Proton".to_owned()),
                        properties: vec!["openDirectory"],
                        ..OpenDialog::default()
                    })
                    .await;
                match chosen.first() {
                    None => Ok(Value::Null),
                    Some(path) => manager
                        .add_custom(&path.to_string_lossy())
                        .map(|entry| entry.map_or(Value::Null, |entry| entry.to_json()))
                        .map_err(|error| error.message),
                }
            }
            "proton-download" => manager
                .ensure_reference()
                .await
                .map(|_| manager.status())
                .map_err(|error| error.message),
            "proton-update" => manager
                .apply_update()
                .await
                .map(|_| manager.status())
                .map_err(|error| error.message),
            _ => return None,
        };
        Some(result)
    }
}
