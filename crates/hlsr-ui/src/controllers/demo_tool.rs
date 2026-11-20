//! Чистая логика страницы «Демо» (`demo-tool.js` 0.4.9, без окна и IPC): время, сочетания
//! разрешение × FPS, группы списка, тексты ошибок по кодам главного процесса.
//!
//! Данные — значения JSON ответов каналов `demo-*`, как их читает JS рендерера: поля, которого
//! нет, — `undefined`, `x || 0`, `String(x)`.

use super::boot::Translate;
use super::jsv;
use hlsr_core::js;
use serde_json::Value;

/// Скорости плеера.
pub const RATES: [f64; 7] = [0.25, 0.5, 0.75, 1., 1.25, 1.5, 2.];
/// Скорости клипа в окне экспорта.
pub const CLIP_SPEEDS: [f64; 6] = [0.25, 0.5, 1., 1.5, 2., 4.];
/// Разрешения экспорта: высота и подпись.
pub const RESOLUTIONS: [(f64, &str); 4] = [
    (720., "720p"),
    (1080., "1080p"),
    (1440., "1440p"),
    (2160., "4K"),
];

const GROUP_ORDER: [&str; 5] = [
    "hl1-won",
    "ghosting-renewed",
    "hl2-4260",
    "hl2-2707",
    "hl2-5288",
];

/// Расширения демок, которые принимает страница.
pub const DEMO_EXTENSIONS: [&str; 2] = [".dem", ".ghplaylist"];

/// `GROUP_NAMES[id]`: название группы игры.
pub fn group_name(id: &str) -> Option<&'static str> {
    Some(match id {
        "hl1-won" => "Half-Life",
        "ghosting-renewed" => "Ghosting Mod: Renewed",
        "hl2-4260" => "Half-Life 2 (4260)",
        "hl2-2707" => "Half-Life 2 (2707)",
        "hl2-5288" => "Half-Life 2 (5288)",
        _ => return None,
    })
}

/// `gameName(game)`: движок записи → название; неизвестный — пусто.
pub fn game_name(game: &str) -> &'static str {
    match game {
        "hl1" => "Half-Life",
        "ghosting" => "Ghosting",
        "hl2" => "Half-Life 2",
        _ => "",
    }
}

/// `encoderName(id)`.
pub fn encoder_name(id: &str) -> String {
    match id {
        "h264_nvenc" => "NVIDIA NVENC".into(),
        "h264_amf" => "AMD AMF".into(),
        "h264_qsv" => "Intel Quick Sync".into(),
        "libx264" => "x264".into(),
        other => other.into(),
    }
}

/// `resolutionLabel(height)`: «720p», «4K»; не из списка — «<высота>p».
pub fn resolution_label(height: f64) -> String {
    RESOLUTIONS.iter().find(|(h, _)| *h == height).map_or_else(
        || format!("{}p", js::number::number_to_string(height)),
        |(_, label)| (*label).to_owned(),
    )
}

/// `formatClock(seconds, digits)`: «1:05.3», «1:02:05.3»; `digits` — знаков после точки.
pub fn format_clock(seconds: f64, digits: usize) -> String {
    let value = if seconds.is_nan() {
        0.
    } else {
        seconds.max(0.)
    };
    let scale = 10f64.powi(digits as i32);
    let total = jsv::round(value * scale) / scale;
    let hours = (total / 3600.).floor();
    let minutes = ((total % 3600.) / 60.).floor();
    let rest = total - hours * 3600. - minutes * 60.;
    let mut secs = js::number::to_fixed(rest, digits);
    if rest < 10. {
        secs.insert(0, '0');
    }
    if hours > 0. {
        format!("{}:{:02}:{secs}", hours as u64, minutes as u64)
    } else {
        format!("{}:{secs}", minutes as u64)
    }
}

/// `formatClock(seconds)` с одним знаком.
pub fn clock(seconds: f64) -> String {
    format_clock(seconds, 1)
}

/// `parseClock(text)`: «1:05.3», «65,3», «1:02:05» → секунды; непонятное — `None`.
pub fn parse_clock(text: &str) -> Option<f64> {
    let text = text.trim().replacen(',', ".", 1);
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() == 1 && parts[0].is_empty() {
        return None;
    }
    let numeric = |part: &&str| {
        let (int, frac) = part.split_once('.').unwrap_or((part, "1"));
        !int.is_empty()
            && !frac.is_empty()
            && int.bytes().all(|b| b.is_ascii_digit())
            && frac.bytes().all(|b| b.is_ascii_digit())
    };
    if parts.len() > 3 || !parts.iter().all(numeric) {
        return None;
    }
    let numbers: Vec<f64> = parts
        .iter()
        .map(|part| part.parse().unwrap_or(0.))
        .collect();
    if numbers[1..].iter().any(|value| *value >= 60.) {
        return None;
    }
    if numbers[..numbers.len() - 1]
        .iter()
        .any(|value| value.fract() != 0.)
    {
        return None;
    }
    Some(numbers.iter().fold(0., |sum, value| sum * 60. + value))
}

/// Интервал во времени демо.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipRange {
    pub from: f64,
    pub to: f64,
}

impl ClipRange {
    /// `{ from, to }` из ответа; не объект — `None`.
    pub fn from_value(value: Option<&Value>) -> Option<ClipRange> {
        let value = value.filter(|value| value.is_object())?;
        Some(ClipRange {
            from: jsv::num(jsv::prop(value, "from")).unwrap_or(f64::NAN),
            to: jsv::num(jsv::prop(value, "to")).unwrap_or(f64::NAN),
        })
    }

    pub fn to_value(self) -> Value {
        serde_json::json!({"from": jsv::number_value(self.from), "to": jsv::number_value(self.to)})
    }
}

/// `range` и `speed` таймлайна видео: нет — весь ран со скоростью 1.
fn video_range_speed(video: Option<&Value>) -> (ClipRange, f64) {
    let range = ClipRange::from_value(jsv::field(video, "range")).unwrap_or(ClipRange {
        from: 0.,
        to: f64::INFINITY,
    });
    let speed = match jsv::num(jsv::field(video, "speed")) {
        Some(speed) if speed != 0. && !speed.is_nan() => speed,
        _ => 1.,
    };
    (range, speed)
}

/// `marker.t` и `marker.sourceTime` (только конечное число — `Number.isFinite`).
fn anchor_of(marker: &Value) -> Option<(f64, f64)> {
    let source = jsv::prop(marker, "sourceTime")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())?;
    Some((jsv::num(jsv::prop(marker, "t")).unwrap_or(f64::NAN), source))
}

/// `demoTimeAt(videoTime, video, markers)`: время видео → время демо. Ближайшая метка слева
/// учитывает вырезанные загрузки, дальше время идёт со скоростью клипа.
pub fn demo_time_at(video_time: f64, video: Option<&Value>, markers: &[Value]) -> f64 {
    let (range, speed) = video_range_speed(video);
    let mut anchor = (0., range.from);
    for (t, source) in markers.iter().filter_map(anchor_of) {
        if t <= video_time && t >= anchor.0 {
            anchor = (t, source);
        }
    }
    let value = anchor.1 + (video_time - anchor.0) * speed;
    range.to.min(range.from.max(value))
}

/// `videoTimeAt(demoTime, video, markers)`: время демо → время видео.
pub fn video_time_at(demo_time: f64, video: Option<&Value>, markers: &[Value]) -> f64 {
    let (range, speed) = video_range_speed(video);
    let mut anchor = (0., range.from);
    for (t, source) in markers.iter().filter_map(anchor_of) {
        if source <= demo_time && source >= anchor.1 {
            anchor = (t, source);
        }
    }
    (anchor.0 + (demo_time - anchor.1) / speed).max(0.)
}

