//! События процесса игры (`attachProcessEvents`, `handleCrash`, `crashInfo`, `openCrashDump`):
//! снимок дампов при запуске, статистика, онлайн, трей, таймер LiveSplit и Discord при старте,
//! падение по свежему дампу, уведомление `GAME_CRASHED` и события окна при выходе.

use super::{PlatformApi, channels, error_code};
use crate::crash_dumps::{self, DumpContext, WaitOptions};
use crate::game_process_service::{GameProcessService, ListenerId, ProcessEvent};
use hlsr_core::js;
use hlsr_delivery::manifests;
use hlsr_delivery::tool_service::LaunchOptions;
use hlsr_delivery::tools as tool_registry;
use serde_json::{Value, json};
use std::sync::Arc;

/// `SESSION_GOALS` (`profile/achievements.js` 0.4.9): «Марафон» и «24/7» — столько секунд за один
/// запуск игры.
pub const SESSION_GOALS: [(&str, f64); 2] = [
    ("marathon", 3.0 * 3600.0),
    ("around-the-clock", 24.0 * 3600.0),
];

impl PlatformApi {
    /// `attachProcessEvents()`: подписка на службу процессов новой библиотеки. `state` — статус
    /// игры в доставке (в 0.4.7 его слушает `GameDeliveryService`).
    pub(crate) fn attach_process_events(&self, processes: &GameProcessService) -> ListenerId {
        let me = self.me.clone();
        processes.subscribe(Arc::new(move |event: &ProcessEvent| {
            let Some(api) = me.upgrade() else {
                return;
            };
            let game_id = event
                .payload
                .get("gameId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            match event.name.as_str() {
                "state" => {
                    if !game_id.is_empty()
                        && let Some(delivery) = api.delivery()
                    {
                        delivery.process_state_changed(&game_id);
                    }
                }
                "starting" => api.on_starting(&game_id),
                "running" => api.on_running(&game_id, &event.payload),
                "finished" => api.on_finished(&game_id, &event.payload),
                _ => {}
            }
        }))
    }

    /// `process.on("starting")`: снимок дампов до запуска игры.
    fn on_starting(&self, game_id: &str) {
        let install_path = self.session().and_then(|session| {
            session
                .service
                .install_service()
                .read_installed(game_id)
                .ok()
                .flatten()
                .and_then(|installed| {
                    installed
                        .get("installPath")
                        .filter(|path| js::truthy(path))
                        .map(|path| js::to_template_string(Some(path)))
                })
        });
        let context = DumpContext {
            snapshot: crash_dumps::snapshot_dumps(
                install_path.as_deref(),
                &self.options.env,
                Some(game_id),
            ),
            game_id: Some(game_id.to_owned()),
            install_path,
            started_at_ms: Some(self.now_ms() as f64),
        };
        let mut state = self.state();
        match state
            .dump_snapshots
            .iter_mut()
            .find(|(id, _)| id == game_id)
        {
            Some(entry) => entry.1 = context,
            None => state.dump_snapshots.push((game_id.to_owned(), context)),
        }
    }

    /// `process.on("running")`.
    fn on_running(&self, game_id: &str, event: &Value) {
        // У подхваченного после перезапуска лаунчера процесса startedAt — из записи, сверенной со
        // временем старта процесса в ОС (matchesRecord).
        let started_at = event
            .get("startedAt")
            .and_then(Value::as_str)
            .and_then(js::date_parse);
        {
            let mut state = self.state();
            state.process_starts.retain(|(id, _)| id != game_id);
            if let Some(started_at) = started_at {
                state.process_starts.push((game_id.to_owned(), started_at));
            }
        }

        if let Some(stats) = self.stats_store()
            && let Err(error) = stats.session_start(game_id, None)
        {
            self.logger().warn(&format!(
                "[game-stats] сессия не открыта: {}",
                error.message
            ));
        }

        self.options.online.game_running(game_id);

        self.emit_to(channels::PROCESS_RUNNING, event.clone());

        if let Some(tray) = &self.tray {
            tray.on_game_started();
        }

        let me = self.me.clone();
        let id = game_id.to_owned();
        self.spawn(async move {
            if let Some(api) = me.upgrade() {
                api.start_timer_with_game(&id).await;
            }
        });

        self.emit_presence(json!({
            "type": "running",
            "gameId": game_id,
            "episode": event.get("episode").filter(|episode| js::truthy(episode)),
        }));
    }

