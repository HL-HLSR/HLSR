//! Документ cfg игрока (`shared/cfg-document.js` 0.4.9): разбор по строкам и точечная правка на
//! месте. Текст — уже декодированная строка (кодировку выбирает вызывающий, для файла игрока это
//! байты как latin1, байт в байт). Все функции чистые: возвращают новый документ.
//!
//! Позиции команд — байтовые смещения в строке Rust (у 0.4.9 — единицы UTF-16): правка режет
//! строку по тем же границам символов, итог тот же.

use crate::game_config::{Command, Token, line_commands};
use hlsr_core::js;

/// Заголовки разделов файла игрока по порядку.
pub const SECTIONS: [(&str, &str); 6] = [
    ("rules", "// ===== HLSR: category rules ====="),
    ("settings", "// ===== HLSR: settings ====="),
    ("personalization", "// ===== HLSR: personalization ====="),
    ("binds", "// ===== HLSR: binds ====="),
    ("scripts", "// ===== HLSR: scripts ====="),
    ("own", "// ===== Your own ====="),
];

/// Заголовок раздела по имени (`SECTIONS[name]`); неизвестное имя — пусто.
pub fn header(name: &str) -> &'static str {
    SECTIONS
        .iter()
        .find(|(key, _)| *key == name)
        .map_or("", |(_, header)| header)
}

fn is_header(text: &str) -> bool {
    SECTIONS.iter().any(|(_, header)| *header == text)
}

/// Глаголы, у которых второй аргумент — не значение переменной.
const NOT_CVAR: [&str; 9] = [
    "exec",
    "echo",
    "say",
    "say_team",
    "map",
    "changelevel",
    "connect",
    "wait",
    "toggle",
];

fn block_begin(id: &str) -> String {
    format!("// HLSR script {id} begin")
}

fn block_end(id: &str) -> String {
    format!("// HLSR script {id} end")
}

/// Строка документа и её перевод строки (`""` у последней без перевода).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub eol: String,
}

/// Разобранный cfg: метка порядка байтов, перевод строки файла (первый встреченный, иначе CRLF)
/// и строки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    pub bom: bool,
    pub eol: String,
    pub lines: Vec<Line>,
}

impl Doc {
    fn with_lines(&self, lines: Vec<Line>) -> Doc {
        Doc {
            bom: self.bom,
            eol: self.eol.clone(),
            lines,
        }
    }

    fn line(&self, text: impl Into<String>) -> Line {
        Line {
            text: text.into(),
            eol: self.eol.clone(),
        }
    }
}

/// `parse(text)`: строки по `/([^\r\n]*)(\r\n|\n|\r|$)/g`; одиночный `\r` — тоже перевод строки.
pub fn parse(text: &str) -> Doc {
    let (bom, mut rest) = match text.strip_prefix('\u{FEFF}') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let mut lines = Vec::new();
    loop {
        let end = rest.find(['\r', '\n']).unwrap_or(rest.len());
        let after = &rest[end..];
        let eol = if after.starts_with("\r\n") {
            "\r\n"
        } else if after.starts_with('\n') {
            "\n"
        } else if after.starts_with('\r') {
            "\r"
        } else {
            ""
        };
        if end == 0 && eol.is_empty() {
            break;
        }
        lines.push(Line {
            text: rest[..end].to_owned(),
            eol: eol.to_owned(),
        });
        rest = &after[eol.len()..];
    }
    let eol = lines
        .iter()
        .find(|line| !line.eol.is_empty())
        .map_or("\r\n", |line| line.eol.as_str())
        .to_owned();
    Doc { bom, eol, lines }
}

/// `serialize(doc)`.
pub fn serialize(doc: &Doc) -> String {
    let mut out = String::new();
    if doc.bom {
        out.push('\u{FEFF}');
    }
    for line in &doc.lines {
        out.push_str(&line.text);
        out.push_str(&line.eol);
    }
    out
}

