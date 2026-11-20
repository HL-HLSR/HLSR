//! `RichText` (решение 4 спецификации ПП5, карта §0.2): текст со
//! строчными стилями, ссылками, плашками `code` и `kbd`, списками, блоками
//! кода и картинками — то, что 0.4.7 рисовала через `v-html` (новости, описания
//! гайда) и кнопки-ссылки внутри фразы (согласие в `WelcomeModal`/`ProfileModal`).
//!
//! Здесь — документ ([`Doc`]) и таблицы стилей; раскладку и отрисовку делает вид.
//!
//! Таблицы стилей ([`Sheet`]) — CSS 0.4.7 мест, где текст был богатым:
//! `NewsPost.vue` (.news-post__body), `GameScripts.vue` (.gsc-usage, .gsc-notes,
//! .gsc-kbd), `WelcomeModal.vue`/`ProfileModal.vue` (согласие).

// ------------------------------------------------------------------ документ

/// Документ: блоки и цели ссылок (индекс — `Style::link`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Doc {
    pub blocks: Vec<Block>,
    pub links: Vec<Link>,
}

/// Куда ведёт ссылка.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    /// `data-script`: другой скрипт гайда.
    Script(String),
    /// `data-href`: внешняя ссылка `https://`.
    Href(String),
    /// Кнопка внутри фразы (согласие): значение задаёт экран.
    Action(String),
}

impl Link {
    pub fn kind(&self) -> &'static str {
        match self {
            Link::Script(_) => "script",
            Link::Href(_) => "href",
            Link::Action(_) => "action",
        }
    }

    pub fn value(&self) -> &str {
        match self {
            Link::Script(value) | Link::Href(value) | Link::Action(value) => value,
        }
    }
}

/// Блок потока.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// `p`.
    Paragraph(Vec<Inline>),
    /// `h3`–`h6`.
    Heading(u8, Vec<Inline>),
    /// `ul` / `ol`.
    List(List),
    /// `pre > code`.
    Code(String),
    /// Строчное содержимое без полей: текст пункта списка, заметка гайда, фраза.
    Inline(Vec<Inline>),
}

/// Список; пункт — блоки (`Inline` — его текст, `List` — вложенный список).
#[derive(Debug, Clone, PartialEq)]
pub struct List {
    pub ordered: bool,
    /// `start` у `ol` (как его прочтёт HTML: целая часть записи числа).
    pub start: Option<f64>,
    pub items: Vec<Vec<Block>>,
}

/// Строчный узел.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text {
        text: String,
        style: Style,
    },
    /// `<br>`.
    Break,
    /// Картинка (`display: block` — делит строчное содержимое).
    Image {
        src: String,
        alt: String,
    },
}

/// Строчный стиль: `strong`, `em`, `code`, `kbd`, ссылка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Style {
    pub strong: bool,
    pub em: bool,
    pub code: bool,
    /// `kbd` таблицы стилей: плашка `inline-block`.
    pub kbd: bool,
    /// `inline-block` (кнопка-ссылка): внутри не переносится.
    pub atomic: bool,
    pub link: Option<u32>,
}

impl Doc {
    /// `textContent` того же HTML: текст без разметки, `<br>` и картинки — пусто.
    pub fn text_content(&self) -> String {
        fn inlines(out: &mut String, nodes: &[Inline]) {
            for node in nodes {
                if let Inline::Text { text, .. } = node {
                    out.push_str(text);
                }
            }
        }
        fn blocks(out: &mut String, list: &[Block]) {
            for block in list {
                match block {
                    Block::Paragraph(nodes) | Block::Heading(_, nodes) | Block::Inline(nodes) => {
                        inlines(out, nodes)
                    }
                    Block::List(list) => {
                        for item in &list.items {
                            blocks(out, item);
                        }
                    }
                    Block::Code(code) => out.push_str(code),
                }
            }
        }
        let mut out = String::new();
        blocks(&mut out, &self.blocks);
        out
    }
}

// ------------------------------------------------------------- таблицы стилей

/// Цвет темы для текста и полос (номер — поле `role` структур Slint).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// `--l-text`.
    Text = 0,
    /// `--l-text-dim`.
    TextDim = 1,
    /// `--l-text-faint`.
    TextFaint = 2,
    /// `--l-accent`.
    LAccent = 3,
    /// `--accent`.
    Accent = 4,
}

