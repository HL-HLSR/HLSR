//! hlsr-snap — сверочный стенд GPUI против Electron 0.4.9 (docs/gpui/00-plan.md, §2).
//!
//!   hlsr-snap screens …  — сцены tools/snap/screens/049 в окне `hlsr` (dev-hooks) против эталона
//!                          Electron 0.4.9 (screens.rs)
//!   hlsr-snap windows …  — окна установщика и апдейтера (tools/snap/windows.json) против эталона
//!                          snap-windows.js 0.4.9 (windows.rs)
//!
//! Только на невидимом столе: `python scripts/slint/headless-win.py target/debug/hlsr-snap.exe
//! screens --only games/frame` (Linux — scripts/slint/headless.sh).

mod masks;
mod screens;
mod windows;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("screens") => screens::main(&args[1..]),
        Some("windows") => windows::main(&args[1..]),
        _ => Err("команда: screens | windows (см. tools/snap/src/{screens,windows}.rs)".into()),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("hlsr-snap: {e}");
            ExitCode::from(2)
        }
    }
}
