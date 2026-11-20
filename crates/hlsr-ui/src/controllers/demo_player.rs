//! Логика плеера «Демо» (`DemoPlayer.vue` 0.4.9) без окна: длина и место шкалы (с видео — время
//! клипа, без него — время демо), интервал экспорта на шкале, метка у курсора (привязка 6 px),
//! подсказка, время отметки интервала, громкость, скорость, «во весь экран», клавиши.
//! Перевод времени клипа ↔ времени демо — `demo_tool::{demo_time_at, video_time_at}`.

use super::boot::Translate;
use super::demo_tool::{ClipRange, clock, demo_time_at, marker_text, video_time_at};
use super::jsv;
use serde_json::Value;

/// Метка ближе стольких CSS px к курсору — щелчок и подсказка берут её.
pub const SNAP_PX: f64 = 6.;
/// Шаг стрелок, с; с Shift — 1 с.
pub const STEP: f64 = 5.;

/// Пропсы плеера, нужные логике (`:src`, `:video`, `:markers`, `:duration`, `:range`).
#[derive(Debug, Clone, Copy)]
pub struct Props<'a> {
    /// Видео есть (`!!src`).
    pub src: bool,
    pub video: Option<&'a Value>,
    pub markers: &'a [Value],
    pub duration: f64,
    pub range: Option<ClipRange>,
}

/// Подсказка над шкалой: место (доля ширины), время, подписи меток точки и тип первой.
#[derive(Debug, Clone, PartialEq)]
pub struct Hover {
    pub left: f64,
    pub time: String,
    pub label: String,
    pub kind: String,
}

/// Клавиша плеера (`onKey`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Key {
    TogglePlay,
    /// Перейти ко времени шкалы.
    Seek(f64),
    /// Громкость (`setVolume`).
    Volume(f64),
    Fullscreen,
    CloseMenu,
}

/// Состояние плеера (`data()` компонента).
#[derive(Debug, Clone, PartialEq)]
pub struct Player {
    pub playing: bool,
    /// Время видео (`current`).
    pub current: f64,
    /// Место на шкале без видео — время демо (`cursor`).
    pub cursor: f64,
    /// Длина файла после `loadedmetadata` (`mediaDuration`).
    pub media_duration: f64,
    pub volume: f64,
    pub muted: bool,
    pub rate: f64,
    pub rate_open: bool,
    /// Плеер на всё окно (`expanded`).
    pub expanded: bool,
    /// Окно ушло в полный экран через `demo-player-fullscreen` (`windowFull`).
    pub window_full: bool,
    /// Ждём ответ `demo-player-fullscreen` (`switching`).
    pub switching: bool,
    pub hover: Option<Hover>,
    pub dragging: bool,
}

impl Default for Player {
    fn default() -> Self {
        Player {
            playing: false,
            current: 0.,
            cursor: 0.,
            media_duration: 0.,
            volume: 1.,
            muted: false,
            rate: 1.,
            rate_open: false,
            expanded: false,
            window_full: false,
            switching: false,
            hover: None,
            dragging: false,
        }
    }
}

/// `Math.max(lo, Math.min(hi, x))`.
fn clamp(x: f64, lo: f64, hi: f64) -> f64 {
    lo.max(hi.min(x))
}

impl Player {
    /// Длина шкалы: файл (или `timeline.video.duration`), без видео — длина демо.
    pub fn length(&self, p: &Props) -> f64 {
        if p.src {
            if self.media_duration != 0. && !self.media_duration.is_nan() {
                return self.media_duration;
            }
            return jsv::or_zero(jsv::field(p.video, "duration"));
        }
        if p.duration.is_nan() { 0. } else { p.duration }
    }

    /// Место головки: время видео или курсор демо.
    pub fn position(&self, p: &Props) -> f64 {
        if p.src { self.current } else { self.cursor }
    }

    /// `percent(t)` долей ширины: 0…1.
    pub fn fraction(&self, t: f64, p: &Props) -> f64 {
        let length = self.length(p);
        if length != 0. && !length.is_nan() {
            clamp(t / length, 0., 1.)
        } else {
            0.
        }
    }

