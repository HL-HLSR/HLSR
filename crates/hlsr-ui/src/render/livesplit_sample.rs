//! Примерный LiveSplit для превью оверлея, пока настоящий не запущен
//! (`livesplit-sample.js` 0.4.7, задача 33): стандартный макет (заголовок, сплиты,
//! таймер, предыдущий отрезок) в стандартных цветах, чтобы маски вели себя так же, как
//! на настоящем окне. Шрифт — `"Segoe UI", "Noto Sans", sans-serif` из ОС: сверка с
//! допуском по шрифту.

use super::canvas_text::{self, Align, Baseline, Font, Ink, Paint, Place};
use tiny_skia::{
    Color, GradientStop, LinearGradient, Paint as SkPaint, Pixmap, Point, Rect, Shader, SpreadMode,
    Transform,
};

pub const WIDTH: u32 = 280;
pub const HEIGHT: u32 = 332;

const SPLITS_GHOSTING: [(&str, &str, &str); 6] = [
    ("Point Insertion", "-0:04.1", "2:41.09"),
    ("Red Letter Day", "-0:01.6", "4:02.77"),
    ("Route Kanal", "+0:02.3", "9:18.40"),
    ("Water Hazard", "", "15:56.31"),
    ("Black Mesa East", "", "18:20.64"),
    ("Ravenholm", "", "24:47.02"),
];

const SPLITS_HL1: [(&str, &str, &str); 6] = [
    ("Black Mesa Inbound", "-0:00.8", "0:44.21"),
    ("Anomalous Materials", "-0:02.1", "1:52.66"),
    ("Unforeseen Consequences", "+0:01.4", "5:09.31"),
    ("Office Complex", "", "8:47.90"),
    ("We've Got Hostiles", "", "11:30.12"),
    ("Blast Pit", "", "15:02.58"),
];

/// Цвета стандартного макета LiveSplit.
const AHEAD: [u8; 3] = [0x00, 0xcc, 0x36];
const BEHIND: [u8; 3] = [0xcc, 0x12, 0x00];
const CURRENT: usize = 3;
const FAMILIES: &str = "\"Segoe UI\", \"Noto Sans\", sans-serif";

fn rgb(c: [u8; 3]) -> Color {
    Color::from_rgba8(c[0], c[1], c[2], 255)
}

fn gradient(top: f32, bottom: f32, from: [u8; 3], to: [u8; 3]) -> Option<Shader<'static>> {
    LinearGradient::new(
        Point::from_xy(0.0, top),
        Point::from_xy(0.0, bottom),
        vec![
            GradientStop::new(0.0, rgb(from)),
            GradientStop::new(1.0, rgb(to)),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    )
}

fn fill(canvas: &mut Pixmap, rect: (f32, f32, f32, f32), shader: Shader<'static>) {
    let Some(rect) = Rect::from_xywh(rect.0, rect.1, rect.2, rect.3) else {
        return;
    };
    let paint = SkPaint {
        shader,
        anti_alias: false,
        ..SkPaint::default()
    };
    canvas.fill_rect(rect, &paint, Transform::identity(), None);
}

struct TextStyle {
    size: f32,
    weight: u16,
    align: Align,
    color: [u8; 3],
}

const WHITE: [u8; 3] = [255, 255, 255];
const DIM: [u8; 3] = [0xd0, 0xd0, 0xd0];

fn text(canvas: &mut Pixmap, value: &str, x: f32, y: f32, style: TextStyle) {
    let font = Font::parse(&format!("{} {}px {FAMILIES}", style.weight, style.size));
    let [r, g, b] = style.color;
    canvas_text::text(
        canvas,
        value,
        &font,
        Place {
            x,
            y,
            align: style.align,
            baseline: Baseline::Middle,
            max_width: None,
        },
        Paint::Fill,
        &Ink::Solid([r, g, b, 255]),
        1.0,
    );
}

fn style(size: f32, align: Align, color: [u8; 3]) -> TextStyle {
    TextStyle {
        size,
        weight: 400,
        align,
        color,
    }
}

