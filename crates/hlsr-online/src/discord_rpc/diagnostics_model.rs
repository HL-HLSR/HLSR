//! `discord-rpc/diagnostics-model.js`: снимок подключения к Discord и моста игры для окна —
//! только разрешённые поля, приведённые к своим типам.

use super::constants::{Code, State};
use crate::js_value::{field, is_object, to_number};
use hlsr_core::js;
use hlsr_core::json_text::js_number;
use serde_json::{Value, json};

/// `ALLOWED_KEYS`: поля снимка.
pub const ALLOWED_KEYS: [&str; 7] = [
    "enabled",
    "state",
    "connected",
    "lastStableCode",
    "reconnectPending",
    "lastConnectedAt",
    "gameBridge",
];

/// `BRIDGE_KEYS`: поля моста игры.
pub const BRIDGE_KEYS: [&str; 5] = ["listening", "connected", "rejected", "lifecycle", "hasMap"];
const BRIDGE_LIFECYCLES: [&str; 3] = ["menu", "playing", "exited"];

/// `RECONNECT_STATES`: состояния, в которых ждётся повторное подключение.
pub const RECONNECT_STATES: [State; 3] =
    [State::RetryWait, State::Unavailable, State::Disconnected];

/// Мост игры в снимке.
#[derive(Debug, Clone, PartialEq)]
pub struct Bridge {
    pub listening: bool,
    pub connected: bool,
    /// Отклонённые подключения: неотрицательное целое число JS.
    pub rejected: f64,
    pub lifecycle: Option<&'static str>,
    pub has_map: bool,
}

impl Bridge {
    pub fn to_json(&self) -> Value {
        json!({
            "listening": self.listening,
            "connected": self.connected,
            "rejected": js_number(self.rejected),
            "lifecycle": self.lifecycle,
            "hasMap": self.has_map,
        })
    }
}

/// Снимок подключения.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostics {
    pub enabled: bool,
    pub state: State,
    pub connected: bool,
    pub last_stable_code: Option<Code>,
    pub reconnect_pending: bool,
    pub last_connected_at: Option<f64>,
    pub game_bridge: Option<Bridge>,
}

impl Diagnostics {
    /// `DISABLED`: присутствие выключено.
    pub const DISABLED: Diagnostics = Diagnostics {
        enabled: false,
        state: State::Disabled,
        connected: false,
        last_stable_code: Some(Code::Disabled),
        reconnect_pending: false,
        last_connected_at: None,
        game_bridge: None,
    };

    pub fn to_json(&self) -> Value {
        json!({
            "enabled": self.enabled,
            "state": self.state.as_str(),
            "connected": self.connected,
            "lastStableCode": self.last_stable_code.map(Code::as_str),
            "reconnectPending": self.reconnect_pending,
            "lastConnectedAt": self.last_connected_at.map_or(Value::Null, js_number),
            "gameBridge": self.game_bridge.as_ref().map_or(Value::Null, Bridge::to_json),
        })
    }
}

/// `normalizeBridge(raw)`: не объект — `None`.
pub fn normalize_bridge(raw: Option<&Value>) -> Option<Bridge> {
    if !is_object(raw) {
        return None;
    }
    let get = |key: &str| field(raw, key);
    let is_true = |key: &str| get(key) == Some(&Value::Bool(true));
    let rejected = to_number(get("rejected"));
    Some(Bridge {
        listening: is_true("listening"),
        connected: is_true("connected"),
        rejected: if rejected.is_finite() && rejected >= 0.0 {
            // `Math.floor(-0)` — `-0`, в JSON это 0.
            rejected.floor() + 0.0
        } else {
            0.0
        },
        lifecycle: get("lifecycle")
            .and_then(Value::as_str)
            .and_then(|text| BRIDGE_LIFECYCLES.iter().copied().find(|l| *l == text)),
        has_map: is_true("hasMap"),
    })
}

/// `normalize(raw)`: выключено (или не объект) — [`Diagnostics::DISABLED`] с мостом;
/// неизвестное состояние — `disconnected`; код — первый истинный из `lastCode`,
/// `lastStableCode`, `code`, если он известен.
pub fn normalize(raw: &Value) -> Diagnostics {
    let raw = Some(raw);
    if !is_object(raw) {
        return Diagnostics::DISABLED;
    }
    let get = |key: &str| field(raw, key);
    let game_bridge = normalize_bridge(get("gameBridge"));

    if get("enabled") != Some(&Value::Bool(true)) {
        return Diagnostics {
            game_bridge,
            ..Diagnostics::DISABLED
        };
    }

    let state = get("state")
        .and_then(Value::as_str)
        .and_then(State::parse)
        .unwrap_or(State::Disconnected);
    let code = ["lastCode", "lastStableCode", "code"]
        .into_iter()
        .filter_map(get)
        .find(|value| js::truthy(value));
    let at = to_number(get("lastConnectedAt"));

    Diagnostics {
        enabled: true,
        state,
        connected: state == State::Connected,
        last_stable_code: code.and_then(Value::as_str).and_then(Code::parse),
        reconnect_pending: RECONNECT_STATES.contains(&state),
        last_connected_at: (at.is_finite() && at > 0.0).then(|| at.floor()),
        game_bridge,
    }
}
