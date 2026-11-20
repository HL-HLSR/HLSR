//! Сцены каталога `tools/snap/screens` (формат — `tools/reference/README.md`, «Эталонные экраны»):
//! слои с базами сливаются так же, как `loadScene` в `tools/reference/lib/electron-screens.js` —
//! поля сцены заменяют поля базы, `invoke`, `sendSync`, `store`, `prefs`, `localStorage`, `https`
//! сливаются по ключу первого уровня; от базы не наследуются `id`, `expect`, `hashes`.
//!
//! Каталог 0.4.9 — `tools/snap/screens/049/<область>/<сцена>.json` ([`screens_049_dir`]): id сцены —
//! путь от каталога без `.json` (`games/frame`), база по умолчанию — `_base.json` каталога. Варианты
//! сцен 0.4.9 — ещё и зум раскладки 1600×900 (`zooms`) и колонка значков (`rails`).

use hlsr_core::language::Language;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// Каталог сцен 0.4.7 этого репозитория (сцены Slint — образец для переноса сцен 0.4.9).
pub fn screens_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/snap/screens")
}

/// Каталог сцен 0.4.9 (`hlsr-snap screens`, эталон — Electron 0.4.9).
pub fn screens_049_dir() -> PathBuf {
    screens_dir().join("049")
}

const BASE: &str = "_base";
const MERGED: [&str; 6] = [
    "invoke",
    "sendSync",
    "store",
    "prefs",
    "localStorage",
    "https",
];
/// Сгенерированные файлы каталога — не сцены.
const GENERATED: [&str; 2] = ["index.json", "animations.json"];

/// Все сцены каталога (обход папок, кроме `assets` и тех, что на «_»): id — путь от каталога без
/// `.json` через `/`; файлы на «_» и сгенерированные — не сцены.
pub fn scene_ids(dir: &Path) -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    collect_ids(dir, "", &mut ids)?;
    ids.sort();
    Ok(ids)
}

fn collect_ids(dir: &Path, prefix: &str, ids: &mut Vec<String>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name.starts_with('_') {
            continue;
        }
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            if name != "assets" {
                collect_ids(&entry.path(), &format!("{prefix}{name}/"), ids)?;
            }
        } else if name.ends_with(".json") && !GENERATED.contains(&name.as_str()) {
            ids.push(format!("{prefix}{}", name.trim_end_matches(".json")));
        }
    }
    Ok(())
}

fn read_layer(dir: &Path, name: &str) -> Result<Map<String, Value>, String> {
    let path = dir.join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    match serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))? {
        Value::Object(map) => Ok(map),
        _ => Err(format!("{}: не объект", path.display())),
    }
}

/// `{...base, ...scene}` и слияние `MERGED` по первому уровню (порядок ключей — как у JS).
fn merge(base: Map<String, Value>, scene: Map<String, Value>) -> Map<String, Value> {
    let mut merged = base.clone();
    for (key, value) in &scene {
        merged.insert(key.clone(), value.clone());
    }
    for key in MERGED {
        let mut joined = match base.get(key) {
            Some(Value::Object(map)) => map.clone(),
            _ => Map::new(),
        };
        if let Some(Value::Object(own)) = scene.get(key) {
            for (k, v) in own {
                joined.insert(k.clone(), v.clone());
            }
        }
        merged.insert(key.to_owned(), Value::Object(joined));
    }
    merged
}

fn load_layer(dir: &Path, name: &str, seen: &[String]) -> Result<Map<String, Value>, String> {
    if seen.iter().any(|s| s == name) {
        return Err(format!("база по кругу: {} → {name}", seen.join(" → ")));
    }
    let mut layer = read_layer(dir, name)?;
    let parent = match layer.remove("base") {
        None if name == BASE => None,
        None => Some(BASE.to_owned()),
        Some(Value::String(parent)) => Some(parent),
        Some(Value::Null) => None,
        Some(other) => return Err(format!("{name}.json: base {other}")),
    };
    let Some(parent) = parent else {
        return Ok(layer);
    };
    let mut chain = seen.to_vec();
    chain.push(name.to_owned());
    let mut inherited = load_layer(dir, &parent, &chain)?;
    for own in ["id", "expect", "hashes"] {
        inherited.remove(own);
    }
    Ok(merge(inherited, layer))
}

