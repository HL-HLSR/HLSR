//! Архив демо 0.4.9: самотест `demo-archive-selftest.js` (список, выбор, имена в zip и упаковка
//! настоящим 7za) и каналы `game-demos-*` фасада. Плюс `tool-*` 0.4.9: зависимости «Демо» на
//! каналах панели — `TOOL_UNKNOWN`.

mod support;

use chrono::{NaiveDate, NaiveDateTime};
use hlsr_core::json_text::stringify;
use hlsr_delivery::api::DeliveryApi;
use hlsr_delivery::archive_service::ArchiveService;
use hlsr_delivery::demo_archive::{self as demo, Demo, PackRequest};
use hlsr_delivery::game_actions::LauncherIdentity;
use hlsr_delivery::manifests;
use hlsr_delivery::ports::MemoryLogger;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use support::api::{Setup, harness, real_7za};
use support::scenario::temp_root;

fn put(root: &Path, relative: &str, text: &str) {
    let file = root.join(relative);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

fn at(day: u32, hour: u32, minute: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 11, day)
        .unwrap()
        .and_hms_opt(hour, minute, 0)
        .unwrap()
}

fn item(dir: &str, name: &str) -> Demo {
    Demo {
        dir: dir.to_owned(),
        name: name.to_owned(),
        size: 1,
        mtime_ms: 1,
    }
}

