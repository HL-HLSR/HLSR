//! Окно «Экспорт клипа» (`DemoExportModal.vue` 0.4.9) без окна: настройки (пресет, разрешение,
//! FPS, кодировщик, звук, проверка данных, вид в кадре, скорость), план и оценка времени от
//! главного процесса, правка интервала, быстрые исправления, тексты задания и итога.
//!
//! Каналы (`demo-options`, `demo-settings-get`, `demo-plan`, `demo-estimate`, `demo-output-pick`,
//! `demo-reveal`) зовёт вид и отдаёт ответы сюда (`*_done`), как `await ipcRenderer.invoke(…)`
//! компонента. Пропсы 0.4.9 (`duration`, `range`, `ffmpeg`, `job`, `result`, `jobError`, `busy`)
//! вид берёт у страницы и передаёт аргументами. Задание запускает и ведёт страница.

use super::boot::{Translate, format_number, format_size};
use super::demo_panel::{Answer, DemoPanel, Job};
use super::demo_tool::{
    CLIP_SPEEDS, ClipRange, Combo, RESOLUTIONS, clock, combos_for, duration_text, encoder_name,
    error_text, fps_for, heights_of, parse_clock, pick_combo, resolution_label, warning_keys,
};
use super::jsv;
use hlsr_core::js;
use serde_json::{Map, Value, json};

/// Пресеты окна: id, подпись, значок (`preview` — пресет предпросмотра, окно его не шлёт).
pub const PRESETS: [(&str, &str, &str); 2] = [
    ("quality", "#L_DEMO_EX_QUALITY", "fas fa-gem"),
    ("discord", "#L_DEMO_EX_DISCORD", "fab fa-discord"),
];

/// Режимы кодировщика: id и подпись.
pub const ENCODER_MODES: [(&str, &str); 3] = [
    ("auto", "#L_DEMO_EX_ENC_AUTO"),
    ("gpu", "#L_DEMO_EX_ENC_GPU"),
    ("cpu", "#L_DEMO_EX_ENC_CPU"),
];

/// Ключи «Дополнительных настроек» (`DemoViewSettings`).
const VIEW_KEYS: [&str; 4] = ["keyhud", "hud", "ownConfig", "ownHud"];

/// Настройки экспорта (`settings` компонента).
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub preset: String,
    pub height: f64,
    pub fps: f64,
    pub audio: bool,
    pub keyhud: bool,
    pub integrity: bool,
    pub encoder: String,
    pub speed: f64,
    pub hud: bool,
    pub own_config: bool,
    pub own_hud: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            preset: "quality".into(),
            height: 1080.,
            fps: 60.,
            audio: true,
            keyhud: false,
            integrity: false,
            encoder: "auto".into(),
            speed: 1.,
            hud: true,
            own_config: true,
            own_hud: true,
        }
    }
}

impl Settings {
    /// `for (key of Object.keys(settings)) if (saved[key] !== undefined) settings[key] = saved[key]`.
    /// shortcut: значение не своего типа пропускается (JS взял бы и его) — главный процесс шлёт
    /// только свои настройки.
    fn apply(&mut self, saved: &Map<String, Value>) {
        let text = |key: &str, slot: &mut String| {
            if let Some(Value::String(value)) = saved.get(key) {
                slot.clone_from(value);
            }
        };
        let number = |key: &str, slot: &mut f64| {
            if let Some(value) = saved.get(key).and_then(js::value::number) {
                *slot = value;
            }
        };
        let flag = |key: &str, slot: &mut bool| {
            if let Some(Value::Bool(value)) = saved.get(key) {
                *slot = *value;
            }
        };
        text("preset", &mut self.preset);
        number("height", &mut self.height);
        number("fps", &mut self.fps);
        flag("audio", &mut self.audio);
        flag("keyhud", &mut self.keyhud);
        flag("integrity", &mut self.integrity);
        text("encoder", &mut self.encoder);
        number("speed", &mut self.speed);
        flag("hud", &mut self.hud);
        flag("ownConfig", &mut self.own_config);
        flag("ownHud", &mut self.own_hud);
    }

    /// Переключатель «Дополнительных настроек» по ключу 0.4.9.
    fn view_flag(&mut self, key: &str) -> Option<&mut bool> {
        Some(match key {
            "keyhud" => &mut self.keyhud,
            "hud" => &mut self.hud,
            "ownConfig" => &mut self.own_config,
            "ownHud" => &mut self.own_hud,
            _ => return None,
        })
    }
}

/// Кнопка разрешения.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    pub height: f64,
    pub width: Option<f64>,
    pub label: String,
    pub allowed: bool,
}

/// Что делает быстрое исправление.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FixAction {
    /// Выбрать разрешение (`pickHeight`).
    Height(f64),
    /// Новый интервал (`$emit("range", …)`).
    Range(ClipRange),
}

/// Быстрое исправление: `again` — сначала вернуться к форме (`$emit("again")`, ошибка задания).
#[derive(Debug, Clone, PartialEq)]
pub struct Fix {
    pub id: &'static str,
    pub label: String,
    pub again: bool,
    pub action: FixAction,
}

/// Поле интервала.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    From,
    To,
}