/// Сцена со слитыми базами — значение JSON (как `loadScene` 0.4.7-скрипта).
pub fn load_scene_value(dir: &Path, id: &str) -> Result<Map<String, Value>, String> {
    let scene = load_layer(dir, id, &[])?;
    match scene.get("id") {
        Some(Value::String(own)) if own == id => Ok(scene),
        other => Err(format!(
            "{id}.json: id «{}» не совпадает с именем файла",
            other.map_or("нет".to_owned(), Value::to_string)
        )),
    }
}

/// Размер окна сцены, CSS px.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
pub struct WindowSize {
    pub width: f32,
    pub height: f32,
}

/// Ожидания сцены, снятые с Electron (`--write-expect`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Expect {
    /// Журнал IPC по языку: `[{kind, channel, args}]`.
    pub ipc_log: Map<String, Value>,
    pub animations: Vec<Value>,
}

/// Сцена каталога.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scene {
    pub id: String,
    #[serde(default)]
    pub screen: String,
    #[serde(default)]
    pub state: String,
    pub route: String,
    pub platform: String,
    pub window: WindowSize,
    #[serde(default = "one")]
    pub ui_scale: f32,
    pub themes: Vec<String>,
    pub languages: Vec<String>,
    pub scales: Vec<f64>,
    /// Зум раскладки 1600×900 (сцены 0.4.9): окно `window × зум`, `zoomFactor` эталона и `rem`
    /// GPUI — `16 × зум`. Пусто — без зума в вариантах (окно `window × uiScale`).
    #[serde(default)]
    pub zooms: Vec<f64>,
    /// Колонка значков (сцены 0.4.9, где она видна): `closed`, `open` → `prefs.railExpanded`.
    /// Пусто — как в `prefs` сцены.
    #[serde(default)]
    pub rails: Vec<String>,
    pub now: String,
    pub timezone: String,
    #[serde(default)]
    pub seed: u32,
    #[serde(default)]
    pub reduced_motion: bool,
    #[serde(default)]
    pub parallax: bool,
    /// Свои правила страницы эталона (прячут всё, кроме проверяемого блока); у служебных окон
    /// отсюда берётся цвет сплошного фона.
    #[serde(default)]
    pub css: String,
    #[serde(default)]
    pub prefs: Map<String, Value>,
    #[serde(default)]
    pub local_storage: Map<String, Value>,
    #[serde(default)]
    pub store: Map<String, Value>,
    #[serde(default)]
    pub send_sync: Map<String, Value>,
    #[serde(default)]
    pub invoke: Map<String, Value>,
    #[serde(default)]
    pub https: Map<String, Value>,
    #[serde(default)]
    pub steps: Vec<Value>,
    #[serde(default)]
    pub settle_ms: u64,
    /// `timerCeilingMs`: таймеры страницы эталона с задержкой от стольких мс не срабатывают
    /// никогда (`electron-screens-preload.js`: опросы, страховки — иначе зависят от длительности
    /// съёмки); нет поля — 10 000, `null` — срабатывают все. Окно сцены повторяет это для срока
    /// жизни тостов (2 800 мс).
    #[serde(default = "timer_ceiling")]
    pub timer_ceiling_ms: Option<u64>,
    /// Снимок в середине движения (сцена смены темы): всё, что запустил последний шаг, — в этот
    /// момент от начала, мс (`Motion.frozen-time`; эталон ставит анимации на паузу там же), а не
    /// в конце.
    #[serde(default)]
    pub freeze_at: Option<u64>,
    /// Окно сцены — id корневого вида GPUI: пусто — окно лаунчера целиком; иначе вид одного
    /// экрана на ровном фоне (в эталоне всё остальное спрятано полем `css`). В сценах 0.4.7 —
    /// `probe` (`settings`, `header`, `home`).
    #[serde(default, alias = "probe")]
    pub view: Option<String>,
    /// Сверка журнала с хвоста: запись `{kind, channel, args}`, с последнего вхождения которой
    /// (в обоих журналах) идёт сравнение — первый вызов самого окна в пробнике; до неё в журнале
    /// Electron — вызовы оболочки, которой в пробнике нет. Пусто — журналы целиком.
    #[serde(default)]
    pub journal_from: Option<Value>,
    /// Области снимка без сравнения — известные отступления вида от 0.4.9: `[x, y, ширина,
    /// высота]` в CSS px окна (× зум варианта).
    #[serde(default)]
    pub masks: Vec<[f32; 4]>,
    #[serde(default)]
    pub expect: Expect,
    #[serde(default)]
    pub hashes: Map<String, Value>,
}

