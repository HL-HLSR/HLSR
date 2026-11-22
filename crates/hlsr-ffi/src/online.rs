//! Онлайн в приложении — онлайн-части `AppManager.js`: перенос входа 0.4.7 до показа окна (шаг
//! `onlineSession`), службы онлайна ([`OnlineApi`]) в порядке конструктора и `initialize()` (после
//! доставки и Steam, [`Online::initialize`]), окно загрузилось (`noteActiveDay`, `ready`:
//! [`window_loaded`]), опрос онлайна при показе окна, остановка (`dispose`: Discord первым).
//! Для онлайна окно — это видимость (`isVisible() && !isMinimized()`, общий опрос `visibility.rs`),
//! показ при входе и диалог ссылки новости; браузер — `shell.openExternal`
//! ([`crate::platform::open_external`]).
//!
//! Порты онлайна (`OnlineApi::ports`), Steam (`set_steam_providers`) и мост присутствия
//! (`OnlineApi::discord`) подключает связывание доставки и платформы (`platform.rs`).

use crate::visibility::WindowVisibility;
use hlsr_core::paths::Paths;
use hlsr_online::api::{BoxFuture, LinkDialog, OnlineApi, OnlineOptions, OnlineWindow};
use hlsr_online::notifications::version_events::Receipt;
#[cfg(any(debug_assertions, feature = "dev-hooks"))]
use hlsr_online::online_profile::local_qa::{self, QaEnv, QaError, QaSettings};
#[cfg(any(debug_assertions, feature = "dev-hooks"))]
use hlsr_online::telemetry::transport::OfflineTransport;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// Кеш картинок из сети (аватары, новости, спонсоры, гайд) в папке данных: `Cache/` Chromium
/// 0.4.7 при переносе удаляется.
const IMAGE_CACHE_DIR: &str = "image-cache";
/// Сколько ждать `session/end` и закрытия Discord при выходе.
const FINISH_TIMEOUT: Duration = Duration::from_secs(3);

/// Окно для онлайна: видимость — общий опрос окна (`visibility.rs`).
struct Window {
    visible: WindowVisibility,
}

impl OnlineWindow for Window {
    fn visible(&self) -> bool {
        self.visible.visible()
    }

    fn reveal(&self) {
        crate::host::command("reveal", serde_json::Value::Null);
    }

    fn confirm_link(&self, dialog: LinkDialog) -> BoxFuture<'static, bool> {
        Box::pin(async move {
            // Владелец — окно лаунчера (`showMessageBox(this.window, …)`): пока вопрос открыт,
            // лаунчер не принимает ввод.
            let owner = crate::host::dialog_owner().await;
            tokio::task::spawn_blocking(move || platform::confirm_link(&dialog, owner))
                .await
                .unwrap_or(false)
        })
    }
}

/// Онлайн запущенного лаунчера.
pub struct Online {
    api: Arc<OnlineApi>,
    profile_qa: bool,
}

/// QA-подмена API профиля (`local-qa.js`, только отладка и `dev-hooks`): окружение разбирается
/// один раз на запуск — её спрашивают онлайн, загрузчик картинок окна и Steam платформы.
pub struct ProfileQa {
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    settings: Result<Option<QaSettings>, QaError>,
}

impl ProfileQa {
    /// Из окружения процесса; выпуск без `dev-hooks` подмены не знает.
    pub fn from_process() -> ProfileQa {
        ProfileQa {
            #[cfg(any(debug_assertions, feature = "dev-hooks"))]
            settings: local_qa::settings(&QaEnv::from_process()),
        }
    }

    /// Подмена принята: адреса онлайна — на `127.0.0.1`, аккаунт и друзья — из её файлов.
    pub fn active(&self) -> bool {
        #[cfg(any(debug_assertions, feature = "dev-hooks"))]
        {
            matches!(self.settings, Ok(Some(_)))
        }
        #[cfg(not(any(debug_assertions, feature = "dev-hooks")))]
        {
            false
        }
    }
}

