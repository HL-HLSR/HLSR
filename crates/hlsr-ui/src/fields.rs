//! Логика полей ввода (Ф5): семантика `<input type="number">` (min, max, step,
//! пустое значение, stepUp/stepDown), `<input type="range">` (привязка к шагу,
//! клавиши), `maxlength`, `#rrggbb` — по HTML Standard и Blink, как их читает
//! код 0.4.7 (`GxNumStep.vue`, `GhostingSettingsExtras.vue`, `gx-range`).
//!
//! Числа считаются десятичными (как `Decimal` Blink): `0.1 + 0.2` — ровно
//! `0.3`, а не `0.30000000000000004`.

use crate::Rgba as Color;
use std::cmp::Ordering;

/// Десятичное число `mantissa × 10^exp` (exp ≤ 0 после нормализации).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dec {
    mantissa: i128,
    /// Число знаков после запятой.
    scale: u32,
}

/// Сколько знаков мантиссы допускается (`i128` держит 38).
const MAX_DIGITS: usize = 30;
/// Сколько знаков после запятой допускается.
const MAX_SCALE: u32 = 18;

impl Dec {
    pub const ZERO: Dec = Dec {
        mantissa: 0,
        scale: 0,
    };

    pub fn from_int(value: i64) -> Dec {
        Dec {
            mantissa: i128::from(value),
            scale: 0,
        }
    }

    /// Допустимое число с плавающей точкой по HTML Standard: `-?(\d+(\.\d+)?|\.\d+)([eE][-+]?\d+)?`.
    /// Запятая вместо точки принимается (локаль ввода у Chromium).
    pub fn parse(text: &str) -> Option<Dec> {
        let text = text.replace(',', ".");
        let bytes = text.as_bytes();
        let mut i = 0;
        let negative = bytes.first() == Some(&b'-');
        if negative {
            i += 1;
        }
        let int_start = i;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        let int_digits = &text[int_start..i];
        let mut frac_digits = "";
        if bytes.get(i) == Some(&b'.') {
            let start = i + 1;
            let mut j = start;
            while bytes.get(j).is_some_and(u8::is_ascii_digit) {
                j += 1;
            }
            // «5.» и «.» недопустимы: после точки нужна цифра.
            if j == start {
                return None;
            }
            frac_digits = &text[start..j];
            i = j;
        } else if int_digits.is_empty() {
            return None;
        }
        let mut exponent: i32 = 0;
        if matches!(bytes.get(i), Some(b'e' | b'E')) {
            i += 1;
            let sign = match bytes.get(i) {
                Some(b'-') => {
                    i += 1;
                    -1
                }
                Some(b'+') => {
                    i += 1;
                    1
                }
                _ => 1,
            };
            let start = i;
            while bytes.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
            if i == start || i - start > 4 {
                return None;
            }
            exponent = sign * text[start..i].parse::<i32>().ok()?;
        }
        if i != bytes.len() {
            return None;
        }
        let digits = format!("{int_digits}{frac_digits}");
        let digits = digits.trim_start_matches('0');
        if digits.len() > MAX_DIGITS {
            return None;
        }
        let mut mantissa: i128 = if digits.is_empty() {
            0
        } else {
            digits.parse().ok()?
        };
        if negative {
            mantissa = -mantissa;
        }
        // Знаков после запятой: из дроби минус порядок.
        let mut scale = frac_digits.len() as i32 - exponent;
        if scale < 0 {
            let shift = (-scale) as u32;
            if shift as usize + digits.len() > MAX_DIGITS {
                return None;
            }
            mantissa = mantissa.checked_mul(10_i128.checked_pow(shift)?)?;
            scale = 0;
        }
        if scale as u32 > MAX_SCALE {
            return None;
        }
        Some(
            Dec {
                mantissa,
                scale: scale as u32,
            }
            .trimmed(),
        )
    }

    fn trimmed(mut self) -> Dec {
        while self.scale > 0 && self.mantissa % 10 == 0 {
            self.mantissa /= 10;
            self.scale -= 1;
        }
        self
    }

