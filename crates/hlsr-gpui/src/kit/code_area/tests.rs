//! Редактор на `VisualTestContext`: ввод через обработчик окна, клавиши, мышь, отмена группами
//! (сценарии `tests/cfg_files.rs` контроллера), IME, буфер обмена, поиск и переход, 5000 строк.

use super::*;
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

type Events = Rc<RefCell<Vec<CodeEvent>>>;

/// Окно с одним редактором в фокусе; события копятся в `Events`.
fn area<'a>(
    cx: &'a mut TestAppContext,
    text: &str,
) -> (Entity<CodeArea>, &'a mut VisualTestContext, Events) {
    cx.update(crate::init);
    let text = text.to_owned();
    let (area, cx) = cx.add_window_view(|window, cx| {
        let mut area = CodeArea::new(window, cx);
        area.set_text(text, cx);
        area
    });
    let events = Events::default();
    let sink = events.clone();
    cx.update(|_, cx| {
        cx.subscribe(&area, move |_, event: &CodeEvent, _| {
            sink.borrow_mut().push(*event)
        })
        .detach()
    });
    area.update_in(cx, |area, window, cx| window.focus(&area.focus_handle, cx));
    cx.run_until_parked();
    (area, cx, events)
}

fn text(area: &Entity<CodeArea>, cx: &mut VisualTestContext) -> String {
    area.read_with(cx, |area, _| area.text().to_owned())
}

fn selection(area: &Entity<CodeArea>, cx: &mut VisualTestContext) -> Range<usize> {
    area.read_with(cx, |area, _| area.selection())
}

fn steps(area: &Entity<CodeArea>, cx: &mut VisualTestContext) -> usize {
    area.read_with(cx, |area, _| area.history.undo_len())
}