fn archive() -> ArchiveService {
    ArchiveService::new(real_7za(), Arc::new(MemoryLogger::default()))
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

async fn zip_entries(zip: &Path) -> Vec<String> {
    let mut entries: Vec<String> = archive()
        .list(zip)
        .await
        .unwrap()
        .entries
        .into_iter()
        .map(|entry| entry.path)
        .collect();
    entries.sort();
    entries
}

async fn pack(
    install: &Path,
    items: &[Demo],
    out: &Path,
    slug: &str,
    archive: &ArchiveService,
) -> Result<demo::Packed, hlsr_delivery::errors::Failure> {
    demo::pack_demos(PackRequest {
        install_path: install,
        items,
        archives_dir: out,
        slug,
        now: at(7, 4, 5),
        archive,
        on_progress: None,
    })
    .await
}

#[test]
fn archive_name_uses_local_time_and_suffixes() {
    let date = at(7, 4, 5);
    let base = "hl1-demos-2026-11-07-0405";
    let taken = |names: &[&str]| {
        names
            .iter()
            .map(|n| (*n).to_owned())
            .collect::<HashSet<_>>()
    };
    assert_eq!(
        demo::archive_name("hl1", date, &taken(&[])),
        format!("{base}.zip")
    );
    assert_eq!(
        demo::archive_name("hl1", date, &taken(&[&format!("{base}.zip")])),
        format!("{base}-2.zip")
    );
    let both = [format!("{base}.zip"), format!("{base}-2.zip")];
    let both: Vec<&str> = both.iter().map(String::as_str).collect();
    assert_eq!(
        demo::archive_name("ghosting", date, &taken(&both)),
        "ghosting-demos-2026-11-07-0405.zip"
    );
    assert_eq!(
        demo::archive_name("hl1", date, &taken(&both)),
        format!("{base}-3.zip")
    );
}

#[test]
fn pick_items_refuses_empty_foreign_and_dangerous_names() {
    let scanned = [item("a/valve_WON", "run.dem")];
    assert_eq!(demo::pick_items(&json!([]), &scanned), Err("NO_ITEMS"));
    assert_eq!(demo::pick_items(&Value::Null, &scanned), Err("NO_ITEMS"));
    assert_eq!(
        demo::pick_items(
            &json!([{"dir": "a/valve_WON", "name": "other.dem"}]),
            &scanned
        ),
        Err("DEMO_NOT_FOUND")
    );
    for bad in [
        json!({"dir": "../x", "name": "a.dem"}),
        json!({"dir": "C:/x", "name": "a.dem"}),
        json!({"dir": "a/valve_WON", "name": "a/../b.dem"}),
        json!({"dir": "a/valve_WON", "name": "..\\run.dem"}),
        json!(null),
        json!(5),
    ] {
        assert_eq!(
            demo::pick_items(&json!([bad]), &scanned),
            Err("DEMO_NOT_FOUND")
        );
    }
    let twice = json!([{"dir": "a/valve_WON", "name": "run.dem"}, {"dir": "a/valve_WON", "name": "run.dem"}]);
    assert_eq!(demo::pick_items(&twice, &scanned).unwrap(), scanned);
}

#[test]
fn entry_names_prefix_the_folder_on_a_repeat() {
    let items = [
        item("Half-Life/valve_WON", "run.dem"),
        item("Half-Life/valve_WON_russian", "run.dem"),
    ];
    assert_eq!(
        demo::entry_names(&items),
        ["run.dem", "valve_WON_russian_run.dem"]
    );
    let items = [
        item("Half-Life/valve_WON", "run.dem"),
        item("Half-Life/valve_WON_russian", "RUN.dem"),
        item("Half-Life/valve_WON", "valve_WON_russian_run.dem"),
    ];
    assert_eq!(
        demo::entry_names(&items),
        [
            "run.dem",
            "valve_WON_russian_RUN-2.dem",
            "valve_WON_russian_run.dem"
        ]
    );
}

#[test]
fn is_archive_name_takes_only_demo_zips() {
    for ok in [
        "hl1-demos-2026-11-07-0405.zip",
        "ghosting-demos-2026-11-07-0405-2.zip",
    ] {
        assert!(demo::is_archive_name(ok), "{ok}");
    }
    for bad in [
        "../x.zip",
        "hl1-demos-2026.zip\\..\\a",
        "HL1-demos-2026-11-07-0405.zip",
        "x.zip",
        "hl1-demos-.zip",
        "hl1-demos-2026.zip\n",
    ] {
        assert!(!demo::is_archive_name(bad), "{bad}");
    }
}

#[tokio::test]
async fn pack_two_demos_gives_two_names_in_the_zip_root_and_no_staging() {
    let (_temp, root) = temp_root();
    let (install, out) = (root.join("install"), root.join("Demos"));
    put(&install, "Half-Life/valve_WON/run.dem", "demo");
    put(&install, "Half-Life/valve_WON_russian/run.dem", "demo");
    let items = [
        item("Half-Life/valve_WON", "run.dem"),
        item("Half-Life/valve_WON_russian", "run.dem"),
    ];
    let progress: Arc<Mutex<Vec<u32>>> = Arc::default();
    let sink = progress.clone();
    let packed = demo::pack_demos(PackRequest {
        install_path: &install,
        items: &items,
        archives_dir: &out,
        slug: "hl1",
        now: at(7, 4, 5),
        archive: &archive(),
        on_progress: Some(Arc::new(move |percent| sink.lock().unwrap().push(percent))),
    })
    .await
    .unwrap();
    assert_eq!(packed.name, "hl1-demos-2026-11-07-0405.zip");
    assert!(packed.size > 0);
    assert!(progress.lock().unwrap().iter().all(|p| *p <= 100));
    assert_eq!(names(&out), std::slice::from_ref(&packed.name));
    assert_eq!(
        zip_entries(&out.join(&packed.name)).await,
        ["run.dem", "valve_WON_russian_run.dem"]
    );
}

#[tokio::test]
async fn three_colliding_names_give_three_entries() {
    let (_temp, root) = temp_root();
    let (install, out) = (root.join("install"), root.join("Demos"));
    put(&install, "Half-Life/valve_WON/run.dem", "1");
    put(&install, "Half-Life/valve_WON_russian/run.dem", "2");
    put(
        &install,
        "Half-Life/valve_WON/valve_WON_russian_run.dem",
        "3",
    );
    let items = [
        item("Half-Life/valve_WON", "run.dem"),
        item("Half-Life/valve_WON_russian", "run.dem"),
        item("Half-Life/valve_WON", "valve_WON_russian_run.dem"),
    ];
    let packed = pack(&install, &items, &out, "hl1", &archive())
        .await
        .unwrap();
    let mut expected = demo::entry_names(&items);
    expected.sort();
    assert_eq!(zip_entries(&out.join(&packed.name)).await, expected);
}

#[tokio::test]
async fn old_staging_folder_is_cleaned_and_the_name_skips_taken_ones() {
    let (_temp, root) = temp_root();
    let (install, out) = (root.join("install"), root.join("Demos"));
    put(&install, "ghostingmod/a.dem", "demo");
    put(&out, ".staging-old/x.dem", "old");
    put(&out, "ghosting-demos-2026-11-07-0405.zip", "older");
    let packed = pack(
        &install,
        &[item("ghostingmod", "a.dem")],
        &out,
        "ghosting",
        &archive(),
    )
    .await
    .unwrap();
    assert_eq!(packed.name, "ghosting-demos-2026-11-07-0405-2.zip");
    assert!(!out.join(".staging-old").exists());
}

#[tokio::test]
async fn archiver_failure_leaves_neither_zip_nor_staging() {
    let (_temp, root) = temp_root();
    let (install, out) = (root.join("install"), root.join("Demos"));
    put(&install, "ghostingmod/a.dem", "demo");
    let broken = ArchiveService::new(root.join("no-7za.exe"), Arc::new(MemoryLogger::default()));
    let failure = pack(
        &install,
        &[item("ghostingmod", "a.dem")],
        &out,
        "ghosting",
        &broken,
    )
    .await
    .unwrap_err();
    assert!(!demo::is_demo_not_found(&failure));
    assert_eq!(names(&out), Vec::<String>::new());
}

#[tokio::test]
async fn vanished_demo_is_demo_not_found_and_the_folder_stays_clean() {
    let (_temp, root) = temp_root();
    let (install, out) = (root.join("install"), root.join("Demos"));
    std::fs::create_dir_all(&install).unwrap();
    let failure = pack(
        &install,
        &[item("ghostingmod", "gone.dem")],
        &out,
        "ghosting",
        &archive(),
    )
    .await
    .unwrap_err();
    assert!(demo::is_demo_not_found(&failure), "{failure:?}");
    assert_eq!(names(&out), Vec::<String>::new());
}

#[test]
fn list_ghosting_reads_mod_and_demos_without_nested() {
    let (_temp, install) = temp_root();
    let config = manifests::local_config_of("ghosting-renewed").unwrap();
    let listed = |install: &Path| -> Vec<String> {
        let mut out: Vec<String> = demo::list_demos(config, install)
            .iter()
            .map(|d| format!("{}/{}", d.dir, d.name))
            .collect();
        out.sort();
        out
    };
    put(&install, "ghostingmod/a.dem", "demo");
    put(&install, "ghostingmod/runs/x.run", "run");
    assert_eq!(listed(&install), ["ghostingmod/a.dem"]);
    put(&install, "ghostingmod/demos/b.dem", "demo");
    put(&install, "ghostingmod/demos/sub/x.dem", "demo");
    assert_eq!(
        listed(&install),
        ["ghostingmod/a.dem", "ghostingmod/demos/b.dem"]
    );
}

#[test]
fn list_hl1_keeps_same_names_from_two_folders() {
    let (_temp, install) = temp_root();
    let config = manifests::local_config_of("hl1-won").unwrap();
    assert_eq!(demo::list_demos(config, &install), []);
    put(&install, "Half-Life/valve_WON/run.dem", "demo");
    put(&install, "Half-Life/valve_WON_russian/run.dem", "demo");
    let listed = demo::list_demos(config, &install);
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|d| d.name == "run.dem" && d.mtime_ms > 0));
}

