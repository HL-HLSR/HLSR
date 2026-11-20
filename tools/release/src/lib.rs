//! Инструменты выпуска HLSR (подпроект 6, задача 8): упаковка установщика Windows, архива и
//! AppImage Linux, `latest*.yml`, удостоверение QA-сборки, раскладка ресурсов. Подпись канала,
//! нарезка на куски и публикация остаются Node-скриптами 0.4.7 (`tools/release/node`, решение Р8):
//! они работают с ключом, и каноническая форма подписанного JSON проверена байт в байт.

pub mod appimage;
pub mod archive;
pub mod cli;
pub mod error;
pub mod hashes;
pub mod latest_yml;
pub mod pe_version;
pub mod qa_identity;
pub mod setup;
pub mod stage;
pub mod testing;
pub mod tree;
pub mod version;
