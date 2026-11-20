//! npm `discord-rpc` 4.0.1 (MIT), которым 0.4.7 говорит с Discord: транспорт IPC
//! (`src/transports/ipc.js`), клиент (`src/client.js`: `login`, `setActivity`, `clearActivity`,
//! `destroy`), `register` и `uuid` (`src/util.js`). Транспорт WebSocket и вход через OAuth
//! (`AUTHORIZE`, `AUTHENTICATE`) лаунчер не использует и не переносится.

pub mod client;
pub mod ipc;
pub mod register;
pub mod util;