    fn aligned(a: Dec, b: Dec) -> Option<(i128, i128, u32)> {
        let scale = a.scale.max(b.scale);
        let up = |d: Dec| {
            10_i128
                .checked_pow(scale - d.scale)
                .and_then(|k| d.mantissa.checked_mul(k))
        };
        Some((up(a)?, up(b)?, scale))
    }

    pub fn checked_add(self, other: Dec) -> Option<Dec> {
        let (a, b, scale) = Dec::aligned(self, other)?;
        Some(
            Dec {
                mantissa: a.checked_add(b)?,
                scale,
            }
            .trimmed(),
        )
    }

    pub fn checked_sub(self, other: Dec) -> Option<Dec> {
        let (a, b, scale) = Dec::aligned(self, other)?;
        Some(
            Dec {
                mantissa: a.checked_sub(b)?,
                scale,
            }
            .trimmed(),
        )
    }

    pub fn mul_int(self, k: i64) -> Option<Dec> {
        Some(
            Dec {
                mantissa: self.mantissa.checked_mul(i128::from(k))?,
                scale: self.scale,
            }
            .trimmed(),
        )
    }

    pub fn is_zero(self) -> bool {
        self.mantissa == 0
    }

    /// `⌊self / step⌋` (шаг положителен).
    fn floor_div(self, step: Dec) -> Option<i64> {
        let (a, b, _) = Dec::aligned(self, step)?;
        i64::try_from(a.div_euclid(b)).ok()
    }

    /// `⌈self / step⌉`.
    fn ceil_div(self, step: Dec) -> Option<i64> {
        let (a, b, _) = Dec::aligned(self, step)?;
        let floor = a.div_euclid(b);
        i64::try_from(if a.rem_euclid(b) == 0 {
            floor
        } else {
            floor + 1
        })
        .ok()
    }

    fn is_multiple_of(self, step: Dec) -> bool {
        Dec::aligned(self, step).is_some_and(|(a, b, _)| a.rem_euclid(b) == 0)
    }

    pub fn to_f64(self) -> f64 {
        self.mantissa as f64 / 10_f64.powi(self.scale as i32)
    }
}

impl Ord for Dec {
    fn cmp(&self, other: &Dec) -> Ordering {
        match Dec::aligned(*self, *other) {
            Some((a, b, _)) => a.cmp(&b),
            None => self.to_f64().total_cmp(&other.to_f64()),
        }
    }
}

impl PartialOrd for Dec {
    fn partial_cmp(&self, other: &Dec) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl std::fmt::Display for Dec {
    /// Позиционная запись без показателя степени, без хвостовых нулей.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sign = if self.mantissa < 0 { "-" } else { "" };
        let digits = self.mantissa.unsigned_abs().to_string();
        if self.scale == 0 {
            return write!(f, "{sign}{digits}");
        }
        let scale = self.scale as usize;
        let padded = format!("{digits:0>width$}", width = scale + 1);
        let (int, frac) = padded.split_at(padded.len() - scale);
        write!(f, "{sign}{int}.{frac}")
    }
}

/// Шаг `<input step>`: число или `any`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Any,
    By(Dec),
}

impl Step {
    /// Атрибут `step`: пусто или неправильно — 1; `any` — без шага (регистр
    /// не важен); число ≤ 0 — 1.
    pub fn from_attribute(text: &str) -> Step {
        let text = text.trim();
        if text.eq_ignore_ascii_case("any") {
            return Step::Any;
        }
        match Dec::parse(text) {
            Some(step) if step > Dec::ZERO => Step::By(step),
            _ => Step::By(Dec::from_int(1)),
        }
    }
}

/// Границы и шаг числового поля (`min`, `max`, `step`; пустая строка — нет).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NumberRange {
    pub min: Option<Dec>,
    pub max: Option<Dec>,
    pub step: Step,
}

impl NumberRange {
    pub fn from_attributes(min: &str, max: &str, step: &str) -> NumberRange {
        NumberRange {
            min: Dec::parse(min.trim()),
            max: Dec::parse(max.trim()),
            step: Step::from_attribute(step),
        }
    }

