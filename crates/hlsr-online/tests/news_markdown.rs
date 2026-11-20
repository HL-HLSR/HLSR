//! Markdown новостей и гайда против 0.4.7: фикстура `news-markdown.json` из
//! `tools/reference/online/news-markdown.js`. Node (`shared/news-markdown.js`,
//! `shared/guide-markdown.js`) отдаёт строку HTML; Rust разбирает Markdown в дерево блоков, тест
//! печатает дерево в тот же HTML (`to_html`) и сравнивает со строкой Node. Корпус — выборки
//! самотестов 0.4.7 и случаи на границы автомата (списки, отступы, ограды кода, просмотр назад в
//! курсиве, пробелы Unicode, перевод строки CR и U+2028).

mod support;

use hlsr_online::shared::guide_markdown::{
    self, Block, GuideOptions, Inline, LinkTarget, escape_html, inline, inline_html, to_html,
};
use hlsr_online::shared::news_markdown;
use serde_json::Value;
use std::collections::HashSet;
use support::{fixture, items, str_of};

fn options_of(spec: &Value) -> GuideOptions {
    GuideOptions {
        headings: spec["headings"].as_bool().unwrap_or(false),
        preserve_code_indent: spec["preserveCodeIndent"].as_bool().unwrap_or(false),
        image_base: spec["imageBase"].as_str().map(str::to_owned),
        script_ids: spec["scriptIds"].as_array().map(|ids| {
            ids.iter()
                .map(|id| str_of(id).to_owned())
                .collect::<HashSet<_>>()
        }),
    }
}

/// Собрать расхождения: текст разницы по каждому случаю.
fn report(what: &str, failures: Vec<String>) {
    assert!(
        failures.is_empty(),
        "{what}: {} расхождений с 0.4.7 (первые 8):\n\n{}",
        failures.len(),
        failures
            .iter()
            .take(8)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n")
    );
}

#[test]
fn news_render_matches_node() {
    let fixture = fixture("news-markdown.json");
    let docs = items(&fixture["docs"]);
    assert!(docs.len() > 300);
    let mut failures = Vec::new();
    for doc in docs {
        let markdown = str_of(&doc["markdown"]);
        let html = to_html(&news_markdown::render(markdown));
        if html != str_of(&doc["news"]) {
            failures.push(format!(
                "новости {markdown:?}\n  Node: {}\n  Rust: {html}",
                doc["news"]
            ));
        }
    }
    report("news-markdown.render", failures);
}

#[test]
fn guide_render_matches_node() {
    let fixture = fixture("news-markdown.json");
    let options: Vec<GuideOptions> = items(&fixture["options"]).iter().map(options_of).collect();
    let mut failures = Vec::new();
    for doc in items(&fixture["docs"]) {
        let markdown = str_of(&doc["markdown"]);
        for (at, options) in options.iter().enumerate() {
            let html = to_html(&guide_markdown::render(markdown, options));
            let expected = str_of(&doc["guide"][at]);
            if html != expected {
                failures.push(format!(
                    "гайд[{at}] {markdown:?}\n  Node: {expected}\n  Rust: {html}"
                ));
            }
        }
    }
    report("guide-markdown.render", failures);
}

#[test]
fn inline_matches_node() {
    let fixture = fixture("news-markdown.json");
    let options: Vec<GuideOptions> = items(&fixture["options"]).iter().map(options_of).collect();
    let cases = items(&fixture["inline"]);
    assert!(cases.len() > 60);
    let mut failures = Vec::new();
    for case in cases {
        let text = str_of(&case["text"]);
        for (at, options) in options.iter().enumerate() {
            let html = inline_html(&inline(text, options));
            let expected = str_of(&case["html"][at]);
            if html != expected {
                failures.push(format!(
                    "inline[{at}] {text:?}\n  Node: {expected}\n  Rust: {html}"
                ));
            }
        }
    }
    report("guide-markdown.inline", failures);
}

#[test]
fn escape_html_matches_node() {
    let fixture = fixture("news-markdown.json");
    for case in items(&fixture["escapes"]) {
        assert_eq!(escape_html(str_of(&case["text"])), str_of(&case["html"]));
    }
}

#[test]
fn news_tree_has_closed_set_of_nodes() {
    // Тело новости из самотеста 0.4.7: узлы дерева, а не HTML.
    let blocks = news_markdown::render(
        "# Заголовок\n\n**Жирный** и *курсив* `код`\n\n- один\n- два\n\n[Сайт](https://example.com) [плохой](javascript:alert)\n\n<img src=x onerror=evil()>\n\n![img](https://evil.example/x.png)\n\n```\n  код\n```",
    );
    assert!(matches!(&blocks[0], Block::Heading { level: 3, .. }));
    let Block::Paragraph(content) = &blocks[1] else {
        panic!("абзац: {:?}", blocks[1]);
    };
    assert!(content.iter().any(|node| matches!(node, Inline::Strong(_))));
    assert!(
        content
            .iter()
            .any(|node| matches!(node, Inline::Emphasis(_)))
    );
    assert!(
        content
            .iter()
            .any(|node| matches!(node, Inline::Code(code) if code == "код"))
    );
    let Block::List(list) = &blocks[2] else {
        panic!("список: {:?}", blocks[2]);
    };
    assert!(!list.ordered && list.items.len() == 2);
    // Только https-ссылка остаётся ссылкой; «плохая» — текст.
    let Block::Paragraph(links) = &blocks[3] else {
        panic!("ссылки: {:?}", blocks[3]);
    };
    assert!(links.iter().any(|node| matches!(
        node,
        Inline::Link { target: LinkTarget::Href(href), .. } if href == "https://example.com"
    )));
    assert!(!to_html(&blocks).contains("javascript"));
    // Сырой HTML остаётся текстом, картинок нет (базы адресов нет), код с отступом.
    assert_eq!(
        blocks[4],
        Block::Paragraph(vec![Inline::Text("<img src=x onerror=evil()>".to_owned())])
    );
    assert_eq!(blocks[5], Block::Paragraph(Vec::new()));
    assert_eq!(blocks[6], Block::Code("  код".to_owned()));
    assert_eq!(blocks.len(), 7);
}

#[test]
fn guide_tree_keeps_script_links_and_images_from_the_guide_repository() {
    let options = GuideOptions {
        script_ids: Some(HashSet::from(["jump".to_owned()])),
        image_base: Some("https://example.com/guide".to_owned()),
        ..GuideOptions::default()
    };
    let blocks = guide_markdown::render(
        "[Скрипт](#jump) ![Кадр](images/shot.png) [Чужой](#evil)",
        &options,
    );
    let Block::Paragraph(content) = &blocks[0] else {
        panic!("{blocks:?}");
    };
    assert!(content.contains(&Inline::Link {
        target: LinkTarget::Script("jump".to_owned()),
        label: vec![Inline::Text("Скрипт".to_owned())],
    }));
    assert!(content.contains(&Inline::Image {
        src: "https://example.com/guide/images/shot.png".to_owned(),
        alt: "Кадр".to_owned(),
    }));
    // Неизвестный скрипт — один текст ссылки.
    assert_eq!(content.last(), Some(&Inline::Text("Чужой".to_owned())));
}