fn one() -> f32 {
    1.0
}

/// `TIMER_CEILING_MS` скрипта эталона (`electron-screens.js`).
fn timer_ceiling() -> Option<u64> {
    Some(10_000)
}

impl Scene {
    /// Сцена каталога `dir` со слитыми базами.
    pub fn load(dir: &Path, id: &str) -> Result<Scene, String> {
        let value = load_scene_value(dir, id)?;
        serde_json::from_value(Value::Object(value)).map_err(|e| format!("{id}.json: {e}"))
    }

    /// Варианты: темы × языки × масштабы × зумы × колонка (фильтр — пустой список или значения;
    /// порядок — как `variantsOf` эталона).
    pub fn variants(&self, filter: &VariantFilter) -> Vec<Variant> {
        let pick = |all: &[String], only: &[String]| -> Vec<String> {
            all.iter()
                .filter(|v| only.is_empty() || only.contains(v))
                .cloned()
                .collect()
        };
        let numbers = |all: &[f64], only: &[String]| -> Vec<f64> {
            all.iter()
                .copied()
                .filter(|v| only.is_empty() || only.contains(&scale_text(*v)))
                .collect()
        };
        // Нет поля — одно значение «нет» (вариант без зума или без колонки).
        let zooms: Vec<Option<f64>> = if self.zooms.is_empty() {
            vec![None]
        } else {
            numbers(&self.zooms, &filter.zooms)
                .into_iter()
                .map(Some)
                .collect()
        };
        let rails: Vec<Option<String>> = if self.rails.is_empty() {
            vec![None]
        } else {
            pick(&self.rails, &filter.rails)
                .into_iter()
                .map(Some)
                .collect()
        };
        let mut out = Vec::new();
        for theme in pick(&self.themes, &filter.themes) {
            for lang in pick(&self.languages, &filter.languages) {
                for scale in numbers(&self.scales, &filter.scales) {
                    for zoom in &zooms {
                        for rail in &rails {
                            out.push(Variant {
                                theme: theme.clone(),
                                lang: lang.clone(),
                                scale,
                                zoom: *zoom,
                                rail: rail.clone(),
                            });
                        }
                    }
                }
            }
        }
        out
    }

    /// Зум окна варианта: зум варианта или `uiScale` сцены.
    pub fn zoom_of(&self, variant: &Variant) -> f64 {
        variant.zoom.unwrap_or(f64::from(self.ui_scale))
    }

    /// Размер окна варианта в логических px (`Math.round(window × зум)` эталона).
    pub fn window_of(&self, variant: &Variant) -> (u32, u32) {
        let zoom = self.zoom_of(variant);
        let side = |v: f32| (f64::from(v) * zoom).round() as u32;
        (side(self.window.width), side(self.window.height))
    }

    /// `prefs` варианта, как `localStorage()` эталона: prefs сцены, тема варианта и колонка.
    pub fn prefs_of(&self, variant: &Variant) -> Map<String, Value> {
        let mut prefs = self.prefs.clone();
        prefs.insert("launcherTheme".into(), Value::String(variant.theme.clone()));
        if let Some(rail) = &variant.rail {
            prefs.insert("railExpanded".into(), Value::Bool(rail == "open"));
        }
        prefs
    }

    /// Файлы картинок `https` сцены: адрес → путь (относительно каталога сцен).
    pub fn https_files(&self, dir: &Path) -> std::collections::HashMap<String, PathBuf> {
        self.https
            .iter()
            .filter_map(|(url, file)| Some((url.clone(), dir.join(file.as_str()?))))
            .collect()
    }

    /// Срабатывает ли в сцене таймер страницы с задержкой `delay` (`timerCeilingMs`).
    pub fn timer_fires(&self, delay: std::time::Duration) -> bool {
        self.timer_ceiling_ms
            .is_none_or(|ceiling| delay < std::time::Duration::from_millis(ceiling))
    }
}

/// Отбор вариантов: пустой список — все.
#[derive(Debug, Clone, Default)]
pub struct VariantFilter {
    pub themes: Vec<String>,
    pub languages: Vec<String>,
    pub scales: Vec<String>,
    pub zooms: Vec<String>,
    pub rails: Vec<String>,
}

