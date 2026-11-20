//! Удаление и очистка (`uninstall/` 0.4.7): проверки и очистка библиотеки игр, настроек и профиля
//! после удаления лаунчера, режим `--hlsr-uninstall-cleanup`. Удаление на Linux
//! (`launcher-uninstall.js`) — в `linux::launcher_uninstall`.

pub mod cleanup_cli;
pub mod library_cleanup;
