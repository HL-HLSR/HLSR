//! `hlsr-snap screens`: сцены каталога `tools/snap/screens/049` в окне `hlsr` (dev-hooks) против
//! Electron 0.4.9 (docs/gpui/00-plan.md, §2).
//!
//!   hlsr-snap screens [--only <id или маска с *>[,…]] [--themes …] [--languages …]
//!                     [--zoom 1,0.75,0.72,1.25] [--rail closed,open] [--run <имя>] [--jobs 2]
//!                     [--hlsr <exe>] [--reference <каталог>] [--no-reference]
//!                     [--fail-on image,text,journal,steps]
//!
//! Запуск — только на невидимом столе: `python scripts/slint/headless-win.py target/debug/hlsr-snap.exe
//! screens …`; `hlsr` — сборка `cargo build -p hlsr-app --features dev-hooks`.
//!
//! 1. Эталон (кэш `target/snap/ref-049`): сцена снимается заново (`tools/reference/snap-screens.js`
//!    в Electron 0.4.9), если её слитый JSON не совпал с `<сцена>/scene.json` эталона или нет PNG
//!    какого-то варианта; `--no-reference` — не снимать.
//! 2. GPUI: процесс `hlsr` на вариант (`HLSR_SNAP_*`, временные `HLSR_DATA_DIR`/`HLSR_LOGS_DIR`):
//!    `<вариант>.png`, `.ipc-log.json`, `.run.json` (шаги, проблемы фикстуры) в
//!    `target/snap/<run>/<сцена>/`; зависший процесс снимается по своему PID через минуту.
//! 3. Сверка: вне масок текста эталона — порог 32/255 по каналу и не больше 1 % пикселей (`image`),
//!    в масках — то же после размытия σ = 1 px (`text`; DirectWrite ≠ Chromium); области `masks`
//!    сцены не сравниваются. Журнал IPC — против журнала эталона, летучие поля нормализованы
//!    (`journal`). Шаги — все прошли (`steps`). Отчёт с парами PNG — `target/snap/<run>/index.html`.
//!    Код 1 — есть варианты, не прошедшие группы `--fail-on` (по умолчанию `image,journal,steps`).

use crate::masks::{self, Region};
use hlsr_ui::fixtures::journal;
use hlsr_ui::fixtures::scene::load_scene_value;
use hlsr_ui::fixtures::{Scene, Variant, VariantFilter, scene_ids, screens_049_dir};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const THRESHOLD: u8 = 32;
const MAX_RATIO: f64 = 0.01;
const SIGMA: f32 = 1.0;
const GROUPS: [&str; 4] = ["image", "text", "journal", "steps"];
/// Процесс `hlsr` одного варианта дольше этого — завис.
const VARIANT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
struct Options {
    only: Vec<String>,
    filter: VariantFilter,
    run: String,
    jobs: usize,
    hlsr: PathBuf,
    reference: PathBuf,
    shoot_reference: bool,
    fail_on: Vec<String>,
}

pub fn repo_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .nth(2)
        .unwrap_or(manifest)
        .to_path_buf()
}

fn list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

fn parse(args: &[String]) -> Result<Options, String> {
    let exe = if cfg!(windows) { "hlsr.exe" } else { "hlsr" };
    let mut o = Options {
        only: Vec::new(),
        filter: VariantFilter::default(),
        run: "latest".into(),
        jobs: 2,
        hlsr: repo_root().join("target/debug").join(exe),
        reference: repo_root().join("target/snap/ref-049"),
        shoot_reference: true,
        fail_on: ["image", "journal", "steps"].map(String::from).to_vec(),
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{arg}: нет значения"));
        match arg.as_str() {
            "--only" => o.only = list(&value()?),
            "--themes" => o.filter.themes = list(&value()?),
            "--languages" => o.filter.languages = list(&value()?),
            "--zoom" | "--zooms" => o.filter.zooms = list(&value()?),
            "--rail" | "--rails" => o.filter.rails = list(&value()?),
            "--run" => o.run = value()?,
            "--jobs" => {
                o.jobs = value()?
                    .parse()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or("--jobs: число > 0")?
            }
            "--hlsr" => o.hlsr = PathBuf::from(value()?),
            "--reference" => o.reference = PathBuf::from(value()?),
            "--no-reference" => o.shoot_reference = false,
            "--fail-on" => o.fail_on = list(&value()?),
            other => return Err(format!("неизвестный аргумент {other}")),
        }
    }
    if let Some(bad) = o.fail_on.iter().find(|g| !GROUPS.contains(&g.as_str())) {
        return Err(format!("--fail-on {bad}: {}", GROUPS.join(" | ")));
    }
    Ok(o)
}

