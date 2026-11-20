//! Самотесты 0.4.7 для менеджера Proton, Python и пространств имён:
//! `proton-update-selftest.js` (проверка и установка обновлений), `cancellation-selftest.js`
//! (отмена Proton на стадиях метаданных и загрузки), `linux-selftest.js` (закреплённый архив,
//! ссылки в архиве, переход с CachyOS, сборка-заглушка, Python, AppArmor). Архивы собирает
//! системный `tar`; сеть, `pkexec` и `bwrap` — подделки.

mod proton_support;

use hlsr_delivery::providers::BoxFuture;
use hlsr_platform::linux::abort::Abort;
use hlsr_platform::linux::prefix_command::ProtonError;
use hlsr_platform::linux::proton_manager::{
    self, Body, Fetch, PinnedArchive, ProtonEvent, ProtonManager, ProtonManagerOptions,
    RELEASE_TAG, RELEASES_API, ReadText, SelectionData, safe_release_asset, tar_program,
    validate_archive_listing,
};
use hlsr_platform::linux::python_runtime::{Distribution, PythonOptions, PythonRuntime};
use hlsr_platform::linux::user_namespaces::{
    Execute, UserNamespaces, UserNamespacesOptions, umu_root,
};
use hlsr_platform::spawn::{Env, PATH_DELIMITER};
use proton_support::{Root, digests, write_file};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

const UPDATE_TAG: &str = "wineland-11.0-20991231-hlsr99";
const UPDATE_DIR: &str = "proton-wineland-11.0-20991231-hlsr99-x86_64";

fn write_proton(directory: &Path) {
    write_file(
        &directory.join("proton"),
        "#!/bin/sh\nexit 0\n",
        Some(0o755),
    );
}

/// Архив `<name>.tar.xz` с папкой `<name>/proton`, собранный системным `tar`.
fn pack_proton(work: &Path, name: &str) -> Vec<u8> {
    let source = work.join("source");
    write_proton(&source.join(name));
    let file = work.join(format!("{name}.tar.xz"));
    let status = std::process::Command::new(tar_program())
        .arg("-cJf")
        .arg(&file)
        .arg("-C")
        .arg(&source)
        .arg(name)
        .status()
        .unwrap();
    assert!(status.success(), "tar не собрал архив");
    std::fs::read(&file).unwrap()
}

// ---------------------------------------------------------------- подделки сети

/// Сеть самотестов: `readText` с подменяемым поведением и `request` со счётчиком.
#[derive(Default)]
struct Net {
    releases: Mutex<String>,
    sums: Mutex<String>,
    reads: Mutex<Vec<String>>,
    body: Mutex<Vec<u8>>,
    downloads: AtomicUsize,
    /// Читать `.sha512sum` до отмены.
    hang_sums: AtomicBool,
    started: Notify,
    /// При каждом чтении пометить сборку занятой.
    busy_on_read: Mutex<Option<Arc<AtomicBool>>>,
}

struct NetText(Arc<Net>);

impl ReadText for NetText {
    fn read_text<'a>(
        &'a self,
        url: &'a str,
        _limit: Option<usize>,
        abort: Option<&'a Abort>,
    ) -> BoxFuture<'a, Result<String, ProtonError>> {
        let net = self.0.clone();
        Box::pin(async move {
            if let Some(busy) = net.busy_on_read.lock().unwrap().as_ref() {
                busy.store(true, Ordering::SeqCst);
            }
            if url != RELEASES_API && net.hang_sums.load(Ordering::SeqCst) {
                net.started.notify_one();
                abort.expect("сигнал отмены").aborted().await;
                return Err(ProtonError::abort("aborted"));
            }
            net.reads.lock().unwrap().push(url.to_owned());
            Ok(if url == RELEASES_API {
                net.releases.lock().unwrap().clone()
            } else {
                net.sums.lock().unwrap().clone()
            })
        })
    }
}

struct NetRequest(Arc<Net>);

impl Fetch for NetRequest {
    fn request<'a>(
        &'a self,
        _url: &'a str,
        _abort: Option<&'a Abort>,
    ) -> BoxFuture<'a, Result<Box<dyn Body>, ProtonError>> {
        let net = self.0.clone();
        Box::pin(async move {
            net.downloads.fetch_add(1, Ordering::SeqCst);
            let body = net.body.lock().unwrap().clone();
            Ok(Box::new(proton_manager::MemoryBody::new([body])) as Box<dyn Body>)
        })
    }
}

/// `fixture(t)` из `proton-update-selftest.js`: прежняя закреплённая сборка выбрана, на GitHub
/// лежит сборка новее с архивом и `.sha512sum`.
struct UpdateRig {
    root: Root,
    old: String,
    target: String,
    net: Arc<Net>,
    manager: ProtonManager,
    content: Vec<u8>,
    busy: Arc<AtomicBool>,
}

