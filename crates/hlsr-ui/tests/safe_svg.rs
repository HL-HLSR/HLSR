//! SVG своих значков (`hlsr_ui::safe_svg`): локальный файл из `<image href>` не читается, `data:`-картинка
//! внутри SVG рисуется, как в `<img>` Chromium (итоговая проверка, находка 15).
//!
//! Проверка — по настоящим пикселям: тот же SVG через `safe_svg::render` (usvg без защиты) рисует
//! картинку из файла, через `load_svg` — нет.

use base64::Engine as _;
use hlsr_ui::Rgba8Image;
use hlsr_ui::safe_svg::{load_svg, render};
use image::{ImageFormat, Rgba, RgbaImage};
use std::io::Cursor;
use std::path::PathBuf;

/// Своя временная папка теста (у `hlsr-ui` нет `tempfile`).
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hlsr-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Красный PNG 4×4.
fn red_png() -> Vec<u8> {
    let mut bytes = Vec::new();
    RgbaImage::from_pixel(4, 4, Rgba([255, 0, 0, 255]))
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .unwrap();
    bytes
}

fn svg_with(image_tag: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="8" height="8" viewBox="0 0 8 8">{image_tag}</svg>"#
    )
}

/// Сумма альфа-канала по всем пикселям: 0 — ничего не нарисовано.
fn painted(image: &Rgba8Image) -> u32 {
    image
        .as_slice()
        .iter()
        .map(|pixel| u32::from(pixel.a))
        .sum()
}

/// Без защиты, тем же растром.
fn unguarded(svg: &str) -> Rgba8Image {
    render(svg.as_bytes(), 8, 8).unwrap()
}

fn guarded(svg: &str) -> Option<Rgba8Image> {
    load_svg(svg.as_bytes(), 8, 8)
}

#[test]
fn a_local_file_in_image_href_is_not_read() {
    let dir = temp_dir("safe-svg-file");
    let file = dir.join("secret.png");
    std::fs::write(&file, red_png()).unwrap();
    let path = file.to_str().unwrap();
    let svg = svg_with(&format!(
        r#"<image href="{path}" width="8" height="8"/><image xlink:href="{path}" width="8" height="8"/>"#
    ));

    // Без защиты usvg открывает путь как файл: проверка проверяет то, от чего защищает.
    assert!(
        painted(&unguarded(&svg)) > 0,
        "usvg больше не читает файл из <image href>: пересмотреть safe_svg"
    );

    let guarded = guarded(&svg).expect("значок без внешнего ресурса рисуется");
    assert_eq!(painted(&guarded), 0, "файл из <image href> прочитан");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn file_urls_and_feimage_files_are_not_read_either() {
    let dir = temp_dir("safe-svg-fe");
    let file = dir.join("secret.png");
    std::fs::write(&file, red_png()).unwrap();
    let path = file.to_str().unwrap();
    let url = format!("file:///{}", path.replace('\\', "/"));
    let svg = svg_with(&format!(
        r#"<filter id="f" x="0" y="0" width="1" height="1"><feImage href="{path}"/></filter>
<rect width="8" height="8" fill="none" filter="url(#f)"/>
<image href="{url}" width="8" height="8"/>"#
    ));

    let unguarded = unguarded(&svg);
    let guarded = guarded(&svg).expect("значок без внешнего ресурса рисуется");
    // feImage с файлом — единственное, что здесь рисуется без защиты; после защиты — ничего.
    assert!(
        painted(&unguarded) > 0,
        "feImage с файлом больше не рисуется без защиты"
    );
    assert_eq!(painted(&guarded), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_data_image_inside_the_svg_is_still_drawn() {
    let png = base64::engine::general_purpose::STANDARD.encode(red_png());
    let svg = svg_with(&format!(
        r#"<image href="data:image/png;base64,{png}" width="8" height="8"/>"#
    ));
    let pixels = guarded(&svg).expect("значок с data:-картинкой рисуется");
    assert_eq!((pixels.width(), pixels.height()), (8, 8));
    assert!(
        pixels
            .as_slice()
            .iter()
            .all(|pixel| (pixel.r, pixel.g, pixel.b, pixel.a) == (255, 0, 0, 255)),
        "data:-картинка внутри SVG должна занять весь значок"
    );
    // То же, что без защиты: чистка ничего не меняет у тех, кто и так безопасен.
    assert_eq!(unguarded(&svg), pixels);
}

#[test]
fn a_plain_icon_is_drawn_as_before() {
    let svg = svg_with(r##"<path d="M0 0H8V8H0Z" fill="#0f0"/>"##);
    let guarded = guarded(&svg).unwrap();
    assert_eq!(guarded, unguarded(&svg));
    assert!(painted(&guarded) > 0);
}
