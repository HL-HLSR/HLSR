//! Урезанный markdown (`shared/guide-markdown.js`): абзацы, списки с вложенностью, блоки
//! ```кода```, `код`, **жирный**, *курсив*, ссылки и картинки. В 0.4.7 это строка HTML; здесь
//! разбор даёт дерево блоков (спецификация, карта §0.7): окно рисует его само, а тест печатает
//! дерево в тот же HTML ([`to_html`]) и сверяет с `render` Node.
//!
//! Ссылки не открываются сами: у них либо `#id` (другой скрипт гайда), либо внешняя
//! `https`-ссылка; переход делает окно. Разбор повторяет автомат `render` 1:1, включая его
//! порядок закрытия списков; регулярные выражения — `regress` (JS: просмотр назад,
//! `\s` с U+FEFF и без U+0085, `.` без переводов строк).

use hlsr_core::js;
use hlsr_core::json_text::js_number;
use regress::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

/// `INLINE_SOURCE`.
static INLINE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"`([^`]+)`|\*\*(.+?)\*\*|(?:^|(?<=[\s(]))\*(?!\s)(.+?)\*(?=$|[\s.,;:!?)])|!\[([^\]]*)\]\(([^)\s]+)\)|\[([^\]]+)\]\(([^)\s]+)\)",
    )
    .expect("шаблон INLINE_SOURCE")
});

/// `LIST_ITEM`.
static LIST_ITEM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s*)([-*]|\d+\.)\s+(.*)$").expect("шаблон LIST_ITEM"));

/// Заголовок: `/^(#{1,6})\s+(.+)$/`.
static HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(#{1,6})\s+(.+)$").expect("шаблон заголовка"));

/// `options` для `render`: `{headings, preserveCodeIndent, imageBase, scriptIds}`.
#[derive(Debug, Clone, Default)]
pub struct GuideOptions {
    /// `# заголовки` (уровень `min(6, решёток + 2)`).
    pub headings: bool,
    /// Строки блока кода остаются с отступом (иначе обрезаются).
    pub preserve_code_indent: bool,
    /// База адресов картинок; нет или пусто — картинки отбрасываются.
    pub image_base: Option<String>,
    /// Известные `id` скриптов: ссылка `#id` на другой остаётся только для них (нет набора —
    /// остаются все).
    pub script_ids: Option<HashSet<String>>,
}

/// Строчный узел.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    /// `код`.
    Code(String),
    /// `**жирный**`.
    Strong(Vec<Inline>),
    /// `*курсив*`.
    Emphasis(Vec<Inline>),
    Link {
        target: LinkTarget,
        label: Vec<Inline>,
    },
    /// Картинка из репозитория гайда: готовый адрес и подпись.
    Image {
        src: String,
        alt: String,
    },
    /// Перенос строки внутри абзаца или пункта (`<br>`).
    LineBreak,
}

/// Куда ведёт ссылка.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// `[текст](#id)`: другой скрипт гайда (`data-script`).
    Script(String),
    /// `[текст](https://…)`: внешняя ссылка (`data-href`).
    Href(String),
}

/// Часть пункта списка: строчный узел или вложенный список.
#[derive(Debug, Clone, PartialEq)]
pub enum ItemPart {
    Inline(Inline),
    List(List),
}

/// Список (`ul` или `ol`).
#[derive(Debug, Clone, PartialEq)]
pub struct List {
    pub ordered: bool,
    /// `start` у `ol`, если первый номер не `1.` (`Number.parseInt`: до 10²¹ и дальше — число
    /// с плавающей точкой, как в JS).
    pub start: Option<f64>,
    pub items: Vec<Vec<ItemPart>>,
}

/// Блок.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Paragraph(Vec<Inline>),
    /// `h3`…`h6`: уровень `min(6, решёток + 2)`.
    Heading {
        level: u8,
        content: Vec<Inline>,
    },
    List(List),
    /// Блок кода (`pre > code`).
    Code(String),
}

/// `escapeHtml(text)`.
pub fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// `imageUrl(src, options)`: картинки гайда лежат в его репозитории, путь — только простые
/// символы.
fn image_url(src: &str, options: &GuideOptions) -> Option<String> {
    let base = options
        .image_base
        .as_deref()
        .filter(|base| !base.is_empty())?;
    let simple = !src.is_empty()
        && src
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'/' | b'%' | b'-'));
    if !simple || src.contains("..") {
        return None;
    }
    let rest = src
        .strip_prefix("./")
        .or_else(|| src.strip_prefix('/'))
        .unwrap_or(src);
    Some(format!("{base}/{rest}"))
}

/// `/^https:\/\/[^\s"'<>]+$/`.
fn is_https_link(href: &str) -> bool {
    href.strip_prefix("https://").is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .chars()
                .all(|c| !js::is_whitespace(c) && !matches!(c, '"' | '\'' | '<' | '>'))
    })
}

