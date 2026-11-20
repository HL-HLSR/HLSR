//! Ожидание выхода лаунчера (`updater/core/process-wait.js`): пока есть процессы с именем образа
//! `HLSR.exe`, установщик не запускается; опрос раз в 500 мс, предел 60 с, затем
//! `UPD_HLSR_STILL_RUNNING`. 0.4.7 спрашивал `tasklist /FI "IMAGENAME eq HLSR.exe" /NH /FO CSV`,
//! здесь — снимок процессов Toolhelp32 (спецификация, решение 14); имена сравниваются так же
//! (`toLowerCase()` с обеих сторон). Разбор вывода `tasklist` оставлен для сверки этого сравнения.

use crate::errors::{UpdCode, UpdaterError};
use crate::ports::{BoxFuture, Clock};
use hlsr_core::js;
use serde_json::json;
use std::time::Duration;

/// `DEFAULT_TIMEOUT_MS`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(60_000);
/// `POLL_INTERVAL_MS`.
pub const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Образ лаунчера, которого ждёт апдейтер.
pub const LAUNCHER_IMAGE: &str = "HLSR.exe";

/// Один и тот же образ: `a.toLowerCase() === b.toLowerCase()`.
pub fn image_name_matches(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// `listPids` 0.4.7 над выводом `tasklist … /NH /FO CSV`: строки `"<образ>","<PID>",…`.
pub fn list_pids_from_tasklist_csv(stdout: &str, image_name: &str) -> Vec<u32> {
    let mut pids = Vec::new();
    for line in stdout.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        // /^"([^"]+)","(\d+)"/ над line.trim()
        let line = js::trim(line);
        let Some(rest) = line.strip_prefix('"') else {
            continue;
        };
        let Some(end) = rest.find('"') else { continue };
        let name = &rest[..end];
        let Some(tail) = rest[end..].strip_prefix("\",\"") else {
            continue;
        };
        let digits: &str = &tail[..tail.bytes().take_while(u8::is_ascii_digit).count()];
        if name.is_empty() || digits.is_empty() || !tail[digits.len()..].starts_with('"') {
            continue;
        }
        if !image_name_matches(name, image_name) {
            continue;
        }
        pids.push(js::to_number(digits) as u32);
    }
    pids
}

/// Процессы по имени образа.
pub trait ProcessList: Send + Sync {
    fn list_pids(&self, image_name: &str) -> Vec<u32>;
}

/// Снимок процессов ОС (Toolhelp32). Не Windows — пусто (апдейтер есть только у Windows).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemProcessList;

impl ProcessList for SystemProcessList {
    #[cfg(windows)]
    fn list_pids(&self, image_name: &str) -> Vec<u32> {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        };
        let mut pids = Vec::new();
        // SAFETY: снимок закрывается ниже; запись передаётся с выставленным dwSize.
        unsafe {
            let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
                return pids;
            };
            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
            while more {
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|unit| *unit == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
                if image_name_matches(&name, image_name) {
                    pids.push(entry.th32ProcessID);
                }
                more = Process32NextW(snapshot, &mut entry).is_ok();
            }
            let _ = CloseHandle(snapshot);
        }
        pids
    }

    #[cfg(not(windows))]
    fn list_pids(&self, _image_name: &str) -> Vec<u32> {
        Vec::new()
    }
}

/// Что сообщает ожидание (`onWait`): кто ещё работает и сколько осталось.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitInfo {
    pub pids: Vec<u32>,
    pub remaining_ms: i64,
}

impl WaitInfo {
    pub fn to_json(&self) -> serde_json::Value {
        json!({"pids": self.pids, "remainingMs": self.remaining_ms})
    }
}

/// Пауза между опросами (`sleep`).
pub type Sleep<'a> = &'a (dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync);

/// Пауза на часах tokio.
pub fn tokio_sleep(duration: Duration) -> BoxFuture<'static, ()> {
    Box::pin(tokio::time::sleep(duration))
}

/// Параметры [`wait_for_exit`].
pub struct WaitOptions<'a> {
    pub image_name: &'a str,
    pub timeout: Duration,
    pub poll: Duration,
    pub list: &'a dyn ProcessList,
    pub clock: &'a dyn Clock,
    pub sleep: Sleep<'a>,
    pub on_wait: &'a (dyn Fn(&WaitInfo) + Send + Sync),
}

/// `waitForExit(options)`: `Ok(())` — процессов нет (`{ok: true, waitedFor: []}`).
pub async fn wait_for_exit(options: WaitOptions<'_>) -> Result<(), UpdaterError> {
    let timeout_ms = options.timeout.as_millis() as i64;
    let deadline = options.clock.now_ms() + timeout_ms;
    loop {
        let pids = options.list.list_pids(options.image_name);
        if pids.is_empty() {
            return Ok(());
        }
        if options.clock.now_ms() >= deadline {
            let seconds = js::math_round(timeout_ms as f64 / 1000.0);
            let list = pids
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(UpdaterError::new(
                UpdCode::HlsrStillRunning,
                format!("HLSR не завершился за {seconds} с (PID: {list})"),
            )
            .with_details(json!({"pids": pids})));
        }
        let remaining = (deadline - options.clock.now_ms()).max(0);
        (options.on_wait)(&WaitInfo {
            pids,
            remaining_ms: remaining,
        });
        (options.sleep)(options.poll).await;
    }
}
