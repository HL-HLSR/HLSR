//! Markdown новостей и описаний гайда → документ [`RichText`](crate::richtext)
//! (решение 4 спецификации ПП5). Разбор — общий с ПП4:
//! `hlsr_online::shared::guide_markdown` (перенос `shared/guide-markdown.js`,
//! сверен с Node), уже с картинками при `imageBase` и ссылками `data-script`;
//! здесь дерево блоков переводится в блоки и строчные стили раскладки.
//!
//! Места 0.4.7: тело новости — `news-markdown.js` (`NewsPost.vue`: заголовки,
//! отступы в коде), описание скрипта — `markdown.render(usage, MD_OPTIONS)`,
//! заметки — `markdown.inline(note, MD_OPTIONS)` (`GameScripts.vue:280-285`).

use crate::richtext::{Block, Doc, Inline, Link, List, Style};
use hlsr_online::shared::guide_markdown::{
    self as md, GuideOptions, ItemPart, LinkTarget, inline as parse_inline, render,
};
use std::collections::HashSet;

/// `options` разбора (`{headings, preserveCodeIndent, imageBase, scriptIds}`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    pub headings: bool,
    pub preserve_code_indent: bool,
    pub image_base: Option<String>,
    /// Известные скрипты гайда: ссылка `#id` на другой остаётся только для них.
    pub script_ids: Option<HashSet<String>>,
}

impl Options {
    /// `news-markdown.js`: заголовки и отступы в блоках кода, без картинок.
    pub fn news() -> Options {
        Options {
            headings: true,
            preserve_code_indent: true,
            ..Options::default()
        }
    }

    fn guide(&self) -> GuideOptions {
        GuideOptions {
            headings: self.headings,
            preserve_code_indent: self.preserve_code_indent,
            image_base: self.image_base.clone(),
            script_ids: self.script_ids.clone(),
        }
    }
}

/// Тело новости.
pub fn news(body: &str) -> Doc {
    doc(body, &Options::news())
}

/// `render(markdown, options)` → документ.
pub fn doc(markdown: &str, options: &Options) -> Doc {
    let mut doc = Doc::default();
    let blocks = render(markdown, &options.guide());
    doc.blocks = blocks
        .iter()
        .map(|block| convert_block(block, &mut doc.links))
        .collect();
    doc
}

/// `inline(markdown, options)` (заметка гайда) → документ из одной фразы.
pub fn inline_doc(markdown: &str, options: &Options) -> Doc {
    let mut doc = Doc::default();
    let mut nodes = Vec::new();
    convert_inlines(
        &parse_inline(markdown, &options.guide()),
        Style::default(),
        &mut doc.links,
        &mut nodes,
    );
    doc.blocks.push(Block::Inline(nodes));
    doc
}

fn convert_block(block: &md::Block, links: &mut Vec<Link>) -> Block {
    match block {
        md::Block::Paragraph(content) => {
            let mut nodes = Vec::new();
            convert_inlines(content, Style::default(), links, &mut nodes);
            Block::Paragraph(nodes)
        }
        md::Block::Heading { level, content } => {
            let mut nodes = Vec::new();
            convert_inlines(content, Style::default(), links, &mut nodes);
            Block::Heading(*level, nodes)
        }
        md::Block::List(list) => Block::List(convert_list(list, links)),
        md::Block::Code(code) => Block::Code(code.clone()),
    }
}

/// Пункт `<li>`: строчное содержимое подряд — анонимный блок, вложенный
/// список — свой блок.
fn convert_list(list: &md::List, links: &mut Vec<Link>) -> List {
    let items = list
        .items
        .iter()
        .map(|item| {
            let mut blocks = Vec::new();
            let mut nodes = Vec::new();
            for part in item {
                match part {
                    ItemPart::Inline(node) => {
                        convert_inlines(
                            std::slice::from_ref(node),
                            Style::default(),
                            links,
                            &mut nodes,
                        );
                    }
                    ItemPart::List(nested) => {
                        if !nodes.is_empty() {
                            blocks.push(Block::Inline(std::mem::take(&mut nodes)));
                        }
                        blocks.push(Block::List(convert_list(nested, links)));
                    }
                }
            }
            if !nodes.is_empty() {
                blocks.push(Block::Inline(nodes));
            }
            blocks
        })
        .collect();
    List {
        ordered: list.ordered,
        start: list.start,
        items,
    }
}

fn convert_inlines(
    nodes: &[md::Inline],
    style: Style,
    links: &mut Vec<Link>,
    out: &mut Vec<Inline>,
) {
    for node in nodes {
        match node {
            md::Inline::Text(text) => out.push(Inline::Text {
                text: text.clone(),
                style,
            }),
            md::Inline::Code(code) => out.push(Inline::Text {
                text: code.clone(),
                style: Style {
                    code: true,
                    ..style
                },
            }),
            md::Inline::Strong(inner) => convert_inlines(
                inner,
                Style {
                    strong: true,
                    ..style
                },
                links,
                out,
            ),
            md::Inline::Emphasis(inner) => {
                convert_inlines(inner, Style { em: true, ..style }, links, out)
            }
            md::Inline::Link { target, label } => {
                links.push(match target {
                    LinkTarget::Script(id) => Link::Script(id.clone()),
                    LinkTarget::Href(href) => Link::Href(href.clone()),
                });
                let link = Some((links.len() - 1) as u32);
                convert_inlines(label, Style { link, ..style }, links, out);
            }
            md::Inline::Image { src, alt } => out.push(Inline::Image {
                src: src.clone(),
                alt: alt.clone(),
            }),
            md::Inline::LineBreak => out.push(Inline::Break),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_nest_and_links_are_numbered() {
        let doc = doc(
            "**жирный [ссылка](https://example.org) `код`** и *курсив* [скрипт](#other)",
            &Options::default(),
        );
        assert_eq!(
            doc.links,
            [
                Link::Href("https://example.org".into()),
                Link::Script("other".into())
            ]
        );
        let Block::Paragraph(nodes) = &doc.blocks[0] else {
            panic!("{doc:?}");
        };
        let style_of = |needle: &str| {
            nodes
                .iter()
                .find_map(|node| match node {
                    Inline::Text { text, style } if text == needle => Some(*style),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{needle}: {nodes:?}"))
        };
        assert!(style_of("ссылка").strong && style_of("ссылка").link == Some(0));
        assert!(style_of("код").code && style_of("код").strong);
        assert!(style_of("курсив").em);
        assert_eq!(style_of("скрипт").link, Some(1));
    }

    #[test]
    fn list_items_split_text_and_nested_lists() {
        let doc = doc("- один\n  - вложенный\n- два", &Options::default());
        let Block::List(list) = &doc.blocks[0] else {
            panic!("{doc:?}");
        };
        assert_eq!(list.items.len(), 2);
        assert!(matches!(list.items[0][0], Block::Inline(_)));
        assert!(matches!(list.items[0][1], Block::List(_)));
    }

    #[test]
    fn images_only_with_image_base() {
        let with = Options {
            image_base: Some("https://raw/base".into()),
            ..Options::default()
        };
        let doc_with = doc("текст ![кадр](img/a.png)", &with);
        let doc_without = doc("текст ![кадр](img/a.png)", &Options::default());
        let has_image = |doc: &Doc| {
            doc.blocks.iter().any(|block| {
                matches!(block, Block::Paragraph(nodes) if nodes.iter().any(|n| matches!(n, Inline::Image { src, .. } if src == "https://raw/base/img/a.png")))
            })
        };
        assert!(has_image(&doc_with));
        assert!(!has_image(&doc_without));
    }
}