/// Состояние окна (`data()`).
#[derive(Debug, Clone, PartialEq)]
pub struct DemoExport {
    pub loading: bool,
    /// Ответ `demo-options`.
    pub options: Option<Value>,
    pub options_error: Option<String>,
    pub settings: Settings,
    pub plan: Option<Value>,
    pub plan_error: Option<Value>,
    /// Оценка времени рендера по [`PRESETS`] (элемент `items` ответа `demo-estimate` или `null`).
    pub estimates: Option<Vec<Value>>,
    pub output_dir: String,
    /// Ключ текста ошибки полей «Начало» и «Конец».
    pub invalid: [Option<&'static str>; 2],
    /// Ревизия полей интервала: после правки их текст берётся заново.
    pub range_rev: u64,
    /// Ошибка действия в окне («Показать в папке», выбор папки).
    pub action_error: Option<String>,
    plan_ticket: u64,
    estimate_ticket: u64,
}

impl Default for DemoExport {
    fn default() -> Self {
        DemoExport {
            loading: true,
            options: None,
            options_error: None,
            settings: Settings::default(),
            plan: None,
            plan_error: None,
            estimates: None,
            output_dir: String::new(),
            invalid: [None, None],
            range_rev: 0,
            action_error: None,
            plan_ticket: 0,
            estimate_ticket: 0,
        }
    }
}

fn ok(answer: &Answer) -> Option<&Value> {
    answer
        .as_ref()
        .ok()
        .filter(|answer| jsv::is_str(jsv::prop(answer, "status"), "ok"))
}

fn text_of(value: Option<&Value>) -> String {
    value.map(js::to_string).unwrap_or_default()
}

fn code(code: &str) -> Value {
    json!({ "code": code })
}

fn state_is(status: Option<&Value>, state: &str) -> bool {
    jsv::is_str(jsv::field(status, "state"), state)
}

impl DemoExport {
    fn option(&self, name: &str) -> Option<&Value> {
        jsv::field(self.options.as_ref(), name)
    }

    fn caps(&self, name: &str) -> Option<&Value> {
        jsv::field(self.option("capabilities"), name)
    }

    // ---------- вычисляемое ----------

    /// Рендер этой демки не готов (`options.render.ok` ложно).
    pub fn render_blocked(&self) -> bool {
        self.option("render")
            .is_some_and(|render| js::truthy(render) && !jsv::truthy(jsv::prop(render, "ok")))
    }

    fn render_code(&self) -> Value {
        code(&text_of(jsv::field(self.option("render"), "code")))
    }

    /// Рендер HL1 ждёт bxt-rs: вместо строки «не готово» — карточка скачивания.
    pub fn bxtrs_needed(&self, bxtrs: Option<&Value>) -> bool {
        bxtrs.is_some()
            && self.render_blocked()
            && jsv::is_str(jsv::field(self.option("render"), "code"), "BXTRS_MISSING")
    }

    fn combos(&self) -> Vec<Combo> {
        combos_for(self.options.as_ref(), &self.settings.encoder)
    }

    /// Разрешения адаптера; недоступные кодировщику — неактивны.
    pub fn resolution_choices(&self) -> Vec<Resolution> {
        let heights = heights_of(&self.combos());
        let allowed = |height: f64| heights.contains(&height);
        match self.option("resolutions").filter(|list| js::truthy(list)) {
            Some(offered) => offered
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .map(|res| {
                    let height = jsv::num(jsv::prop(res, "height")).unwrap_or(f64::NAN);
                    let label = jsv::prop(res, "label")
                        .filter(|label| js::truthy(label))
                        .map_or_else(|| resolution_label(height), js::to_string);
                    Resolution {
                        height,
                        width: jsv::num(jsv::prop(res, "width")),
                        label,
                        allowed: allowed(height),
                    }
                })
                .collect(),
            None => RESOLUTIONS
                .iter()
                .map(|(height, label)| Resolution {
                    height: *height,
                    width: None,
                    label: (*label).to_owned(),
                    allowed: allowed(*height),
                })
                .collect(),
        }
    }

    /// Частоты адаптера и доступна ли каждая при выбранной высоте.
    pub fn fps_choices(&self) -> Vec<(f64, bool)> {
        let caps: Vec<f64> = self
            .caps("fps")
            .filter(|fps| js::truthy(fps))
            .and_then(Value::as_array)
            .map_or_else(
                || vec![30., 60., 120.],
                |list| list.iter().filter_map(js::value::number).collect(),
            );
        let allowed = fps_for(&self.combos(), self.settings.height);
        caps.into_iter()
            .map(|fps| (fps, allowed.contains(&fps)))
            .collect()
    }

    /// Режимы кодировщика: id, подпись, доступен ли (без FFmpeg — все).
    pub fn encoder_modes(&self) -> Vec<(&'static str, &'static str, bool)> {
        let modes = self.option("modes").filter(|modes| js::truthy(modes));
        ENCODER_MODES
            .iter()
            .map(|(id, label)| {
                let available = modes.is_none_or(|modes| jsv::truthy(jsv::prop(modes, id)));
                (*id, *label, available)
            })
            .collect()
    }

    /// «Будет: NVIDIA NVENC»; режим без кодировщика — пусто.
    pub fn encoder_label(&self) -> String {
        jsv::field(self.option("modes"), &self.settings.encoder)
            .filter(|entry| js::truthy(entry))
            .map(|entry| encoder_name(&text_of(jsv::prop(entry, "encoder"))))
            .unwrap_or_default()
    }

    pub fn audio_available(&self) -> bool {
        jsv::truthy(self.caps("audio"))
    }

