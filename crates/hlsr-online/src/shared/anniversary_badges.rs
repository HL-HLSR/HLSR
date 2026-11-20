//! Значки годовщины в HLSR (`shared/anniversary-badges.js` 0.4.9): `anniversary-N` — N лет с
//! регистрации профиля (N до 10). Сервер считает их сам по дате регистрации и присылает в `badges`
//! только текущий. Свечение (`GLOW`) и подпись (`labelKey`) нужны только окну — они в
//! `hlsr-ui::controllers::profile`.

/// `IDS`: `anniversary-1` … `anniversary-10`.
pub const IDS: [&str; 10] = [
    "anniversary-1",
    "anniversary-2",
    "anniversary-3",
    "anniversary-4",
    "anniversary-5",
    "anniversary-6",
    "anniversary-7",
    "anniversary-8",
    "anniversary-9",
    "anniversary-10",
];
