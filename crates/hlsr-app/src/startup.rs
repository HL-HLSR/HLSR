//! Порядок запуска лаунчера: экземпляр, перенос, настройки, службы, окно GPUI; выход — по шагам
//! 0.4.7 (`quit`), затем остановка рантайма.

use crate::dialogs;
use crate::modes::dev_env;
use crate::online;
use crate::platform;
use crate::reduced_motion;
use crate::services;
use crate::tray;
use crate::update;
use crate::visibility::{Visibility, WindowEvents};
use crate::window;
use crate::window_ops;
use gpui::{QuitMode, WindowHandle};
use hlsr_core::args::LaunchArgs;
use hlsr_core::error::{CoreError, exit};
use hlsr_core::instance::{self, Acquired, PrimaryInstance};
use hlsr_core::language::ensure_language;
use hlsr_core::launcher_prefs::LauncherPrefs;
use hlsr_core::migration::local_storage::migrate_local_storage;
use hlsr_core::paths::Paths;
use hlsr_core::settings::Settings;
use hlsr_gpui::theme::Theme;
use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime};

const ELECTRON_WAIT: Duration = Duration::from_secs(10);
/// Первый запуск после перехода (свежая квитанция): 0.4.7 может выходить дольше — ждать, как
/// установщик Windows (решение 16 ПП6).
const ELECTRON_WAIT_AFTER_UPDATE: Duration = Duration::from_secs(60);
const ELECTRON_POLL: Duration = Duration::from_millis(250);
/// Сколько после стопов ждать блокирующие задачи tokio. Вопрос «Открыть ссылку?» и диалоги rfd
/// без окна-владельца, хеш `.part` не держат процесс, у которого уже нет окна: в 0.4.7 диалог был
/// модален окну и уходил вместе с приложением.
const RUNTIME_SHUTDOWN: Duration = Duration::from_secs(1);
/// Код выхода процесса Rust, когда `main` заканчивается паникой.
const PANIC_EXIT: i32 = 101;
/// Чем рисует GPUI (строки «Отрисовка интерфейса» диагностики): выбора рендерера больше нет.
const GPU_BACKEND: &str = if cfg!(windows) { "dx11" } else { "vulkan" };

#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("рантайм tokio не запущен: {0}")]
    Runtime(std::io::Error),
    #[error("окно не создано: {0}")]
    Window(String),
    #[error("отладочный отказ запуска (HLSR_DEBUG_FAIL_STARTUP)")]
    DebugFail,
}

/// Запустить лаунчер. Возвращает код выхода.
pub fn run(args: LaunchArgs, paths: &Paths, started: Instant) -> Result<i32, StartupError> {
    #[cfg(feature = "dev-hooks")]
    if let Some(code) = crate::snap::run(paths) {
        return Ok(code);
    }
    let instance = match instance::acquire(paths, args.forwarded.clone())? {
        Acquired::Forwarded => return Ok(exit::OK),
        Acquired::Primary(instance) => instance,
    };
    if instance::electron_running(&paths.data_dir) {
        let wait = if update::fresh_receipt(paths) {
            ELECTRON_WAIT_AFTER_UPDATE
        } else {
            ELECTRON_WAIT
        };
        tracing::info!(
            "[instance] запущен HLSR 0.4, ждём его выхода (до {} с)",
            wait.as_secs()
        );
        if !instance::wait_for_electron_exit(&paths.data_dir, wait, ELECTRON_POLL) {
            tracing::warn!("[instance] HLSR 0.4 не закрылся");
            dialogs::close_old_launcher();
            return Ok(exit::OTHER_LAUNCHER_RUNNING);
        }
    }

    migrate_local_storage(paths, SystemTime::now());
    #[cfg(target_os = "linux")]
    ensure_linux_integration(paths);
    let mut settings = Settings::open(paths);
    let language = ensure_language(&mut settings);
    tracing::info!("[startup] язык {}", language.code());
    let prefs = LauncherPrefs::open(paths);
    if dev_env("HLSR_DEBUG_FAIL_STARTUP").as_deref() == Some("1") {
        return Err(StartupError::DebugFail);
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("hlsr-worker")
        .enable_all()
        .build()
        .map_err(StartupError::Runtime)?;
    // Окно и службы живут и разрушаются внутри `launch`, в среде рантайма.
    let ended = {
        let _runtime = runtime.enter();
        launch(&runtime, paths, started, instance, settings, prefs)
    };
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN);
    match ended? {
        Ended::Exit(code) => Ok(code),
        Ended::Panicked(payload) => std::panic::resume_unwind(payload),
    }
}

