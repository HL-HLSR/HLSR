//! `HLSR.exe --hlsr-uninstall-cleanup …` (`cleanupCli.runFromMain(app)` 0.4.7): очистка данных
//! после удаления для скриптов, старых деинсталляторов и тихого удаления; строки отчёта — в stdout,
//! код выхода — `CODES` `library-cleanup.js`. Журналы и кеш апдейтера — только Windows
//! (`%LOCALAPPDATA%`), как в 0.4.7.

use super::uninstall_telemetry::OnlineUninstallTelemetry;
use hlsr_core::paths::Paths;
use hlsr_update::uninstall::cleanup_cli::{self, RunOptions};

pub fn run(paths: &Paths) -> i32 {
    let env = super::mode_env();
    let lookup = env.lookup();
    let app_name = paths.app_name;
    let local = env.get("LOCALAPPDATA").filter(|value| !value.is_empty());
    let path = hlsr_update::windows::layout::win_path();
    let steam = super::steam_libraries();
    let install_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.to_string_lossy().into_owned()));
    let windows = cfg!(windows);
    let result = cleanup_cli::run(
        &RunOptions {
            argv: std::env::args().collect(),
            user_data: Some(paths.data_dir.to_string_lossy().into_owned()),
            app_name: Some(app_name.to_owned()),
            launcher_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            logs_dir: local
                .as_ref()
                .filter(|_| windows)
                .map(|local| path.join(&[local, app_name, "logs"])),
            updater_cache: local
                .as_ref()
                .filter(|_| windows)
                .map(|local| path.join(&[local, &format!("{app_name}-updater")])),
            install_dir,
            env: Some(&lookup),
            steam: Some(steam.as_ref()),
            telemetry: Some(&OnlineUninstallTelemetry),
            ..RunOptions::default()
        },
        &mut std::io::stdout(),
    );
    tracing::info!(
        "[cleanup] {} — код {}",
        result.lines.join(" | "),
        result.code
    );
    result.code
}
