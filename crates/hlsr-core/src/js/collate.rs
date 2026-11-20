//! Порядок строк как у `a.localeCompare(b, locale, { sensitivity })` в Node: V8 сравнивает
//! через ICU, здесь — тот же алгоритм (UCA/CLDR) из ICU4X. От языка зависит порядок
//! письменностей: в `ru` кириллица идёт раньше латиницы, в `en-US` и `de` — позже.

use icu_collator::options::{CollatorOptions, Strength};
use icu_collator::{Collator as Icu, CollatorBorrowed, CollatorPreferences};
use icu_locale_core::Locale;
use std::cmp::Ordering;

/// Язык по умолчанию процесса в Linux так, как его выбирает ICU в main-процессе Electron 0.4.7, а
/// значит, и V8 для `localeCompare` без языка. Chromium в самом начале вызывает
/// `setlocale(LC_ALL, "")`, а glibc пропускает пустые переменные, поэтому:
///
/// - первая **непустая** из `LC_ALL`, `LC_MESSAGES`, `LANG` задаёт язык, если она не `C` и не
///   `POSIX` (`C.UTF-8` — обычная локаль, она даёт `en-US`); заданная, но пустая переменная
///   (так её оставляет, например, fish) ничего не значит: `LC_ALL=` + `LANG=ru_RU.UTF-8` — `ru-RU`;
/// - непустой нет или первая непустая — `C`/`POSIX`: glibc отвечает `C`, и ICU читает окружение
///   напрямую — первая **заданная** из тех же трёх; пустая даёт корневой язык `und` (порядок
///   тот же, что у `en-US`), `C`, `POSIX` и «ничего не задано» — `en-US`;
/// - `LANGUAGE`, `LC_CTYPE` и `LC_COLLATE` не читаются.
///
/// Кодировка (`.UTF-8`) и модификатор (`@euro`) отбрасываются, `_` → `-`. Отличается от
/// `sys_locale` (то же, что `app.getLocale()`): тот сначала читает `LANGUAGE`. Простой Node
/// (`ELECTRON_RUN_AS_NODE`) `setlocale` не вызывает и читает окружение напрямую: пустая переменная
/// даёт у него `und` — это не тот процесс, поведение которого повторяется.
///
/// Не моделируется: выбор рассчитан на то, что локаль из переменной установлена в системе. Если её
/// нет (`setlocale` не удался), Electron читает окружение напрямую, как во втором пункте:
/// `LC_ALL=` + `LANG=de_DE.UTF-8` при отсутствующей de_DE даёт `und`, а не `de-DE` (без пустой
/// переменной перед ней результат тот же, `de-DE`). Список установленных локалей (`locale -a`)
/// не читается. `env` — чтение переменной окружения (для проверок подменяется).
pub fn posix_locale(env: impl Fn(&str) -> Option<String>) -> String {
    const VARIABLES: [&str; 3] = ["LC_ALL", "LC_MESSAGES", "LANG"];
    let chosen = VARIABLES
        .into_iter()
        .filter_map(&env)
        .find(|id| !id.is_empty());
    let id = match chosen {
        Some(id) if id != "C" && id != "POSIX" => id,
        _ => match VARIABLES.into_iter().find_map(&env) {
            Some(id) => id,
            None => return "en-US".to_owned(),
        },
    };
    let name = id.split(['.', '@']).next().unwrap_or_default();
    match name {
        "C" | "POSIX" => "en-US".to_owned(),
        "" => "und".to_owned(),
        _ if name.starts_with('_') => format!("und{}", name.replace('_', "-")),
        _ => name.replace('_', "-"),
    }
}

/// Язык по умолчанию процесса: в Linux по окружению ([`posix_locale`]), в Windows — язык
/// форматов (`GetUserDefaultLocaleName`), как ICU Node.
pub fn default_locale() -> String {
    #[cfg(windows)]
    {
        windows_locale().unwrap_or_else(|| "en-US".to_owned())
    }
    #[cfg(not(windows))]
    {
        posix_locale(|name| std::env::var(name).ok())
    }
}

#[cfg(windows)]
fn windows_locale() -> Option<String> {
    use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
    // LOCALE_NAME_MAX_LENGTH
    let mut buffer = [0u16; 85];
    // SAFETY: буфер на 85 единиц UTF-16, длина передана.
    let length = unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), 85) };
    // Длина считается с конечным нулём.
    let length = usize::try_from(length).ok().filter(|n| *n > 1)?;
    Some(String::from_utf16_lossy(&buffer[..length - 1]))
}

/// `sensitivity` из `Intl.Collator`: что различает сравнение.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sensitivity {
    /// `"base"`: только буквы (`a` = `A` = `á`).
    Base,
    /// `"variant"` — умолчание `localeCompare`: буквы, ударения и регистр.
    Variant,
}

/// Сравнение строк для одного языка и одной чувствительности.
pub struct Collator {
    inner: CollatorBorrowed<'static>,
}

impl Collator {
    /// Для языка BCP 47 (`ru-RU`, `en-US`). Непонятный язык (`C`, `POSIX`, пусто) — как
    /// корневой: ICU Node в таком случае берёт `en-US`, порядок латиницы у них одинаков.
    pub fn new(locale: &str, sensitivity: Sensitivity) -> Collator {
        let locale: Locale = locale.parse().unwrap_or(Locale::UNKNOWN);
        let mut options = CollatorOptions::default();
        options.strength = Some(match sensitivity {
            Sensitivity::Base => Strength::Primary,
            Sensitivity::Variant => Strength::Tertiary,
        });
        let inner = Icu::try_new(CollatorPreferences::from(&locale), options)
            .expect("данные сравнения вшиты в сборку");
        Collator { inner }
    }

    /// `a.localeCompare(b, locale, { sensitivity })`: знак результата.
    pub fn compare(&self, a: &str, b: &str) -> Ordering {
        self.inner.compare(a, b)
    }
}