    /// `process.on("finished")`.
    fn on_finished(&self, game_id: &str, event: &Value) {
        // Подписка `GameDeliveryService` (раньше этой): вышел эпизод — его config.cfg в Ghosting.
        if let Some(episode) = event.get("episode").and_then(Value::as_str)
            && let Some(delivery) = self.delivery()
        {
            delivery.return_episode_configs(game_id, episode);
        }

        if let Some(tray) = &self.tray {
            tray.on_game_finished();
        }

        if let Some(account) = self.steam_account() {
            account.refresh_connection();
            self.push_steam_account();
        }

        let mut last_exit = event.get("lastExit").cloned().unwrap_or(Value::Null);
        let outcome = |last_exit: &Value| {
            last_exit
                .get("outcome")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        };
        let requested = js::truthy(last_exit.get("requested").unwrap_or(&Value::Null));
        if last_exit.is_object() && outcome(&last_exit) != "crashed" && !requested {
            let context = self.dump_context(game_id);
            if crash_dumps::left_new_dump(context.as_ref(), &self.options.env) {
                self.logger().warn(&format!(
                    "[game-process] {game_id} вышла с кодом {}, но оставила дамп — считаем \
                     падением",
                    js::to_template_string(last_exit.get("exitCode"))
                ));
                last_exit["outcome"] = json!("crashed");
                if let Some(session) = self.session() {
                    session.processes.mark_last_exit_crashed(game_id);
                }
            }
        }

        let crashed = outcome(&last_exit) == "crashed" && !requested;
        let channel = if outcome(&last_exit) == "crashed" {
            channels::PROCESS_CRASHED
        } else {
            channels::PROCESS_EXITED
        };

        self.emit_presence(json!({"type": "stopped", "gameId": game_id}));

        let started_at = {
            let mut state = self.state();
            let at = state
                .process_starts
                .iter()
                .position(|(id, _)| id == game_id);
            at.map(|at| state.process_starts.remove(at).1)
        };
        if let Some(stats) = self.stats_store() {
            match stats.session_end(game_id) {
                Ok(session) => {
                    // Сессия статистики после перезапуска лаунчера начинается заново, поэтому
                    // берётся и время от старта процесса.
                    let since_start = started_at
                        .filter(|at| *at != 0)
                        .map_or(0.0, |at| (self.now_ms() - at) as f64 / 1000.0);
                    let played = session
                        .map_or(0.0, |session| session.seconds)
                        .max(since_start);
                    for (id, goal) in SESSION_GOALS {
                        if played >= goal {
                            self.achieve(id);
                        }
                    }
                }
                Err(error) => self.logger().warn(&format!(
                    "[game-stats] сессия не закрыта: {}",
                    error.message
                )),
            }
        }

        self.options
            .online
            .game_finished(game_id, Some(&last_exit), crashed);

        if crashed {
            let exit_code = last_exit
                .get("exitCode")
                .and_then(Value::as_i64)
                .or_else(|| {
                    last_exit
                        .get("exitCode")
                        .and_then(Value::as_f64)
                        .map(|code| code as i64)
                });
            let context = self.dump_context(game_id).unwrap_or_default();
            let me = self.me.clone();
            let id = game_id.to_owned();
            self.spawn(async move {
                if let Some(api) = me.upgrade() {
                    api.handle_crash(&id, exit_code, context).await;
                }
            });
        }

        let field = |key: &str| last_exit.get(key).cloned().unwrap_or(Value::Null);
        self.emit_to(
            channel,
            json!({
                "gameId": game_id,
                "outcome": field("outcome"),
                "exitCode": field("exitCode"),
                "signal": field("signal"),
                "requested": field("requested"),
                "at": field("at"),
            }),
        );

        if let (Some(delivery), Some(session)) = (self.delivery(), self.session()) {
            delivery.check_for_updates_if_due(&session.service, game_id, "after-exit");
        }
    }

    fn dump_context(&self, game_id: &str) -> Option<DumpContext> {
        self.state()
            .dump_snapshots
            .iter()
            .find(|(id, _)| id == game_id)
            .map(|(_, context)| context.clone())
    }

