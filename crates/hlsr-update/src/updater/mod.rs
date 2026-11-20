//! Внешний апдейтер Windows (`src/updater` 0.4.7 и `launcher-update/updater-launch.js`,
//! `updater-mode.js`): копия `HLSR.exe` во временном каталоге проверяет план, качает установщик (или
//! собирает версию из чанков), ждёт выхода лаунчера, ставит обновление и запускает лаунчер. Окно
//! апдейтера — задача 10; здесь всё, что без окна (карта, строки 6.12a, 6.12b).

pub mod download;
pub mod install;
pub mod launch;
pub mod log;
pub mod mode;
pub mod paths;
pub mod plan;
pub mod process_wait;
pub mod processes;
pub mod receipt;
pub mod session;
pub mod single_instance;
