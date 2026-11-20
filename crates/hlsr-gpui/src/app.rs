//! Приложение на GPUI: платформа и корень окна лаунчера. Корень — шаг темы и `<router-view>`
//! App.vue: заставка ([`BootScreen`], S15), затем оболочка ([`ShellRoot`], S1: кадр окна,
//! шапка, разделы, окна, тосты; события сервисов окна она слушает сама). Смена экранов —
//! переход `slide`, под обоими — фон `#app`.

use crate::layer::Layers;
use crate::motion::swap;
use crate::shell::boot::{BootScreen, Leave};
use crate::shell::root::ShellRoot;
use gpui::{
    AppContext, Context, Empty, Entity, IntoElement, ParentElement, Render, Styled, Window, div,
    rgb,
};
use hlsr_ui::services::Services;
use std::rc::Rc;
use std::time::Duration;

/// Платформа GPUI. Windows — напрямую, без `gpui_platform`: его манифест спорит с
/// `hlsr.manifest` (CVT1100).
#[cfg(windows)]
pub fn platform() -> Result<Rc<dyn gpui::Platform>, String> {
    gpui_windows::WindowsPlatform::new(false)
        .map(|platform| Rc::new(platform) as Rc<dyn gpui::Platform>)
        .map_err(|error| format!("{error:#}"))
}

#[cfg(target_os = "linux")]
pub fn platform() -> Result<Rc<dyn gpui::Platform>, String> {
    Ok(gpui_linux::current_platform(false))
}

/// Корень окна лаунчера.
pub struct LauncherRoot {
    services: Services,
    boot: Option<Entity<BootScreen>>,
    /// Оболочка монтируется, когда заставка уходит (`router.push({name: "launcher"})`).
    shell: Option<Entity<ShellRoot>>,
    /// `preload.js`: путь библиотеки по умолчанию (`sendSync` до `ready`, как в 0.4.9).
    default_install_path: String,
}

/// `preload.js` 0.4.9 при загрузке страницы: `sendSync("app-default-library-path")`.
fn preload(services: &Services) -> String {
    services
        .send_sync("app-default-library-path", Vec::new())
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

impl LauncherRoot {
    /// Окно с маршрута `#/`: заставка, после неё оболочка.
    pub fn new(services: Services, cx: &mut Context<Self>) -> LauncherRoot {
        let default_install_path = preload(&services);
        // App.vue `mounted`: окно готово (до `mounted` заставки).
        services.send("ready", Vec::new());
        let boot = cx.new(|cx| BootScreen::new(services.clone(), cx));
        cx.subscribe(&boot, |root, _, _: &Leave, cx| root.launch(cx))
            .detach();
        LauncherRoot {
            services,
            boot: Some(boot),
            shell: None,
            default_install_path,
        }
    }

    /// Окно сразу с маршрута `#/launcher` (сцены стенда): оболочка без заставки.
    pub fn launcher(services: Services, cx: &mut Context<Self>) -> LauncherRoot {
        let default_install_path = preload(&services);
        services.send("ready", Vec::new());
        let path = default_install_path.clone();
        let shell = cx.new(|cx| ShellRoot::new(services.clone(), path, cx));
        LauncherRoot {
            services,
            boot: None,
            shell: Some(shell),
            default_install_path,
        }
    }

    /// Заставка, пока она на экране или уходит.
    pub fn boot(&self) -> Option<Entity<BootScreen>> {
        self.boot.clone()
    }

    fn launch(&mut self, cx: &mut Context<Self>) {
        if self.shell.is_some() {
            return;
        }
        let services = self.services.clone();
        let path = self.default_install_path.clone();
        self.shell = Some(cx.new(|cx| ShellRoot::new(services, path, cx)));
        // Уход `slide` — .16 с; потом картинки заставки — из атласа.
        let gone = cx.background_executor().timer(Duration::from_millis(500));
        cx.spawn(async move |root, cx| {
            gone.await;
            root.update(cx, |root, cx| {
                if let Some(boot) = root.boot.take() {
                    boot.update(cx, |boot, cx| boot.release(cx));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

impl Render for LauncherRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::theme::frame(window, cx);
        // Живой инспектор: корень рисуется в каждом кадре — таблица начинается здесь.
        #[cfg(feature = "dev-hooks")]
        crate::live::begin_frame();
        let (boot, shell) = (self.boot.clone(), self.shell.clone());
        // #app: фон окна под экранами — во время смены видны оба. Разрядка — `body` 0.4.9.
        div()
            .size_full()
            .rounded(Layers::radius(cx))
            .bg(rgb(0x0b0e13))
            .letter_spacing(crate::scale::u(crate::text::BODY_SPACING))
            .child(
                swap("app-router", shell.is_some(), "slide", move |launched| {
                    let screen = if *launched {
                        shell.clone().map(|shell| shell.into_any_element())
                    } else {
                        boot.clone().map(|boot| boot.into_any_element())
                    };
                    screen.unwrap_or_else(|| Empty.into_any_element())
                })
                .appear()
                .size_full(),
            )
    }
}