/// Чем кончилось окно, когда службы уже остановлены.
enum Ended {
    Exit(i32),
    /// Паника цикла событий: продолжится после остановки рантайма.
    Panicked(Box<dyn Any + Send>),
}

/// Службы, окно и цикл событий; выход — [`quit`] при любом исходе. Вызывать в среде `runtime`.
fn launch(
    runtime: &tokio::runtime::Runtime,
    paths: &Paths,
    started: Instant,
    mut instance: PrimaryInstance,
    settings: Settings,
    prefs: LauncherPrefs,
) -> Result<Ended, StartupError> {
    window::set_app_user_model_id();
    let gpui_platform = hlsr_gpui::app::platform().map_err(StartupError::Window)?;
    tracing::info!(
        "[startup] reduced-motion ОС: {}",
        reduced_motion::os_prefers_reduced_motion()
    );
    let second_instances = instance.listen();
    let instance = Arc::new(Mutex::new(instance));
    // Видимость и фокус окна — одна сверка для онлайна, платформы и интерфейса.
    let visibility = Visibility::new();
    let profile_qa = online::ProfileQa::from_process();
    let online = online::start_online(paths, visibility.state(), &profile_qa);
    // Служба обновления — после онлайна (уведомления, события версий, телеметрия) и до доставки,
    // как в конструкторе `AppManager`.
    let configured = settings.renderer().as_str().to_owned();
    let update = update::start_update(
        paths,
        &online,
        update::RendererInfo {
            configured,
            started_with: GPU_BACKEND.to_owned(),
            active: GPU_BACKEND.to_owned(),
        },
    );
    // Игра из аргументов запуска выбирается, когда окно появится (`ready`).
    let platform = platform::start_platform(paths, &online, instance.clone(), visibility.state());
    update.bind_platform(platform.delivery(), platform.api());
    // `initialize()` (`AppManager.js:1047-1072`): служба обновления, доставка и Proton, затем
    // телеметрия, опрос онлайна и профиля — первый `runner/sync` и друзья берут аккаунт Steam у
    // платформы.
    update.api().initialize();
    platform.initialize();
    online.initialize();
    visibility.connect(Box::new(LauncherEvents {
        online: Arc::downgrade(online.api()),
        update: update.clone(),
        delivery: Arc::downgrade(platform.delivery()),
        platform: Arc::downgrade(platform.api()),
    }));
    listen_second_instances(second_instances, platform.delivery());
    // Каналы окна — фасады через `ChannelRouter`.
    let window_services =
        services::start(runtime.handle(), paths, &platform, &online, update.api());
    #[cfg(feature = "dev-hooks")]
    crate::live::start(runtime.handle(), &window_services);
    #[cfg(feature = "dev-hooks")]
    crate::inspect::start(paths);
    tracing::info!(
        "[startup] фасады подняты: доставка, платформа, онлайн, обновление ({} мс)",
        started.elapsed().as_millis()
    );

    let images = online::image_remote(paths, runtime.handle(), &profile_qa);
    let opened: std::rc::Rc<std::cell::Cell<Option<String>>> = Default::default();
    let failure = opened.clone();
    let handle = runtime.handle().clone();
    let watch = visibility.clone();
    let tray_paths = paths.clone();
    // «Показать HLSR» и щелчок по значку, пока окно спрятано из-за игры, — через трей платформы.
    let game_tray = platform.api().tray().cloned();
    let ui_scale = smoke_ui_scale().unwrap_or_else(|| settings.ui_scale());
    let (theme_id, accent) = (
        prefs.theme_id(),
        prefs
            .get("paperAccent")
            .and_then(|value| value.as_str())
            .map(str::to_owned),
    );
    let app = gpui::Application::with_platform(gpui_platform)
        .with_assets(hlsr_gpui::assets::Assets)
        .with_quit_mode(QuitMode::Explicit);
    let ran = std::panic::catch_unwind(AssertUnwindSafe(move || {
        app.run(move |cx| {
            hlsr_gpui::init(cx);
            hlsr_gpui::bridge::init(cx, handle);
            cx.global_mut::<Theme>()
                .set(&theme_id, accent.as_deref(), false, Instant::now());
            window_ops::set_events(cx, window_services.clone());
            cx.set_global(hlsr_gpui::shell::root::AppPaths(tray_paths.clone()));
            // Картинки из сети (аватары, новости) и арты игр: загрузчик ПП4 с кешем на диске.
            // Предел 48 МиБ (вместо 64): фон + передний план Ghosting/HL2 на 1440p/4K занимают
            // ~28–31 МиБ и не должны вытеснять друг друга при переключении сцен; ушедшие игры
            // сразу освобождаются через `Background::release_gone` по области `games/<id>`.
            let env = hlsr_gpui::env::GpuiEnv::new(cx);
            let loader = hlsr_ui::images::ImageLoader::new(images, &*env);
            loader.set_budget(48 * 1024 * 1024);
            cx.set_global(hlsr_gpui::image::Pictures::new(loader));
            if let Err(error) = window::open_launcher(cx, ui_scale, window_services) {
                failure.set(Some(error));
                cx.quit();
                return;
            }
            // Значок создаётся в этом потоке, когда окно прячут: цикл событий уже идёт.
            tray::install(cx, &tray_paths, game_tray);
            if let Some(launcher) = window::launcher(cx) {
                after_open(launcher, &watch, cx);
            }
        })
    }));
    hlsr_gpui::bridge::close();
    visibility.closed();
    let failed = opened.take();
    // Службы закрываются при любом исходе, в том числе при ошибке окна и панике: иначе сессии игр
    // на сервере остались бы без `session/end`, а Discord — без кадров закрытия.
    quit(
        &mut LauncherQuit {
            runtime,
            instance: &instance,
            online: &online,
            update: &update,
            platform: Some(platform),
        },
        quit_code(&ran, failed.is_some()),
    );
    match (ran, failed) {
        (Err(payload), _) => Ok(Ended::Panicked(payload)),
        (Ok(()), Some(error)) => Err(StartupError::Window(error)),
        // Живой прогон (V2): код выхода — итог сценария.
        #[cfg(feature = "dev-hooks")]
        (Ok(()), None) => Ok(Ended::Exit(crate::live::exit_code())),
        #[cfg(not(feature = "dev-hooks"))]
        (Ok(()), None) => Ok(Ended::Exit(exit::OK)),
    }
}