/// Создать онлайн (конструктор `AppManager`) и запустить Discord. Вызывать в среде tokio, до
/// показа окна; опросы — [`Online::initialize`].
pub fn start_online(paths: &Paths, visible: WindowVisibility, qa: &ProfileQa) -> Online {
    let window = Arc::new(Window { visible });
    #[allow(unused_mut)]
    let mut options = OnlineOptions::new(
        paths.data_dir.clone(),
        env!("CARGO_PKG_VERSION"),
        window,
        Arc::new(|url: String| {
            Box::pin(async move {
                tokio::task::spawn_blocking(move || crate::platform::open_external(&url))
                    .await
                    .map_err(|error| error.to_string())?
            })
        }),
    );
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    {
        profile_qa(&mut options, qa);
        dev_network(&mut options);
        // Стенд (QA-подмена или `HLSR_DEBUG_OFFICIAL=1`): вход, профили, спонсорство и
        // «Главная» без ключа аттестации.
        options.pretend_official =
            qa.active() || crate::modes::dev_env("HLSR_DEBUG_OFFICIAL").as_deref() == Some("1");
        if options.pretend_official {
            tracing::info!("[online] стенд: окно считает сборку официальной");
        }
    }
    hlsr_online::api::migrate_session(
        paths,
        &options.session,
        options.logger.as_ref(),
        SystemTime::now(),
    );
    let api = OnlineApi::new(options);
    // `discordRpc.start()` — в конструкторе `AppManager`, раньше `initialize()`.
    api.start_discord();
    Online {
        api,
        profile_qa: qa.active(),
    }
}

/// Сеть загрузчика картинок окна: загрузчик ПП4 (`https:` без Referer, кеш на диске
/// `<данные>/image-cache`, PNG/JPEG/WebP по заголовку). Отладочная сборка и `dev-hooks` без
/// `HLSR_ONLINE_REAL_API=1` берут картинки только с loopback (как трафик онлайна).
pub fn image_remote(
    paths: &Paths,
    runtime: &tokio::runtime::Handle,
    #[cfg_attr(
        not(any(debug_assertions, feature = "dev-hooks")),
        allow(unused_variables)
    )]
    qa: &ProfileQa,
) -> std::rc::Rc<dyn hlsr_ui::images::RemoteImages> {
    use hlsr_online::image_loader::{ImageLoader, ImageLoaderOptions};
    #[allow(unused_mut)]
    let mut options = ImageLoaderOptions::default();
    #[cfg(any(debug_assertions, feature = "dev-hooks"))]
    {
        let real_api = std::env::var("HLSR_ONLINE_REAL_API").ok();
        if dev_network_of(qa.active(), real_api.as_deref()) != DevNetwork::Real {
            options.hosts = Some(LOOPBACK_HOSTS.map(str::to_owned).to_vec());
        }
    }
    let loader = Arc::new(ImageLoader::new(
        paths.data_dir.join(IMAGE_CACHE_DIR),
        options,
    ));
    std::rc::Rc::new(hlsr_ui::images::OnlineImages::new(loader, runtime.clone()))
}

/// Хосты loopback (`URL.hostname`) для картинок отладочного запуска.
#[cfg(any(debug_assertions, feature = "dev-hooks"))]
const LOOPBACK_HOSTS: [&str; 4] = ["127.0.0.1", "localhost", "[::1]", "::1"];

/// QA-подмена API профиля (`local-qa.js`, только отладка и `dev-hooks`): все адреса онлайна — на
/// `http://127.0.0.1`, аккаунт и друзья Steam — из файлов подмены. Папку данных задаёт
/// `HLSR_DATA_DIR`.
#[cfg(any(debug_assertions, feature = "dev-hooks"))]
fn profile_qa(options: &mut OnlineOptions, qa: &ProfileQa) {
    use hlsr_online::online_profile::profile_state::LocalAccount;
    match &qa.settings {
        Ok(Some(qa)) => {
            tracing::info!("[online] QA-подмена API профиля: {}", qa.api_base);
            options.api_base = Some(qa.api_base.clone());
            let account = qa.clone();
            options.account_provider = Some(Arc::new(move || {
                let value = local_qa::account(&account);
                let text = |key: &str| value[key].as_str().map(str::to_owned);
                LocalAccount {
                    steam_id64: text("steamId64"),
                    persona_name: text("personaName"),
                    avatar_data_uri: text("avatarDataUri"),
                }
            }));
            let friends = qa.clone();
            options.friend_provider = Some(Arc::new(move || {
                local_qa::friends(&friends)
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            }));
        }
        Ok(None) => {}
        Err(error) => tracing::error!("[online] QA-подмена не принята: {error}"),
    }
}

