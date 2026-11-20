//! Запуск и остановка игры (`game-process-*`, `launch`), Steam для окна запуска
//! (`game-launch-*`), перезапуск от администратора и снимок окна LiveSplit
//! (`livesplit-overlay-capture`).

use super::{PlatformApi, error_code};
use crate::elevation::Elevated;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hlsr_core::js;
use hlsr_delivery::episodes;
use hlsr_delivery::errors::{Code, DeliveryError, Failure, RawError};
use hlsr_delivery::tools as tool_registry;
use serde_json::{Value, json};
use std::sync::Arc;

/// Ошибка как объект для `errorMap.toTelemetryCode(error)`.
fn error_value(error: &DeliveryError) -> Value {
    json!({"code": error.code.as_str(), "message": error.message})
}

impl PlatformApi {
    /// `requireGameId(id)`.
    fn require_game_id(&self, id: &Value) -> Result<String, Failure> {
        self.resolve_game_id(id).ok_or_else(|| {
            RawError::new(format!(
                "Неизвестная игра: {}",
                js::to_template_string(Some(id))
            ))
            .into()
        })
    }

    /// `game-process-launch(id, options)`: второй аргумент — `{episode: "ep1" | "ep2"}` для эпизода
    /// Ghosting; чужое значение — отказ.
    pub async fn game_process_launch(&self, id: &Value, options: &Value) -> Value {
        let Some(delivery) = self.delivery() else {
            return json!({"status": "error", "error": null});
        };
        delivery
            .run(|_| async move {
                let episode = options
                    .as_object()
                    .and_then(|options| options.get("episode"))
                    .filter(|episode| !episode.is_null());
                let episode = match episode {
                    None => None,
                    Some(Value::String(id)) if episodes::is_episode(id) => Some(id.as_str()),
                    Some(other) => {
                        return Err(DeliveryError::new(
                            Code::LaunchConfigInvalid,
                            "Неизвестный эпизод",
                        )
                        .with_details(json!({"episode": js::to_string(other)}))
                        .into());
                    }
                };
                let game_id = self.require_game_id(id)?;
                Ok(self.launch(&game_id, episode).await?)
            })
            .await
    }

    /// `launch(gameId, {episode})`: тестовый процесс в QA-режиме или настоящий запуск игры.
    pub async fn launch(
        &self,
        game_id: &str,
        episode: Option<&str>,
    ) -> Result<Value, DeliveryError> {
        let session = self
            .session()
            .ok_or_else(|| DeliveryError::new(Code::LaunchFailed, "Служба доставки не создана"))?;
        if session.service.is_game_running(game_id) {
            return Ok(json!({"status": "already-running"}));
        }
        // Инструмент «Демо» рендерит эту игру в фоне.
        if self.render_active(game_id) {
            return Err(DeliveryError::new(
                Code::RenderActive,
                "Идёт рендер демо этой игры",
            ));
        }
        // Mute, оставшийся после рендера «Демо», снимается до запуска; не вышло — игра всё равно
        // запускается.
        if let Some(before_launch) = self.before_launch_hook()
            && let Err(message) = before_launch(game_id).await
        {
            self.logger().warn(&format!(
                "[game-delivery] подготовка к запуску {game_id}: {message}"
            ));
        }

        // QA-процесс вместо игры эпизодов не знает: иначе вместо эпизода «запустилась» бы Ghosting.
        #[cfg(any(debug_assertions, feature = "dev-hooks"))]
        if episode.is_some() && hlsr_delivery::qa_launch::is_enabled(&self.options.env) {
            return Err(DeliveryError::new(
                Code::LaunchConfigInvalid,
                "Эпизоды Ghosting в QA-запуске не запускаются",
            )
            .with_details(json!({"gameId": game_id})));
        }

        let installed = session
            .service
            .install_service()
            .read_installed(game_id)
            .ok()
            .flatten();
        self.options
            .online
            .game_starting(game_id, installed.as_ref());

        #[cfg(any(debug_assertions, feature = "dev-hooks"))]
        if hlsr_delivery::qa_launch::is_enabled(&self.options.env) {
            return self.qa_launch(&session, game_id).await;
        }

        if let Some(delivery) = self.delivery() {
            delivery.ensure_practice_saves(game_id);
        }

        match session.launcher.launch(game_id, episode).await {
            Ok(result) => Ok(result),
            Err(error) => {
                self.options
                    .online
                    .game_launch_failed(game_id, &error_value(&error));
                Err(error)
            }
        }
    }