/// Допустимое сочетание разрешения и FPS.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Combo {
    pub height: f64,
    pub width: Option<f64>,
    pub fps: f64,
}

fn combo_of(value: &Value) -> Combo {
    Combo {
        height: jsv::num(jsv::prop(value, "height")).unwrap_or(f64::NAN),
        width: jsv::num(jsv::prop(value, "width")),
        fps: jsv::num(jsv::prop(value, "fps")).unwrap_or(f64::NAN),
    }
}

/// `combosFor(options, mode)`: сочетания режима кодировщика из `demo-options`. FFmpeg не готов
/// (`modes: null`) — всё, что умеет адаптер.
pub fn combos_for(options: Option<&Value>, mode: &str) -> Vec<Combo> {
    let Some(options) = options.filter(|options| js::truthy(options)) else {
        return Vec::new();
    };
    let array =
        |value: Option<&Value>| value.and_then(Value::as_array).cloned().unwrap_or_default();
    if let Some(modes) = jsv::prop(options, "modes").filter(|modes| js::truthy(modes)) {
        let entry = jsv::prop(modes, mode).filter(|entry| js::truthy(entry));
        return array(jsv::field(entry, "combos"))
            .iter()
            .map(combo_of)
            .collect();
    }
    let fps = array(jsv::field(jsv::prop(options, "capabilities"), "fps"));
    let mut combos = Vec::new();
    for resolution in array(jsv::prop(options, "resolutions")) {
        for rate in &fps {
            combos.push(Combo {
                height: jsv::num(jsv::prop(&resolution, "height")).unwrap_or(f64::NAN),
                width: jsv::num(jsv::prop(&resolution, "width")),
                fps: jsv::num(Some(rate)).unwrap_or(f64::NAN),
            });
        }
    }
    combos
}

fn sorted_unique(mut values: Vec<f64>) -> Vec<f64> {
    values.sort_by(f64::total_cmp);
    values.dedup();
    values
}

/// `heightsOf(combos)`: высоты по возрастанию.
pub fn heights_of(combos: &[Combo]) -> Vec<f64> {
    sorted_unique(combos.iter().map(|combo| combo.height).collect())
}

/// `fpsFor(combos, height)`: частоты кадров высоты по возрастанию.
pub fn fps_for(combos: &[Combo], height: f64) -> Vec<f64> {
    sorted_unique(
        combos
            .iter()
            .filter(|combo| combo.height == height)
            .map(|combo| combo.fps)
            .collect(),
    )
}

/// `pickCombo(combos, height, fps)`: ближайшее допустимое — не выше выбранного (иначе самое
/// низкое). Нет сочетаний (или у высоты нет частот: высота NaN) — `None`.
pub fn pick_combo(combos: &[Combo], height: f64, fps: f64) -> Option<(f64, f64)> {
    let nearest = |values: &[f64], wanted: f64| {
        if values.contains(&wanted) {
            return Some(wanted);
        }
        values
            .iter()
            .rev()
            .find(|value| **value <= wanted)
            .or(values.first())
            .copied()
    };
    let h = nearest(&heights_of(combos), height)?;
    Some((h, nearest(&fps_for(combos, h), fps)?))
}

/// Текст с аргументами для `$t`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub key: String,
    pub args: Vec<String>,
}

impl Message {
    fn new(key: impl Into<String>, args: &[&str]) -> Message {
        Message {
            key: key.into(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        }
    }

    /// `$t(key, ...args)`.
    pub fn text(&self, t: Translate) -> String {
        let args: Vec<&str> = self.args.iter().map(String::as_str).collect();
        t(&self.key, &args)
    }
}

/// `durationText(seconds)`: «~45 с», «~21 мин», «~1 ч 05 мин».
pub fn duration_text(seconds: f64) -> Message {
    let value = if seconds.is_nan() {
        0.
    } else {
        seconds.max(0.)
    };
    if value < 60. {
        let rounded = ((value / 5.).ceil() * 5.).max(5.);
        return Message::new("#L_DEMO_EST_SEC", &[&js::number::number_to_string(rounded)]);
    }
    let minutes = (value / 60.).ceil() as u64;
    if minutes < 60 {
        return Message::new("#L_DEMO_EST_MIN", &[&minutes.to_string()]);
    }
    Message::new(
        "#L_DEMO_EST_HOUR",
        &[&(minutes / 60).to_string(), &format!("{:02}", minutes % 60)],
    )
}

/// `chapterKey(id)`: номер главы HL2 из метки → ключ её названия.
pub fn chapter_key(id: &str) -> Option<&'static str> {
    Some(match id {
        "1" => "#L_DEMO_CHAPTER_1",
        "2" => "#L_DEMO_CHAPTER_2",
        "3" => "#L_DEMO_CHAPTER_3",
        "4" => "#L_DEMO_CHAPTER_4",
        "5" => "#L_DEMO_CHAPTER_5",
        "6" => "#L_DEMO_CHAPTER_6",
        "7" => "#L_DEMO_CHAPTER_7",
        "8" => "#L_DEMO_CHAPTER_8",
        "9" => "#L_DEMO_CHAPTER_9",
        "9a" => "#L_DEMO_CHAPTER_9A",
        "10" => "#L_DEMO_CHAPTER_10",
        "11" => "#L_DEMO_CHAPTER_11",
        "12" => "#L_DEMO_CHAPTER_12",
        "13" => "#L_DEMO_CHAPTER_13",
        _ => return None,
    })
}

/// `String(value)`; `undefined` — пусто.
fn text_of(value: Option<&Value>) -> String {
    value.map(js::to_string).unwrap_or_default()
}

/// `markerText(marker)` страницы: подпись метки по типу (сегмент, загрузка, таймер, глава,
/// финиш); у карты — сама метка.
pub fn marker_text(marker: &Value, t: Translate) -> String {
    let label = text_of(jsv::prop(marker, "label"));
    let key = match jsv::str_of(jsv::prop(marker, "type")) {
        Some("segment") => "#L_DEMO_MARK_SEGMENT",
        Some("saveload") => "#L_DEMO_MARK_SAVELOAD",
        Some("timer") => "#L_DEMO_MARK_TIMER",
        Some("finish") => "#L_DEMO_MARK_FINISH",
        Some("chapter") => {
            let name = chapter_key(&label).map_or_else(|| label.clone(), |key| t(key, &[]));
            return t("#L_DEMO_MARK_CHAPTER", &[&name]);
        }
        _ => return label,
    };
    t(key, &[&label])
}

/// Что получает плеер (`playerInput`): видео только выбранной записи, иначе шкала — время её
/// демки с метками источника.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerInput {
    pub own: bool,
    pub src: Option<String>,
    pub video: Option<Value>,
    pub markers: Vec<Value>,
}

/// `playerInput(media, detail)`: `media` — `{ itemId, result }`, `detail` — ответ `demo-inspect`.
pub fn player_input(media: Option<&Value>, detail: Option<&Value>) -> PlayerInput {
    let markers = |value: Option<&Value>| {
        value
            .filter(|value| js::truthy(value))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let item_id = jsv::field(jsv::field(detail, "item"), "id");
    let own = media.is_some_and(js::truthy)
        && detail.is_some_and(js::truthy)
        && jsv::field(media, "itemId") == item_id;
    if own {
        let result = jsv::field(media, "result");
        return PlayerInput {
            own,
            src: jsv::str_of(jsv::field(result, "mediaUrl")).map(str::to_owned),
            video: jsv::field(result, "video").cloned(),
            markers: markers(jsv::field(result, "markers")),
        };
    }
    PlayerInput {
        own,
        src: None,
        video: None,
        markers: markers(jsv::field(detail, "markers")),
    }
}

/// Какое видео удалили: точный путь из ответа (`file`) или имя и папка вывода.
#[derive(Debug, Clone, Default)]
pub struct RenderedWhich<'a> {
    pub name: &'a str,
    pub output_dir: &'a str,
    pub file: Option<&'a str>,
}

