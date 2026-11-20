//! Имена файлов от игр на Source и GoldSrc (`shared/ansi-name.js` 0.4.9). Движок держит строки в
//! UTF-8, а файл создаёт ANSI-функцией Windows, поэтому на русской системе (cp1251) запись
//! «спидран_005.run» ложится на диск как «СЃРїРёРґСЂР°РЅ_005.run». Переименовывать файл нельзя:
//! игра ищет его по тому же испорченному имени. Поэтому чиним только то, что видит игрок.
//!
//! shortcut: только cp1251 (русская и украинская Windows); другие кодовые страницы — если игроки
//! пришлют такие имена.

/// Байты 0x80–0xFF в windows-1251 (как `TextDecoder("windows-1251")`: 0x98 — U+0098).
const CP1251_HIGH: [char; 128] = [
    '\u{0402}', '\u{0403}', '\u{201A}', '\u{0453}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{20AC}', '\u{2030}', '\u{0409}', '\u{2039}', '\u{040A}', '\u{040C}', '\u{040B}', '\u{040F}',
    '\u{0452}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{0098}', '\u{2122}', '\u{0459}', '\u{203A}', '\u{045A}', '\u{045C}', '\u{045B}', '\u{045F}',
    '\u{00A0}', '\u{040E}', '\u{045E}', '\u{0408}', '\u{00A4}', '\u{0490}', '\u{00A6}', '\u{00A7}',
    '\u{0401}', '\u{00A9}', '\u{0404}', '\u{00AB}', '\u{00AC}', '\u{00AD}', '\u{00AE}', '\u{0407}',
    '\u{00B0}', '\u{00B1}', '\u{0406}', '\u{0456}', '\u{0491}', '\u{00B5}', '\u{00B6}', '\u{00B7}',
    '\u{0451}', '\u{2116}', '\u{0454}', '\u{00BB}', '\u{0458}', '\u{0405}', '\u{0455}', '\u{0457}',
    '\u{0410}', '\u{0411}', '\u{0412}', '\u{0413}', '\u{0414}', '\u{0415}', '\u{0416}', '\u{0417}',
    '\u{0418}', '\u{0419}', '\u{041A}', '\u{041B}', '\u{041C}', '\u{041D}', '\u{041E}', '\u{041F}',
    '\u{0420}', '\u{0421}', '\u{0422}', '\u{0423}', '\u{0424}', '\u{0425}', '\u{0426}', '\u{0427}',
    '\u{0428}', '\u{0429}', '\u{042A}', '\u{042B}', '\u{042C}', '\u{042D}', '\u{042E}', '\u{042F}',
    '\u{0430}', '\u{0431}', '\u{0432}', '\u{0433}', '\u{0434}', '\u{0435}', '\u{0436}', '\u{0437}',
    '\u{0438}', '\u{0439}', '\u{043A}', '\u{043B}', '\u{043C}', '\u{043D}', '\u{043E}', '\u{043F}',
    '\u{0440}', '\u{0441}', '\u{0442}', '\u{0443}', '\u{0444}', '\u{0445}', '\u{0446}', '\u{0447}',
    '\u{0448}', '\u{0449}', '\u{044A}', '\u{044B}', '\u{044C}', '\u{044D}', '\u{044E}', '\u{044F}',
];

/// `readableName(name)`: имя как его записала игра; не похожее на UTF-8, прочитанный как cp1251, —
/// без изменений.
pub fn readable_name(name: &str) -> String {
    if name.is_ascii() {
        return name.to_owned();
    }
    let mut bytes = Vec::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii() {
            bytes.push(c as u8);
        } else if let Some(at) = CP1251_HIGH.iter().position(|&high| high == c) {
            bytes.push(0x80 + at as u8);
        } else {
            return name.to_owned(); // символа нет в cp1251 — имя настоящее
        }
    }
    // Настоящая кириллица в cp1251 почти никогда не складывается в UTF-8. TextDecoder снимает BOM.
    match String::from_utf8(bytes) {
        Ok(text) => match text.strip_prefix('\u{FEFF}') {
            Some(rest) => rest.to_owned(),
            None => text,
        },
        Err(_) => name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Как игра кладёт имя на диск: байты UTF-8, прочитанные Windows как cp1251.
    fn as_written(name: &str) -> String {
        name.bytes()
            .map(|b| match b {
                0..=0x7F => b as char,
                _ => CP1251_HIGH[(b - 0x80) as usize],
            })
            .collect()
    }

    // ansi-name-selftest.js 0.4.9.
    #[test]
    fn game_names_read_in_russian() {
        for name in [
            "спидран_005.run",
            "Пёс.run",
            "Ёжик.run",
            "призрак Гордона.run",
            "ран №3.dem",
        ] {
            assert_eq!(readable_name(&as_written(name)), name);
        }
        assert_eq!(readable_name("СЃРїРёРґСЂР°РЅ_005.run"), "спидран_005.run");
    }

    #[test]
    fn real_names_stay() {
        for name in [
            "myrun_005.run",
            "спидран_005.run",
            "Пёс.run",
            "Привет мир",
            "日本語.run",
            "",
            "a—b.run",
        ] {
            assert_eq!(readable_name(name), name);
        }
        assert_eq!(readable_name(&as_written("\u{FEFF}x.run")), "x.run");
    }
}
