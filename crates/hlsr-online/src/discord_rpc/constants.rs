//! `discord-rpc/constants.js`: приложение Discord, картинки, тексты игры, состояния и коды
//! подключения, кнопки, задержки, каналы окна.

use hlsr_delivery::links::LINKS;

/// Приложение Discord лаунчера.
pub const APPLICATION_ID: &str = "1531057982518071296";

/// `ASSETS`: ключи картинок приложения.
pub mod assets {
    pub const LARGE: &str = "hlsr";
    pub const MENU: &str = "background";
    pub const SMALL_IDLE: &str = "playing";
    pub const SMALL_RUNNING: &str = "running";
}

pub const ALLOWED_SMALL_IMAGE_KEYS: [&str; 2] = ["running", "playing"];
pub const ALLOWED_SMALL_IMAGE_TEXTS: [&str; 3] = ["Run in progress", "Playing", "In the menu"];

pub const GAME_TITLE: &str = "Ghosting Mod: Renewed";

/// `GAME_TEXT`: тексты присутствия в игре — всегда по-английски.
pub mod game_text {
    pub const TITLE: &str = super::GAME_TITLE;
    pub const MENU_STATE: &str = "In the menu";
    pub const PLAYING_STATE: &str = "Playing";
    pub const RUNNING_SMALL: &str = "Run in progress";
    pub const FPS_LABEL: &str = "FPS";
    pub const GAME_LABEL: &str = "Game";
}

/// `STATES`: состояние подключения к Discord.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Disabled,
    Disconnected,
    Connecting,
    Connected,
    RetryWait,
    Unavailable,
    ShuttingDown,
}

impl State {
    /// В порядке объекта `STATES`.
    pub const ALL: [State; 7] = [
        State::Disabled,
        State::Disconnected,
        State::Connecting,
        State::Connected,
        State::RetryWait,
        State::Unavailable,
        State::ShuttingDown,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            State::Disabled => "disabled",
            State::Disconnected => "disconnected",
            State::Connecting => "connecting",
            State::Connected => "connected",
            State::RetryWait => "retry_wait",
            State::Unavailable => "unavailable",
            State::ShuttingDown => "shutting_down",
        }
    }

    /// Ключ объекта `STATES`: `RETRY_WAIT`.
    pub fn name(self) -> String {
        self.as_str().to_uppercase()
    }

    pub fn parse(text: &str) -> Option<State> {
        State::ALL.into_iter().find(|state| state.as_str() == text)
    }
}

/// `CODES`: последний устойчивый код подключения (для окна диагностики).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    Disabled,
    Connecting,
    Connected,
    DiscordUnavailable,
    Disconnected,
    UpdateFailed,
    Shutdown,
}

impl Code {
    /// В порядке объекта `CODES`.
    pub const ALL: [Code; 7] = [
        Code::Disabled,
        Code::Connecting,
        Code::Connected,
        Code::DiscordUnavailable,
        Code::Disconnected,
        Code::UpdateFailed,
        Code::Shutdown,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Code::Disabled => "RPC_DISABLED",
            Code::Connecting => "RPC_CONNECTING",
            Code::Connected => "RPC_CONNECTED",
            Code::DiscordUnavailable => "RPC_DISCORD_UNAVAILABLE",
            Code::Disconnected => "RPC_DISCONNECTED",
            Code::UpdateFailed => "RPC_UPDATE_FAILED",
            Code::Shutdown => "RPC_SHUTDOWN",
        }
    }

    /// Ключ объекта `CODES`: `DISCORD_UNAVAILABLE`.
    pub fn name(self) -> &'static str {
        &self.as_str()["RPC_".len()..]
    }

    pub fn parse(text: &str) -> Option<Code> {
        Code::ALL.into_iter().find(|code| code.as_str() == text)
    }
}

/// Кнопки присутствия — ключи [`LINKS`].
pub const BUTTON_KEYS: [&str; 2] = ["site", "discord"];

/// `TIMINGS`.
pub mod timings {
    /// Задержка отправки присутствия после изменения.
    pub const DEBOUNCE_MS: u64 = 1500;
    pub const RETRY_BASE_MS: u64 = 5000;
    pub const RETRY_MAX_MS: u64 = 5 * 60 * 1000;
    pub const RETRY_JITTER: f64 = 0.25;
    pub const LOG_THROTTLE_MS: u64 = 60 * 1000;
}

/// `CHANNELS`: каналы окна.
pub mod channels {
    pub const GET: &str = "discord-rpc-get";
    pub const SET: &str = "discord-rpc-set";
    pub const STATE: &str = "discord-rpc-state";
    /// Окно → мост: раздел, где сейчас игрок (`presence_model::ACTIVITY_KINDS`).
    pub const ACTIVITY: &str = "discord-rpc-activity";
}

/// `buttonUrls()`: ключ кнопки и адрес из [`LINKS`]; ключи без адреса пропускаются.
pub fn button_urls() -> Vec<(&'static str, &'static str)> {
    BUTTON_KEYS
        .iter()
        .filter_map(|key| {
            LINKS
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, url)| (*key, *url))
        })
        .collect()
}
