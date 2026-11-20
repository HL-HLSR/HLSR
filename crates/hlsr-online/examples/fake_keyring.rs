//! Поддельные службы ключей на шине сеанса — для съёма фикстуры с настоящим Electron 0.4.7
//! (`tools/reference/online/safe-storage.js`, только внутри `scripts/slint/headless.sh`: там своя
//! шина D-Bus). Не для шины пользователя: на ней занятые имена не отдаются, и пример выходит.
//!
//!   fake_keyring <seed.json> <journal.jsonl>
//!
//! `seed.json` — состояние (`state_from_seed`) и `services`: какие имена занять (`secrets`,
//! `kwalletd6`, `kwalletd5`, `kwalletd`). Журнал вызовов переписывается после каждого вызова.
//! Готовность — строка `ready` в stdout.

#[cfg(target_os = "linux")]
#[path = "../tests/support/fake_keyring.rs"]
mod fake_keyring;

#[cfg(target_os = "linux")]
#[tokio::main(flavor = "current_thread")]
async fn main() {
    use std::sync::{Arc, Mutex};
    use zbus::fdo::RequestNameFlags;

    if std::env::var_os("HLSR_HEADLESS_ROOT").is_none() {
        eprintln!("fake_keyring: только внутри scripts/slint/headless.sh");
        std::process::exit(2);
    }
    let args: Vec<String> = std::env::args().collect();
    let seed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&args[1]).unwrap()).unwrap();
    let mut state = fake_keyring::state_from_seed(&seed);
    state.journal_file = Some(args[2].clone().into());
    let _ = std::fs::write(&args[2], "");
    let state = Arc::new(Mutex::new(state));
    let connection = zbus::Connection::session().await.expect("шина сеанса");
    fake_keyring::serve(&connection, state)
        .await
        .expect("службы");
    for service in seed["services"].as_array().into_iter().flatten() {
        let name = match service.as_str().unwrap_or("") {
            "secrets" => "org.freedesktop.secrets",
            "kwalletd6" => "org.kde.kwalletd6",
            "kwalletd5" => "org.kde.kwalletd5",
            "kwalletd" => "org.kde.kwalletd",
            other => panic!("неизвестная служба {other}"),
        };
        connection
            .request_name_with_flags(name, RequestNameFlags::DoNotQueue.into())
            .await
            .unwrap_or_else(|error| panic!("имя {name} не получено: {error}"));
    }
    println!("ready");
    std::future::pending::<()>().await;
}

#[cfg(not(target_os = "linux"))]
fn main() {}
