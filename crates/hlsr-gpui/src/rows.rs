//! Окно видимости длинного списка в [`crate::scroll::ScrollArea`] (лента, рекорды и игроки
//! «Главной»): строятся строки у видимой области и строки, чья высота ещё не измерена, прочие
//! заменяет распорка их измеренной высоты. Без него каждый кадр свайпа вкладок и прокрутки
//! строил, раскладывал и рисовал все карточки обеих страниц, включая скрытые за краем.
//!
//! Высоты меряются после раскладки ([`Rows::measure`]) и живут по ключу строки. Пока ширина
//! строк меняется от кадра к кадру (выдвигается колонка, меняется окно), строятся все строки:
//! ширину кадра до раскладки не узнать, а распорки прошлой ширины сдвинули бы видимое. Строка в
//! фокусе ([`Rows::focus`]) строится всегда, с соседями: фокус не теряется, когда её прокрутили за
//! край, и Tab идёт по списку дальше, как без окна.

use crate::scale::{zoom, zpx};
use crate::scroll::Scroll;
use gpui::{App, Bounds, Div, Pixels, Window};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Сколько строится за краем видимой области: доля её высоты сверху и снизу. Аватар строки
/// успевает загрузиться и перевестись, пока строка ещё не видна.
const MARGIN: f32 = 0.5;

/// Неизмеренных строк за окном видимости строится не больше стольких за кадр: остальные —
/// распорка средней высоты, их меряют следующие кадры (замер новых высот сам просит кадр). Раньше
/// первый кадр списка строил все строки сразу, и открытие раздела с тысячей записей стоило
/// кадра в десятки мс.
const BATCH: usize = 8;
/// То же, пока высота области неизвестна (первый кадр до раскладки): строк с запасом на экран.
const BATCH_FIRST: usize = 24;

/// Окно одного списка: вид держит его полем рядом со [`Scroll`] области (клон — то же окно).
#[derive(Clone, Default)]
pub struct Rows(Rc<RefCell<State>>);

#[derive(Default)]
struct State {
    /// Высота строки по ключу, px окна.
    heights: HashMap<String, Pixels>,
    /// Ширина строк и зум прошлого замера.
    measured_at: Option<(Pixels, f32)>,
    /// Ширина в прошлом замере сменилась: строить всё, пока она не устоится.
    resizing: bool,
    /// Начало колонки в содержимом области, px окна.
    origin: Pixels,
    /// Ключи строк этого кадра и что стоит в колонке: строка (индекс) или распорка.
    keys: Vec<String>,
    slots: Vec<Option<usize>>,
    /// Строка в фокусе: строители отмечают её в кадре, план следующего кадра её строит.
    focused: Option<String>,
}

/// Место в колонке.
#[derive(Debug, Clone, PartialEq)]
pub enum Slot {
    /// Строка с этим индексом: построить.
    Row(usize),
    /// Строки за краем: распорка этой высоты (px окна).
    Space(Pixels),
}

impl Rows {
    /// План колонки строк `keys` (стабильные id) с промежутком `gap` (CSS px) в области
    /// `scroll`: какие строки строить, а какие заменить распорками. Звать раз за кадр, до
    /// строителей строк.
    pub fn plan(
        &self,
        scroll: &Scroll,
        keys: Vec<String>,
        gap: f32,
        window: &Window,
        cx: &App,
    ) -> Vec<Slot> {
        let zoom = zoom(window);
        let top = scroll.top_at(cx.background_executor().now());
        let scrolling = scroll.moving();
        let viewport = scroll.handle().bounds().size.height;
        let mut state = self.0.borrow_mut();
        if state
            .measured_at
            .is_some_and(|(_, measured)| measured != zoom)
        {
            state.heights.clear();
        }
        let slots = state.slots_for(&keys, zpx(zoom, gap), top, viewport, scrolling);
        state.slots = slots
            .iter()
            .map(|slot| match slot {
                Slot::Row(index) => Some(*index),
                Slot::Space(_) => None,
            })
            .collect();
        state.keys = keys;
        slots
    }

    /// Строка `index` плана в фокусе (звать строителю, когда её фокус `is_focused`).
    pub fn focus(&self, index: usize) {
        let mut state = self.0.borrow_mut();
        state.focused = state.keys.get(index).cloned();
    }

    /// Колонка `column` с детьми по плану (сначала строки и распорки, дальше — что угодно):
    /// после раскладки меряет построенные строки.
    pub fn measure(&self, scroll: &Scroll, column: Div) -> Div {
        let (rows, handle) = (self.clone(), scroll.handle());
        column.on_children_prepainted(move |bounds, window, _| {
            let area = handle.bounds();
            // Содержимое области начинается выше её окна на scrollTop (сдвиг отрицательный).
            let content_top = area.top() + handle.offset().y;
            if rows.measured(&bounds, content_top, zoom(window)) {
                crate::motion::monitor_frame(window);
            }
        })
    }

