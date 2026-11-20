use super::*;
use crate::intl;
use std::cell::Cell;

/// 2026-10-16T12:00:00Z.
const NOW: i64 = 1_792_152_000_000;

struct Harness {
    machine: NotifMachine,
    mono: Rc<Cell<f64>>,
}

fn en() -> Tr {
    Rc::new(|key, args| crate::tr(Language::En, key, args))
}

fn ru() -> Tr {
    Rc::new(|key, args| crate::tr(Language::Ru, key, args))
}

fn t(key: &str, args: &[&str]) -> String {
    crate::tr(Language::En, key, args)
}

fn harness() -> Harness {
    let mono = Rc::new(Cell::new(0.0));
    let clock = mono.clone();
    Harness {
        machine: NotifMachine::new(WallClock::fixed(NOW, 0), Rc::new(move || clock.get())),
        mono,
    }
}

fn item(id: u32, code: &str, extra: Value) -> Value {
    let mut item = json!({"id": id, "code": code, "level": "info", "read": false,
                          "ts": NOW - 1000, "count": 1, "detail": null, "meta": {}});
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        item[key] = value;
    }
    item
}

impl Harness {
    /// Ответ на последний `notifications-list` (номер — из эффекта).
    fn answer(&mut self, items: Vec<Value>) -> Vec<Effect> {
        let sequence = self.machine.sequence;
        self.machine.list_reply(sequence, Ok(snapshot(items)))
    }
}

#[test]
fn mounting_asks_for_the_list_once() {
    let mut h = harness();
    assert_eq!(
        h.machine.mounted(),
        vec![Effect::Invoke {
            tag: Tag::List(1),
            channel: "notifications-list",
            args: vec![]
        }]
    );
}

#[test]
fn the_first_snapshot_previews_only_updates_and_items_younger_than_15_s() {
    let mut h = harness();
    h.machine.mounted();
    let old = item(1, "NETWORK_ERROR", json!({"ts": NOW - 60_000}));
    let young = item(2, "STEAM_NOT_RUNNING", json!({"ts": NOW - 5_000}));
    let update = item(
        3,
        "LAUNCHER_UPDATE_AVAILABLE",
        json!({"ts": NOW - 3_600_000}),
    );
    let effects = h.answer(vec![old, young, update]);
    // Очередь собирается с конца: старшие показываются позже.
    assert_eq!(h.machine.previewing().as_deref(), Some("3"));
    assert_eq!(h.machine.queued(), vec!["2"]);
    assert_eq!(effects, vec![Effect::StartPreviewTimer(PREVIEW_MS)]);
}

#[test]
fn later_snapshots_preview_new_ids_and_bigger_counts_only() {
    let mut h = harness();
    h.machine.mounted();
    h.answer(vec![item(1, "NETWORK_ERROR", json!({"ts": 0}))]);
    assert_eq!(h.machine.previewing(), None);
    h.machine.changed();
    // Тот же id и тот же счётчик — не новость.
    h.answer(vec![item(1, "NETWORK_ERROR", json!({"ts": 0}))]);
    assert_eq!(h.machine.previewing(), None);
    h.machine.changed();
    h.answer(vec![item(1, "NETWORK_ERROR", json!({"ts": 0, "count": 2}))]);
    assert_eq!(h.machine.previewing().as_deref(), Some("1"));
    h.machine.changed();
    h.answer(vec![
        item(1, "NETWORK_ERROR", json!({"ts": 0, "count": 2})),
        item(5, "GAME_CRASHED", json!({})),
    ]);
    assert_eq!(h.machine.queued(), vec!["5"]);
}

#[test]
fn a_stale_or_failed_list_answer_leaves_the_shown_items() {
    let mut h = harness();
    h.machine.mounted();
    h.answer(vec![item(1, "NETWORK_ERROR", json!({}))]);
    let first = h.machine.sequence;
    h.machine.changed();
    // Ответ на прежний запрос — мимо.
    assert!(h.machine.list_reply(first, Ok(snapshot(vec![]))).is_empty());
    let current = h.machine.sequence;
    assert!(h.machine.list_reply(current, Err("x".into())).is_empty());
    assert!(h.machine.list_reply(current, Ok(Value::Null)).is_empty());
    assert_eq!(h.machine.rows(Language::En, &en()).len(), 1);
}

