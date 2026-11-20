//! Кнопка карточки Ghosting, когда в «Версии сборки» выбран эпизод (`shared/episode-button.js`
//! 0.4.9). Эпизод подменяет только «Играть» установленной Ghosting: всё остальное (установка,
//! обновление, ремонт, ошибки, загрузка, запущенная игра) — кнопка Ghosting (`main_button`).

use crate::js_value as jv;
use serde_json::{Value, json};

fn view(state: &str, action: &str) -> Value {
    json!({ "state": state, "action": action, "enabled": true, "spinner": false, "progress": false })
}

/// `episodeButton(mainButton, delivery, episodeId)`.
pub fn episode_button(
    main_button: &Value,
    delivery: Option<&Value>,
    episode_id: Option<&str>,
) -> Value {
    let Some(episode_id) = episode_id.filter(|id| !id.is_empty()) else {
        return main_button.clone();
    };
    if main_button.get("state").and_then(Value::as_str) != Some("ready") {
        return main_button.clone();
    }
    let episode = delivery
        .and_then(|delivery| delivery.get("episodes"))
        .and_then(|episodes| episodes.get(episode_id));
    let flag = |key: &str| jv::truthy(episode.and_then(|episode| episode.get(key)));
    if flag("paused") {
        return view("episode-paused", "resume");
    }
    if !flag("installed") {
        return view("episode-not-installed", "install");
    }
    main_button.clone()
}