    fn keyhud_available(&self) -> bool {
        jsv::truthy(jsv::field(self.option("keyhud"), "available"))
    }

    /// Надпись проверки данных — только у адаптеров, которые её умеют (Ghosting).
    pub fn integrity_available(&self) -> bool {
        jsv::truthy(self.caps("integrity"))
    }

    /// Что умеет адаптер из «Дополнительных настроек» (`options.capabilities`).
    pub fn view_available(&self, key: &str) -> bool {
        jsv::truthy(self.caps(key))
    }

    /// Выбор «Дополнительных настроек» (`view`): KeyHUD — только если адаптер его умеет.
    pub fn view(&self, key: &str) -> bool {
        let s = &self.settings;
        match key {
            "keyhud" => s.keyhud && self.keyhud_available(),
            "hud" => s.hud,
            "ownConfig" => s.own_config,
            "ownHud" => s.own_hud,
            _ => false,
        }
    }

    /// Переключатель «Дополнительных настроек» (`view = { …view, [key]: value }`): все четыре
    /// берутся из `view` (недоступный KeyHUD выключается), затем меняется один.
    pub fn set_view(&mut self, key: &str, on: bool) {
        for name in VIEW_KEYS {
            let value = self.view(name);
            if let Some(flag) = self.settings.view_flag(name) {
                *flag = value;
            }
        }
        if let Some(flag) = self.settings.view_flag(key) {
            *flag = on;
        }
    }

    /// Скорости клипа в пределах адаптера.
    pub fn speeds(&self) -> Vec<f64> {
        let limits = self.option("speed").filter(|speed| js::truthy(speed));
        let (min, max) = match limits {
            Some(limits) => (
                jsv::num(jsv::prop(limits, "min")).unwrap_or(f64::NAN),
                jsv::num(jsv::prop(limits, "max")).unwrap_or(f64::NAN),
            ),
            None => (0.25, 4.),
        };
        CLIP_SPEEDS
            .into_iter()
            .filter(|speed| *speed >= min && *speed <= max)
            .collect()
    }

    /// Текст полей интервала: «Начало» и «Конец».
    pub fn range_text(range: Option<ClipRange>, duration: f64) -> [String; 2] {
        match range {
            Some(range) => [clock(range.from), clock(range.to)],
            None => [clock(0.), clock(duration)],
        }
    }

    /// Настройки для `demo-plan`, `demo-estimate` и `demo-export-start` (`payload`).
    pub fn payload(&self, range: Option<ClipRange>) -> Value {
        let s = &self.settings;
        json!({
            "preset": s.preset,
            "height": jsv::number_value(s.height),
            "fps": jsv::number_value(s.fps),
            "audio": s.audio && self.audio_available(),
            "keyhud": s.keyhud && self.keyhud_available(),
            "integrity": s.integrity && self.integrity_available(),
            "encoder": s.encoder,
            "speed": jsv::number_value(s.speed),
            "hud": s.hud,
            "ownConfig": s.own_config,
            "ownHud": s.own_hud,
            "range": range.map_or(Value::Null, ClipRange::to_value),
        })
    }

    /// Оценка пресета (`estimates[preset]`), если она есть.
    pub fn estimate_of(&self, preset: &str) -> Option<&Value> {
        let index = PRESETS.iter().position(|(id, _, _)| *id == preset)?;
        self.estimates
            .as_ref()?
            .get(index)
            .filter(|estimate| js::truthy(estimate))
    }

    /// «~45 с» оценки (`durationLabel(estimate.seconds)`).
    pub fn estimate_label(estimate: &Value, t: Translate) -> String {
        duration_text(js::value::to_number(jsv::prop(estimate, "seconds"))).text(t)
    }

    /// Строка итога: длина клипа, кадр, размер.
    pub fn estimate_text(&self, t: Translate) -> Option<String> {
        let plan = self.plan.as_ref()?;
        let get = |name: &str| jsv::prop(plan, name);
        let size = format_size(js::value::to_number(get("estimatedBytes")), t);
        let length = clock(js::value::to_number(get("outputDuration")));
        let size_key = if jsv::is_str(get("mode"), "bitrate") {
            "#L_DEMO_EX_SIZE_UPTO"
        } else {
            "#L_DEMO_EX_SIZE_ABOUT"
        };
        Some(
            [
                t("#L_DEMO_EX_LENGTH", &[&length]),
                format!(
                    "{}×{}, {} FPS",
                    text_of(get("width")),
                    text_of(get("height")),
                    text_of(get("fps"))
                ),
                t(size_key, &[&size]),
            ]
            .join(" · "),
        )
    }

    /// Битрейт плана Discord: «видео 2,1 Мбит/с, звук 96 кбит/с».
    pub fn bitrate_text(&self, t: Translate) -> Option<String> {
        let plan = self
            .plan
            .as_ref()
            .filter(|plan| jsv::is_str(jsv::prop(plan, "mode"), "bitrate"))?;
        let video = js::value::to_number(jsv::prop(plan, "videoBps")) / 1e6;
        let video = format_number(video, 1, t);
        let audio = jsv::prop(plan, "audioBps").filter(|audio| js::truthy(audio));
        Some(match audio {
            None => t("#L_DEMO_EX_BITRATE_MUTE", &[&video]),
            Some(audio) => {
                let kbps = jsv::round(js::value::to_number(Some(audio)) / 1000.);
                t("#L_DEMO_EX_BITRATE", &[&video, &jsv::number_text(kbps)])
            }
        })
    }