#[test]
fn the_preview_runs_4_8_seconds_pauses_under_the_pointer_and_resumes_with_the_rest() {
    let mut h = harness();
    h.machine.mounted();
    h.mono.set(1000.0);
    h.answer(vec![item(1, "LAUNCHER_UPDATE_AVAILABLE", json!({}))]);
    h.mono.set(2500.0);
    assert_eq!(h.machine.pause_preview(), vec![Effect::StopPreviewTimer]);
    assert_eq!(h.machine.remaining(), PREVIEW_MS - 1500.0);
    // Пауза дважды ничего не меняет.
    assert!(h.machine.pause_preview().is_empty());
    // Указатель всё ещё над превью — таймер не запускается.
    assert!(h.machine.resume_preview(true).is_empty());
    h.mono.set(9000.0);
    assert_eq!(
        h.machine.resume_preview(false),
        vec![Effect::StartPreviewTimer(PREVIEW_MS - 1500.0)]
    );
}

#[test]
fn the_preview_flows_back_into_the_bell_then_the_next_one_comes() {
    let mut h = harness();
    h.machine.mounted();
    h.answer(vec![
        item(1, "LAUNCHER_UPDATE_AVAILABLE", json!({})),
        item(2, "GAME_UPDATE_AVAILABLE", json!({})),
    ]);
    assert_eq!(h.machine.previewing().as_deref(), Some("2"));
    let effects = h.machine.dismiss_preview();
    assert_eq!(effects, vec![Effect::StopPreviewTimer]);
    let view = h.machine.view(Language::En, &en());
    assert!(view.returning && !view.peek_shown);
    // Пока карточка уходит, следующая не появляется.
    assert!(h.machine.next_preview().is_empty());
    let effects = h.machine.after_preview_leave();
    assert_eq!(h.machine.previewing().as_deref(), Some("1"));
    assert_eq!(effects, vec![Effect::StartPreviewTimer(PREVIEW_MS)]);
    assert!(!h.machine.view(Language::En, &en()).returning);
}

#[test]
fn opening_the_bell_drops_the_preview_and_the_queue_and_asks_the_list_again() {
    let mut h = harness();
    h.machine.mounted();
    h.answer(vec![
        item(1, "LAUNCHER_UPDATE_AVAILABLE", json!({})),
        item(2, "GAME_UPDATE_AVAILABLE", json!({})),
    ]);
    let opened = h.machine.show();
    assert!(opened.contains(&Effect::StopPreviewTimer));
    assert!(opened.contains(&Effect::OpenMenus(1)));
    assert!(opened.iter().any(|e| matches!(
        e,
        Effect::Invoke {
            channel: "notifications-list",
            ..
        }
    )));
    assert_eq!(h.machine.previewing(), None);
    assert!(h.machine.queued().is_empty());
    assert!(h.machine.view(Language::En, &en()).open);
}

#[test]
fn while_the_panel_is_open_new_items_get_no_preview() {
    let mut h = harness();
    h.machine.mounted();
    h.answer(vec![item(1, "NETWORK_ERROR", json!({"ts": 0}))]);
    h.machine.show();
    h.answer(vec![
        item(1, "NETWORK_ERROR", json!({"ts": 0})),
        item(7, "GAME_CRASHED", json!({})),
    ]);
    assert_eq!(h.machine.previewing(), None);
    assert!(h.machine.queued().is_empty());
    let effects = h.machine.close(true);
    assert_eq!(effects, vec![Effect::OpenMenus(-1), Effect::FocusToggle]);
    assert!(!h.machine.is_open());
}

#[test]
fn windows_on_top_send_the_preview_back_to_the_queue() {
    let mut h = harness();
    h.machine.mounted();
    h.answer(vec![item(1, "LAUNCHER_UPDATE_AVAILABLE", json!({}))]);
    let effects = h.machine.set_suspended(true);
    assert_eq!(effects, vec![Effect::StopPreviewTimer]);
    assert_eq!(h.machine.previewing(), None);
    assert_eq!(h.machine.queued(), vec!["1"]);
    // Карточка ушла с экрана, окно закрылось — превью возвращается.
    assert!(h.machine.after_preview_leave().is_empty());
    let effects = h.machine.set_suspended(false);
    assert_eq!(h.machine.previewing().as_deref(), Some("1"));
    assert_eq!(effects, vec![Effect::StartPreviewTimer(PREVIEW_MS)]);
}

