//! Замок единственного экземпляра при перезапуске от администратора
//! (`game-launch-restart-elevated` 0.4.7): `releaseSingleInstanceLock` — следующий запуск
//! становится первым; `requestSingleInstanceLock` после отказа UAC — замок и канал снова наши.

use hlsr_core::instance::{self, Acquired};
use hlsr_core::paths::Paths;
use std::time::Duration;

#[test]
fn released_lock_goes_to_the_next_start_and_comes_back() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::under(root.path());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let _enter = rt.enter();

    let Acquired::Primary(mut first) = instance::acquire(&paths, vec![]).unwrap() else {
        panic!("первый запуск должен стать первым экземпляром");
    };
    let mut rx = first.listen();
    assert!(first.is_held());
    assert!(first.reacquire(), "замок и так наш");

    first.release();
    assert!(!first.is_held());
    assert!(first.endpoint().is_none());

    // Замок свободен: следующий запуск (лаунчер от администратора) — первый экземпляр.
    let Acquired::Primary(second) = instance::acquire(&paths, vec![]).unwrap() else {
        panic!("отпущенный замок должен достаться следующему запуску");
    };
    assert!(!first.reacquire(), "замок держит другой экземпляр");
    assert!(!first.is_held());
    drop(second);

    // Отказ UAC: замок берётся обратно, канал снова отдаёт аргументы в прежний приёмник.
    assert!(first.reacquire());
    assert!(first.is_held());
    assert!(first.endpoint().is_some());
    assert!(matches!(
        instance::acquire(&paths, vec!["--select-game=hl1-won".into()]).unwrap(),
        Acquired::Forwarded
    ));
    let got = rt.block_on(tokio::time::timeout(Duration::from_secs(10), rx.recv()));
    assert_eq!(
        got.unwrap().unwrap(),
        vec!["--select-game=hl1-won".to_owned()]
    );
}

/// Установщик и удаление берут только замок (`try_lock`): аргументы никому не передаются, пока
/// лаунчер запущен — `None`; пока замок у установщика, лаунчер не становится первым.
#[test]
fn setup_takes_only_the_lock() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::under(root.path());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let _enter = rt.enter();

    let Acquired::Primary(launcher) = instance::acquire(&paths, vec![]).unwrap() else {
        panic!("первый запуск должен стать первым экземпляром");
    };
    assert!(
        instance::try_lock(&paths).unwrap().is_none(),
        "лаунчер запущен"
    );
    drop(launcher);

    let setup = instance::try_lock(&paths).unwrap().expect("замок свободен");
    assert!(setup.is_held());
    assert!(setup.endpoint().is_none(), "канала у установщика нет");
    assert!(matches!(
        instance::acquire(&paths, vec![]).unwrap(),
        Acquired::Forwarded
    ));
    drop(setup);
    assert!(matches!(
        instance::acquire(&paths, vec![]).unwrap(),
        Acquired::Primary(_)
    ));
}