    /// Предупреждения плана (ключи текстов).
    pub fn plan_warnings(&self) -> Vec<&'static str> {
        warnings_of(self.plan.as_ref())
    }

    /// Предупреждения итога задания.
    pub fn result_warnings(result: &Value) -> Vec<&'static str> {
        warnings_of(Some(result))
    }

    /// Строка итога задания: размер, кадр, длина, кодировщик.
    pub fn result_meta(result: &Value, t: Translate) -> String {
        let video = jsv::prop(result, "video");
        let get = |name: &str| jsv::field(video, name);
        let mut parts = vec![format_size(
            js::value::to_number(jsv::prop(result, "size")),
            t,
        )];
        if jsv::truthy(get("width")) {
            parts.push(format!(
                "{}×{}, {} FPS",
                text_of(get("width")),
                text_of(get("height")),
                text_of(get("fps"))
            ));
        }
        if jsv::truthy(get("duration")) {
            parts.push(clock(js::value::to_number(get("duration"))));
        }
        let encoder = encoder_name(&text_of(jsv::prop(result, "encoder")));
        parts.push(t("#L_DEMO_EX_ENC_USED", &[&encoder]));
        parts.join(" · ")
    }

    /// Быстрые исправления: «Сделать 720p», «Укоротить до N с» (Discord) или наибольшее
    /// разрешение, которое влезает (игра отрисовала меньше).
    pub fn fixes(
        &self,
        job_error: Option<&Value>,
        range: Option<ClipRange>,
        duration: f64,
        t: Translate,
    ) -> Vec<Fix> {
        fn code_of(error: Option<&Value>) -> Option<&str> {
            jsv::str_of(jsv::field(error, "code"))
        }
        let choices = self.resolution_choices();
        if let Some(limit) = job_error.filter(|_| code_of(job_error) == Some("RESOLUTION_LIMITED"))
        {
            let height = js::value::to_number(jsv::prop(limit, "height"));
            let width = js::value::to_number(jsv::prop(limit, "width"));
            let fit = choices.into_iter().rfind(|res| {
                res.allowed && res.height <= height && res.width.is_none_or(|w| w <= width)
            });
            return fit
                .map(|fit| Fix {
                    id: "fit",
                    label: t("#L_DEMO_EX_FIX_RES", &[&fit.label]),
                    again: true,
                    action: FixAction::Height(fit.height),
                })
                .into_iter()
                .collect();
        }
        let from_job = code_of(job_error) == Some("DISCORD_TOO_LONG");
        let error = if from_job {
            job_error
        } else {
            self.plan_error.as_ref()
        };
        let Some(error) = error.filter(|error| code_of(Some(error)) == Some("DISCORD_TOO_LONG"))
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let suggested = jsv::prop(error, "suggestedHeight").filter(|height| js::truthy(height));
        if let Some(height) = suggested.and_then(js::value::number)
            && choices
                .iter()
                .any(|res| res.height == height && res.allowed)
        {
            out.push(Fix {
                id: "res",
                label: t("#L_DEMO_EX_FIX_RES", &[&resolution_label(height)]),
                again: from_job,
                action: FixAction::Height(height),
            });
        }
        let seconds = jsv::or_zero(jsv::prop(error, "recommendedMaxSeconds"));
        let seconds = if seconds.is_nan() {
            0.
        } else {
            seconds.floor()
        };
        if seconds >= 1. {
            let from = range.map_or(0., |range| range.from);
            let to = duration.min(from + seconds * self.settings.speed);
            out.push(Fix {
                id: "cut",
                label: t("#L_DEMO_EX_FIX_CUT", &[&jsv::number_text(seconds)]),
                again: from_job,
                action: FixAction::Range(ClipRange { from, to }),
            });
        }
        out
    }

    /// Фаза задания (или повтор Discord) и сегмент — строка над полосой без процента.
    pub fn job_text(job: &Job, t: Translate) -> String {
        // jobLine страницы — та же строка с процентом в конце: процент окно пишет справа.
        let line = DemoPanel::job_line(job, t);
        line.rsplit_once(" · ")
            .map_or_else(|| line.clone(), |(head, _)| head.to_owned())
    }

    /// Почему «Сохранить клип» недоступна (подсказка кнопки).
    pub fn start_blocker(
        &self,
        busy: bool,
        job: bool,
        ffmpeg: Option<&Value>,
        t: Translate,
    ) -> Option<String> {
        if busy && !job {
            return Some(t("#L_DEMO_ERR_BUSY", &[]));
        }
        if self.render_blocked() {
            return Some(error_text(&self.render_code(), t));
        }
        if ffmpeg.is_some() && !state_is(ffmpeg, "ready") {
            return Some(t("#L_DEMO_ERR_FFMPEG_MISSING", &[]));
        }
        self.plan_error.as_ref().map(|error| error_text(error, t))
    }

    /// Запись идёт игрой: Steam на это время покажет «в игре».
    pub fn steam_note(&self, outcome: bool) -> bool {
        !outcome && self.options.is_some() && !self.render_blocked()
    }

    pub fn can_start(&self, busy: bool, job: bool, ffmpeg: Option<&Value>) -> bool {
        self.options.is_some()
            && self.plan.is_some()
            && self.plan_error.is_none()
            && !self.render_blocked()
            && !job
            && !busy
            && !self.loading
            && state_is(ffmpeg, "ready")
    }

    // ---------- ответы каналов ----------

    /// `loadOptions()`: перед `demo-options`.
    pub fn options_begin(&mut self) {
        self.loading = true;
        self.options_error = None;
    }

    /// Ответ `demo-options`: запомненные настройки поверх стартовых. `true` — сразу план и
    /// оценка.
    pub fn options_done(&mut self, answer: &Answer, initial: Option<&Value>, t: Translate) -> bool {
        self.loading = false;
        let Some(answer) = ok(answer) else {
            let error = match answer {
                Ok(error) if jsv::truthy(jsv::prop(error, "code")) => error.clone(),
                _ => code("DEMO_FAILED"),
            };
            self.options_error = Some(error_text(&error, t));
            return false;
        };
        self.options = Some(answer.clone());
        let mut saved = jsv::prop(answer, "defaults")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        if let Some(initial) = initial.and_then(Value::as_object) {
            saved.extend(initial.clone());
        }
        self.settings.apply(&saved);
        if !PRESETS.iter().any(|(id, _, _)| *id == self.settings.preset) {
            self.settings.preset = "quality".into();
        }
        let encoder = self.settings.encoder.clone();
        if self
            .encoder_modes()
            .iter()
            .any(|(id, _, available)| *id == encoder && !available)
        {
            self.settings.encoder = "auto".into();
        }
        self.fit_combo();
        true
    }

    /// Ответ `demo-settings-get`: папка вывода.
    pub fn settings_done(&mut self, answer: &Answer) {
        if let Some(answer) = ok(answer) {
            self.output_dir = text_of(jsv::prop(answer, "outputDir"));
        }
    }

    /// Номер запроса `demo-plan` (без `demo-options` плана нет).
    pub fn plan_begin(&mut self) -> Option<u64> {
        self.options.as_ref()?;
        self.plan_ticket += 1;
        Some(self.plan_ticket)
    }

    /// Ответ `demo-plan`; ответ на устаревший запрос не нужен.
    pub fn plan_done(&mut self, ticket: u64, answer: &Answer) {
        if ticket != self.plan_ticket {
            return;
        }
        match answer {
            Ok(value) if jsv::is_str(jsv::prop(value, "status"), "ok") => {
                self.plan = jsv::prop(value, "plan").cloned();
                self.plan_error = None;
            }
            Ok(value) => {
                self.plan = None;
                // Без FFmpeg плана нет: про это уже говорит карточка FFmpeg.
                let ffmpeg = js::truthy(value)
                    && jsv::str_of(jsv::prop(value, "code"))
                        .is_some_and(|code| code.starts_with("FFMPEG_"));
                self.plan_error = if ffmpeg {
                    None
                } else if js::truthy(value) {
                    Some(value.clone())
                } else {
                    Some(code("DEMO_FAILED"))
                };
            }
            Err(()) => self.plan_error = Some(code("DEMO_FAILED")),
        }
    }

    /// Номер запроса `demo-estimate` и варианты по пресетам.
    pub fn estimate_begin(&mut self, range: Option<ClipRange>) -> Option<(u64, Value)> {
        self.options.as_ref()?;
        self.estimate_ticket += 1;
        let payload = self.payload(range);
        let variants: Vec<Value> = PRESETS
            .iter()
            .map(|(id, _, _)| {
                let mut variant = payload.clone();
                variant["preset"] = json!(id);
                variant
            })
            .collect();
        Some((self.estimate_ticket, Value::Array(variants)))
    }

    /// Ответ `demo-estimate` (исключение — без оценки).
    pub fn estimate_done(&mut self, ticket: u64, answer: &Answer) {
        if ticket != self.estimate_ticket {
            return;
        }
        self.estimates = ok(answer).map(|answer| {
            let items = jsv::prop(answer, "items");
            (0..PRESETS.len())
                .map(|index| {
                    items
                        .and_then(|items| items.get(index))
                        .cloned()
                        .unwrap_or(Value::Null)
                })
                .collect()
        });
    }

    /// Ответ `demo-output-pick` (исключение — `DIALOG_UNAVAILABLE`).
    pub fn pick_folder_done(&mut self, answer: &Answer, t: Translate) {
        let answer = match answer {
            Ok(value) => value.clone(),
            Err(()) => json!({"status": "error", "code": "DIALOG_UNAVAILABLE"}),
        };
        let status = jsv::str_of(jsv::prop(&answer, "status"));
        if status == Some("ok") {
            self.output_dir = text_of(jsv::prop(&answer, "outputDir"));
        } else if status == Some("error") {
            self.action_error = Some(error_text(&answer, t));
        }
    }

    /// Ответ `demo-reveal`.
    pub fn reveal_done(&mut self, answer: &Answer, t: Translate) {
        if ok(answer).is_none() {
            let error = match answer {
                Ok(value) if js::truthy(value) => value.clone(),
                _ => json!({}),
            };
            self.action_error = Some(error_text(&error, t));
        }
    }

    // ---------- выбор ----------

    fn fit_combo(&mut self) {
        let combos = self.combos();
        if let Some((height, fps)) = pick_combo(&combos, self.settings.height, self.settings.fps) {
            self.settings.height = height;
            self.settings.fps = fps;
        }
    }

    pub fn pick_height(&mut self, height: f64) {
        self.settings.height = height;
        self.fit_combo();
    }

    pub fn pick_encoder(&mut self, id: &str) {
        self.settings.encoder = id.to_owned();
        self.fit_combo();
    }

    /// Интервал поменяли снаружи (плеер, «Взять весь ран», быстрые исправления).
    pub fn range_changed(&mut self) {
        self.invalid = [None, None];
        self.range_rev += 1;
    }

    /// Ключ текста под строкой интервала (первая ошибка полей).
    pub fn range_problem(&self) -> Option<&'static str> {
        self.invalid[0].or(self.invalid[1])
    }

    /// `editRange(which, text)`: свой текст на каждую ошибку (формат, вне демки, конец раньше
    /// начала). `Some(range)` — новый интервал для страницы (`None` в нём — весь ран).
    pub fn edit_range(
        &mut self,
        edge: Edge,
        text: &str,
        range: Option<ClipRange>,
        duration: f64,
    ) -> Option<Option<ClipRange>> {
        let value = parse_clock(text);
        let mut next = range.unwrap_or(ClipRange {
            from: 0.,
            to: duration,
        });
        let slot = edge as usize;
        let problem = match value {
            None => Some("#L_DEMO_EX_RANGE_BAD"),
            Some(value) => {
                match edge {
                    Edge::From => next.from = value,
                    Edge::To => next.to = value,
                }
                if value > duration + 0.05 {
                    Some("#L_DEMO_EX_RANGE_OUT")
                } else if next.to <= next.from {
                    Some("#L_DEMO_ERR_RANGE")
                } else {
                    None
                }
            }
        };
        self.invalid[slot] = problem;
        if problem.is_some() {
            return None;
        }
        next.to = next.to.min(duration);
        let next = (next.from > 0. || next.to < duration).then_some(next);
        if next == range {
            self.range_changed();
            return None;
        }
        Some(next)
    }
}