/// `line-height`: в px (наследуется числом) или множитель (от своего кегля).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineHeight {
    Px(f32),
    Factor(f32),
}

impl LineHeight {
    pub fn px(self, size: f32) -> f32 {
        match self {
            LineHeight::Px(px) => px,
            LineHeight::Factor(factor) => factor * size,
        }
    }
}

/// Подчёркивание ссылки.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Underline {
    None,
    /// `text-decoration: underline`; `offset` — `text-underline-offset`.
    Always {
        offset: Option<f32>,
    },
    /// Только при наведении (`a:hover { text-decoration: underline }`).
    Hover {
        offset: Option<f32>,
    },
}

/// Кегль `code`: в px или в долях кегля блока (`.93em`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CodeSize {
    Px(f32),
    Em(f32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CodeStyle {
    pub family: &'static str,
    pub size: CodeSize,
    pub pad_x: f32,
    pub pad_y: f32,
    pub radius: f32,
    pub background: [u8; 4],
    pub color: Option<Role>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreStyle {
    /// Поля сверху и снизу; `None` — UA `margin: 1em 0` (1em кегля блока).
    pub margins: Option<(f32, f32)>,
    pub pad_x: f32,
    pub pad_y: f32,
    pub radius: f32,
    pub background: [u8; 4],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ListStyle {
    pub padding_left: f32,
    pub margins: (f32, f32),
    /// Вложенный список (`li > ul`).
    pub nested_margins: (f32, f32),
    pub item_margins: (f32, f32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeadingStyle {
    /// Кегли h3, h4, h5, h6.
    pub sizes: [f32; 4],
    pub margins: (f32, f32),
    pub line_height: f32,
    pub color: Role,
    pub weight: u16,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageStyle {
    pub max_width: f32,
    pub margins: (f32, f32),
    pub radius: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KbdStyle {
    pub min_width: f32,
    pub pad_x: f32,
    pub radius: f32,
    pub background: [u8; 4],
    /// `box-shadow: inset 0 -1px 0 …`.
    pub shade: [u8; 4],
    pub color: Role,
    pub weight: u16,
}

/// Таблица стилей места в 0.4.7.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sheet {
    pub family: &'static str,
    pub size: f32,
    pub weight: u16,
    pub line_height: LineHeight,
    /// Наследуется от body: `0.01em` × 14px = 0.14px.
    pub letter_spacing: f32,
    pub color: Role,
    /// `max-width` в `ch` (ширина «0» шрифта блока).
    pub max_width_ch: Option<f32>,
    /// `overflow-wrap: anywhere`.
    pub break_anywhere: bool,
    pub paragraph: (f32, f32),
    pub heading: HeadingStyle,
    pub strong_weight: u16,
    pub strong_color: Option<Role>,
    pub link_color: Role,
    pub underline: Underline,
    pub code: CodeStyle,
    pub pre: PreStyle,
    pub list: ListStyle,
    pub image: ImageStyle,
    pub kbd: KbdStyle,
}

const fn rgba(r: u8, g: u8, b: u8, alpha: f32) -> [u8; 4] {
    // Доля CSS → байт с округлением (как цвет Slint из CSS-значения).
    [r, g, b, (alpha * 255.0 + 0.5) as u8]
}

const CASCADIA: &str = "Cascadia Mono";

/// UA: h5 — 0.83em, h6 — 0.67em от 14px.
const UA_H5_14: f32 = 14.0 * 0.83;
const UA_H6_14: f32 = 14.0 * 0.67;

const NO_KBD: KbdStyle = KbdStyle {
    min_width: 18.0,
    pad_x: 5.0,
    radius: 4.0,
    background: rgba(255, 255, 255, 0.09),
    shade: rgba(0, 0, 0, 0.35),
    color: Role::Text,
    weight: 600,
};

impl Sheet {
    /// `.news-post__body` (`NewsPost.vue`): 14/23, `--l-text-dim`, 90ch.
    pub const NEWS: Sheet = Sheet {
        family: "Rubik",
        size: 14.0,
        weight: 400,
        line_height: LineHeight::Px(23.0),
        letter_spacing: 0.14,
        color: Role::TextDim,
        max_width_ch: Some(90.0),
        break_anywhere: true,
        paragraph: (12.0, 12.0),
        heading: HeadingStyle {
            sizes: [18.0, 16.0, UA_H5_14, UA_H6_14],
            margins: (20.0, 8.0),
            line_height: 1.4,
            color: Role::Text,
            weight: 700,
        },
        strong_weight: 700,
        strong_color: Some(Role::Text),
        link_color: Role::LAccent,
        underline: Underline::Always { offset: Some(3.0) },
        code: CodeStyle {
            family: CASCADIA,
            size: CodeSize::Em(0.93),
            pad_x: 5.0,
            pad_y: 2.0,
            radius: 4.0,
            background: rgba(255, 255, 255, 0.07),
            color: Some(Role::Text),
        },
        // App.vue: `* { margin: 0 }` снимает поля UA у pre.
        pre: PreStyle {
            margins: Some((0.0, 0.0)),
            pad_x: 16.0,
            pad_y: 12.0,
            radius: 6.0,
            background: rgba(0, 0, 0, 0.2),
        },
        list: ListStyle {
            padding_left: 24.0,
            margins: (12.0, 12.0),
            nested_margins: (12.0, 12.0),
            item_margins: (0.0, 0.0),
        },
        image: ImageStyle {
            max_width: f32::INFINITY,
            margins: (0.0, 0.0),
            radius: 0.0,
        },
        kbd: NO_KBD,
    };

    /// `.gsc-usage` (`GameScripts.vue`): 13px × 1.55, `--l-text`.
    pub const GUIDE: Sheet = Sheet {
        family: "Rubik",
        size: 13.0,
        weight: 400,
        line_height: LineHeight::Factor(1.55),
        letter_spacing: 0.14,
        color: Role::Text,
        max_width_ch: None,
        break_anywhere: true,
        paragraph: (0.0, 8.0),
        heading: Sheet::NEWS.heading,
        strong_weight: 600,
        strong_color: None,
        link_color: Role::Accent,
        underline: Underline::Hover { offset: None },
        code: CodeStyle {
            family: CASCADIA,
            size: CodeSize::Px(12.0),
            pad_x: 4.0,
            pad_y: 0.0,
            radius: 4.0,
            background: rgba(255, 255, 255, 0.07),
            color: Some(Role::Text),
        },
        pre: PreStyle {
            margins: Some((0.0, 8.0)),
            pad_x: 10.0,
            pad_y: 8.0,
            radius: 6.0,
            background: rgba(0, 0, 0, 0.3),
        },
        list: ListStyle {
            padding_left: 20.0,
            margins: (0.0, 8.0),
            nested_margins: (2.0, 4.0),
            item_margins: (2.0, 2.0),
        },
        image: ImageStyle {
            max_width: 480.0,
            margins: (6.0, 6.0),
            radius: 6.0,
        },
        kbd: NO_KBD,
    };

    /// Заметка гайда (`.gsc-notes li`): 12px × 1.4, `--l-text-dim`, без
    /// `overflow-wrap`; `code` и ссылки — как у `.gsc-usage`.
    pub const GUIDE_NOTE: Sheet = Sheet {
        size: 12.0,
        line_height: LineHeight::Factor(1.4),
        color: Role::TextDim,
        break_anywhere: false,
        strong_weight: 700,
        ..Sheet::GUIDE
    };

    /// Согласие (`.welcome__consent`, `.runner-signin__consent`): 12.5/20,
    /// `--l-text-dim`; кнопки-ссылки `--l-accent`, подчёркнуты на 3px ниже.
    pub const CONSENT: Sheet = Sheet {
        size: 12.5,
        line_height: LineHeight::Px(20.0),
        color: Role::TextDim,
        break_anywhere: false,
        link_color: Role::LAccent,
        underline: Underline::Always { offset: Some(3.0) },
        ..Sheet::GUIDE
    };

    /// По имени варианта разметки (`RichSource.variant`); незнакомое — согласие.
    pub fn by_name(name: &str) -> &'static Sheet {
        match name {
            "news" => &Sheet::NEWS,
            "guide" => &Sheet::GUIDE,
            "guide-note" => &Sheet::GUIDE_NOTE,
            _ => &Sheet::CONSENT,
        }
    }

    pub fn code_size(&self, block_size: f32) -> f32 {
        match self.code.size {
            CodeSize::Px(px) => px,
            CodeSize::Em(em) => em * block_size,
        }
    }
}
