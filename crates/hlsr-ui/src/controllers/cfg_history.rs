//! История правок редактора cfg (решение 23 спецификации ПП5, риск 14 карты): своя отмена
//! и повтор вместо встроенных у `TextInput` Slint — шаги и выделение как у `textarea`
//! Chromium 0.4.7 (`blink::UndoStack`, `TypingCommand`).
//!
//! Как Chromium группирует правки в шаги отмены:
//! - ввод с клавиатуры — «открытая» команда набора (`TypingCommand`): следующий набранный
//!   символ (и перевод строки) дописывается в неё, пока набор не закрыт; Backspace дописывается
//!   только к открытой команде удаления назад, Delete — только к удалению вперёд
//!   (`DeleteKeyPressed`/`ForwardDeleteKeyPressed`); иначе начинается новый шаг;
//! - набор закрывает любая смена выделения не набором (стрелки, щелчок, `setSelectionRange`),
//!   отмена и повтор, правка не набором;
//! - вставка, вырезание и правки программы (`execCommand('insertText')` в `applyEdit`) — каждая
//!   своим шагом;
//! - отмена возвращает выделение начала шага, повтор — выделение конца; новый шаг стирает
//!   повторы; глубина — 1000 шагов (`kMaximumUndoStackDepth`).
//!
//! Шаг хранит одну замену участка текста (что было и что стало), смещения — байты UTF-8;
//! выделение — единицы UTF-16, как у `textarea`.

/// Шагов отмены не больше (`UndoStack::kMaximumUndoStackDepth`).
pub const MAX_STEPS: usize = 1000;

/// Чем вызвана правка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    /// Набор символа или перевод строки (`InsertText`, `InsertLineBreak`).
    Insert,
    /// Backspace и Ctrl+Backspace (`DeleteKey`).
    DeleteBackward,
    /// Delete и Ctrl+Delete (`ForwardDeleteKey`).
    DeleteForward,
    /// Вставка из буфера обмена (`ReplaceSelectionCommand`).
    Paste,
    /// Вырезание (`DeleteSelectionCommand`).
    Cut,
    /// Правка программы (`execCommand` из `applyEdit`) и всё прочее: свой шаг.
    Command,
}

impl EditKind {
    /// Правка набором (`TypingCommand`): может остаться открытой.
    fn is_typing(self) -> bool {
        matches!(
            self,
            EditKind::Insert | EditKind::DeleteBackward | EditKind::DeleteForward
        )
    }
}

/// Выделение `[start, end]` в единицах UTF-16.
pub type Range16 = (usize, usize);

/// Замена участка: в тексте «до» с `at` стоял `removed`, в тексте «после» — `inserted`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Change {
    at: usize,
    removed: String,
    inserted: String,
}

impl Change {
    /// Наименьшая замена, переводящая `before` в `after` (общие начало и конец по символам).
    fn between(before: &str, after: &str) -> Change {
        let prefix = common_prefix(before, after);
        let suffix = common_suffix(&before[prefix..], &after[prefix..]);
        Change {
            at: prefix,
            removed: before[prefix..before.len() - suffix].to_owned(),
            inserted: after[prefix..after.len() - suffix].to_owned(),
        }
    }

    /// Применить к тексту «до».
    fn apply(&self, text: &str) -> String {
        splice(text, self.at, self.removed.len(), &self.inserted)
    }

    /// Откатить у текста «после».
    fn revert(&self, text: &str) -> String {
        splice(text, self.at, self.inserted.len(), &self.removed)
    }

    /// Одна замена из двух подряд: `self` (A → B), затем `next` (B → C), `b` — текст B.
    fn then(&self, next: &Change, b: &str) -> Change {
        let start = self.at.min(next.at);
        // Конец затронутого участка в B: длина общего хвоста — меньшая из двух.
        let tail_self = b.len() - (self.at + self.inserted.len());
        let tail_next = b.len() - (next.at + next.removed.len());
        let tail = tail_self.min(tail_next);
        let b_end = b.len() - tail;
        // Участок B[start..b_end] в A и в C.
        let a_part = {
            let before = &b[start..self.at];
            let after = &b[self.at + self.inserted.len()..b_end];
            format!("{before}{}{after}", self.removed)
        };
        let c_part = {
            let before = &b[start..next.at];
            let after = &b[next.at + next.removed.len()..b_end];
            format!("{before}{}{after}", next.inserted)
        };
        Change {
            at: start,
            removed: a_part,
            inserted: c_part,
        }
    }
}

