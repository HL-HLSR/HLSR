//! Числа JS в текст: `String(x)` и `x.toFixed(n)`.

/// `String(x)` (`Number.prototype.toString()`): кратчайшая запись, которая читается обратно в то
/// же число; `1e+21` и `1e-7` с знаком порядка, `-0` — `0`, `NaN`, `Infinity`, `-Infinity`.
pub fn number_to_string(x: f64) -> String {
    if x.is_nan() {
        "NaN".to_owned()
    } else if x.is_infinite() {
        if x > 0.0 { "Infinity" } else { "-Infinity" }.to_owned()
    } else if x == 0.0 {
        "0".to_owned()
    } else {
        ryu_js::Buffer::new().format_finite(x).to_owned()
    }
}

/// `x.toFixed(digits)` для `digits` от 0 до 100.
///
/// Округление — по точному десятичному значению числа, ровно половина — вверх (к большему
/// `n`), как в стандарте; `format!("{:.N}")` Rust округляет половину к чётному и расходится
/// на числах вроде `0.125` (`toFixed(2)` — `0.13`). Знак `-` ставится у любого `x < 0`, даже
/// если всё округлилось до нуля (`(-1e-7).toFixed(2)` — `-0.00`); `-0` знака не имеет.
/// От 1e21 и для не конечных чисел — `String(x)`.
pub fn to_fixed(x: f64, digits: usize) -> String {
    assert!(digits <= 100, "toFixed: digits от 0 до 100");
    if x.is_nan() || x.abs() >= 1e21 {
        return number_to_string(x);
    }
    let negative = x < 0.0;
    // Точное разложение: у числа с плавающей точкой не больше 1074 знаков после точки.
    let exact = format!("{:.1100}", x.abs());
    let (whole, fraction) = exact.split_once('.').expect("точка есть при точности 1100");
    let round_up = fraction.as_bytes()[digits] >= b'5';
    let mut kept: Vec<u8> = whole.bytes().chain(fraction.bytes().take(digits)).collect();
    if round_up {
        let mut at = kept.len();
        loop {
            if at == 0 {
                kept.insert(0, b'1');
                break;
            }
            at -= 1;
            if kept[at] == b'9' {
                kept[at] = b'0';
            } else {
                kept[at] += 1;
                break;
            }
        }
    }
    let point = kept.len() - digits;
    let mut text = String::with_capacity(kept.len() + 2);
    if negative {
        text.push('-');
    }
    text.push_str(std::str::from_utf8(&kept[..point]).expect("цифры"));
    if digits > 0 {
        text.push('.');
        text.push_str(std::str::from_utf8(&kept[point..]).expect("цифры"));
    }
    text
}