    /// Замер кадра; `true` — план следующего кадра выйдет другим (новая строка, сменились
    /// высоты или ширина): вид перерисовать.
    fn measured(&self, bounds: &[Bounds<Pixels>], content_top: Pixels, zoom: f32) -> bool {
        let mut state = self.0.borrow_mut();
        let state = &mut *state;
        if state.slots.is_empty() {
            return false;
        }
        if let Some(first) = bounds.first() {
            state.origin = first.top() - content_top;
        }
        let mut changed = false;
        let mut width = None;
        for (slot, bounds) in state.slots.iter().zip(bounds) {
            let Some(key) = slot.and_then(|index| state.keys.get(index)) else {
                continue;
            };
            width = Some(bounds.size.width);
            let height = bounds.size.height;
            // Новая строка тоже: план ставил её по средней высоте, и строка у края могла
            // остаться распоркой.
            if state.heights.insert(key.clone(), height) != Some(height) {
                changed = true;
            }
        }
        if let Some(width) = width {
            let resized = state
                .measured_at
                .is_some_and(|(before, measured)| before != width || measured != zoom);
            changed |= resized || state.resizing;
            state.resizing = resized;
            state.measured_at = Some((width, zoom));
        }
        changed
    }
}

impl State {
    fn slots_for(
        &mut self,
        keys: &[String],
        gap: Pixels,
        top: Pixels,
        viewport: Pixels,
        scrolling: bool,
    ) -> Vec<Slot> {
        let known: Vec<Pixels> = keys
            .iter()
            .filter_map(|key| self.heights.get(key).copied())
            .collect();
        // Место неизмеренной строки до замера — средняя высота измеренных.
        let guess = if known.is_empty() {
            // До первого замера — десятая часть области: в окно попадает с десяток строк.
            viewport * 0.1
        } else {
            known.iter().sum::<Pixels>() * (1. / known.len() as f32)
        };
        let height = |key: &String| self.heights.get(key).copied();
        let total = self.origin
            + keys
                .iter()
                .map(|key| height(key).unwrap_or(guess) + gap)
                .sum::<Pixels>();
        // Сдвиг до раскладки может быть за краем (он прижмётся только в раскладке).
        let top = top.min((total - viewport).max(Pixels::ZERO));
        let (from, to) = (top - viewport * MARGIN, top + viewport * (1. + MARGIN));
        let focused = self
            .focused
            .take()
            .and_then(|focused| keys.iter().position(|key| *key == focused));
        let mut slots = Vec::new();
        let mut space: Option<Pixels> = None;
        let mut y = self.origin;
        let mut fresh = 0;
        let batch = if viewport <= Pixels::ZERO {
            BATCH_FIRST
        } else if scrolling {
            // Во время активной прокрутки не тратим бюджет кадра на домер далёких строк.
            0
        } else {
            BATCH
        };
        for (index, key) in keys.iter().enumerate() {
            let near_focus = focused.is_some_and(|at| at.abs_diff(index) <= 1);
            let build = match height(key) {
                None => {
                    let near = near_focus || (y + guess > from && y < to);
                    let build = near || fresh < batch;
                    if build && !near {
                        fresh += 1;
                    }
                    build
                }
                Some(h) => self.resizing || near_focus || (y + h > from && y < to),
            };
            let h = height(key).unwrap_or(guess);
            if build {
                slots.extend(space.take().map(Slot::Space));
                slots.push(Slot::Row(index));
            } else {
                // Промежуток между строками распорки — внутри неё, вокруг — от колонки.
                space = Some(space.map_or(h, |space| space + gap + h));
            }
            y += h + gap;
        }
        slots.extend(space.map(Slot::Space));
        slots
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, size};

    fn p(value: f32) -> Pixels {
        Pixels::from(value)
    }

    fn keys(n: usize) -> Vec<String> {
        (0..n).map(|index| format!("k{index}")).collect()
    }

    /// 10 строк по 100 px через 10: всё измерено при ширине 500.
    fn measured_state() -> State {
        let mut state = State::default();
        for key in keys(10) {
            state.heights.insert(key, p(100.));
        }
        state.measured_at = Some((p(500.), 1.));
        state
    }

    #[test]
    fn rows_outside_the_window_become_one_spacer_each_side() {
        let mut state = measured_state();
        // Видно 400..600, окно с запасом 300..700: строки 2 (220..320) – 6 (660..760).
        let slots = state.slots_for(&keys(10), p(10.), p(400.), p(200.), false);
        assert_eq!(
            slots,
            vec![
                // Строки и промежутки между ними; промежуток после распорки — от колонки.
                Slot::Space(p(210.)),
                Slot::Row(2),
                Slot::Row(3),
                Slot::Row(4),
                Slot::Row(5),
                Slot::Row(6),
                Slot::Space(p(320.)),
            ]
        );
    }

