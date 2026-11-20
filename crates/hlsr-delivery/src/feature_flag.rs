//! Флаги доставки (`feature-flag.js`): удалённая доставка, локальный HTTP,
//! разрешённые типы источников.

use crate::channel_config::{self, is_production_build};
use crate::environment::Environment;

pub const FLAG: &str = "GAME_DELIVERY_REMOTE_ENABLED";
pub const LOCAL_HTTP_FLAG: &str = "GAME_DELIVERY_ALLOW_LOCAL_HTTP";

fn is_truthy(value: Option<String>) -> bool {
    matches!(value.as_deref(), Some("true" | "1"))
}

/// Встроенный канал включён (в 0.4.7 — только в production).
pub fn is_release_profile_active(env: &Environment) -> bool {
    channel_config::has_active_release_profile(env)
}

pub fn is_remote_delivery_enabled(env: &Environment) -> bool {
    is_truthy(env.var(FLAG)) || is_release_profile_active(env)
}

/// Локальный HTTP — не в production, а в этой реализации ещё и только в сборке с
/// отладочными возможностями.
pub fn is_local_http_allowed(env: &Environment) -> bool {
    !is_production_build() && !env.is_production() && is_truthy(env.var(LOCAL_HTTP_FLAG))
}

pub fn allowed_provider_types(env: &Environment) -> Vec<&'static str> {
    let mut types = Vec::new();
    if is_remote_delivery_enabled(env) {
        types.extend(["yandex-public", "https-direct"]);
    }
    if is_local_http_allowed(env) {
        types.push("local-http");
    }
    types
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Description {
    pub flag: &'static str,
    pub remote_enabled: bool,
    pub release_profile_active: bool,
    pub local_http_allowed: bool,
    pub allowed_provider_types: Vec<&'static str>,
}

pub fn describe(env: &Environment) -> Description {
    Description {
        flag: FLAG,
        remote_enabled: is_remote_delivery_enabled(env),
        release_profile_active: is_release_profile_active(env),
        local_http_allowed: is_local_http_allowed(env),
        allowed_provider_types: allowed_provider_types(env),
    }
}