/// Окно открыто: видимость и фокус, выход из полного экрана силами системы, (dev-hooks)
/// снимок кадра и `HLSR_SMOKE_FULLSCREEN`.
fn after_open(
    launcher: WindowHandle<hlsr_gpui::app::LauncherRoot>,
    visibility: &Visibility,
    cx: &mut gpui::App,
) {
    visibility.watch(launcher, cx);
    window_ops::watch(launcher, cx);
    window_ops::restore_fullscreen(cx);
    #[cfg(feature = "dev-hooks")]
    {
        if dev_env("HLSR_SMOKE_FULLSCREEN").as_deref() == Some("1") {
            let answer = window_ops::window_command(cx, Some(&serde_json::json!("fullscreen")));
            tracing::info!("[smoke] window-command fullscreen: {answer}");
        }
        let _ = launcher.update(cx, |_, window, cx| hlsr_gpui::smoke::schedule(window, cx));
    }
}

/// `HLSR_SMOKE_UI_SCALE` (отладка и dev-hooks): масштаб интерфейса окна вместо `settings.json`.
fn smoke_ui_scale() -> Option<f64> {
    let value = dev_env("HLSR_SMOKE_UI_SCALE")?;
    value
        .parse()
        .inspect_err(|_| tracing::warn!("[smoke] HLSR_SMOKE_UI_SCALE={value} — не число"))
        .ok()
}

