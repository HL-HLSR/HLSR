//! `Math.max` и `Math.min` для двух чисел: `NaN` в любом из них даёт `NaN` (у `f64::max` он
//! пропускается), а `-0` меньше `+0`.

/// `Math.max(a, b)`.
pub fn max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a == 0.0 && b == 0.0 {
        // Равны по значению: больше тот, у кого знак плюс.
        if a.is_sign_positive() { a } else { b }
    } else {
        a.max(b)
    }
}

/// `Math.min(a, b)`.
pub fn min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a == 0.0 && b == 0.0 {
        if a.is_sign_negative() { a } else { b }
    } else {
        a.min(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nan_wins_unlike_f64_max() {
        assert!(max(f64::NAN, 1.0).is_nan());
        assert!(max(1.0, f64::NAN).is_nan());
        assert!(min(f64::NAN, 1.0).is_nan());
        assert!(min(1.0, f64::NAN).is_nan());
        // Для сравнения: `f64::max` пропускает NaN.
        assert_eq!(f64::NAN.max(1.0), 1.0);
    }

    #[test]
    fn ordinary_numbers() {
        assert_eq!(max(3.0, 30.0), 30.0);
        assert_eq!(min(900.0, 30.0), 30.0);
        assert_eq!(max(f64::NEG_INFINITY, -1.0), -1.0);
        assert_eq!(min(f64::INFINITY, 5.0), 5.0);
    }

    #[test]
    fn negative_zero_is_below_positive_zero() {
        assert!(max(-0.0, 0.0).is_sign_positive());
        assert!(max(0.0, -0.0).is_sign_positive());
        assert!(max(-0.0, -0.0).is_sign_negative());
        assert!(min(-0.0, 0.0).is_sign_negative());
        assert!(min(0.0, -0.0).is_sign_negative());
        assert!(min(0.0, 0.0).is_sign_positive());
    }
}