    /// `handleCrash(event)`: ждёт дамп, запоминает падение, пишет журнал и уведомление.
    pub(crate) async fn handle_crash(
        &self,
        game_id: &str,
        exit_code: Option<i64>,
        context: DumpContext,
    ) {
        let dumps = crash_dumps::wait_for_dump(
            context.install_path.as_deref(),
            Some(&context.snapshot),
            context.started_at_ms,
            &WaitOptions {
                settle_ms: self.options.timings.dump_settle_ms,
                poll_ms: self.options.timings.dump_poll_ms,
                env: Some(self.options.env.clone()),
                game_id: Some(game_id.to_owned()),
            },
        )
        .await;
        self.state().dump_snapshots.retain(|(id, _)| id != game_id);

        let dump = dumps.first();
        let log_path = dump.and_then(|dump| crash_dumps::crash_log_of(&dump.path));
        let archive_path = crash_dumps::crash_archive_of(
            context.install_path.as_deref(),
            context.started_at_ms,
            Some(game_id),
            &self.options.env,
        );
        let hint = self.crash_hint_of(game_id, exit_code);

        let info = json!({
            "at": js::iso_string(std::time::UNIX_EPOCH
                + std::time::Duration::from_millis(self.now_ms().max(0) as u64)),
            "exitCode": exit_code,
            "dumpPath": dump.map(|dump| dump.path.clone()),
            "dumpSize": dump.map(|dump| dump.size),
            "logPath": log_path,
            "archivePath": archive_path,
            "installPath": context.install_path,
            "hint": hint,
        });
        {
            let mut state = self.state();
            match state.last_crash.iter_mut().find(|(id, _)| id == game_id) {
                Some(entry) => entry.1 = info,
                None => state.last_crash.push((game_id.to_owned(), info)),
            }
        }

        if let Some(hint) = &hint {
            self.logger()
                .warn(&format!("[crash] {game_id}: вероятная причина — {hint}"));
        }
        match dump {
            Some(dump) => self.logger().warn(&format!(
                "[crash] дамп {game_id} найден: {} ({} байт)",
                dump.path, dump.size
            )),
            None => self
                .logger()
                .warn("[crash] crash dump не найден в разрешённых каталогах"),
        }
        if let Some(archive) = &archive_path {
            self.logger()
                .warn(&format!("[crash] архив сбоя {game_id}: {archive}"));
        }
        let has_dump = dump.is_some() || archive_path.is_some();

        if let Some(notifications) = &self.options.notifications {
            let detail = format!(
                "код выхода {}",
                exit_code.map_or_else(|| "?".to_owned(), |code| code.to_string())
            );
            let meta = json!({
                "hasDump": has_dump,
                "exitCode": exit_code,
                "gameId": game_id,
                "launcherId": manifests::launcher_id_of(game_id),
                "hint": hint,
            });
            if let Err(error) =
                notifications.add("error", "GAME_CRASHED", Some(&detail), Some(&meta))
            {
                self.logger()
                    .warn(&format!("[crash] уведомление не записано: {error}"));
            }
        }

        // Окно о падении (0.4.9): уведомление остаётся, окно — чтобы игрок прислал файл. Архив
        // сбоя (Ghosting) — один файл с логом и дампом: просим его, иначе лог, иначе дамп. Окно
        // выводит вперёд приложение по этому событию.
        let file = archive_path
            .clone()
            .or(log_path)
            .or_else(|| dump.map(|dump| dump.path.clone()));
        self.emit_to(
            channels::CRASH_REPORT,
            json!({
                "gameId": game_id,
                "launcherId": manifests::launcher_id_of(game_id),
                "exitCode": exit_code,
                "hasDump": has_dump,
                "archive": archive_path.is_some(),
                "file": file,
                "hint": hint,
            }),
        );
    }

    /// `crashHintOf(gameId, exitCode)`: подсказка DEP только для GoldSrc.
    fn crash_hint_of(&self, game_id: &str, exit_code: Option<i64>) -> Option<String> {
        let config = manifests::launch_config_of(game_id)?;
        if config.engine != Some("goldsrc") {
            return None;
        }
        self.session()?.launcher.dep_crash_hint(exit_code)
    }

    /// `crashInfo(id)`: падение игры; без игры (или неизвестная) — последнее падение.
    pub fn game_crash_info(&self, id: Option<&Value>) -> Value {
        let id = id.filter(|id| !id.is_null() && id.as_str() != Some(""));
        let Some(game_id) = id.and_then(|id| self.resolve_game_id(id)) else {
            return self.latest_crash();
        };
        self.state()
            .last_crash
            .iter()
            .find(|(id, _)| *id == game_id)
            .map_or(Value::Null, |(_, info)| info.clone())
    }

