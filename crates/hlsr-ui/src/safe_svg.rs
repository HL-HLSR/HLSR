//! SVG своих значков из админки без внешних ресурсов (итоговая проверка, находка 15).
//!
//! В 0.4.7 иконку рисовал `<img>` (или CSS-маска) с `data:`-адресом, и Chromium в такой картинке не
//! грузит ничего снаружи документа: ни сеть, ни файлы; картинки-`data:` внутри SVG показывает.
//! usvg разбирает SVG с обычным разрешением ссылок: у `<image>` и `<feImage>` всё, что не
//! разобралось как `data:`-адрес, он открывает как путь к файлу (в Windows и `\\сервер\ресурс` — обращение к сети).
//!
//! Поэтому перед загрузкой у `image`/`feImage` убираются все `href` (с любым префиксом пространства
//! имён), кроме `data:`-адресов. Элемент без ссылки usvg пропускает — значок рисуется без картинки,
//! как в Chromium. Документ, который не разобрался или в котором после правки осталась внешняя
//! ссылка, не рисуется вовсе. Ссылка `feImage` на элемент документа (`#id`) убирается тоже: проверять,
//! найдёт ли usvg этот элемент, дороже, чем стоит значок с такой экзотикой.

use crate::Rgba8Image;
use resvg::{tiny_skia, usvg};
use roxmltree::{Document, ParsingOptions};
use std::borrow::Cow;
use std::ops::Range;

/// SVG значка в предумноженную картинку `width × height` физических px (растянут на неё); `None`
/// — это не SVG, документ не разобрался, в нём остался бы внешний ресурс или размер пуст.
pub fn load_svg(bytes: &[u8], width: u32, height: u32) -> Option<Rgba8Image> {
    // usvg сам разжимает gzip: сжатые байты сюда не пускаем, они не UTF-8 и до разбора не дойдут.
    let text = std::str::from_utf8(bytes).ok()?;
    let safe = without_external_resources(text)?;
    render(safe.as_bytes(), width, height)
}

/// Растр SVG без проверок (тесты сверяют им [`load_svg`]).
#[doc(hidden)]
pub fn render(svg: &[u8], width: u32, height: u32) -> Option<Rgba8Image> {
    let tree = usvg::Tree::from_data(svg, &usvg::Options::default()).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
    let size = tree.size();
    let transform = tiny_skia::Transform::from_scale(
        width as f32 / size.width(),
        height as f32 / size.height(),
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Some(Rgba8Image::from_bytes(width, height, pixmap.data()).premultiplied())
}

/// Документ без `href` у `image`/`feImage`, кроме `data:`; без таких ссылок — тот же текст.
pub fn without_external_resources(svg: &str) -> Option<Cow<'_, str>> {
    let doc = parse(svg)?;
    let mut cuts = external_hrefs(&doc);
    if cuts.is_empty() {
        return Some(Cow::Borrowed(svg));
    }
    cuts.sort_by_key(|range| range.start);
    let mut out = String::with_capacity(svg.len());
    let mut at = 0;
    for cut in cuts {
        if cut.start < at
            || cut.end > svg.len()
            || !svg.is_char_boundary(cut.start)
            || !svg.is_char_boundary(cut.end)
        {
            return None;
        }
        out.push_str(&svg[at..cut.start]);
        at = cut.end;
    }
    out.push_str(&svg[at..]);
    // `Attribute::range()` точен (предел u16 — только у `range_qname`/`range_value`, их не берём);
    // повторный разбор — страховка: в usvg уходит только текст, в котором самом не нашлось внешних
    // ссылок, а не тот, что лишь вычислен вырезами.
    let clean = parse(&out).is_some_and(|doc| external_hrefs(&doc).is_empty());
    clean.then_some(Cow::Owned(out))
}

/// Разбор как у usvg, но без DTD: сущности меняют текст и сдвигают позиции, а в значке из админки
/// (`<svg` в начале строки) DTD быть не может.
fn parse(svg: &str) -> Option<Document<'_>> {
    let options = ParsingOptions {
        allow_dtd: false,
        ..ParsingOptions::default()
    };
    Document::parse_with_options(svg, options).ok()
}