    /// QA-режим: вместо игры — тестовый процесс из папки игры.
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    async fn qa_launch(
        &self,
        session: &super::Session,
        game_id: &str,
    ) -> Result<Value, DeliveryError> {
        use crate::game_process_service::StartedProcess;
        use crate::launch_service::LaunchDelivery;
        let installed = session.service.installed_of(game_id)?;
        session.processes.mark_starting(game_id);
        let started = async {
            let delay = crate::qa_launch::startup_delay(&self.options.env);
            if delay > 0.0 {
                tokio::time::sleep(std::time::Duration::from_millis(delay as u64)).await;
            }
            let install_path = js::to_template_string(installed.get("installPath"));
            let launched = crate::qa_launch::launch(&install_path, &self.options.env)?;
            self.logger().warn(&format!(
                "[game-delivery] QA-РЕЖИМ: вместо игры запущен тестовый процесс {} (PID {})",
                launched.executable_path, launched.pid
            ));
            let link = super::DeliveryLink {
                service: session.service.clone(),
                processes: session.processes.clone(),
            };
            link.register_started_process(
                game_id,
                StartedProcess {
                    pid: Some(i64::from(launched.pid)),
                    executable_path: Some(launched.executable_path),
                    started_at: Some(launched.started_at),
                    child: Some(launched.child),
                    ..StartedProcess::default()
                },
            )
        }
        .await;
        match started {
            Ok(state) => Ok(json!({"status": "started", "process": state})),
            Err(error) => {
                session
                    .processes
                    .cancel_starting(game_id, Some(&error.message));
                self.options
                    .online
                    .game_launch_failed(game_id, &error_value(&error));
                Err(error)
            }
        }
    }

    /// `game-process-stop(id, {force})`.
    pub async fn game_process_stop(&self, id: &Value, options: &Value) -> Value {
        let Some(delivery) = self.delivery() else {
            return json!({"status": "error", "error": null});
        };
        let force = js::truthy(options.get("force").unwrap_or(&Value::Null));
        delivery
            .run(|service| async move {
                let game_id = self.require_game_id(id)?;
                service.stop_game(&game_id, force).await
            })
            .await
    }

    /// `game-launch-steam-status(id)`: `{ok, code}` для окна запуска.
    pub fn game_launch_steam_status(&self, id: &Value) -> Value {
        let Some(session) = self.ensure_session() else {
            return json!({"ok": false, "code": "LAUNCH_CONFIG_INVALID"});
        };
        let game_id = self
            .resolve_game_id(id)
            .unwrap_or_else(|| "ghosting-renewed".to_owned());
        let status = session.launcher.describe_availability(&game_id);
        json!({
            "ok": status.get("ok").cloned().unwrap_or(Value::Null),
            "code": status.get("code").filter(|code| js::truthy(code)).cloned().unwrap_or(Value::Null),
        })
    }

    /// `game-launch-open-steam`.
    pub fn game_launch_open_steam(&self) -> Value {
        // Как 0.4.7: openExternal без ожидания — отказ оболочки ответ не меняет.
        let _ = self.options.shell.open_external("steam://open/main");
        json!({"status": "ok"})
    }

    /// `game-launch-install-sdk`: Source SDK Base 2006.
    pub fn game_launch_install_sdk(&self) -> Value {
        // Как 0.4.7: openExternal без ожидания — отказ оболочки ответ не меняет.
        let _ = self.options.shell.open_external("steam://install/243730");
        json!({"status": "ok"})
    }