/// `link(label, href, options)`: ссылка или, если цель не годится, один текст ссылки.
fn push_link(label: Vec<Inline>, href: &str, options: &GuideOptions, out: &mut Vec<Inline>) {
    if let Some(id) = href.strip_prefix('#') {
        if options
            .script_ids
            .as_ref()
            .is_some_and(|ids| !ids.contains(id))
        {
            out.extend(label);
            return;
        }
        out.push(Inline::Link {
            target: LinkTarget::Script(id.to_owned()),
            label,
        });
    } else if is_https_link(href) {
        out.push(Inline::Link {
            target: LinkTarget::Href(href.to_owned()),
            label,
        });
    } else {
        out.extend(label);
    }
}

fn push_text(out: &mut Vec<Inline>, text: &str) {
    if !text.is_empty() {
        out.push(Inline::Text(text.to_owned()));
    }
}

/// `inline(text, options)`: строчные узлы строки; жирный и курсив разбираются рекурсивно.
pub fn inline(text: &str, options: &GuideOptions) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut last = 0;
    for found in INLINE_PATTERN.find_iter(text) {
        let range = found.range();
        push_text(&mut out, &text[last..range.start]);
        last = range.end;
        let group = |index: usize| found.group(index).map(|span| &text[span]);
        if let Some(code) = group(1) {
            out.push(Inline::Code(code.to_owned()));
        } else if let Some(strong) = group(2) {
            out.push(Inline::Strong(inline(strong, options)));
        } else if let Some(emphasis) = group(3) {
            out.push(Inline::Emphasis(inline(emphasis, options)));
        } else if let Some(src) = group(5) {
            if let Some(src) = image_url(src, options) {
                out.push(Inline::Image {
                    src,
                    alt: group(4).unwrap_or_default().to_owned(),
                });
            }
        } else {
            let label = inline(group(6).unwrap_or_default(), options);
            push_link(label, group(7).unwrap_or_default(), options, &mut out);
        }
    }
    push_text(&mut out, &text[last..]);
    out
}

/// Открытый список автомата `render`: тег, отступ и собранное содержимое.
struct OpenList {
    indent: usize,
    list: List,
}

/// Автомат `render`.
struct Parser<'a> {
    options: &'a GuideOptions,
    blocks: Vec<Block>,
    paragraph: Vec<String>,
    lists: Vec<OpenList>,
}

impl Parser<'_> {
    fn flush_paragraph(&mut self) {
        if self.paragraph.is_empty() {
            return;
        }
        let mut content = Vec::new();
        for (at, line) in std::mem::take(&mut self.paragraph).iter().enumerate() {
            if at > 0 {
                content.push(Inline::LineBreak);
            }
            content.extend(inline(line, self.options));
        }
        self.blocks.push(Block::Paragraph(content));
    }

    /// `closeLists(indent)`: закрыть списки с отступом не меньше `indent`; закрытый список
    /// ложится в последний пункт родителя или в блоки.
    fn close_lists(&mut self, indent: usize) {
        while self.lists.last().is_some_and(|open| open.indent >= indent) {
            let Some(OpenList { list, .. }) = self.lists.pop() else {
                return;
            };
            match self.lists.last_mut() {
                Some(parent) => {
                    if let Some(item) = parent.list.items.last_mut() {
                        item.push(ItemPart::List(list));
                    }
                }
                None => self.blocks.push(Block::List(list)),
            }
        }
    }

    /// Содержимое текущего (последнего) пункта открытого списка.
    fn current_item(&mut self) -> Option<&mut Vec<ItemPart>> {
        self.lists.last_mut()?.list.items.last_mut()
    }
}

/// Пробел или иной пробельный знак в начале: `/^\s+/.test(line)`.
fn starts_with_space(line: &str) -> bool {
    line.chars().next().is_some_and(js::is_whitespace)
}

/// `/^\s*```/.test(line)`.
fn is_fence(line: &str) -> bool {
    line.trim_start_matches(js::is_whitespace)
        .starts_with("```")
}

/// Отступ пункта: табуляции — по два пробела (`replace(/\t/g, "  ").length`).
fn indent_width(spaces: &str) -> usize {
    spaces
        .chars()
        .map(|c| if c == '\t' { 2 } else { c.len_utf16() })
        .sum()
}

