//! Страница «Демо» (`DemoPanel.vue` 0.4.9) без окна: вкладки «Демки / Отрендеренные», список по
//! играм, выбранная запись, экраны поверх плеера, интервал экспорта, задания предпросмотра и
//! экспорта, установка FFmpeg и bxt-rs, видео папки вывода.
//!
//! Каналы зовёт вид и отдаёт ответы сюда (`*_done`), как `await ipcRenderer.invoke(…)`
//! компонента; события `demo-job-*`, `demo-ffmpeg-progress`, `demo-bxtrs-progress` — в
//! [`DemoPanel::on_event`]. Ответы каналов — значения JSON (`{ status: "ok", … }` или
//! `{ status: "error", code, … }`); `Err(())` — исключение `invoke`.

use super::boot::{Translate, format_size};
use super::demo_tool::{
    self, ClipRange, Edge, PlayerInput, RenderedWhich, clock, error_text, first_pick, format_clock,
    game_name, group_items, group_name, has_demo_extension, mark_range, player_input,
};
use super::game_demos::format_date_local;
use super::jsv;
use crate::intl::{Plural, plural};
use hlsr_core::js;
use hlsr_core::language::Language;
use serde_json::{Value, json};

/// Ответ канала: значение или исключение `invoke`.
pub type Answer = Result<Value, ()>;

/// Вкладка под заголовком.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Demos,
    Rendered,
}

/// Вид задания.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Preview,
    Export,
}

impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            JobKind::Preview => "preview",
            JobKind::Export => "export",
        }
    }

    fn of(value: Option<&Value>) -> Option<JobKind> {
        match jsv::str_of(value)? {
            "preview" => Some(JobKind::Preview),
            "export" => Some(JobKind::Export),
            _ => None,
        }
    }
}

/// Идущее задание: `{ jobId, kind, itemId, phase, percent, segment, message }`.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub job_id: Option<Value>,
    pub kind: JobKind,
    pub item_id: String,
    pub phase: String,
    pub percent: f64,
    pub segment: Value,
    pub message: Option<String>,
}

/// Не подошедшие файлы: имя и причина.
pub type Rejected = Vec<(String, String)>;

/// Строка над списком: итог добавления или ошибка.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub text: String,
    pub rejected: Rejected,
    pub error: bool,
}

impl Notice {
    fn error(text: String) -> Notice {
        Notice {
            text,
            rejected: Vec::new(),
            error: true,
        }
    }
}

/// Итог последнего экспорта записи: видео или ошибка.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub item_id: String,
    pub result: Option<Value>,
    pub error: Option<Value>,
}

/// Что показать поверх экрана плеера (`screen`); `None` у [`DemoPanel::screen`] — само видео.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Empty,
    Unsupported,
    NotReady,
    Bxtrs,
    Ffmpeg,
    PreviewJob,
    PreviewError,
    MediaError,
    Ready,
}

/// Вкладка «Отрендеренные»: список папки вывода и открытое видео.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rendered {
    pub items: Vec<Value>,
    pub output_dir: String,
    pub loading: bool,
    /// `{ name, url, video, source, markers, sourceId }`.
    pub open: Option<Value>,
    pub media_error: bool,
    /// Имя видео в окне подтверждения удаления.
    pub confirm: Option<String>,
    pub deleting: bool,
}

/// После ответа списка: выбрать запись или перечитать выбранную.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    Nothing,
    Select(String),
    Inspect(String),
}

/// Тост оболочки (`$emit("notify")`): текст и значок.
pub type Toast = (String, &'static str);

/// Что делать виду после события задания.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JobEffect {
    pub toast: Option<Toast>,
    /// Новое видео в папке вывода: перечитать «Отрендеренные».
    pub reload_rendered: bool,
    /// Предпросмотр готов: фокус плееру.
    pub focus_player: bool,
}

/// Что вернуть оболочке при пересоздании страницы (смена языка): задание, итог экспорта и
/// готовое видео (`carried` 0.4.9).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Carried {
    pub job: Option<Job>,
    pub export_outcome: Option<Outcome>,
    pub media: Option<Value>,
    pub card_hidden: bool,
}

/// Состояние страницы (`data()`).
#[derive(Debug, Clone, PartialEq)]
pub struct DemoPanel {
    /// Нижняя карточка свёрнута (кнопка посередине).
    pub card_hidden: bool,
    /// Меняется только через `set_items`/`regroup`: `groups` — их кэш для отрисовки.
    items: Vec<Value>,
    groups: Vec<demo_tool::Group>,
    pub loading: bool,
    pub selected_id: Option<String>,
    /// Ответ `demo-inspect`: `{ item, segments, markers, render }`.
    pub detail: Option<Value>,
    pub detail_loading: bool,
    pub reordering: bool,
    pub removing: bool,
    pub notice: Option<Notice>,
    pub dragging: bool,
    /// Интервал экспорта во времени демо выбранной записи; `None` — весь ран.
    pub range: Option<ClipRange>,
    /// Подсказка у строки интервала (начало и конец поменялись местами).
    pub range_note: Option<&'static str>,
    /// Последняя отмена: запись и вид задания.
    pub cancelled: Option<(String, JobKind)>,
    pub ffmpeg: Option<Value>,
    pub ffmpeg_error: Option<String>,
    pub bxtrs: Option<Value>,
    pub bxtrs_error: Option<String>,
    pub job: Option<Job>,
    /// Видео в плеере: `{ itemId, result }` (предпросмотр или готовый экспорт).
    pub media: Option<Value>,
    pub media_error: bool,
    /// Ошибка предпросмотра: `{ itemId, code, … }`.
    pub preview_error: Option<Value>,
    pub export_outcome: Option<Outcome>,
    pub export_open: bool,
    pub player_expanded: bool,
    pub active: bool,
    /// Окно экспорта вместо долгого предпросмотра: `{ itemId, seconds, range, segments }`.
    pub long_preview: Option<Value>,
    /// Стартовые настройки окна экспорта (из «Отрендеренных»): `{ speed }`.
    pub export_initial: Option<Value>,
    pub tab: Tab,
    /// Вид предпросмотра: `{ keyhud, hud, ownConfig, ownHud }`.
    pub preview_view: Value,
    pub rendered: Rendered,
}

impl Default for DemoPanel {
    fn default() -> Self {
        DemoPanel::new(Carried::default())
    }
}

fn status_ok(answer: &Value) -> bool {
    jsv::is_str(jsv::prop(answer, "status"), "ok")
}

fn ok(answer: &Answer) -> Option<&Value> {
    answer.as_ref().ok().filter(|answer| status_ok(answer))
}

fn text_of(value: Option<&Value>) -> String {
    value.map(js::to_string).unwrap_or_default()
}

fn array_of(value: Option<&Value>) -> Vec<Value> {
    value.and_then(Value::as_array).cloned().unwrap_or_default()
}

/// Ответ-ошибка или `{}` (`answer || {}`).
fn error_of(answer: &Answer) -> Value {
    match answer {
        Ok(value) if js::truthy(value) => value.clone(),
        _ => json!({}),
    }
}

fn code(code: &str) -> Value {
    json!({ "code": code })
}

impl DemoPanel {
    pub fn new(carried: Carried) -> DemoPanel {
        DemoPanel {
            card_hidden: carried.card_hidden,
            items: Vec::new(),
            groups: Vec::new(),
            loading: false,
            selected_id: None,
            detail: None,
            detail_loading: false,
            reordering: false,
            removing: false,
            notice: None,
            dragging: false,
            range: None,
            range_note: None,
            cancelled: None,
            ffmpeg: None,
            ffmpeg_error: None,
            bxtrs: None,
            bxtrs_error: None,
            job: carried.job,
            media: carried.media,
            media_error: false,
            preview_error: None,
            export_outcome: carried.export_outcome,
            export_open: false,
            player_expanded: false,
            active: false,
            long_preview: None,
            export_initial: None,
            tab: Tab::Demos,
            preview_view: json!({"keyhud": true, "hud": true, "ownConfig": true, "ownHud": true}),
            rendered: Rendered::default(),
        }
    }

    /// `beforeUnmount`: что переходит в новую страницу (предпросмотр снят).
    pub fn carried(&self) -> Carried {
        let file = |media: &&Value| jsv::truthy(jsv::field(jsv::prop(media, "result"), "file"));
        Carried {
            job: self.job.clone(),
            export_outcome: self.export_outcome.clone(),
            media: self.media.as_ref().filter(file).cloned(),
            card_hidden: self.card_hidden,
        }
    }

    // ---------- вычисляемое ----------

    pub fn items(&self) -> &[Value] {
        &self.items
    }

    /// Группы списка: страница рисует их каждый кадр, поэтому кэш, а не `group_items` заново.
    pub fn groups(&self) -> &[demo_tool::Group] {
        &self.groups
    }

    fn set_items(&mut self, items: Option<&Value>) {
        self.items = array_of(items);
        self.regroup();
    }

    fn regroup(&mut self) {
        self.groups = group_items(&self.items);
    }

    pub fn list_busy(&self) -> bool {
        match self.tab {
            Tab::Rendered => self.rendered.loading,
            Tab::Demos => self.loading,
        }
    }

    /// Запись выбранной демки (`detail.item`).
    pub fn item(&self) -> Option<&Value> {
        jsv::field(self.detail.as_ref(), "item")
    }

    fn item_id(&self) -> Option<String> {
        self.item().map(|item| text_of(jsv::prop(item, "id")))
    }

    fn is_selected_item(&self, id: &str) -> bool {
        self.item_id().as_deref() == Some(id)
    }

    /// Плеер видит только видео выбранной записи (иначе шкалу её демки).
    pub fn player(&self) -> PlayerInput {
        player_input(self.media.as_ref(), self.detail.as_ref())
    }

    /// Длина выбранной демки для шкалы (`detail.item.duration || 0`).
    pub fn duration(&self) -> f64 {
        jsv::or_zero(jsv::field(self.item(), "duration"))
    }

