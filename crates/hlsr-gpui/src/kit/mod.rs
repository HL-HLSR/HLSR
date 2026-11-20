//! Контролы вида: поле ввода (F6), кнопки, переключатель, флажок, выбор, ссылка, вкладки,
//! ползунок и стрелки числа (F8), редактор кода (F10); составные контролы (F11): выпадающий
//! список, поле цвета, разделитель панелей, GX-набор.

pub mod button;
pub mod check;
pub mod code_area;
pub mod color;
pub mod dropdown;
pub mod gx;
pub mod input;
pub mod link;
pub mod num_step;
pub mod pane_splitter;
pub mod radio;
pub mod slider;
pub mod tabs;
pub mod toggle;

pub use button::{Button, Handler, Look, Preview, Tone, button, outline};
pub use check::{Check, CheckLook, Checked, check};
pub use code_area::{CodeArea, CodeEvent};
pub use color::{ColorEvent, ColorField};
pub use dropdown::{Dropdown, Item, ToggleState, dropdown};
pub use input::{FieldEvent, FieldStyle, Kind, TextInput};
pub use link::{Link, Underline, link, text_link};
pub use num_step::{NumStep, num_step};
pub use pane_splitter::{Orientation, PaneSplitter, pane_splitter};
pub use radio::{Radio, radio};
pub use slider::{Slider, slider};
pub use tabs::{Tab, TabStyle, Tabs, tabs};
pub use toggle::{Toggle, toggle};

/// Клавиши контролов — до первого окна (из [`crate::init`]).
pub fn init(cx: &mut gpui::App) {
    input::bind_keys(cx);
    code_area::bind_keys(cx);
}