    /// `rangeOnBar`: интервал экспорта на шкале (доли: начало, ширина); с видео — во времени
    /// клипа. Пустой или вывернутый — нет.
    pub fn range_on_bar(&self, p: &Props) -> Option<(f64, f64)> {
        let range = p.range?;
        let length = self.length(p);
        if length == 0. || length.is_nan() {
            return None;
        }
        let at = |t: f64| {
            if p.src {
                video_time_at(t, p.video, p.markers)
            } else {
                t
            }
        };
        let a = clamp(at(range.from), 0., length);
        let b = clamp(at(range.to), 0., length);
        // `b <= a` и NaN: полосы нет.
        if b.partial_cmp(&a) != Some(std::cmp::Ordering::Greater) {
            return None;
        }
        Some((self.fraction(a, p), (b - a) / length))
    }

    /// Сменилось видео (`watch: src`).
    pub fn src_changed(&mut self) {
        self.playing = false;
        self.current = 0.;
        self.media_duration = 0.;
    }

    /// Сменилась длина демо (`watch: duration`).
    pub fn duration_changed(&mut self) {
        self.cursor = 0.;
    }

    /// `seek(t)`: место в пределах шкалы. Ответ — время видео для перемотки (с видео).
    pub fn seek(&mut self, t: f64, p: &Props) -> Option<f64> {
        let value = clamp(t, 0., self.length(p));
        if p.src {
            self.current = value;
            Some(value)
        } else {
            self.cursor = value;
            None
        }
    }

    /// `seekDemo(demoTime)`: переход по метке карточки (время демо).
    pub fn seek_demo(&mut self, demo_time: f64, p: &Props) -> Option<f64> {
        let t = if p.src {
            video_time_at(demo_time, p.video, p.markers)
        } else {
            demo_time
        };
        self.seek(t, p)
    }

    /// `mark(which)`: время демо в текущей точке до мс — у клипа через интервал, скорость и метки.
    pub fn mark_time(&self, p: &Props) -> f64 {
        let demo_time = if p.src {
            demo_time_at(self.current, p.video, p.markers)
        } else {
            self.cursor
        };
        jsv::round(demo_time * 1000.) / 1000.
    }