/// Вариант сцены.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Variant {
    pub theme: String,
    pub lang: String,
    pub scale: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rail: Option<String>,
}

impl Variant {
    /// `<тема>-<язык>@<масштаб>[-z<зум>][-<колонка>]` (`variantKey` скрипта эталона).
    pub fn key(&self) -> String {
        let mut key = format!("{}-{}@{}", self.theme, self.lang, scale_text(self.scale));
        if let Some(zoom) = self.zoom {
            key += &format!("-z{}", scale_text(zoom));
        }
        if let Some(rail) = &self.rail {
            key += &format!("-{rail}");
        }
        key
    }

    /// Язык варианта: `en` = 0, `ru` = 1 (`settings.config.language`).
    pub fn language(&self) -> Result<Language, String> {
        match self.lang.as_str() {
            "en" => Ok(Language::En),
            "ru" => Ok(Language::Ru),
            other => Err(format!("язык {other}")),
        }
    }
}

/// Масштаб как `String(scale)` в JS: `1`, `1.25`.
pub fn scale_text(scale: f64) -> String {
    format!("{scale}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scene(extra: Value) -> Scene {
        let mut base = json!({
            "id": "games/frame", "route": "/launcher", "platform": "win32",
            "window": {"width": 1600, "height": 900},
            "themes": ["hlsr", "paper"], "languages": ["ru"], "scales": [1],
            "now": "2026-11-17T12:00:00.000Z", "timezone": "UTC",
            "prefs": {"railExpanded": false, "animations": true}
        });
        let extra = extra.as_object().cloned().unwrap_or_default();
        base.as_object_mut().unwrap().extend(extra);
        serde_json::from_value(base).unwrap()
    }

    #[test]
    fn variants_cover_zooms_and_rails_like_the_reference() {
        let s = scene(json!({"zooms": [1, 0.75, 0.72, 1.25], "rails": ["closed", "open"]}));
        let all = s.variants(&VariantFilter::default());
        assert_eq!(all.len(), 2 * 4 * 2);
        assert_eq!(all[0].key(), "hlsr-ru@1-z1-closed");
        assert_eq!(all[3].key(), "hlsr-ru@1-z0.75-open");
        let only = s.variants(&VariantFilter {
            themes: vec!["paper".into()],
            zooms: vec!["0.72".into()],
            rails: vec!["open".into()],
            ..VariantFilter::default()
        });
        assert_eq!(only.len(), 1);
        let v = &only[0];
        assert_eq!(v.key(), "paper-ru@1-z0.72-open");
        assert_eq!(s.window_of(v), (1152, 648));
        let prefs = s.prefs_of(v);
        assert_eq!(prefs["railExpanded"], json!(true));
        assert_eq!(prefs["launcherTheme"], json!("paper"));
        // Вариант переживает JSON (окружение `hlsr` в режиме сцены).
        let back: Variant = serde_json::from_str(&serde_json::to_string(v).unwrap()).unwrap();
        assert_eq!(&back, v);
    }

    #[test]
    fn scenes_without_zooms_keep_the_047_keys() {
        let s = scene(json!({"uiScale": 1.25, "scales": [1, 1.25]}));
        let all = s.variants(&VariantFilter::default());
        assert_eq!(
            all.iter().map(Variant::key).collect::<Vec<_>>(),
            ["hlsr-ru@1", "hlsr-ru@1.25", "paper-ru@1", "paper-ru@1.25"]
        );
        assert_eq!(s.window_of(&all[0]), (2000, 1125));
        assert_eq!(s.prefs_of(&all[0])["railExpanded"], json!(false));
    }

    #[test]
    fn the_049_catalog_is_walked_by_folders() {
        let dir = screens_049_dir();
        let ids = scene_ids(&dir).unwrap();
        assert!(ids.contains(&"games/frame".to_owned()), "{ids:?}");
        let frame = Scene::load(&dir, "games/frame").unwrap();
        assert_eq!(frame.id, "games/frame");
        assert!(frame.view.is_none());
        assert_eq!(frame.rails, ["closed", "open"]);
        assert_eq!(frame.zooms, [1.0, 0.75, 0.72, 1.25]);
    }
}