fn splice(text: &str, at: usize, len: usize, with: &str) -> String {
    let mut out = String::with_capacity(text.len() + with.len());
    out.push_str(&text[..at]);
    out.push_str(with);
    out.push_str(&text[at + len..]);
    out
}

/// Длина общего начала в байтах, по границе символа (символы до неё совпали в обоих).
fn common_prefix(a: &str, b: &str) -> usize {
    a.char_indices()
        .zip(b.chars())
        .find(|((_, x), y)| x != y)
        .map_or(a.len().min(b.len()), |((at, _), _)| at)
}

/// Длина общего конца в байтах, по границе символа.
fn common_suffix(a: &str, b: &str) -> usize {
    let mut len = 0;
    for (x, y) in a.chars().rev().zip(b.chars().rev()) {
        if x != y {
            break;
        }
        len += x.len_utf8();
    }
    len
}

#[derive(Debug, Clone)]
struct Step {
    change: Change,
    /// Выделение до шага (возвращает отмена) и после (возвращает повтор).
    before: Range16,
    after: Range16,
    /// Тип последней правки шага (`CommandTypeOfOpenCommand`).
    kind: EditKind,
}

/// Что вернуть в поле после отмены или повтора.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restored {
    pub text: String,
    pub selection: Range16,
}

/// История правок одного открытого файла.
#[derive(Debug, Clone, Default)]
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// Последний шаг — открытая команда набора.
    typing_open: bool,
}

impl History {
    pub fn new() -> History {
        History::default()
    }

    /// Новый текст поля не правкой (файл открыт, «Вернуть»): прежние шаги к нему не относятся.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.typing_open = false;
    }

    /// Выделение сменилось не набором (`TypingCommand::CloseTyping`).
    pub fn close_typing(&mut self) {
        self.typing_open = false;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Шагов отмены.
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    /// Открыта ли команда набора.
    pub fn typing_open(&self) -> bool {
        self.typing_open
    }

    /// Правка поля: текст был `before` с выделением `selection_before`, стал `after` с
    /// `selection_after`.
    pub fn record(
        &mut self,
        kind: EditKind,
        before: &str,
        after: &str,
        selection_before: Range16,
        selection_after: Range16,
    ) {
        let change = Change::between(before, after);
        let coalesce = self.typing_open
            && match kind {
                // Набор дописывается в любую открытую команду набора.
                EditKind::Insert => true,
                EditKind::DeleteBackward | EditKind::DeleteForward => {
                    self.undo.last().is_some_and(|step| step.kind == kind)
                }
                _ => false,
            };
        if coalesce && let Some(step) = self.undo.last_mut() {
            step.change = step.change.then(&change, before);
            step.after = selection_after;
            step.kind = kind;
        } else {
            self.redo.clear();
            self.undo.push(Step {
                change,
                before: selection_before,
                after: selection_after,
                kind,
            });
            if self.undo.len() > MAX_STEPS {
                self.undo.remove(0);
            }
        }
        self.typing_open = kind.is_typing();
    }

    /// Ctrl+Z: текст и выделение до последнего шага; `None` — отменять нечего.
    pub fn undo(&mut self, text: &str) -> Option<Restored> {
        let step = self.undo.pop()?;
        self.typing_open = false;
        let restored = Restored {
            text: step.change.revert(text),
            selection: step.before,
        };
        self.redo.push(step);
        Some(restored)
    }

    /// Ctrl+Y, Ctrl+Shift+Z: повтор отменённого шага.
    pub fn redo(&mut self, text: &str) -> Option<Restored> {
        let step = self.redo.pop()?;
        self.typing_open = false;
        let restored = Restored {
            text: step.change.apply(text),
            selection: step.after,
        };
        self.undo.push(step);
        Some(restored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_compose_into_one_replacement() {
        let a = "abc def";
        let b = "abcX def";
        let c = "abcXY def";
        let first = Change::between(a, b);
        let second = Change::between(b, c);
        let both = first.then(&second, b);
        assert_eq!(both.apply(a), c);
        assert_eq!(both.revert(c), a);

        // Удаление после вставки в другом месте.
        let d = "abcXY de";
        let third = Change::between(c, d);
        let all = both.then(&third, c);
        assert_eq!(all.apply(a), d);
        assert_eq!(all.revert(d), a);
    }

    #[test]
    fn prefix_and_suffix_stay_on_char_boundaries() {
        let change = Change::between("ая", "аюя");
        assert_eq!(change.inserted, "ю");
        assert_eq!(change.apply("ая"), "аюя");
        let change = Change::between("ёё", "ё");
        assert_eq!(change.revert("ё"), "ёё");
    }
}
