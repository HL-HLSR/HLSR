//! Запись ключа аттестации (`security/build_key.rs`): как DefinePlugin 0.4.7 — только в сборку
//! выпуска без `dev-hooks`. Скрипт выпуска задаёт `HLSR_ATTEST_SEAL`; в остальных сборках
//! переменная пуста, и ключа нет.

fn main() {
    println!("cargo:rerun-if-env-changed=HLSR_ATTEST_SEAL");
    let release = std::env::var("PROFILE").as_deref() == Ok("release");
    let dev_hooks = std::env::var_os("CARGO_FEATURE_DEV_HOOKS").is_some();
    let seal = if release && !dev_hooks {
        std::env::var("HLSR_ATTEST_SEAL").unwrap_or_default()
    } else {
        String::new()
    };
    println!("cargo:rustc-env=HLSR_EMBEDDED_ATTEST_SEAL={seal}");
}