    /// База шагов: min или 0.
    fn base(&self) -> Dec {
        self.min.unwrap_or(Dec::ZERO)
    }

    /// Наибольшее значение сетки шага не больше max.
    fn aligned_max(&self) -> Option<Dec> {
        let max = self.max?;
        match self.step {
            Step::Any => Some(max),
            Step::By(step) => {
                let base = self.base();
                let n = max.checked_sub(base)?.floor_div(step)?;
                base.checked_add(step.mul_int(n)?)
            }
        }
    }

    /// Новое значение после `count` шагов от `current` (`stepUp(count)` для
    /// положительных, `stepDown(-count)` для отрицательных), по
    /// `InputType::ApplyStep` Blink; `None` — значение не меняется.
    fn apply_step(&self, current: Dec, count: i64) -> Option<Dec> {
        // 2. min > max — шаг не делается.
        if let (Some(min), Some(max)) = (self.min, self.max)
            && min > max
        {
            return None;
        }
        // 3. Между min и max нет значения сетки.
        let aligned_max = self.aligned_max();
        if let (Some(min), Some(aligned)) = (self.min, aligned_max)
            && aligned < min
        {
            return None;
        }
        let step_size = match self.step {
            Step::By(step) => step,
            // `any`: stepUp бросает InvalidStateError — поля 0.4.7 так не бывают.
            Step::Any => return None,
        };
        let base = self.base();
        let mut value = current;
        let mut count = count;
        if !value.checked_sub(base)?.is_multiple_of(step_size) {
            // Значение вне сетки: сперва ближайшее значение сетки в сторону шага.
            if count < 0 {
                let n = value.checked_sub(base)?.floor_div(step_size)?;
                value = base.checked_add(step_size.mul_int(n)?)?;
                count += 1;
            } else if count > 0 {
                let n = value.checked_sub(base)?.ceil_div(step_size)?;
                value = base.checked_add(step_size.mul_int(n)?)?;
                count -= 1;
            }
        }
        value = value.checked_add(step_size.mul_int(count)?)?;
        // 8. Меньше min — ближайшее значение сетки не меньше min.
        if let Some(min) = self.min
            && value < min
        {
            let n = min.checked_sub(base)?.ceil_div(step_size)?;
            value = base.checked_add(step_size.mul_int(n)?)?;
        }
        // 9. Больше max — наибольшее значение сетки не больше max.
        if let (Some(max), Some(aligned)) = (self.max, aligned_max)
            && value > max
        {
            value = aligned;
        }
        // 10. stepDown не должен увеличить, а stepUp — уменьшить значение.
        let direction = count.signum();
        if (direction < 0 && current < value) || (direction > 0 && current > value) {
            return None;
        }
        Some(value)
    }
}

/// Символ, который принимает `<input type="number">` при вводе (цифры, знаки,
/// точка и запятая — локальный разделитель, `e`/`E`).
pub fn accepts_number_char(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | ',' | 'e' | 'E')
}

/// Текст поля без символов, которые числовое поле не принимает (вставка).
pub fn filter_number_input(text: &str) -> String {
    text.chars().filter(|c| accepts_number_char(*c)).collect()
}

/// `input.value` числового поля: допустимое число — как напечатано (запятая —
/// точкой), иначе — пустая строка (badInput).
pub fn number_value(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    match Dec::parse(text) {
        Some(_) => text.replace(',', "."),
        None => String::new(),
    }
}