/// `drawSample(variant)`: `hl1` или (иначе) `ghosting`.
pub fn draw_sample(variant: &str) -> Option<Pixmap> {
    let mut canvas = Pixmap::new(WIDTH, HEIGHT)?;
    let hl1 = variant == "hl1";
    let splits = if hl1 { &SPLITS_HL1 } else { &SPLITS_GHOSTING };
    let (title, category) = if hl1 {
        ("Half-Life", "Any% Scriptless")
    } else {
        ("Half-Life 2: Ghosting", "Any%")
    };
    let (w, h) = (WIDTH as f32, HEIGHT as f32);

    // Фон окна — вертикальный градиент, как у стандартного макета.
    fill(
        &mut canvas,
        (0.0, 0.0, w, h),
        gradient(0.0, h, [0x2a, 0x2a, 0x2a], [0x0f, 0x0f, 0x0f])?,
    );
    text(
        &mut canvas,
        title,
        w / 2.0,
        18.0,
        style(14.0, Align::Center, WHITE),
    );
    text(
        &mut canvas,
        category,
        w / 2.0,
        37.0,
        style(13.0, Align::Center, DIM),
    );
    text(
        &mut canvas,
        "128",
        w - 8.0,
        37.0,
        style(12.0, Align::Right, DIM),
    );

    let (row_height, top) = (25.0, 54.0);
    for (index, (name, delta, time)) in splits.iter().enumerate() {
        let y = top + index as f32 * row_height;
        if index == CURRENT {
            fill(
                &mut canvas,
                (0.0, y, w, row_height),
                gradient(y, y + row_height, [0x33, 0x73, 0xf4], [0x15, 0x35, 0x74])?,
            );
        }
        let middle = y + row_height / 2.0;
        text(
            &mut canvas,
            name,
            8.0,
            middle,
            style(13.0, Align::Left, WHITE),
        );
        if !delta.is_empty() {
            let color = if delta.starts_with('-') {
                AHEAD
            } else {
                BEHIND
            };
            text(
                &mut canvas,
                delta,
                w - 72.0,
                middle,
                style(13.0, Align::Right, color),
            );
        }
        text(
            &mut canvas,
            time,
            w - 8.0,
            middle,
            style(13.0, Align::Right, WHITE),
        );
    }

    let timer_top = top + splits.len() as f32 * row_height + 10.0;
    fill(
        &mut canvas,
        (0.0, timer_top - 6.0, w, 1.0),
        Shader::SolidColor(Color::from_rgba(1.0, 1.0, 1.0, 0.08)?),
    );
    // Большой таймер: целые секунды крупно, доли — мельче, как в LiveSplit.
    let timer_y = timer_top + 38.0;
    let mut ink = Pixmap::new(WIDTH, HEIGHT)?;
    fill(
        &mut ink,
        (0.0, 0.0, w, h),
        gradient(timer_y - 24.0, timer_y + 24.0, [0x66, 0xff, 0x8f], AHEAD)?,
    );
    let small = Font::parse(&format!("600 30px {FAMILIES}"));
    let big = Font::parse(&format!("600 46px {FAMILIES}"));
    let fraction = ".47";
    let fraction_width = canvas_text::layout(fraction, &small).width;
    let place = |x: f32, y: f32| Place {
        x,
        y,
        align: Align::Right,
        baseline: Baseline::Middle,
        max_width: None,
    };
    canvas_text::text(
        &mut canvas,
        fraction,
        &small,
        place(w - 8.0, timer_y + 6.0),
        Paint::Fill,
        &Ink::Image(&ink),
        1.0,
    );
    canvas_text::text(
        &mut canvas,
        "11:02",
        &big,
        place(w - 8.0 - fraction_width, timer_y),
        Paint::Fill,
        &Ink::Image(&ink),
        1.0,
    );

    let info_top = timer_y + 40.0;
    text(
        &mut canvas,
        "Previous Segment",
        8.0,
        info_top,
        style(13.0, Align::Left, WHITE),
    );
    text(
        &mut canvas,
        "-1.62",
        w - 8.0,
        info_top,
        style(13.0, Align::Right, AHEAD),
    );
    text(
        &mut canvas,
        "Sum of Best",
        8.0,
        info_top + 22.0,
        style(13.0, Align::Left, WHITE),
    );
    text(
        &mut canvas,
        "1:19:44.08",
        w - 8.0,
        info_top + 22.0,
        style(13.0, Align::Right, WHITE),
    );
    Some(canvas)
}