    /// `game-launch-restart-elevated`: замок экземпляра отпускается, лаунчер запускается от
    /// администратора и выходит через 600 мс; отказ или сбой — замок берётся обратно. Нет ответа
    /// UAC за 3 минуты — `pending` (итог применяется, когда он придёт).
    pub async fn game_launch_restart_elevated(&self) -> Value {
        if self.options.platform != hlsr_core::js::Platform::Win32 {
            return json!({"status": "unsupported"});
        }
        let Some(elevation) = self.options.elevation.clone() else {
            return json!({"status": "unsupported"});
        };
        if let Err(message) = elevation.lock.release() {
            self.logger().warn(&format!(
                "[launch] замок единственного экземпляра не отпущен: {message}"
            ));
        }

        let logger = self.options.logger.clone();
        let quit_delay = self.options.timings.elevation_quit_delay;
        let runtime = self.runtime();
        let elevation_for_outcome = elevation.clone();
        let outcome = move |result: Result<Elevated, String>| -> Value {
            match result {
                Err(message) => {
                    logger.warn(&format!(
                        "[launch] перезапуск от администратора не удался: {message}"
                    ));
                    elevation_for_outcome.lock.reacquire();
                    json!({"status": "error"})
                }
                Ok(Elevated::Declined(code)) => {
                    logger.info(&format!("[launch] повышение прав отклонено, код {code}"));
                    elevation_for_outcome.lock.reacquire();
                    json!({"status": "declined"})
                }
                Ok(Elevated::Started) => {
                    let quit = elevation_for_outcome.quit.clone();
                    match &runtime {
                        Some(runtime) => {
                            runtime.spawn(async move {
                                tokio::time::sleep(quit_delay).await;
                                quit();
                            });
                        }
                        None => quit(),
                    }
                    json!({"status": "ok"})
                }
            }
        };

        let mut pending = elevation.elevator.run_as_admin();
        let wait = self.options.timings.elevation_wait;
        match tokio::time::timeout(wait, &mut pending).await {
            Ok(result) => outcome(result),
            Err(_) => {
                // Окно получает `pending`, а итог UAC применяется, когда придёт.
                self.spawn(async move {
                    let result = pending.await;
                    outcome(result);
                });
                json!({"status": "pending"})
            }
        }
    }

    /// `captureLiveSplit(id)`: снимок окна LiveSplit для предпросмотра оверлея; одновременные
    /// запросы получают один снимок.
    pub async fn capture_live_split(&self, id: &Value) -> Value {
        let Some(game_id) = self.resolve_game_id(id) else {
            return error_code("UNKNOWN_GAME");
        };
        let mut receiver = {
            let mut state = self.state();
            match &state.capture {
                Some(receiver) => receiver.clone(),
                None => {
                    let (sender, receiver) = tokio::sync::watch::channel(None);
                    state.capture = Some(receiver.clone());
                    let me = self.me.clone();
                    self.spawn(async move {
                        let Some(api) = me.upgrade() else {
                            return;
                        };
                        let result = api.run_live_split_capture(&game_id).await;
                        api.state().capture = None;
                        let _ = sender.send(Some(result));
                    });
                    receiver
                }
            }
        };
        loop {
            if let Some(result) = receiver.borrow_and_update().clone() {
                return result;
            }
            if receiver.changed().await.is_err() {
                return json!({"status": "error", "error": {"code": "LIVESPLIT_CAPTURE_FAILED"}});
            }
        }
    }

