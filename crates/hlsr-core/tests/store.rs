use hlsr_core::defaults::defaults;
use hlsr_core::json_store::write_state;
use hlsr_core::json_text::JsonFormat;
use hlsr_core::language::Language;
use hlsr_core::paths::Paths;
use hlsr_core::settings::{RendererChoice, Settings};
use hlsr_core::store::Store;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/json")
        .join(name)
}

fn settings_with(dir: &Path, data: Value) -> (Paths, Settings) {
    let paths = Paths::under(dir);
    write_state(&paths.file("settings.json"), &data, JsonFormat::Store).unwrap();
    let settings = Settings::open(&paths);
    (paths, settings)
}

#[test]
fn missing_file_gives_defaults_and_is_writable() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(dir.path().join("settings.json"), defaults("settings"));
    assert_eq!(s.get("config").unwrap()["gpuAcceleration"], true);
    assert!(s.set("x", json!(1)));
    assert!(dir.path().join("settings.json").exists());
}

#[test]
fn broken_file_is_quarantined_then_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("settings.json");
    std::fs::write(&file, "{\"config\":").unwrap();
    let s = Store::open(file.clone(), defaults("settings"));
    assert!(s.is_writable());
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        names.iter().any(|n| n.starts_with("settings.json.broken-")),
        "{names:?}"
    );
    assert!(!file.exists());
}

#[cfg(unix)]
#[test]
fn unreadable_file_is_never_overwritten() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("settings.json");
    std::fs::write(&file, "{\"config\":{\"language\":1}}").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
    let mut s = Store::open(file.clone(), defaults("settings"));
    assert!(!s.is_writable());
    assert!(!s.set("config", json!({})));
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "{\"config\":{\"language\":1}}"
    );
}

#[test]
fn settings_keeps_unknown_config_fields_and_order() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    std::fs::copy(fixture("settings-store.json"), paths.file("settings.json")).unwrap();
    let mut s = Settings::open(&paths);
    assert_eq!(s.ui_scale(), 1.25);
    assert_eq!(s.renderer(), RendererChoice::Software); // gpuAcceleration: false, ключа renderer нет
    assert_eq!(s.language(), Some(Language::Ru));
    assert!(s.set_language(Language::En));
    let text = std::fs::read_to_string(paths.file("settings.json")).unwrap();
    let keys: Vec<_> = serde_json::from_str::<Value>(&text).unwrap()["config"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        keys,
        ["language", "gpuAcceleration", "uiScale", "futureField"]
    );
    assert!(text.contains("\"nested\": [\n\t\t\t\t1,"));
}

#[test]
fn settings_rejects_non_integer_language_and_bad_types() {
    let dir = tempfile::tempdir().unwrap();
    let (_, s) = settings_with(
        dir.path(),
        json!({"config": {"language": "1", "uiScale": "1.25", "gpuAcceleration": "no"}}),
    );
    assert_eq!(s.language(), None);
    assert_eq!(s.ui_scale(), 1.25); // как Number("1.25")
    assert_eq!(s.renderer(), RendererChoice::Auto);

    for (language, expected) in [
        (json!(0), Some(Language::En)),
        (json!(1.0), Some(Language::Ru)), // Number.isInteger(1.0)
        (json!(1.5), None),
        (json!(2), None),
        (json!(-1), None),
        (Value::Null, None),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (_, s) = settings_with(dir.path(), json!({"config": {"language": language}}));
        assert_eq!(s.language(), expected, "{language}");
    }
}

#[test]
fn renderer_key_wins_over_gpu_acceleration() {
    let renderer = |config: Value| {
        let dir = tempfile::tempdir().unwrap();
        settings_with(dir.path(), json!({ "config": config }))
            .1
            .renderer()
    };
    assert_eq!(
        renderer(json!({"gpuAcceleration": false, "renderer": "vulkan"})),
        RendererChoice::Vulkan
    );
    assert_eq!(
        renderer(json!({"gpuAcceleration": true, "renderer": "software"})),
        RendererChoice::Software
    );
    assert_eq!(
        renderer(json!({"renderer": "opengl"})),
        RendererChoice::OpenGl
    );
    assert_eq!(renderer(json!({"renderer": "auto"})), RendererChoice::Auto);
    assert_eq!(
        renderer(json!({"gpuAcceleration": false, "renderer": "metal"})),
        RendererChoice::Software
    );
    assert_eq!(
        renderer(json!({"gpuAcceleration": false, "renderer": 1})),
        RendererChoice::Software
    );
    assert_eq!(
        renderer(json!({"gpuAcceleration": 0})),
        RendererChoice::Auto
    );
    assert_eq!(renderer(json!({})), RendererChoice::Auto);
    #[cfg(windows)]
    assert_eq!(renderer(json!({"renderer": "dx12"})), RendererChoice::Dx12);
    #[cfg(not(windows))]
    assert_eq!(renderer(json!({"renderer": "dx12"})), RendererChoice::Auto);
    assert_eq!(RendererChoice::Dx12.as_str(), "dx12");
}

#[test]
fn set_ui_scale_normalizes_like_0_4_7() {
    let dir = tempfile::tempdir().unwrap();
    let (paths, mut s) = settings_with(dir.path(), json!({"config": {"language": 1}}));
    assert!(s.set_ui_scale(1.5));
    assert_eq!(s.ui_scale(), 1.5);
    assert!(s.set_ui_scale(2.0));
    assert_eq!(s.ui_scale(), 1.0);
    let text = std::fs::read_to_string(paths.file("settings.json")).unwrap();
    assert_eq!(
        text,
        "{\n\t\"config\": {\n\t\t\"language\": 1,\n\t\t\"uiScale\": 1\n\t}\n}"
    );
}

#[test]
fn defaults_are_from_0_4_7() {
    assert_eq!(defaults("library")["lastLaunched"], "");
    assert_eq!(defaults("settings")["config"]["theme"], 0);
    assert!(defaults("scripts").get("data").is_some());
    assert_eq!(defaults("unknown"), json!({}));
}