/// Места `href` у `image`/`feImage`, которые не `data:`-адрес.
fn external_hrefs(doc: &Document<'_>) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    for node in doc.descendants().filter(roxmltree::Node::is_element) {
        let name = node.tag_name().name();
        if !name.eq_ignore_ascii_case("image") && !name.eq_ignore_ascii_case("feImage") {
            continue;
        }
        for attribute in node.attributes() {
            if attribute.name().eq_ignore_ascii_case("href") && !is_data_url(attribute.value()) {
                found.push(attribute.range());
            }
        }
    }
    found
}

/// `data:` в начале и запятая раньше `#`: ровно то, что usvg (`data-url`) разбирает как data-URL, а
/// не как путь к файлу. Строже не бывает: пробелы и управляющие знаки в начале не прощаются.
fn is_data_url(href: &str) -> bool {
    let bytes = href.as_bytes();
    bytes.len() > 5
        && bytes[..5].eq_ignore_ascii_case(b"data:")
        && bytes[5..]
            .iter()
            .find(|byte| matches!(byte, b',' | b'#'))
            .is_some_and(|byte| *byte == b',')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_urls_are_only_what_usvg_treats_as_data() {
        assert!(is_data_url("data:image/png;base64,AAAA"));
        assert!(is_data_url("DATA:,x"));
        assert!(is_data_url("data:image/svg+xml,%3Csvg/%3E#frag"));
        // usvg без запятой до `#` считает адрес путём к файлу.
        assert!(!is_data_url("data:"));
        assert!(!is_data_url("data:image/png;base64"));
        assert!(!is_data_url("data:#,"));
        assert!(!is_data_url(" data:,x"));
        assert!(!is_data_url("\tdata:,x"));
        assert!(!is_data_url("file:///C:/secret.png"));
        assert!(!is_data_url("//server/share/a.png"));
        assert!(!is_data_url("secret.png"));
        assert!(!is_data_url(""));
    }

    #[test]
    fn an_icon_without_images_is_the_same_text() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8"><use href="#a"/><rect id="a" width="8" height="8"/></svg>"##;
        assert!(matches!(
            without_external_resources(svg),
            Some(Cow::Borrowed(same)) if same == svg
        ));
    }

    #[test]
    fn external_references_are_cut_and_data_ones_stay() {
        let svg = concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:svg="http://www.w3.org/2000/svg" "#,
            r#"xmlns:x="http://www.w3.org/1999/xlink">"#,
            r#"<image width="8" height="8" href="file:///C:/secret.png"/>"#,
            r#"<image x:href='\\server\share\c.png' width="8"/>"#,
            r#"<svg:image x:href="a.png"/>"#,
            r#"<filter id="f"><feImage href="../b.png"/></filter>"#,
            r#"<image href="data:image/png;base64,AAAA" width="8" height="8"/>"#,
            r#"<a href="https://example.com/"><rect width="1" height="1"/></a>"#,
            r#"</svg>"#,
        );
        let safe = without_external_resources(svg).unwrap();
        assert!(!safe.contains("secret.png"));
        assert!(!safe.contains("server"));
        assert!(!safe.contains("c.png"));
        assert!(!safe.contains("a.png"));
        assert!(!safe.contains("b.png"));
        assert!(safe.contains(r#"href="data:image/png;base64,AAAA""#));
        // Ссылки у других элементов не трогаются: сети и файлов они не читают.
        assert!(safe.contains(r#"<a href="https://example.com/">"#));
        assert!(safe.contains(r#"<image width="8" height="8" />"#));
    }

    #[test]
    fn references_hidden_in_entities_or_dtd_are_rejected() {
        // Символьная ссылка в значении видна разобранной, и атрибут вырезается целиком.
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg"><image href="&#102;ile:///C:/x.png"/></svg>"#;
        let safe = without_external_resources(svg).unwrap();
        assert!(!safe.contains("x.png"));
        // DTD со своими сущностями не принимается: позиции в нём не совпадают с текстом.
        let dtd =
            r#"<!DOCTYPE svg [<!ENTITY a "x">]><svg xmlns="http://www.w3.org/2000/svg">&a;</svg>"#;
        assert!(without_external_resources(dtd).is_none());
        assert!(without_external_resources("<svg").is_none());
        assert!(without_external_resources("").is_none());
    }

    #[test]
    fn compressed_and_non_utf8_bytes_are_not_loaded() {
        // Начало gzip: usvg его бы разжал и прочитал `<image>` в обход проверки.
        assert!(load_svg(&[0x1f, 0x8b, 0x08, 0x00, 0x00], 8, 8).is_none());
        assert!(load_svg(b"\xff\xfe<svg/>", 8, 8).is_none());
    }
}