/// `isRenderedFile(result, which)`: итог экспорта (`{ file, fileName }`) — это видео папки
/// вывода. Регистр путей не важен.
pub fn is_rendered_file(result: Option<&Value>, which: &RenderedWhich) -> bool {
    let Some(file) = jsv::str_of(jsv::field(result, "file")).filter(|file| !file.is_empty()) else {
        return false;
    };
    if let Some(exact) = which.file.filter(|exact| !exact.is_empty()) {
        return file.to_lowercase() == exact.to_lowercase();
    }
    let dir = match file.rfind(['\\', '/']) {
        Some(cut) => &file[..cut],
        None => file,
    };
    let output = which.output_dir.trim_end_matches(['\\', '/']);
    jsv::str_of(jsv::field(result, "fileName")) == Some(which.name)
        && dir.to_lowercase() == output.to_lowercase()
}

/// Край интервала: «Начало здесь» или «Конец здесь».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    From,
    To,
}

/// `markRange(range, which, demoTime, duration)`: время демки в пределах рана; конец раньше
/// начала — края меняются местами (`swapped`). Весь ран и пустой отрезок — `None`.
pub fn mark_range(
    range: Option<ClipRange>,
    which: Edge,
    demo_time: f64,
    duration: f64,
) -> (Option<ClipRange>, bool) {
    let length = if duration.is_nan() {
        0.
    } else {
        duration.max(0.)
    };
    let time = if demo_time.is_nan() { 0. } else { demo_time };
    let mut next = range.unwrap_or(ClipRange {
        from: 0.,
        to: length,
    });
    let value = length.min(time.max(0.));
    match which {
        Edge::From => next.from = value,
        Edge::To => next.to = value,
    }
    let swapped = next.to < next.from;
    if swapped {
        std::mem::swap(&mut next.from, &mut next.to);
    }
    if next.to <= next.from || (next.from <= 0. && next.to >= length) {
        return (None, false);
    }
    (Some(next), swapped)
}

/// Группа списка: игра или «Добавленные».
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub id: String,
    pub name: Option<&'static str>,
    pub items: Vec<Value>,
}

fn supported(item: &Value) -> bool {
    jsv::truthy(jsv::prop(item, "supported"))
}

/// `groupItems(items)`: игры в постоянном порядке, затем добавленные вручную; внутри —
/// поддерживаемые выше, новые сверху.
pub fn group_items(items: &[Value]) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for item in items {
        let key = if jsv::is_str(jsv::prop(item, "source"), "manual") {
            "manual".to_owned()
        } else {
            match jsv::prop(item, "gameId").filter(|id| js::truthy(id)) {
                Some(id) => js::to_string(id),
                None => "other".to_owned(),
            }
        };
        match groups.iter_mut().find(|group| group.id == key) {
            Some(group) => group.items.push(item.clone()),
            None => groups.push(Group {
                name: group_name(&key),
                id: key,
                items: vec![item.clone()],
            }),
        }
    }
    let rank = |key: &str| match key {
        "manual" => 1000,
        _ => GROUP_ORDER
            .iter()
            .position(|known| *known == key)
            .unwrap_or(500),
    };
    // shortcut: ключи групп — id игр ASCII, `localeCompare` равных рангов — побайтно; ICU нужен,
    // если появятся id не из ASCII.
    groups.sort_by(|a, b| rank(&a.id).cmp(&rank(&b.id)).then_with(|| a.id.cmp(&b.id)));
    for group in &mut groups {
        group.items.sort_by(|a, b| {
            supported(b).cmp(&supported(a)).then_with(|| {
                jsv::or_zero(jsv::prop(b, "mtimeMs"))
                    .total_cmp(&jsv::or_zero(jsv::prop(a, "mtimeMs")))
            })
        });
    }
    groups
}

/// `firstPick(groups)`: первая поддерживаемая запись, иначе первая.
pub fn first_pick(groups: &[Group]) -> Option<String> {
    let all = || groups.iter().flat_map(|group| group.items.iter());
    let pick = all()
        .find(|item| supported(item))
        .or_else(|| all().next())?;
    Some(text_of(jsv::prop(pick, "id")))
}

