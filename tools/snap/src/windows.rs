//! `hlsr-snap windows` — окна установщика и апдейтера (S16) против эталона Electron 0.4.9:
//! сцены `tools/snap/windows.json` × языки × масштабы. Эталон снимает
//! `tools/reference/ui/snap-windows.js` (`<ref>/<сцена>/<язык>@<масштаб>.png`), GPUI — процесс
//! `hlsr --hlsr-updater` (dev-hooks) с `HLSR_SNAP_WINDOW` на вариант; масштаб — rem окна, как
//! `--force-device-scale-factor` эталона. Две доли отличий (порог канала 32): «вся» — как есть,
//! «размытая» — после размытия σ = 1 обеих картинок (сглаживание текста DirectWrite ≠ Chromium).
//!
//!   python scripts/slint/headless-win.py -- target/debug/hlsr-snap.exe windows \
//!     [--ref target/snap/windows-ref] [--out target/snap/windows] [--only часть,…] [--hlsr exe]

use crate::masks::masked_diff_except;
use crate::screens::repo_root;
use image::RgbaImage;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const THRESHOLD: u8 = 32;
const SIGMA: f32 = 1.0;
/// Вариант проходит, если размытая доля не больше 1 %.
const MAX_RATIO: f64 = 0.01;
const TIMEOUT: Duration = Duration::from_secs(60);

struct Options {
    reference: PathBuf,
    out: PathBuf,
    only: Vec<String>,
    hlsr: PathBuf,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let root = repo_root();
    let exe = if cfg!(windows) { "hlsr.exe" } else { "hlsr" };
    let mut o = Options {
        reference: root.join("target/snap/windows-ref"),
        out: root.join("target/snap/windows"),
        only: Vec::new(),
        hlsr: root.join("target/debug").join(exe),
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{arg}: нет значения"));
        match arg.as_str() {
            "--ref" => o.reference = PathBuf::from(value()?),
            "--out" => o.out = PathBuf::from(value()?),
            "--only" => o.only = value()?.split(',').map(str::to_owned).collect(),
            "--hlsr" => o.hlsr = PathBuf::from(value()?),
            other => return Err(format!("неизвестный аргумент {other}")),
        }
    }
    Ok(o)
}

/// Снимок GPUI одного варианта: `Ok(секунды)`.
fn shoot(o: &Options, job: &Value, png: &Path, work: &Path) -> Result<f32, String> {
    let _ = std::fs::remove_dir_all(work);
    std::fs::create_dir_all(work).map_err(|e| e.to_string())?;
    let file = work.join("scene.json");
    std::fs::write(&file, job.to_string()).map_err(|e| e.to_string())?;
    if let Some(dir) = png.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_file(png);
    let started = Instant::now();
    let mut child = Command::new(&o.hlsr)
        .arg("--hlsr-updater")
        .env("HLSR_SNAP_WINDOW", &file)
        .env("HLSR_SMOKE_SNAPSHOT", png)
        .env("HLSR_SMOKE_SNAPSHOT_DELAY", "700")
        .env("HLSR_DATA_DIR", work.join("data"))
        .env("HLSR_LOGS_DIR", work.join("logs"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{}: {e}", o.hlsr.display()))?;
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            if !png.exists() {
                return Err(format!("снимка нет (код {status}; сборка с dev-hooks?)"));
            }
            return Ok(started.elapsed().as_secs_f32());
        }
        if started.elapsed() > TIMEOUT {
            // Свой процесс — по своему PID.
            let _ = child.kill();
            let _ = child.wait();
            return Err("hlsr не закончил за минуту".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn load(path: &Path) -> Option<RgbaImage> {
    image::open(path).ok().map(|img| img.to_rgba8())
}

pub fn main(args: &[String]) -> Result<bool, String> {
    let o = parse(args)?;
    if cfg!(windows) && std::env::var_os("HLSR_HEADLESS_ROOT").is_none() {
        return Err("запускать через scripts/slint/headless-win.py: окна не на экран".into());
    }
    let file = repo_root().join("tools/snap/windows.json");
    let catalog: Value = serde_json::from_str(
        &std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?,
    )
    .map_err(|e| format!("{}: {e}", file.display()))?;
    let list = |key: &str| catalog[key].as_array().cloned().unwrap_or_default();
    let (scales, languages) = (list("scales"), list("languages"));
    let work = o.out.join(".work");
    let mut rows = Vec::new();
    for scene in list("scenes") {
        let id = scene["id"].as_str().unwrap_or_default().to_owned();
        if !o.only.is_empty() && !o.only.iter().any(|part| id.contains(part.as_str())) {
            continue;
        }
        for scale in &scales {
            let zoom = scale.as_f64().unwrap_or(1.);
            let (width, height) = if scene["window"] == "installer" {
                (640., 560.)
            } else {
                let content = &catalog["updaterContent"][scale.to_string()];
                (
                    content["width"].as_f64().unwrap_or(444.),
                    content["height"].as_f64().unwrap_or(241.),
                )
            };
            for language in &languages {
                let name = format!("{}@{scale}.png", language.as_str().unwrap_or("ru"));
                let mut job = scene.clone();
                for (key, value) in [
                    ("zoom", json!(zoom)),
                    ("width", json!(width)),
                    ("height", json!(height)),
                    ("language", language.clone()),
                ] {
                    job[key] = value;
                }
                let png = o.out.join(&id).join(&name);
                let row = match shoot(&o, &job, &png, &work) {
                    Err(e) => format!("{id} {name}: {e}"),
                    Ok(seconds) => match (load(&o.reference.join(&id).join(&name)), load(&png)) {
                        (None, _) => format!("{id} {name}: нет эталона"),
                        (_, None) => format!("{id} {name}: снимок не читается"),
                        (Some(expected), Some(actual)) => {
                            let raw =
                                masked_diff_except(&expected, &actual, None, &[], THRESHOLD, SIGMA);
                            let all = RgbaImage::from_pixel(
                                expected.width(),
                                expected.height(),
                                image::Rgba([255; 4]),
                            );
                            let soft = masked_diff_except(
                                &expected,
                                &actual,
                                Some(&all),
                                &[],
                                THRESHOLD,
                                SIGMA,
                            );
                            let _ = raw.map.save(png.with_extension("diff.png"));
                            let pass = raw.same_size && soft.inside <= MAX_RATIO;
                            rows.push(pass);
                            format!(
                                "{} {id} {name}: вся {:.2} %, размытая {:.2} %{} ({seconds:.1} с)",
                                if pass { "ok  " } else { "FAIL" },
                                raw.outside * 100.,
                                soft.inside * 100.,
                                if raw.same_size {
                                    String::new()
                                } else {
                                    format!(
                                        ", размер {}x{} ≠ {}x{}",
                                        actual.width(),
                                        actual.height(),
                                        expected.width(),
                                        expected.height()
                                    )
                                },
                            )
                        }
                    },
                };
                println!("{row}");
            }
        }
    }
    let _ = std::fs::remove_dir_all(&work);
    let passed = rows.iter().filter(|pass| **pass).count();
    println!(
        "итог: {passed}/{} вариантов в пределах {} % (размытая доля)",
        rows.len(),
        MAX_RATIO * 100.
    );
    Ok(passed == rows.len())
}