/// `stepUp()`/`stepDown()` (`direction` +1/−1, при желании больше) для текста
/// поля: новый текст или `None`, если значение не изменилось. Пустое поле
/// начинает с min или нуля (`DefaultValueForStepUp`).
pub fn number_step(text: &str, min: &str, max: &str, step: &str, direction: i64) -> Option<String> {
    let range = NumberRange::from_attributes(min, max, step);
    let step_size = match range.step {
        Step::By(step) => step,
        Step::Any => return None,
    };
    let parsed = Dec::parse(text);
    let current = match parsed {
        Some(current) => current,
        None => {
            // Пустое или неправильное: 0, но не ниже min − шаг и не выше max − шаг.
            let diff = step_size.mul_int(direction)?;
            let mut start = Dec::ZERO;
            if let Some(min) = range.min
                && start < min.checked_sub(diff)?
            {
                start = min.checked_sub(diff)?;
            }
            if let Some(max) = range.max
                && start > max.checked_sub(diff)?
            {
                start = max.checked_sub(diff)?;
            }
            start
        }
    };
    let next = range.apply_step(current, direction)?;
    if parsed == Some(next) {
        return None;
    }
    Some(next.to_string())
}

/// Значение ползунка `<input type="range">` после привязки к шагу и границам
/// (`step` — `None` для `any`): шаг отсчитывается от min, значение не выходит
/// за [min, наибольшее значение сетки ≤ max].
pub fn range_snap(value: f64, min: f64, max: f64, step: Option<f64>) -> f64 {
    let max = max.max(min);
    let value = value.clamp(min, max);
    let Some(step) = step.filter(|s| *s > 0.0) else {
        return value;
    };
    let steps = ((value - min) / step).round();
    let snapped = min + steps * step;
    let top = min + ((max - min) / step).floor() * step;
    // Ошибки двоичных дробей: 0.1 × 3 — не 0.30000000000000004.
    round_digits(snapped.clamp(min, top), 12)
}

fn round_digits(value: f64, digits: i32) -> f64 {
    let k = 10_f64.powi(digits);
    (value * k).round() / k
}

/// Клавиша ползунка: сдвиг значения (`RangeInputType::HandleKeydownEvent`).
/// `key`: 0 — стрелка вверх/вправо, 1 — вниз/влево, 2 — PageUp, 3 — PageDown,
/// 4 — Home, 5 — End. Шаг `any` — сотая доля диапазона, страница — десятая
/// (не меньше шага).
pub fn range_key(value: f64, min: f64, max: f64, step: Option<f64>, key: i32) -> f64 {
    let max = max.max(min);
    let one = step.filter(|s| *s > 0.0).unwrap_or((max - min) / 100.0);
    let page = ((max - min) / 10.0).max(one);
    let next = match key {
        0 => value + one,
        1 => value - one,
        2 => value + page,
        3 => value - page,
        4 => min,
        _ => max,
    };
    range_snap(next, min, max, step)
}