    /// Метка не дальше [`SNAP_PX`] от `x` на шкале шириной `width` (CSS px); ближайшая.
    pub fn marker_near<'a>(&self, x: f64, width: f64, p: &Props<'a>) -> Option<&'a Value> {
        let length = self.length(p);
        let mut best: Option<(&Value, f64)> = None;
        for marker in p.markers {
            let t = jsv::num(jsv::prop(marker, "t")).unwrap_or(f64::NAN);
            let distance = (t / length * width - x).abs();
            if distance <= SNAP_PX && best.is_none_or(|(_, near)| distance < near) {
                best = Some((marker, distance));
            }
        }
        best.map(|(marker, _)| marker)
    }

    /// Нажатие на шкалу в `x` из `width` (CSS px): головка — к метке рядом или под курсор, тянем.
    /// Ответ — время видео для перемотки.
    pub fn track_down(&mut self, x: f64, width: f64, p: &Props) -> Option<f64> {
        let length = self.length(p);
        if length == 0. || length.is_nan() {
            return None;
        }
        let x = clamp(x, 0., width);
        let t = self.marker_near(x, width, p).map_or_else(
            || if width != 0. { x / width * length } else { 0. },
            |near| jsv::num(jsv::prop(near, "t")).unwrap_or(f64::NAN),
        );
        self.dragging = true;
        self.seek(t, p)
    }

    /// Движение над шкалой (или, пока тянут, по окну): подсказка; тянут — головка за курсором.
    /// Ответ — время видео для перемотки.
    pub fn track_move(&mut self, x: f64, width: f64, p: &Props, t: Translate) -> Option<f64> {
        let length = self.length(p);
        if length == 0. || length.is_nan() {
            return None;
        }
        let x = clamp(x, 0., width);
        let at = if width != 0. { x / width * length } else { 0. };
        let seek = if self.dragging {
            self.seek(at, p)
        } else {
            None
        };
        let near = if self.dragging {
            None
        } else {
            self.marker_near(x, width, p)
        };
        let near_t = near.and_then(|near| jsv::num(jsv::prop(near, "t")));
        // Все метки этой точки: на одном стыке бывают карта, глава и таймер.
        let label = near.map_or_else(String::new, |_| {
            p.markers
                .iter()
                .filter(|marker| jsv::num(jsv::prop(marker, "t")) == near_t)
                .map(|marker| marker_text(marker, t))
                .collect::<Vec<_>>()
                .join(" · ")
        });
        self.hover = Some(Hover {
            left: x / width,
            time: clock(if near.is_some() {
                near_t.unwrap_or(f64::NAN)
            } else {
                at
            }),
            label,
            kind: jsv::str_of(near.and_then(|near| jsv::prop(near, "type")))
                .unwrap_or_default()
                .to_owned(),
        });
        seek
    }

    /// Отпустили шкалу. Ответ — тянули ли.
    pub fn track_up(&mut self) -> bool {
        std::mem::take(&mut self.dragging)
    }

    /// Громкость, которую слышно: без звука — 0.
    pub fn heard_volume(&self) -> f64 {
        if self.muted { 0. } else { self.volume }
    }

    /// `setVolume(value)`: 0 — без звука.
    pub fn set_volume(&mut self, value: f64) {
        self.volume = value;
        self.muted = value == 0.;
    }

    /// `toggleMute`: звук на 0 включается на половине.
    pub fn toggle_mute(&mut self) {
        self.muted = !self.muted;
        if !self.muted && self.volume == 0. {
            self.volume = 0.5;
        }
    }

    pub fn volume_icon(&self) -> &'static str {
        if self.muted || self.volume == 0. {
            "fas fa-volume-mute"
        } else if self.volume < 0.5 {
            "fas fa-volume-down"
        } else {
            "fas fa-volume-up"
        }
    }

    /// `setRate(value)`: меню закрывается.
    pub fn set_rate(&mut self, value: f64) {
        self.rate = value;
        self.rate_open = false;
    }

    /// Ответ `demo-player-fullscreen` на вход: `true` — окно на экране; иначе плеер на всё окно.
    pub fn entered(&mut self, window_full: bool) {
        self.window_full = window_full;
        self.expanded = true;
    }

    /// `exitFull`: ответ — отпустить полный экран окна (`demo-player-fullscreen` false).
    pub fn exit_full(&mut self) -> bool {
        self.expanded = false;
        std::mem::take(&mut self.window_full)
    }

    /// Окно вышло из полного экрана само (`demo-player-fullscreen-changed` false).
    pub fn window_left(&mut self) {
        if self.window_full {
            self.window_full = false;
            self.expanded = false;
        }
    }

    /// `onKey`: клавиша GPUI (`space`, `k`, стрелки, `home`, `end`, `f`, `escape`). Сочетания с
    /// Ctrl/Alt/Win плееру не принадлежат; Shift — шаг стрелок 1 с.
    pub fn key(&self, key: &str, shift: bool, chord: bool, p: &Props) -> Option<Key> {
        if chord {
            return None;
        }
        let step = if shift { 1. } else { STEP };
        let volume = self.heard_volume();
        // `.toFixed(2)`: громкость стрелками — до сотых.
        let fixed = |v: f64| jsv::round(v * 100.) / 100.;
        Some(match key {
            "space" | "k" => Key::TogglePlay,
            "left" => Key::Seek(self.position(p) - step),
            "right" => Key::Seek(self.position(p) + step),
            "up" => Key::Volume(fixed((volume + 0.1).min(1.))),
            "down" => Key::Volume(fixed((volume - 0.1).max(0.))),
            "home" => Key::Seek(0.),
            "end" => Key::Seek(self.length(p)),
            "f" => Key::Fullscreen,
            "escape" if self.rate_open => Key::CloseMenu,
            _ => return None,
        })
    }
}

/// `rateText(value)`: «0,25×», «1×» (запятая — `#L_DECIMAL`).
pub fn rate_text(value: f64, decimal: &str) -> String {
    format!("{}×", jsv::number_text(value).replacen('.', decimal, 1))
}

