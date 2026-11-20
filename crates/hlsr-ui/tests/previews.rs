//! Растры персонализации против эталона 0.4.9 (задачи 33, 34, 38; S8 — ник, значок подписчика
//! и рамка; G9 — кадры превью, сцены призрака и спрея, HUD-мод; эталон снят с `HLSR_ELECTRON` =
//! копия 0.4.9): случаи
//! `tools/snap/previews/cases.json`, эталон — PNG `tools/reference/snap-previews.js`
//! (`target/snap/previews-win` на Windows, `target/snap/previews` на Linux; в git их нет).
//! Без эталона случаи рисуются (кадр нужного размера, без паники), а точные кадры HL1
//! сверяются по хешам `tests/fixtures/previews-hl1.json` (снятым с того же эталона;
//! `HLSR_PREVIEWS_REHASH=1` переписывает их по эталону на диске).
//! Отчёт и различия — `target/snap/previews/report.md`, `diff-<id>.png`.

use hlsr_ui::render::hl1_overlay::Hl1Renderer;
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn cases() -> Value {
    let text = std::fs::read_to_string(repo().join("tools/snap/previews/cases.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// Каталог эталона: `HLSR_PREVIEWS_REFERENCE` или набор этой ОС.
fn reference_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("HLSR_PREVIEWS_REFERENCE") {
        return PathBuf::from(dir);
    }
    repo().join("target/snap").join(if cfg!(windows) {
        "previews-win"
    } else {
        "previews"
    })
}

fn out_dir() -> PathBuf {
    let dir = repo().join("target/snap/previews");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Правка поверх `defaultPersonalization()` (extra — только правка).
fn personalization(patch: &Value) -> Value {
    let mut value = hlsr_delivery::ghosting_personalization::default_personalization().to_value();
    let object = value.as_object_mut().unwrap();
    if let Some(patch) = patch.as_object() {
        for (k, v) in patch {
            object.insert(k.clone(), v.clone());
        }
    }
    let extra = patch.get("extra").cloned().unwrap_or_else(|| json!({}));
    object.insert("extra".into(), extra);
    value
}

/// Различие кадра и эталона (RGB, без предумножения); `blurred32` — пиксели, которые
/// отличаются больше чем на 32/255 после размытия обоих σ = 1 физ. px (метрика текста
/// решения 9).
#[derive(Debug, Default, Clone, Copy)]
struct Diff {
    differ: usize,
    over32: usize,
    max: u8,
    total: usize,
    blurred32: usize,
}

fn blur1(image: &image::RgbaImage) -> Vec<[f32; 3]> {
    let (w, h) = (image.width() as i32, image.height() as i32);
    let kernel: Vec<f32> = (-3..=3)
        .map(|i: i32| (-(i * i) as f32 / 2.0).exp())
        .collect();
    let sum: f32 = kernel.iter().sum();
    let at = |x: i32, y: i32| {
        let p = image.get_pixel(x.clamp(0, w - 1) as u32, y.clamp(0, h - 1) as u32);
        [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])]
    };
    let mut tmp = vec![[0f32; 3]; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0f32; 3];
            for (k, weight) in kernel.iter().enumerate() {
                let p = at(x + k as i32 - 3, y);
                for c in 0..3 {
                    acc[c] += p[c] * weight / sum;
                }
            }
            tmp[(y * w + x) as usize] = acc;
        }
    }
    let mut out = vec![[0f32; 3]; tmp.len()];
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0f32; 3];
            for (k, weight) in kernel.iter().enumerate() {
                let yy = (y + k as i32 - 3).clamp(0, h - 1);
                let p = tmp[(yy * w + x) as usize];
                for c in 0..3 {
                    acc[c] += p[c] * weight / sum;
                }
            }
            out[(y * w + x) as usize] = acc;
        }
    }
    out
}

