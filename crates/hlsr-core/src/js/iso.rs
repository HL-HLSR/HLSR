//! `new Date(ms).toISOString()` для любого момента, который выдерживает `Date` (|ms| ≤ 8,64·10¹⁵):
//! до 1970 года тоже, а год вне 0000–9999 — со знаком и шестью цифрами, как у V8.

/// `new Date(ms).toISOString()`: `2026-10-16T12:00:00.000Z`, `-000001-12-31T23:59:59.999Z`,
/// `+275760-09-13T00:00:00.000Z`.
pub fn iso_from_ms(ms: i64) -> String {
    const DAY_MS: i64 = 86_400_000;
    let days = ms.div_euclid(DAY_MS);
    let time = ms.rem_euclid(DAY_MS);
    let (year, month, day) = civil_from_days(days);
    let year = if (0..=9999).contains(&year) {
        format!("{year:04}")
    } else {
        format!("{}{:06}", if year < 0 { '-' } else { '+' }, year.abs())
    };
    format!(
        "{year}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        time / 3_600_000,
        time / 60_000 % 60,
        time / 1000 % 60,
        time % 1000
    )
}

/// Год, месяц (1–12) и день по номеру дня от 1970-01-01 (пролептический григорианский календарь).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}
