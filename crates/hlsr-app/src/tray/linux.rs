//! Linux: StatusNotifierItem (`ksni`) на рантайме tokio.

use super::{TrayBackend, TrayError, TrayEvent, TrayEvents, TrayLabels, icon_rgba};
use ksni::TrayMethods;
use ksni::menu::{MenuItem, StandardItem};
use std::time::Duration;

/// Регистрация у StatusNotifierWatcher — несколько вызовов D-Bus; дольше —
/// значка нет.
const REGISTER_TIMEOUT: Duration = Duration::from_secs(3);
/// Пауза перед снятием значка: ответ на щелчок успевает уйти.
const SHUTDOWN_DELAY: Duration = Duration::from_millis(200);

pub struct Ksni {
    events: TrayEvents,
    handle: Option<ksni::Handle<SniTray>>,
}

impl Ksni {
    pub fn new(events: TrayEvents) -> Ksni {
        Ksni {
            events,
            handle: None,
        }
    }
}

impl TrayBackend for Ksni {
    fn create(&mut self, labels: TrayLabels) -> Result<(), TrayError> {
        let (rgba, width, height) = icon_rgba()?;
        // ARGB32, порядок байтов сети.
        let (pixels, _) = rgba.as_chunks::<4>();
        let data = pixels
            .iter()
            .flat_map(|&[r, g, b, a]| [a, r, g, b])
            .collect();
        let tray = SniTray {
            labels,
            icon: ksni::Icon {
                width: width as i32,
                height: height as i32,
                data,
            },
            events: self.events.clone(),
        };
        // shortcut: регистрация ждёт StatusNotifierWatcher до 3 с в потоке окна — при зависшем
        // watcher окно стоит; трей платформы всё равно получает настоящий ответ (`ask_ui`).
        // Значок асинхронно, окно прятать после ответа — с живыми Linux-проверками (V4).
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|e| TrayError(format!("нет рантайма tokio: {e}")))?;
        let handle = runtime
            .block_on(async { tokio::time::timeout(REGISTER_TIMEOUT, tray.spawn()).await })
            .map_err(|_| TrayError("StatusNotifierWatcher не ответил за 3 с".to_owned()))?
            .map_err(|e| TrayError(e.to_string()))?;
        self.handle = Some(handle);
        Ok(())
    }

    fn destroy(&mut self) {
        let Some(handle) = self.handle.take() else {
            return;
        };
        // Значок снимают из обработчика его же щелчка или пункта меню: без
        // паузы соединение D-Bus закрывается раньше ответа на этот вызов, и
        // панель получает NoReply.
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(async move {
                    tokio::time::sleep(SHUTDOWN_DELAY).await;
                    handle.shutdown().await;
                });
            }
            Err(_) => drop(handle.shutdown()),
        }
    }
}

struct SniTray {
    labels: TrayLabels,
    icon: ksni::Icon,
    events: TrayEvents,
}

impl ksni::Tray for SniTray {
    fn id(&self) -> String {
        "hlsr".to_owned()
    }

    fn title(&self) -> String {
        self.labels.tooltip.clone()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![self.icon.clone()]
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: self.labels.tooltip.clone(),
            ..Default::default()
        }
    }

    // Щелчок по значку (у SNI двойного щелчка нет — он приходит как activate).
    fn activate(&mut self, _x: i32, _y: i32) {
        (self.events)(TrayEvent::Show);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: self.labels.show.clone(),
                activate: Box::new(|tray: &mut Self| (tray.events)(TrayEvent::Show)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: self.labels.quit.clone(),
                activate: Box::new(|tray: &mut Self| (tray.events)(TrayEvent::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }
}