/// Ключ команды: `cvar:<имя>`, `bind:<клавиша>`, `alias:<имя>`; иначе `None`.
pub fn key_of(args: &[Token]) -> Option<String> {
    let verb = args.first()?.value.to_lowercase();
    match verb.as_str() {
        "bind" => (args.len() >= 3).then(|| format!("bind:{}", args[1].value.to_lowercase())),
        "unbind" => (args.len() >= 2).then(|| format!("bind:{}", args[1].value.to_lowercase())),
        "alias" => (args.len() >= 3).then(|| format!("alias:{}", args[1].value.to_lowercase())),
        _ if verb.starts_with(['+', '-']) || NOT_CVAR.contains(&verb.as_str()) => None,
        _ => (args.len() >= 2).then(|| format!("cvar:{verb}")),
    }
}

/// Номер аргумента со значением: у bind и alias — третий, у cvar — второй.
fn value_arg(key: &str) -> usize {
    if key.starts_with("cvar:") { 1 } else { 2 }
}

/// Команда документа с ключом.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub line: usize,
    pub index: usize,
    pub command: Command,
    pub key: Option<String>,
}

/// `commandsOf(doc)`: все команды документа с ключом.
pub fn commands_of(doc: &Doc) -> Vec<Item> {
    let mut result = Vec::new();
    for (line_no, line) in doc.lines.iter().enumerate() {
        for (index, command) in line_commands(&line.text).into_iter().enumerate() {
            let key = key_of(&command.args);
            result.push(Item {
                line: line_no,
                index,
                command,
                key,
            });
        }
    }
    result
}

fn is_unbind(command: &Command) -> bool {
    command.args[0].value.to_lowercase() == "unbind"
}

/// Значение ключа: последняя команда с ним (unbind — значения нет).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub value: String,
    pub line: usize,
    pub index: usize,
}

/// `get(doc, key)`.
pub fn get(doc: &Doc, key: &str) -> Option<Found> {
    let found = commands_of(doc)
        .into_iter()
        .rfind(|item| item.key.as_deref() == Some(key))?;
    if is_unbind(&found.command) {
        return None;
    }
    Some(Found {
        value: found.command.args[value_arg(key)].value.clone(),
        line: found.line,
        index: found.index,
    })
}

/// Позиции слов команды внутри строки (`/"[^"]*"?|\S+/g`): `(start, end, quoted)`.
fn spans(line: &str, command: &Command) -> Vec<(usize, usize, bool)> {
    let mut result = Vec::new();
    let segment = &line[command.start..command.end];
    let mut at = 0;
    while at < segment.len() {
        let ch = segment[at..].chars().next().expect("есть символ");
        if ch == '"' {
            let end = segment[at + 1..]
                .find('"')
                .map_or(segment.len(), |p| at + 1 + p + 1);
            result.push((command.start + at, command.start + end, true));
            at = end;
        } else if js::is_whitespace(ch) {
            at += ch.len_utf8();
        } else {
            let length: usize = segment[at..]
                .chars()
                .take_while(|c| !js::is_whitespace(*c))
                .map(char::len_utf8)
                .sum();
            result.push((command.start + at, command.start + at + length, false));
            at += length;
        }
    }
    result
}

/// Границы раздела: заголовок и строка после его последней непустой строки.
fn section(doc: &Doc, name: &str) -> Option<(usize, usize)> {
    let wanted = header(name);
    let header_at = doc
        .lines
        .iter()
        .position(|line| js::trim(&line.text) == wanted)?;
    let mut end = header_at + 1;
    while end < doc.lines.len() && !is_header(js::trim(&doc.lines[end].text)) {
        end += 1;
    }
    // Пустые строки перед следующим заголовком остаются разделителем.
    while end - 1 > header_at
        && js::trim(&doc.lines[end - 1].text).is_empty()
        && end < doc.lines.len()
    {
        end -= 1;
    }
    Some((header_at, end))
}