/// `hasDemoExtension(name)`.
pub fn has_demo_extension(name: &str) -> bool {
    let lower = name.to_lowercase();
    DEMO_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

/// `ERROR_KEYS[code]`: код главного процесса → ключ текста.
pub fn error_key(code: &str) -> Option<&'static str> {
    Some(match code {
        "CANCELLED" => "#L_DEMO_ERR_CANCELLED",
        "RENDER_NOT_READY" => "#L_DEMO_ERR_RENDER_NOT_READY",
        "RENDER_FAILED" => "#L_DEMO_ERR_RENDER_FAILED",
        "DEMO_UNSUPPORTED" => "#L_DEMO_ERR_UNSUPPORTED",
        "DEMO_PROTOCOL_UNSUPPORTED" => "#L_DEMO_ERR_PROTOCOL",
        "DEMO_TRUNCATED" => "#L_DEMO_ERR_TRUNCATED",
        "DEMO_HEADER_INVALID" | "DEMO_DIRECTORY_INVALID" => "#L_DEMO_ERR_BROKEN",
        "DEMO_NOT_A_DEMO" => "#L_DEMO_ERR_NOT_DEMO",
        "DEMO_NOT_FOUND" => "#L_DEMO_ERR_NOT_FOUND",
        "DEMO_READ_FAILED" | "DEMO_TOO_LARGE" => "#L_DEMO_ERR_READ",
        "PLAYLIST_INVALID" => "#L_DEMO_ERR_PLAYLIST",
        "PLAYLIST_PATH_ESCAPE" => "#L_DEMO_ERR_PLAYLIST_PATH",
        "SEGMENT_MISSING" => "#L_DEMO_ERR_SEGMENT_MISSING",
        "SEGMENTS_MIXED" => "#L_DEMO_ERR_MIXED",
        "DURATION_UNKNOWN" => "#L_DEMO_ERR_DURATION",
        "FILE_TYPE_UNSUPPORTED" => "#L_DEMO_ERR_FILE_TYPE",
        "PATH_INVALID" => "#L_DEMO_ERR_PATH",
        "LIST_FULL" => "#L_DEMO_ERR_LIST_FULL",
        "FFMPEG_MISSING" | "NOT_INSTALLED" => "#L_DEMO_ERR_FFMPEG_MISSING",
        "FFMPEG_NOT_IN_PATH" | "FFMPEG_INSTALL_UNSUPPORTED" => "#L_DEMO_ERR_FFMPEG_PATH",
        "FFMPEG_BROKEN" => "#L_DEMO_ERR_FFMPEG_BROKEN",
        "FFMPEG_VERSION_MISMATCH" => "#L_DEMO_ERR_FFMPEG_VERSION",
        "FFMPEG_FAILED" => "#L_DEMO_ERR_FFMPEG_FAILED",
        "FFMPEG_START_FAILED" => "#L_DEMO_ERR_FFMPEG_START",
        "FFMPEG_TIMEOUT" => "#L_DEMO_ERR_FFMPEG_TIMEOUT",
        "VERIFY_FAILED" => "#L_DEMO_ERR_VERIFY",
        "OUTPUT_WRITE_FAILED" => "#L_DEMO_ERR_WRITE",
        "OUTPUT_DIR_UNAVAILABLE" => "#L_DEMO_ERR_OUTPUT_DIR",
        "NO_SPACE" => "#L_DEMO_ERR_NO_SPACE",
        "JOB_BUSY" => "#L_DEMO_ERR_BUSY",
        "COMBO_UNSUPPORTED" => "#L_DEMO_ERR_COMBO",
        "ENCODER_GPU_UNAVAILABLE" => "#L_DEMO_ERR_NO_GPU",
        "ENCODER_UNAVAILABLE" => "#L_DEMO_ERR_NO_ENCODER",
        "RANGE_INVALID" => "#L_DEMO_ERR_RANGE",
        "RESULT_NOT_FOUND" => "#L_DEMO_ERR_RESULT_GONE",
        "TIMELINE_INVALID" => "#L_DEMO_ERR_TIMELINE",
        "NETWORK_ERROR" | "NETWORK_TIMEOUT" => "#L_DEMO_ERR_NETWORK",
        "SIZE_MISMATCH"
        | "CHECKSUM_MISMATCH"
        | "EXTRACT_FAILED"
        | "ARCHIVE_LIMIT_EXCEEDED"
        | "ARCHIVE_LIST_FAILED"
        | "ARCHIVE_UNSAFE_ENTRY" => "#L_DEMO_ERR_DOWNLOAD",
        "ANTIVIRUS_BLOCKED" => "#L_DEMO_ERR_ANTIVIRUS",
        "PATH_NOT_WRITABLE" => "#L_DEMO_ERR_TOOLS_DIR",
        "ALREADY_RUNNING" => "#L_DEMO_ERR_INSTALLING",
        "FFMPEG_INSTALLING" => "#L_DEMO_ERR_FFMPEG_INSTALLING",
        "JOBS_DIR_UNAVAILABLE" => "#L_DEMO_ERR_JOBS_DIR",
        "JOBS_DIR_UNSAFE" => "#L_DEMO_ERR_JOBS_DIR_UNSAFE",
        "STEAM_NOT_RUNNING" => "#L_DEMO_ERR_STEAM_NOT_RUNNING",
        "GAME_RUNNING" => "#L_DEMO_ERR_GAME_RUNNING",
        // Тот же текст, что у запуска игры во время записи (`#L_D_ERR_<код>` оболочки).
        "RENDER_ACTIVE" => "#L_D_ERR_RENDER_ACTIVE",
        "STEAM_NOT_INSTALLED" => "#L_DEMO_ERR_STEAM_NOT_INSTALLED",
        "STEAM_ELEVATED" => "#L_DEMO_ERR_STEAM_ELEVATED",
        "SEGMENT_CHANGED" => "#L_DEMO_ERR_SEGMENT_CHANGED",
        "MAP_MISSING" => "#L_DEMO_ERR_MAP_MISSING",
        "DEMO_OPEN_FAILED" => "#L_DEMO_ERR_DEMO_OPEN",
        "GAME_ERROR" => "#L_DEMO_ERR_GAME_ERROR",
        "GAME_EXITED" => "#L_DEMO_ERR_GAME_EXITED",
        "RENDER_TIMEOUT" => "#L_DEMO_ERR_RENDER_TIMEOUT",
        "RENDER_HOST_MISSING" => "#L_DEMO_ERR_HOST_MISSING",
        "RENDER_HOST_FAILED" => "#L_DEMO_ERR_HOST_FAILED",
        "REGISTRY_FAILED" => "#L_DEMO_ERR_REGISTRY",
        "AUDIO_MISSING" => "#L_DEMO_ERR_AUDIO_MISSING",
        "AUDIO_INVALID" => "#L_DEMO_ERR_AUDIO_INVALID",
        "GAME_NOT_INSTALLED" => "#L_DEMO_ERR_GAME_NOT_INSTALLED",
        "DEMO_EMPTY" => "#L_DEMO_ERR_DEMO_EMPTY",
        "DIALOG_UNAVAILABLE" => "#L_DEMO_ERR_DIALOG",
        "ORDER_INVALID" => "#L_DEMO_ERR_ORDER",
        "SETTINGS_INVALID" => "#L_DEMO_ERR_SETTINGS",
        "TOOLS_UNAVAILABLE" => "#L_DEMO_ERR_TOOLS",
        "UNTRUSTED" => "#L_DEMO_ERR_UNTRUSTED",
        "REMOTE_DISABLED" => "#L_DEMO_ERR_REMOTE_DISABLED",
        "MANIFEST_INVALID" => "#L_DEMO_ERR_MANIFEST",
        "GAME_BUSY" => "#L_DEMO_ERR_GAME_BUSY",
        "RESOLUTION_LIMITED" => "#L_DEMO_ERR_RESOLUTION_LIMITED",
        "BXTRS_MISSING" => "#L_DEMO_ERR_BXTRS_MISSING",
        "BXTRS_BROKEN" => "#L_DEMO_ERR_BXTRS_BROKEN",
        "BXTRS_FAILED" => "#L_DEMO_ERR_BXTRS_FAILED",
        "BXTRS_INSTALL_UNSUPPORTED" => "#L_DEMO_ERR_RENDER_NOT_READY",
        "GOLDSRC_RUNNING" => "#L_DEMO_ERR_GOLDSRC_RUNNING",
        "FFMPEG_IN_GAME_DIR" => "#L_DEMO_ERR_FFMPEG_IN_GAME_DIR",
        "SEGMENT_REPLACED" => "#L_DEMO_ERR_SEGMENT_REPLACED",
        "GHRENDER_UNSUPPORTED" => "#L_DEMO_ERR_GHRENDER_UNSUPPORTED",
        "BAD_OPTION" => "#L_DEMO_ERR_BAD_OPTION",
        "SEGMENT_MALFORMED" => "#L_DEMO_ERR_SEGMENT_MALFORMED",
        "SEGMENT_UNSAFE" => "#L_DEMO_ERR_SEGMENT_UNSAFE",
        "DEMO_FAILED" | "JOB_FAILED" => "#L_DEMO_ERR_FAILED",
        "KEYHUD_NOT_AVAILABLE" => "#L_DEMO_EX_KEYHUD_NA",
        "TRASH_FAILED" => "#L_DEMO_ERR_TRASH",
        _ => return None,
    })
}

/// `WARNING_KEYS[code]`: предупреждения плана и итога; неизвестные не показываются.
pub fn warning_key(code: &str) -> Option<&'static str> {
    Some(match code {
        "DISCORD_LOW_QUALITY" => "#L_DEMO_WARN_DISCORD_LOW_QUALITY",
        "CPU_SLOW" => "#L_DEMO_WARN_CPU_SLOW",
        "KEYHUD_UNAVAILABLE" => "#L_DEMO_EX_KEYHUD_NA",
        "AUDIO_UNAVAILABLE" => "#L_DEMO_EX_AUDIO_NA",
        "KEYHUD_PARTIAL" => "#L_DEMO_WARN_KEYHUD_PARTIAL",
        _ => return None,
    })
}

/// `warningKeys(warnings)`: ключи без повторов, в порядке первого появления.
pub fn warning_keys(warnings: &[&str]) -> Vec<&'static str> {
    let mut keys = Vec::new();
    for key in warnings.iter().filter_map(|code| warning_key(code)) {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}