#[test]
fn reading_something_already_read_asks_nothing() {
    let mut h = harness();
    h.machine.mounted();
    h.answer(vec![
        item(1, "NETWORK_ERROR", json!({"read": true})),
        item(2, "NETWORK_ERROR", json!({})),
    ]);
    assert!(h.machine.mark_read("1").is_empty());
    assert_eq!(
        h.machine.mark_read("2"),
        vec![Effect::Invoke {
            tag: Tag::Then,
            channel: "notifications-mark-read",
            args: vec![json!(2)]
        }]
    );
    assert_eq!(h.machine.mark_all_read().len(), 1);
    let cleared = h.machine.clear_all();
    assert!(matches!(
        cleared.last(),
        Some(Effect::Invoke {
            channel: "notifications-clear",
            ..
        })
    ));
}

#[test]
fn titles_follow_the_code_table_and_fall_back_to_the_generic_one() {
    let mut h = harness();
    h.machine.mounted();
    h.answer(vec![
        item(1, "STEAM_NOT_RUNNING", json!({})),
        item(2, "SOMETHING_NEW", json!({})),
        item(
            3,
            "GAME_UPDATED",
            json!({"meta": {"launcherId": "218", "newVersion": "1.1.0", "oldVersion": "1.0.9"}}),
        ),
        item(4, "GAME_INSTALLED", json!({"meta": {"launcherId": "999"}})),
    ]);
    let rows = h.machine.rows(Language::En, &en());
    assert_eq!(rows[0].title, t("#L_S_LAUNCH_STEAM_NOT_RUNNING", &[]));
    assert_eq!(rows[1].title, t("#L_N_GENERIC", &[]));
    assert_eq!(
        rows[2].title,
        t("#L_N_V_GAME_UPDATED", &["HL2 Ghosting", "1.1.0"])
    );
    assert_eq!(rows[2].detail, "1.0.9 → 1.1.0");
    let fallback = t("#L_N_V_GAME_FALLBACK", &[]);
    assert_eq!(
        rows[3].title,
        t("#L_N_V_GAME_INSTALLED", &[fallback.as_str()])
    );
    assert!(!rows[2].whats_new && !rows[2].update);
}

#[test]
fn version_codes_get_their_buttons_and_the_preview_its_own_title_and_detail() {
    let mut h = harness();
    h.machine.mounted();
    let update = item(
        1,
        "PROTON_UPDATE_AVAILABLE",
        json!({"meta": {"target": "proton", "newVersion": "wineland-10.2", "oldVersion": "wineland-10.1", "hasReleaseNotes": true}}),
    );
    h.answer(vec![update]);
    let view = h.machine.view(Language::En, &en());
    let rows = h.machine.rows(Language::En, &en());
    let row = &rows[0];
    assert!(row.whats_new && row.update);
    assert_eq!(row.title, t("#L_N_PROTON_UPDATE_AVAILABLE", &[]));
    assert_eq!(row.detail, t("#L_N_PROTON_UPDATE_TOAST", &["10.2"]));
    assert_eq!(view.peek.title, t("#L_N_UPDATE_FOR", &["Proton Wineland"]));
    assert_eq!(view.peek.detail, "10.1 → 10.2");
    assert!(view.peek_update && view.peek_shown && view.announcing);
}

#[test]
fn a_crash_lists_its_actions_and_explains_itself() {
    let mut h = harness();
    h.machine.mounted();
    h.answer(vec![
        item(
            1,
            "GAME_CRASHED",
            json!({"meta": {"hasDump": true, "gameId": "ghosting-renewed", "launcherId": "218",
                            "exitCode": -1073741819, "hint": "DEP_ALWAYS_ON"}}),
        ),
        item(2, "GAME_CRASHED", json!({"meta": {}})),
    ]);
    let rows = h.machine.rows(Language::En, &en());
    assert!(rows[0].crash && rows[0].crash_dump && !rows[1].crash_dump);
    assert_eq!(rows[0].detail, t("#L_CR_DEP_ALWAYS_ON", &[]));
    assert_eq!(rows[1].detail, t("#L_CR_TEXT_NO_DUMP", &[]));
    assert_eq!(
        h.machine.copy_code("1"),
        vec![Effect::Copy(
            "GAME_CRASHED exit=-1073741819 DEP_ALWAYS_ON".into()
        )]
    );
    assert_eq!(
        h.machine.copy_code("2"),
        vec![Effect::Copy("GAME_CRASHED exit=?".into())]
    );
    assert_eq!(
        h.machine.open_dump("1"),
        vec![Effect::Emit(Event::OpenDump(Some(
            "ghosting-renewed".into()
        )))]
    );
}

