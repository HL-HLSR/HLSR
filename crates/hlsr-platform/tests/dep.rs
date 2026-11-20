//! Совместимость с DEP (`dep-compat.js`): чистые функции против фикстуры 0.4.7
//! (`tools/reference/platform/launch.js`) и `ensureExemption` с поддельным реестром — те же
//! случаи, что в самотесте `scripts/hl1-selftest.js` (строки 309–342). Режим системы на Windows
//! проверяется настоящим `GetSystemDEPPolicy` (значение не меняем, только читаем).

mod support;

use hlsr_delivery::ports::MemoryLogger;
use hlsr_platform::dep_compat::{
    self, DepRegistry, DepState, Exemption, LAYER, crash_hint, layers_of, merge_layers,
    parse_policy,
};
use std::sync::Mutex;
use support::{fixture, items};

fn opt_string(value: &serde_json::Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

#[test]
fn pure_functions_match_reference() {
    let data = fixture("launch.json");

    // Константы.
    let dep = &data["dep"];
    assert_eq!(
        dep["COMPAT_LAYERS"].as_str(),
        Some(dep_compat::COMPAT_LAYERS)
    );
    assert_eq!(dep["LAYER"].as_str(), Some(LAYER));
    assert_eq!(
        dep["ACCESS_VIOLATION"].as_u64(),
        Some(u64::from(dep_compat::ACCESS_VIOLATION))
    );
    let policy = &dep["POLICY"];
    for (key, name) in [
        (0, "AlwaysOff"),
        (1, "AlwaysOn"),
        (2, "OptIn"),
        (3, "OptOut"),
    ] {
        assert_eq!(policy[key.to_string()].as_str(), Some(name));
        assert_eq!(dep_compat::policy_name(key), Some(name));
    }
    assert_eq!(dep_compat::policy_name(4), None);

    // layersOf.
    for row in items(&data["layersOf"]) {
        let stdout = row["stdout"].as_str().unwrap_or("");
        let exe = row["exe"].as_str().unwrap_or("");
        assert_eq!(
            layers_of(stdout, exe),
            opt_string(&row["result"]),
            "layersOf stdout={stdout:?} exe={exe}"
        );
    }

    // mergeLayers.
    for row in items(&data["mergeLayers"]) {
        let current = row["current"].as_str();
        assert_eq!(
            merge_layers(current),
            opt_string(&row["result"]),
            "mergeLayers current={current:?}"
        );
    }

    // parsePolicy.
    for row in items(&data["parsePolicy"]) {
        let stdout = row["stdout"].as_str().unwrap_or("");
        assert_eq!(
            parse_policy(stdout),
            row["result"].as_i64(),
            "parsePolicy stdout={stdout:?}"
        );
    }

    // crashHint.
    for row in items(&data["crashHint"]) {
        let state = state_of(&row["state"]);
        let exit_code = row["exitCode"].as_i64();
        let expected = row["result"].as_str();
        assert_eq!(
            crash_hint(state.as_ref(), exit_code),
            expected,
            "crashHint state={:?} exitCode={exit_code:?}",
            row["state"]
        );
    }
}

/// `\s` JS: U+FEFF — пробел, U+0085 — нет (у `char::is_whitespace` Rust наоборот): разбор строки
/// `reg query` берёт класс из `hlsr_core::js`.
#[test]
fn layers_of_uses_the_js_whitespace_class() {
    let line = |gap: &str| format!("hl.exe{gap}REG_SZ{gap}~ DisableNXShowUI");
    assert_eq!(
        layers_of(&line("\u{FEFF}\u{FEFF}"), "hl.exe"),
        Some("~ DisableNXShowUI".to_owned())
    );
    assert_eq!(layers_of(&line("\u{85}\u{85}"), "hl.exe"), None);
    assert_eq!(
        layers_of(&line("  \u{A0}"), "hl.exe"),
        Some("~ DisableNXShowUI".to_owned())
    );
    // `parseInt`: U+FEFF отбрасывается как пробел, U+0085 — нет (тогда цифр нет, `NaN`).
    assert_eq!(dep_compat::parse_policy("\u{FEFF}3\r\n"), Some(3));
    assert_eq!(dep_compat::parse_policy("\u{85}3"), None);
}

fn state_of(value: &serde_json::Value) -> Option<DepState> {
    let object = value.as_object()?;
    let exemption = match object["exemption"].as_str() {
        Some("added") => Exemption::Added,
        Some("failed") => Exemption::Failed,
        _ => Exemption::Present,
    };
    Some(DepState {
        policy: object["policy"].as_i64(),
        exemption,
    })
}

/// Поддельный реестр, как `fakeReg` самотеста: держит одну строку данных слоя.
struct FakeReg {
    exe: String,
    data: Mutex<Option<String>>,
    deny: bool,
}

impl FakeReg {
    fn new(exe: &str, initial: Option<&str>, deny: bool) -> FakeReg {
        FakeReg {
            exe: exe.to_owned(),
            data: Mutex::new(initial.map(str::to_owned)),
            deny,
        }
    }

    fn data(&self) -> Option<String> {
        self.data.lock().unwrap().clone()
    }
}

impl DepRegistry for FakeReg {
    fn query_layers(&self, _exe: &str) -> Option<String> {
        self.data.lock().unwrap().as_ref().map(|data| {
            format!(
                "\r\n{}\r\n    {}    REG_SZ    {data}\r\n\r\n",
                dep_compat::COMPAT_LAYERS,
                self.exe
            )
        })
    }

    fn add_layer(&self, _exe: &str, data: &str) -> Result<(), String> {
        if self.deny {
            return Err("Отказано в доступе".to_owned());
        }
        *self.data.lock().unwrap() = Some(data.to_owned());
        Ok(())
    }
}

const EXE: &str = "D:\\HLSR\\Half-Life\\Games\\hl1-won\\Half-Life\\hl.exe";

#[test]
fn ensure_exemption_adds_layer_in_hkcu() {
    let logger = MemoryLogger::default();

    let fresh = FakeReg::new(EXE, None, false);
    assert_eq!(
        dep_compat::ensure_exemption(&fresh, EXE, &logger),
        Exemption::Added
    );
    assert_eq!(fresh.data().as_deref(), Some("~ DisableNXShowUI"));

    let admin = FakeReg::new(EXE, Some("~ RUNASADMIN WINXPSP3"), false);
    assert_eq!(
        dep_compat::ensure_exemption(&admin, EXE, &logger),
        Exemption::Added
    );
    assert_eq!(
        admin.data().as_deref(),
        Some("~ RUNASADMIN WINXPSP3 DisableNXShowUI"),
        "флаги из свойств exe сохраняются"
    );

    let present = FakeReg::new(EXE, Some("~ DisableNXShowUI"), false);
    assert_eq!(
        dep_compat::ensure_exemption(&present, EXE, &logger),
        Exemption::Present
    );

    let denied = FakeReg::new(EXE, None, true);
    assert_eq!(
        dep_compat::ensure_exemption(&denied, EXE, &logger),
        Exemption::Failed
    );
}

#[cfg(windows)]
#[test]
fn system_dep_policy_reads_a_valid_mode_or_none() {
    // Настоящий GetSystemDEPPolicy: только читаем, ничего не меняем.
    if let Some(value) = dep_compat::query_policy() {
        assert!(
            dep_compat::policy_name(value).is_some(),
            "режим DEP вне 0..3: {value}"
        );
    }
}