/// `errorMessage(error)`: `{ key, args }` текста ошибки; неизвестный код — общий текст с кодом.
pub fn error_message(error: &Value) -> Message {
    let code = jsv::str_of(jsv::prop(error, "code"))
        .filter(|code| !code.is_empty())
        .unwrap_or("JOB_FAILED");
    let field = |name: &str| jsv::prop(error, name).filter(|value| !value.is_null());
    let value = |name: &str| field(name).map_or_else(|| "?".to_owned(), js::to_string);
    let truthy = |name: &str| jsv::truthy(jsv::prop(error, name));
    if code == "DISCORD_TOO_LONG" {
        let seconds = jsv::or_zero(jsv::prop(error, "recommendedMaxSeconds"));
        let seconds = if seconds.is_nan() {
            0.
        } else {
            seconds.floor().max(0.)
        };
        let seconds = js::number::number_to_string(seconds);
        return if truthy("suggestedHeight") {
            let height = jsv::num(jsv::prop(error, "suggestedHeight")).unwrap_or(f64::NAN);
            Message::new(
                "#L_DEMO_ERR_DISCORD_LONG_RES",
                &[&seconds, &resolution_label(height)],
            )
        } else {
            Message::new("#L_DEMO_ERR_DISCORD_LONG", &[&seconds])
        };
    }
    let key = error_key(code);
    match code {
        "FFMPEG_FAILED" => return Message::new(key.unwrap_or_default(), &[&value("exitCode")]),
        // У HL1 кода выхода игры нет (запускает Injector) — текст без него.
        "GAME_EXITED" => {
            return match field("exitCode") {
                None => Message::new("#L_DEMO_ERR_GAME_EXITED_ANY", &[]),
                Some(_) => Message::new("#L_DEMO_ERR_GAME_EXITED", &[&value("exitCode")]),
            };
        }
        "SEGMENT_MISSING" | "SEGMENT_CHANGED" | "SEGMENT_REPLACED" => {
            let key = key.unwrap_or_default();
            let number = if truthy("segment") {
                value("segment")
            } else {
                String::new()
            };
            let name = if truthy("name") {
                value("name")
            } else {
                String::new()
            };
            return match (number.is_empty(), name.is_empty()) {
                (false, false) => Message::new(format!("{key}_NAMED"), &[&number, &name]),
                (false, true) => Message::new(key, &[&number]),
                (true, false) => Message::new(key, &[&name]),
                (true, true) => Message::new(format!("{key}_ANY"), &[]),
            };
        }
        "SEGMENT_MALFORMED" | "SEGMENT_UNSAFE" => {
            let key = key.unwrap_or_default();
            return if truthy("segment") {
                Message::new(key, &[&value("segment")])
            } else {
                Message::new(format!("{key}_ANY"), &[])
            };
        }
        "GAME_NOT_INSTALLED" if truthy("gameName") => {
            return Message::new(
                "#L_DEMO_ERR_GAME_NOT_INSTALLED_NAMED",
                &[&value("gameName")],
            );
        }
        "SETTINGS_INVALID" => return Message::new(key.unwrap_or_default(), &[&value("field")]),
        "RESOLUTION_LIMITED" => {
            return Message::new(
                key.unwrap_or_default(),
                &[&value("width"), &value("height")],
            );
        }
        // HL1 (bxt-rs) счёта кадров не даёт — текст без чисел.
        "RENDER_TIMEOUT" => {
            return if truthy("framesTotal") {
                Message::new(
                    key.unwrap_or_default(),
                    &[&value("framesDone"), &value("framesTotal")],
                )
            } else {
                Message::new("#L_DEMO_ERR_RENDER_TIMEOUT_ANY", &[])
            };
        }
        _ => {}
    }
    match key {
        Some(key) => Message::new(key, &[]),
        None => Message::new("#L_DEMO_ERR_GENERIC", &[code]),
    }
}