/// Куда ходит онлайн в отладочной сборке и с `dev-hooks`.
#[cfg(any(debug_assertions, feature = "dev-hooks"))]
#[derive(Debug, PartialEq, Eq)]
enum DevNetwork {
    /// QA-подмена: адреса уже на `127.0.0.1`.
    Qa,
    /// `HLSR_ONLINE_REAL_API=1`: настоящий `api.hlsr.pro`.
    Real,
    /// Сети нет: смоук и запуск разработчика не трогают настоящий сервер.
    Offline,
}

#[cfg(any(debug_assertions, feature = "dev-hooks"))]
fn dev_network_of(qa: bool, real_api: Option<&str>) -> DevNetwork {
    if qa {
        DevNetwork::Qa
    } else if real_api == Some("1") {
        DevNetwork::Real
    } else {
        DevNetwork::Offline
    }
}

/// Отладочная сборка и `dev-hooks` не ходят в настоящий `api.hlsr.pro` (смоук CI показывает
/// окно, а окно опрашивает счётчики онлайна), пока не попросили явно: `HLSR_ONLINE_REAL_API=1`.
/// С QA-подменой (`HLSR_PROFILE_QA_ROOT`) трафик идёт на `127.0.0.1`. Выпуск без `dev-hooks` этого
/// кода не содержит.
#[cfg(any(debug_assertions, feature = "dev-hooks"))]
fn dev_network(options: &mut OnlineOptions) {
    let real_api = std::env::var("HLSR_ONLINE_REAL_API").ok();
    match dev_network_of(options.api_base.is_some(), real_api.as_deref()) {
        DevNetwork::Qa => {}
        DevNetwork::Real => {
            tracing::info!("[online] HLSR_ONLINE_REAL_API=1: настоящий api.hlsr.pro")
        }
        DevNetwork::Offline => {
            tracing::info!(
                "[online] отладочный запуск: сеть онлайна выключена (HLSR_ONLINE_REAL_API=1 — \
                 настоящий api.hlsr.pro, HLSR_PROFILE_QA_ROOT — подмена на 127.0.0.1)"
            );
            options.transport = Arc::new(OfflineTransport);
        }
    }
}

/// Онлайн-часть `ready` окна (`AppManager.js:1023-1043`; день активности — `did-finish-load`):
/// приглашения версий и квитанция установки. Отдаёт квитанцию до `consumeReceipt`
/// (`AppManager.js:1031-1032`) — по ней обновление убирает следы 0.4.7.
pub fn window_loaded(api: &OnlineApi) -> Option<Receipt> {
    let receipt = api.version_events().peek_receipt();
    api.window_loaded();
    receipt
}

impl Online {
    /// Службы онлайна (порты для доставки и платформы, Steam, Discord).
    pub fn api(&self) -> &Arc<OnlineApi> {
        &self.api
    }

    /// Принята QA-подмена API профиля: аккаунт и друзья Steam — из её файлов, не от платформы.
    pub fn profile_qa(&self) -> bool {
        self.profile_qa
    }

    /// Онлайн-часть `initialize()` (`AppManager.js:1055-1072`): согласие, телеметрия, опрос онлайна
    /// и профиля. Звать после доставки и платформы (`gameDelivery.initialize()`): первый
    /// `runner/sync` и друзья берут аккаунт Steam из провайдеров платформы.
    pub fn initialize(&self) {
        self.api.initialize();
    }

    /// Начало `dispose()` (`AppManager.js:1080-1084`): Discord закрывается первым.
    pub fn stop_discord(&self) {
        self.api.stop_discord();
    }

