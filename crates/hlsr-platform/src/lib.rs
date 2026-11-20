//! Запуск и платформа без интерфейса: то, что 0.4.7 делает для игр после установки — процессы
//! и их состояния, остановка, Steam (поиск, аккаунт, друзья и аватары, депо), статистика игр и
//! мост присутствия с игрой, дампы падений и трей во время игры. Модули повторяют модули 0.4.7
//! (имя файла JS → `snake_case.rs`, `linux/` — как в `src/main/linux/`), функции — те же имена
//! в `snake_case`. Крейт реализует порты `hlsr_delivery::ports` (`ProcessProbe`,
//! `ProcessStopper`, `SteamDepot`, `ToolRunner`), а наружу даёт [`api::PlatformApi`].

pub mod api;
pub mod child_env;
pub mod crash_dumps;
pub mod dep_compat;
pub mod depot_install;
pub mod elevation;
pub mod game_presence;
pub mod game_process_service;
pub mod game_stats;
pub mod goldsrc_launch;
pub mod launch_service;
pub mod legacy_app_id;
pub mod linux;
pub mod livesplit_overlay;
pub mod livesplit_overlay_shared;
pub mod livesplit_window_capture;
pub mod net_fallback;
pub mod process_inspect;
pub mod process_stop;
pub mod profile;
#[cfg(any(debug_assertions, feature = "dev-hooks"))]
pub mod qa_launch;
pub mod rinput_menu_fix;
#[cfg(windows)]
pub mod rinput_menu_fix_win;
pub mod spawn;
pub mod steam_account;
pub mod steam_depot;
pub mod steam_fs;
pub mod steam_locator;
pub mod tool_runner;
pub mod tray_controller;
pub mod vdf;
pub mod windows_tools;