/// Громкость ползунка под `x` из `width` CSS px (`<input type="range" step="0.05">`): бегунок
/// 12 px ходит от края до края, значение — к шагу.
pub fn volume_at(x: f64, width: f64) -> f64 {
    const THUMB: f64 = 12.;
    let span = width - THUMB;
    let along = if span > 0. {
        clamp((x - THUMB / 2.) / span, 0., 1.)
    } else {
        0.
    };
    jsv::round(along * 20.) / 20.
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tr(key: &str, args: &[&str]) -> String {
        if args.is_empty() {
            key.to_owned()
        } else {
            format!("{key}({})", args.join(","))
        }
    }

    fn markers() -> Vec<Value> {
        // Клип с 20-й секунды демо на ×2; загрузка вырезана: 15 с видео — 60 с демо.
        vec![
            json!({"t": 0, "type": "map", "label": "c1a0", "sourceTime": 20}),
            json!({"t": 15, "type": "saveload", "label": "2", "sourceTime": 60}),
            json!({"t": 15, "type": "timer", "label": "1:00", "sourceTime": 60}),
        ]
    }

    #[test]
    fn clip_time_and_demo_time_round_trip_through_markers_speed_and_range() {
        let video = json!({"range": {"from": 20, "to": 80}, "speed": 2, "duration": 25});
        let markers = markers();
        let p = Props {
            src: true,
            video: Some(&video),
            markers: &markers,
            duration: 300.,
            range: Some(ClipRange { from: 30., to: 70. }),
        };
        let mut player = Player::default();
        // Без файла — длина из timeline.video.
        assert_eq!(player.length(&p), 25.);
        player.media_duration = 25.04;
        assert_eq!(player.length(&p), 25.04);
        // Видео 2 с → демо 20 + 2·2 = 24; после стыка: видео 17 → 60 + 2·2 = 64.
        player.seek(2., &p);
        assert_eq!(player.mark_time(&p), 24.);
        player.seek(17., &p);
        assert_eq!(player.mark_time(&p), 64.);
        // Обратно: метка карточки 64 с демо → 17 с видео.
        assert_eq!(player.seek_demo(64., &p), Some(17.));
        assert_eq!(player.current, 17.);
        // До мс, как Math.round(t * 1000) / 1000.
        player.seek(1.2345, &p);
        assert_eq!(player.mark_time(&p), 22.469);
        // Интервал 30…70 демо → 5…20 с видео на шкале 25,04 с.
        let (left, width) = player.range_on_bar(&p).unwrap();
        assert!((left - 5. / 25.04).abs() < 1e-12);
        assert!((width - 15. / 25.04).abs() < 1e-12);
        // Перемотка не выходит за шкалу.
        assert_eq!(player.seek(99., &p), Some(25.04));
        assert_eq!(player.seek(-3., &p), Some(0.));
    }

    #[test]
    fn without_video_the_timeline_is_demo_time() {
        let markers = markers();
        let mut p = Props {
            src: false,
            video: None,
            markers: &markers,
            duration: 144.46,
            range: Some(ClipRange {
                from: 61.2,
                to: 40.,
            }),
        };
        let mut player = Player::default();
        assert_eq!(player.length(&p), 144.46);
        assert_eq!(player.seek(25.85, &p), None);
        assert_eq!(player.cursor, 25.85);
        assert_eq!(player.position(&p), 25.85);
        assert_eq!(player.mark_time(&p), 25.85);
        assert_eq!(player.seek_demo(98.4, &p), None);
        assert_eq!(player.cursor, 98.4);
        // Вывернутый интервал на шкале не рисуется; нормальный — во времени демо.
        assert_eq!(player.range_on_bar(&p), None);
        p.range = Some(ClipRange {
            from: 40.,
            to: 61.2,
        });
        let (left, width) = player.range_on_bar(&p).unwrap();
        assert!((left - 40. / 144.46).abs() < 1e-12 && (width - 21.2 / 144.46).abs() < 1e-12);
        // Новая демка — курсор в начало; демки нет — шкалы нет.
        player.duration_changed();
        assert_eq!(player.cursor, 0.);
        p.duration = 0.;
        assert_eq!(player.fraction(10., &p), 0.);
        assert_eq!(player.range_on_bar(&p), None);
        assert_eq!(player.track_down(50., 100., &p), None);
        assert!(!player.dragging);
    }

    #[test]
    fn the_track_snaps_to_markers_within_six_px_and_the_tip_names_all_of_them() {
        let video = json!({"range": {"from": 20, "to": 80}, "speed": 2, "duration": 30});
        let markers = markers();
        let p = Props {
            src: true,
            video: Some(&video),
            markers: &markers,
            duration: 0.,
            range: None,
        };
        let mut player = Player::default();
        // Шкала 300 px на 30 с: метка 15 с — на 150 px.
        assert_eq!(player.track_move(144., 300., &p, &tr), None);
        let hover = player.hover.clone().unwrap();
        assert_eq!(hover.time, "0:15.0");
        assert_eq!(
            hover.label,
            "#L_DEMO_MARK_SAVELOAD(2) · #L_DEMO_MARK_TIMER(1:00)"
        );
        assert_eq!((hover.kind.as_str(), hover.left), ("saveload", 0.48));
        // 7 px — уже мимо: время под курсором, без подписи.
        player.track_move(143., 300., &p, &tr);
        let hover = player.hover.clone().unwrap();
        assert_eq!((hover.time.as_str(), hover.label.as_str()), ("0:14.3", ""));
        // Нажатие рядом с меткой — головка на неё; дальше тянут без привязки и за край.
        assert_eq!(player.track_down(147., 300., &p), Some(15.));
        assert!(player.dragging);
        assert_eq!(player.track_move(147., 300., &p, &tr), Some(14.7));
        assert_eq!(player.track_move(75., 300., &p, &tr), Some(7.5));
        assert_eq!(player.hover.as_ref().unwrap().label, "");
        assert_eq!(player.track_move(400., 300., &p, &tr), Some(30.));
        assert!(player.track_up());
        assert!(!player.track_up());
    }

    #[test]
    fn volume_rate_fullscreen_and_keys() {
        let markers = Vec::new();
        let p = Props {
            src: true,
            video: None,
            markers: &markers,
            duration: 0.,
            range: None,
        };
        let mut player = Player {
            media_duration: 60.,
            current: 3.,
            ..Player::default()
        };
        assert_eq!(player.volume_icon(), "fas fa-volume-up");
        player.set_volume(0.4);
        assert_eq!(player.volume_icon(), "fas fa-volume-down");
        player.set_volume(0.);
        assert!(player.muted);
        assert_eq!(player.volume_icon(), "fas fa-volume-mute");
        // Звук на нуле включается на половине; выключенный слышен как 0.
        player.toggle_mute();
        assert_eq!((player.muted, player.volume), (false, 0.5));
        player.toggle_mute();
        assert_eq!(player.heard_volume(), 0.);
        // Стрелки громкости — от слышимой, до сотых, в пределах 0…1.
        assert_eq!(player.key("up", false, false, &p), Some(Key::Volume(0.1)));
        player.set_volume(0.95);
        assert_eq!(player.key("up", false, false, &p), Some(Key::Volume(1.)));
        player.set_volume(0.07);
        assert_eq!(player.key("down", false, false, &p), Some(Key::Volume(0.)));
        assert_eq!(player.key("left", false, false, &p), Some(Key::Seek(-2.)));
        assert_eq!(player.key("right", true, false, &p), Some(Key::Seek(4.)));
        assert_eq!(player.key("end", false, false, &p), Some(Key::Seek(60.)));
        assert_eq!(player.key("k", false, false, &p), Some(Key::TogglePlay));
        assert_eq!(player.key("f", false, true, &p), None);
        assert_eq!(player.key("escape", false, false, &p), None);
        player.rate_open = true;
        assert_eq!(player.key("escape", false, false, &p), Some(Key::CloseMenu));
        player.set_rate(0.25);
        assert!(!player.rate_open);
        assert_eq!(rate_text(0.25, ","), "0,25×");
        assert_eq!(rate_text(1., ","), "1×");
        // Полный экран окна: вход, выход системой, отказ канала — на всё окно.
        player.entered(true);
        assert!(player.expanded && player.window_full);
        player.window_left();
        assert!(!player.expanded && !player.window_full);
        player.entered(false);
        player.window_left();
        assert!(
            player.expanded,
            "плеер на всё окно, окно не в полном экране"
        );
        assert!(!player.exit_full());
        player.entered(true);
        assert!(player.exit_full());
        assert!(!player.expanded);
        // Ползунок 76 px: бегунок 12 px от края до края, шаг 0,05.
        assert_eq!(volume_at(0., 76.), 0.);
        assert_eq!(volume_at(38., 76.), 0.5);
        assert_eq!(volume_at(70., 76.), 1.);
        assert_eq!(volume_at(20., 76.), 0.2);
    }
}