/// `*` — любые символы (в том числе `/`); без `*` — id целиком.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !text.starts_with(first) || text.len() < first.len() + last.len() || !text.ends_with(last) {
        return false;
    }
    let mut rest = &text[first.len()..text.len() - last.len()];
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    true
}

pub fn main(args: &[String]) -> Result<bool, String> {
    let options = parse(args)?;
    if cfg!(windows) && std::env::var_os("HLSR_HEADLESS_ROOT").is_none() {
        return Err(
            "запускать через scripts/slint/headless-win.py: окна не должны появляться на экране"
                .into(),
        );
    }
    if !has_scene_mode(&options.hlsr)? {
        return Err(format!(
            "{}: сборка без dev-hooks, режима сцены нет (запустился бы настоящий лаунчер): cargo build -p hlsr-app --features dev-hooks",
            options.hlsr.display()
        ));
    }
    let dir = screens_049_dir();
    let ids: Vec<String> = scene_ids(&dir)?
        .into_iter()
        .filter(|id| options.only.is_empty() || options.only.iter().any(|p| glob_match(p, id)))
        .collect();
    if ids.is_empty() {
        return Err("нет сцен по --only".into());
    }
    let scenes = ids
        .iter()
        .map(|id| Scene::load(&dir, id))
        .collect::<Result<Vec<_>, _>>()?;
    if options.shoot_reference {
        shoot_reference(&options, &dir, &scenes)?;
    }
    let out = repo_root().join("target/snap").join(&options.run);
    for scene in &scenes {
        let _ = std::fs::remove_dir_all(out.join(&scene.id));
    }
    let jobs: Vec<(&Scene, Variant)> = scenes
        .iter()
        .flat_map(|scene| {
            scene
                .variants(&options.filter)
                .into_iter()
                .map(move |v| (scene, v))
        })
        .collect();
    if jobs.is_empty() {
        return Err("нет вариантов по фильтрам".into());
    }
    let started = Instant::now();
    shoot_gpui(&options, &dir, &out, &jobs);
    println!(
        "GPUI: {} вариантов за {:.1} с",
        jobs.len(),
        started.elapsed().as_secs_f32()
    );
    let rows: Vec<Row> = jobs
        .iter()
        .map(|(scene, variant)| compare(&options, &out, scene, variant))
        .collect();
    report(&options, &out, &rows)
}

/// В `hlsr` есть режим сцены: строка `HLSR_SNAP_SCENE` есть только в `hlsr-app/src/snap.rs` под
/// dev-hooks. Файл читается кусками по 1 МиБ.
fn has_scene_mode(exe: &Path) -> Result<bool, String> {
    const MARK: &[u8] = b"HLSR_SNAP_SCENE";
    let mut file = std::fs::File::open(exe).map_err(|e| format!("{}: {e}", exe.display()))?;
    let mut buf = vec![0; 1 << 20];
    let mut kept = 0;
    loop {
        let n = std::io::Read::read(&mut file, &mut buf[kept..])
            .map_err(|e| format!("{}: {e}", exe.display()))?;
        if n == 0 {
            return Ok(false);
        }
        let end = kept + n;
        if buf[..end].windows(MARK.len()).any(|w| w == MARK) {
            return Ok(true);
        }
        // Хвост куска — в начало: метка может лечь на границу.
        kept = (MARK.len() - 1).min(end);
        buf.copy_within(end - kept..end, 0);
    }
}

