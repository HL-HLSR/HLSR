//! Логика заставки: `hlsr_ui::controllers::boot::BootMachine` против `Loading.vue` 0.4.7
//! (скрипт выполнен в Node на виртуальных часах, `tools/reference/ui/boot.js` →
//! `tests/fixtures/boot.json`): вызовы `boot-gate-run`, запись `wasLaunched`, моменты перехода
//! в лаунчер, фаза и надписи после каждого шага.

use hlsr_ui::controllers::boot::{
    BootDecision, BootMachine, Effect, FAILSAFE_MS, GateAction, MIN_SPLASH_MS, UpdateProgress,
    UpdaterStatus,
};
use serde_json::Value;

/// Переводчик с меткой, как `t` в скрипте Node: `T(ключ|аргументы)`.
fn label(key: &str, args: &[&str]) -> String {
    if args.is_empty() {
        format!("T({key})")
    } else {
        format!("T({key}|{})", args.join("|"))
    }
}

fn number(value: &Value) -> Option<f64> {
    value.as_f64()
}

fn decision(value: &Value) -> Option<BootDecision> {
    if value.is_null() {
        return None;
    }
    let action = match value["action"].as_str() {
        Some("proceed") => GateAction::Proceed,
        Some("installing") => GateAction::Installing,
        Some("blocked") => GateAction::Blocked,
        // Незнакомое действие 0.4.7 считает «идти дальше».
        _ => GateAction::Other,
    };
    Some(BootDecision {
        action,
        reason: value["reason"].as_str().map(str::to_owned),
        error: value["error"].as_str().map(str::to_owned),
    })
}

fn status(value: &Value) -> Option<UpdaterStatus> {
    if value.is_null() {
        return None;
    }
    let progress = &value["progress"];
    Some(UpdaterStatus {
        state: value["state"].as_str().unwrap_or_default().to_owned(),
        version: value["version"].as_str().map(str::to_owned),
        progress: (!progress.is_null()).then(|| UpdateProgress {
            percent: number(&progress["percent"]),
            transferred: number(&progress["transferred"]),
            total: number(&progress["total"]),
            bytes_per_second: number(&progress["bytesPerSecond"]),
        }),
    })
}

/// Виртуальные часы: таймеры в порядке срока и номера, как в `createClock` скрипта.
#[derive(Default)]
struct Sim {
    now: f64,
    seq: u64,
    timers: Vec<(f64, u64, Timer)>,
    calls: Vec<(f64, bool)>,
    writes: Vec<f64>,
    pushes: Vec<f64>,
}

#[derive(Clone, Copy, Debug)]
enum Timer {
    Failsafe,
    Navigate,
}

impl Sim {
    fn run(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::RunGate { retry } => self.calls.push((self.now, retry)),
                Effect::MarkLaunched => self.writes.push(self.now),
                Effect::ArmFailsafe { after_ms } => self.arm(after_ms, Timer::Failsafe),
                Effect::Navigate { after_ms } => self.arm(after_ms, Timer::Navigate),
            }
        }
    }

    fn arm(&mut self, after_ms: f64, timer: Timer) {
        self.seq += 1;
        self.timers.push((self.now + after_ms, self.seq, timer));
    }

    fn advance_to(&mut self, machine: &mut BootMachine, to: f64) {
        loop {
            let next = self
                .timers
                .iter()
                .filter(|(at, _, _)| *at <= to)
                .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.cmp(&b.1)))
                .copied();
            let Some((at, seq, timer)) = next else { break };
            self.timers.retain(|(_, id, _)| *id != seq);
            self.now = self.now.max(at);
            match timer {
                Timer::Failsafe => {
                    let effects = machine.failsafe(self.now);
                    self.run(effects);
                }
                Timer::Navigate => self.pushes.push(self.now),
            }
        }
        self.now = self.now.max(to);
    }
}

#[test]
fn constants_of_the_screen() {
    assert_eq!(MIN_SPLASH_MS, 1200.0);
    assert_eq!(FAILSAFE_MS, 20000.0);
}

