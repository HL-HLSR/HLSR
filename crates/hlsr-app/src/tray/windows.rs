//! Windows: `tray-icon` в главном потоке (цикл сообщений GPUI); события — через обработчики
//! `tray-icon` в поток окна (`bridge::invoke`).

use super::{TrayBackend, TrayError, TrayEvent, TrayEvents, TrayLabels, icon_rgba};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

const SHOW_ID: &str = "hlsr-show";
const QUIT_ID: &str = "hlsr-quit";

pub struct TrayIcon {
    icon: Option<tray_icon::TrayIcon>,
}

impl TrayIcon {
    pub fn new(events: TrayEvents) -> TrayIcon {
        let on_icon = events.clone();
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }
            | TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } => on_icon(TrayEvent::Show),
            _ => {}
        }));
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if event.id == SHOW_ID {
                events(TrayEvent::Show);
            } else if event.id == QUIT_ID {
                events(TrayEvent::Quit);
            }
        }));
        TrayIcon { icon: None }
    }
}

impl TrayBackend for TrayIcon {
    fn create(&mut self, labels: TrayLabels) -> Result<(), TrayError> {
        let (rgba, width, height) = icon_rgba()?;
        let icon = tray_icon::Icon::from_rgba(rgba, width, height)
            .map_err(|e| TrayError(e.to_string()))?;
        let menu = Menu::new();
        menu.append_items(&[
            &MenuItem::with_id(SHOW_ID, &labels.show, true, None),
            &PredefinedMenuItem::separator(),
            &MenuItem::with_id(QUIT_ID, &labels.quit, true, None),
        ])
        .map_err(|e| TrayError(e.to_string()))?;
        let tray = TrayIconBuilder::new()
            .with_tooltip(&labels.tooltip)
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .map_err(|e| TrayError(e.to_string()))?;
        self.icon = Some(tray);
        Ok(())
    }

    fn destroy(&mut self) {
        // Значок снимается, когда TrayIcon уничтожается.
        self.icon = None;
    }
}
