//! `discord-rpc/src/util.js`: `uuid` и `register`.

use super::register::ProtocolClient;
use hlsr_delivery::timing::Random;

/// `uuid4122()` библиотеки: 32 шестнадцатеричные цифры с дефисами после 8-й, 12-й, 16-й и 20-й.
/// Это не UUID RFC 4122, как ни называется функция: цифра версии (13-я) — 4, а «вариант»
/// (17-я) — `random & 3`, то есть 0…3. Discord принимает любой `nonce`.
pub fn uuid4122(random: &Random) -> String {
    let mut uuid = String::with_capacity(36);
    for i in 0..32 {
        if matches!(i, 8 | 12 | 16 | 20) {
            uuid.push('-');
        }
        let digit = if i == 12 {
            4
        } else {
            // `Math.random() * 16 | 0`.
            let value = (random() * 16.0) as u32;
            if i == 16 { value & 3 } else { value }
        };
        uuid.push(char::from_digit(digit, 16).unwrap_or('0'));
    }
    uuid
}

/// `register(id)` (`src/index.js`): просит у ОС протокол `discord-<id>`; `false` — не вышло.
pub fn register(client: &dyn ProtocolClient, id: &str) -> bool {
    client.set_as_default_protocol_client(&format!("discord-{id}"))
}