    #[test]
    fn unmeasured_rows_and_the_focused_row_with_neighbours_are_built() {
        let mut state = measured_state();
        state.heights.remove("k8");
        state.focused = Some("k0".into());
        let slots = state.slots_for(&keys(10), p(10.), p(400.), p(200.), false);
        let built: Vec<usize> = slots
            .iter()
            .filter_map(|slot| match slot {
                Slot::Row(index) => Some(*index),
                Slot::Space(_) => None,
            })
            .collect();
        assert_eq!(built, vec![0, 1, 2, 3, 4, 5, 6, 8]);
        // Отметка фокуса — на один план: дальше её ставит строитель.
        assert_eq!(state.focused, None);
    }

    #[test]
    fn unmeasured_rows_far_away_are_built_a_batch_per_frame() {
        // 100 строк по 100 px, измерены первые 10: видно 0..200, окно −100..300 (строки 0–2).
        // Из неизмеренных 10–99 за кадр строится BATCH, остальные — распорка средней высоты.
        let mut state = measured_state();
        let slots = state.slots_for(&keys(100), p(10.), p(0.), p(200.), false);
        let built: Vec<usize> = slots
            .iter()
            .filter_map(|slot| match slot {
                Slot::Row(index) => Some(*index),
                Slot::Space(_) => None,
            })
            .collect();
        let mut want = vec![0, 1, 2];
        want.extend(10..10 + BATCH);
        assert_eq!(built, want);
        // Колонка той же высоты, что со всеми строками: распорки по средней высоте.
        let total: Pixels = slots
            .iter()
            .map(|slot| match slot {
                Slot::Row(_) => p(100.),
                Slot::Space(h) => *h,
            })
            .sum::<Pixels>()
            + p(10.) * (slots.len() as f32 - 1.);
        assert_eq!(total, p(100. * 100. + 10. * 99.));
        // Во время активной прокрутки далёкие неизмеренные строки не строятся.
        let scrolling_slots = state.slots_for(&keys(100), p(10.), p(0.), p(200.), true);
        let scrolling_built: Vec<usize> = scrolling_slots
            .iter()
            .filter_map(|slot| match slot {
                Slot::Row(index) => Some(*index),
                Slot::Space(_) => None,
            })
            .collect();
        assert_eq!(scrolling_built, vec![0, 1, 2]);
    }

    #[test]
    fn scroll_past_the_end_keeps_the_last_rows() {
        let mut state = measured_state();
        // Сдвиг 5000 при содержимом 1100: прижат к 900 (видно 900..1100, последние строки).
        let slots = state.slots_for(&keys(10), p(10.), p(5000.), p(200.), false);
        assert_eq!(slots.last(), Some(&Slot::Row(9)));
        assert!(slots.contains(&Slot::Row(8)));
    }

    #[test]
    fn width_change_builds_everything_until_it_settles() {
        let rows = Rows::default();
        let scroll_keys = keys(3);
        {
            let mut state = rows.0.borrow_mut();
            state.keys = scroll_keys;
            state.slots = vec![Some(0), Some(1), Some(2)];
        }
        let row = |y: f32, w: f32, h: f32| Bounds::new(point(p(0.), p(y)), size(p(w), p(h)));
        let frame = |w: f32, h: f32| vec![row(0., w, h), row(h, w, h), row(2. * h, w, h)];
        // Первый замер: строки новые, план до замера стоял на догадке — перерисовать.
        assert!(rows.measured(&frame(500., 100.), p(0.), 1.));
        assert!(!rows.0.borrow().resizing);
        // Тот же кадр — ничего не сменилось.
        assert!(!rows.measured(&frame(500., 100.), p(0.), 1.));
        // Ширина сменилась: строить всё, пока не устоится.
        assert!(rows.measured(&frame(480., 120.), p(0.), 1.));
        assert!(rows.0.borrow().resizing);
        let slots = rows
            .0
            .borrow_mut()
            .slots_for(&keys(3), p(0.), p(0.), p(10.), false);
        assert_eq!(slots, vec![Slot::Row(0), Slot::Row(1), Slot::Row(2)]);
        // Устоялась: ещё кадр перерисовки, дальше — окно.
        assert!(rows.measured(&frame(480., 120.), p(0.), 1.));
        assert!(!rows.0.borrow().resizing);
        assert!(!rows.measured(&frame(480., 120.), p(0.), 1.));
    }
}
