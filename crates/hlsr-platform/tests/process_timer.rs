//! Таймер опроса процесса (раз в 1,5 с) и `dispose` — на виртуальном времени tokio: без
//! ожидания по часам.

use hlsr_core::js::Platform;
use hlsr_delivery::ports::{Logger, MemoryLogger};
use hlsr_platform::game_process_service::{
    GameProcessService, ProcessEvent, ServiceOptions, StartedProcess, signal_name,
};
use hlsr_platform::process_inspect::{ProcessInfo, ProcessInspector};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct Flag(Arc<AtomicBool>);

impl ProcessInspector for Flag {
    fn is_alive(&self, _pid: u32) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    fn inspect(&self, _pid: u32) -> Option<ProcessInfo> {
        None
    }
}

struct Setup {
    service: GameProcessService,
    alive: Arc<AtomicBool>,
    events: Arc<Mutex<Vec<ProcessEvent>>>,
    _state: tempfile::TempDir,
}

fn setup(poll_interval_ms: Option<u64>) -> Setup {
    let state = tempfile::tempdir().unwrap();
    let alive = Arc::new(AtomicBool::new(true));
    let service = GameProcessService::new(ServiceOptions {
        state_dir: state.path().to_path_buf(),
        logger: Some(Arc::new(MemoryLogger::default()) as Arc<dyn Logger>),
        inspector: Some(Arc::new(Flag(alive.clone()))),
        poll_interval_ms,
        platform: Some(Platform::Win32),
        ..ServiceOptions::default()
    });
    let events: Arc<Mutex<Vec<ProcessEvent>>> = Arc::default();
    {
        let events = events.clone();
        service.subscribe(Arc::new(move |event| {
            events.lock().unwrap().push(event.clone())
        }));
    }
    service
        .register_started_process(
            "hl1-won",
            StartedProcess {
                pid: Some(5),
                executable_path: Some("C:\\g\\hl.exe".into()),
                install_path: Some("C:\\g".into()),
                started_at: Some("2026-10-16T12:00:00.000Z".into()),
                ..StartedProcess::default()
            },
        )
        .unwrap();
    Setup {
        service,
        alive,
        events,
        _state: state,
    }
}

/// Дать задачам отработать (в том числе задаче опроса — она ставит свой таймер при первом заходе).
async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

async fn advance(ms: u64) {
    tokio::time::advance(Duration::from_millis(ms)).await;
    settle().await;
}

fn finished(events: &Arc<Mutex<Vec<ProcessEvent>>>) -> usize {
    events
        .lock()
        .unwrap()
        .iter()
        .filter(|event| event.name == "finished")
        .count()
}

#[tokio::test(start_paused = true)]
async fn timer_checks_the_process_every_interval() {
    let s = setup(None);
    settle().await;
    advance(1499).await;
    assert!(
        s.service.is_running("hl1-won"),
        "интервал 1,5 с ещё не прошёл"
    );

    s.alive.store(false, Ordering::SeqCst);
    advance(1).await;
    assert!(
        !s.service.is_running("hl1-won"),
        "процесса нет — игра закрыта"
    );
    assert_eq!(finished(&s.events), 1);

    // Дальше таймер не работает: ничего не приходит.
    advance(10_000).await;
    assert_eq!(finished(&s.events), 1);
}

#[tokio::test(start_paused = true)]
async fn a_live_process_keeps_the_game_running() {
    let s = setup(Some(100));
    settle().await;
    advance(5_000).await;
    assert!(s.service.is_running("hl1-won"));
    assert_eq!(finished(&s.events), 0);
}

#[tokio::test(start_paused = true)]
async fn dispose_stops_the_timer() {
    let s = setup(Some(100));
    settle().await;
    s.alive.store(false, Ordering::SeqCst);
    s.service.dispose();
    advance(5_000).await;
    assert_eq!(
        finished(&s.events),
        0,
        "после dispose выход не отслеживается"
    );
    assert!(!s.service.is_running("hl1-won"));
}

#[tokio::test(start_paused = true)]
async fn unsubscribed_listener_hears_nothing() {
    let s = setup(Some(100));
    settle().await;
    let heard: Arc<Mutex<usize>> = Arc::default();
    let id = {
        let heard = heard.clone();
        s.service
            .subscribe(Arc::new(move |_| *heard.lock().unwrap() += 1))
    };
    s.service.unsubscribe(id);
    s.alive.store(false, Ordering::SeqCst);
    advance(200).await;
    assert_eq!(*heard.lock().unwrap(), 0);
    assert_eq!(
        finished(&s.events),
        1,
        "прежний подписчик продолжает слышать"
    );
}

/// Службу получает доставка игр как `ProcessProbe` и `ProcessStopper` (порты `hlsr-delivery`).
#[tokio::test(start_paused = true)]
async fn the_service_serves_the_delivery_ports() {
    use hlsr_delivery::errors::Failure;
    use hlsr_delivery::ports::{ProcessProbe, ProcessStopper};

    let s = setup(Some(100));
    let probe: Arc<dyn ProcessProbe> = Arc::new(s.service.clone());
    let stopper: Arc<dyn ProcessStopper> = Arc::new(s.service.clone());

    assert!(probe.is_running("hl1-won"));
    assert!(!probe.is_running("hl2-4260"));
    assert_eq!(probe.any_running().as_deref(), Some("hl1-won"));
    assert_eq!(probe.process_state("hl1-won")["state"], "running");
    assert_eq!(probe.process_state("hl2-4260")["state"], "not-running");

    let idle = stopper.request_stop("hl2-4260", false).await.unwrap();
    assert_eq!(idle["status"], "not-running");

    // Инспектор ничего не знает о процессе: PID не принадлежит игре, остановка отменяется.
    let refused = stopper.request_stop("hl1-won", true).await.unwrap_err();
    let Failure::Delivery(error) = refused else {
        panic!("ожидалась ошибка доставки");
    };
    assert_eq!(error.code.as_str(), "PROCESS_MISMATCH");
    assert_eq!(error.details.unwrap()["gameId"], "hl1-won");
}

#[test]
fn signal_names_are_the_ones_node_shows() {
    assert_eq!(signal_name(15), "SIGTERM");
    assert_eq!(signal_name(9), "SIGKILL");
    assert_eq!(signal_name(11), "SIGSEGV");
    assert_eq!(signal_name(99), "SIG99");
}