#[test]
fn replays_every_scenario_of_loading_vue() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/boot.json")).expect("boot.json");
    let scenarios = fixture["scenarios"].as_array().unwrap();
    assert!(scenarios.len() >= 14);
    for scenario in scenarios {
        let name = scenario["name"].as_str().unwrap();
        let mut sim = Sim::default();
        let mut machine = BootMachine::new(scenario["wasLaunched"].as_bool().unwrap(), 0.0);
        let effects = machine.mounted(0.0);
        sim.run(effects);
        let timeline = scenario["timeline"].as_array().unwrap();
        let mut compared = 0;
        let mut check = |sim: &Sim, machine: &BootMachine, expected: &Value| {
            let texts = machine.texts(&label);
            let what = format!("{name} @ {} {}", expected["at"], expected["label"]);
            assert_eq!(machine.phase().as_str(), expected["phase"], "{what}: фаза");
            assert_eq!(
                machine.done(),
                expected["done"].as_bool().unwrap(),
                "{what}: done"
            );
            assert_eq!(
                texts.status,
                expected["statusText"].as_str().unwrap(),
                "{what}: надпись"
            );
            assert_eq!(
                texts.checking_hint,
                expected["substatus"].as_bool().unwrap(),
                "{what}: проверка"
            );
            assert_eq!(
                f64::from(texts.percent),
                expected["percent"].as_f64().unwrap(),
                "{what}: %"
            );
            assert_eq!(
                texts.bytes,
                expected["bytes"].as_str().unwrap(),
                "{what}: байты"
            );
            assert_eq!(
                texts.speed,
                expected["speed"].as_str().unwrap(),
                "{what}: скорость"
            );
            assert_eq!(
                texts.error.as_deref(),
                expected["bootError"].as_str(),
                "{what}: ошибка"
            );
            assert_eq!(
                machine.update_version(),
                expected["updateVersion"].as_str(),
                "{what}: версия"
            );
            assert_eq!(
                sim.pushes.len() as u64,
                expected["pushes"].as_u64().unwrap(),
                "{what}: переходы"
            );
            compared += 1;
        };
        check(&sim, &machine, &timeline[0]);
        assert_eq!(
            machine.splash_key(),
            Some(if scenario["wasLaunched"].as_bool().unwrap() {
                "#UI_WELCOME_BACK"
            } else {
                "#UI_WELCOME_HLSR"
            })
        );
        for (step, expected) in scenario["steps"]
            .as_array()
            .unwrap()
            .iter()
            .zip(&timeline[1..])
        {
            sim.advance_to(&mut machine, step["at"].as_f64().unwrap());
            match step["do"].as_str().unwrap() {
                "resolve" => {
                    let effects = machine.gate_response(sim.now, Ok(decision(&step["value"])));
                    sim.run(effects);
                }
                "reject" => {
                    let effects = machine.gate_response(sim.now, Err(()));
                    sim.run(effects);
                }
                "updater" => {
                    if let Some(status) = status(&step["status"]) {
                        machine.updater_state(&status);
                    }
                }
                "retry" => {
                    let effects = machine.retry();
                    sim.run(effects);
                }
                "observe" => {}
                other => panic!("{name}: шаг {other}"),
            }
            check(&sim, &machine, expected);
        }
        assert_eq!(compared, timeline.len(), "{name}: все записи сверены");

        let calls: Vec<(f64, bool)> = scenario["calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|call| {
                (
                    call["at"].as_f64().unwrap(),
                    call["options"]["retry"].as_bool().unwrap_or(false),
                )
            })
            .collect();
        assert_eq!(sim.calls, calls, "{name}: вызовы boot-gate-run");
        let writes: Vec<f64> = scenario["writes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["at"].as_f64().unwrap())
            .collect();
        assert_eq!(sim.writes, writes, "{name}: запись wasLaunched");
        let pushes: Vec<f64> = scenario["pushes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["at"].as_f64().unwrap())
            .collect();
        assert_eq!(sim.pushes, pushes, "{name}: моменты перехода");
    }
}

#[test]
fn texts_without_translation_use_the_keys() {
    // Переводчик, который возвращает ключ: надпись заставки до mounted() пуста (data().splash).
    let machine = BootMachine::new(false, 0.0);
    let texts = machine.texts(&|key, _| key.to_owned());
    assert_eq!(texts.status, "");
    assert_eq!(machine.splash_key(), None);
}