fn warnings_of(value: Option<&Value>) -> Vec<&'static str> {
    let list = jsv::field(value, "warnings")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let codes: Vec<&str> = list.iter().filter_map(Value::as_str).collect();
    warning_keys(&codes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(key: &str, args: &[&str]) -> String {
        if args.is_empty() {
            key.to_owned()
        } else {
            format!("{key}({})", args.join(","))
        }
    }

    /// `demo-options` Ghosting: 720p–1440p, 30/60, GPU без 1440p@60.
    fn options() -> Value {
        json!({
            "status": "ok", "id": "gh", "duration": 144.46,
            "render": {"ok": true, "code": null},
            "capabilities": {"maxHeight": 1440, "fps": [30, 60], "audio": true, "integrity": true,
                "keyhud": true, "hud": false, "ownConfig": true, "ownHud": true},
            "keyhud": {"available": true, "code": null},
            "resolutions": [
                {"height": 720, "width": 1280, "label": "720p"},
                {"height": 1080, "width": 1920, "label": "1080p"},
                {"height": 1440, "width": 2560, "label": "1440p"}
            ],
            "speed": {"min": 0.25, "max": 4},
            "modes": {
                "auto": {"encoder": "h264_nvenc", "combos": [
                    {"height": 720, "width": 1280, "fps": 30}, {"height": 720, "width": 1280, "fps": 60},
                    {"height": 1080, "width": 1920, "fps": 30}, {"height": 1080, "width": 1920, "fps": 60},
                    {"height": 1440, "width": 2560, "fps": 30}]},
                "gpu": {"encoder": "h264_nvenc", "combos": [{"height": 720, "width": 1280, "fps": 60}]},
                "cpu": null
            },
            "defaults": {"preset": "preview", "height": 1440, "fps": 60, "encoder": "cpu",
                "speed": 1.5, "keyhud": true, "integrity": true}
        })
    }

    fn loaded() -> DemoExport {
        let mut ex = DemoExport::default();
        ex.options_begin();
        assert!(ex.options_done(&Ok(options()), None, &t));
        ex
    }

    #[test]
    fn saved_settings_fit_what_the_encoder_can_do() {
        let ex = loaded();
        let s = &ex.settings;
        // «preview» окно не шлёт, «Процессор» недоступен, 1440p@60 нет — 1440p@30.
        assert_eq!(s.preset, "quality");
        assert_eq!(s.encoder, "auto");
        assert_eq!((s.height, s.fps, s.speed), (1440., 30., 1.5));
        assert!(!ex.loading);
        assert_eq!(
            ex.encoder_modes()
                .iter()
                .map(|(id, _, on)| (*id, *on))
                .collect::<Vec<_>>(),
            [("auto", true), ("gpu", true), ("cpu", false)]
        );
        assert_eq!(ex.encoder_label(), "NVIDIA NVENC");
        assert_eq!(ex.fps_choices(), [(30., true), (60., false)]);
        // «Экспорт» готового видео: стартовая скорость поверх запомненной.
        let mut again = DemoExport::default();
        again.options_done(&Ok(options()), Some(&json!({"speed": 2})), &t);
        assert_eq!(again.settings.speed, 2.);

        let mut gpu = loaded();
        gpu.pick_encoder("gpu");
        assert_eq!((gpu.settings.height, gpu.settings.fps), (720., 60.));
        let allowed: Vec<bool> = gpu
            .resolution_choices()
            .iter()
            .map(|res| res.allowed)
            .collect();
        assert_eq!(allowed, [true, false, false]);
    }

    #[test]
    fn the_payload_carries_only_what_the_game_can_do() {
        let mut ex = loaded();
        ex.settings.hud = false;
        let range = Some(ClipRange {
            from: 10.,
            to: 70.5,
        });
        assert_eq!(
            ex.payload(range),
            json!({"preset": "quality", "height": 1440, "fps": 30, "audio": true, "keyhud": true,
                "integrity": true, "encoder": "auto", "speed": 1.5, "hud": false, "ownConfig": true,
                "ownHud": true, "range": {"from": 10, "to": 70.5}})
        );
        let mut options = options();
        options["capabilities"]["audio"] = json!(false);
        options["capabilities"]["integrity"] = json!(false);
        options["keyhud"] = json!({"available": false, "code": "KEYHUD_NOT_AVAILABLE"});
        let mut hl1 = DemoExport::default();
        hl1.options_done(&Ok(options), None, &t);
        let payload = hl1.payload(None);
        assert_eq!(
            (&payload["audio"], &payload["keyhud"], &payload["integrity"]),
            (&json!(false), &json!(false), &json!(false))
        );
        assert_eq!(payload["range"], Value::Null);
        assert!(!hl1.view("keyhud"));
        // Переключатель вида: недоступный KeyHUD выключается, остальное — как было.
        hl1.settings.keyhud = true;
        hl1.set_view("hud", false);
        let s = &hl1.settings;
        assert!(!s.keyhud && !s.hud && s.own_config && s.own_hud);
        // Оценки по пресетам: тот же payload с пресетом.
        let (ticket, variants) = ex.estimate_begin(None).unwrap();
        assert_eq!(variants[0]["preset"], "quality");
        assert_eq!(variants[1]["preset"], "discord");
        ex.estimate_done(
            ticket,
            &Ok(json!({"status": "ok", "items": [{"seconds": 40}]})),
        );
        assert_eq!(
            DemoExport::estimate_label(ex.estimate_of("quality").unwrap(), &t),
            "#L_DEMO_EST_SEC(40)"
        );
        assert!(ex.estimate_of("discord").is_none());
        let (old, _) = ex.estimate_begin(None).unwrap();
        let (_, _) = ex.estimate_begin(None).unwrap();
        ex.estimate_done(old, &Err(()));
        assert!(ex.estimates.is_some(), "устаревший ответ не нужен");
    }

    #[test]
    fn the_plan_texts_and_its_errors() {
        let mut ex = loaded();
        assert_eq!(DemoExport::default().plan_begin(), None, "без options");
        let ticket = ex.plan_begin().unwrap();
        ex.plan_done(
            ticket,
            &Ok(json!({"status": "ok", "plan": {"mode": "bitrate", "width": 1280, "height": 720,
                "fps": 60, "outputDuration": 65.34, "estimatedBytes": 9961472, "videoBps": 2400000,
                "audioBps": 96000, "warnings": ["DISCORD_LOW_QUALITY", "NOPE", "DISCORD_LOW_QUALITY"]}})),
        );
        assert_eq!(
            ex.estimate_text(&t).unwrap(),
            "#L_DEMO_EX_LENGTH(1:05.3) · 1280×720, 60 FPS · #L_DEMO_EX_SIZE_UPTO(9#L_DECIMAL5\u{a0}#L_MB)"
        );
        assert_eq!(
            ex.bitrate_text(&t).unwrap(),
            "#L_DEMO_EX_BITRATE(2#L_DECIMAL4,96)"
        );
        assert_eq!(ex.plan_warnings(), ["#L_DEMO_WARN_DISCORD_LOW_QUALITY"]);
        assert!(ex.can_start(false, false, Some(&json!({"state": "ready"}))));
        assert!(!ex.can_start(true, false, Some(&json!({"state": "ready"}))));
        assert_eq!(
            ex.start_blocker(true, false, None, &t).as_deref(),
            Some("#L_DEMO_ERR_BUSY")
        );

        // FFmpeg пропал — про это говорит карточка, не план.
        let ticket = ex.plan_begin().unwrap();
        ex.plan_done(
            ticket,
            &Ok(json!({"status": "error", "code": "FFMPEG_MISSING"})),
        );
        assert_eq!((ex.plan.as_ref(), ex.plan_error.as_ref()), (None, None));
        assert_eq!(
            ex.start_blocker(false, false, Some(&json!({"state": "missing"})), &t)
                .as_deref(),
            Some("#L_DEMO_ERR_FFMPEG_MISSING")
        );
        // Discord не влезает: исправления разрешением и длиной.
        let ticket = ex.plan_begin().unwrap();
        ex.plan_done(
            ticket,
            &Ok(json!({"status": "error", "code": "DISCORD_TOO_LONG",
                "recommendedMaxSeconds": 41.7, "suggestedHeight": 720})),
        );
        let fixes = ex.fixes(
            None,
            Some(ClipRange {
                from: 100.,
                to: 140.,
            }),
            144.46,
            &t,
        );
        assert_eq!(
            fixes,
            [
                Fix {
                    id: "res",
                    label: "#L_DEMO_EX_FIX_RES(720p)".into(),
                    again: false,
                    action: FixAction::Height(720.)
                },
                Fix {
                    id: "cut",
                    label: "#L_DEMO_EX_FIX_CUT(41)".into(),
                    again: false,
                    action: FixAction::Range(ClipRange {
                        from: 100.,
                        to: 144.46
                    })
                }
            ]
        );
        assert!(!ex.can_start(false, false, Some(&json!({"state": "ready"}))));
        // Ошибка задания важнее плана: сначала вернуться к форме.
        let job = json!({"code": "DISCORD_TOO_LONG", "recommendedMaxSeconds": 20});
        let fixes = ex.fixes(Some(&job), None, 144.46, &t);
        assert_eq!(fixes.len(), 1);
        assert!(fixes[0].again);
        assert_eq!(
            fixes[0].action,
            FixAction::Range(ClipRange { from: 0., to: 30. })
        );
        // Игра отрисовала меньше: наибольшее разрешение, которое влезает.
        let limited = json!({"code": "RESOLUTION_LIMITED", "width": "1920", "height": 1080});
        let fixes = ex.fixes(Some(&limited), None, 144.46, &t);
        assert_eq!(
            fixes.iter().map(|fix| fix.action).collect::<Vec<_>>(),
            [FixAction::Height(1080.)]
        );
        // Исключение канала: ошибка без плана.
        let ticket = ex.plan_begin().unwrap();
        ex.plan_done(ticket, &Err(()));
        assert_eq!(ex.plan_error, Some(json!({"code": "DEMO_FAILED"})));
    }

    #[test]
    fn range_fields_say_what_is_wrong() {
        let mut ex = loaded();
        let d = 144.46;
        assert_eq!(ex.edit_range(Edge::From, "1:x", None, d), None);
        assert_eq!(ex.range_problem(), Some("#L_DEMO_EX_RANGE_BAD"));
        assert_eq!(ex.edit_range(Edge::To, "3:00", None, d), None);
        assert_eq!(ex.invalid[1], Some("#L_DEMO_EX_RANGE_OUT"));
        let r = Some(ClipRange { from: 10., to: 20. });
        assert_eq!(ex.edit_range(Edge::To, "0:05", r, d), None);
        assert_eq!(ex.invalid[1], Some("#L_DEMO_ERR_RANGE"));
        assert_eq!(
            ex.edit_range(Edge::From, "0:12.5", r, d),
            Some(Some(ClipRange {
                from: 12.5,
                to: 20.
            }))
        );
        // Весь ран — `null`; 0,05 с за концом — ещё конец демки.
        assert_eq!(
            ex.edit_range(
                Edge::To,
                "2:24.5",
                r.map(|r| ClipRange { from: 0., ..r }),
                d
            ),
            Some(None)
        );
        // То же значение: поля сбрасываются, страница не зовётся.
        let rev = ex.range_rev;
        assert_eq!(ex.edit_range(Edge::From, "0:10.0", r, d), None);
        assert_eq!((ex.invalid, ex.range_rev), ([None, None], rev + 1));
        assert_eq!(
            DemoExport::range_text(None, d),
            ["0:00.0".to_owned(), "2:24.5".to_owned()]
        );
    }

    #[test]
    fn job_and_result_lines() {
        let job = Job {
            job_id: Some(json!("e1")),
            kind: super::super::demo_panel::JobKind::Export,
            item_id: "gh".into(),
            phase: "encode".into(),
            percent: 61.4,
            segment: json!({"index": 2, "count": 3}),
            message: Some("retry-1".into()),
        };
        assert_eq!(
            DemoExport::job_text(&job, &t),
            "#L_DEMO_JOB_RETRY(2) · #L_DEMO_JOB_SEGMENT(2,3)"
        );
        let result = json!({"id": "r1", "file": "C:\\v\\gh.mp4", "fileName": "gh.mp4",
            "size": 9437184, "encoder": "libx264", "attempts": 2,
            "video": {"width": 1280, "height": 720, "fps": 60, "duration": 41.5},
            "warnings": ["CPU_SLOW"]});
        assert_eq!(
            DemoExport::result_meta(&result, &t),
            "9#L_DECIMAL0\u{a0}#L_MB · 1280×720, 60 FPS · 0:41.5 · #L_DEMO_EX_ENC_USED(x264)"
        );
        assert_eq!(
            DemoExport::result_warnings(&result),
            ["#L_DEMO_WARN_CPU_SLOW"]
        );
        let mut ex = loaded();
        ex.pick_folder_done(&Ok(json!({"status": "ok", "outputDir": "D:\\Clips"})), &t);
        assert_eq!(ex.output_dir, "D:\\Clips");
        ex.pick_folder_done(&Ok(json!({"status": "cancelled"})), &t);
        assert_eq!(ex.action_error, None);
        ex.pick_folder_done(&Err(()), &t);
        assert_eq!(ex.action_error.as_deref(), Some("#L_DEMO_ERR_DIALOG"));
        ex.reveal_done(&Ok(Value::Null), &t);
        assert_eq!(ex.action_error.as_deref(), Some("#L_DEMO_ERR_FAILED"));
        // Ошибка options: текст кода или общий.
        let mut bad = DemoExport::default();
        assert!(!bad.options_done(
            &Ok(json!({"status": "error", "code": "DEMO_NOT_FOUND"})),
            None,
            &t
        ));
        assert_eq!(bad.options_error.as_deref(), Some("#L_DEMO_ERR_NOT_FOUND"));
        assert!(!bad.options_done(&Err(()), None, &t));
        assert_eq!(bad.options_error.as_deref(), Some("#L_DEMO_ERR_FAILED"));
    }
}