/// Новый кадр (раскладка — по нему).
fn frame(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

/// Точка окна посреди символа `offset` (по раскладке прошлого кадра).
fn at(area: &Entity<CodeArea>, cx: &mut VisualTestContext, offset: usize) -> Point<Pixels> {
    area.update_in(cx, |area, window, _| {
        let m = metrics(window);
        let ix = area.lines.of(offset);
        let y =
            area.frame.top - f32::from(area.scroll.top()) + m.pad_y + (ix as f32 + 0.5) * m.line;
        let x = area.frame.text_left + m.pad_x - area.scroll_x + area.x_of(offset, window);
        point(Pixels::from(x + 1.), Pixels::from(y))
    })
}

#[gpui::test]
fn typing_and_enter_go_through_the_window_input_handler(cx: &mut TestAppContext) {
    let (area, cx, events) = area(cx, "");
    cx.simulate_input("bind");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("x");
    assert_eq!(text(&area, cx), "bind\nx");
    assert_eq!(area.read_with(cx, |area, _| area.lines()), 2);
    assert_eq!(selection(&area, cx), 6..6);
    assert_eq!(*events.borrow(), vec![CodeEvent::Input; 6]);
    cx.simulate_keystrokes("left");
    assert_eq!(events.borrow().last(), Some(&CodeEvent::Select));
}

#[gpui::test]
fn shift_arrows_select_across_lines(cx: &mut TestAppContext) {
    let (area, cx, _) = area(cx, "abc\ndef\nghi");
    cx.simulate_keystrokes("shift-right shift-right");
    assert_eq!(selection(&area, cx), 0..2);
    cx.simulate_keystrokes("shift-down");
    assert_eq!(selection(&area, cx), 0..6);
    cx.simulate_keystrokes("shift-end");
    assert_eq!(selection(&area, cx), 0..7);
    cx.simulate_keystrokes("shift-up");
    assert_eq!(selection(&area, cx), 0..3);
    cx.simulate_keystrokes("ctrl-shift-end");
    assert_eq!(selection(&area, cx), 0..11);
    cx.simulate_keystrokes("ctrl-home");
    assert_eq!(selection(&area, cx), 0..0);
    // Ctrl+Shift+← от конца — к началу слова, выделение назад; ← сворачивает к его началу.
    cx.simulate_keystrokes("ctrl-end ctrl-shift-left");
    assert_eq!(selection(&area, cx), 8..11);
    assert!(area.read_with(cx, |area, _| area.reversed));
    cx.simulate_keystrokes("shift-left");
    assert_eq!(selection(&area, cx), 7..11);
    cx.simulate_keystrokes("left");
    assert_eq!(selection(&area, cx), 7..7);
}

#[gpui::test]
fn up_and_down_keep_the_column_over_a_short_line(cx: &mut TestAppContext) {
    let (area, cx, _) = area(cx, "abcdef\nx\nabcdef");
    area.update(cx, |area, cx| area.set_selection(5..5, cx));
    cx.simulate_keystrokes("down");
    assert_eq!(selection(&area, cx), 8..8);
    cx.simulate_keystrokes("down");
    assert_eq!(selection(&area, cx), 14..14);
    // За последней строкой — конец текста, за первой — начало.
    cx.simulate_keystrokes("down");
    assert_eq!(selection(&area, cx), 15..15);
    cx.simulate_keystrokes("ctrl-home up");
    assert_eq!(selection(&area, cx), 0..0);
}

#[gpui::test]
fn typing_is_one_undo_step_until_the_caret_moves(cx: &mut TestAppContext) {
    let (area, cx, _) = area(cx, "");
    cx.simulate_input("hello");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("world");
    assert_eq!(steps(&area, cx), 1, "набор с переводами строк — один шаг");
    // Стрелка закрывает набор: дальше — новый шаг.
    cx.simulate_keystrokes("ctrl-home end");
    cx.simulate_input("!");
    assert_eq!(text(&area, cx), "hello!\nworld");
    assert_eq!(steps(&area, cx), 2);
    cx.simulate_keystrokes("ctrl-z");
    assert_eq!(text(&area, cx), "hello\nworld");
    assert_eq!(selection(&area, cx), 5..5);
    cx.simulate_keystrokes("ctrl-z");
    assert_eq!(text(&area, cx), "");
    assert_eq!(selection(&area, cx), 0..0);
    cx.simulate_keystrokes("ctrl-y");
    assert_eq!(text(&area, cx), "hello\nworld");
    assert_eq!(selection(&area, cx), 11..11);
    cx.simulate_keystrokes("ctrl-shift-z");
    assert_eq!(text(&area, cx), "hello!\nworld");
}

#[gpui::test]
fn deletes_group_by_direction_and_typing_joins_an_open_delete(cx: &mut TestAppContext) {
    let (area, cx, _) = area(cx, "");
    cx.simulate_input("abcdef");
    cx.simulate_keystrokes("backspace backspace");
    assert_eq!(text(&area, cx), "abcd");
    // Backspace после набора — новый шаг, Backspace подряд — в нём же.
    assert_eq!(steps(&area, cx), 2);
    // Набор после удаления дописывается в открытую команду удаления.
    cx.simulate_input("XY");
    assert_eq!(steps(&area, cx), 2);
    // Delete после набора — снова новый шаг.
    cx.simulate_keystrokes("home delete delete");
    assert_eq!(text(&area, cx), "cdXY");
    assert_eq!(steps(&area, cx), 3);
    cx.simulate_keystrokes("ctrl-z");
    assert_eq!(text(&area, cx), "abcdXY");
    cx.simulate_keystrokes("ctrl-z");
    assert_eq!(text(&area, cx), "abcdef");
    assert_eq!(selection(&area, cx), 6..6);
    cx.simulate_keystrokes("ctrl-z");
    assert_eq!(text(&area, cx), "");
    assert!(!area.read_with(cx, |area, _| area.history.can_undo()));
}

#[gpui::test]
fn paste_cut_and_program_edits_are_steps_of_their_own(cx: &mut TestAppContext) {
    let (area, cx, _) = area(cx, "");
    cx.simulate_input("ab");
    cx.write_to_clipboard(ClipboardItem::new_string("PASTE".to_owned()));
    cx.simulate_keystrokes("ctrl-v");
    cx.simulate_input("c");
    assert_eq!(steps(&area, cx), 3);
    // Ctrl+/ владельца — правка программы, затем набор — новый шаг.
    area.update_in(cx, |area, window, cx| {
        area.apply_edit(0..0, "// ", 0..11, window, cx)
    });
    assert_eq!(text(&area, cx), "// abPASTEc");
    assert_eq!(selection(&area, cx), 0..11);
    cx.simulate_keystrokes("ctrl-end");
    cx.simulate_input("d");
    assert_eq!(text(&area, cx), "// abPASTEcd");
    assert_eq!(steps(&area, cx), 5);
    // Ctrl+X.
    area.update(cx, |area, cx| area.set_selection(0..3, cx));
    cx.simulate_keystrokes("ctrl-x");
    assert_eq!(text(&area, cx), "abPASTEcd");
    assert_eq!(steps(&area, cx), 6);
    cx.simulate_keystrokes("ctrl-z");
    assert_eq!(text(&area, cx), "// abPASTEcd");
    assert_eq!(selection(&area, cx), 0..3);
    // Новая правка стирает повтор.
    cx.simulate_input("z");
    assert!(!area.read_with(cx, |area, _| area.history.can_redo()));
}

#[gpui::test]
fn ime_composition_is_marked_and_undone_as_one_step(cx: &mut TestAppContext) {
    let (area, cx, _) = area(cx, "A");
    area.update(cx, |area, cx| area.set_selection(1..1, cx));
    area.update_in(cx, |area, window, cx| {
        area.replace_and_mark_text_in_range(None, "ni", Some(2..2), window, cx);
        assert_eq!(area.text(), "Ani");
        assert_eq!(area.marked_text_range(window, cx), Some(1..3));
        area.replace_and_mark_text_in_range(None, "nih", Some(3..3), window, cx);
        assert_eq!(area.selection(), 4..4);
        area.replace_text_in_range(None, "你好", window, cx);
        assert_eq!(area.text(), "A你好");
        assert_eq!(area.marked, None);
        assert_eq!(area.selection(), 7..7);
        // UTF-16 для IME: «你好» — две единицы.
        assert_eq!(area.text_length_utf16(window, cx), Some(3));
    });
    assert_eq!(steps(&area, cx), 1);
    cx.simulate_keystrokes("ctrl-z");
    assert_eq!(text(&area, cx), "A");
}

#[gpui::test]
fn paste_during_ime_composition_drops_it_and_keeps_undo_in_step(cx: &mut TestAppContext) {
    let (area, cx, _) = area(cx, "A");
    area.update(cx, |area, cx| area.set_selection(1..1, cx));
    area.update_in(cx, |area, window, cx| {
        area.replace_and_mark_text_in_range(None, "ni", Some(2..2), window, cx)
    });
    cx.write_to_clipboard(ClipboardItem::new_string("P".to_owned()));
    cx.simulate_keystrokes("ctrl-v");
    assert_eq!(text(&area, cx), "AP");
    // Конец составления IME — набор у каретки, своим шагом.
    area.update_in(cx, |area, window, cx| {
        area.replace_text_in_range(None, "你", window, cx)
    });
    assert_eq!(text(&area, cx), "AP你");
    assert_eq!(steps(&area, cx), 2);
    cx.simulate_keystrokes("ctrl-z ctrl-z");
    assert_eq!(text(&area, cx), "A");
}

#[gpui::test]
fn clipboard_keeps_lines_and_paste_normalizes_breaks(cx: &mut TestAppContext) {
    let (area, cx, _) = area(cx, "a\nb");
    cx.simulate_keystrokes("ctrl-a ctrl-c");
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("a\nb".to_owned())
    );
    cx.write_to_clipboard(ClipboardItem::new_string("x\r\n\ty\u{1b}\rz\0".to_owned()));
    cx.simulate_keystrokes("ctrl-a ctrl-v");
    assert_eq!(text(&area, cx), "x\n\ty\nz");
    assert_eq!(area.read_with(cx, |area, _| area.lines()), 3);
}