/// Не больше `max` единиц UTF-16 (`maxlength`); 0 — без ограничения.
pub fn truncate_utf16(text: &str, max: usize) -> String {
    if max == 0 {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut units = 0;
    for c in text.chars() {
        units += c.len_utf16();
        if units > max {
            break;
        }
        out.push(c);
    }
    out
}

/// `#rrggbb` строчными (значение `<input type="color">`).
pub fn color_hex(color: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

/// `#rrggbb` (регистр не важен; `#rgb` — нет, как у `setColor` 0.4.7) → цвет.
pub fn parse_hex(text: &str) -> Option<Color> {
    let body = text.strip_prefix('#')?;
    if body.len() != 6 || !body.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&body[at..at + 2], 16).ok();
    Some(Color::rgb(channel(0)?, channel(2)?, channel(4)?))
}

/// Сжатые поля в низком окне (`@media (max-height: 640px)`, settings-fields.scss:319).
pub fn is_compact(window_height: f32) -> bool {
    window_height <= 640.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(text: &str, min: &str, max: &str, step: &str, n: i64) -> Option<String> {
        number_step(text, min, max, step, n)
    }

    #[test]
    fn valid_floating_point_numbers_only() {
        for good in ["0", "-1", "12.5", ".5", "1e3", "1E-2", "-0.25", "1,5"] {
            assert!(Dec::parse(good).is_some(), "{good}");
        }
        for bad in [
            "", "-", ".", "5.", "+5", "1e", "e1", "1.2.3", "--1", "1 2", "abc", "0x10",
        ] {
            assert!(Dec::parse(bad).is_none(), "{bad}");
        }
        assert_eq!(number_value("12.5"), "12.5");
        assert_eq!(number_value("1,5"), "1.5");
        assert_eq!(number_value("5."), "");
        assert_eq!(number_value(""), "");
        assert_eq!(Dec::parse("1e3").unwrap().to_string(), "1000");
        assert_eq!(Dec::parse("-0.250").unwrap().to_string(), "-0.25");
        assert_eq!(Dec::parse(".5").unwrap().to_string(), "0.5");
    }

    #[test]
    fn step_up_and_down_with_bounds() {
        assert_eq!(step("5", "0", "10", "1", 1).as_deref(), Some("6"));
        assert_eq!(step("5", "0", "10", "1", -1).as_deref(), Some("4"));
        // На границе значение не меняется: GxNumStep не шлёт change.
        assert_eq!(step("10", "0", "10", "1", 1), None);
        assert_eq!(step("0", "0", "10", "1", -1), None);
        // Шаг 0.1 без ошибок двоичных дробей.
        assert_eq!(step("0.1", "0", "1", "0.1", 1).as_deref(), Some("0.2"));
        assert_eq!(step("0.2", "0", "1", "0.1", 1).as_deref(), Some("0.3"));
        assert_eq!(step("0.7", "0", "1", "0.1", 1).as_deref(), Some("0.8"));
        assert_eq!(step("0.8", "0", "1", "0.1", 1).as_deref(), Some("0.9"));
        // Не меньше min и не больше max даже с шагом, не делящим диапазон.
        assert_eq!(step("9", "0", "10", "4", 1).as_deref(), Some("8"));
        assert_eq!(step("8", "0", "10", "4", 1), None);
        // Шаг от min: min 1, шаг 2 → 1, 3, 5.
        assert_eq!(step("1", "1", "", "2", 1).as_deref(), Some("3"));
        assert_eq!(step("5", "1", "", "2", -1).as_deref(), Some("3"));
    }

    #[test]
    fn value_off_the_step_grid_goes_to_the_next_grid_value() {
        // 5.5 при шаге 1 от нуля: вверх — 6, вниз — 5.
        assert_eq!(step("5.5", "0", "10", "1", 1).as_deref(), Some("6"));
        assert_eq!(step("5.5", "0", "10", "1", -1).as_deref(), Some("5"));
        // Ниже min — вверх подтягивается к min, а вниз значение не меняется
        // (шаг 10 ApplyStep: stepDown не может увеличить значение).
        assert_eq!(step("-5", "0", "10", "1", 1).as_deref(), Some("0"));
        assert_eq!(step("-5", "0", "10", "1", -1), None);
        // Выше max — наоборот: вниз — к наибольшему значению сетки, вверх — ничего.
        assert_eq!(step("50", "0", "10", "1", -1).as_deref(), Some("10"));
        assert_eq!(step("50", "0", "10", "1", 1), None);
    }

    #[test]
    fn empty_field_starts_from_min_or_zero() {
        assert_eq!(step("", "", "", "1", 1).as_deref(), Some("1"));
        assert_eq!(step("", "", "", "1", -1).as_deref(), Some("-1"));
        assert_eq!(step("", "5", "10", "1", 1).as_deref(), Some("5"));
        assert_eq!(step("", "5", "10", "1", -1).as_deref(), Some("5"));
        // Пустое поле при max −2: старт max − шаг = −3, шаг вверх — −2.
        assert_eq!(step("", "-10", "-2", "1", 1).as_deref(), Some("-2"));
        // Неправильный текст ведёт себя как пустой.
        assert_eq!(step("abc", "0", "10", "1", 1).as_deref(), Some("1"));
    }

    #[test]
    fn any_step_and_broken_ranges_do_nothing() {
        assert_eq!(step("5", "0", "10", "any", 1), None);
        assert_eq!(step("5", "10", "0", "1", 1), None);
        // Диапазон из одного значения сетки: min 0.5, max 0.9, шаг 1 — шагать некуда.
        assert_eq!(step("0.5", "0.5", "0.9", "1", 1), None);
        // Шаг ≤ 0 или мусор — шаг 1.
        assert_eq!(step("5", "", "", "0", 1).as_deref(), Some("6"));
        assert_eq!(step("5", "", "", "-2", 1).as_deref(), Some("6"));
        assert_eq!(step("5", "", "", "x", 1).as_deref(), Some("6"));
    }

    #[test]
    fn range_value_snaps_to_step_from_min_and_clamps() {
        assert_eq!(range_snap(7.4, 0.0, 10.0, Some(1.0)), 7.0);
        assert_eq!(range_snap(7.5, 0.0, 10.0, Some(1.0)), 8.0);
        assert_eq!(range_snap(-3.0, 0.0, 10.0, Some(1.0)), 0.0);
        assert_eq!(range_snap(99.0, 0.0, 10.0, Some(1.0)), 10.0);
        // Шаг от min: min 1, шаг 2 → 1, 3, 5, 7, 9.
        assert_eq!(range_snap(4.2, 1.0, 10.0, Some(2.0)), 5.0);
        assert_eq!(range_snap(10.0, 1.0, 10.0, Some(2.0)), 9.0);
        // step="any" — без привязки (поля Ghosting сами снимают шаг: snap в 0.4.7).
        assert_eq!(range_snap(7.4321, 0.0, 10.0, None), 7.4321);
        // Дробный шаг без ошибок двоичных дробей.
        assert_eq!(range_snap(0.31, 0.0, 1.0, Some(0.1)), 0.3);
        assert_eq!(range_snap(0.5, 0.0, 1.0, Some(0.01)), 0.5);
        // max < min — диапазон из одного значения.
        assert_eq!(range_snap(5.0, 3.0, 1.0, Some(1.0)), 3.0);
    }

    #[test]
    fn range_keys_follow_blink() {
        assert_eq!(range_key(5.0, 0.0, 100.0, Some(1.0), 0), 6.0);
        assert_eq!(range_key(5.0, 0.0, 100.0, Some(1.0), 1), 4.0);
        assert_eq!(range_key(5.0, 0.0, 100.0, Some(1.0), 2), 15.0);
        assert_eq!(range_key(5.0, 0.0, 100.0, Some(1.0), 3), 0.0);
        assert_eq!(range_key(50.0, 0.0, 100.0, Some(1.0), 4), 0.0);
        assert_eq!(range_key(50.0, 0.0, 100.0, Some(1.0), 5), 100.0);
        // any: шаг — сотая доля, страница — десятая.
        assert_eq!(range_key(10.0, 0.0, 200.0, None, 0), 12.0);
        assert_eq!(range_key(10.0, 0.0, 200.0, None, 2), 30.0);
        // Страница не меньше шага.
        assert_eq!(range_key(0.0, 0.0, 5.0, Some(2.0), 2), 2.0);
    }

    #[test]
    fn maxlength_counts_utf16_units() {
        assert_eq!(truncate_utf16("abcdef", 4), "abcd");
        assert_eq!(truncate_utf16("abc", 4), "abc");
        assert_eq!(truncate_utf16("abc", 0), "abc");
        // Эмодзи — две единицы UTF-16: «a😀» при пределе 2 теряет эмодзи целиком.
        assert_eq!(truncate_utf16("a😀b", 2), "a");
        assert_eq!(truncate_utf16("a😀b", 3), "a😀");
        assert_eq!(truncate_utf16("Привет", 3), "При");
    }

    #[test]
    fn number_input_filter_and_hex() {
        assert_eq!(filter_number_input("a1b2.5-e+"), "12.5-e+");
        assert_eq!(filter_number_input("1 000"), "1000");
        assert_eq!(color_hex(Color::rgb(0, 171, 255)), "#00abff");
        assert_eq!(parse_hex("#00ABFF"), Some(Color::rgb(0, 171, 255)));
        assert_eq!(parse_hex("#00abf"), None);
        assert_eq!(parse_hex("00abff"), None);
        assert_eq!(parse_hex("#00abfg"), None);
    }
}