/// Эталон устарел: слитая сцена другая или нет PNG какого-то варианта.
fn reference_stale(options: &Options, dir: &Path, scene: &Scene) -> bool {
    let saved: Option<Value> =
        std::fs::read_to_string(options.reference.join(&scene.id).join("scene.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok());
    let merged = load_scene_value(dir, &scene.id).ok().map(Value::Object);
    let same = saved.as_ref().and_then(|s| s.get("scene")) == merged.as_ref();
    !same
        || scene.variants(&options.filter).iter().any(|v| {
            !options
                .reference
                .join(&scene.id)
                .join(format!("{}.png", v.key()))
                .exists()
        })
}

fn shoot_reference(options: &Options, dir: &Path, scenes: &[Scene]) -> Result<(), String> {
    let stale: Vec<&str> = scenes
        .iter()
        .filter(|s| reference_stale(options, dir, s))
        .map(|s| s.id.as_str())
        .collect();
    if stale.is_empty() {
        println!("эталон: все сцены в кэше {}", options.reference.display());
        return Ok(());
    }
    println!("эталон Electron 0.4.9: {}", stale.join(", "));
    let mut command = Command::new("node");
    command
        .current_dir(repo_root())
        .arg("tools/reference/snap-screens.js")
        .args(["--only", &stale.join(",")])
        .arg("--out")
        .arg(&options.reference)
        .env("HLSR_SNAP_SCREENS", dir);
    let filter = &options.filter;
    for (key, values) in [
        ("--themes", &filter.themes),
        ("--languages", &filter.languages),
        ("--zooms", &filter.zooms),
        ("--rails", &filter.rails),
    ] {
        if !values.is_empty() {
            command.args([key, &values.join(",")]);
        }
    }
    let status = command
        .status()
        .map_err(|e| format!("node tools/reference/snap-screens.js: {e}"))?;
    if !status.success() {
        // Сцены без эталона в отчёте — «нет эталона», сверка остального идёт.
        eprintln!("эталон снят не весь: snap-screens.js, код {status}");
    }
    Ok(())
}

fn variant_path(out: &Path, id: &str, variant: &Variant, ext: &str) -> PathBuf {
    out.join(id).join(format!("{}.{ext}", variant.key()))
}

/// Процессы `hlsr` вариантов, по `jobs` сразу.
fn shoot_gpui(options: &Options, dir: &Path, out: &Path, jobs: &[(&Scene, Variant)]) {
    let next = AtomicUsize::new(0);
    let log = Mutex::new(());
    let work = out.join(".work");
    std::thread::scope(|scope| {
        for _ in 0..options.jobs.min(jobs.len()) {
            scope.spawn(|| {
                loop {
                    let n = next.fetch_add(1, Ordering::SeqCst);
                    let Some((scene, variant)) = jobs.get(n) else {
                        break;
                    };
                    let result =
                        shoot_variant(options, dir, out, &work.join(n.to_string()), scene, variant);
                    let _guard = log.lock();
                    match result {
                        Ok(seconds) => println!("{} {}: {seconds:.1} с", scene.id, variant.key()),
                        Err(e) => eprintln!("{} {}: {e}", scene.id, variant.key()),
                    }
                }
            });
        }
    });
    let _ = std::fs::remove_dir_all(&work);
}

fn shoot_variant(
    options: &Options,
    dir: &Path,
    out: &Path,
    work: &Path,
    scene: &Scene,
    variant: &Variant,
) -> Result<f32, String> {
    let base = out.join(&scene.id).join(variant.key());
    std::fs::create_dir_all(out.join(&scene.id)).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(work);
    let (data, logs) = (work.join("data"), work.join("logs"));
    for d in [&data, &logs] {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    // stderr — в файл: из трубы, читаемой после выхода, hlsr мог бы застрять на записи.
    let stderr_path = work.join("stderr.txt");
    let stderr_file = std::fs::File::create(&stderr_path)
        .map_err(|e| format!("{}: {e}", stderr_path.display()))?;
    let started = Instant::now();
    let mut child = Command::new(&options.hlsr)
        .env("HLSR_SNAP_SCENE", &scene.id)
        .env(
            "HLSR_SNAP_VARIANT",
            serde_json::to_string(variant).map_err(|e| e.to_string())?,
        )
        .env("HLSR_SNAP_OUT", &base)
        .env("HLSR_SNAP_SCREENS", dir)
        .env("HLSR_DATA_DIR", &data)
        .env("HLSR_LOGS_DIR", &logs)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr_file)
        .spawn()
        .map_err(|e| format!("{}: {e}", options.hlsr.display()))?;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if started.elapsed() > VARIANT_TIMEOUT {
            // Свой процесс — по своему PID.
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "hlsr не закончил за {} с",
                VARIANT_TIMEOUT.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let stderr = std::fs::read_to_string(&stderr_path).unwrap_or_default();
    if !variant_path(out, &scene.id, variant, "run.json").exists() {
        return Err(format!(
            "hlsr без итога (код {status}; сборка с --features dev-hooks?) {}",
            stderr.lines().last().unwrap_or("")
        ));
    }
    Ok(started.elapsed().as_secs_f32())
}

/// Строка отчёта.
#[derive(Debug, Default)]
struct Row {
    scene: String,
    variant: String,
    image: String,
    text: String,
    journal: String,
    steps: String,
    failed: Vec<&'static str>,
    missing_reference: bool,
    details: Vec<String>,
    /// Пары PNG (относительно каталога прогона): эталон, GPUI, карта отличий.
    pngs: [Option<String>; 3],
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

fn load_png(path: &Path) -> Option<image::RgbaImage> {
    image::open(path).ok().map(|img| img.to_rgba8())
}

fn percent(ratio: f64) -> String {
    format!("{:.2} %", ratio * 100.0)
}

/// Области `masks` сцены в px снимка.
fn ignored(scene: &Scene, variant: &Variant) -> Vec<Region> {
    let zoom = scene.zoom_of(variant) * variant.scale;
    scene
        .masks
        .iter()
        .map(|[x, y, w, h]| {
            let at = |v: f32| (f64::from(v) * zoom).floor().max(0.) as u32;
            let len = |start: f32, v: f32| {
                ((f64::from(start + v) * zoom).ceil() as u32).saturating_sub(at(start))
            };
            Region {
                x: at(*x),
                y: at(*y),
                width: len(*x, *w),
                height: len(*y, *h),
            }
        })
        .collect()
}

fn compare(options: &Options, out: &Path, scene: &Scene, variant: &Variant) -> Row {
    let key = variant.key();
    let rel = |name: String| format!("{}/{name}", scene.id);
    let mut row = Row {
        scene: scene.id.clone(),
        variant: key.clone(),
        image: "—".into(),
        text: "—".into(),
        journal: "—".into(),
        steps: "—".into(),
        ..Row::default()
    };
    let run: Value =
        read_json(&variant_path(out, &scene.id, variant, "run.json")).unwrap_or(Value::Null);
    if run.is_null() {
        row.image = "нет снимка GPUI".into();
        row.failed.extend(["image", "journal", "steps"]);
        return row;
    }
    // Шаги и проблемы фикстуры.
    let steps = run["steps"].as_array().cloned().unwrap_or_default();
    let bad: Vec<&Value> = steps.iter().filter(|s| s["status"] != "ок").collect();
    row.steps = format!("{}/{}", steps.len() - bad.len(), steps.len());
    for s in &bad {
        row.details.push(format!(
            "шаг {}: {}",
            s["step"],
            s["status"].as_str().unwrap_or("?")
        ));
    }
    if let Some(error) = run["error"].as_str() {
        row.details.push(format!("ошибка: {error}"));
    }
    if !bad.is_empty() || !run["error"].is_null() {
        row.failed.push("steps");
    }
    for p in run["problems"].as_array().into_iter().flatten() {
        row.details
            .push(format!("фикстура GPUI: {}", p.as_str().unwrap_or("?")));
    }
    // Снимок.
    let reference = options.reference.join(&scene.id);
    let actual = load_png(&variant_path(out, &scene.id, variant, "png"));
    row.pngs[1] = actual.as_ref().map(|_| rel(format!("{key}.png")));
    let expected = load_png(&reference.join(format!("{key}.png")));
    if let Some(expected) = &expected {
        let copy = variant_path(out, &scene.id, variant, "ref.png");
        if std::fs::copy(reference.join(format!("{key}.png")), &copy).is_ok() {
            row.pngs[0] = Some(rel(format!("{key}.ref.png")));
        }
        if let Ok(problems) =
            std::fs::read_to_string(reference.join(format!("{key}.problems.json")))
            && let Ok(list) = serde_json::from_str::<Vec<String>>(&problems)
        {
            row.details
                .extend(list.into_iter().map(|p| format!("фикстура Electron: {p}")));
        }
        match &actual {
            None => {
                row.image = "нет снимка GPUI".into();
                row.failed.push("image");
            }
            Some(actual) => {
                let mask = load_png(&reference.join(format!("{key}.text.png")));
                let d = masks::masked_diff_except(
                    expected,
                    actual,
                    mask.as_ref(),
                    &ignored(scene, variant),
                    THRESHOLD,
                    SIGMA,
                );
                if d.map
                    .save(variant_path(out, &scene.id, variant, "diff.png"))
                    .is_ok()
                {
                    row.pngs[2] = Some(rel(format!("{key}.diff.png")));
                }
                if !d.same_size {
                    row.image = format!(
                        "размер {}x{}, эталон {}x{}",
                        actual.width(),
                        actual.height(),
                        expected.width(),
                        expected.height()
                    );
                    row.failed.push("image");
                } else {
                    row.image = format!("{} (макс. {})", percent(d.outside), d.max_outside);
                    row.text = match &mask {
                        // Такую маску сверка отбросила: текст не проверен.
                        Some(m) if m.dimensions() != expected.dimensions() => {
                            row.failed.push("text");
                            format!("маска {}x{} не того размера", m.width(), m.height())
                        }
                        Some(_) => format!(
                            "{} (до размытия {})",
                            percent(d.inside),
                            percent(d.inside_raw)
                        ),
                        None => "нет маски".into(),
                    };
                    if d.outside > MAX_RATIO {
                        row.failed.push("image");
                    }
                    if d.inside > MAX_RATIO {
                        row.failed.push("text");
                    }
                }
            }
        }
    } else {
        row.image = "нет эталона".into();
        row.missing_reference = true;
    }
    // Журнал IPC: expect сцены, иначе журнал эталона.
    let expected_log = scene
        .expect
        .ipc_log
        .get(&variant.lang)
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| read_json::<Vec<Value>>(&reference.join(format!("{key}.ipc-log.json"))));
    let actual_log: Option<Vec<Value>> =
        read_json(&variant_path(out, &scene.id, variant, "ipc-log.json"));
    match (expected_log, actual_log) {
        (Some(expected), Some(actual)) => {
            let diff = journal::compare_from(&expected, &actual, scene.journal_from.as_ref());
            row.journal = diff.summary();
            if !diff.equal() {
                row.failed.push("journal");
                row.details
                    .push(format!("журнал:\n{}", diff.lines.join("\n")));
            }
        }
        (None, _) => row.journal = "нет журнала Electron".into(),
        (_, None) => {
            row.journal = "нет журнала GPUI".into();
            row.failed.push("journal");
        }
    }
    row
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn report(options: &Options, out: &Path, rows: &[Row]) -> Result<bool, String> {
    let blocking = |r: &Row| {
        r.failed
            .iter()
            .any(|g| options.fail_on.iter().any(|f| f == g))
    };
    let passed = rows
        .iter()
        .filter(|r| r.failed.is_empty() && !r.missing_reference)
        .count();
    let without = rows.iter().filter(|r| r.missing_reference).count();
    let failing = rows.iter().filter(|r| blocking(r)).count();
    let mut html = String::from(
        "<!doctype html><meta charset=\"utf-8\"><title>hlsr-snap screens</title>\n<style>\
         body{font:14px system-ui,sans-serif;background:#111;color:#ddd;margin:16px}\
         table{border-collapse:collapse}td,th{border:1px solid #333;padding:4px 8px;vertical-align:top}\
         .bad{color:#f66}.ok{color:#6c6}img{width:400px;display:block}\
         .pair{display:flex;gap:8px;flex-wrap:wrap}pre{white-space:pre-wrap;margin:4px 0;color:#bbb}\
         </style>\n",
    );
    html += &format!(
        "<h1>hlsr-snap screens: GPUI против Electron 0.4.9</h1>\n<p>Вне масок текста — порог {THRESHOLD}/255 и не больше {} % пикселей (image); в масках — после размытия σ = {SIGMA} px (text); журнал IPC (journal); шаги (steps). Код выхода — по группам {}.</p>\n<p>Вариантов: {}; проходят: {passed}; без эталона: {without}; не проходят по --fail-on: {failing}.</p>\n",
        MAX_RATIO * 100.0,
        options.fail_on.join(", "),
        rows.len()
    );
    html += "<table><tr><th>Сцена</th><th>Вариант</th><th>Вне масок</th><th>Текст</th><th>Журнал</th><th>Шаги</th><th>Итог</th></tr>\n";
    for (n, r) in rows.iter().enumerate() {
        let verdict = match (r.failed.is_empty(), r.missing_reference) {
            (true, true) => "<span>нет эталона</span>".to_owned(),
            (true, false) => "<span class=ok>да</span>".to_owned(),
            (false, _) => format!("<span class=bad>нет: {}</span>", r.failed.join(", ")),
        };
        html += &format!(
            "<tr><td>{}</td><td><a href=\"#v{n}\">{}</a></td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{verdict}</td></tr>\n",
            escape(&r.scene),
            escape(&r.variant),
            escape(&r.image),
            escape(&r.text),
            escape(&r.journal),
            escape(&r.steps)
        );
    }
    html += "</table>\n";
    for (n, r) in rows.iter().enumerate() {
        html += &format!(
            "<h2 id=\"v{n}\">{} — {}</h2>\n<div class=pair>",
            escape(&r.scene),
            escape(&r.variant)
        );
        for (png, title) in r.pngs.iter().zip(["Electron 0.4.9", "GPUI", "Отличия"]) {
            html += &match png {
                Some(src) => format!(
                    "<figure><figcaption>{title}</figcaption><a href=\"{0}\"><img src=\"{0}\"></a></figure>",
                    escape(src)
                ),
                None => format!("<figure><figcaption>{title}: нет</figcaption></figure>"),
            };
        }
        html += "</div>\n";
        for d in &r.details {
            html += &format!("<pre>{}</pre>\n", escape(d));
        }
    }
    let path = out.join("index.html");
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    std::fs::write(&path, html).map_err(|e| format!("{}: {e}", path.display()))?;
    let summary = json!({
        "variants": rows.len(),
        "passed": passed,
        "withoutReference": without,
        "failing": failing,
        "rows": rows.iter().map(|r| json!({
            "scene": r.scene, "variant": r.variant, "image": r.image, "text": r.text,
            "journal": r.journal, "steps": r.steps, "failed": r.failed,
        })).collect::<Vec<_>>(),
    });
    let summary_path = out.join("summary.json");
    std::fs::write(
        &summary_path,
        serde_json::to_string_pretty(&summary).unwrap_or_default() + "\n",
    )
    .map_err(|e| format!("{}: {e}", summary_path.display()))?;
    println!(
        "вариантов {}, проходят {passed}, без эталона {without}, не проходят по --fail-on {}: {failing} ({}) → {}",
        rows.len(),
        options.fail_on.join(","),
        rows.iter()
            .filter(|r| !r.failed.is_empty())
            .map(|r| format!("{} {}: {}", r.scene, r.variant, r.failed.join("+")))
            .collect::<Vec<_>>()
            .join("; "),
        path.display()
    );
    Ok(failing == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_pick_scene_ids() {
        assert!(glob_match("games/frame", "games/frame"));
        assert!(!glob_match("games/fram", "games/frame"));
        assert!(glob_match("games/*", "games/frame"));
        assert!(glob_match("*frame", "games/frame"));
        assert!(glob_match("g*s/*e", "games/frame"));
        assert!(!glob_match("home/*", "games/frame"));
        assert!(!glob_match("*x*", "games/frame"));
    }

    #[test]
    fn options_default_to_the_049_cache_and_blocking_groups() {
        let o = parse(&[]).unwrap();
        assert!(o.reference.ends_with("target/snap/ref-049"));
        assert_eq!(o.fail_on, ["image", "journal", "steps"]);
        let o =
            parse(&["--zoom", "1,0.72", "--rail", "open", "--only", "games/*"].map(String::from))
                .unwrap();
        assert_eq!(o.filter.zooms, ["1", "0.72"]);
        assert_eq!(o.filter.rails, ["open"]);
        assert!(parse(&["--fail-on", "pixels"].map(String::from)).is_err());
        assert!(parse(&["--jobs", "0"].map(String::from)).is_err());
    }

    #[test]
    fn scene_masks_scale_with_the_zoom_outwards() {
        let mut scene: Scene = serde_json::from_value(json!({
            "id": "t", "route": "/launcher", "platform": "win32",
            "window": {"width": 1600, "height": 900}, "themes": ["hlsr"], "languages": ["ru"],
            "scales": [1], "zooms": [0.75], "now": "2026-11-17T12:00:00.000Z", "timezone": "UTC",
            "masks": [[10, 10, 101, 3]]
        }))
        .unwrap();
        let v = scene.variants(&VariantFilter::default()).remove(0);
        assert_eq!(
            ignored(&scene, &v),
            [Region {
                x: 7,
                y: 7,
                width: 77,
                height: 3
            }]
        );
        scene.masks.clear();
        assert!(ignored(&scene, &v).is_empty());
    }

    #[test]
    fn a_build_without_the_scene_mode_is_refused() {
        let dir = std::env::temp_dir().join(format!("hlsr-snap-mark-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Метка на границе кусков по 1 МиБ.
        let mut bytes = vec![0u8; (1 << 20) - 5];
        bytes.extend_from_slice(b"HLSR_SNAP_SCENE");
        std::fs::write(dir.join("dev.exe"), &bytes).unwrap();
        std::fs::write(dir.join("plain.exe"), &bytes[..bytes.len() - 1]).unwrap();
        assert_eq!(has_scene_mode(&dir.join("dev.exe")), Ok(true));
        assert_eq!(has_scene_mode(&dir.join("plain.exe")), Ok(false));
        assert!(has_scene_mode(&dir.join("nope.exe")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_049_catalog_loads() {
        let dir = screens_049_dir();
        for id in scene_ids(&dir).unwrap() {
            Scene::load(&dir, &id).unwrap();
        }
    }
}