/// Шаги выхода 0.4.7: `before-quit`, `dispose()` (`AppManager.js:1079-1124`; на Linux — на
/// `will-quit`, после `before-quit` службы обновления, `index.js:194-198`), `quit`
/// (electron-updater).
trait QuitSteps {
    /// `before-quit` службы обновления: Linux снимает загрузку и откладывает выход, затем ставит
    /// обновление; Windows — ничего.
    fn before_quit(&mut self);
    /// `discordRpc.shutdown()` — первым.
    fn stop_discord(&mut self);
    /// Канал присутствия, мост, платформа и доставка (`gameDelivery.dispose()`).
    fn stop_platform(&mut self);
    /// `updateManager.dispose()`.
    fn dispose_update(&mut self);
    /// Телеметрия, онлайн и профиль (`session/end`); кадры закрытия Discord ждутся недолго.
    fn stop_online(&mut self);
    /// `quit` с кодом выхода (Windows): установка при выходе, только при коде 0.
    fn install_on_quit(&mut self, exit_code: i32);
    /// `app.relaunch()` (Linux): новая версия, когда эта отпустила экземпляр.
    fn relaunch(&mut self);
}

/// Выйти по шагам 0.4.7 с кодом выхода процесса `exit_code`.
fn quit(steps: &mut impl QuitSteps, exit_code: i32) {
    steps.before_quit();
    steps.stop_discord();
    steps.stop_platform();
    steps.dispose_update();
    steps.stop_online();
    steps.install_on_quit(exit_code);
    steps.relaunch();
}

/// Код выхода после цикла событий (`exitCode` события `quit`): окно не открылось — ошибка запуска
/// (`main`), паника — код паники.
fn quit_code(ran: &std::thread::Result<()>, window_failed: bool) -> i32 {
    match ran {
        Err(_) => PANIC_EXIT,
        Ok(()) if window_failed => exit::STARTUP_ERROR,
        Ok(()) => exit::OK,
    }
}

/// Шаги выхода на службах лаунчера.
struct LauncherQuit<'a> {
    runtime: &'a tokio::runtime::Runtime,
    instance: &'a Mutex<PrimaryInstance>,
    online: &'a online::Online,
    update: &'a update::Update,
    platform: Option<platform::Platform>,
}

impl QuitSteps for LauncherQuit<'_> {
    fn before_quit(&mut self) {
        self.update.before_quit(self.runtime);
    }

    fn stop_discord(&mut self) {
        self.online.stop_discord();
    }

    fn stop_platform(&mut self) {
        if let Some(platform) = self.platform.take() {
            platform.stop();
        }
    }

    fn dispose_update(&mut self) {
        self.update.dispose();
    }

    fn stop_online(&mut self) {
        self.online.stop(self.runtime);
    }

    fn install_on_quit(&mut self, exit_code: i32) {
        self.update.install_on_quit(exit_code);
    }

    fn relaunch(&mut self) {
        if !self.update.relaunch_pending() {
            return;
        }
        if let Ok(mut instance) = self.instance.lock() {
            instance.release();
        }
        self.update.spawn_relaunch();
    }
}

/// События окна для служб (`visibility.rs`) в порядке `AppManager.js`.
struct LauncherEvents {
    online: Weak<hlsr_online::api::OnlineApi>,
    update: update::Update,
    delivery: Weak<hlsr_delivery::api::DeliveryApi>,
    platform: Weak<hlsr_platform::api::PlatformApi>,
}

impl WindowEvents for LauncherEvents {
    /// `ready` (`AppManager.js:1018-1045`): игра из аргументов запуска, приглашения версий и
    /// квитанция установки, фоновая проверка обновлений (и уборка следов 0.4.7 по квитанции).
    fn ready(&self) {
        if let Some(delivery) = self.delivery.upgrade() {
            platform::window_ready(&delivery);
        }
        if let Some(online) = self.online.upgrade() {
            let receipt = online::window_loaded(&online);
            self.update.window_loaded(receipt);
        }
    }

