//! Системные программы Windows по полному пути (`windows-tools.js`): каталог WindowsPowerShell
//! бывает выпадает из `PATH` (сторонние установщики, «чистка» переменных), и программа по имени
//! не находится вовсе. PowerShell в Rust не вызывается (процессы, DEP, маска ЦП, права
//! администратора — прямые вызовы WinAPI, решение 2 спецификации), но путь к нему и каталог
//! системы нужны тем, кто ищет системные программы (`tasklist.exe`, `taskkill.exe`).

use hlsr_core::js::Platform;
use hlsr_core::js::path::NodePath;
use hlsr_delivery::environment::Environment;

/// `systemRoot(env)`: `SystemRoot`, иначе `WINDIR`, иначе `C:\Windows`.
pub fn system_root(env: &Environment) -> String {
    let given = |name: &str| env.var(name).filter(|value| !value.is_empty());
    given("SystemRoot")
        .or_else(|| given("WINDIR"))
        .unwrap_or_else(|| "C:\\Windows".to_owned())
}

/// `powershellPath(env)`: `<SystemRoot>\System32\WindowsPowerShell\v1.0\powershell.exe`.
pub fn powershell_path(env: &Environment) -> String {
    NodePath::new(Platform::Win32, "C:\\").join(&[
        &system_root(env),
        "System32",
        "WindowsPowerShell",
        "v1.0",
        "powershell.exe",
    ])
}
