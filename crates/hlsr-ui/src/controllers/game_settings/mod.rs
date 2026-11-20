//! Настройки игры на странице «Персонализация» (`GameSettingsModal.vue` embedded 0.4.9,
//! `GameConfigEditor.vue`): разделы «Параметры запуска», «Версия», «Призраки» и «Конфиг»
//! (редактор настроек и импорт cfg), общий подвал. «Клавиатура», «Скрипты», внешний вид игры и
//! «Демо» — свои части страницы; сессия держит их общий черновик ([`FilesSlot`]).
//!
//! Логика без UI: [`Session`] ведёт состояние и вызовы главного процесса, вид страницы
//! (`hlsr-gpui` `configs/`) рисует [`View`]. Куски, общие с импортом биндов
//! (задача 29): [`import::pick_chain`], [`import::with_companions`], [`import::import_report`].

pub mod config;
pub mod draft;
pub mod import;
pub mod records;
pub mod sections;
pub mod session;
pub mod tasks;

pub use config::{ConfigEditor, Edits, FieldView, Kind, SectionView, TabItem};
pub use draft::{Draft, Item};
pub use session::{
    ConfigMode, EditorView, Env, Event, FilesHook, FilesSlot, FooterView, LoadState, RunRow,
    RunsBody, SelectView, Session, TabView, ToggleView, View, VulkanView, Zone,
};
