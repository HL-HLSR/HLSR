//! Markdown → документ RichText на корпусе 0.4.7 (раскладку делает вид):
//! - самотест новостей и граничные случаи разбора — фикстура ПП4
//!   `hlsr-online/tests/fixtures/news-markdown.json` (HTML `render`/`inline` из
//!   Node): текст документа = `textContent` этого HTML, ссылок и картинок
//!   столько же, сколько `<a>` и `<img>`;
//! - описания гайда HL1 (75 скриптов, en и ru: `usage` и `notes` с
//!   `MD_OPTIONS` из `GameScripts.vue`) — тот же текст, что у разбора ПП4.

use hlsr_ui::markdown::{self, Options};
use hlsr_ui::richtext::{self, Doc};
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;

fn read(path: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

fn options_of(spec: &Value) -> Options {
    Options {
        headings: spec["headings"].as_bool().unwrap_or(false),
        preserve_code_indent: spec["preserveCodeIndent"].as_bool().unwrap_or(false),
        image_base: spec["imageBase"].as_str().map(str::to_owned),
        script_ids: spec["scriptIds"].as_array().map(|ids| {
            ids.iter()
                .map(|id| id.as_str().unwrap().to_owned())
                .collect::<HashSet<_>>()
        }),
    }
}

/// `textContent` строки HTML `guide-markdown.js`: без тегов, сущности `escapeHtml`
/// раскрыты.
fn text_content(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        rest = rest[at..].split_once('>').map_or("", |(_, after)| after);
    }
    out.push_str(rest);
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

fn count(html: &str, tag: &str) -> usize {
    html.matches(tag).count()
}

struct Check {
    failures: Vec<String>,
    checked: usize,
}

impl Check {
    fn new() -> Check {
        Check {
            failures: Vec::new(),
            checked: 0,
        }
    }

    fn doc(&mut self, what: &str, doc: &Doc, html: &str) {
        self.checked += 1;
        let text = doc.text_content();
        let expected = text_content(html);
        if text != expected {
            self.failures.push(format!(
                "{what}: текст\n  Node: {expected:?}\n  Rust: {text:?}"
            ));
        }
        if doc.links.len() != count(html, "<a ") {
            self.failures.push(format!(
                "{what}: ссылок {} ≠ <a> {}",
                doc.links.len(),
                count(html, "<a ")
            ));
        }
        let images = doc.text_content_images();
        if images != count(html, "<img ") {
            self.failures.push(format!(
                "{what}: картинок {images} ≠ <img> {}",
                count(html, "<img ")
            ));
        }
    }

    fn finish(self, what: &str, at_least: usize) {
        assert!(
            self.checked >= at_least,
            "{what}: проверено {}",
            self.checked
        );
        assert!(
            self.failures.is_empty(),
            "{what}: {} расхождений (первые 8):\n{}",
            self.failures.len(),
            self.failures
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

trait Images {
    fn text_content_images(&self) -> usize;
}

impl Images for Doc {
    fn text_content_images(&self) -> usize {
        use richtext::{Block, Inline};
        fn inlines(nodes: &[Inline]) -> usize {
            nodes
                .iter()
                .filter(|node| matches!(node, Inline::Image { .. }))
                .count()
        }
        fn blocks(list: &[Block]) -> usize {
            list.iter()
                .map(|block| match block {
                    Block::Paragraph(nodes) | Block::Heading(_, nodes) | Block::Inline(nodes) => {
                        inlines(nodes)
                    }
                    Block::List(list) => list.items.iter().map(|item| blocks(item)).sum(),
                    Block::Code(_) => 0,
                })
                .sum()
        }
        blocks(&self.blocks)
    }
}

const FIXTURE: &str = "../hlsr-online/tests/fixtures/news-markdown.json";

#[test]
fn news_corpus_keeps_text_links_and_images() {
    let fixture = read(FIXTURE);
    let mut check = Check::new();
    for doc in fixture["docs"].as_array().unwrap() {
        let source = doc["markdown"].as_str().unwrap();
        check.doc(
            &format!("новость {source:?}"),
            &markdown::news(source),
            doc["news"].as_str().unwrap(),
        );
    }
    check.finish("news-markdown", 300);
}

#[test]
fn guide_corpus_keeps_text_links_and_images() {
    let fixture = read(FIXTURE);
    let options: Vec<Options> = fixture["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(options_of)
        .collect();
    let mut check = Check::new();
    for doc in fixture["docs"].as_array().unwrap() {
        let source = doc["markdown"].as_str().unwrap();
        for (at, options) in options.iter().enumerate() {
            check.doc(
                &format!("гайд[{at}] {source:?}"),
                &markdown::doc(source, options),
                doc["guide"][at].as_str().unwrap(),
            );
        }
    }
    for case in fixture["inline"].as_array().unwrap() {
        let source = case["text"].as_str().unwrap();
        for (at, options) in options.iter().enumerate() {
            check.doc(
                &format!("inline[{at}] {source:?}"),
                &markdown::inline_doc(source, options),
                case["html"][at].as_str().unwrap(),
            );
        }
    }
    check.finish("guide-markdown", 1500);
}

/// Описания гайда 0.4.7: английские из `hl1-guide-data.json`, русские из
/// `hl1-guide-ru.json`.
fn guide_descriptions() -> (Options, Vec<(String, String, Vec<String>)>) {
    let data = read("../hlsr-delivery/data/hl1-guide-data.json");
    let ru = read("../hlsr-delivery/data/hl1-guide-ru.json");
    let commit = data["source"]["commit"].as_str().unwrap();
    let scripts = data["scripts"].as_array().unwrap();
    let ids: HashSet<String> = scripts
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_owned())
        .collect();
    // GameScripts.vue:190 MD_OPTIONS.
    let options = Options {
        image_base: Some(format!(
            "https://raw.githubusercontent.com/yavats/hl1-scripts/{commit}"
        )),
        script_ids: Some(ids),
        ..Options::default()
    };
    let strings = |value: &Value| -> Vec<String> {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_owned())
            .collect()
    };
    let mut out = Vec::new();
    for script in scripts {
        let id = script["id"].as_str().unwrap();
        out.push((
            format!("{id}/en"),
            script["usage"].as_str().unwrap().to_owned(),
            strings(&script["notes"]),
        ));
        let translated = &ru["scripts"][id];
        if translated.is_object() {
            out.push((
                format!("{id}/ru"),
                translated["usage"].as_str().unwrap().to_owned(),
                strings(&translated["notes"]),
            ));
        }
    }
    (options, out)
}

#[test]
fn guide_descriptions_convert() {
    let (options, descriptions) = guide_descriptions();
    assert!(descriptions.len() >= 140, "описаний {}", descriptions.len());
    let mut failures = Vec::new();
    let mut checked = 0;
    for (what, usage, notes) in &descriptions {
        // Текст — тот же, что у разбора ПП4 (сверен с Node).
        let tree = hlsr_online::shared::guide_markdown::render(
            usage,
            &hlsr_online::shared::guide_markdown::GuideOptions {
                image_base: options.image_base.clone(),
                script_ids: options.script_ids.clone(),
                ..Default::default()
            },
        );
        let html = hlsr_online::shared::guide_markdown::to_html(&tree);
        let doc = markdown::doc(usage, &options);
        if doc.text_content() != text_content(&html) {
            failures.push(format!("{what}: текст usage"));
        }
        checked += 1;
        for note in notes {
            if markdown::inline_doc(note, &options)
                .text_content()
                .trim()
                .is_empty()
            {
                failures.push(format!("{what}: пустая заметка"));
            }
        }
    }
    assert!(checked >= 140, "описаний {checked}");
    assert!(
        failures.is_empty(),
        "{} расхождений (первые 8):\n{}",
        failures.len(),
        failures
            .iter()
            .take(8)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
