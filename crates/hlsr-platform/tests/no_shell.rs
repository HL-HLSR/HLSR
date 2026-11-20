//! Без оболочки (C-6, A-4/A-5 аудита 07.11): в коде лаунчера (`crates/*/src`, `tools/*/src`) нет
//! запуска `cmd.exe` (`cmd /c …`) и `%ComSpec%` — путь с `&` или `^` не должен разбираться как
//! команда. Аналог статической проверки `exec/execSync/ComSpec` 0.4.9.

use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Строка запускает оболочку: `cmd`/`cmd.exe` как программа, ключ `"/c"` или `ComSpec`.
fn shell_call(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.contains("new(\"cmd\"")
        || lower.contains("\"cmd.exe\"")
        || lower.contains("\"/c\"")
        || lower.contains("comspec")
}

#[test]
fn no_cmd_exe_in_launcher_sources() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let mut files = Vec::new();
    for group in ["crates", "tools"] {
        let Ok(members) = std::fs::read_dir(root.join(group)) else {
            continue;
        };
        for member in members.flatten() {
            rust_files(&member.path().join("src"), &mut files);
        }
    }
    assert!(files.len() > 100, "исходников мало: {}", files.len());
    let found: Vec<String> = files
        .iter()
        .flat_map(|file| {
            let text = std::fs::read_to_string(file).unwrap_or_default();
            text.lines()
                .enumerate()
                .filter(|(_, line)| shell_call(line))
                .map(|(at, line)| format!("{}:{}: {}", file.display(), at + 1, line.trim()))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(found.is_empty(), "запуск оболочки:\n{}", found.join("\n"));
}

#[test]
fn the_check_sees_a_shell_call() {
    assert!(shell_call(
        r#"Command::new("cmd.exe").args(["/c", "mklink"])"#
    ));
    assert!(shell_call(r#"std::env::var("ComSpec")"#));
    assert!(!shell_call(r#"json!({"cmd": "DISPATCH"})"#));
}
