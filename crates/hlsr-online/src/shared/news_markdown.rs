//! Тело новости (`shared/news-markdown.js`): тот же разбор, что у гайда, с заголовками и
//! отступами в блоках кода. В 0.4.7 результат — строка HTML для `NewsPost.vue`; здесь — дерево
//! блоков ([`Block`]), окно рисует его само.
//!
//! Набор узлов закрыт: абзацы с переносами, заголовки `h3`–`h6`, списки (`ul`, `ol[start]`),
//! блоки кода, `код`, жирный, курсив. Ссылки — только `https://` (`LinkTarget::Href`); `#id`
//! остаётся ссылкой на скрипт гайда и в ленте новостей инертна (`NewsPost` ловит только
//! `data-href`). Картинки в теле выбрасываются: базы адресов нет.

use super::guide_markdown::{Block, GuideOptions, render as render_guide};

/// `render(body)`.
pub fn render(body: &str) -> Vec<Block> {
    render_guide(
        body,
        &GuideOptions {
            headings: true,
            preserve_code_indent: true,
            ..GuideOptions::default()
        },
    )
}