    /// `latestCrash()`: самое позднее по `at` (строкой).
    fn latest_crash(&self) -> Value {
        let state = self.state();
        let mut newest: Option<&Value> = None;
        for (_, info) in &state.last_crash {
            let at = js::to_template_string(info.get("at"));
            if newest.is_none_or(|newest| at > js::to_template_string(newest.get("at"))) {
                newest = Some(info);
            }
        }
        newest.cloned().unwrap_or(Value::Null)
    }

    /// `game-crash-open-dump`: «Показать в папке» только для файла из разрешённых папок — архив
    /// сбоя, иначе лог рядом с дампом, иначе сам дамп.
    pub fn game_crash_open_dump(&self, id: Option<&Value>) -> Value {
        let info = self.game_crash_info(id);
        let text = |key: &str| {
            info.get(key)
                .filter(|path| js::truthy(path))
                .map(|path| js::to_template_string(Some(path)))
        };
        let install_path = text("installPath");
        let allowed = |file: &str| {
            crash_dumps::is_allowed_dump_path(file, install_path.as_deref(), &self.options.env)
        };
        if let Some(archive) = text("archivePath").filter(|archive| allowed(archive)) {
            self.options
                .shell
                .show_item_in_folder(std::path::Path::new(&archive));
            return json!({"status": "ok"});
        }
        let Some(dump_path) = text("dumpPath") else {
            return error_code("NO_DUMP");
        };
        if !allowed(&dump_path) {
            return error_code("PATH_INVALID");
        }
        // Текстовый лог лежит рядом с дампом (`crashLogOf`) — показываем его.
        let path = hlsr_core::js::path::NodePath::native();
        let shown = text("logPath")
            .filter(|log| path.dirname(log) == path.dirname(&dump_path))
            .unwrap_or(dump_path);
        self.options
            .shell
            .show_item_in_folder(std::path::Path::new(&shown));
        json!({"status": "ok"})
    }

    /// `startTimerWithGame(gameId)`: LiveSplit вместе с игрой, если это включено в настройках.
    pub(crate) async fn start_timer_with_game(&self, game_id: &str) {
        let Some(delivery) = self.delivery() else {
            return;
        };
        let Some(tools) = delivery.tools() else {
            return;
        };
        let Some(store) = delivery.settings_store().cloned() else {
            return;
        };
        let settings = store.get(game_id).to_value();
        if settings.get("startTimer") != Some(&json!(true)) {
            return;
        }
        let status = tools.status(tool_registry::LIVESPLIT);
        if status.get("state").and_then(Value::as_str) != Some("ready") {
            self.logger()
                .info("[tools] автозапуск таймера пропущен: он не установлен");
            return;
        }
        let splits = delivery.live_split_choice(game_id);
        let launch = LaunchOptions {
            game_id: Some(game_id.to_owned()),
            splits: splits.clone(),
            restart: false,
            server_port: delivery.live_split_server_port(game_id),
        };
        match tools.launch(tool_registry::LIVESPLIT, &launch).await {
            Ok(_) => self.logger().info(&format!(
                "[tools] таймер запущен вместе с {game_id} (сплиты: {})",
                splits.as_deref().unwrap_or("по умолчанию")
            )),
            Err(failure) => self.logger().warn(&format!(
                "[tools] таймер не запустился вместе с игрой: {}",
                hlsr_delivery::errors::to_delivery_error(failure, None).message
            )),
        }
    }

    /// `ensureStatsStore().summary(gameId)`: сводка `game-stats.json` для отчёта диагностики
    /// (`AppManager.js:938-946`, ПП6); `None` — хранилища нет.
    pub fn game_stats_summary(&self, game_id: &str) -> Option<Value> {
        Some(self.stats_store()?.summary(game_id))
    }

    /// `game-stats-get`: при входе — снимок сервера, иначе — `game-stats.json`.
    pub fn game_stats_get(&self, id: &Value) -> Value {
        let stats = self.stats_store();
        let game_id = self.resolve_game_id(id);
        let (Some(stats), Some(game_id)) = (stats, game_id) else {
            return json!({"available": false});
        };
        if let Some(remote) = self.options.online.game_stats_from_server(&game_id) {
            return remote;
        }
        stats.summary(&game_id)
    }
}