    /// `runLiveSplitCapture(gameId)`. Windows: снимок находит любой `LiveSplit.exe`, как и оверлей
    /// в игре, — запущенный и не из лаунчера. Linux: только свой, его префикс известен.
    async fn run_live_split_capture(&self, game_id: &str) -> Value {
        let linux = self.is_linux();
        let launched = self
            .delivery()
            .and_then(|delivery| delivery.tools())
            .and_then(|tools| tools.running_info(tool_registry::LIVESPLIT));
        let launched_game = launched.as_ref().map(|running| running.game_id.clone());
        if launched_game.is_none() && linux {
            return json!({"status": "not-running"});
        }
        let launched_game = launched_game.flatten().filter(|id| !id.is_empty());
        let tool = self
            .options
            .overlay
            .as_ref()
            .and_then(|overlay| overlay.capture_tool().ok().flatten());
        let Some(tool) = tool else {
            return error_code("LIVESPLIT_OVERLAY_UNAVAILABLE");
        };
        let prefix_game_id =
            linux.then(|| launched_game.clone().unwrap_or_else(|| game_id.to_owned()));
        // Окно персонализации знает игру по id лаунчера, а префикс назван по gameId: оба
        // приведены к gameId здесь. На Windows LiveSplit тоже открыт для одной игры: оверлей
        // других игр выключен (`bindTo`).
        let bound = launched_game.or_else(|| prefix_game_id.clone());
        let other_game = bound.as_deref().is_some_and(|bound| bound != game_id);
        for file in [&tool.snapshot, &tool.status] {
            let _ = std::fs::remove_file(file);
        }
        let failed = |prefix: &Option<String>| {
            json!({"status": "error", "error": {"code": "LIVESPLIT_CAPTURE_FAILED"},
                   "prefixGameId": prefix, "otherGame": other_game})
        };

        let code = if linux {
            let Some(proton) = &self.options.proton else {
                return error_code("PROTON_NOT_INSTALLED");
            };
            self.linux_live_split_capture(proton, prefix_game_id.as_deref(), &tool)
                .await
        } else {
            self.windows_live_split_capture(&tool).await
        };
        let code = match code {
            Ok(code) => code,
            Err(message) => {
                self.logger().warn(&format!(
                    "[livesplit-overlay] снимок LiveSplit не сделан: {message}"
                ));
                return failed(&prefix_game_id);
            }
        };
        // 2 — окно LiveSplit не найдено (свёрнуто или ещё открывается). Не из лаунчера и окна
        // нет — значит, LiveSplit просто не запущен.
        if code == Some(2) {
            let status = if launched.is_some() {
                "no-window"
            } else {
                "not-running"
            };
            return json!({"status": status, "prefixGameId": prefix_game_id,
                          "otherGame": other_game});
        }
        if code != Some(0) {
            self.logger().warn(&format!(
                "[livesplit-overlay] снимок LiveSplit не сделан: {}",
                code.map_or_else(|| "нет ответа".to_owned(), |code| format!("код {code}"))
            ));
            return failed(&prefix_game_id);
        }
        let snapshot = self
            .options
            .overlay
            .as_ref()
            .and_then(|overlay| overlay.read_snapshot(&tool.snapshot));
        let Some(snapshot) = snapshot else {
            return failed(&prefix_game_id);
        };
        json!({
            "status": "ok",
            "width": snapshot.width,
            "height": snapshot.height,
            // Строкой base64: массив чисел JSON — по Value на байт (десятки МБ на снимок).
            "pixels": STANDARD.encode(&snapshot.pixels),
            "prefixGameId": prefix_game_id,
            "otherGame": other_game,
        })
    }