#[test]
fn details_in_the_wrong_language_are_hidden_in_english_only() {
    let mut h = harness();
    h.machine.mounted();
    h.answer(vec![
        item(1, "NETWORK_ERROR", json!({"detail": "Нет сети"})),
        item(2, "NETWORK_ERROR", json!({"detail": "ECONNRESET"})),
    ]);
    let en_rows = h.machine.rows(Language::En, &en());
    assert_eq!(en_rows[0].detail, "");
    assert_eq!(en_rows[1].detail, "ECONNRESET");
    let ru_rows = h.machine.rows(Language::Ru, &ru());
    assert_eq!(ru_rows[0].detail, "Нет сети");
}

#[test]
fn moderation_details_join_term_reason_and_hint() {
    let mut h = harness();
    h.machine.mounted();
    h.answer(vec![
        item(
            1,
            "MOD_MUTE",
            json!({"detail": "spam", "meta": {"until": "2026-10-20T10:30:00.000Z"}}),
        ),
        item(2, "MOD_BAN", json!({"meta": {"forever": true}})),
        item(3, "MOD_RESET_NICK", json!({"detail": "rule 3"})),
    ]);
    let rows = h.machine.rows(Language::En, &en());
    let until = intl::format_date_in(
        Language::En,
        DateStyle::DateTimeLong,
        1_792_492_200_000.0,
        &chrono::Utc,
    );
    assert_eq!(
        rows[0].detail,
        format!(
            "{} · {}",
            t("#L_N_MOD_UNTIL", &[until.as_str()]),
            t("#L_MOD_REASON", &["spam"])
        )
    );
    assert_eq!(rows[1].detail, t("#L_N_MOD_FOREVER", &[]));
    assert_eq!(
        rows[2].detail,
        format!(
            "{} · {}",
            t("#L_MOD_REASON", &["rule 3"]),
            t("#L_N_MOD_NICK_HINT", &[])
        )
    );
}

#[test]
fn time_is_hours_and_minutes_today_and_day_month_before() {
    let mut h = harness();
    h.machine.mounted();
    h.answer(vec![
        item(
            1,
            "NETWORK_ERROR",
            json!({"ts": NOW - 3 * 3600 * 1000 - 5 * 60 * 1000}),
        ),
        item(2, "NETWORK_ERROR", json!({"ts": NOW - 26 * 3600 * 1000})),
        item(3, "NETWORK_ERROR", json!({"count": 3})),
    ]);
    let rows = h.machine.rows(Language::En, &en());
    assert_eq!(rows[0].time, "08:55");
    assert_eq!(rows[1].time, "15.10 10:00");
    assert_eq!(rows[2].count, "×3");
    assert_eq!(rows[0].count, "");
}

#[test]
fn the_badge_shows_99_plus_and_the_label_carries_the_count() {
    let mut h = harness();
    h.machine.mounted();
    let many: Vec<Value> = (0..120)
        .map(|id| item(id, "NETWORK_ERROR", json!({"ts": 0})))
        .collect();
    h.answer(many);
    let view = h.machine.view(Language::En, &en());
    assert_eq!(view.unread, 120);
    assert_eq!(view.badge, "99+");
    assert_eq!(view.bell_label, format!("{} (120)", t("#L_N_TITLE", &[])));
    let mut quiet = harness();
    quiet.machine.mounted();
    quiet.answer(vec![]);
    let view = quiet.machine.view(Language::En, &en());
    assert_eq!(view.unread, 0);
    assert_eq!(view.bell_label, t("#L_N_TITLE", &[]));
}