/// `errorText(error)` страницы: `$t` текста ошибки.
pub fn error_text(error: &Value, t: Translate) -> String {
    error_message(error).text(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn format_clock_and_parse_clock() {
        assert_eq!(format_clock(0., 1), "0:00.0");
        assert_eq!(format_clock(65.34, 1), "1:05.3");
        assert_eq!(format_clock(59.96, 1), "1:00.0");
        assert_eq!(format_clock(3725.5, 1), "1:02:05.5");
        assert_eq!(format_clock(144.46, 0), "2:24");
        assert_eq!(format_clock(f64::NAN, 1), "0:00.0");
        assert_eq!(format_clock(-4., 1), "0:00.0");
        assert_eq!(parse_clock("1:05.3"), Some(65.3));
        assert_eq!(parse_clock("65,3"), Some(65.3));
        assert_eq!(parse_clock("1:02:05"), Some(3725.));
        assert_eq!(parse_clock(" 12 "), Some(12.));
        for bad in ["", "abc", "1:75", "1.5:10", "-3", "1:2:3:4", "1.", ".5"] {
            assert_eq!(parse_clock(bad), None, "{bad}");
        }
    }

    #[test]
    fn demo_time_from_clip_time_with_range_speed_and_cut_loads() {
        let video = json!({"range": {"from": 2, "to": 10}, "speed": 2, "duration": 4});
        // Без меток: от + время × скорость.
        assert_eq!(demo_time_at(0., Some(&video), &[]), 2.);
        assert_eq!(demo_time_at(1.5, Some(&video), &[]), 5.);
        assert_eq!(
            demo_time_at(99., Some(&video), &[]),
            10.,
            "не дальше конца интервала"
        );
        // Метка после вырезанной загрузки: 3 с демо выпали, метка в видео на 1 с.
        let markers = [
            json!({"t": 0, "sourceTime": 2}),
            json!({"t": 1, "sourceTime": 7}),
        ];
        assert_eq!(demo_time_at(0.5, Some(&video), &markers), 3.);
        assert_eq!(demo_time_at(1.5, Some(&video), &markers), 8.);
        // Обратно — для подсветки интервала на полосе.
        assert_eq!(video_time_at(8., Some(&video), &markers), 1.5);
        assert_eq!(video_time_at(3., Some(&video), &markers), 0.5);
        // Предпросмотр всего рана: время совпадает.
        let whole = json!({"range": {"from": 0, "to": 144.46}, "speed": 1});
        assert_eq!(demo_time_at(12.25, Some(&whole), &[]), 12.25);
        // Без видео — время как есть; метка без sourceTime не якорь.
        assert_eq!(demo_time_at(7., None, &[json!({"t": 1})]), 7.);
    }

    #[test]
    fn the_player_gets_only_the_selected_demo_video_and_marks_only_its_time() {
        let gh2 = json!({"item": {"id": "gh2", "duration": 144.46},
            "markers": [{"t": 25.85, "type": "map", "label": "d1_trainstation_02"}]});
        let gh1 = json!({"item": {"id": "gh1", "duration": 52.31},
            "markers": [{"t": 4.36, "type": "saveload", "label": "2"}]});
        let media = json!({"itemId": "gh2", "result": {"mediaUrl": "hlsr-media://media/x",
            "video": {"range": {"from": 0, "to": 20}, "speed": 1},
            "markers": [{"t": 0, "type": "map", "sourceTime": 0}]}});
        let own = player_input(Some(&media), Some(&gh2));
        assert!(own.own);
        assert_eq!(own.src.as_deref(), Some("hlsr-media://media/x"));
        assert_eq!(own.video, Some(media["result"]["video"].clone()));
        assert_eq!(
            own.markers,
            media["result"]["markers"].as_array().unwrap().clone()
        );
        let foreign = player_input(Some(&media), Some(&gh1));
        assert_eq!(
            foreign,
            PlayerInput {
                own: false,
                src: None,
                video: None,
                markers: gh1["markers"].as_array().unwrap().clone()
            },
            "чужое видео не в плеере"
        );
        assert_eq!(player_input(None, Some(&gh1)).markers, foreign.markers);
        assert_eq!(
            player_input(Some(&media), None),
            PlayerInput {
                own: false,
                src: None,
                video: None,
                markers: vec![]
            }
        );
        // Отметки прижимаются к длине своей демки.
        let range = |from: f64, to: f64| Some(ClipRange { from, to });
        assert_eq!(
            mark_range(None, Edge::From, 10., 52.31),
            (range(10., 52.31), false)
        );
        assert_eq!(
            mark_range(range(10., 30.), Edge::To, 140., 52.31),
            (range(10., 52.31), false),
            "время чужой, более длинной демки прижато к своей"
        );
        assert_eq!(
            mark_range(range(10., 20.), Edge::From, 30., 52.31),
            (range(20., 30.), true)
        );
        assert_eq!(
            mark_range(range(30., 52.31), Edge::To, 5., 52.31),
            (range(5., 30.), true)
        );
        assert_eq!(
            mark_range(None, Edge::From, 0., 52.31),
            (None, false),
            "весь ран"
        );
        assert_eq!(mark_range(None, Edge::To, 0., 52.31), (None, false));
        assert_eq!(mark_range(None, Edge::From, 52.31, 52.31), (None, false));
    }

    #[test]
    fn resolution_and_fps_combos_from_demo_options() {
        let options = json!({
            "resolutions": [{"height": 720, "width": 1280}, {"height": 1080, "width": 1920},
                {"height": 2160, "width": 3840}],
            "capabilities": {"fps": [30, 60, 120]},
            "modes": {
                "auto": {"encoder": "h264_nvenc", "combos": [
                    {"height": 720, "fps": 30}, {"height": 720, "fps": 60}, {"height": 720, "fps": 120},
                    {"height": 1080, "fps": 30}, {"height": 1080, "fps": 60},
                    {"height": 2160, "fps": 30}]},
                "gpu": null,
                "cpu": {"encoder": "libx264", "combos": [{"height": 720, "fps": 30}]}
            }
        });
        let auto = combos_for(Some(&options), "auto");
        assert_eq!(heights_of(&auto), [720., 1080., 2160.]);
        assert_eq!(fps_for(&auto, 1080.), [30., 60.]);
        assert_eq!(
            pick_combo(&auto, 1080., 60.),
            Some((1080., 60.)),
            "умолчание 1080p60"
        );
        assert_eq!(
            pick_combo(&auto, 1080., 120.),
            Some((1080., 60.)),
            "120 нет — ниже"
        );
        assert_eq!(
            pick_combo(&auto, 1440., 60.),
            Some((1080., 60.)),
            "1440 нет — ниже"
        );
        assert_eq!(pick_combo(&auto, 2160., 60.), Some((2160., 30.)));
        assert_eq!(
            pick_combo(&auto, 480., 10.),
            Some((720., 30.)),
            "ниже всех — самое низкое"
        );
        assert!(
            combos_for(Some(&options), "gpu").is_empty(),
            "GPU нет — выбрать нечего"
        );
        assert_eq!(pick_combo(&[], 1080., 60.), None);
        let no_height = Combo {
            height: f64::NAN,
            width: None,
            fps: 60.,
        };
        assert_eq!(pick_combo(&[no_height], 1080., 60.), None);
        // FFmpeg не готов: возможности адаптера целиком.
        let mut pending = options.clone();
        pending["modes"] = Value::Null;
        assert_eq!(combos_for(Some(&pending), "auto").len(), 9);
        assert!(combos_for(None, "auto").is_empty());
    }

    #[test]
    fn list_groups_hl1_ghosting_other_games_then_added() {
        let groups = group_items(&[
            json!({"id": "m", "source": "manual", "gameId": null, "mtimeMs": 5}),
            json!({"id": "g1", "source": "game", "gameId": "ghosting-renewed", "mtimeMs": 1}),
            json!({"id": "h1", "source": "game", "gameId": "hl1-won", "mtimeMs": 1}),
            json!({"id": "h2", "source": "game", "gameId": "hl1-won", "mtimeMs": 9}),
            json!({"id": "x", "source": "game", "gameId": "hl2-4260", "mtimeMs": 1}),
        ]);
        let ids: Vec<&str> = groups.iter().map(|group| group.id.as_str()).collect();
        assert_eq!(ids, ["hl1-won", "ghosting-renewed", "hl2-4260", "manual"]);
        let items: Vec<&Value> = groups[0].items.iter().map(|item| &item["id"]).collect();
        assert_eq!(items, [&json!("h2"), &json!("h1")], "новые сверху");
        assert_eq!(groups[0].name, Some("Half-Life"));
        assert_eq!(groups[3].name, None);
        // Неподдерживаемые — после поддерживаемых, даже если новее.
        let broken = group_items(&[
            json!({"id": "bad", "source": "game", "gameId": "ghosting-renewed", "supported": false, "mtimeMs": 99}),
            json!({"id": "run", "source": "game", "gameId": "ghosting-renewed", "supported": true, "mtimeMs": 1}),
        ]);
        let items: Vec<&Value> = broken[0].items.iter().map(|item| &item["id"]).collect();
        assert_eq!(items, [&json!("run"), &json!("bad")]);
        assert_eq!(first_pick(&broken).as_deref(), Some("run"));
        let only = group_items(&[json!({"id": "only", "source": "manual", "supported": false})]);
        assert_eq!(
            first_pick(&only).as_deref(),
            Some("only"),
            "нет поддерживаемых — первая"
        );
        assert_eq!(first_pick(&[]), None);
        // Неизвестная игра — после известных, до добавленных.
        let other = group_items(&[
            json!({"id": "m", "source": "manual"}),
            json!({"id": "z", "source": "game"}),
            json!({"id": "h", "source": "game", "gameId": "hl1-won"}),
        ]);
        let ids: Vec<&str> = other.iter().map(|group| group.id.as_str()).collect();
        assert_eq!(ids, ["hl1-won", "other", "manual"]);
    }

    /// Строки языка (не строки — служебные поля файла — пропускаются).
    fn locale(lang: &str) -> BTreeMap<String, String> {
        let path = format!("{}/locales/{lang}.json", env!("CARGO_MANIFEST_DIR"));
        let all: BTreeMap<String, Value> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        all.into_iter()
            .filter_map(|(key, value)| Some((key, value.as_str()?.to_owned())))
            .collect()
    }

    #[test]
    fn error_codes_give_texts_and_every_text_exists_in_ru_and_en() {
        let message = |error: Value| error_message(&error);
        assert_eq!(
            message(json!({"code": "RENDER_NOT_READY"})),
            Message::new("#L_DEMO_ERR_RENDER_NOT_READY", &[])
        );
        assert_eq!(
            message(
                json!({"code": "DISCORD_TOO_LONG", "recommendedMaxSeconds": 118.7, "suggestedHeight": 720})
            ),
            Message::new("#L_DEMO_ERR_DISCORD_LONG_RES", &["118", "720p"])
        );
        assert_eq!(
            message(
                json!({"code": "DISCORD_TOO_LONG", "recommendedMaxSeconds": 40, "suggestedHeight": null})
            ),
            Message::new("#L_DEMO_ERR_DISCORD_LONG", &["40"])
        );
        assert_eq!(
            message(json!({"code": "FFMPEG_FAILED", "exitCode": 1})).args,
            ["1"]
        );
        assert_eq!(message(json!({"code": "FFMPEG_FAILED"})).args, ["?"]);
        assert_eq!(
            message(json!({"code": "SOMETHING_NEW"})),
            Message::new("#L_DEMO_ERR_GENERIC", &["SOMETHING_NEW"])
        );
        assert_eq!(
            message(json!({})),
            Message::new("#L_DEMO_ERR_FAILED", &[]),
            "без кода — JOB_FAILED"
        );
        assert_eq!(
            message(json!({"code": "RENDER_TIMEOUT", "framesDone": 120, "framesTotal": 8668})).args,
            ["120", "8668"]
        );
        assert_eq!(
            message(json!({"code": "RENDER_TIMEOUT"})).key,
            "#L_DEMO_ERR_RENDER_TIMEOUT_ANY"
        );
        assert_eq!(
            message(json!({"code": "GAME_EXITED", "exitCode": 3})).args,
            ["3"]
        );
        assert_eq!(
            message(json!({"code": "SEGMENT_CHANGED", "segment": 17})),
            Message::new("#L_DEMO_ERR_SEGMENT_CHANGED", &["17"])
        );
        assert_eq!(
            message(json!({"code": "GAME_EXITED"})),
            Message::new("#L_DEMO_ERR_GAME_EXITED_ANY", &[])
        );
        assert_eq!(
            message(json!({"code": "SEGMENT_REPLACED", "segment": 2, "name": "c1a0d.dem"})),
            Message::new("#L_DEMO_ERR_SEGMENT_REPLACED_NAMED", &["2", "c1a0d.dem"])
        );
        assert_eq!(
            message(json!({"code": "SEGMENT_MISSING"})).key,
            "#L_DEMO_ERR_SEGMENT_MISSING_ANY"
        );
        assert_eq!(
            message(json!({"code": "SEGMENT_UNSAFE"})).key,
            "#L_DEMO_ERR_SEGMENT_UNSAFE_ANY"
        );
        assert_eq!(
            message(json!({"code": "RESOLUTION_LIMITED", "width": 1707, "height": 960})),
            Message::new("#L_DEMO_ERR_RESOLUTION_LIMITED", &["1707", "960"])
        );
        assert_eq!(
            message(json!({"code": "GAME_NOT_INSTALLED", "gameName": "Ghosting"})),
            Message::new("#L_DEMO_ERR_GAME_NOT_INSTALLED_NAMED", &["Ghosting"])
        );
        assert_eq!(
            warning_keys(&["KEYHUD_PARTIAL"]),
            ["#L_DEMO_WARN_KEYHUD_PARTIAL"]
        );
        assert_eq!(
            warning_keys(&["CPU_SLOW", "UNKNOWN", "CPU_SLOW"]),
            ["#L_DEMO_WARN_CPU_SLOW"]
        );
        for code in [
            "GOLDSRC_RUNNING",
            "FFMPEG_IN_GAME_DIR",
            "DEMO_TOO_LARGE",
            "GAME_RUNNING",
            "STEAM_NOT_RUNNING",
            "STEAM_NOT_INSTALLED",
            "STEAM_ELEVATED",
            "MAP_MISSING",
            "DEMO_OPEN_FAILED",
            "GAME_ERROR",
            "RENDER_HOST_MISSING",
            "RENDER_HOST_FAILED",
            "REGISTRY_FAILED",
            "AUDIO_MISSING",
            "AUDIO_INVALID",
            "GAME_NOT_INSTALLED",
            "DEMO_EMPTY",
            "RENDER_ACTIVE",
        ] {
            assert_ne!(
                message(json!({"code": code})).key,
                "#L_DEMO_ERR_GENERIC",
                "{code}"
            );
        }
        assert_eq!(error_key("RENDER_ACTIVE"), Some("#L_D_ERR_RENDER_ACTIVE"));

        // Все тексты кодов, предупреждений и вариантов (`_ANY`, `_NAMED`) — в обоих языках, с
        // одинаковыми параметрами.
        let (ru, en) = (locale("ru"), locale("en"));
        let mut keys: BTreeSet<String> = [
            "#L_DEMO_ERR_GENERIC",
            "#L_DEMO_ERR_DISCORD_LONG",
            "#L_DEMO_ERR_DISCORD_LONG_RES",
            "#L_DEMO_ERR_GAME_EXITED_ANY",
            "#L_DEMO_ERR_RENDER_TIMEOUT_ANY",
            "#L_DEMO_ERR_GAME_NOT_INSTALLED_NAMED",
            "#L_DEMO_ERR_SEGMENT_MALFORMED_ANY",
            "#L_DEMO_ERR_SEGMENT_UNSAFE_ANY",
            "#L_DEMO_EST_SEC",
            "#L_DEMO_EST_MIN",
            "#L_DEMO_EST_HOUR",
            "#L_DEMO_MARK_SEGMENT",
            "#L_DEMO_MARK_SAVELOAD",
            "#L_DEMO_MARK_TIMER",
            "#L_DEMO_MARK_CHAPTER",
            "#L_DEMO_MARK_FINISH",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        for key in ["SEGMENT_MISSING", "SEGMENT_CHANGED", "SEGMENT_REPLACED"] {
            let base = error_key(key).unwrap();
            keys.extend([format!("{base}_ANY"), format!("{base}_NAMED")]);
        }
        for id in [
            "1", "2", "3", "4", "5", "6", "7", "8", "9", "9a", "10", "11", "12", "13",
        ] {
            keys.insert(chapter_key(id).unwrap().to_owned());
        }
        for code in ERROR_CODES {
            keys.insert(error_key(code).unwrap().to_owned());
        }
        for code in [
            "DISCORD_LOW_QUALITY",
            "CPU_SLOW",
            "KEYHUD_UNAVAILABLE",
            "AUDIO_UNAVAILABLE",
            "KEYHUD_PARTIAL",
        ] {
            keys.insert(warning_key(code).unwrap().to_owned());
        }
        let params = |text: &str| {
            (0..10)
                .filter(|i| text.contains(&format!("%{i}%")))
                .collect::<Vec<_>>()
        };
        for key in &keys {
            let (Some(r), Some(e)) = (ru.get(key), en.get(key)) else {
                panic!("нет текста {key}");
            };
            assert_eq!(params(r), params(e), "параметры {key}");
        }
    }

    /// Коды, у которых есть свой текст (`ERROR_KEYS` 0.4.9).
    const ERROR_CODES: &[&str] = &[
        "CANCELLED",
        "RENDER_NOT_READY",
        "RENDER_FAILED",
        "DEMO_UNSUPPORTED",
        "DEMO_PROTOCOL_UNSUPPORTED",
        "DEMO_TRUNCATED",
        "DEMO_HEADER_INVALID",
        "DEMO_DIRECTORY_INVALID",
        "DEMO_NOT_A_DEMO",
        "DEMO_NOT_FOUND",
        "DEMO_READ_FAILED",
        "PLAYLIST_INVALID",
        "PLAYLIST_PATH_ESCAPE",
        "SEGMENT_MISSING",
        "SEGMENTS_MIXED",
        "DURATION_UNKNOWN",
        "FILE_TYPE_UNSUPPORTED",
        "PATH_INVALID",
        "LIST_FULL",
        "FFMPEG_MISSING",
        "FFMPEG_NOT_IN_PATH",
        "FFMPEG_BROKEN",
        "FFMPEG_VERSION_MISMATCH",
        "FFMPEG_FAILED",
        "FFMPEG_START_FAILED",
        "FFMPEG_TIMEOUT",
        "FFMPEG_INSTALL_UNSUPPORTED",
        "VERIFY_FAILED",
        "OUTPUT_WRITE_FAILED",
        "OUTPUT_DIR_UNAVAILABLE",
        "NO_SPACE",
        "JOB_BUSY",
        "COMBO_UNSUPPORTED",
        "ENCODER_GPU_UNAVAILABLE",
        "ENCODER_UNAVAILABLE",
        "RANGE_INVALID",
        "RESULT_NOT_FOUND",
        "TIMELINE_INVALID",
        "NETWORK_ERROR",
        "NETWORK_TIMEOUT",
        "SIZE_MISMATCH",
        "CHECKSUM_MISMATCH",
        "EXTRACT_FAILED",
        "ANTIVIRUS_BLOCKED",
        "PATH_NOT_WRITABLE",
        "ALREADY_RUNNING",
        "FFMPEG_INSTALLING",
        "JOBS_DIR_UNAVAILABLE",
        "JOBS_DIR_UNSAFE",
        "STEAM_NOT_RUNNING",
        "GAME_RUNNING",
        "RENDER_ACTIVE",
        "STEAM_NOT_INSTALLED",
        "STEAM_ELEVATED",
        "SEGMENT_CHANGED",
        "MAP_MISSING",
        "DEMO_OPEN_FAILED",
        "GAME_ERROR",
        "GAME_EXITED",
        "RENDER_TIMEOUT",
        "RENDER_HOST_MISSING",
        "RENDER_HOST_FAILED",
        "REGISTRY_FAILED",
        "AUDIO_MISSING",
        "AUDIO_INVALID",
        "GAME_NOT_INSTALLED",
        "DEMO_EMPTY",
        "DIALOG_UNAVAILABLE",
        "ORDER_INVALID",
        "SETTINGS_INVALID",
        "TOOLS_UNAVAILABLE",
        "UNTRUSTED",
        "REMOTE_DISABLED",
        "MANIFEST_INVALID",
        "ARCHIVE_LIMIT_EXCEEDED",
        "ARCHIVE_LIST_FAILED",
        "ARCHIVE_UNSAFE_ENTRY",
        "NOT_INSTALLED",
        "GAME_BUSY",
        "RESOLUTION_LIMITED",
        "BXTRS_MISSING",
        "BXTRS_BROKEN",
        "BXTRS_FAILED",
        "BXTRS_INSTALL_UNSUPPORTED",
        "GOLDSRC_RUNNING",
        "FFMPEG_IN_GAME_DIR",
        "SEGMENT_REPLACED",
        "DEMO_TOO_LARGE",
        "GHRENDER_UNSUPPORTED",
        "BAD_OPTION",
        "SEGMENT_MALFORMED",
        "SEGMENT_UNSAFE",
        "DEMO_FAILED",
        "JOB_FAILED",
        "KEYHUD_NOT_AVAILABLE",
        "TRASH_FAILED",
    ];

    /// Коды берутся из исходников бэкенда `hlsr-demo`: новый код без текста валит тест, а не
    /// показывает игроку «Что-то пошло не так (КОД)».
    #[test]
    fn every_code_of_the_demo_backend_has_its_own_text() {
        let dir = format!("{}/../hlsr-demo/src", env!("CARGO_MANIFEST_DIR"));
        let mut codes = BTreeSet::new();
        let mut stack = vec![std::path::PathBuf::from(dir)];
        while let Some(path) = stack.pop() {
            for entry in std::fs::read_dir(&path).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                for piece in text.split('"').skip(1).step_by(2) {
                    let code = piece.as_bytes();
                    let shaped = code.len() > 3
                        && code[0].is_ascii_uppercase()
                        && piece.contains('_')
                        && code
                            .iter()
                            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_');
                    if shaped {
                        codes.insert(piece.to_owned());
                    }
                }
            }
        }
        // Не коды: переменные окружения, ключ реестра, предупреждения (у них свои тексты);
        // сырые коды ghrender, которые `ghrender.rs` сводит к RENDER_FAILED, и префикс `SEGMENT_`.
        let skip = |code: &str| {
            [
                "HLSR_",
                "NODE_",
                "SLINT_",
                "WINIT_",
                "ELECTRON_",
                "HKEY_",
                "BXT_",
            ]
            .iter()
            .any(|prefix| code.starts_with(prefix))
                || warning_key(code).is_some()
                || ["DISCORD_TOO_LONG", "WRITE_FAILED", "NO_FRAMES", "SEGMENT_"].contains(&code)
        };
        let checked: Vec<&String> = codes.iter().filter(|code| !skip(code)).collect();
        assert!(checked.len() > 30, "кодов найдено: {}", checked.len());
        let missing: Vec<&&String> = checked
            .iter()
            .filter(|code| error_message(&json!({"code": code})).key == "#L_DEMO_ERR_GENERIC")
            .collect();
        assert!(missing.is_empty(), "коды без текста: {missing:?}");
    }

    #[test]
    fn duration_in_words_seconds_minutes_hours() {
        assert_eq!(duration_text(3.), Message::new("#L_DEMO_EST_SEC", &["5"]));
        assert_eq!(duration_text(42.), Message::new("#L_DEMO_EST_SEC", &["45"]));
        assert_eq!(duration_text(61.), Message::new("#L_DEMO_EST_MIN", &["2"]));
        assert_eq!(
            duration_text(1302.),
            Message::new("#L_DEMO_EST_MIN", &["22"])
        );
        assert_eq!(
            duration_text(3900.),
            Message::new("#L_DEMO_EST_HOUR", &["1", "05"])
        );
    }

    #[test]
    fn a_deleted_video_is_known_by_name_and_folder_then_by_path() {
        let out = r"C:\Users\me\Videos\HLSR";
        let result = json!({"file": format!(r"{out}\myrun2.mp4"), "fileName": "myrun2.mp4"});
        let which = |name, output_dir, file| RenderedWhich {
            name,
            output_dir,
            file,
        };
        assert!(is_rendered_file(
            Some(&result),
            &which("myrun2.mp4", out, None)
        ));
        let lower = format!("{}\\", out.to_lowercase());
        assert!(
            is_rendered_file(Some(&result), &which("myrun2.mp4", &lower, None)),
            "регистр и слеш в конце"
        );
        assert!(!is_rendered_file(
            Some(&result),
            &which("myrun2.mp4", r"D:\Clips", None)
        ));
        assert!(!is_rendered_file(
            Some(&result),
            &which("myrun1.mp4", out, None)
        ));
        let exact = format!(r"{}\MYRUN2.mp4", out.to_lowercase());
        assert!(is_rendered_file(
            Some(&result),
            &which("", "", Some(&exact))
        ));
        let linux = json!({"file": "/home/me/Videos/HLSR/a.mp4", "fileName": "a.mp4"});
        assert!(is_rendered_file(
            Some(&linux),
            &which("a.mp4", "/home/me/Videos/HLSR", None)
        ));
        let preview = json!({"file": null, "fileName": null});
        assert!(!is_rendered_file(
            Some(&preview),
            &which("myrun2.mp4", "x", None)
        ));
        assert!(!is_rendered_file(None, &which("myrun2.mp4", "", None)));
    }

    #[test]
    fn marker_texts_by_type_with_chapter_names() {
        let t = |key: &str, args: &[&str]| {
            if args.is_empty() {
                key.to_owned()
            } else {
                format!("{key}({})", args.join(","))
            }
        };
        let text = |marker: Value| marker_text(&marker, &t);
        assert_eq!(text(json!({"type": "map", "label": "c1a0"})), "c1a0");
        assert_eq!(
            text(json!({"type": "saveload", "label": 2})),
            "#L_DEMO_MARK_SAVELOAD(2)"
        );
        assert_eq!(
            text(json!({"type": "chapter", "label": "9a"})),
            "#L_DEMO_MARK_CHAPTER(#L_DEMO_CHAPTER_9A)"
        );
        assert_eq!(
            text(json!({"type": "chapter", "label": "x"})),
            "#L_DEMO_MARK_CHAPTER(x)"
        );
        assert_eq!(
            text(json!({"type": "finish", "label": "GHRD"})),
            "#L_DEMO_MARK_FINISH(GHRD)"
        );
        assert!(has_demo_extension("RUN.DEM") && has_demo_extension("a.ghplaylist"));
        assert!(!has_demo_extension("a.mp4"));
        assert_eq!(resolution_label(1440.), "1440p");
        assert_eq!(resolution_label(960.), "960p");
        assert_eq!(encoder_name("h264_qsv"), "Intel Quick Sync");
        assert_eq!(game_name("ghosting"), "Ghosting");
    }
}