#[gpui::test]
fn mouse_click_drag_double_and_triple_click(cx: &mut TestAppContext) {
    let (area, cx, _) = area(cx, "bind w +forward\nalias go \"+jump\"\nexec x.cfg");
    let (from, to) = (at(&area, cx, 5), at(&area, cx, 20));
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    assert_eq!(selection(&area, cx), 5..20);
    // Мышь отпущена: движение больше не выделяет.
    let past = at(&area, cx, 30);
    cx.simulate_mouse_move(past, None, Modifiers::default());
    assert_eq!(selection(&area, cx), 5..20);
    // Отпускание не дошло: движение без кнопки выделение не тянет и после.
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(past, None, Modifiers::default());
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    assert_eq!(selection(&area, cx), 5..5);
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());

    let word = at(&area, cx, 18);
    for click_count in [1, 2] {
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: word,
            modifiers: Modifiers::default(),
            click_count,
            first_mouse: false,
        });
        cx.simulate_mouse_up(word, MouseButton::Left, Modifiers::default());
    }
    assert_eq!(selection(&area, cx), 16..21);
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: word,
        modifiers: Modifiers::default(),
        click_count: 3,
        first_mouse: false,
    });
    assert_eq!(
        selection(&area, cx),
        16..33,
        "строка вместе с переводом строки"
    );

    // Колонка номеров не трогает ни выделение, ни фокус.
    cx.update(|window, cx| window.blur(cx));
    let gutter = point(Pixels::from(4.), at(&area, cx, 0).y);
    cx.simulate_click(gutter, Modifiers::default());
    assert_eq!(selection(&area, cx), 16..33);
    assert!(!area.update_in(cx, |area, window, _| area.focus_handle.is_focused(window)));
}