    /// Идёт экспорт выбранной записи.
    pub fn own_export(&self) -> bool {
        self.job
            .as_ref()
            .is_some_and(|job| job.kind == JobKind::Export && self.is_selected_item(&job.item_id))
    }

    /// Итог экспорта выбранной записи.
    pub fn own_outcome(&self) -> Option<&Outcome> {
        self.export_outcome
            .as_ref()
            .filter(|outcome| self.is_selected_item(&outcome.item_id))
    }

    /// Идёт экспорт записи `id` (спиннер в списке).
    pub fn exporting(&self, id: &str) -> bool {
        self.job
            .as_ref()
            .is_some_and(|job| job.kind == JobKind::Export && job.item_id == id)
    }

    /// Название игры записи для текста «не готово».
    pub fn item_game_name(&self) -> String {
        let Some(item) = self.item() else {
            return String::new();
        };
        group_name(&text_of(jsv::prop(item, "gameId")))
            .unwrap_or_else(|| game_name(&text_of(jsv::prop(item, "game"))))
            .to_owned()
    }

    /// Метки в карточке — всегда во времени демо: `(метка, время демо)`.
    pub fn side_markers(&self) -> Vec<(Value, f64)> {
        if self.detail.is_none() {
            return Vec::new();
        }
        let player = self.player();
        let field = if player.own { "sourceTime" } else { "t" };
        player
            .markers
            .into_iter()
            .map(|marker| {
                let time = jsv::num(jsv::prop(&marker, field)).unwrap_or(f64::NAN);
                (marker, time)
            })
            .collect()
    }

    pub fn ffmpeg_ready(&self) -> bool {
        jsv::is_str(jsv::field(self.ffmpeg.as_ref(), "state"), "ready")
    }

    fn render_ok(&self) -> bool {
        jsv::truthy(jsv::field(jsv::field(self.detail.as_ref(), "render"), "ok"))
    }

    fn render_code(&self) -> String {
        text_of(jsv::field(
            jsv::field(self.detail.as_ref(), "render"),
            "code",
        ))
    }

    fn item_supported(&self) -> bool {
        jsv::truthy(jsv::field(self.item(), "supported"))
    }

    pub fn can_preview(&self) -> bool {
        self.detail.is_some()
            && self.item_supported()
            && self.render_ok()
            && self.ffmpeg_ready()
            && self.job.is_none()
    }

    /// Что показать поверх экрана плеера; `None` — само видео.
    pub fn screen(&self) -> Option<Screen> {
        let Some(id) = self.item_id().filter(|_| self.detail.is_some()) else {
            return Some(Screen::Empty);
        };
        if self
            .job
            .as_ref()
            .is_some_and(|job| job.kind == JobKind::Preview && job.item_id == id)
        {
            return Some(Screen::PreviewJob);
        }
        if self.player().own {
            return self.media_error.then_some(Screen::MediaError);
        }
        if !self.item_supported() {
            return Some(Screen::Unsupported);
        }
        if !self.render_ok() {
            return Some(
                if self.render_code() == "BXTRS_MISSING" && self.bxtrs.is_some() {
                    Screen::Bxtrs
                } else {
                    Screen::NotReady
                },
            );
        }
        if self.ffmpeg.is_some() && !self.ffmpeg_ready() {
            return Some(Screen::Ffmpeg);
        }
        if jsv::field(self.preview_error.as_ref(), "itemId") == Some(&json!(id)) {
            return Some(Screen::PreviewError);
        }
        Some(Screen::Ready)
    }

    /// Отмена предпросмотра выбранной записи («Предпросмотр отменён» на экране).
    pub fn preview_cancelled(&self) -> bool {
        self.cancelled
            .as_ref()
            .is_some_and(|(id, kind)| *kind == JobKind::Preview && self.is_selected_item(id))
    }

    /// Факты карточки: подпись и значение.
    pub fn facts(&self, t: Translate) -> Vec<(String, String)> {
        let Some(item) = self.item() else {
            return Vec::new();
        };
        let get = |name: &str| jsv::prop(item, name);
        let or_dash = |text: String| {
            if text.is_empty() {
                "—".to_owned()
            } else {
                text
            }
        };
        let mut facts = vec![(
            t("#L_DEMO_F_GAME", &[]),
            or_dash(game_name(&text_of(get("game"))).to_owned()),
        )];
        let maps = array_of(get("maps"));
        if maps.len() > 1 {
            let maps: Vec<String> = maps.iter().map(js::to_string).collect();
            facts.push((t("#L_DEMO_F_MAPS", &[]), maps.join(", ")));
        } else {
            let map = get("map").filter(|map| js::truthy(map)).map(js::to_string);
            facts.push((t("#L_DEMO_F_MAP", &[]), map.unwrap_or_else(|| "—".into())));
        }
        let duration = jsv::or_zero(get("duration"));
        facts.push((
            t("#L_DEMO_F_LENGTH", &[]),
            if duration != 0. && !duration.is_nan() {
                clock(duration)
            } else {
                "—".into()
            },
        ));
        if jsv::truthy(get("runTime")) {
            facts.push((
                t("#L_DEMO_F_TIMER", &[]),
                clock(jsv::or_zero(get("runTime"))),
            ));
        }
        let segments = jsv::or_zero(get("segmentCount"));
        if segments > 1. {
            facts.push((
                t("#L_DEMO_F_SEGMENTS", &[]),
                js::number::number_to_string(segments),
            ));
        }
        // Серия из папки игры: каких номеров нет (удалены или не записались).
        let missing = array_of(get("missing"));
        if jsv::truthy(get("series")) && !missing.is_empty() {
            let missing: Vec<String> = missing.iter().map(js::to_string).collect();
            facts.push((t("#L_DEMO_F_MISSING", &[]), missing.join(", ")));
        }
        facts.push((
            t("#L_DEMO_F_DATE", &[]),
            format_date_local(jsv::num(get("mtimeMs")).unwrap_or(f64::NAN)),
        ));
        let size = jsv::or_zero(get("size"));
        facts.push((
            t("#L_DEMO_F_SIZE", &[]),
            if size != 0. && !size.is_nan() {
                format_size(size, t)
            } else {
                "—".into()
            },
        ));
        facts
    }

    /// Факты открытого видео «Отрендеренных».
    pub fn rendered_facts(&self, t: Translate) -> Vec<(String, String)> {
        let Some(open) = &self.rendered.open else {
            return Vec::new();
        };
        let name = jsv::prop(open, "name");
        let item = self
            .rendered
            .items
            .iter()
            .find(|video| jsv::prop(video, "name") == name)
            .cloned()
            .unwrap_or(json!({}));
        let video = jsv::prop(open, "video");
        let source = jsv::prop(open, "source");
        let mut facts = vec![(
            t("#L_DEMO_F_GAME", &[]),
            match game_name(&text_of(jsv::field(source, "game"))) {
                "" => "—".to_owned(),
                name => name.to_owned(),
            },
        )];
        facts.push((
            t("#L_DEMO_F_MAP", &[]),
            jsv::field(source, "map")
                .filter(|map| js::truthy(map))
                .map_or_else(|| "—".to_owned(), js::to_string),
        ));
        let duration = jsv::or_zero(jsv::field(video, "duration"));
        facts.push((
            t("#L_DEMO_F_LENGTH", &[]),
            if duration != 0. && !duration.is_nan() {
                clock(duration)
            } else {
                "—".into()
            },
        ));
        let (width, height) = (jsv::field(video, "width"), jsv::field(video, "height"));
        if jsv::truthy(width) && jsv::truthy(height) {
            let fps = jsv::field(video, "fps").filter(|fps| js::truthy(fps));
            facts.push((
                t("#L_DEMO_F_VIDEO", &[]),
                format!(
                    "{}×{}{}",
                    text_of(width),
                    text_of(height),
                    fps.map_or_else(String::new, |fps| format!(", {} FPS", js::to_string(fps)))
                ),
            ));
        }
        let speed = jsv::or_zero(jsv::field(video, "speed"));
        if speed != 0. && !speed.is_nan() && speed != 1. {
            let text = js::number::number_to_string(speed).replacen('.', &t("#L_DECIMAL", &[]), 1);
            facts.push((t("#L_DEMO_F_SPEED", &[]), format!("{text}×")));
        }
        if jsv::truthy(jsv::prop(&item, "mtimeMs")) {
            facts.push((
                t("#L_DEMO_F_DATE", &[]),
                format_date_local(jsv::or_zero(jsv::prop(&item, "mtimeMs"))),
            ));
        }
        if jsv::truthy(jsv::prop(&item, "size")) {
            facts.push((
                t("#L_DEMO_F_SIZE", &[]),
                format_size(jsv::or_zero(jsv::prop(&item, "size")), t),
            ));
        }
        facts
    }

    // ---------- подписи ----------

    /// `reasonText(item)`: почему запись не поддерживается.
    pub fn reason_text(item: &Value, t: Translate) -> String {
        let reason = jsv::str_of(jsv::prop(item, "reason"))
            .filter(|reason| !reason.is_empty())
            .unwrap_or("DEMO_UNSUPPORTED");
        let failed = jsv::prop(item, "failedSegment").filter(|value| js::truthy(value));
        // У SEGMENT_* номер уже в тексте.
        if reason.starts_with("SEGMENT_") {
            let mut error = json!({ "code": reason });
            if let Some(failed) = failed {
                error["segment"] = failed.clone();
            }
            return error_text(&error, t);
        }
        let text = error_text(&code(reason), t);
        match failed {
            // «Сегмент N:» — только у рана из нескольких файлов.
            Some(failed) if jsv::or_zero(jsv::prop(item, "segmentCount")) > 1. => {
                t("#L_DEMO_SEGMENT_PREFIX", &[&js::to_string(failed), &text])
            }
            _ => text,
        }
    }