/// `render(markdown, options)`: дерево блоков.
pub fn render(markdown: &str, options: &GuideOptions) -> Vec<Block> {
    let text = markdown.replace("\r\n", "\n");
    let lines: Vec<&str> = text.split('\n').collect();
    let mut parser = Parser {
        options,
        blocks: Vec::new(),
        paragraph: Vec::new(),
        lists: Vec::new(),
    };

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if is_fence(line) {
            parser.flush_paragraph();
            parser.close_lists(0);
            let mut code: Vec<&str> = Vec::new();
            while i < lines.len() && !is_fence(lines[i]) {
                code.push(if options.preserve_code_indent {
                    lines[i]
                } else {
                    js::trim(lines[i])
                });
                i += 1;
            }
            // Закрывающая черта (если есть) пропускается вместе с остальным.
            i += 1;
            parser.blocks.push(Block::Code(code.join("\n")));
            continue;
        }
        if options.headings
            && let Some(heading) = HEADING.find(line)
        {
            parser.flush_paragraph();
            parser.close_lists(0);
            let marks = heading.group(1).map_or(0, |span| span.len());
            let level = (marks + 2).min(6) as u8;
            let content = inline(&line[heading.group(2).unwrap_or(0..0)], options);
            parser.blocks.push(Block::Heading { level, content });
            continue;
        }
        if let Some(item) = LIST_ITEM.find(line) {
            parser.flush_paragraph();
            let indent = indent_width(&line[item.group(1).unwrap_or(0..0)]);
            let marker = &line[item.group(2).unwrap_or(0..0)];
            let ordered = marker.ends_with('.');
            if parser.lists.last().is_some_and(|top| top.indent > indent) {
                parser.close_lists(indent + 1);
            }
            let same_list = parser
                .lists
                .last()
                .is_some_and(|current| current.indent == indent && current.list.ordered == ordered);
            if same_list {
                if let Some(open) = parser.lists.last_mut() {
                    open.list.items.push(Vec::new());
                }
            } else {
                if parser
                    .lists
                    .last()
                    .is_some_and(|current| current.indent == indent)
                {
                    parser.close_lists(indent);
                }
                let start = (ordered && marker != "1.").then(|| {
                    let digits = marker.trim_end_matches('.');
                    digits.parse::<f64>().unwrap_or(f64::NAN)
                });
                parser.lists.push(OpenList {
                    indent,
                    list: List {
                        ordered,
                        start,
                        items: vec![Vec::new()],
                    },
                });
            }
            let content = inline(&line[item.group(3).unwrap_or(0..0)], options);
            if let Some(current) = parser.current_item() {
                current.extend(content.into_iter().map(ItemPart::Inline));
            }
            continue;
        }
        if js::trim(line).is_empty() {
            parser.flush_paragraph();
            parser.close_lists(0);
            continue;
        }
        // Строка с отступом под пунктом списка продолжает этот пункт.
        if !parser.lists.is_empty() && starts_with_space(line) {
            let content = inline(js::trim(line), options);
            if let Some(current) = parser.current_item() {
                current.push(ItemPart::Inline(Inline::LineBreak));
                current.extend(content.into_iter().map(ItemPart::Inline));
            }
            continue;
        }
        parser.close_lists(0);
        parser.paragraph.push(js::trim(line).to_owned());
    }
    parser.flush_paragraph();
    parser.close_lists(0);
    parser.blocks
}

// ------------------------------------------------------------------ печать в HTML (сверка с Node)

/// Строчные узлы как HTML, который собирал `inline` в 0.4.7.
pub fn inline_html(nodes: &[Inline]) -> String {
    let mut out = String::new();
    for node in nodes {
        match node {
            Inline::Text(text) => out.push_str(&escape_html(text)),
            Inline::Code(code) => {
                out.push_str("<code>");
                out.push_str(&escape_html(code));
                out.push_str("</code>");
            }
            Inline::Strong(inner) => {
                out.push_str("<strong>");
                out.push_str(&inline_html(inner));
                out.push_str("</strong>");
            }
            Inline::Emphasis(inner) => {
                out.push_str("<em>");
                out.push_str(&inline_html(inner));
                out.push_str("</em>");
            }
            Inline::Link { target, label } => {
                let (attribute, value) = match target {
                    LinkTarget::Script(id) => ("data-script", id),
                    LinkTarget::Href(href) => ("data-href", href),
                };
                out.push_str(&format!(
                    "<a href=\"#\" {attribute}=\"{}\">{}</a>",
                    escape_html(value),
                    inline_html(label)
                ));
            }
            Inline::Image { src, alt } => out.push_str(&format!(
                "<img src=\"{}\" alt=\"{}\" loading=\"lazy\">",
                escape_html(src),
                escape_html(alt)
            )),
            Inline::LineBreak => out.push_str("<br>"),
        }
    }
    out
}

fn list_html(list: &List, out: &mut String) {
    let tag = if list.ordered { "ol" } else { "ul" };
    out.push('<');
    out.push_str(tag);
    if let Some(start) = list.start {
        out.push_str(&format!(" start=\"{}\"", js_number(start)));
    }
    out.push('>');
    for item in &list.items {
        out.push_str("<li>");
        for part in item {
            match part {
                ItemPart::Inline(node) => out.push_str(&inline_html(std::slice::from_ref(node))),
                ItemPart::List(nested) => list_html(nested, out),
            }
        }
        out.push_str("</li>");
    }
    out.push_str("</");
    out.push_str(tag);
    out.push('>');
}

/// Блоки как HTML, который собирал `render` в 0.4.7.
pub fn to_html(blocks: &[Block]) -> String {
    let mut out = String::new();
    for block in blocks {
        match block {
            Block::Paragraph(content) => {
                out.push_str("<p>");
                out.push_str(&inline_html(content));
                out.push_str("</p>");
            }
            Block::Heading { level, content } => {
                out.push_str(&format!("<h{level}>{}</h{level}>", inline_html(content)));
            }
            Block::List(list) => list_html(list, &mut out),
            Block::Code(code) => {
                out.push_str("<pre><code>");
                out.push_str(&escape_html(code));
                out.push_str("</code></pre>");
            }
        }
    }
    out
}