#[gpui::test]
fn read_only_selects_and_copies_but_does_not_edit(cx: &mut TestAppContext) {
    let (area, cx, _) = area(cx, "HLSR-CODE");
    area.update(cx, |area, cx| area.set_read_only(true, cx));
    cx.run_until_parked();
    cx.simulate_input("x");
    cx.simulate_keystrokes("backspace ctrl-a ctrl-c ctrl-x ctrl-z");
    assert_eq!(text(&area, cx), "HLSR-CODE");
    assert_eq!(selection(&area, cx), 0..9);
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("HLSR-CODE".to_owned())
    );
    // Правка программы проходит.
    area.update_in(cx, |area, window, cx| {
        area.apply_edit(0..9, "new", 3..3, window, cx)
    });
    assert_eq!(text(&area, cx), "new");
}

/// 5000 строк cfg.
fn big_file() -> String {
    (1..=5000)
        .map(|n| format!("bind \"k{n}\" \"+cmd{n}; wait; -cmd{n}\" // строка {n}\n"))
        .collect()
}

#[gpui::test]
fn reveal_and_go_to_line_scroll_a_big_file(cx: &mut TestAppContext) {
    let text = big_file();
    let (area, cx, _) = area(cx, &text);
    frame(cx);
    let found = text.find("+cmd3000;").unwrap();
    area.update_in(cx, |area, window, cx| {
        area.reveal(found..found + 9, window, cx)
    });
    frame(cx);
    let (top, view, line) = area.update_in(cx, |area, window, _| {
        (
            f32::from(area.scroll.top()),
            f32::from(area.frame.view.size.height),
            metrics(window).line,
        )
    });
    // Строка 3000 — по центру видимого (`top - clientHeight / 2`).
    let expected = 2999. * line - view / 2.;
    assert!((top - expected).abs() < 1., "{top} != {expected}");
    assert_eq!(selection(&area, cx), found..found + 9);
    // Переход к строке: каретка в начале строки, номер — в пределах текста.
    area.update_in(cx, |area, window, cx| area.go_to_line(4000, window, cx));
    let start = text.match_indices('\n').nth(3998).unwrap().0 + 1;
    assert_eq!(selection(&area, cx), start..start);
    area.update_in(cx, |area, window, cx| area.go_to_line(99_999, window, cx));
    let last = text.len();
    assert_eq!(selection(&area, cx), last..last);
}