    /// `itemName(item)`: серия HL1 из папки игры — «hlsrrun (60 демок)».
    pub fn item_name(item: &Value, lang: Language, t: Translate) -> String {
        let name = text_of(jsv::prop(item, "name"));
        if !jsv::truthy(jsv::prop(item, "series")) {
            return name;
        }
        let count = jsv::num(jsv::prop(item, "segmentCount")).unwrap_or(f64::NAN);
        let key = match plural(lang, count) {
            Plural::One => "#L_DEMO_SERIES_ONE",
            Plural::Few => "#L_DEMO_SERIES_FEW",
            _ => "#L_DEMO_SERIES_MANY",
        };
        t(key, &[&name, &text_of(jsv::prop(item, "segmentCount"))])
    }

    /// `itemMeta(item)`: игра и карта.
    pub fn item_meta(item: &Value) -> String {
        let game = game_name(&text_of(jsv::prop(item, "game")));
        let map = jsv::prop(item, "map")
            .filter(|map| js::truthy(map))
            .map(js::to_string)
            .unwrap_or_default();
        [game, map.as_str()]
            .iter()
            .filter(|part| !part.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// `itemDate(item)`: длина, сегменты и дата; переносится только после «·».
    pub fn item_date(item: &Value, t: Translate) -> String {
        let mut parts = Vec::new();
        let duration = jsv::or_zero(jsv::prop(item, "duration"));
        if duration != 0. && !duration.is_nan() {
            parts.push(format_clock(duration, 0));
        }
        let segments = jsv::or_zero(jsv::prop(item, "segmentCount"));
        if segments > 1. && !jsv::truthy(jsv::prop(item, "series")) {
            parts.push(t(
                "#L_DEMO_SEGMENTS",
                &[&js::number::number_to_string(segments)],
            ));
        }
        parts.push(format_date_local(
            jsv::num(jsv::prop(item, "mtimeMs")).unwrap_or(f64::NAN),
        ));
        parts
            .iter()
            .filter(|part| !part.is_empty())
            .map(|part| part.replace(' ', "\u{a0}"))
            .collect::<Vec<_>>()
            .join("\u{a0}· ")
    }

    /// `jobLine(job)`: фаза (или повтор), сегмент, процент.
    pub fn job_line(job: &Job, t: Translate) -> String {
        let phase = match job.phase.as_str() {
            "render" => "#L_DEMO_PHASE_RENDER",
            "encode" => "#L_DEMO_PHASE_ENCODE",
            "verify" => "#L_DEMO_PHASE_VERIFY",
            "done" => "#L_DEMO_PHASE_DONE",
            _ => "#L_DEMO_PHASE_PREPARE",
        };
        let retry = job
            .message
            .as_deref()
            .and_then(|message| message.strip_prefix("retry-"))
            .filter(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
            .map(|digits| digits.parse::<f64>().unwrap_or(f64::INFINITY) + 1.);
        let mut parts = vec![match retry {
            Some(attempt) => t(
                "#L_DEMO_JOB_RETRY",
                &[&js::number::number_to_string(attempt)],
            ),
            None => t(phase, &[]),
        }];
        let count = jsv::or_zero(jsv::prop(&job.segment, "count"));
        if count > 1. {
            parts.push(t(
                "#L_DEMO_JOB_SEGMENT",
                &[
                    &text_of(jsv::prop(&job.segment, "index")),
                    &js::number::number_to_string(count),
                ],
            ));
        }
        let percent = if job.percent.is_nan() {
            0.
        } else {
            job.percent
        };
        parts.push(format!(
            "{}%",
            js::number::number_to_string(jsv::round(percent))
        ));
        parts.join(" · ")
    }

    /// Строка видео «Отрендеренных»: игра и карта.
    pub fn rendered_source(video: &Value) -> String {
        DemoPanel::item_meta(jsv::prop(video, "source").unwrap_or(&Value::Null))
    }

    /// Строка видео «Отрендеренных»: длина, размер, дата.
    pub fn rendered_line(video: &Value, t: Translate) -> String {
        let mut parts = Vec::new();
        let duration = jsv::or_zero(jsv::field(jsv::prop(video, "video"), "duration"));
        if duration != 0. && !duration.is_nan() {
            parts.push(format_clock(duration, 0));
        }
        if jsv::truthy(jsv::prop(video, "size")) {
            parts.push(format_size(jsv::or_zero(jsv::prop(video, "size")), t));
        }
        parts.push(format_date_local(
            jsv::num(jsv::prop(video, "mtimeMs")).unwrap_or(f64::NAN),
        ));
        parts.retain(|part| !part.is_empty());
        parts.join(" · ")
    }

    // ---------- жизнь страницы ----------

    /// `activated()`: вид зовёт `demo-list`, `-ffmpeg-status`, `-bxtrs-status`,
    /// `-settings-get`, `-rendered-list`.
    pub fn activate(&mut self) {
        self.active = true;
    }

    /// `deactivated()`: окно экспорта и вопрос удаления закрываются, предпросмотр гасится
    /// (вид ставит плееры на паузу и зовёт `demo-preview-clear`).
    pub fn deactivate(&mut self) {
        self.active = false;
        self.set_export_open(false);
        self.rendered.confirm = None;
        if self.media.as_ref().is_some_and(|media| !has_file(media)) {
            self.media = None;
        }
    }

    /// `exportOpen = open` с `watch.exportOpen`: закрытое окно забывает «Экспорт отменён».
    pub fn set_export_open(&mut self, open: bool) {
        self.export_open = open;
        if !open {
            if self
                .cancelled
                .as_ref()
                .is_some_and(|(_, kind)| *kind == JobKind::Export)
            {
                self.cancelled = None;
            }
            self.long_preview = None;
            self.export_initial = None;
        }
    }

    pub fn toggle_card(&mut self) {
        self.card_hidden = !self.card_hidden;
    }

    /// `setTab(id)`; `true` — перечитать «Отрендеренные». Плеер прежней вкладки — на паузу.
    pub fn set_tab(&mut self, tab: Tab) -> bool {
        if tab == self.tab {
            return false;
        }
        self.tab = tab;
        tab == Tab::Rendered
    }

    /// Ответ `demo-settings-get`.
    pub fn settings_done(&mut self, answer: &Answer) {
        if let Some(preview) = ok(answer)
            .and_then(|answer| jsv::prop(answer, "preview"))
            .filter(|preview| js::truthy(preview))
            && let (Some(view), Some(preview)) =
                (self.preview_view.as_object_mut(), preview.as_object())
        {
            for (key, value) in preview {
                view.insert(key.clone(), value.clone());
            }
        }
    }

    // ---------- список ----------

    /// `loadList` начинается: вид зовёт `demo-list`.
    pub fn list_begin(&mut self) {
        self.loading = true;
    }

    /// Ответ `demo-list` (`loadList`).
    pub fn list_done(&mut self, answer: &Answer, t: Translate) -> Next {
        self.loading = false;
        match answer {
            Err(()) => {
                self.notice = Some(Notice::error(error_text(&code("DEMO_FAILED"), t)));
                Next::Nothing
            }
            Ok(answer) if status_ok(answer) => self.apply_items(jsv::prop(answer, "items")),
            Ok(_) => Next::Nothing,
        }
    }

    /// `applyItems(items)`.
    pub fn apply_items(&mut self, items: Option<&Value>) -> Next {
        self.set_items(items);
        if let Some(id) = &self.selected_id
            && !self
                .items
                .iter()
                .any(|item| text_of(jsv::prop(item, "id")) == *id)
        {
            self.selected_id = None;
            self.detail = None;
        }
        match &self.selected_id {
            None if !self.items.is_empty() => {
                first_pick(&self.groups).map_or(Next::Nothing, Next::Select)
            }
            Some(id) => Next::Inspect(id.clone()),
            None => Next::Nothing,
        }
    }

    /// `select(id)`: `true` — вид зовёт `demo-inspect` (и ставит плеер на паузу).
    pub fn select(&mut self, id: &str) -> bool {
        if self.selected_id.as_deref() == Some(id) && self.detail.is_some() {
            return false;
        }
        self.selected_id = Some(id.to_owned());
        self.range = None;
        self.range_note = None;
        self.cancelled = None;
        self.media_error = false;
        true
    }

    /// `inspect(id)` начинается: вид зовёт `demo-inspect`.
    pub fn inspect_begin(&mut self) {
        self.detail_loading = true;
    }

    /// Ответ `demo-inspect` записи `id`.
    pub fn inspect_done(&mut self, id: &str, answer: &Answer, t: Translate) {
        self.detail_loading = false;
        if self.selected_id.as_deref() != Some(id) {
            return;
        }
        match answer {
            Ok(answer) if status_ok(answer) => self.detail = Some(answer.clone()),
            Ok(_) => {
                self.detail = None;
                self.notice = Some(Notice::error(error_text(&error_of(answer), t)));
            }
            Err(()) => self.detail = None,
        }
    }

    /// Ответ `demo-pick-files`: отмена диалога — ничего; иначе — итог добавления.
    pub fn pick_done(&mut self, answer: &Answer, t: Translate) -> Option<String> {
        match answer {
            Err(()) => {
                self.notice = Some(Notice::error(error_text(&code("DIALOG_UNAVAILABLE"), t)));
                None
            }
            Ok(value) if jsv::is_str(jsv::prop(value, "status"), "cancelled") => None,
            Ok(_) => self.applied(answer, Vec::new(), t),
        }
    }

    /// Ответ `demo-add-paths` (бросание файлов): `extra` — что отсеяно до вызова.
    pub fn add_paths_done(
        &mut self,
        answer: &Answer,
        extra: Rejected,
        t: Translate,
    ) -> Option<String> {
        if answer.is_err() {
            self.notice = Some(Notice::error(error_text(&code("DEMO_FAILED"), t)));
            return None;
        }
        self.applied(answer, extra, t)
    }

    /// `applyAdded`: сколько добавлено и что не подошло; добавленное — на «Демки» и в выбор
    /// (возвращает id для [`DemoPanel::select`]).
    fn applied(&mut self, answer: &Answer, extra: Rejected, t: Translate) -> Option<String> {
        let Some(answer) = ok(answer) else {
            let error = error_of(answer);
            // PATH_INVALID на весь вызов — больше 512 путей разом.
            let text = if jsv::is_str(jsv::prop(&error, "code"), "PATH_INVALID") {
                t("#L_DEMO_ERR_TOO_MANY", &[])
            } else {
                error_text(&error, t)
            };
            self.notice = Some(Notice::error(text));
            return None;
        };
        let mut rejected = extra;
        for entry in array_of(jsv::prop(answer, "rejected")) {
            let reason = error_text(&json!({ "code": jsv::prop(&entry, "code") }), t);
            rejected.push((text_of(jsv::prop(&entry, "name")), reason));
        }
        let added = array_of(jsv::prop(answer, "added"));
        let text = if !added.is_empty() {
            t("#L_DEMO_ADDED", &[&added.len().to_string()])
        } else if rejected.is_empty() {
            t("#L_DEMO_NOTHING_ADDED", &[])
        } else {
            String::new()
        };
        self.notice = Some(Notice {
            text,
            error: added.is_empty() && !rejected.is_empty(),
            rejected,
        });
        if let Some(items) = jsv::prop(answer, "items").filter(|items| js::truthy(items)) {
            self.set_items(Some(items));
        }
        let first = added.first()?;
        self.tab = Tab::Demos;
        self.selected_id = None;
        Some(text_of(Some(first)))
    }

    /// Бросили файлы на страницу: демки — в `demo-add-paths` (с отсеянными), иначе — строка
    /// ошибки. `Some((пути, отсеянные))` — звать канал.
    pub fn drop_paths(
        &mut self,
        paths: &[String],
        t: Translate,
    ) -> Option<(Vec<String>, Rejected)> {
        self.dragging = false;
        let name = |path: &String| {
            path.rsplit(['\\', '/'])
                .next()
                .unwrap_or(path.as_str())
                .to_owned()
        };
        let (good, wrong): (Vec<&String>, Vec<&String>) = paths
            .iter()
            .partition(|path| has_demo_extension(&name(path)));
        let wrong: Vec<String> = wrong.into_iter().map(name).collect();
        if !good.is_empty() {
            let extra = if wrong.is_empty() {
                Vec::new()
            } else {
                vec![(wrong.join(", "), t("#L_DEMO_ERR_FILE_TYPE", &[]))]
            };
            return Some((good.into_iter().cloned().collect(), extra));
        }
        self.notice = Some(Notice::error(t("#L_DEMO_DROP_WRONG", &[&wrong.join(", ")])));
        None
    }

    /// `removeItem`: id ручной записи для `demo-remove`.
    pub fn remove_begin(&mut self) -> Option<String> {
        let id = self.item_id()?;
        if self.removing {
            return None;
        }
        self.removing = true;
        Some(id)
    }

    /// Ответ `demo-remove`.
    pub fn remove_done(&mut self, id: &str, answer: &Answer, t: Translate) -> Next {
        self.removing = false;
        let Some(answer) = ok(answer) else {
            self.notice = Some(Notice::error(error_text(&error_of(answer), t)));
            return Next::Nothing;
        };
        if jsv::field(self.media.as_ref(), "itemId") == Some(&json!(id)) {
            self.media = None;
        }
        self.selected_id = None;
        self.detail = None;
        self.apply_items(jsv::prop(answer, "items"))
    }

    /// `move(index, delta)`: id и новый порядок для `demo-reorder`.
    pub fn reorder_begin(&mut self, index: usize, delta: isize) -> Option<(String, Vec<usize>)> {
        let count = array_of(jsv::field(self.detail.as_ref(), "segments")).len();
        let other = index
            .checked_add_signed(delta)
            .filter(|other| *other < count)?;
        if index >= count || self.reordering {
            return None;
        }
        let mut order: Vec<usize> = (0..count).collect();
        order.swap(index, other);
        self.reordering = true;
        Some((self.item_id()?, order))
    }

    /// Ответ `demo-reorder`: новая запись; старый предпросмотр снят с другого порядка.
    pub fn reorder_done(&mut self, answer: &Answer, t: Translate) {
        self.reordering = false;
        let Some(answer) = ok(answer) else {
            self.notice = Some(Notice::error(error_text(&error_of(answer), t)));
            return;
        };
        if self.player().own {
            self.media = None;
        }
        self.detail = Some(answer.clone());
        self.range = None;
        let item = jsv::prop(answer, "item").cloned().unwrap_or(Value::Null);
        let id = jsv::prop(&item, "id").cloned();
        for entry in &mut self.items {
            if jsv::prop(entry, "id") == id.as_ref() {
                *entry = item.clone();
            }
        }
        self.regroup();
    }

    // ---------- плеер ----------

    /// `onMark(which, demoTime)`: «Начало/Конец здесь».
    pub fn on_mark(&mut self, edge: Edge, demo_time: f64) {
        if self.detail.is_none() {
            return;
        }
        let (range, swapped) = mark_range(self.range, edge, demo_time, self.duration());
        self.range = range;
        self.range_note = swapped.then_some("#L_DEMO_RANGE_SWAPPED");
    }

    pub fn reset_range(&mut self) {
        self.range = None;
        self.range_note = None;
    }

    /// `startPreview`: id записи, если предпросмотр можно начать (до оценки).
    pub fn preview_check(&self) -> Option<String> {
        self.can_preview().then(|| self.item_id()).flatten()
    }

    /// Аргумент `demo-estimate`: `[{ preset: "preview", range }]`.
    pub fn estimate_args(&self, item_id: &str) -> Vec<Value> {
        vec![
            json!(item_id),
            json!([{ "preset": "preview", "range": self.range_value() }]),
        ]
    }

    fn range_value(&self) -> Value {
        self.range.map_or(Value::Null, ClipRange::to_value)
    }

    /// Ответ `demo-estimate`: `true` — начинать; долгий предпросмотр — окно экспорта с советом.
    pub fn estimate_done(&mut self, item_id: &str, answer: &Answer) -> bool {
        if !self.is_selected_item(item_id) || self.job.is_some() {
            return false;
        }
        let Some(answer) = ok(answer) else {
            return true;
        };
        let Some(preview) = jsv::prop(answer, "items")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .filter(|preview| js::truthy(preview))
        else {
            return true;
        };
        let seconds = jsv::num(jsv::prop(preview, "seconds")).unwrap_or(f64::NAN);
        let threshold = jsv::num(jsv::prop(answer, "threshold")).unwrap_or(f64::NAN);
        if seconds > threshold {
            self.long_preview = Some(json!({
                "itemId": item_id,
                "seconds": jsv::prop(preview, "seconds"),
                "range": jsv::prop(preview, "range"),
                "segments": jsv::prop(preview, "segments"),
            }));
            self.export_open = true;
            return false;
        }
        true
    }

    /// Предпросмотр начинается: аргументы `demo-preview-start` (`[itemId, { range, …вид }]`).
    pub fn preview_begin(&mut self, item_id: &str) -> Vec<Value> {
        self.preview_error = None;
        self.cancelled = None;
        self.media_error = false;
        if self.media.as_ref().is_some_and(|media| !has_file(media)) {
            self.media = None;
        }
        self.job = Some(Job {
            job_id: None,
            kind: JobKind::Preview,
            item_id: item_id.to_owned(),
            phase: "prepare".into(),
            percent: 0.,
            segment: Value::Null,
            message: None,
        });
        let mut options = json!({ "range": self.range_value() });
        if let (Some(options), Some(view)) =
            (options.as_object_mut(), self.preview_view.as_object())
        {
            for (key, value) in view {
                options.insert(key.clone(), value.clone());
            }
        }
        vec![json!(item_id), options]
    }

    /// Ответ `demo-preview-start` или `demo-export-start`.
    pub fn job_started(&mut self, kind: JobKind, item_id: &str, answer: &Answer) {
        let answer = match answer {
            Ok(value) => value.clone(),
            Err(()) => code("DEMO_FAILED"),
        };
        if status_ok(&answer) {
            if let Some(job) = self.job.as_mut().filter(|job| job.kind == kind)
                && job.job_id.is_none()
            {
                job.job_id = jsv::prop(&answer, "jobId").cloned();
            }
            return;
        }
        self.job = None;
        let error = if js::truthy(&answer) {
            answer
        } else {
            json!({})
        };
        if jsv::is_str(jsv::prop(&error, "code"), "CANCELLED") {
            self.cancelled = Some((item_id.to_owned(), kind));
            return;
        }
        match kind {
            JobKind::Preview => self.preview_error = Some(with_item(item_id, &error)),
            JobKind::Export => {
                self.export_outcome = Some(Outcome {
                    item_id: item_id.to_owned(),
                    result: None,
                    error: Some(error),
                })
            }
        }
    }

    /// `startExport(settings)`: аргументы `demo-export-start`, если задания нет.
    pub fn export_begin(&mut self, settings: Value) -> Option<Vec<Value>> {
        if self.job.is_some() {
            return None;
        }
        let item_id = self.item_id()?;
        self.export_outcome = None;
        self.cancelled = None;
        self.job = Some(Job {
            job_id: None,
            kind: JobKind::Export,
            item_id: item_id.clone(),
            phase: "prepare".into(),
            percent: 0.,
            segment: Value::Null,
            message: None,
        });
        Some(vec![json!(item_id), settings])
    }

    /// `cancelJob`: аргументы `demo-cancel` (без jobId — текущее задание).
    pub fn cancel_args(&self) -> Option<Vec<Value>> {
        let job = self.job.as_ref()?;
        Some(job.job_id.iter().cloned().collect())
    }

    /// `ownJob(payload)`: событие нашего задания; jobId может прийти раньше ответа на старт.
    fn own_job(&mut self, payload: &Value) -> bool {
        let Some(job) = self.job.as_mut() else {
            return false;
        };
        if !js::truthy(payload) {
            return false;
        }
        if let Some(id) = &job.job_id {
            return jsv::prop(payload, "jobId") == Some(id);
        }
        if JobKind::of(jsv::prop(payload, "kind")) != Some(job.kind) {
            return false;
        }
        job.job_id = jsv::prop(payload, "jobId").cloned();
        true
    }

    /// События `demo-job-*`, `demo-ffmpeg-progress`, `demo-bxtrs-progress`.
    pub fn on_event(&mut self, channel: &str, payload: &Value, t: Translate) -> JobEffect {
        match channel {
            "demo-job-progress" => {
                if self.own_job(payload)
                    && let Some(job) = self.job.as_mut()
                {
                    job.phase = text_of(jsv::prop(payload, "phase"));
                    job.percent = jsv::num(jsv::prop(payload, "percent")).unwrap_or(f64::NAN);
                    job.segment = jsv::prop(payload, "segment")
                        .cloned()
                        .unwrap_or(Value::Null);
                    job.message = jsv::str_of(jsv::prop(payload, "message")).map(str::to_owned);
                }
                JobEffect::default()
            }
            "demo-job-done" => self.job_done(payload, t),
            "demo-job-error" => self.job_error(payload, t),
            "demo-ffmpeg-progress" => {
                progress(&mut self.ffmpeg, payload);
                JobEffect::default()
            }
            "demo-bxtrs-progress" => {
                progress(&mut self.bxtrs, payload);
                JobEffect::default()
            }
            _ => JobEffect::default(),
        }
    }

    fn job_done(&mut self, payload: &Value, t: Translate) -> JobEffect {
        if !self.own_job(payload) {
            return JobEffect::default();
        }
        let item_id = self.job.take().map(|job| job.item_id).unwrap_or_default();
        let result = jsv::prop(payload, "result").cloned().unwrap_or(Value::Null);
        if JobKind::of(jsv::prop(payload, "kind")) == Some(JobKind::Preview) {
            if !self.active {
                return JobEffect::default();
            }
            self.media = Some(json!({ "itemId": item_id, "result": result }));
            self.media_error = false;
            return JobEffect {
                focus_player: true,
                ..JobEffect::default()
            };
        }
        let file_name = text_of(jsv::prop(&result, "fileName"));
        self.export_outcome = Some(Outcome {
            item_id: item_id.clone(),
            result: Some(result),
            error: None,
        });
        let seen = self.active && self.export_open && self.is_selected_item(&item_id);
        JobEffect {
            toast: (!seen).then(|| (t("#L_DEMO_EXPORT_DONE_TOAST", &[&file_name]), "fas fa-film")),
            reload_rendered: true,
            focus_player: false,
        }
    }

    fn job_error(&mut self, payload: &Value, t: Translate) -> JobEffect {
        if !self.own_job(payload) {
            return JobEffect::default();
        }
        let Some(job) = self.job.take() else {
            return JobEffect::default();
        };
        let kind = JobKind::of(jsv::prop(payload, "kind"));
        let error = jsv::prop(payload, "error")
            .filter(|error| js::truthy(error))
            .cloned()
            .unwrap_or(json!({}));
        let toast = |text: String, icon: &'static str| JobEffect {
            toast: Some((text, icon)),
            ..JobEffect::default()
        };
        // Отмена — не ошибка, но и не молчание: строка в окне или на экране плеера.
        if jsv::is_str(jsv::prop(&error, "code"), "CANCELLED") {
            if let Some(kind) = kind {
                self.cancelled = Some((job.item_id.clone(), kind));
            }
            if kind == Some(JobKind::Export) && (!self.active || !self.export_open) {
                return toast(t("#L_DEMO_EXPORT_CANCELLED", &[]), "fas fa-ban");
            }
            return JobEffect::default();
        }
        let selected = self.is_selected_item(&job.item_id);
        let text = error_text(&error, t);
        if kind == Some(JobKind::Preview) {
            self.preview_error = Some(with_item(&job.item_id, &error));
            if !self.active || !selected {
                return toast(
                    t("#L_DEMO_PREVIEW_FAILED_TOAST", &[&text]),
                    "fas fa-exclamation-triangle",
                );
            }
            return JobEffect::default();
        }
        self.export_outcome = Some(Outcome {
            item_id: job.item_id,
            result: None,
            error: Some(error),
        });
        // Окно скрыто или страница не открыта: ошибку видно тостом, в карточке остаётся строка.
        if !self.active || !self.export_open || !selected {
            return toast(
                t("#L_DEMO_EXPORT_FAILED_TOAST", &[&text]),
                "fas fa-exclamation-triangle",
            );
        }
        JobEffect::default()
    }

    /// «Всё равно предпросмотр» из окна про долгий предпросмотр: окно закрывается.
    pub fn preview_anyway(&mut self) {
        self.set_export_open(false);
    }

    /// `openResult(result)`: видео итога в плеер; другая запись — выбрать её (id).
    pub fn open_result(&mut self, result: Value) -> Option<String> {
        self.set_export_open(false);
        let item_id = self
            .export_outcome
            .as_ref()
            .map(|outcome| outcome.item_id.clone())
            .or_else(|| self.selected_id.clone())?;
        self.media = Some(json!({ "itemId": item_id, "result": result }));
        self.media_error = false;
        (self.selected_id.as_deref() != Some(item_id.as_str())).then_some(item_id)
    }

    /// `closeMedia` (вид ставит плеер на паузу).
    pub fn close_media(&mut self) {
        self.media = None;
        self.media_error = false;
    }

    // ---------- FFmpeg и bxt-rs ----------

    /// Ответ `demo-ffmpeg-status`.
    pub fn ffmpeg_status_done(&mut self, answer: &Answer) {
        if let Some(answer) = ok(answer) {
            self.ffmpeg = jsv::prop(answer, "ffmpeg").cloned();
        }
    }

    /// Ответ `demo-bxtrs-status`.
    pub fn bxtrs_status_done(&mut self, answer: &Answer) {
        if let Some(answer) = ok(answer) {
            self.bxtrs = jsv::prop(answer, "bxtrs").cloned();
        }
    }

    /// «Скачать FFmpeg» / «Скачать bxt-rs»: `true` — звать установку.
    pub fn install_begin(&mut self, bxtrs: bool) -> bool {
        let (status, error) = self.tool(bxtrs);
        let Some(current) = status.as_mut() else {
            return false;
        };
        if jsv::is_str(jsv::prop(current, "state"), "installing") {
            return false;
        }
        *error = None;
        if let Some(object) = current.as_object_mut() {
            object.insert("state".into(), json!("installing"));
            object.insert("percent".into(), json!(0));
        }
        true
    }

    /// Ответ установки: `true` — перечитать статус (не вышло).
    pub fn install_done(&mut self, bxtrs: bool, answer: &Answer) -> bool {
        let field = if bxtrs { "bxtrs" } else { "ffmpeg" };
        let answer = match answer {
            Ok(value) => value.clone(),
            Err(()) => code("DEMO_FAILED"),
        };
        let (status, error) = self.tool(bxtrs);
        if status_ok(&answer) {
            *status = jsv::prop(&answer, field).cloned();
            return false;
        }
        if js::truthy(&answer) && !jsv::is_str(jsv::prop(&answer, "status"), "cancelled") {
            *error = Some(
                jsv::str_of(jsv::prop(&answer, "code"))
                    .filter(|code| !code.is_empty())
                    .unwrap_or("DEMO_FAILED")
                    .to_owned(),
            );
        }
        true
    }

    fn tool(&mut self, bxtrs: bool) -> (&mut Option<Value>, &mut Option<String>) {
        if bxtrs {
            (&mut self.bxtrs, &mut self.bxtrs_error)
        } else {
            (&mut self.ffmpeg, &mut self.ffmpeg_error)
        }
    }

    // ---------- отрендеренные ----------

    /// `loadRendered` начинается: вид зовёт `demo-rendered-list`.
    pub fn rendered_list_begin(&mut self) {
        self.rendered.loading = true;
    }

    /// Ответ `demo-rendered-list`.
    pub fn rendered_list_done(&mut self, answer: &Answer) {
        self.rendered.loading = false;
        if let Some(answer) = ok(answer) {
            self.apply_rendered(answer);
        }
    }

    fn apply_rendered(&mut self, answer: &Value) {
        self.rendered.items = array_of(jsv::prop(answer, "items"));
        self.rendered.output_dir = jsv::str_of(jsv::prop(answer, "outputDir"))
            .unwrap_or_default()
            .to_owned();
        // Открытое видео пропало из папки — плеер пустеет.
        if let Some(open) = &self.rendered.open {
            let name = jsv::prop(open, "name");
            if !self
                .rendered
                .items
                .iter()
                .any(|video| jsv::prop(video, "name") == name)
            {
                self.rendered.open = None;
            }
        }
    }

    /// Открытое видео «Отрендеренных».
    pub fn open_name(&self) -> Option<String> {
        self.rendered
            .open
            .as_ref()
            .map(|open| text_of(jsv::prop(open, "name")))
    }

    /// `openRendered(name)`: `true` — звать `demo-rendered-open` (плеер — на паузу).
    pub fn open_rendered_begin(&self, name: &str) -> bool {
        self.open_name().as_deref() != Some(name)
    }

    /// Ответ `demo-rendered-open`: `true` — перечитать список (ошибка).
    pub fn open_rendered_done(&mut self, answer: &Answer, t: Translate) -> bool {
        if let Some(answer) = ok(answer) {
            let markers = jsv::prop(answer, "markers")
                .filter(|markers| js::truthy(markers))
                .cloned()
                .unwrap_or(json!([]));
            let source_id = jsv::prop(answer, "sourceId")
                .filter(|id| js::truthy(id))
                .cloned()
                .unwrap_or(Value::Null);
            self.rendered.open = Some(json!({
                "name": jsv::prop(answer, "name"),
                "url": jsv::prop(answer, "url"),
                "video": jsv::prop(answer, "video"),
                "source": jsv::prop(answer, "source"),
                "markers": markers,
                "sourceId": source_id,
            }));
            self.rendered.media_error = false;
            return false;
        }
        self.notice = Some(Notice::error(error_text(&error_of(answer), t)));
        true
    }

    /// Ответ `demo-rendered-reveal`: `true` — перечитать список (ошибка).
    pub fn reveal_done(&mut self, answer: &Answer, t: Translate) -> bool {
        if ok(answer).is_some() {
            return false;
        }
        self.notice = Some(Notice::error(error_text(&error_of(answer), t)));
        true
    }

    /// Видео удалили: плеер «Демок» и итог экспорта о нём забывают.
    fn forget_rendered(&mut self, name: &str, file: Option<&str>) {
        let which = RenderedWhich {
            name,
            output_dir: &self.rendered.output_dir,
            file,
        };
        let media_gone =
            demo_tool::is_rendered_file(jsv::field(self.media.as_ref(), "result"), &which);
        let outcome_gone = demo_tool::is_rendered_file(
            self.export_outcome
                .as_ref()
                .and_then(|outcome| outcome.result.as_ref()),
            &which,
        );
        if media_gone {
            self.close_media();
        }
        if outcome_gone {
            self.export_outcome = None;
        }
    }

    /// «Экспорт» готового видео: id исходной демки, если её можно выбрать (вкладка — «Демки»).
    pub fn export_from_rendered_begin(&mut self) -> Option<String> {
        let source = jsv::field(self.rendered.open.as_ref(), "sourceId")
            .filter(|id| js::truthy(id))
            .map(js::to_string)?;
        if self.job.is_some() {
            return None;
        }
        self.set_tab(Tab::Demos);
        Some(source)
    }

    /// После выбора исходной демки: окно экспорта с тем же отрезком и скоростью.
    pub fn export_from_rendered_finish(&mut self, source_id: &str) {
        if !self.is_selected_item(source_id) {
            return;
        }
        let video = jsv::field(self.rendered.open.as_ref(), "video").cloned();
        let range = ClipRange::from_value(jsv::field(video.as_ref(), "range"));
        let duration = self.duration();
        self.range = range.filter(|range| !(range.from <= 0.001 && range.to >= duration - 0.001));
        self.range_note = None;
        let speed = jsv::field(video.as_ref(), "speed")
            .filter(|speed| js::truthy(speed))
            .cloned()
            .unwrap_or(json!(1));
        self.export_initial = Some(json!({ "speed": speed }));
        self.export_open = true;
    }

    /// Корзина: «Удалить» открыло вопрос.
    pub fn ask_delete(&mut self) {
        self.rendered.confirm = self.open_name();
    }

    /// Ответ на вопрос удаления: имя для `demo-rendered-delete`. Плееры отпускают файл.
    pub fn delete_resolved(&mut self, accepted: bool) -> Option<String> {
        let name = self.rendered.confirm.take()?;
        if !accepted || self.rendered.deleting {
            return None;
        }
        if self.open_name().as_deref() == Some(name.as_str()) {
            self.rendered.open = None;
        }
        self.forget_rendered(&name, None);
        self.rendered.deleting = true;
        Some(name)
    }

    /// Ответ `demo-rendered-delete`: тост или ошибка; `true` вторым — перечитать список.
    pub fn delete_done(
        &mut self,
        name: &str,
        answer: &Answer,
        t: Translate,
    ) -> (Option<Toast>, bool) {
        self.rendered.deleting = false;
        if let Some(answer) = ok(answer) {
            let deleted = jsv::str_of(jsv::prop(answer, "deleted")).map(str::to_owned);
            self.forget_rendered(name, deleted.as_deref());
            self.apply_rendered(answer);
            return (
                Some((t("#L_DEMO_RENDERED_DELETED", &[name]), "fas fa-trash-alt")),
                false,
            );
        }
        self.notice = Some(Notice::error(error_text(&error_of(answer), t)));
        (None, true)
    }

    /// Пункты вопроса удаления: видео и его `timeline.json`.
    pub fn delete_items(name: &str) -> Vec<String> {
        let stem = if name.to_lowercase().ends_with(".mp4") {
            &name[..name.len() - 4]
        } else {
            name
        };
        let timeline = if stem.len() == name.len() {
            name.to_owned()
        } else {
            format!("{stem}.timeline.json")
        };
        vec![name.to_owned(), timeline]
    }
}

fn has_file(media: &Value) -> bool {
    jsv::truthy(jsv::field(jsv::prop(media, "result"), "file"))
}

/// `Object.assign({ itemId }, error)`.
fn with_item(item_id: &str, error: &Value) -> Value {
    let mut out = json!({ "itemId": item_id });
    if let (Some(out), Some(error)) = (out.as_object_mut(), error.as_object()) {
        for (key, value) in error {
            out.insert(key.clone(), value.clone());
        }
    }
    out
}

/// Ход установки: `{ state: "installing", percent }` поверх статуса.
fn progress(status: &mut Option<Value>, payload: &Value) {
    if let Some(object) = status.as_mut().and_then(Value::as_object_mut)
        && js::truthy(payload)
    {
        object.insert("state".into(), json!("installing"));
        object.insert(
            "percent".into(),
            jsv::prop(payload, "percent")
                .cloned()
                .unwrap_or(Value::Null),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(key: &str, args: &[&str]) -> String {
        match key {
            "#L_DECIMAL" => ",".to_owned(),
            _ if args.is_empty() => key.to_owned(),
            _ => format!("{key}({})", args.join(",")),
        }
    }

    fn item(id: &str, extra: Value) -> Value {
        let mut item = json!({"id": id, "name": format!("{id}.dem"), "source": "game",
            "gameId": "ghosting-renewed", "game": "ghosting", "kind": "demo", "map": "d1_trainstation_01",
            "duration": 144.46, "segmentCount": 1, "mtimeMs": 1_763_380_800_000_i64, "size": 3_145_728,
            "supported": true});
        if let (Some(item), Some(extra)) = (item.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                item.insert(key.clone(), value.clone());
            }
        }
        item
    }

    fn detail(item: Value, render: Value) -> Value {
        json!({"status": "ok", "item": item, "segments": [{"name": "a.dem", "duration": 144.46}],
            "markers": [{"t": 25.85, "type": "map", "label": "d1_trainstation_02"}], "render": render})
    }

    fn ready() -> Value {
        json!({"ok": true, "code": null, "view": {"keyhud": false, "hud": true, "ownConfig": true, "ownHud": true}})
    }

    fn loaded(panel: &mut DemoPanel) {
        let answer = Ok(json!({"status": "ok", "items": [
            item("bad", json!({"supported": false, "reason": "DEMO_TRUNCATED", "mtimeMs": 1_900_000_000_000_i64})),
            item("gh", json!({})),
        ]}));
        assert_eq!(
            panel.list_done(&answer, &t),
            Next::Select("gh".into()),
            "первая поддерживаемая"
        );
        assert!(panel.select("gh"));
        panel.inspect_done("gh", &Ok(detail(item("gh", json!({})), ready())), &t);
        panel.ffmpeg_status_done(&Ok(json!({"status": "ok", "ffmpeg": {"state": "ready"}})));
    }

    #[test]
    fn screens_follow_the_selection_tools_and_jobs() {
        let mut panel = DemoPanel::default();
        assert_eq!(panel.screen(), Some(Screen::Empty));
        loaded(&mut panel);
        assert_eq!(panel.screen(), Some(Screen::Ready));
        assert!(panel.can_preview());
        // FFmpeg не готов — его карточка.
        panel.ffmpeg = Some(json!({"state": "not-installed", "installable": true}));
        assert_eq!(panel.screen(), Some(Screen::Ffmpeg));
        assert!(!panel.can_preview());
        // Рендер HL1 без bxt-rs: карточка bxt-rs, если статус известен, иначе «не готово».
        panel.detail = Some(detail(
            item("gh", json!({})),
            json!({"ok": false, "code": "BXTRS_MISSING"}),
        ));
        assert_eq!(panel.screen(), Some(Screen::NotReady));
        panel.bxtrs = Some(json!({"state": "missing"}));
        assert_eq!(panel.screen(), Some(Screen::Bxtrs));
        panel.detail = Some(detail(item("gh", json!({"supported": false})), ready()));
        assert_eq!(panel.screen(), Some(Screen::Unsupported));
        // Предпросмотр идёт; готовое видео — без экрана; ошибка видео — свой экран.
        panel.detail = Some(detail(item("gh", json!({})), ready()));
        panel.ffmpeg = Some(json!({"state": "ready"}));
        let args = panel.preview_begin("gh");
        assert_eq!(args[1]["range"], Value::Null);
        assert_eq!(args[1]["keyhud"], json!(true));
        assert_eq!(panel.screen(), Some(Screen::PreviewJob));
        let effect = panel.on_event(
            "demo-job-done",
            &json!({"jobId": "j1", "kind": "preview", "result": {"mediaUrl": "hlsr-media://p", "markers": []}}),
            &t,
        );
        assert!(
            !effect.focus_player,
            "страница не открыта — видео не ставится"
        );
        assert_eq!(panel.media, None);
        panel.activate();
        panel.preview_begin("gh");
        let effect = panel.on_event(
            "demo-job-done",
            &json!({"jobId": "j2", "kind": "preview", "result": {"mediaUrl": "hlsr-media://p", "markers": []}}),
            &t,
        );
        assert!(effect.focus_player);
        assert_eq!(panel.screen(), None);
        panel.media_error = true;
        assert_eq!(panel.screen(), Some(Screen::MediaError));
        // Уход со страницы снимает предпросмотр (у него нет файла).
        panel.deactivate();
        assert_eq!(panel.media, None);
    }

    #[test]
    fn jobs_match_events_by_kind_then_id_and_toast_when_unseen() {
        let mut panel = DemoPanel::default();
        loaded(&mut panel);
        panel.activate();
        let args = panel.export_begin(json!({"preset": "discord"})).unwrap();
        assert_eq!(args, [json!("gh"), json!({"preset": "discord"})]);
        assert!(panel.export_begin(json!({})).is_none(), "одно задание");
        assert!(panel.own_export());
        assert_eq!(panel.cancel_args(), Some(vec![]), "без jobId — текущее");
        // Чужое по виду событие не наше; наше — запоминает jobId.
        panel.on_event(
            "demo-job-progress",
            &json!({"jobId": "p", "kind": "preview", "percent": 50}),
            &t,
        );
        assert_eq!(panel.job.as_ref().unwrap().percent, 0.);
        panel.on_event(
            "demo-job-progress",
            &json!({"jobId": "e1", "kind": "export", "phase": "render", "percent": 41.6,
                "segment": {"index": 2, "count": 3}, "message": "retry-1"}),
            &t,
        );
        let job = panel.job.clone().unwrap();
        assert_eq!(job.job_id, Some(json!("e1")));
        assert_eq!(
            DemoPanel::job_line(&job, &t),
            "#L_DEMO_JOB_RETRY(2) · #L_DEMO_JOB_SEGMENT(2,3) · 42%"
        );
        assert_eq!(panel.cancel_args(), Some(vec![json!("e1")]));
        // Ответ на старт после события не затирает id.
        panel.job_started(
            JobKind::Export,
            "gh",
            &Ok(json!({"status": "ok", "jobId": "e1"})),
        );
        // Окно экспорта закрыто — итог тостом и перечитать «Отрендеренные».
        let effect = panel.on_event(
            "demo-job-done",
            &json!({"jobId": "e1", "kind": "export", "result": {"file": "C:\\v\\gh.mp4", "fileName": "gh.mp4"}}),
            &t,
        );
        assert_eq!(
            effect.toast,
            Some(("#L_DEMO_EXPORT_DONE_TOAST(gh.mp4)".into(), "fas fa-film"))
        );
        assert!(effect.reload_rendered);
        assert!(panel.own_outcome().unwrap().result.is_some());
        // Ошибка экспорта при открытом окне — без тоста, строка в карточке.
        panel.export_begin(json!({}));
        panel.set_export_open(true);
        let effect = panel.on_event(
            "demo-job-error",
            &json!({"jobId": "e2", "kind": "export", "error": {"code": "NO_SPACE"}}),
            &t,
        );
        assert_eq!(effect.toast, None);
        assert_eq!(
            panel.own_outcome().unwrap().error,
            Some(json!({"code": "NO_SPACE"}))
        );
        // Отмена при закрытом окне — тост; закрытие окна забывает «Экспорт отменён».
        panel.export_begin(json!({}));
        panel.set_export_open(false);
        let effect = panel.on_event(
            "demo-job-error",
            &json!({"jobId": "e3", "kind": "export", "error": {"code": "CANCELLED"}}),
            &t,
        );
        assert_eq!(
            effect.toast,
            Some(("#L_DEMO_EXPORT_CANCELLED".into(), "fas fa-ban"))
        );
        panel.set_export_open(true);
        assert!(panel.cancelled.is_some());
        panel.set_export_open(false);
        assert!(panel.cancelled.is_none());
        // Старт отклонён: отмена — «отменено», иначе ошибка предпросмотра на экране.
        panel.preview_begin("gh");
        panel.job_started(
            JobKind::Preview,
            "gh",
            &Ok(json!({"status": "error", "code": "CANCELLED"})),
        );
        assert!(panel.preview_cancelled());
        assert_eq!(panel.screen(), Some(Screen::Ready));
        panel.preview_begin("gh");
        panel.job_started(JobKind::Preview, "gh", &Err(()));
        assert_eq!(
            panel.preview_error,
            Some(json!({"itemId": "gh", "code": "DEMO_FAILED"}))
        );
        assert_eq!(panel.screen(), Some(Screen::PreviewError));
    }

    #[test]
    fn a_long_preview_opens_the_export_window_instead() {
        let mut panel = DemoPanel::default();
        loaded(&mut panel);
        panel.on_mark(Edge::From, 10.);
        assert_eq!(
            panel.range,
            Some(ClipRange {
                from: 10.,
                to: 144.46
            })
        );
        assert_eq!(
            panel.estimate_args("gh"),
            [
                json!("gh"),
                json!([{"preset": "preview", "range": {"from": 10, "to": 144.46}}])
            ]
        );
        let quick = Ok(json!({"status": "ok", "threshold": 60, "items": [{"seconds": 20}]}));
        assert!(panel.estimate_done("gh", &quick));
        let slow = Ok(json!({"status": "ok", "threshold": 60,
            "items": [{"seconds": 600, "range": {"from": 10, "to": 144.46}, "segments": 1}]}));
        assert!(!panel.estimate_done("gh", &slow));
        assert!(panel.export_open);
        assert_eq!(panel.long_preview.as_ref().unwrap()["seconds"], json!(600));
        panel.preview_anyway();
        assert!(!panel.export_open && panel.long_preview.is_none());
        assert!(
            panel.estimate_done("gh", &Err(())),
            "оценки нет — сразу предпросмотр"
        );
        assert!(!panel.estimate_done("other", &quick), "выбор сменился");
        // Конец раньше начала — края меняются, подсказка; сброс.
        panel.on_mark(Edge::To, 5.);
        assert_eq!(panel.range, Some(ClipRange { from: 5., to: 10. }));
        assert_eq!(panel.range_note, Some("#L_DEMO_RANGE_SWAPPED"));
        panel.reset_range();
        assert_eq!((panel.range, panel.range_note), (None, None));
    }

    #[test]
    fn adding_files_reports_added_and_rejected_and_selects_the_first() {
        let mut panel = DemoPanel::default();
        loaded(&mut panel);
        panel.tab = Tab::Rendered;
        let answer = Ok(json!({"status": "ok", "added": ["m1", "m2"],
            "rejected": [{"name": "x.dem", "code": "DEMO_NOT_A_DEMO"}],
            "items": [item("gh", json!({})), item("m1", json!({"source": "manual"}))]}));
        let extra = vec![("a.txt".to_owned(), "#L_DEMO_ERR_FILE_TYPE".to_owned())];
        assert_eq!(panel.add_paths_done(&answer, extra, &t), Some("m1".into()));
        assert_eq!(panel.tab, Tab::Demos);
        assert_eq!(panel.selected_id, None);
        let notice = panel.notice.clone().unwrap();
        assert_eq!(notice.text, "#L_DEMO_ADDED(2)");
        assert!(!notice.error);
        assert_eq!(
            notice.rejected,
            [
                ("a.txt".to_owned(), "#L_DEMO_ERR_FILE_TYPE".to_owned()),
                ("x.dem".to_owned(), "#L_DEMO_ERR_NOT_DEMO".to_owned())
            ]
        );
        // Ничего не подошло — ошибка без текста; ничего не выбрано — «ничего не добавлено».
        let none = Ok(
            json!({"status": "ok", "added": [], "rejected": [{"name": "y", "code": "LIST_FULL"}]}),
        );
        assert_eq!(panel.add_paths_done(&none, vec![], &t), None);
        assert!(
            panel.notice.as_ref().unwrap().error && panel.notice.as_ref().unwrap().text.is_empty()
        );
        assert_eq!(panel.pick_done(&Ok(json!({"status": "ok"})), &t), None);
        assert_eq!(panel.notice.as_ref().unwrap().text, "#L_DEMO_NOTHING_ADDED");
        assert_eq!(
            panel.pick_done(&Ok(json!({"status": "cancelled"})), &t),
            None
        );
        assert_eq!(panel.pick_done(&Err(()), &t), None);
        assert_eq!(panel.notice.as_ref().unwrap().text, "#L_DEMO_ERR_DIALOG");
        let many = Ok(json!({"status": "error", "code": "PATH_INVALID"}));
        panel.add_paths_done(&many, vec![], &t);
        assert_eq!(panel.notice.as_ref().unwrap().text, "#L_DEMO_ERR_TOO_MANY");
        // Бросание: демки — в канал с отсеянными, одни чужие — строка ошибки.
        let paths = vec![r"C:\d\run.DEM".to_owned(), r"C:\d\clip.mp4".to_owned()];
        let (good, extra) = panel.drop_paths(&paths, &t).unwrap();
        assert_eq!(good, [r"C:\d\run.DEM"]);
        assert_eq!(
            extra,
            [("clip.mp4".to_owned(), "#L_DEMO_ERR_FILE_TYPE".to_owned())]
        );
        assert!(panel.drop_paths(&paths[1..], &t).is_none());
        assert_eq!(
            panel.notice.as_ref().unwrap().text,
            "#L_DEMO_DROP_WRONG(clip.mp4)"
        );
    }

    #[test]
    fn remove_and_reorder_manual_records() {
        let mut panel = DemoPanel::default();
        let set = item(
            "set",
            json!({"source": "manual", "kind": "segments", "segmentCount": 3}),
        );
        panel.apply_items(Some(&json!([set.clone()])));
        panel.select("set");
        let mut answer = detail(set.clone(), ready());
        answer["segments"] = json!([{"name": "a"}, {"name": "b"}, {"name": "c"}]);
        panel.inspect_done("set", &Ok(answer.clone()), &t);
        assert_eq!(panel.reorder_begin(0, -1), None, "выше первого некуда");
        assert_eq!(
            panel.reorder_begin(1, 1),
            Some(("set".into(), vec![0, 2, 1]))
        );
        assert_eq!(panel.reorder_begin(0, 1), None, "уже идёт");
        answer["item"]["segmentCount"] = json!(4);
        panel.reorder_done(&Ok(answer.clone()), &t);
        assert!(!panel.reordering);
        assert_eq!(
            panel.groups()[0].items,
            [answer["item"].clone()],
            "кэш групп обновлён"
        );
        assert_eq!(panel.remove_begin(), Some("set".into()));
        let next = panel.remove_done(
            "set",
            &Ok(json!({"status": "ok", "items": [item("gh", json!({}))]})),
            &t,
        );
        assert_eq!(next, Next::Select("gh".into()));
        // Выбранная запись пропала из списка — выбор снят; есть — перечитать.
        panel.select("gh");
        assert_eq!(
            panel.apply_items(Some(&json!([item("gh", json!({}))]))),
            Next::Inspect("gh".into())
        );
        assert_eq!(panel.apply_items(Some(&json!([]))), Next::Nothing);
        assert_eq!(panel.selected_id, None);
    }

    #[test]
    fn rendered_videos_open_delete_and_export_from_their_source() {
        let mut panel = DemoPanel::default();
        loaded(&mut panel);
        let list = json!({"status": "ok", "outputDir": "C:\\Videos\\HLSR", "items": [
            {"name": "gh.mp4", "size": 1_048_576, "mtimeMs": 1_763_380_800_000_i64,
             "video": {"duration": 20, "width": 1920, "height": 1080, "fps": 60, "speed": 1.5},
             "source": {"game": "ghosting", "map": "d1_trainstation_01"}, "markerCount": 1}]});
        panel.rendered_list_done(&Ok(list.clone()));
        assert!(panel.open_rendered_begin("gh.mp4"));
        let open = Ok(
            json!({"status": "ok", "name": "gh.mp4", "url": "hlsr-media://r",
            "video": {"duration": 20, "width": 1920, "height": 1080, "fps": 60, "speed": 1.5,
                      "range": {"from": 10, "to": 30}},
            "source": {"game": "ghosting", "map": "d1_trainstation_01"},
            "markers": [{"t": 0, "type": "map", "label": "d1"}], "sourceId": "gh"}),
        );
        assert!(!panel.open_rendered_done(&open, &t));
        assert!(!panel.open_rendered_begin("gh.mp4"), "уже открыто");
        let facts = panel.rendered_facts(&t);
        assert!(facts.contains(&("#L_DEMO_F_VIDEO".into(), "1920×1080, 60 FPS".into())));
        assert!(facts.contains(&("#L_DEMO_F_SPEED".into(), "1,5×".into())));
        assert_eq!(
            DemoPanel::rendered_source(&list["items"][0]),
            "Ghosting · d1_trainstation_01"
        );
        // «Экспорт» видео: исходная демка, тот же отрезок и скорость.
        panel.tab = Tab::Rendered;
        assert_eq!(panel.export_from_rendered_begin(), Some("gh".into()));
        assert_eq!(panel.tab, Tab::Demos);
        panel.export_from_rendered_finish("gh");
        assert!(panel.export_open);
        assert_eq!(panel.range, Some(ClipRange { from: 10., to: 30. }));
        assert_eq!(panel.export_initial, Some(json!({"speed": 1.5})));
        // Удаление: вопрос, плеер отпускает файл, итог экспорта этого видео забыт.
        panel.export_outcome = Some(Outcome {
            item_id: "gh".into(),
            result: Some(json!({"file": "C:\\Videos\\HLSR\\gh.mp4", "fileName": "gh.mp4"})),
            error: None,
        });
        panel.ask_delete();
        assert_eq!(panel.rendered.confirm.as_deref(), Some("gh.mp4"));
        assert_eq!(
            DemoPanel::delete_items("gh.mp4"),
            ["gh.mp4", "gh.timeline.json"]
        );
        assert_eq!(panel.delete_resolved(true), Some("gh.mp4".into()));
        assert_eq!(panel.rendered.open, None);
        assert_eq!(panel.export_outcome, None);
        let (toast, reload) = panel.delete_done(
            "gh.mp4",
            &Ok(json!({"status": "ok", "deleted": "C:\\Videos\\HLSR\\gh.mp4", "outputDir": "C:\\Videos\\HLSR", "items": []})),
            &t,
        );
        assert_eq!(
            toast,
            Some((
                "#L_DEMO_RENDERED_DELETED(gh.mp4)".into(),
                "fas fa-trash-alt"
            ))
        );
        assert!(!reload && panel.rendered.items.is_empty());
        assert_eq!(panel.delete_resolved(true), None, "вопроса нет");
        let (_, reload) = panel.delete_done(
            "x",
            &Ok(json!({"status": "error", "code": "TRASH_FAILED"})),
            &t,
        );
        assert!(reload);
        assert_eq!(panel.notice.as_ref().unwrap().text, "#L_DEMO_ERR_TRASH");
    }

    #[test]
    fn tools_install_with_progress_and_errors() {
        let mut panel = DemoPanel::default();
        assert!(!panel.install_begin(false), "статуса нет");
        panel.ffmpeg_status_done(&Ok(
            json!({"status": "ok", "ffmpeg": {"state": "not-installed", "installable": true}}),
        ));
        assert!(panel.install_begin(false));
        assert!(!panel.install_begin(false), "уже ставится");
        panel.on_event("demo-ffmpeg-progress", &json!({"percent": 42}), &t);
        assert_eq!(
            panel.ffmpeg,
            Some(json!({"state": "installing", "installable": true, "percent": 42}))
        );
        assert!(panel.install_done(
            false,
            &Ok(json!({"status": "error", "code": "NETWORK_ERROR"}))
        ));
        assert_eq!(panel.ffmpeg_error.as_deref(), Some("NETWORK_ERROR"));
        assert!(panel.install_done(false, &Ok(json!({"status": "cancelled"}))));
        assert!(!panel.install_done(
            false,
            &Ok(json!({"status": "ok", "ffmpeg": {"state": "ready"}}))
        ));
        assert!(panel.ffmpeg_ready());
        panel.bxtrs_status_done(&Ok(json!({"status": "ok", "bxtrs": {"state": "missing"}})));
        assert!(panel.install_begin(true));
        assert!(panel.install_done(true, &Err(())));
        assert_eq!(panel.bxtrs_error.as_deref(), Some("DEMO_FAILED"));
    }

    #[test]
    fn card_texts_facts_names_and_reasons() {
        let mut panel = DemoPanel::default();
        let series = item(
            "s",
            json!({"series": true, "name": "hlsrrun", "segmentCount": 3,
            "missing": [2], "maps": ["c1a0", "c1a1"], "runTime": 61.2, "game": "hl1"}),
        );
        assert_eq!(
            DemoPanel::item_name(&series, Language::Ru, &t),
            "#L_DEMO_SERIES_FEW(hlsrrun,3)"
        );
        assert_eq!(
            DemoPanel::item_name(&series, Language::En, &t),
            "#L_DEMO_SERIES_MANY(hlsrrun,3)"
        );
        let one = item(
            "o",
            json!({"series": true, "name": "x", "segmentCount": 21}),
        );
        assert_eq!(
            DemoPanel::item_name(&one, Language::Ru, &t),
            "#L_DEMO_SERIES_ONE(x,21)"
        );
        assert_eq!(
            DemoPanel::item_meta(&series),
            "Half-Life · d1_trainstation_01"
        );
        let date = DemoPanel::item_date(&item("d", json!({"segmentCount": 2})), &t);
        assert!(
            date.starts_with("2:24\u{a0}· #L_DEMO_SEGMENTS(2)\u{a0}· "),
            "{date}"
        );
        assert_eq!(date.matches(' ').count(), 2, "переносы только после «·»");
        panel.detail = Some(detail(series, ready()));
        let facts = panel.facts(&t);
        let labels: Vec<&str> = facts.iter().map(|(label, _)| label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "#L_DEMO_F_GAME",
                "#L_DEMO_F_MAPS",
                "#L_DEMO_F_LENGTH",
                "#L_DEMO_F_TIMER",
                "#L_DEMO_F_SEGMENTS",
                "#L_DEMO_F_MISSING",
                "#L_DEMO_F_DATE",
                "#L_DEMO_F_SIZE"
            ]
        );
        assert_eq!(facts[1].1, "c1a0, c1a1");
        assert_eq!(facts[3].1, "1:01.2");
        assert_eq!(facts[7].1, "3,0\u{a0}#L_MB");
        // Причина: сегмент в тексте или префиксом у рана из нескольких файлов.
        let reason = |extra: Value| DemoPanel::reason_text(&item("r", extra), &t);
        assert_eq!(
            reason(json!({"reason": "DEMO_TRUNCATED"})),
            "#L_DEMO_ERR_TRUNCATED"
        );
        assert_eq!(
            reason(json!({"reason": "DEMO_TRUNCATED", "failedSegment": 2, "segmentCount": 3})),
            "#L_DEMO_SEGMENT_PREFIX(2,#L_DEMO_ERR_TRUNCATED)"
        );
        assert_eq!(
            reason(json!({"reason": "SEGMENT_MISSING", "failedSegment": 4})),
            "#L_DEMO_ERR_SEGMENT_MISSING(4)"
        );
        assert_eq!(reason(json!({})), "#L_DEMO_ERR_UNSUPPORTED");
        // Метки карточки — во времени демо: своё видео — sourceTime.
        assert_eq!(panel.side_markers()[0].1, 25.85);
        panel.media = Some(
            json!({"itemId": "s", "result": {"markers": [{"t": 1, "sourceTime": 7, "type": "map"}]}}),
        );
        assert_eq!(panel.side_markers()[0].1, 7.);
        // Смена языка: файл экспорта переживает, предпросмотр — нет.
        assert_eq!(panel.carried().media, None);
        panel.media = Some(json!({"itemId": "s", "result": {"file": "C:\\v.mp4"}}));
        panel.card_hidden = true;
        let carried = panel.carried();
        assert!(carried.media.is_some() && carried.card_hidden);
    }
}
