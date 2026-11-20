//! Ворота движения окна лаунчера. `still` — движения нет совсем (`.no-anim` и
//! `prefers-reduced-motion` 0.4.9: переходы мгновенные, бесконечное — по правилу `still`
//! реестра). `hidden` и `top_layer` только ставят бесконечное на паузу: окно свёрнуто или
//! спрятано в трей (A2), поверх открыто окно-слой (F7).

use gpui::{App, Global};
use hlsr_ui::motion::Still;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnimGate {
    pub still: Still,
    pub hidden: bool,
    /// Верхний открытый слой окон: бесконечное ниже него стоит.
    pub top_layer: usize,
}

impl Global for AnimGate {}

impl AnimGate {
    pub fn get(cx: &App) -> AnimGate {
        cx.try_global::<AnimGate>().copied().unwrap_or_default()
    }

    pub fn still(cx: &App) -> Still {
        Self::get(cx).still
    }

    /// prefs `animations = false` (класс `.no-anim`).
    pub fn set_no_anim(cx: &mut App, on: bool) {
        Self::update(cx, |gate| gate.still.no_anim = on);
    }

    /// Системный reduced-motion.
    pub fn set_reduce_motion(cx: &mut App, on: bool) {
        Self::update(cx, |gate| gate.still.os_reduced = on);
    }

    /// Окно свёрнуто или спрятано.
    pub fn set_window_hidden(cx: &mut App, hidden: bool) {
        Self::update(cx, |gate| gate.hidden = hidden);
    }

    /// Сколько окон-слоёв открыто (0 — ни одного).
    pub fn set_top_layer(cx: &mut App, layer: usize) {
        Self::update(cx, |gate| gate.top_layer = layer);
    }

    fn update(cx: &mut App, change: impl FnOnce(&mut AnimGate)) {
        let mut gate = Self::get(cx);
        change(&mut gate);
        if gate == Self::get(cx) {
            return;
        }
        cx.set_global(gate);
        // with_animation/with_spring GPUI (кольцо fa-spin и др.) встают в конечное состояние.
        cx.set_reduce_motion(gate.still.any());
        // Снятая пауза: остановленному никто больше не попросит кадр.
        cx.refresh_windows();
    }

    pub(super) fn runs(cx: &App, layer: usize) -> bool {
        let gate = Self::get(cx);
        !gate.hidden && layer >= gate.top_layer
    }
}