    /// Конец `dispose()`, после доставки и обновления: телеметрия, онлайн и профиль; `session/end`
    /// и кадры закрытия Discord ждутся недолго.
    pub fn stop(&self, runtime: &tokio::runtime::Runtime) {
        self.api.shutdown();
        let _ = runtime.block_on(tokio::time::timeout(FINISH_TIMEOUT, self.api.finish()));
    }
}

#[cfg(windows)]
mod platform {
    use hlsr_core::win32::wide;
    use hlsr_online::api::LinkDialog;
    use windows_sys::Win32::UI::Controls::{
        TASKDIALOG_BUTTON, TASKDIALOGCONFIG, TD_INFORMATION_ICON, TDF_ALLOW_DIALOG_CANCELLATION,
        TDF_POSITION_RELATIVE_TO_WINDOW, TDF_SIZE_TO_CONTENT, TaskDialogIndirect,
    };

    /// Номера кнопок `buttons: [Отмена, Открыть в браузере]` 0.4.9.
    const CANCEL: i32 = 100;
    const OPEN: i32 = 101;

    /// Вопрос «Открыть ссылку?» как `dialog.showMessageBox` Electron на Windows (TaskDialog,
    /// `noLink`): сообщение — крупной строкой, адрес — под ним, кнопки «Отмена» и «Открыть в
    /// браузере» в этом порядке, по умолчанию и на Esc — «Отмена». С владельцем окно модально:
    /// лаунчер ждёт ответа.
    pub fn confirm_link(dialog: &LinkDialog, owner: Option<isize>) -> bool {
        let title = wide(&dialog.title);
        let message = wide(&dialog.message);
        let detail = wide(&dialog.detail);
        let cancel = wide(&dialog.cancel);
        let open = wide(&dialog.open);
        let buttons = [
            TASKDIALOG_BUTTON {
                nButtonID: CANCEL,
                pszButtonText: cancel.as_ptr(),
            },
            TASKDIALOG_BUTTON {
                nButtonID: OPEN,
                pszButtonText: open.as_ptr(),
            },
        ];
        // SAFETY: TASKDIALOGCONFIG — структура C, нули в ней — «не задано».
        let mut config: TASKDIALOGCONFIG = unsafe { std::mem::zeroed() };
        config.cbSize = size_of::<TASKDIALOGCONFIG>() as u32;
        config.hwndParent = owner.map_or(std::ptr::null_mut(), |hwnd| hwnd as _);
        config.dwFlags =
            TDF_ALLOW_DIALOG_CANCELLATION | TDF_POSITION_RELATIVE_TO_WINDOW | TDF_SIZE_TO_CONTENT;
        config.pszWindowTitle = title.as_ptr();
        config.Anonymous1.pszMainIcon = TD_INFORMATION_ICON;
        config.pszMainInstruction = message.as_ptr();
        config.pszContent = detail.as_ptr();
        config.pButtons = buttons.as_ptr();
        config.cButtons = buttons.len() as u32;
        config.nDefaultButton = CANCEL;
        let mut pressed = CANCEL;
        // SAFETY: строки и кнопки живут до конца вызова; радиокнопок и флажка нет.
        let result = unsafe {
            TaskDialogIndirect(
                &config,
                &mut pressed,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        result == 0 && pressed == OPEN
    }
}

#[cfg(not(windows))]
mod platform {
    use hlsr_online::api::LinkDialog;

    /// Вопрос «Открыть ссылку?» с кнопками «Открыть в браузере» и «Отмена».
    /// shortcut: rfd без окна-владельца (`set_parent` просит `HasWindowHandle`); поправить, когда
    /// займёмся Linux.
    pub fn confirm_link(dialog: &LinkDialog, _owner: Option<isize>) -> bool {
        let answer = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Info)
            .set_title(&dialog.title)
            .set_description(format!("{}\n\n{}", dialog.message, dialog.detail))
            .set_buttons(rfd::MessageButtons::OkCancelCustom(
                dialog.open.clone(),
                dialog.cancel.clone(),
            ))
            .show();
        match answer {
            rfd::MessageDialogResult::Ok => true,
            rfd::MessageDialogResult::Custom(label) => label == dialog.open,
            _ => false,
        }
    }
}