#[test]
fn whats_new_and_run_update_reach_the_shell_and_close_the_panel() {
    let mut h = harness();
    h.machine.mounted();
    h.answer(vec![item(
        1,
        "LAUNCHER_UPDATE_AVAILABLE",
        json!({"meta": {"target": "launcher", "newVersion": "0.5.0", "hasReleaseNotes": true}}),
    )]);
    h.machine.show();
    let target = Target {
        target: Some("launcher".into()),
        game_id: None,
        version: Some("0.5.0".into()),
    };
    let effects = h.machine.whats_new("1");
    assert!(matches!(
        effects[0],
        Effect::Invoke {
            channel: "notifications-mark-read",
            ..
        }
    ));
    assert_eq!(effects[1], Effect::Emit(Event::WhatsNew(target.clone())));
    assert_eq!(effects[2], Effect::OpenMenus(-1));
    h.machine.show();
    let effects = h.machine.run_update("1");
    assert!(effects.contains(&Effect::Emit(Event::RunUpdate(target))));
    assert!(effects.contains(&Effect::OpenMenus(-1)));
}

#[test]
fn codes_of_0_4_9_have_titles_and_support_thanks_its_text() {
    let mut h = harness();
    h.machine.mounted();
    h.answer(vec![
        item(1, "PRESERVE_FAILED", json!({"ts": 0})),
        item(2, "RENDER_ACTIVE", json!({"ts": 0})),
        item(3, "SUPPORT_THANKS", json!({"ts": 0, "detail": "x"})),
    ]);
    let rows = h.machine.rows(Language::En, &en());
    assert_eq!(rows[0].title, t("#L_N_DELIVERY", &[]));
    assert_eq!(rows[1].title, t("#L_D_ERR_RENDER_ACTIVE", &[]));
    assert_eq!(rows[2].title, t("#L_N_SUPPORT_THANKS", &[]));
    assert_eq!(rows[2].detail, t("#L_THANKS_TEXT", &[]));
}

#[test]
fn a_proxy_error_becomes_the_proxy_hint_in_any_language() {
    let mut h = harness();
    h.machine.mounted();
    h.answer(vec![
        item(
            1,
            "NETWORK_ERROR",
            json!({"ts": 0, "detail": "net::ERR_PROXY_CONNECTION_FAILED"}),
        ),
        item(
            2,
            "NETWORK_ERROR",
            json!({"ts": 0, "detail": "Ошибка: ERR_TUNNEL_CONNECTION_FAILED"}),
        ),
        item(
            3,
            "NETWORK_ERROR",
            json!({"ts": 0, "detail": "xERR_PROXY_A ERR_PROXY_ ERR_PROXY_Ab"}),
        ),
    ]);
    let hint = if cfg!(windows) {
        "#L_N_PROXY_HINT"
    } else {
        "#L_N_PROXY_HINT_SYSTEM"
    };
    let rows = h.machine.rows(Language::En, &en());
    assert_eq!(rows[0].detail, t(hint, &[]));
    assert_eq!(rows[1].detail, t(hint, &[]));
    assert_eq!(rows[2].detail, "xERR_PROXY_A ERR_PROXY_ ERR_PROXY_Ab");
    let ru_rows = h.machine.rows(Language::Ru, &ru());
    assert_eq!(ru_rows[0].detail, crate::tr(Language::Ru, hint, &[]));
    assert!(is_proxy_error("ERR_MANDATORY_PROXY_CONFIGURATION_FAILED."));
    assert!(is_proxy_error("(ERR_PROXY_AUTH_UNSUPPORTED_2)"));
}

#[test]
fn fifty_items_unlock_hoarder_on_every_snapshot() {
    let mut h = harness();
    h.machine.mounted();
    let hoarder = Effect::Invoke {
        tag: Tag::Ignore,
        channel: "runner-secret-unlock",
        args: vec![json!("hoarder")],
    };
    let items = |n: u32| {
        (0..n)
            .map(|id| item(id, "NETWORK_ERROR", json!({"ts": 0})))
            .collect()
    };
    assert!(!h.answer(items(49)).contains(&hoarder));
    h.machine.changed();
    assert_eq!(h.answer(items(50)).first(), Some(&hoarder));
}
