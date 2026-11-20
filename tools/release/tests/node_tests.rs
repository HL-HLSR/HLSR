//! Node-тесты инструментов выпуска (`tools/release/node/tests`) и замороженных копий 0.4.7
//! (`tools/compat-047/tests`) входят в `cargo test`: отдельной цепочки тестов Node в 0.5 нет.
//! Нужен `node` в PATH (как для фикстур `tools/reference`); сверка копий побайтно с `~/HLSR-electron`
//! идёт, если эталонная копия есть, и пропускается с причиной, если её нет.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("корень репозитория")
        .to_owned()
}

fn test_files(directory: &str) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(repo().join(directory))
        .unwrap_or_else(|error| panic!("{directory}: {error}"))
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".test.js"))
        })
        .collect();
    files.sort();
    files
}

#[test]
fn node_tests_of_the_release_tools_and_the_frozen_copies_pass() {
    let mut files = test_files("tools/release/node/tests");
    files.extend(test_files("tools/compat-047/tests"));
    assert!(files.len() >= 2, "{files:?}");
    let output = Command::new("node")
        .arg("--test")
        .args(&files)
        .current_dir(repo())
        .output()
        .unwrap_or_else(|error| panic!("не запустился node (нужен в PATH): {error}"));
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