// ---------------------------------------------------------------- каналы

const NOW: i64 = 1_792_152_000_000;

fn installed_ghosting(root: &Path) -> PathBuf {
    let install_path = root.join("lib").join("Games").join("ghosting-renewed");
    put(&install_path, "ghostingmod/a.dem", "demo");
    let installed = json!({
        "schemaVersion": 1, "gameId": "ghosting-renewed", "version": "1.0.13", "state": "ready",
        "installPath": install_path.to_string_lossy(), "installedAt": "2026-10-16T12:00:00.000Z",
    });
    std::fs::write(
        install_path.join("installed.json"),
        stringify(&installed, "  "),
    )
    .unwrap();
    install_path
}

async fn call(api: &DeliveryApi, channel: &str, args: Value) -> Value {
    let args = args.as_array().cloned().unwrap_or_default();
    api.call(channel, &args)
        .await
        .unwrap_or_else(|| panic!("нет канала {channel}"))
}

#[tokio::test]
async fn demos_channels_list_pack_and_reveal_only_demo_zips() {
    let (_temp, root) = temp_root();
    let mut h = harness(Setup {
        root: root.clone(),
        registry: None,
        now: NOW,
        launcher: LauncherIdentity {
            exec_path: PathBuf::from("hlsr"),
            app_image: None,
            packaged: true,
            app_path: None,
        },
    });
    assert_eq!(
        call(&h.api, "game-demos-list", json!(["ghosting-renewed"])).await,
        json!({"status": "error", "code": "NOT_INSTALLED"})
    );
    installed_ghosting(&root);

    let listed = call(&h.api, "game-demos-list", json!(["218"])).await;
    assert_eq!(listed["status"], "ok");
    assert_eq!(listed["demos"][0]["dir"], "ghostingmod");
    assert_eq!(listed["demos"][0]["name"], "a.dem");

    for (items, code) in [
        (json!([]), "NO_ITEMS"),
        (
            json!([{"dir": "ghostingmod", "name": "b.dem"}]),
            "DEMO_NOT_FOUND",
        ),
    ] {
        assert_eq!(
            call(&h.api, "game-demos-pack", json!(["218", items])).await,
            json!({"status": "error", "code": code})
        );
    }
    h.take_events();
    let packed = call(
        &h.api,
        "game-demos-pack",
        json!(["218", [{"dir": "ghostingmod", "name": "a.dem"}]]),
    )
    .await;
    assert_eq!(packed["status"], "ok", "{packed}");
    let name = packed["archive"]["name"].as_str().unwrap().to_owned();
    assert!(name.starts_with("ghosting-demos-") && demo::is_archive_name(&name));
    let zip = root.join("lib").join("Demos").join(&name);
    assert_eq!(
        packed["archive"]["size"],
        json!(std::fs::metadata(&zip).unwrap().len())
    );
    assert!(
        h.take_events()
            .iter()
            .filter(|event| event.channel == "game-demos-pack-progress")
            .all(|event| event.payload["gameId"] == "218")
    );

    h.take_effects();
    for bad in [
        json!("../x.zip"),
        json!("hl1-demos-2000-01-01-0000.zip"),
        json!(5),
    ] {
        assert_eq!(
            call(&h.api, "game-demos-archive-reveal", json!([bad])).await,
            json!({"status": "error", "code": "NOT_FOUND"})
        );
    }
    assert_eq!(
        call(&h.api, "game-demos-archive-reveal", json!([name])).await,
        json!({"status": "ok"})
    );
    assert_eq!(
        h.take_effects(),
        [json!({"shell": "showItemInFolder", "path": zip.to_string_lossy()})]
    );

    // tool-* 0.4.9: зависимости «Демо» на каналах панели неизвестны.
    let unknown = json!({"status": "error", "error": {"code": "TOOL_UNKNOWN"}});
    for (channel, args) in [
        ("tool-install", json!(["ffmpeg"])),
        ("tool-launch", json!(["ffmpeg", null, null, null])),
        ("tool-stop", json!(["bxt-rs", {}])),
        ("tool-remove", json!(["bxt-rs"])),
        ("tool-open-folder", json!(["ffmpeg"])),
    ] {
        assert_eq!(call(&h.api, channel, args).await, unknown, "{channel}");
    }
    assert_eq!(
        call(&h.api, "tool-cancel", json!(["ffmpeg"])).await,
        json!(false)
    );
}