#[gpui::test]
fn a_5000_line_file_opens_and_scrolls_drawing_only_visible_lines(cx: &mut TestAppContext) {
    let text = big_file();
    let started = Instant::now();
    let (area, cx, _) = area(cx, &text);
    let opened = started.elapsed();
    let (view, line) = area.update_in(cx, |area, window, _| {
        (f32::from(area.frame.view.size.height), metrics(window).line)
    });
    let visible = (view / line).ceil() as usize + 2;
    let scroll = area.read_with(cx, |area, _| area.scroll.clone());
    let mut slowest = std::time::Duration::ZERO;
    for step in 1..=50 {
        let to = Pixels::from(step as f32 * 100. * line);
        cx.update(|window, cx| scroll.scroll_to(to, false, window, cx));
        let frame_start = Instant::now();
        frame(cx);
        slowest = slowest.max(frame_start.elapsed());
        let painted = area.read_with(cx, |area, _| area.frame.painted);
        assert!(painted <= visible, "{painted} строк из {visible} видимых");
    }
    // Замер для отчёта (debug, тестовая система текста).
    eprintln!(
        "5000 строк: открытие и первый кадр {opened:?}, самый долгий кадр прокрутки {slowest:?}"
    );
    assert!(slowest.as_millis() < 1000, "кадр {slowest:?}");
}

#[gpui::test]
fn long_lines_scroll_horizontally_with_the_caret_and_shift_wheel(cx: &mut TestAppContext) {
    let long = format!("// {}\nshort", "x".repeat(600));
    let (area, cx, _) = area(cx, &long);
    frame(cx);
    let max_x = area.read_with(cx, |area, _| area.frame.max_x);
    assert!(max_x > 0., "строка шире области");
    // Полоса — у низа области, а не под двумя строками текста.
    let (view, line) = area.update_in(cx, |area, window, _| {
        (area.frame.view, metrics(window).line)
    });
    assert!(f32::from(view.size.height) > 10. * line, "{view:?}");
    cx.simulate_keystrokes("end");
    frame(cx);
    let (scroll_x, text_w) = area.read_with(cx, |area, _| (area.scroll_x, area.frame.text_w));
    assert!(scroll_x > 0. && scroll_x <= max_x, "{scroll_x}");
    // Каретка у правого края видимого.
    let caret = area.update_in(cx, |area, window, _| {
        metrics(window).pad_x + area.x_of(area.cursor(), window)
    });
    assert!(
        caret <= scroll_x + text_w && caret >= scroll_x,
        "{caret} {scroll_x}"
    );
    cx.simulate_keystrokes("home");
    assert_eq!(area.read_with(cx, |area, _| area.scroll_x), 0.);
    // Shift + колесо: GPUI отдаёт его как x.
    let position = at(&area, cx, 1);
    cx.simulate_event(ScrollEvent {
        position,
        delta: ScrollDelta::Lines(point(-3., 0.)),
        ..Default::default()
    });
    cx.executor().advance_clock(Duration::from_micros(4_167));
    frame(cx);
    let intermediate = area.read_with(cx, |area, _| area.scroll_x);
    assert!(
        intermediate > 0. && intermediate < 3. * line_step(),
        "{intermediate}"
    );
    cx.executor().advance_clock(Duration::from_millis(220));
    frame(cx);
    let wheeled = area.read_with(cx, |area, _| area.scroll_x);
    assert!((wheeled - 3. * line_step()).abs() < 0.01, "{wheeled}");
}
