//! `process.kill(pid, 0)` и `os.hostname()` против Node 24: фикстура `node-semantics-process.json`
//! из `tools/reference/node-semantics-process.js`. Ответы ядра (pid 1 — EPERM, pid больше
//! pid_max — ESRCH) сняты в Linux не от root; в Windows — проверка аргумента и ESRCH libuv.

use hlsr_core::js::process::{hostname, kill_zero};
use hlsr_core::js::to_number;
use serde_json::Value;

fn fixture() -> Value {
    let file = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/node-semantics-process.json"
    );
    serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
}

#[cfg(unix)]
fn is_root() -> bool {
    rustix::process::geteuid().is_root()
}

#[test]
fn kill_zero_matches_node() {
    for case in fixture()["kill"].as_array().unwrap() {
        let input = case[0].as_str().unwrap();
        let expected = case[1].as_str();
        let pid = if input == "self" {
            f64::from(std::process::id())
        } else {
            to_number(input)
        };
        let actual = kill_zero(pid).err();
        // Ответы ОС сняты в Linux не от root; проверка аргумента от ОС не зависит.
        let os_answer = expected != Some("ERR_INVALID_ARG_TYPE");
        #[cfg(unix)]
        if os_answer && is_root() {
            continue;
        }
        #[cfg(windows)]
        if os_answer && input != "self" {
            continue;
        }
        assert_eq!(actual.as_deref(), expected, "process.kill({input}, 0)");
    }
}

#[cfg(unix)]
#[test]
fn hostname_is_kernel_hostname_like_node() {
    assert_eq!(fixture()["hostnameIsKernelHostname"], Value::Bool(true));
    let kernel = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap();
    assert_eq!(hostname(), kernel.trim());
}

#[cfg(windows)]
#[test]
fn hostname_is_not_empty_on_windows() {
    assert!(!hostname().is_empty());
}

/// libuv в Windows: нет процесса (OpenProcess — ERROR_INVALID_PARAMETER) или он завершён — `ESRCH`.
#[cfg(windows)]
#[test]
fn kill_zero_reports_esrch_on_windows() {
    // Такого большого pid в Windows не бывает.
    assert_eq!(kill_zero(2_147_483_647.0).err().as_deref(), Some("ESRCH"));
    let mut child = std::process::Command::new("cmd")
        .args(["/C", "exit 0"])
        .spawn()
        .unwrap();
    let pid = f64::from(child.id());
    child.wait().unwrap();
    // Ручка `child` ещё открыта: объект процесса жив, код выхода уже не STILL_ACTIVE.
    assert_eq!(kill_zero(pid).err().as_deref(), Some("ESRCH"));
    drop(child);
}