/// `insertLines(doc, name, texts)`: строки в конец раздела; нет заголовка — раздел создаётся перед
/// «Your own» или в конце файла.
pub fn insert_lines(doc: &Doc, name: &str, texts: &[String]) -> Doc {
    let mut lines = doc.lines.clone();
    let make = |list: &[String]| list.iter().map(|text| doc.line(text.as_str())).collect();
    if let Some((_, end)) = section(doc, name) {
        if end == lines.len()
            && let Some(last) = lines.last_mut()
            && last.eol.is_empty()
        {
            last.eol = doc.eol.clone();
        }
        let added: Vec<Line> = make(texts);
        lines.splice(end..end, added);
        return doc.with_lines(lines);
    }
    let mut created = vec![header(name).to_owned()];
    created.extend(texts.iter().cloned());
    let own = if name == "own" {
        None
    } else {
        lines
            .iter()
            .position(|line| js::trim(&line.text) == header("own"))
    };
    if let Some(own) = own {
        created.push(String::new());
        let added: Vec<Line> = make(&created);
        lines.splice(own..own, added);
        return doc.with_lines(lines);
    }
    if let Some(last) = lines.last_mut()
        && last.eol.is_empty()
    {
        last.eol = doc.eol.clone();
    }
    if lines
        .last()
        .is_some_and(|last| !js::trim(&last.text).is_empty())
    {
        created.insert(0, String::new());
    }
    let added: Vec<Line> = make(&created);
    lines.extend(added);
    doc.with_lines(lines)
}

fn new_line(key: &str, value: &str) -> String {
    let name = &key[key.find(':').map_or(0, |at| at + 1)..];
    if key.starts_with("bind:") {
        format!("bind \"{name}\" \"{value}\"")
    } else if key.starts_with("alias:") {
        format!("alias {name} \"{value}\"")
    } else {
        format!("{name} \"{value}\"")
    }
}

fn is_unbindall(item: &Item) -> bool {
    item.command
        .args
        .first()
        .is_some_and(|arg| arg.value.to_lowercase() == "unbindall")
}

fn is_after(item: &Item, other: &Item) -> bool {
    item.line > other.line || (item.line == other.line && item.index > other.index)
}

/// `set(doc, key, value, sectionName)`: значение последней команды с ключом меняется на месте;
/// нет команды — новая строка в раздел.
pub fn set(doc: &Doc, key: &str, value: &str, section_name: &str) -> Doc {
    let commands = commands_of(doc);
    let found = commands
        .iter()
        .rfind(|item| item.key.as_deref() == Some(key));
    // Бинд выше unbindall (шаблон над импортированным config.cfg) в игре снят: правка на месте
    // ничего бы не дала. Новая строка идёт после последнего unbindall (в раздел, если его нет или
    // он ниже), а следующая правка меняет её на месте: мёртвых строк не копится.
    if let Some(found) = found
        && key.starts_with("bind:")
        && let Some(last) = commands.iter().rfind(|item| is_unbindall(item))
        && is_after(last, found)
    {
        match section(doc, section_name) {
            Some((_, end)) if end <= last.line => {
                let mut lines = doc.lines.clone();
                if lines[last.line].eol.is_empty() {
                    lines[last.line].eol = doc.eol.clone();
                }
                lines.insert(last.line + 1, doc.line(new_line(key, value)));
                return doc.with_lines(lines);
            }
            _ => return insert_lines(doc, section_name, &[new_line(key, value)]),
        }
    }
    let Some(found) = found else {
        return insert_lines(doc, section_name, &[new_line(key, value)]);
    };
    let mut lines = doc.lines.clone();
    let text = &doc.lines[found.line].text;
    let words = spans(text, &found.command);
    if is_unbind(&found.command) {
        // Последний unbind заменяем на месте, чтобы новый bind остался последним.
        let (first, last) = (words[0].0, words[words.len() - 1].1);
        lines[found.line].text = format!(
            "{}{}{}",
            &text[..first],
            new_line(key, value),
            &text[last..]
        );
        return doc.with_lines(lines);
    }
    let (start, end, was_quoted) = words[value_arg(key)];
    let quoted =
        was_quoted || value.is_empty() || value.chars().any(|c| js::is_whitespace(c) || c == ';');
    let value = if quoted {
        format!("\"{value}\"")
    } else {
        value.to_owned()
    };
    lines[found.line].text = format!("{}{value}{}", &text[..start], &text[end..]);
    doc.with_lines(lines)
}