    /// Windows: снимок окна прямо в процессе лаунчера (`livesplit_window_capture`), в отдельном
    /// потоке с тайм-аутом: зависший LiveSplit не держит `PrintWindow` вечно.
    async fn windows_live_split_capture(
        &self,
        tool: &crate::livesplit_overlay::CaptureTool,
    ) -> Result<Option<u64>, String> {
        #[cfg(windows)]
        {
            use std::sync::atomic::{AtomicBool, Ordering};
            // Зависший `PrintWindow` тайм-аут не останавливает: пока прежний поток не вернулся,
            // новый не запускаем — иначе потоки с холстами копятся, а поздний снимок затирает свежий.
            static BUSY: AtomicBool = AtomicBool::new(false);
            struct Busy;
            impl Drop for Busy {
                fn drop(&mut self) {
                    BUSY.store(false, Ordering::Release);
                }
            }
            if BUSY.swap(true, Ordering::AcqRel) {
                return Err("прежний снимок LiveSplit ещё не вернулся".to_owned());
            }
            let busy = Busy;
            let file = tool.snapshot.clone();
            let task = tokio::task::spawn_blocking(move || {
                let _busy = busy;
                crate::livesplit_window_capture::capture(&file)
            });
            let outcome = tokio::time::timeout(self.options.timings.capture_timeout, task)
                .await
                .map_err(|_| "снимок LiveSplit не ответил".to_owned())?
                .map_err(|error| error.to_string())??;
            if let Some(error) = &outcome.error {
                self.logger()
                    .warn(&format!("[livesplit-overlay] снимок LiveSplit: {error}"));
            }
            Ok(Some(u64::from(outcome.code)))
        }
        #[cfg(not(windows))]
        {
            let _ = tool;
            Err("снимок окна LiveSplit без помощника есть только на Windows".to_owned())
        }
    }
    /// Linux: под Wine окно LiveSplit снимается только изнутри LiveSplit — 64-битный помощник
    /// внедряет `hlsr-share64.dll` и берёт кадр из общей памяти. Сессия `umu` не завершается,
    /// пока в префиксе работает LiveSplit: ждём файл статуса и закрываем сессию сами.
    async fn linux_live_split_capture(
        &self,
        proton: &super::ProtonPorts,
        prefix_game_id: Option<&str>,
        tool: &crate::livesplit_overlay::CaptureTool,
    ) -> Result<Option<u64>, String> {
        use crate::linux::proton_runner::windows_path_of;
        use crate::tool_runner::{PrefixLaunchOptions, PrefixLauncher};
        let (Some(injector64), Some(share_dll)) = (&tool.injector64, &tool.share_dll) else {
            return Err("помощник снимка LiveSplit не подготовлен".to_owned());
        };
        let text = |path: &std::path::Path| windows_path_of(&path.to_string_lossy());
        let args = vec![
            "capture".to_owned(),
            "--out".to_owned(),
            text(&tool.snapshot),
            "--status".to_owned(),
            text(&tool.status),
            "--dll".to_owned(),
            text(share_dll),
        ];
        let options = PrefixLaunchOptions {
            cwd: tool.dir.clone(),
            font_profile: None,
            env: Some(vec![(
                "PROTON_HIDE_PROCESS_WINDOW".to_owned(),
                "1".to_owned(),
            )]),
        };
        let launched =
            PrefixLauncher::launch(&proton.runner, prefix_game_id, injector64, &args, &options)
                .await
                .map_err(|failure| {
                    hlsr_delivery::errors::to_delivery_error(failure, None).message
                })?;
        let exited = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let pid = launched.child.as_ref().map(|child| child.pid);
        if let Some(child) = launched.child {
            let exited = exited.clone();
            self.spawn(async move {
                let _ = child.wait.await;
                exited.store(true, std::sync::atomic::Ordering::SeqCst);
            });
        }
        let cancelled = {
            let exited = exited.clone();
            move || exited.load(std::sync::atomic::Ordering::SeqCst)
        };
        let code = crate::livesplit_overlay::wait_for_status(
            &tool.status,
            self.options.timings.linux_capture_wait,
            &crate::livesplit_overlay::WaitOptions {
                interval: None,
                cancelled: Some(&cancelled),
            },
        )
        .await;
        if !exited.load(std::sync::atomic::Ordering::SeqCst)
            && let Some(pid) = pid
        {
            // `process.kill(-pid, "SIGTERM")`: сессия `umu` — своя группа процессов.
            use crate::process_stop::Stopper as _;
            let _ = crate::process_stop::SystemStopper.graceful_group(pid).await;
        }
        Ok(code)
    }
}
