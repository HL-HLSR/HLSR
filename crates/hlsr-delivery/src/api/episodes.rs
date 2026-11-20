//! Каналы эпизодов Ghosting (`index.js` 0.4.9): `game-episodes-status`, `game-episode-install`,
//! `game-episode-remove`. Id эпизода (`"ep1" | "ep2"`) проверяет служба.

use super::DeliveryApi;
use crate::episode_install::GAME_ID;
use crate::episodes;
use serde_json::Value;

impl DeliveryApi {
    /// `game-episodes-status`: `{ep1, ep2: {installed, version, size, paused, available}}`.
    pub async fn game_episodes_status(&self) -> Value {
        match self.ensure_service() {
            Some(service) => service.episodes_status().await,
            None => Value::Null,
        }
    }

    /// `game-episode-install` (id): ep2 ставит ep1 первым.
    pub async fn game_episode_install(&self, id: &Value) -> Value {
        self.run(|service| async move {
            let id = episodes::episode_id_of(id)?;
            service.install_episode(id).await
        })
        .await
    }

    /// `game-episode-remove` (id): удаляет и зависимые (ep1 → ep2).
    pub async fn game_episode_remove(&self, id: &Value) -> Value {
        self.run(|service| async move {
            let id = episodes::episode_id_of(id)?;
            service.remove_episode(id)
        })
        .await
    }

    /// Процесс Ghosting вышел (`finished`, и подхваченный после перезапуска): `config.cfg`
    /// эпизода — в Ghosting. Зовёт подсистема запуска, когда в событии есть `episode`.
    pub fn return_episode_configs(&self, game_id: &str, episode: &str) {
        if game_id == GAME_ID
            && let Some(service) = self.service()
        {
            service.return_episode_configs(game_id, Some(&[episode]));
        }
    }
}