    /// `show`/`restore` (`AppManager.js:1074-1075`): счётчики онлайна.
    fn shown(&self) {
        if let Some(online) = self.online.upgrade() {
            online.window_shown();
        }
    }

    /// `focus` (`AppManager.js:664-668`): `updateManager.checkOnFocus()`, доставка, затем
    /// `noteNewDay` профиля.
    fn focused(&self) {
        self.update.api().check_on_focus();
        if let (Some(delivery), Some(platform)) = (self.delivery.upgrade(), self.platform.upgrade())
        {
            platform::window_focused(&delivery, &platform);
        }
        if let Some(online) = self.online.upgrade() {
            online.window_focused();
        }
    }
}

/// `ensureLinuxIntegration()` (`index.js:150-165`): у управляемой установки — команда `hlsr` и пункт
/// меню на `start`; ошибки — только в журнал.
#[cfg(target_os = "linux")]
fn ensure_linux_integration(paths: &Paths) {
    use hlsr_update::linux::{LinuxDirs, SystemRunner};
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let home = std::env::home_dir().unwrap_or_default();
    let env: Vec<(String, String)> = std::env::vars().collect();
    let dirs = LinuxDirs::from_env(&home, &env, &paths.data_dir);
    let icon = hlsr_core::paths::Resources::detect().path("icons/hlsr.png");
    hlsr_update::linux::desktop_integration::ensure_linux_integration(
        &exe,
        &icon,
        &dirs,
        &SystemRunner,
        &hlsr_update::updater::processes::process_child_env(),
        &hlsr_update::ports::TracingLogger,
    );
}

/// Аргументы следующих запусков (`second-instance`): показать окно, игра из ярлыка —
/// `selectGameFromArgs`.
fn listen_second_instances(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Vec<String>>,
    delivery: &Arc<hlsr_delivery::api::DeliveryApi>,
) {
    let delivery = Arc::downgrade(delivery);
    tokio::spawn(async move {
        while let Some(argv) = rx.recv().await {
            if !hlsr_gpui::bridge::invoke(crate::window_ops::reveal) {
                break;
            }
            if let Some(delivery) = delivery.upgrade() {
                delivery.select_game_from_args(&argv);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Подделка служб: журнал шагов выхода.
    #[derive(Default)]
    struct Journal(Vec<String>);

    impl QuitSteps for Journal {
        fn before_quit(&mut self) {
            self.0.push("before-quit".into());
        }
        fn stop_discord(&mut self) {
            self.0.push("discord".into());
        }
        fn stop_platform(&mut self) {
            self.0.push("platform".into());
        }
        fn dispose_update(&mut self) {
            self.0.push("update.dispose".into());
        }
        fn stop_online(&mut self) {
            self.0.push("online".into());
        }
        fn install_on_quit(&mut self, exit_code: i32) {
            self.0.push(format!("install-on-quit {exit_code}"));
        }
        fn relaunch(&mut self) {
            self.0.push("relaunch".into());
        }
    }

    #[test]
    fn quit_follows_0_4_7() {
        // `before-quit` → `dispose()` (Discord первым, затем канал присутствия и доставка,
        // `UpdateManager`, телеметрия и онлайн) → `quit` (установка при выходе) → перезапуск.
        let mut steps = Journal::default();
        quit(&mut steps, exit::OK);
        assert_eq!(
            steps.0,
            [
                "before-quit",
                "discord",
                "platform",
                "update.dispose",
                "online",
                "install-on-quit 0",
                "relaunch",
            ]
        );
    }

    #[test]
    fn install_on_quit_gets_the_exit_code() {
        assert_eq!(quit_code(&Ok(()), false), exit::OK);
        // Окно не открылось или паника — код не 0: служба обновления при выходе ничего не ставит
        // (`exitCode !== 0` electron-updater).
        assert_eq!(quit_code(&Ok(()), true), exit::STARTUP_ERROR);
        assert_eq!(quit_code(&Err(Box::new("сбой")), false), PANIC_EXIT);
    }
}
