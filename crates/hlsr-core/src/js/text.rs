//! Строки и байты так, как их видит JS: длина в единицах UTF-16 (`string.length`) и шестнадцатеричная
//! запись (`Buffer.toString("hex")`).

/// `string.length`: число единиц UTF-16 (символ выше U+FFFF — две).
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `buffer.toString("hex")`: строчные цифры, по два знака на байт.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0F)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_counts_utf16_units() {
        assert_eq!(utf16_len(""), 0);
        assert_eq!(utf16_len("abc"), 3);
        assert_eq!(utf16_len("привет"), 6);
        // U+1F600 — суррогатная пара, как `"😀".length === 2` в JS.
        assert_eq!(utf16_len("😀"), 2);
        assert_eq!(utf16_len("a😀b"), 4);
    }

    #[test]
    fn hex_is_lowercase_two_digits_per_byte() {
        assert_eq!(hex(&[]), "");
        assert_eq!(hex(&[0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
        let all: Vec<u8> = (0..=255).collect();
        let expected: String = all.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(hex(&all), expected);
    }
}
