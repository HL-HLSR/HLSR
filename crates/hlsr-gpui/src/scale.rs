//! Масштаб: всё в rem. Electron 0.4.9 раскладывает интерфейс в CSS px макета 1600×900 и
//! увеличивает страницу (`zoomFactor = uiScale × layoutZoom`, в полном экране `fillZoom`).
//! Здесь тот же множитель — размер rem окна: `rem = 16 × zoom`, а все размеры — `u(css_px)`.
//!
//! `px()` — только линии 1 px и радиус окна (в 0.4.9 они вне зума, `App.vue`). Что берёт
//! `Pixels` (`BoxShadow`, `paint_*`, пружины), умножается на зум через [`zpx`]. Тест ниже ловит
//! `px(` вне этого правила.

use gpui::{Pixels, Rems, Window, px, rems};
use hlsr_core::window_size::{self, Layout, Size};

/// Радиус окна (`#app`), px вне зума.
pub const WINDOW_RADIUS: f32 = 16.;

/// CSS px макета → rem (16 px при зуме 1).
pub fn u(css_px: f32) -> Rems {
    rems(css_px / 16.)
}

/// Зум окна: rem / 16.
pub fn zoom(window: &Window) -> f32 {
    f32::from(window.rem_size()) / 16.
}

/// CSS px макета → `Pixels` окна для того, что не принимает rem (тени, рисование, пружины).
pub fn zpx(zoom: f32, css_px: f32) -> Pixels {
    px(css_px * zoom)
}

/// Размер rem для зума страницы.
pub fn rem_size(zoom: f64) -> Pixels {
    px(16. * zoom as f32)
}

/// Окно лаунчера для рабочей области `area` (логические px) и масштаба интерфейса `ui_scale`.
pub fn window_layout(area: Size, ui_scale: f64) -> Layout {
    window_size::scale_layout(window_size::compute_window_size(area), area, ui_scale)
}

/// CSS-окно (viewport) страницы: размер окна в px, делённый на зум. Ступени сетки темы
/// (`@media`) считаются по нему.
pub fn viewport(window: &Window) -> (f32, f32) {
    let size = window.viewport_size();
    let zoom = zoom(window);
    (f32::from(size.width) / zoom, f32::from(size.height) / zoom)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// Файлы, где `px(` — сама точка перевода CSS px в `Pixels` или вне раскладки.
    const PX_FILES: &[&str] = &["scale.rs", "text.rs", "paint.rs", "smoke.rs", "main.rs"];
    /// Что можно писать через `px(` везде: линия 1 px, ноль, радиус окна.
    const PX_ALLOWED: &[&str] = &["px(0.)", "px(1.)", "px(WINDOW_RADIUS)"];

    #[test]
    fn sizes_go_through_rem() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        scan(&root, &root, &mut found);
        assert!(
            found.is_empty(),
            "размеры — через u()/zpx() (scale.rs), не px():\n{}",
            found.join("\n")
        );
    }

    fn scan(root: &Path, dir: &Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                scan(root, &path, found);
                continue;
            }
            let name = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if !name.ends_with(".rs") || PX_FILES.contains(&name.as_str()) {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for (number, line) in text.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                if bare_px(line) {
                    found.push(format!("{name}:{}: {}", number + 1, line.trim()));
                }
            }
        }
    }

    /// Есть ли в строке `px(…)` (не `zpx(`, не `.px(`), кроме разрешённых.
    fn bare_px(line: &str) -> bool {
        let mut rest = line;
        let mut offset = 0;
        while let Some(at) = rest.find("px(") {
            let start = offset + at;
            let before = line[..start].chars().next_back();
            let word = before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.');
            if !word && !PX_ALLOWED.iter().any(|ok| line[start..].starts_with(ok)) {
                return true;
            }
            offset = start + 3;
            rest = &line[offset..];
        }
        false
    }

    #[test]
    fn lint_sees_bare_px_only() {
        assert!(bare_px("div().w(px(12.))"));
        assert!(bare_px("let a = px(2.);"));
        assert!(!bare_px("div().h(px(1.))"));
        assert!(!bare_px(".rounded(px(WINDOW_RADIUS))"));
        assert!(!bare_px("zpx(zoom, 12.)"));
        assert!(!bare_px("s.px(u(16.))"));
        assert!(!bare_px("gpui::px"));
    }
}
