//! Установка и удаление на Windows (`windows-install/`, `launcher-update/install-location.js`
//! 0.4.7): раскладка и ключи реестра, список файлов сборки, реестр, ярлыки `.lnk`, известные папки,
//! каталог установки в записи «Программ и компонентов»; установка и удаление, режимы установщика,
//! нагрузка `HLSR-Setup.exe` (задача 5).
//!
//! Модули этого каталога не зависят от ОС (проверяются и на Linux): всё, что говорит с Windows,
//! спрятано за трейтами [`registry::Registry`], [`known_folders::KnownFolders`],
//! [`shortcuts::ShellLinks`], а реализации `System*` собираются только под `cfg(windows)`.
//! Настоящий реестр, ярлыки и профиль владельца в тестах не трогаются: их заменяют подделки.

#[cfg(any(debug_assertions, feature = "dev-hooks"))]
pub mod dev_hooks;
pub mod file_list;
pub mod file_ops;
pub mod install;
pub mod install_location;
pub mod known_folders;
pub mod layout;
pub mod payload;
pub mod processes;
pub mod registry;
pub mod setup;
pub mod shortcuts;
pub mod taskbar;
pub mod uninstall;