fn compare(id: &str, ours: &image::RgbaImage, reference: &image::RgbaImage) -> Diff {
    let mut diff = Diff {
        total: (ours.width() * ours.height()) as usize,
        ..Diff::default()
    };
    if ours.dimensions() != reference.dimensions() {
        diff.differ = diff.total;
        diff.over32 = diff.total;
        diff.blurred32 = diff.total;
        diff.max = 255;
        return diff;
    }
    let mut map = image::RgbImage::new(ours.width(), ours.height());
    for (x, y, a) in ours.enumerate_pixels() {
        let b = reference.get_pixel(x, y);
        let d = (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
        if d > 0 {
            diff.differ += 1;
        }
        if d > 32 {
            diff.over32 += 1;
        }
        diff.max = diff.max.max(d);
        let v = if d == 0 { b[0] / 4 } else { 255 };
        let dim = if d > 32 { 0 } else { v };
        map.put_pixel(
            x,
            y,
            image::Rgb([v, dim, if d > 32 { 0 } else { b[2] / 4 }]),
        );
    }
    if diff.differ > 0 {
        map.save(out_dir().join(format!("diff-{id}.png"))).unwrap();
        let (a, b) = (blur1(ours), blur1(reference));
        diff.blurred32 = a
            .iter()
            .zip(&b)
            .filter(|(p, q)| (0..3).any(|c| (p[c] - q[c]).abs() > 32.0))
            .count();
    }
    diff
}

fn to_image(pixmap: &tiny_skia::Pixmap) -> image::RgbaImage {
    let data = pixmap.clone().take_demultiplied();
    image::RgbaImage::from_raw(pixmap.width(), pixmap.height(), data).unwrap()
}

fn percent(n: usize, total: usize) -> f64 {
    n as f64 * 100.0 / total as f64
}

fn report(section: &str, rows: &[(String, Option<Diff>, String)]) {
    // Тесты идут параллельно и пишут один файл.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let path = out_dir().join("report.md");
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    // Раздел пишется заново: другие разделы остаются.
    if let Some(start) = text.find(&format!("## {section}\n")) {
        let end = text[start + 3..]
            .find("\n## ")
            .map_or(text.len(), |e| start + 3 + e + 1);
        text.replace_range(start..end, "");
    }
    let _ = writeln!(text, "## {section}\n");
    let _ = writeln!(
        text,
        "| случай | отличается | > 32/255 | после размытия σ = 1 > 32/255 | макс. | примечание |"
    );
    let _ = writeln!(text, "|---|---|---|---|---|---|");
    for (id, diff, note) in rows {
        match diff {
            Some(d) => {
                let _ = writeln!(
                    text,
                    "| {id} | {} ({:.3} %) | {} ({:.3} %) | {:.3} % | {} | {note} |",
                    d.differ,
                    percent(d.differ, d.total),
                    d.over32,
                    percent(d.over32, d.total),
                    percent(d.blurred32, d.total),
                    d.max
                );
            }
            None => {
                let _ = writeln!(text, "| {id} | — | — | — | — | нет эталона {note} |");
            }
        }
    }
    let _ = writeln!(text);
    std::fs::write(path, text).unwrap();
}

fn load_reference(name: &str) -> Option<image::RgbaImage> {
    let path = reference_dir().join(name);
    path.exists()
        .then(|| image::open(path).unwrap().into_rgba8())
}

/// FNV-1a 64 по байтам RGB.
fn fnv_rgb(image: &image::RgbaImage) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for p in image.pixels() {
        for &b in &p.0[..3] {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

fn hl1_hashes() -> Value {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/previews-hl1.json"),
    )
    .unwrap();
    serde_json::from_str(&text).unwrap()
}

/// Хеши точных случаев HL1 — по эталону на диске (после нового прогона snap-previews.js).
fn rehash(cases: &Value, hashes: &mut Value) {
    let mut out = serde_json::Map::new();
    out.insert("about".into(), hashes["about"].clone());
    for case in cases["hl1"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        if case["exact"].as_bool() == Some(true) {
            let reference = load_reference(&format!("{id}.png")).expect("эталон на диске");
            out.insert(id.into(), json!(fnv_rgb(&reference)));
        }
    }
    *hashes = Value::Object(out);
    let text = serde_json::to_string_pretty(hashes).unwrap() + "\n";
    std::fs::write(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/previews-hl1.json"),
        text,
    )
    .unwrap();
}

#[test]
fn hl1_frames_match_0_4_9() {
    let cases = cases();
    let mut hashes = hl1_hashes();
    if std::env::var_os("HLSR_PREVIEWS_REHASH").is_some() {
        rehash(&cases, &mut hashes);
    }
    let mut renderer = Hl1Renderer::new();
    let mut rows = Vec::new();
    let mut failures = Vec::new();
    for case in cases["hl1"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let exact = case["exact"].as_bool().unwrap_or(false);
        let value = personalization(&case["value"]);
        let frame = renderer.render(&value).unwrap();
        let ours = to_image(&frame);
        assert_eq!(ours.dimensions(), (1600, 900));
        ours.save(out_dir().join(format!("{id}.png"))).unwrap();
        let reference = load_reference(&format!("{id}.png"));
        // Хеш кадра Electron (без эталона на диске сверка идёт по нему).
        if exact {
            let expected = hashes[id].as_str().unwrap_or("");
            if let Some(r) = &reference {
                assert_eq!(fnv_rgb(r), expected, "{id}: хеш эталона в фикстуре устарел");
            }
            if fnv_rgb(&ours) != expected {
                failures.push(format!("{id}: хеш {} вместо {expected}", fnv_rgb(&ours)));
            }
        }
        let diff = reference.map(|r| compare(id, &ours, &r));
        if let Some(d) = diff {
            // Без масштаба — те же пиксели; с масштабом (бикубический фильтр Skia против
            // tiny-skia) — порог снимков: ≤ 1 % пикселей отличается больше чем на 32/255.
            let ok = if exact {
                d.differ == 0
            } else {
                d.over32 as f64 <= d.total as f64 * 0.01
            };
            if !ok {
                failures.push(format!("{id}: {d:?}"));
            }
        }
        rows.push((
            id.to_owned(),
            diff,
            if exact {
                "точно".into()
            } else {
                "допуск".into()
            },
        ));
    }
    report("HL1", &rows);
    assert!(
        failures.is_empty(),
        "кадры HL1 разошлись с 0.4.9: {failures:#?}"
    );
}

#[test]
fn livesplit_sample_matches_0_4_7() {
    let mut rows = Vec::new();
    let mut failures = Vec::new();
    for variant in cases()["livesplit"].as_array().unwrap() {
        let variant = variant.as_str().unwrap();
        let id = format!("ls-{variant}");
        let sample = hlsr_ui::render::livesplit_sample::draw_sample(variant).unwrap();
        let ours = to_image(&sample);
        assert_eq!(ours.dimensions(), (280, 332));
        ours.save(out_dir().join(format!("{id}.png"))).unwrap();
        let diff = load_reference(&format!("{id}.png")).map(|r| compare(&id, &ours, &r));
        // Шрифт ОС, сглаживание DirectWrite/FreeType против swash: метрика текста.
        if let Some(d) = diff
            && d.blurred32 as f64 > d.total as f64 * 0.01
        {
            failures.push(format!("{id}: {d:?}"));
        }
        rows.push((id, diff, "допуск по шрифту".to_owned()));
    }
    report("Пример LiveSplit", &rows);
    assert!(
        failures.is_empty(),
        "пример LiveSplit разошёлся с 0.4.7: {failures:#?}"
    );
}

#[test]
fn canvas_fonts_resolve_by_name() {
    use hlsr_ui::render::canvas_text::{Font, layout};
    let rubik = layout("Ag", &Font::parse("500 15px Rubik"));
    assert_eq!(rubik.faces, ["Rubik Medium"]);
    // USE_TYPO_METRICS: typo 935/250 → 14 и 4 (Blink: высота строки 18).
    assert_eq!((rubik.metrics.ascent, rubik.metrics.descent), (14.0, 4.0));
    if cfg!(windows) {
        let line = layout(
            "11:02",
            &Font::parse("600 46px \"Segoe UI\", \"Noto Sans\", sans-serif"),
        );
        assert_eq!(line.faces, ["Segoe UI Semibold"]);
        assert_eq!(
            layout("A", &Font::parse("bold 10px Arial")).faces,
            ["Arial Bold"]
        );
    }
}

#[test]
fn ghosting_frames_match_0_4_9() {
    use hlsr_ui::render::ghosting_overlay::{Assets, OverlayRenderer};
    use hlsr_ui::render::preview;
    let cases = cases();
    let mut renderer = OverlayRenderer::new();
    let mut rows = Vec::new();
    let mut failures = Vec::new();
    for case in cases["ghosting"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let value = personalization(&case["value"]);
        let width = case["frameWidth"].as_u64().unwrap_or(1600) as u32;
        let section = case["section"].as_str().unwrap_or("hud");
        let time = case["time"].as_f64().unwrap_or(0.0);
        let mut assets = Assets::default();
        if let Some(spray) = case["spray"].as_str() {
            let bytes =
                std::fs::read(repo().join("tools/snap/screens/assets").join(spray)).unwrap();
            assets.spray = Some(std::sync::Arc::new(
                hlsr_ui::render::raster::decode(spray, &bytes).unwrap(),
            ));
        }
        // HUD-мод (0.4.9): шрифт цифр из папки мода не читается — запасной DIN из ОС.
        assets.hud_mod = case["hudMod"].as_str() == Some("condensed-hud");
        let started = std::time::Instant::now();
        let mut frame = renderer
            .render(width, &value, section, &assets, time)
            .unwrap();
        let spent = started.elapsed();
        if let Some(patch) = case.get("overlay") {
            let mut raw = hlsr_delivery::livesplit_overlay::default_overlay().to_value();
            for (k, v) in patch.as_object().unwrap() {
                raw[k] = v.clone();
            }
            let overlay = hlsr_delivery::livesplit_overlay::sanitize_overlay(&raw);
            let keyed = preview::keyed_sample(&overlay, "ghosting").unwrap();
            let image = preview::keyed_pixmap(&keyed).unwrap();
            let rect = preview::overlay_rect(&overlay, width, &keyed).unwrap();
            preview::draw_overlay(&mut frame, &overlay, &image, rect);
        }
        let ours = to_image(&frame);
        assert_eq!(ours.dimensions(), (width, 900));
        ours.save(out_dir().join(format!("{id}.png"))).unwrap();
        let diff = load_reference(&format!("{id}.png")).map(|r| compare(id, &ours, &r));
        // WebGL → ЦП и текст: порог снимков (≤ 1 % пикселей > 32/255).
        if let Some(d) = diff
            && d.over32 as f64 > d.total as f64 * 0.01
        {
            failures.push(format!("{id}: {d:?}"));
        }
        rows.push((
            id.to_owned(),
            diff,
            format!("кадр {} мс (отладочная сборка)", spent.as_millis()),
        ));
    }
    report("Ghosting", &rows);
    assert!(
        failures.is_empty(),
        "кадры Ghosting разошлись с 0.4.9: {failures:#?}"
    );
}

/// Коробка 16×16×72 юнита — модель случая `mesh: "box"` (как `caseMesh` страницы эталона).
fn box_mesh() -> Vec<f64> {
    let (x0, y0, z0, x1, y1, z1) = (-8.0, -8.0, 0.0, 8.0, 8.0, 72.0);
    let v = [
        [x0, y0, z0],
        [x1, y0, z0],
        [x1, y1, z0],
        [x0, y1, z0],
        [x0, y0, z1],
        [x1, y0, z1],
        [x1, y1, z1],
        [x0, y1, z1],
    ];
    let faces = [
        [0, 1, 2, 3],
        [4, 5, 6, 7],
        [0, 1, 5, 4],
        [1, 2, 6, 5],
        [2, 3, 7, 6],
        [3, 0, 4, 7],
    ];
    let mut positions = Vec::new();
    for [a, b, c, d] in faces {
        for i in [a, b, c, a, c, d] {
            positions.extend_from_slice(&v[i]);
        }
    }
    positions
}

/// Сцены 0.4.9 «Призраки и запись» и «Эффекты и спреи» (`ghosting-scene-renderer.js`):
/// пляж размыт как `filter: blur()` холста (ядро тройного прямоугольника против Skia),
/// лента следа — градиентом tiny-skia, имя — текстом swash: порог снимков. Подпись спрея —
/// как у эталона (`<id>.json`).
#[test]
fn scenes_match_0_4_9() {
    use hlsr_ui::render::ghost_scene::{Kind, SceneRenderer};
    use hlsr_ui::render::ghosting_overlay::Assets;
    let mut renderer = SceneRenderer::new();
    let mut rows = Vec::new();
    let mut failures = Vec::new();
    for case in cases()["scenes"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let value = personalization(&case["value"]);
        let width = case["frameWidth"].as_u64().unwrap_or(1600) as u32;
        let kind = if case["kind"] == "ghost" {
            Kind::Ghost
        } else {
            Kind::Spray
        };
        let mut assets = Assets::default();
        if let Some(spray) = case["spray"].as_str() {
            let bytes =
                std::fs::read(repo().join("tools/snap/screens/assets").join(spray)).unwrap();
            assets.spray = Some(std::sync::Arc::new(
                hlsr_ui::render::raster::decode(spray, &bytes).unwrap(),
            ));
        }
        if case["mesh"] == "box" {
            assets.ghost_model =
                hlsr_ui::render::ghost_mesh::mesh_sprite(&box_mesh()).map(std::sync::Arc::new);
        }
        let started = std::time::Instant::now();
        let (frame, info) = renderer.render(width, &value, kind, &assets).unwrap();
        let spent = started.elapsed();
        let ours = to_image(&frame);
        assert_eq!(ours.dimensions(), (width, 900));
        ours.save(out_dir().join(format!("{id}.png"))).unwrap();
        if let Ok(text) = std::fs::read_to_string(reference_dir().join(format!("{id}.json"))) {
            let expected: Value = serde_json::from_str(&text).unwrap();
            let want = &expected["info"];
            let got = info.map(|i| (i.units, u64::from(i.texels)));
            let same = match got {
                Some((units, texels)) => {
                    want["units"].as_f64() == Some(units) && want["texels"].as_u64() == Some(texels)
                }
                None => want.is_null(),
            };
            if !same {
                failures.push(format!("{id}: подпись {got:?} вместо {want}"));
            }
        }
        let diff = load_reference(&format!("{id}.png")).map(|r| compare(id, &ours, &r));
        if let Some(d) = diff
            && d.over32 as f64 > d.total as f64 * 0.01
        {
            failures.push(format!("{id}: {d:?}"));
        }
        rows.push((
            id.to_owned(),
            diff,
            format!("кадр {} мс (отладочная сборка)", spent.as_millis()),
        ));
    }
    report("Сцены", &rows);
    assert!(
        failures.is_empty(),
        "сцены разошлись с 0.4.9: {failures:#?}"
    );
}

/// Вид с зумом (`GhostingPreviewViewport.paint`) над кадром hl1-default: зум 200 %,
/// панорама, мелкий вид (уменьшенная копия), рамка ручки.
#[test]
fn views_match_0_4_9() {
    use hlsr_ui::controllers::preview_view::{self as pv, Geometry, View};
    use hlsr_ui::render::preview::{HandleRect, ViewSource, paint_view};
    let frame = Hl1Renderer::new()
        .render(&personalization(&json!({})))
        .unwrap();
    let source = ViewSource::new(frame);
    let mut rows = Vec::new();
    let mut failures = Vec::new();
    for case in cases()["views"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let width = case["box"].as_f64().unwrap();
        let g = Geometry {
            box_width: width,
            box_height: width * 900.0 / 1600.0,
            frame_width: 1600.0,
            frame_height: 900.0,
        };
        let zoom = match case["percent"].as_f64() {
            Some(percent) => percent / 100.0 / g.fit(),
            None => case["zoom"].as_f64().unwrap(),
        };
        let view = pv::clamp(
            &View {
                zoom,
                cx: case["cx"].as_f64().unwrap(),
                cy: case["cy"].as_f64().unwrap(),
            },
            &g,
        );
        let handle = case.get("handle").map(|h| HandleRect {
            left: h["left"].as_f64().unwrap(),
            top: h["top"].as_f64().unwrap(),
            width: h["width"].as_f64().unwrap(),
            height: h["height"].as_f64().unwrap(),
        });
        let hot = case["hot"].as_bool().unwrap_or(false);
        for ratio in [1.0f64, 1.25] {
            let name = format!("{id}@{ratio}");
            let pixels = paint_view(
                &source,
                g.fit() * view.zoom,
                view.cx,
                view.cy,
                g.box_width,
                g.box_height,
                ratio,
                handle,
                hot,
            )
            .unwrap();
            let ours = to_image(&pixels);
            ours.save(out_dir().join(format!("{name}.png"))).unwrap();
            let diff = load_reference(&format!("{name}.png")).map(|r| compare(&name, &ours, &r));
            // Масштабирование Skia (kHigh) против tiny-skia: порог снимков.
            if let Some(d) = diff
                && d.over32 as f64 > d.total as f64 * 0.01
            {
                failures.push(format!("{name}: {d:?}"));
            }
            rows.push((name, diff, format!("{} %", pv::paint(&view, &g).percent)));
        }
    }
    report("Вид с зумом", &rows);
    assert!(failures.is_empty(), "вид разошёлся с 0.4.9: {failures:#?}");
}

/// Самотест 0.4.7 `scripts/hl1-pack-preview-selftest.js` (рендер): пак HUD вместо
/// стандартного, его цифры цветом HUD, прицел пака по форме и цвету и скрытие стандартного
/// прицела в BXT. Какие слои взял рендер — по декодированным картинкам (у 0.4.7 — вызовы
/// `drawImage` подставного холста); пиксели — сцены `hl1-pack-*` выше.
#[test]
fn hl1_renderer_takes_pack_layers_like_0_4_7() {
    let layers = |value: Value| {
        let mut renderer = Hl1Renderer::new();
        renderer.render(&personalization(&value)).unwrap();
        let mut names: Vec<String> = renderer.loaded().map(str::to_owned).collect();
        names.sort();
        names
    };
    let pack = layers(json!({
        "hudEnabled": true,
        "hudColor": {"r": 40, "g": 150, "b": 200},
        "extra": {"packHud": "ag-hud"}
    }));
    assert!(pack.contains(&"hud-ag".to_owned()), "{pack:?}");
    assert!(
        !pack.iter().any(|n| n == "hud" || n == "hud-stock"),
        "{pack:?}"
    );
    // Цифры пака — цветом HUD: тот же кадр с другим цветом отличается.
    let tinted = |rgb: [u8; 3]| {
        Hl1Renderer::new()
            .render(&personalization(&json!({
                "hudEnabled": true,
                "hudColor": {"r": rgb[0], "g": rgb[1], "b": rgb[2]},
                "extra": {"packHud": "ag-hud"}
            })))
            .unwrap()
    };
    assert_ne!(tinted([40, 150, 200]).data(), tinted([200, 40, 40]).data());

    let changer = layers(json!({
        "extra": {"packCrosshair": "crosshair-changer", "changerShape": "3px", "changerColor": "pink"}
    }));
    assert!(changer.contains(&"cross-changer".to_owned()), "{changer:?}");
    // Фрагмент атласа — тот же, что у 0.4.7: [164, 68, 24, 24].
    let meta: Value = serde_json::from_str(hlsr_ui::render::preview_assets::HL1_PACKS).unwrap();
    let pack_meta = hlsr_delivery::hl1_pack_preview::PackMeta {
        shapes: serde_json::from_value(meta["shapes"].clone()).unwrap(),
        colors: serde_json::from_value(meta["colors"].clone()).unwrap(),
        changer_cell: meta["changerCell"].as_f64().unwrap(),
        changer_inset: meta["changerInset"].as_f64().unwrap(),
        changer_size: meta["changerSize"].as_f64().unwrap(),
    };
    let layer = hlsr_delivery::hl1_pack_preview::crosshair_layer(
        &json!({"packCrosshair": "crosshair-changer", "changerShape": "3px", "changerColor": "pink"}),
        &pack_meta,
    )
    .unwrap();
    assert_eq!(layer.rect, [164.0, 68.0, 24.0, 24.0]);

    let hidden = layers(json!({
        "crossEnabled": true,
        "crossHideDefault": true,
        "extra": {"packCrosshair": "crosshair-changer"}
    }));
    assert!(!hidden.iter().any(|n| n.starts_with("cross")), "{hidden:?}");
    let execut4ble = layers(json!({"extra": {"packCrosshair": "execut4ble-crosshair"}}));
    assert!(
        execut4ble.contains(&"cross-execut4ble".to_owned()),
        "{execut4ble:?}"
    );
}

fn css_rgba(text: &str) -> [f32; 4] {
    hlsr_ui::render::css::color(text).unwrap()
}

fn canvas_of(w: u32, h: u32, color: [f32; 4]) -> tiny_skia::Pixmap {
    let mut p = tiny_skia::Pixmap::new(w, h).unwrap();
    p.fill(tiny_skia::Color::from_rgba(color[0], color[1], color[2], color[3]).unwrap());
    p
}

fn place(canvas: &mut tiny_skia::Pixmap, art: &tiny_skia::Pixmap, x: i32, y: i32, opacity: f32) {
    let paint = tiny_skia::PixmapPaint {
        opacity,
        ..Default::default()
    };
    canvas.draw_pixmap(
        x,
        y,
        art.as_ref(),
        &paint,
        tiny_skia::Transform::identity(),
        None,
    );
}

/// Снимок эталона ника или рамки: картинка, масштаб и где лежит элемент (px CSS от угла).
fn shot(name: &str) -> Option<(image::RgbaImage, f32, f32, f32)> {
    let image = load_reference(&format!("{name}.png"))?;
    let meta: Value = serde_json::from_str(
        &std::fs::read_to_string(reference_dir().join(format!("{name}.json"))).ok()?,
    )
    .ok()?;
    let scale = image.width() as f32 / meta["area"]["width"].as_f64()? as f32;
    let left = (meta["rect"]["left"].as_f64()? - meta["area"]["x"].as_f64()?) as f32;
    let top = (meta["rect"]["top"].as_f64()? - meta["area"]["y"].as_f64()?) as f32;
    Some((image, scale, left, top))
}

/// Ник 0.4.9: готовые стили, CSS заготовки с сервера и значок подписчика слева
/// (`.nick-line`: значок 1.05em, поле .3em, низ на .2em ниже линии букв).
#[test]
fn nicks_match_0_4_9() {
    use hlsr_ui::motion::Still;
    use hlsr_ui::render::nick::{self, BADGE_DROP_EM, BADGE_EM, BADGE_GAP_EM, NickText};
    let cases = cases();
    let spec = &cases["nick"];
    let background = css_rgba(spec["background"].as_str().unwrap());
    let mut rows = Vec::new();
    let mut failures = Vec::new();
    for style in spec["styles"].as_array().unwrap() {
        let subscriber = style["subscriber"].as_bool().unwrap_or(false);
        let (preset, css) = (
            style["style"].as_str().unwrap(),
            style["css"].as_str().unwrap(),
        );
        // Без оформления — обычный ник: тот же растр без эффектов.
        let look = match nick::look_of(preset, css).unwrap() {
            Some(look) => look,
            None => std::sync::Arc::new(nick::parse("").unwrap()),
        };
        for text in spec["texts"].as_array().unwrap() {
            for t in style["times"].as_array().unwrap() {
                for scale in [1.0f32, 1.25] {
                    let t = t.as_f64().unwrap();
                    let name = format!(
                        "nick-{}-{}-t{t}@{scale}",
                        style["id"].as_str().unwrap(),
                        text["id"].as_str().unwrap()
                    );
                    let size = text["size"].as_f64().unwrap() as f32;
                    let art = nick::render(
                        &look,
                        &NickText {
                            text: text["text"].as_str().unwrap(),
                            size: text["size"].as_f64().unwrap() as f32,
                            weight: text["weight"].as_u64().unwrap() as u16,
                            letter_spacing: 0.0,
                            color: [1.0, 1.0, 1.0, 1.0],
                            scale,
                        },
                        Some(t),
                        Still::MOVING,
                    )
                    .unwrap();
                    let Some((reference, s, left, top)) = shot(&name) else {
                        rows.push((name, None, String::new()));
                        continue;
                    };
                    let mut canvas = canvas_of(reference.width(), reference.height(), background);
                    let mut left = left;
                    if subscriber {
                        let badge = nick::badge(size, scale, Some(t), Still::MOVING).unwrap();
                        let badge_top = top + art.ascent + (BADGE_DROP_EM - BADGE_EM) * size;
                        let x = ((left + badge.x) * s).round() as i32;
                        let y = ((badge_top + badge.y) * s).round() as i32;
                        place(&mut canvas, &badge.pixels, x, y, 1.0);
                        left += (BADGE_EM + BADGE_GAP_EM) * size;
                    }
                    let x = ((left + art.x) * s).round() as i32;
                    let y = ((top + art.y) * s).round() as i32;
                    place(&mut canvas, &art.pixels, x, y, art.opacity);
                    let mut ours = to_image(&canvas);
                    let mut note = "текст: метрика после размытия";
                    if subscriber {
                        // Текст за значком начинается с доли пикселя (1.35em) и уходит у
                        // Chromium на ≤1 физ. px: здесь сверяется значок, текст — в случаях
                        // без значка (маска — эталон с начала ника).
                        let from = ((left * s).floor() as u32).saturating_sub(1);
                        for (px, py, p) in ours.enumerate_pixels_mut() {
                            if px >= from {
                                *p = *reference.get_pixel(px, py);
                            }
                        }
                        note = "значок; текст за ним — маска (сдвиг ≤1 физ. px)";
                    }
                    ours.save(out_dir().join(format!("{name}.png"))).unwrap();
                    let d = compare(&name, &ours, &reference);
                    if d.blurred32 as f64 > d.total as f64 * 0.01 {
                        failures.push(format!("{name}: {d:?}"));
                    }
                    rows.push((name, Some(d), note.into()));
                }
            }
        }
    }
    report("Ник", &rows);
    assert!(failures.is_empty(), "ник разошёлся с 0.4.9: {failures:#?}");
}

#[test]
fn frames_match_0_4_9() {
    use hlsr_ui::motion::Still;
    use hlsr_ui::render::frame;
    let cases = cases();
    let spec = &cases["frames"];
    let background = css_rgba(spec["background"].as_str().unwrap());
    let hole = css_rgba(spec["hole"].as_str().unwrap());
    let avatar = css_rgba(spec["avatar"].as_str().unwrap());
    let size = spec["size"].as_f64().unwrap() as f32;
    let width = spec["width"].as_f64().unwrap() as f32;
    let mut rows = Vec::new();
    let mut failures = Vec::new();
    for style in spec["styles"].as_array().unwrap() {
        let look = frame::parse(style["css"].as_str().unwrap()).unwrap();
        for t in style["times"].as_array().unwrap() {
            for scale in [1.0f32, 1.25] {
                let t = t.as_f64().unwrap();
                let name = format!("frame-{}-t{t}@{scale}", style["id"].as_str().unwrap());
                let art = frame::render(
                    &look,
                    size,
                    width,
                    None,
                    hole,
                    scale,
                    Some(t),
                    Still::MOVING,
                )
                .unwrap();
                let Some((reference, s, left, top)) = shot(&name) else {
                    rows.push((name, None, String::new()));
                    continue;
                };
                let mut canvas = canvas_of(reference.width(), reference.height(), background);
                let x = ((left + art.x) * s).round() as i32;
                let y = ((top + art.y) * s).round() as i32;
                place(&mut canvas, &art.pixels, x, y, art.opacity);
                // Аватар — круг цвета avatar поверх.
                let mut pb = tiny_skia::PathBuilder::new();
                pb.push_circle(
                    (left + size / 2.0) * s,
                    (top + size / 2.0) * s,
                    size / 2.0 * s,
                );
                let mut paint = tiny_skia::Paint::default();
                paint.set_color(
                    tiny_skia::Color::from_rgba(avatar[0], avatar[1], avatar[2], 1.0).unwrap(),
                );
                canvas.fill_path(
                    &pb.finish().unwrap(),
                    &paint,
                    tiny_skia::FillRule::Winding,
                    tiny_skia::Transform::identity(),
                    None,
                );
                let ours = to_image(&canvas);
                ours.save(out_dir().join(format!("{name}.png"))).unwrap();
                let d = compare(&name, &ours, &reference);
                // Различия — сглаживание края круга (Skia против 4 × 4 подвыборок):
                // метрика после размытия.
                if d.blurred32 as f64 > d.total as f64 * 0.01 {
                    failures.push(format!("{name}: {d:?}"));
                }
                rows.push((name, Some(d), "край круга: метрика после размытия".into()));
            }
        }
    }
    report("Рамка аватара", &rows);
    assert!(
        failures.is_empty(),
        "рамка разошлась с 0.4.9: {failures:#?}"
    );
}

/// Время кадра Ghosting при «движении» (задача 34: 1600×900 ≤ 16 мс): кадры раздела
/// оружия и клавиш подряд, как `queueRender` в rAF. Запуск — сборка release:
/// `cargo test --release -p hlsr-ui --test previews ghosting_frame_time -- --ignored --nocapture`.
#[test]
#[ignore]
fn ghosting_frame_time() {
    use hlsr_ui::render::ghosting_overlay::{Assets, OverlayRenderer};
    let mut rows = Vec::new();
    for (section, extra, width) in [
        ("weapon", json!({}), 1600u32),
        ("weapon", json!({"viewmodel_offset_x": 2.3}), 1600),
        (
            "keys",
            json!({"gh_keyshud": true, "gh_keyshud_full": true}),
            1600,
        ),
        ("weapon", json!({}), 2133),
    ] {
        let mut renderer = OverlayRenderer::new();
        let value = personalization(&json!({ "extra": extra }));
        let assets = Assets::default();
        // Первый кадр: декодирование слоёв и маски HUD — не в счёт.
        renderer
            .render(width, &value, section, &assets, 0.0)
            .unwrap();
        let mut spent = Vec::new();
        for i in 1..=30 {
            let started = std::time::Instant::now();
            let frame = renderer
                .render(width, &value, section, &assets, f64::from(i) / 60.0)
                .unwrap();
            spent.push(started.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(frame.width(), width);
        }
        spent.sort_by(f64::total_cmp);
        let mean = spent.iter().sum::<f64>() / spent.len() as f64;
        let p90 = spent[spent.len() * 9 / 10];
        eprintln!("{section} {width}: среднее {mean:.1} мс, 90 % {p90:.1} мс");
        rows.push(format!(
            "| {section} {extra} | {width}×900 | {mean:.1} | {p90:.1} |"
        ));
    }
    let profile = if cfg!(debug_assertions) {
        "отладочная"
    } else {
        "release"
    };
    let path = out_dir().join("frame-time.md");
    let mut text = format!(
        "## Время кадра Ghosting ({profile}, {} потоков ЦП)\n\n| раздел | кадр | среднее, мс | 90 %, мс |\n|---|---|---|---|\n",
        std::thread::available_parallelism().map_or(1, |n| n.get())
    );
    for row in rows {
        let _ = writeln!(text, "{row}");
    }
    std::fs::write(path, text).unwrap();
}

/// Подбор предкоррекции глифов по примеру LiveSplit (`--ignored`).
#[test]
#[ignore]
fn tune_text_gamma() {
    use hlsr_ui::render::canvas_text::{TextGamma, set_text_gamma};
    let reference = load_reference("ls-hl1.png").expect("эталон");
    let mut best = Vec::new();
    for contrast in [0.0f32, 0.2, 0.5, 1.0] {
        for paint in [1.0f32, 1.2, 1.4, 1.8, 2.2] {
            for device in [1.0f32, 1.2, 1.4, 1.8, 2.2] {
                set_text_gamma(TextGamma {
                    contrast,
                    paint_gamma: paint,
                    device_gamma: device,
                });
                let ours =
                    to_image(&hlsr_ui::render::livesplit_sample::draw_sample("hl1").unwrap());
                let d = compare("tune", &ours, &reference);
                let sum: u64 = ours
                    .pixels()
                    .zip(reference.pixels())
                    .map(|(a, b)| (0..3).map(|c| u64::from(a[c].abs_diff(b[c]))).sum::<u64>())
                    .sum();
                best.push((sum, d.over32, contrast, paint, device));
            }
        }
    }
    best.sort_by_key(|b| b.0);
    for b in best.iter().take(12) {
        eprintln!("{b:?}");
    }
}