impl UpdateRig {
    fn new() -> UpdateRig {
        let root = Root::new("hlsr-proton-update-");
        let old_name = proton_manager::pinned_archive()
            .name
            .strip_suffix(".tar.xz")
            .unwrap()
            .to_owned();
        let old = root.at(&format!("protons/{old_name}"));
        write_proton(Path::new(&old));
        let work = root.path.join("work");
        let content = pack_proton(&work, UPDATE_DIR);
        let (sha256, sha512) = digests(&content);
        let archive_name = format!("{UPDATE_DIR}.tar.xz");
        let release = json!({
            "tag_name": UPDATE_TAG,
            "assets": [
                {"name": archive_name, "size": content.len(), "digest": format!("sha256:{sha256}"),
                 "browser_download_url": "https://github.com/test/archive"},
                {"name": format!("{UPDATE_DIR}.sha512sum"), "browser_download_url": "https://github.com/test/sums"},
            ],
        });
        let net = Arc::new(Net::default());
        *net.releases.lock().unwrap() = json!([release]).to_string();
        *net.sums.lock().unwrap() = format!("{sha512}  {archive_name}\n");
        *net.body.lock().unwrap() = content.clone();
        let busy = Arc::new(AtomicBool::new(false));
        let in_use = busy.clone();
        let manager = ProtonManager::new(ProtonManagerOptions {
            data_dir: root.path.clone(),
            home: Some(root.path.clone()),
            read_text: Some(Arc::new(NetText(net.clone()))),
            request: Some(Arc::new(NetRequest(net.clone()))),
            in_use: Some(Arc::new(move |_: &Path| in_use.load(Ordering::SeqCst))),
            ..ProtonManagerOptions::default()
        });
        manager.select(&old).unwrap();
        let target = root.at(&format!("protons/{UPDATE_DIR}"));
        UpdateRig {
            root,
            old,
            target,
            net,
            manager,
            content,
            busy,
        }
    }

    fn downloads(&self) -> usize {
        self.net.downloads.load(Ordering::SeqCst)
    }

    fn selected(&self) -> String {
        self.manager.selected().unwrap().path
    }