/// `removeKey(doc, key)`: все команды с ключом; строка из одних таких команд уходит целиком.
pub fn remove_key(doc: &Doc, key: &str) -> Doc {
    let mut lines = Vec::new();
    for line in &doc.lines {
        let all = line_commands(&line.text);
        let mine: Vec<&Command> = all
            .iter()
            .filter(|command| key_of(&command.args).as_deref() == Some(key))
            .collect();
        if mine.is_empty() {
            lines.push(line.clone());
            continue;
        }
        if all.len() == mine.len() {
            continue;
        }
        // Вырезаем команду вместе с «;» после неё (у последней — с «;» перед ней).
        let mut text = line.text.clone();
        for command in mine.iter().rev() {
            let semi = text
                .get(command.end..)
                .and_then(|tail| tail.find(';'))
                .map(|at| command.end + at);
            let next = all.iter().any(|other| other.start > command.start);
            match semi.filter(|_| next) {
                Some(semi) => {
                    let tail = &text[semi + 1..];
                    let tail = if command.start == 0 {
                        tail.trim_start_matches(js::is_whitespace)
                    } else {
                        tail
                    };
                    text = format!("{}{tail}", &text[..command.start]);
                }
                None => {
                    let head = &text[..command.start];
                    let trimmed = head.trim_end_matches(js::is_whitespace);
                    let head = trimmed.strip_suffix(';').unwrap_or(head);
                    text = format!("{head}{}", &text[command.end..]);
                }
            }
        }
        lines.push(Line {
            text,
            eol: line.eol.clone(),
        });
    }
    doc.with_lines(lines)
}

/// Блок скрипта лаунчера: строки начала и конца и тело между ними.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub begin: usize,
    pub end: usize,
    pub body: Vec<String>,
}

/// `findBlock(doc, id)`.
pub fn find_block(doc: &Doc, id: &str) -> Option<Block> {
    let texts: Vec<&str> = doc.lines.iter().map(|line| js::trim(&line.text)).collect();
    let (begin_text, end_text) = (block_begin(id), block_end(id));
    let begin = texts.iter().position(|text| *text == begin_text)?;
    let end = begin
        + 1
        + texts[begin + 1..]
            .iter()
            .position(|text| *text == end_text)?;
    Some(Block {
        begin,
        end,
        body: doc.lines[begin + 1..end]
            .iter()
            .map(|line| line.text.clone())
            .collect(),
    })
}

/// `setBlock(doc, id, bodyLines, sectionName)`: тело блока заменяется; блока нет — он
/// дописывается в раздел.
pub fn set_block(doc: &Doc, id: &str, body: &[String], section_name: &str) -> Doc {
    let Some(block) = find_block(doc, id) else {
        let mut texts = vec![block_begin(id)];
        texts.extend(body.iter().cloned());
        texts.push(block_end(id));
        return insert_lines(doc, section_name, &texts);
    };
    let mut lines = doc.lines.clone();
    let added: Vec<Line> = body.iter().map(|text| doc.line(text.as_str())).collect();
    lines.splice(block.begin + 1..block.end, added);
    doc.with_lines(lines)
}

/// `removeBlock(doc, id)`.
pub fn remove_block(doc: &Doc, id: &str) -> Doc {
    let Some(block) = find_block(doc, id) else {
        return doc.clone();
    };
    let mut lines = doc.lines.clone();
    lines.drain(block.begin..=block.end);
    doc.with_lines(lines)
}

/// `blockIds(doc)`: id блоков скриптов лаунчера в порядке появления.
pub fn block_ids(doc: &Doc) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for line in &doc.lines {
        let Some(id) = js::trim(&line.text)
            .strip_prefix("// HLSR script ")
            .and_then(|rest| rest.strip_suffix(" begin"))
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        if !ids.iter().any(|known| known == id) {
            ids.push(id.to_owned());
        }
    }
    ids
}
