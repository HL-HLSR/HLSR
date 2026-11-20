//! Строки `locales/{en,ru}.json` → таблица Rust `OUT_DIR/strings.rs` для `tr()`.

use hlsr_ui_build::{TargetOs, locales};
use std::env;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let os = TargetOs::from_cargo(&target_os)
        .unwrap_or_else(|| panic!("ОС сборки «{target_os}» не поддерживается"));
    println!("cargo:rerun-if-changed=locales");

    let resolved = locales::resolve(
        &manifest.join("locales/en.json"),
        &manifest.join("locales/ru.json"),
        os,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    for key in &resolved.ru_from_en {
        println!("cargo:warning=ru.json: {key} — нет перевода, взята английская строка");
    }
    if let Err(error) = locales::write_rust_table(&out.join("strings.rs"), &resolved) {
        panic!("запись в OUT_DIR: {error}");
    }
}