    fn events(&self, name: &'static str) -> Arc<Mutex<Vec<Value>>> {
        let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
        let sink = seen.clone();
        self.manager.subscribe(Arc::new(move |event: &ProtonEvent| {
            if event.name == name {
                sink.lock().unwrap().push(event.payload.clone());
            }
        }));
        seen
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_checks_metadata_once_and_offers_update_without_downloading() {
    let rig = UpdateRig::new();
    let offers = rig.events("update-available");
    rig.manager.ensure_reference().await.unwrap();
    let (first, second) = tokio::join!(
        rig.manager.check_for_update(),
        rig.manager.check_for_update()
    );
    assert!(first.is_some() && second.is_some());
    assert_eq!(
        *rig.net.reads.lock().unwrap(),
        vec![RELEASES_API.to_owned()]
    );
    assert_eq!(rig.downloads(), 0);
    assert_eq!(rig.selected(), rig.old);
    assert!(!Path::new(&rig.target).exists());
    assert_eq!(rig.manager.status()["update"]["available"], true);
    assert_eq!(
        *offers.lock().unwrap(),
        vec![json!({"from": RELEASE_TAG, "to": UPDATE_TAG})]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_update_installs_verified_archive_once_and_keeps_old_build() {
    let rig = UpdateRig::new();
    // Ссылка префикса на файл прежней сборки (только там, где их можно создать).
    #[cfg(unix)]
    let link = {
        let prefix = rig.root.path.join("compatdata/test/pfx/drive_c/windows");
        std::fs::create_dir_all(&prefix).unwrap();
        let link = prefix.join("test.exe");
        std::os::unix::fs::symlink(Path::new(&rig.old).join("proton"), &link).unwrap();
        link
    };
    let updated = rig.events("updated");
    let (a, b) = tokio::join!(rig.manager.apply_update(), rig.manager.apply_update());
    assert_eq!(a.unwrap().unwrap().path, rig.target);
    assert_eq!(b.unwrap().unwrap().path, rig.target);
    assert_eq!(rig.downloads(), 1);
    assert_eq!(rig.selected(), rig.target);
    assert!(Path::new(&rig.old).join("proton").exists());
    #[cfg(unix)]
    assert!(link.exists());
    let status = rig.manager.status();
    assert_eq!(status["update"]["available"], false);
    assert_eq!(status["phase"], "ready");
    assert_eq!(updated.lock().unwrap().len(), 1);

    let restarted = ProtonManager::new(ProtonManagerOptions {
        data_dir: rig.root.path.clone(),
        home: Some(rig.root.path.clone()),
        ..ProtonManagerOptions::default()
    });
    restarted.ensure_reference().await.unwrap();
    assert_eq!(restarted.selected().unwrap().path, rig.target);
    restarted.select(&rig.old).unwrap();
    assert_eq!(
        restarted.selected().unwrap().path,
        rig.old,
        "ручной откат доступен"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_automatic_installation_announces_start_and_completion_once() {
    let root = Root::new("hlsr-proton-first-");
    // Закреплённая сборка — маленький архив с тем же именем.
    let name = proton_manager::pinned_archive()
        .name
        .strip_suffix(".tar.xz")
        .unwrap()
        .to_owned();
    let content = pack_proton(&root.path.join("work"), &name);
    let (sha256, sha512) = digests(&content);
    let pinned = PinnedArchive {
        name: format!("{name}.tar.xz"),
        size: content.len() as u64,
        sha256,
        sha512,
    };
    let net = Arc::new(Net::default());
    *net.releases.lock().unwrap() = String::new();
    *net.sums.lock().unwrap() = json!({
        "tag_name": RELEASE_TAG,
        "assets": [{"name": pinned.name, "size": pinned.size, "sha256": pinned.sha256,
                    "sha512": pinned.sha512, "browser_download_url": "https://github.com/test/pinned"}],
    })
    .to_string();
    *net.body.lock().unwrap() = content;
    let manager = ProtonManager::new(ProtonManagerOptions {
        data_dir: root.path.clone(),
        home: Some(root.path.clone()),
        read_text: Some(Arc::new(NetText(net.clone()))),
        request: Some(Arc::new(NetRequest(net.clone()))),
        in_use: Some(Arc::new(|_: &Path| false)),
        pinned: Some(pinned),
        ..ProtonManagerOptions::default()
    });
    let events: Arc<Mutex<Vec<&'static str>>> = Arc::default();
    let sink = events.clone();
    manager.subscribe(Arc::new(move |event: &ProtonEvent| match event.name {
        "install-started" => sink.lock().unwrap().push("start"),
        "installed" => sink.lock().unwrap().push("done"),
        _ => {}
    }));
    let (a, b) = tokio::join!(manager.ensure_reference(), manager.ensure_reference());
    let expected = root.at(&format!("protons/{name}"));
    assert_eq!(a.unwrap().unwrap().path, expected);
    assert_eq!(b.unwrap().unwrap().path, expected);
    assert_eq!(*events.lock().unwrap(), vec!["start", "done"]);
    assert_eq!(manager.state().operation.as_deref(), Some("install"));
    assert_eq!(manager.selected_path(), Some(expected));
    manager.ensure_reference().await.unwrap();
    assert_eq!(*events.lock().unwrap(), vec!["start", "done"]);
    assert_eq!(net.downloads.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupted_update_preserves_selection_clears_staging_and_can_be_retried() {
    let rig = UpdateRig::new();
    *rig.net.body.lock().unwrap() = b"corrupt".to_vec();
    let error = rig.manager.apply_update().await.unwrap_err();
    assert!(error.message.contains("не совпал"), "{error:?}");
    assert_eq!(rig.selected(), rig.old);
    assert!(!rig.manager.is_pending());
    assert!(!rig.manager.has_controller());
    assert_eq!(rig.manager.state().phase, "error");
    assert!(rig.manager.update_state().available);
    let names: Vec<String> = std::fs::read_dir(rig.manager.root())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        vec![
            Path::new(&rig.old)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        ]
    );
    *rig.net.body.lock().unwrap() = rig.content.clone();
    rig.manager.apply_update().await.unwrap();
    assert_eq!(rig.selected(), rig.target);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_while_reading_checksums_preserves_offer_and_supports_retry() {
    let rig = UpdateRig::new();
    rig.manager.check_for_update().await;
    rig.net.hang_sums.store(true, Ordering::SeqCst);
    let started = rig.net.started.notified();
    let manager = rig.manager.clone();
    let pending = tokio::spawn(async move { manager.apply_update().await });
    started.await;
    assert!(rig.manager.cancel());
    assert_eq!(pending.await.unwrap().unwrap(), None);
    assert_eq!(rig.manager.state().phase, "idle");
    assert!(!rig.manager.is_pending());
    assert!(!rig.manager.has_controller());
    assert_eq!(rig.selected(), rig.old);
    assert!(rig.manager.update_state().available);
    rig.net.hang_sums.store(false, Ordering::SeqCst);
    rig.manager.apply_update().await.unwrap();
    assert_eq!(rig.selected(), rig.target);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_wine_blocks_switching_before_and_during_update() {
    let rig = UpdateRig::new();
    rig.busy.store(true, Ordering::SeqCst);
    let error = rig.manager.apply_update().await.unwrap_err();
    assert_eq!(error.code.as_deref(), Some("PROTON_IN_USE"));
    assert_eq!(rig.downloads(), 0);
    rig.busy.store(false, Ordering::SeqCst);
    *rig.net.busy_on_read.lock().unwrap() = Some(rig.busy.clone());
    let error = rig.manager.apply_update().await.unwrap_err();
    assert_eq!(error.code.as_deref(), Some("PROTON_IN_USE"));
    assert_eq!(rig.selected(), rig.old);
    assert!(Path::new(&rig.target).exists());
    assert_eq!(rig.manager.state().phase, "error");
    *rig.net.busy_on_read.lock().unwrap() = None;
    rig.busy.store(false, Ordering::SeqCst);
    rig.manager.apply_update().await.unwrap();
    assert_eq!(rig.downloads(), 1);
    assert_eq!(rig.selected(), rig.target);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_preserves_a_custom_selection() {
    let rig = UpdateRig::new();
    let custom = rig.root.at("custom/GE-Proton");
    write_proton(Path::new(&custom));
    rig.manager.add_custom(&custom).unwrap();
    rig.manager.select(&custom).unwrap();
    rig.manager.apply_update().await.unwrap();
    assert_eq!(rig.selected(), custom);
    assert!(Path::new(&rig.target).exists());
    assert!(Path::new(&rig.old).exists());
}

// ---------------------------------------------------------------- отмена (cancellation-selftest)

/// Тело, которое не кончается; помнит, что его бросили (`response.destroyed`).
struct Endless(Arc<AtomicBool>);

impl Body for Endless {
    fn chunk(&mut self) -> BoxFuture<'_, Result<Option<Vec<u8>>, ProtonError>> {
        Box::pin(std::future::pending())
    }
}

impl Drop for Endless {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct StageText {
    checking: bool,
    started: Arc<Notify>,
}

impl ReadText for StageText {
    fn read_text<'a>(
        &'a self,
        _url: &'a str,
        _limit: Option<usize>,
        abort: Option<&'a Abort>,
    ) -> BoxFuture<'a, Result<String, ProtonError>> {
        Box::pin(async move {
            if self.checking {
                self.started.notify_one();
                abort.unwrap().aborted().await;
            }
            let pinned = proton_manager::pinned_archive();
            Ok(json!({
                "tag_name": RELEASE_TAG,
                "assets": [{"name": pinned.name, "size": pinned.size, "sha256": pinned.sha256,
                            "sha512": pinned.sha512, "browser_download_url": "https://github.com/test"}],
            })
            .to_string())
        })
    }
}

struct StageRequest {
    started: Arc<Notify>,
    destroyed: Arc<AtomicBool>,
}

impl Fetch for StageRequest {
    fn request<'a>(
        &'a self,
        _url: &'a str,
        _abort: Option<&'a Abort>,
    ) -> BoxFuture<'a, Result<Box<dyn Body>, ProtonError>> {
        Box::pin(async move {
            self.started.notify_one();
            Ok(Box::new(Endless(self.destroyed.clone())) as Box<dyn Body>)
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_at_metadata_and_download_stages_clears_busy_state_and_staging() {
    for checking in [true, false] {
        let root = Root::new("hlsr-proton-cancel-");
        let started = Arc::new(Notify::new());
        let destroyed = Arc::new(AtomicBool::new(false));
        let manager = ProtonManager::new(ProtonManagerOptions {
            data_dir: root.path.clone(),
            home: Some(root.path.clone()),
            read_text: Some(Arc::new(StageText {
                checking,
                started: started.clone(),
            })),
            request: Some(Arc::new(StageRequest {
                started: started.clone(),
                destroyed: destroyed.clone(),
            })),
            ..ProtonManagerOptions::default()
        });
        let ready = started.notified();
        let pending = {
            let manager = manager.clone();
            tokio::spawn(async move { manager.ensure_reference().await })
        };
        ready.await;
        assert!(manager.cancel());
        assert_eq!(pending.await.unwrap().unwrap(), None);
        let state = manager.state();
        assert_eq!(state.phase, "idle");
        assert_eq!(state.error, None);
        assert!(!manager.is_pending());
        if Path::new(manager.root()).exists() {
            assert_eq!(std::fs::read_dir(manager.root()).unwrap().count(), 0);
        }
        if !checking {
            assert!(destroyed.load(Ordering::SeqCst), "загрузка брошена");
        }
    }
}

// ---------------------------------------------------------------- linux-selftest

#[test]
fn only_the_pinned_verified_archive_is_accepted() {
    let pinned = proton_manager::pinned_archive();
    let archive = json!({
        "name": pinned.name,
        "browser_download_url": "https://example.test/archive",
        "size": pinned.size,
        "digest": format!("sha256:{}", pinned.sha256),
    });
    let release = json!({"tag_name": RELEASE_TAG, "assets": [archive]});
    assert_eq!(safe_release_asset(&release).unwrap(), archive);
    let mut other = release.clone();
    other["tag_name"] = json!("latest");
    assert!(safe_release_asset(&other).is_err());
    let mut bigger = release.clone();
    bigger["assets"][0]["size"] = json!(pinned.size + 1);
    assert!(safe_release_asset(&bigger).is_err());
    let mut digest = release;
    digest["assets"][0]["digest"] = json!(format!("sha256:{}", "0".repeat(64)));
    assert!(safe_release_asset(&digest).is_err());
}

#[test]
fn archive_allows_only_internal_symlinks() {
    let root = proton_manager::pinned_archive()
        .name
        .strip_suffix(".tar.xz")
        .unwrap()
        .to_owned();
    let names = format!("{root}/\n{root}/proton\n{root}/lib/link\n");
    let safe = [
        format!("drwxr-xr-x 0 0 0 0 Sep 14 00:00 {root}/"),
        format!("-rwxr-xr-x 0 0 0 1 Sep 14 00:00 {root}/proton"),
        format!("lrwxrwxrwx 0 0 0 0 Sep 14 00:00 {root}/lib/link -> ../proton"),
    ]
    .join("\n");
    assert_eq!(
        validate_archive_listing(&names, &safe, &root)
            .unwrap()
            .len(),
        3
    );
    assert!(
        validate_archive_listing(&names, &safe.replace("../proton", "../../outside"), &root)
            .is_err()
    );
    assert!(
        validate_archive_listing(&names, &safe.replace("lrwxrwxrwx", "hrwxrwxrwx"), &root).is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_cachyos_moves_to_wineland_but_manual_choice_stays() {
    let root = Root::new("hlsr-proton-migration-");
    let legacy = root.at("protons/proton-cachyos-11.0-20260703-slr-x86_64");
    let upstream = root.at("protons/proton-wineland-11.0-20260913-x86_64");
    let wineland = root.at("protons/proton-wineland-11.0-20260913-hlsr2-x86_64");
    let other_managed = root.at("protons/Proton-Test");
    let custom = root.at("custom/GE-Proton");
    for directory in [&legacy, &upstream, &wineland, &other_managed, &custom] {
        write_file(&Path::new(directory).join("proton"), "test", Some(0o755));
    }
    let manager = ProtonManager::new(ProtonManagerOptions {
        data_dir: root.path.clone(),
        home: Some(root.path.clone()),
        ..ProtonManagerOptions::default()
    });
    manager.select(&legacy).unwrap();
    assert_eq!(
        manager.ensure_reference().await.unwrap().unwrap().path,
        wineland
    );
    assert_eq!(manager.selected_path(), Some(wineland.clone()));
    manager.select(&upstream).unwrap();
    assert_eq!(
        manager.ensure_reference().await.unwrap().unwrap().path,
        wineland
    );
    assert_eq!(manager.selected_path(), Some(wineland.clone()));
    manager.add_custom(&custom).unwrap();
    manager.select(&custom).unwrap();
    assert_eq!(
        manager.ensure_reference().await.unwrap().unwrap().path,
        wineland
    );
    assert_eq!(manager.selected_path(), Some(custom));
    manager.select(&other_managed).unwrap();
    assert_eq!(
        manager.ensure_reference().await.unwrap().unwrap().path,
        wineland
    );
    assert_eq!(manager.selected_path(), Some(other_managed));
}

#[test]
fn old_managed_build_is_not_used_while_the_hlsr_fork_downloads() {
    let root = Root::new("hlsr-proton-gate-");
    let legacy = root.at("protons/proton-wineland-11.0-20260913-x86_64");
    write_file(&Path::new(&legacy).join("proton"), "test", Some(0o755));
    let manager = ProtonManager::new(ProtonManagerOptions {
        data_dir: root.path.clone(),
        home: Some(root.path.clone()),
        ..ProtonManagerOptions::default()
    });
    manager
        .write_selection(&SelectionData {
            path: Some(legacy),
            custom_paths: Vec::new(),
        })
        .unwrap();
    assert_eq!(manager.selected(), None);
}

#[test]
fn selection_file_is_written_with_two_spaces_and_no_newline() {
    let root = Root::new("hlsr-proton-selection-");
    let manager = ProtonManager::new(ProtonManagerOptions {
        data_dir: root.path.join("data"),
        home: Some(root.path.clone()),
        ..ProtonManagerOptions::default()
    });
    manager
        .write_selection(&SelectionData {
            path: Some("/p".to_owned()),
            custom_paths: vec!["/a".to_owned()],
        })
        .unwrap();
    let text = std::fs::read_to_string(root.path.join("data/proton-selection.json")).unwrap();
    assert_eq!(
        text,
        "{\n  \"path\": \"/p\",\n  \"customPaths\": [\n    \"/a\"\n  ]\n}"
    );
    assert_eq!(
        std::fs::read_dir(root.path.join("data")).unwrap().count(),
        1,
        "временный файл убран"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(root.path.join("data/proton-selection.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[test]
fn directory_in_use_reads_exe_and_cmdline_of_processes() {
    let root = Root::new("hlsr-proton-proc-");
    let proc = root.path.join("proc");
    let build = PathBuf::from(root.at("protons/proton-x"));
    std::fs::create_dir_all(&build).unwrap();
    write_file(&proc.join("self/cmdline"), "ignored", None);
    write_file(&proc.join("12/cmdline"), "/usr/bin/python3\0-c\0x\0", None);
    assert!(!proton_manager::directory_in_use_at(&proc, &build));
    let inside = format!(
        "{}{}files{}bin{}wine\0",
        build.to_string_lossy(),
        std::path::MAIN_SEPARATOR,
        std::path::MAIN_SEPARATOR,
        std::path::MAIN_SEPARATOR
    );
    write_file(&proc.join("13/cmdline"), format!("/bin/sh\0{inside}"), None);
    assert!(proton_manager::directory_in_use_at(&proc, &build));
    // Тот же префикс без разделителя — другая папка.
    write_file(
        &proc.join("13/cmdline"),
        format!("{}-other\0", build.to_string_lossy()),
        None,
    );
    assert!(!proton_manager::directory_in_use_at(&proc, &build));
    assert!(!proton_manager::directory_in_use_at(
        &root.path.join("missing"),
        &build
    ));
}

// ---------------------------------------------------------------- Python

fn probe(
    check: impl Fn(&Path) -> bool + Send + Sync + 'static,
) -> hlsr_platform::linux::python_runtime::Probe {
    let check = Arc::new(check);
    Arc::new(move |file: PathBuf, _env: Env| {
        let check = check.clone();
        Box::pin(async move { check(&file) }) as BoxFuture<'static, bool>
    })
}

struct NoDownload;

impl Fetch for NoDownload {
    fn request<'a>(
        &'a self,
        _url: &'a str,
        _abort: Option<&'a Abort>,
    ) -> BoxFuture<'a, Result<Box<dyn Body>, ProtonError>> {
        panic!("системный Python не должен скачиваться");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn working_system_python_is_reused_and_isolated_from_user_site() {
    let root = Root::new("hlsr-python-test-");
    let runtime = PythonRuntime::new(PythonOptions {
        data_dir: root.path.clone(),
        probe: Some(probe(|file| {
            file.to_string_lossy().replace('\\', "/") == "/usable/bin/python3"
        })),
        fetch: Some(Arc::new(NoDownload)),
        ..PythonOptions::default()
    });
    let mut env = Env::new();
    env.insert(
        "PATH".to_owned(),
        format!("/old/bin{PATH_DELIMITER}/usable/bin"),
    );
    env.insert("DISPLAY".to_owned(), ":0".to_owned());
    let prepared = runtime.prepare_environment(env).await.unwrap();
    assert_eq!(
        prepared["PATH"].replace('\\', "/"),
        format!("/usable/bin{PATH_DELIMITER}/old/bin{PATH_DELIMITER}/usable/bin")
    );
    assert_eq!(prepared["PYTHONNOUSERSITE"], "1");
    assert_eq!(prepared["DISPLAY"], ":0");
}

/// Загрузка Python: ошибка «offline», пока `fail`; иначе — архив `python/bin/python3`.
struct PythonDownload {
    fail: AtomicBool,
    downloads: AtomicUsize,
    installed: Arc<AtomicBool>,
    archive: Vec<u8>,
}

impl Fetch for PythonDownload {
    fn request<'a>(
        &'a self,
        _url: &'a str,
        _abort: Option<&'a Abort>,
    ) -> BoxFuture<'a, Result<Box<dyn Body>, ProtonError>> {
        Box::pin(async move {
            self.downloads.fetch_add(1, Ordering::SeqCst);
            // Обе попытки идут одновременно: дать второму вызову встать в очередь (запас на
            // нагруженную машину: пауза длиннее — тест надёжнее, а не медленнее по сути).
            tokio::time::sleep(Duration::from_millis(250)).await;
            if self.fail.load(Ordering::SeqCst) {
                return Err(ProtonError::new("offline"));
            }
            self.installed.store(true, Ordering::SeqCst);
            Ok(Box::new(proton_manager::MemoryBody::new([self.archive.clone()])) as Box<dyn Body>)
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_python_shares_one_download_and_allows_retry_after_failure() {
    let root = Root::new("hlsr-python-download-test-");
    let work = tempfile::tempdir().unwrap();
    write_file(
        &work.path().join("python/bin/python3"),
        "#!/bin/sh\n",
        Some(0o755),
    );
    let file = work.path().join("python.tar.gz");
    let status = std::process::Command::new(tar_program())
        .arg("-czf")
        .arg(&file)
        .arg("-C")
        .arg(work.path())
        .arg("python")
        .status()
        .unwrap();
    assert!(status.success());
    let archive = std::fs::read(&file).unwrap();
    let (sha256, _) = digests(&archive);
    let installed = Arc::new(AtomicBool::new(false));
    let download = Arc::new(PythonDownload {
        fail: AtomicBool::new(true),
        downloads: AtomicUsize::new(0),
        installed: installed.clone(),
        archive: archive.clone(),
    });
    let data = root.path.clone();
    let runtime = PythonRuntime::new(PythonOptions {
        data_dir: root.path.clone(),
        probe: Some(probe(move |file| {
            installed.load(Ordering::SeqCst) && file.starts_with(&data)
        })),
        fetch: Some(download.clone()),
        distribution: Some(Distribution {
            name: "python.tar.gz".to_owned(),
            size: archive.len() as u64,
            sha256,
            url: "https://github.com/test/python.tar.gz".to_owned(),
        }),
        ..PythonOptions::default()
    });
    let mut env = Env::new();
    env.insert("PATH".to_owned(), "/empty".to_owned());
    let (a, b) = tokio::join!(runtime.ensure(&env), runtime.ensure(&env));
    assert!(a.is_err() && b.is_err());
    assert_eq!(download.downloads.load(Ordering::SeqCst), 1);
    assert!(!runtime.is_pending());
    download.fail.store(false, Ordering::SeqCst);
    let (a, b) = tokio::join!(runtime.ensure(&env), runtime.ensure(&env));
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a, b);
    assert_eq!(download.downloads.load(Ordering::SeqCst), 2);
    assert!(
        a.to_string_lossy()
            .replace('\\', "/")
            .ends_with("/bin/python3")
    );
    assert!(a.starts_with(runtime.root()));
    assert!(runtime.root().join("bin/python3").exists());
    let staging: Vec<String> = std::fs::read_dir(runtime.root().parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        staging,
        vec!["python-3.12.15-20261001".to_owned()],
        "временных папок нет"
    );
}

// ---------------------------------------------------------------- AppArmor

struct FakeExecute {
    bwrap: String,
    granted: AtomicBool,
    approvals: AtomicUsize,
    expected_pkexec: Vec<String>,
}

impl Execute for FakeExecute {
    fn execute<'a>(
        &'a self,
        file: &'a str,
        args: &'a [String],
        _env: &'a Env,
        _timeout: Duration,
    ) -> BoxFuture<'a, Result<String, ProtonError>> {
        Box::pin(async move {
            if file == "/usr/bin/pkexec" {
                self.approvals.fetch_add(1, Ordering::SeqCst);
                let args: Vec<String> = args.iter().map(|arg| arg.replace('\\', "/")).collect();
                assert_eq!(args, self.expected_pkexec);
                self.granted.store(true, Ordering::SeqCst);
                return Ok(String::new());
            }
            assert_eq!(file.replace('\\', "/"), self.bwrap);
            assert_eq!(
                args,
                ["--ro-bind", "/", "/", "--unshare-user", "--", "/bin/true"]
            );
            // Параллельные вызовы успевают встать в очередь за одним запросом (запас на нагрузку).
            tokio::time::sleep(Duration::from_millis(150)).await;
            if self.granted.load(Ordering::SeqCst) {
                Ok(String::new())
            } else {
                Err(ProtonError::new("Permission denied"))
            }
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn apparmor_approval_is_requested_once_and_only_after_bwrap_refuses() {
    let root = Root::new("hlsr-userns-");
    let bwrap = root
        .path
        .join("umu/steamrt4/pressure-vessel/libexec/steam-runtime-tools-0/srt-bwrap");
    write_file(&bwrap, "test", None);
    let execute = Arc::new(FakeExecute {
        bwrap: bwrap.to_string_lossy().replace('\\', "/"),
        granted: AtomicBool::new(false),
        approvals: AtomicUsize::new(0),
        expected_pkexec: vec![
            "/bin/bash".to_owned(),
            "/app/resources/linux/install-userns-profile.sh".to_owned(),
            "1000".to_owned(),
            root.path.join("umu").to_string_lossy().replace('\\', "/"),
        ],
    });
    let support = UserNamespaces::new(UserNamespacesOptions {
        resources_path: PathBuf::from("/app/resources"),
        is_restricted: Some(Arc::new(|| true)),
        execute: Some(execute.clone()),
        uid: Some(1000),
        ..UserNamespacesOptions::default()
    });
    let mut env = Env::new();
    env.insert("XDG_DATA_HOME".to_owned(), root.text.clone());
    let (a, b) = tokio::join!(support.ensure(&env), support.ensure(&env));
    a.unwrap();
    b.unwrap();
    support.ensure(&env).await.unwrap();
    assert_eq!(execute.approvals.load(Ordering::SeqCst), 1);
}

struct NoCommands;

impl Execute for NoCommands {
    fn execute<'a>(
        &'a self,
        _file: &'a str,
        _args: &'a [String],
        _env: &'a Env,
        _timeout: Duration,
    ) -> BoxFuture<'a, Result<String, ProtonError>> {
        panic!("команды не нужны");
    }
}

#[tokio::test]
async fn no_apparmor_restriction_runs_no_commands_and_umu_paths_follow_xdg() {
    let support = UserNamespaces::new(UserNamespacesOptions {
        is_restricted: Some(Arc::new(|| false)),
        execute: Some(Arc::new(NoCommands)),
        ..UserNamespacesOptions::default()
    });
    support.ensure(&Env::new()).await.unwrap();
    let root = |pairs: &[(&str, &str)]| {
        let env: Env = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        umu_root(&env).to_string_lossy().replace('\\', "/")
    };
    assert_eq!(root(&[("XDG_DATA_HOME", "/data")]), "/data/umu");
    assert_eq!(
        root(&[("UMU_FOLDERS_PATH", "/custom"), ("XDG_DATA_HOME", "/data")]),
        "/custom/umu"
    );
    assert_eq!(
        root(&[("container", "flatpak"), ("HOST_XDG_DATA_HOME", "/host")]),
        "/host/umu"
    );
}

#[test]
fn apparmor_sysctl_is_read_from_a_root() {
    let root = Root::new("hlsr-sysctl-");
    let at = &root.path;
    assert!(!hlsr_platform::linux::user_namespaces::app_armor_restricted_in(at));
    write_file(
        &at.join("proc/sys/kernel/apparmor_restrict_unprivileged_userns"),
        "1\n",
        None,
    );
    assert!(hlsr_platform::linux::user_namespaces::app_armor_restricted_in(at));
    write_file(
        &at.join("proc/sys/kernel/apparmor_restrict_unprivileged_userns"),
        "0\n",
        None,
    );
    assert!(!hlsr_platform::linux::user_namespaces::app_armor_restricted_in(at));
}

#[test]
fn pkexec_command_is_only_built_not_run() {
    let args = hlsr_platform::linux::user_namespaces::pkexec_args(
        Path::new("/opt/hlsr/resources/linux/install-userns-profile.sh"),
        1000,
        Path::new("/home/u/.local/share/umu"),
    );
    assert_eq!(
        args,
        [
            "/bin/bash",
            "/opt/hlsr/resources/linux/install-userns-profile.sh",
            "1000",
            "/home/u/.local/share/umu"
        ]
    );
    assert_eq!(
        hlsr_platform::linux::user_namespaces::PKEXEC,
        "/usr/bin/pkexec"
    );
}

// ---------------------------------------------------------------- команда в префиксе

mod prefix_command {
    use hlsr_delivery::errors::RawError;
    use hlsr_platform::linux::prefix_command::{WaitOptions, wait_for_prefix_command};
    use hlsr_platform::spawn::{Child, ChildControl, SpawnOptions, Spawner, Stdio, SystemSpawner};
    use std::sync::{Arc, Mutex};

    /// Сигналы, которые получил поддельный процесс (группы у него нет).
    #[derive(Default)]
    struct Signals(Mutex<Vec<String>>);

    impl ChildControl for Signals {
        fn kill(&self, signal: &str) -> bool {
            self.0.lock().unwrap().push(signal.to_owned());
            true
        }
        fn kill_group(&self, _signal: &str) -> bool {
            false
        }
    }

    fn options(timeout: Option<u64>, grace: Option<u64>) -> WaitOptions {
        WaitOptions {
            label: Some("umu-run winetricks corefonts".to_owned()),
            timeout_ms: timeout,
            kill_grace_ms: grace,
        }
    }

    #[tokio::test]
    async fn failure_keeps_code_and_bounded_cleaned_diagnostics() {
        let (child, controller) = Child::new(Some(10), Arc::new(Signals::default()));
        controller.stdout(format!("{}\ncorefonts download\n", "x".repeat(40_000)).into_bytes());
        controller.stderr(b"HLSR_PRESENCE_TOKEN=do-not-log-me\nCannot find cabextract\n".to_vec());
        controller.exit(Some(1), None);
        let error = wait_for_prefix_command(&child, &options(None, None))
            .await
            .unwrap_err();
        assert_eq!(error.code.as_deref(), Some("PROTON_PREFIX_COMMAND_FAILED"));
        assert_eq!(error.exit_code, Some(1));
        assert!(
            error
                .message
                .starts_with("umu-run winetricks corefonts: завершился с кодом 1\n")
        );
        assert!(error.message.contains("Cannot find cabextract"));
        assert!(error.message.contains("corefonts download"));
        assert!(!error.message.contains("do-not-log-me"));
        // Сначала ошибки, потом вывод.
        let stderr_at = error.message.find("Cannot find cabextract").unwrap();
        let stdout_at = error.message.find("corefonts download").unwrap();
        assert!(stderr_at < stdout_at);
        assert!(error.stdout.unwrap().len() <= 8192);
        assert!(error.stderr.unwrap().len() <= 8192);
    }

    #[tokio::test(start_paused = true)]
    async fn hung_command_gets_sigterm_then_sigkill_and_a_timeout_code() {
        let signals = Arc::new(Signals::default());
        let (child, controller) = Child::new(None, signals.clone());
        controller.stderr(b"download stalled".to_vec());
        let error = wait_for_prefix_command(&child, &options(Some(20), Some(10)))
            .await
            .unwrap_err();
        assert_eq!(error.code.as_deref(), Some("PROTON_PREFIX_COMMAND_TIMEOUT"));
        assert_eq!(
            error.message,
            "umu-run winetricks corefonts: превышено время ожидания (0 с)\ndownload stalled"
        );
        assert_eq!(*signals.0.lock().unwrap(), ["SIGTERM", "SIGKILL"]);
        drop(controller);
    }

    // Часы остановлены: сроки 20 и 40 мс идут в своём порядке, как бы ни была нагружена машина
    // (на настоящих часах задержка в 40 мс отдавала выход раньше срока тайм-аута).
    #[tokio::test(start_paused = true)]
    async fn command_that_exits_after_sigterm_is_still_a_timeout() {
        let signals = Arc::new(Signals::default());
        let (child, controller) = Child::new(Some(5), signals.clone());
        let controller = Arc::new(controller);
        let exit = controller.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            exit.exit(None, Some("SIGTERM"));
        });
        let error = wait_for_prefix_command(&child, &options(Some(20), Some(5_000)))
            .await
            .unwrap_err();
        assert_eq!(error.code.as_deref(), Some("PROTON_PREFIX_COMMAND_TIMEOUT"));
        assert_eq!(*signals.0.lock().unwrap(), ["SIGTERM"]);
    }

    #[tokio::test]
    async fn spawn_error_keeps_its_code_and_signal_exit_is_named() {
        let (child, controller) = Child::new(None, Arc::new(Signals::default()));
        controller.error(RawError {
            code: Some("ENOENT".to_owned()),
            message: "spawn /test/umu-run ENOENT".to_owned(),
            ..RawError::default()
        });
        let error = wait_for_prefix_command(&child, &options(None, None))
            .await
            .unwrap_err();
        assert_eq!(error.code.as_deref(), Some("ENOENT"));
        assert_eq!(
            error.message,
            "umu-run winetricks corefonts: spawn /test/umu-run ENOENT"
        );
        assert_eq!(error.exit_code, None);

        let (child, controller) = Child::new(Some(3), Arc::new(Signals::default()));
        controller.exit(None, Some("SIGKILL"));
        let error = wait_for_prefix_command(&child, &WaitOptions::default())
            .await
            .unwrap_err();
        assert_eq!(error.message, "umu-run: завершился с кодом SIGKILL");
        assert_eq!(error.code.as_deref(), Some("PROTON_PREFIX_COMMAND_FAILED"));
    }

    /// Настоящий дочерний процесс с каналами: вывод, ошибки и код выхода.
    #[tokio::test]
    async fn real_child_output_and_exit_code_are_collected() {
        let (command, args): (&str, Vec<String>) = if cfg!(windows) {
            (
                "cmd",
                vec![
                    "/d".into(),
                    "/c".into(),
                    "echo out& echo err 1>&2& exit 3".into(),
                ],
            )
        } else {
            (
                "/bin/sh",
                vec!["-c".into(), "echo out; echo err 1>&2; exit 3".into()],
            )
        };
        let env = hlsr_platform::spawn::process_env();
        let child = SystemSpawner.spawn(
            command,
            &args,
            SpawnOptions {
                env,
                cwd: None,
                stdio: Stdio::Pipe,
                detached: true,
                windows_hide: None,
            },
        );
        assert!(child.pid().is_some());
        let error = wait_for_prefix_command(&child, &options(Some(30_000), None))
            .await
            .unwrap_err();
        assert_eq!(error.exit_code, Some(3));
        assert_eq!(error.stdout.as_deref(), Some("out"));
        assert_eq!(error.stderr.as_deref(), Some("err"));
        assert_eq!(
            error.message,
            "umu-run winetricks corefonts: завершился с кодом 3\nerr\nout"
        );
    }

    /// Настоящий зависший процесс останавливается по тайм-ауту.
    #[tokio::test]
    async fn real_hung_child_is_stopped_on_timeout() {
        let (command, args): (&str, Vec<String>) = if cfg!(windows) {
            ("ping", vec!["-n".into(), "30".into(), "127.0.0.1".into()])
        } else {
            ("/bin/sleep", vec!["30".into()])
        };
        let child = SystemSpawner.spawn(
            command,
            &args,
            SpawnOptions {
                env: hlsr_platform::spawn::process_env(),
                cwd: None,
                stdio: Stdio::Pipe,
                detached: true,
                windows_hide: None,
            },
        );
        let started = std::time::Instant::now();
        let error = wait_for_prefix_command(&child, &options(Some(300), Some(2_000)))
            .await
            .unwrap_err();
        assert_eq!(error.code.as_deref(), Some("PROTON_PREFIX_COMMAND_TIMEOUT"));
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        assert!(child.exit_state().is_some(), "процесс завершён");
    }
}
