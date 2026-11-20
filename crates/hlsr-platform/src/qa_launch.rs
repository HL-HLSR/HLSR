//! Тестовый запуск в QA-режиме (`qa-launch.js`): вместо игры — exe из переменной окружения,
//! относительный путь от папки игры. Только в отладочной сборке и с `dev-hooks`; признак
//! включения и описание — `hlsr_delivery::qa_launch`.

use crate::game_process_service::ChildHandle;
use hlsr_core::js;
use hlsr_core::js::path::NodePath;
use hlsr_delivery::environment::Environment;
use hlsr_delivery::errors::{Code, DeliveryError};
use hlsr_delivery::paths::assert_inside_with;
pub use hlsr_delivery::qa_launch::{ARGS_FLAG, DELAY_FLAG, EXE_FLAG, describe, is_enabled};
use hlsr_delivery::timing::{iso_from_ms, system_clock};

/// `parseArgs(raw)`: слова через пробел, в кавычках — одно слово (`/"[^"]*"|\S+/g`), крайние
/// кавычки снимаются.
pub fn parse_args(raw: &str) -> Vec<String> {
    let chars: Vec<char> = raw.chars().collect();
    let mut args = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        if js::is_whitespace(chars[at]) {
            at += 1;
            continue;
        }
        // Сначала «в кавычках, с закрывающей», иначе слово до пробела.
        let quoted_end = (chars[at] == '"')
            .then(|| chars[at + 1..].iter().position(|&c| c == '"'))
            .flatten()
            .map(|offset| at + 1 + offset + 1);
        let end = quoted_end.unwrap_or_else(|| {
            chars[at..]
                .iter()
                .position(|&c| js::is_whitespace(c))
                .map_or(chars.len(), |offset| at + offset)
        });
        let token = &chars[at..end];
        // `item.replace(/^"|"$/g, "")`: по одной кавычке с начала и с конца.
        let from = usize::from(token.first() == Some(&'"'));
        let to = if token.len() > from && token.last() == Some(&'"') {
            token.len() - 1
        } else {
            token.len()
        };
        args.push(token[from..to].iter().collect());
        at = end;
    }
    args
}

/// `startupDelay(env)`: задержка перед регистрацией процесса, мс; не больше 10 с.
pub fn startup_delay(env: &Environment) -> f64 {
    let value = env.var(DELAY_FLAG).map_or(0.0, |raw| js::to_number(&raw));
    if value.is_finite() && value > 0.0 {
        value.min(10_000.0)
    } else {
        0.0
    }
}

/// Запущенный тестовый процесс (`{child, pid, executablePath, startedAt}`).
pub struct Launched {
    pub child: ChildHandle,
    pub pid: u32,
    pub executable_path: String,
    pub started_at: String,
}

/// `launch(installPath, env)`: процесс отделён от лаунчера и закрывается только по команде.
/// Вызывается внутри среды выполнения tokio.
pub fn launch(install_path: &str, env: &Environment) -> Result<Launched, DeliveryError> {
    if !is_enabled(env) {
        return Err(DeliveryError::new(
            Code::ProviderUnsupported,
            "Тестовый запуск доступен только в QA-режиме",
        ));
    }

    let relative = env.var(EXE_FLAG).unwrap_or_default();
    let path = NodePath::native();

    if path.is_absolute(&relative) {
        return Err(DeliveryError::new(
            Code::PathInvalid,
            "Тестовый исполняемый файл задаётся путём относительно папки игры",
        ));
    }

    let executable = assert_inside_with(
        &path,
        install_path,
        &path.join(&[install_path, &relative.replace('/', path.sep())]),
        Some("Тестовый исполняемый файл"),
    )?;

    if !std::path::Path::new(&executable).exists() {
        return Err(DeliveryError::new(
            Code::RequiredFileMissing,
            format!("Тестовый процесс не найден: {executable}"),
        ));
    }

    let args = env
        .var(ARGS_FLAG)
        .map(|raw| parse_args(&raw))
        .unwrap_or_default();

    let mut command = tokio::process::Command::new(&executable);
    command
        .args(args)
        .current_dir(install_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    detach(&mut command);

    // Не запустился: в 0.4.7 у `ChildProcess` нет PID, и регистрация отвечает этой ошибкой.
    let spawned = command.spawn().ok();
    let child = spawned
        .and_then(ChildHandle::from_tokio)
        .ok_or_else(|| DeliveryError::new(Code::ProcessNotFound, "Некорректный PID: undefined"))?;

    Ok(Launched {
        pid: child.pid,
        child,
        executable_path: executable,
        started_at: iso_from_ms((system_clock())()),
    })
}

/// `detached: true, windowsHide: true`: отдельная группа процессов, без консоли.
fn detach(command: &mut tokio::process::Command) {
    #[cfg(windows)]
    {
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        command.process_group(0);
    }
}
