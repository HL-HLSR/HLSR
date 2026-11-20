//! `game-presence/`: справочник карт. Мост игры (сервер, протокол, состояние) — в
//! `hlsr-platform::game_presence`; справочник нужен и ему, и модели Discord, поэтому лежит в
//! `hlsr-core` ([`hlsr_core::map_assets`]) и здесь только повторён под прежним именем.

pub use hlsr_core::map_assets;
