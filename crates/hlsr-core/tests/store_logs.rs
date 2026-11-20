//! Тексты предупреждений Store. Отдельный файл: подписчик здесь только на
//! поток теста, и соседние тесты, вызывающие те же предупреждения без
//! подписчика, могли бы закешировать для них «не писать».

use hlsr_core::defaults::defaults;
use hlsr_core::store::Store;
use std::sync::{Arc, Mutex};

#[test]
fn store_warnings_use_0_4_7_texts() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("settings.json");
    std::fs::write(&file, "{").unwrap();
    let logs = capture_logs(|| {
        Store::open(file.clone(), defaults("settings"));
    });
    assert!(
        logs.contains(
            "[store] settings.json: не разобран как JSON, отложен: settings.json.broken-"
        ),
        "{logs}"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&file, "{}").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
        let logs = capture_logs(|| {
            let mut s = Store::open(file.clone(), defaults("settings"));
            s.set("config", serde_json::json!({}));
        });
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(
            logs.contains(
                "[store] settings.json: не прочитан (EACCES); запись отключена до успешного чтения, настройки не тронуты"
            ),
            "{logs}"
        );
        assert!(
            logs.contains(
                "[store] settings.json: запись \"config\" пропущена: файл есть, но не прочитан"
            ),
            "{logs}"
        );
    }
}

#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn capture_logs(f: impl FnOnce()) -> String {
    let buffer = Buffer::default();
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap()
}
