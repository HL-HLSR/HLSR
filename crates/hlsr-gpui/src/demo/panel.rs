//! Страница «Демо» (`DemoPanel.vue` 0.4.9): шапка (обновить, «Добавить»), вкладки «Демки /
//! Отрендеренные», строка итога, список по играм и плеер с карточкой выбранной записи; видео папки
//! вывода; бросание файлов на страницу. Логика — [`State`]; каналы `demo-*` зовёт этот вид.
//!
//! Страница живёт, пока жива рамка оболочки (`keep-alive`): уход на другой раздел — `deactivated`
//! (предпросмотр гасится), возврат — `activated` (списки и статусы заново). Раздел узнаётся по
//! навигации оболочки после каждой перемены корня.

use super::export_modal::DemoExportModal;
use super::player::{DemoPlayer, PlayerEvent, PlayerProps};
use super::screens::DemoScreens;
use super::view_settings::ViewSettings;
use super::{BtnLook, OnClick, btn, btn_icon, count, icon_btn};
use crate::dialogs::confirm::{Answer, ConfirmCard, OnAnswer};
use crate::layer::{Modal, listen};
use crate::scale::{u, viewport};
use crate::scroll::Scroll;
use crate::shell::root::{ShellContext, ShellRoot};
use crate::text::{RUBIK, TextExt, spaced};
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Context, Entity, ExternalPaths, FocusHandle, FontWeight, Global, IntoElement,
    Render, Role, SharedString, Subscription, Task, Window, div, prelude::*,
};
use hlsr_core::language::Language;
use hlsr_ui::controllers::demo_panel::{
    Answer as Reply, Carried, DemoPanel as State, JobKind, Next, Tab,
};
use hlsr_ui::controllers::demo_tool::ClipRange;
use hlsr_ui::controllers::shell::Confirm;
use hlsr_ui::services::Services;
use serde_json::{Value, json};
use std::rc::Rc;

/// Что переходит в новую страницу при пересоздании рамки (смена языка).
#[derive(Default)]
struct CarriedState(Option<Carried>);

impl Global for CarriedState {}

#[cfg(test)]
thread_local! {
    /// Последняя созданная страница (тесты зовут методы окна экспорта).
    pub(super) static LAST: std::cell::RefCell<Option<gpui::WeakEntity<DemoPanel>>> =
        std::cell::RefCell::default();
}

/// Высота окна (CSS px), ниже которой — низкая раскладка (`@media (max-height: 760px)`).
pub const LOW_HEIGHT: f32 = 760.;

pub struct DemoPanel {
    services: Services,
    pub(super) language: Language,
    pub(super) s: State,
    pub(super) player: Entity<DemoPlayer>,
    pub(super) rendered_player: Entity<DemoPlayer>,
    screens: Entity<DemoScreens>,
    rendered_screens: Entity<DemoScreens>,
    pub(super) view_settings: Entity<ViewSettings>,
    export: Entity<DemoExportModal>,
    confirm: Modal,
    confirm_cancel: FocusHandle,
    /// Лаунчер закрывается: снятие страницы — не `beforeUnmount`.
    quitting: bool,
    pub(super) list_scroll: Scroll,
    pub(super) rendered_scroll: Scroll,
    pub(super) info_scroll: Scroll,
    pub(super) side_scroll: Scroll,
    _events: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl DemoPanel {
    pub fn new(services: &Services, cx: &mut Context<DemoPanel>) -> DemoPanel {
        let language = ShellContext::get(cx).map_or(Language::Ru, |shell| shell.shell.language());
        let carried = cx
            .try_global::<CarriedState>()
            .and_then(|carried| carried.0.clone())
            .unwrap_or_default();
        cx.set_global(CarriedState(None));
        let weak = cx.weak_entity();
        let player = cx.new(|cx| DemoPlayer::new(services, cx));
        let rendered_player = cx.new(|cx| DemoPlayer::new(services, cx));
        let screens = cx.new(|_| DemoScreens::new(weak.clone(), false));
        let rendered_screens = cx.new(|_| DemoScreens::new(weak.clone(), true));
        let view_settings = cx.new(|cx| ViewSettings::new(weak.clone(), cx));
        let export = cx.new(|cx| DemoExportModal::new(weak.clone(), services, cx));
        let events = crate::bridge::listen(cx, services.subscribe(), |this, event, cx| {
            this.on_event(&event.channel, &event.payload, cx);
        });
        let mut subscriptions = vec![
            cx.subscribe(&player, |this, _, event, cx| {
                this.on_player(event, false, cx)
            }),
            cx.subscribe(&rendered_player, |this, _, event, cx| {
                this.on_player(event, true, cx)
            }),
        ];
        if let Some(root) = ShellRoot::get(cx) {
            subscriptions.push(cx.observe(&root, |this, _, cx| this.sync_active(cx)));
        }
        subscriptions.push(cx.on_app_quit(|this, _| {
            this.quitting = true;
            async {}
        }));
        // beforeUnmount (рамку пересоздала смена языка); выход из лаунчера страницу не снимает.
        cx.on_release(|this, cx| {
            if this.s.active && !this.quitting {
                fire(&this.services, "demo-preview-clear", Vec::new(), cx);
            }
            cx.set_global(CarriedState(Some(this.s.carried())));
        })
        .detach();
        let confirm_cancel = cx.focus_handle().tab_stop(true);
        let mut confirm = Modal::new("demo-delete", cx).on_top();
        confirm.focus_initial(confirm_cancel.clone());
        let mut panel = DemoPanel {
            services: services.clone(),
            language,
            s: State::new(carried),
            player,
            rendered_player,
            screens,
            rendered_screens,
            view_settings,
            export,
            confirm,
            confirm_cancel,
            quitting: false,
            list_scroll: Scroll::new(),
            rendered_scroll: Scroll::new(),
            info_scroll: Scroll::new(),
            side_scroll: Scroll::new(),
            _events: events,
            _subscriptions: subscriptions,
        };
        #[cfg(test)]
        LAST.with(|last| *last.borrow_mut() = Some(cx.weak_entity()));
        // Страница создаётся при входе в раздел — это и первое `activated()`.
        panel.activate(cx);
        panel
    }

    pub(super) fn t(&self, key: &str, args: &[&str]) -> String {
        hlsr_ui::tr(self.language, key, args)
    }

    /// `$t` страницы для логики.
    pub(super) fn translate(&self) -> impl Fn(&str, &[&str]) -> String + use<> {
        let language = self.language;
        move |key: &str, args: &[&str]| hlsr_ui::tr(language, key, args)
    }

    /// Состояние страницы (окно экспорта читает его, как пропсы 0.4.9).
    pub fn state(&self) -> &State {
        &self.s
    }

    /// `ipcRenderer.invoke(…)` с ответом в этот вид.
    fn call(
        &self,
        channel: &str,
        args: Vec<Value>,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, Reply, &mut Context<Self>) + 'static,
    ) {
        let reply = self.services.invoke(channel, args);
        cx.spawn(async move |this, cx| {
            let result = reply.await.map_err(|_| ());
            this.update(cx, |this, cx| {
                done(this, result, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ---------- жизнь страницы ----------

    /// Раздел «Демо» открыт в навигации оболочки.
    fn shown(cx: &App) -> bool {
        ShellContext::get(cx)
            .is_some_and(|shell| shell.nav.section() == "games" && shell.nav.active_tab() == "demo")
    }

    fn sync_active(&mut self, cx: &mut Context<Self>) {
        let shown = Self::shown(cx);
        if shown && !self.s.active {
            self.activate(cx);
        } else if !shown && self.s.active {
            self.deactivate(cx);
        }
    }

    /// `activated()`: списки, статусы FFmpeg и bxt-rs, вид предпросмотра, «Отрендеренные».
    fn activate(&mut self, cx: &mut Context<Self>) {
        self.s.activate();
        self.load_list(cx);
        self.load_ffmpeg(cx);
        self.load_bxtrs(cx);
        self.call("demo-settings-get", Vec::new(), cx, |this, answer, _| {
            this.s.settings_done(&answer)
        });
        self.load_rendered(cx);
        cx.notify();
    }

    /// `deactivated()`: плееры на паузу и из полного экрана, предпросмотр гасится.
    fn deactivate(&mut self, cx: &mut Context<Self>) {
        self.s.deactivate();
        for player in [self.player.clone(), self.rendered_player.clone()] {
            player.update(cx, |player, cx| {
                player.pause(cx);
                player.exit_full(cx);
            });
        }
        self.view_settings
            .update(cx, |settings, cx| settings.close(cx));
        fire(&self.services, "demo-preview-clear", Vec::new(), cx);
        cx.notify();
    }

    fn on_event(&mut self, channel: &str, payload: &Value, cx: &mut Context<Self>) {
        if !channel.starts_with("demo-") {
            return;
        }
        let t = self.translate();
        let effect = self.s.on_event(channel, payload, &t);
        if let Some((text, icon)) = effect.toast {
            notify(&text, icon, cx);
        }
        if effect.reload_rendered {
            self.load_rendered(cx);
        }
        if effect.focus_player {
            self.player
                .update(cx, |player, cx| player.request_focus(cx));
        }
        cx.notify();
    }

    fn on_player(&mut self, event: &PlayerEvent, rendered: bool, cx: &mut Context<Self>) {
        match event {
            PlayerEvent::Mark(edge, time) if !rendered => self.s.on_mark(*edge, *time),
            PlayerEvent::Mark(..) => {}
            PlayerEvent::Error if rendered => self.s.rendered.media_error = true,
            PlayerEvent::Error => self.s.media_error = true,
            PlayerEvent::Expanded(expanded) => self.s.player_expanded = *expanded,
        }
        cx.notify();
    }

    // ---------- список ----------

    pub(super) fn refresh(&mut self, cx: &mut Context<Self>) {
        match self.s.tab {
            Tab::Rendered => self.load_rendered(cx),
            Tab::Demos => self.load_list(cx),
        }
    }

    fn load_list(&mut self, cx: &mut Context<Self>) {
        self.s.list_begin();
        self.call("demo-list", Vec::new(), cx, |this, answer, cx| {
            let t = this.translate();
            let next = this.s.list_done(&answer, &t);
            this.follow(next, cx);
        });
    }

    /// Ответ списка: выбрать запись или перечитать выбранную.
    fn follow(&mut self, next: Next, cx: &mut Context<Self>) {
        match next {
            Next::Select(id) => self.select(&id, cx),
            Next::Inspect(id) => self.inspect(id, cx),
            Next::Nothing => {}
        }
    }

    pub(super) fn select(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.s.select(id) {
            return;
        }
        self.player.update(cx, |player, cx| player.pause(cx));
        self.inspect(id.to_owned(), cx);
    }

    fn inspect(&mut self, id: String, cx: &mut Context<Self>) {
        self.s.inspect_begin();
        let args = vec![json!(id)];
        self.call("demo-inspect", args, cx, move |this, answer, _| {
            let t = this.translate();
            this.s.inspect_done(&id, &answer, &t);
        });
    }

    pub(super) fn pick_files(&mut self, cx: &mut Context<Self>) {
        self.call("demo-pick-files", Vec::new(), cx, |this, answer, cx| {
            let t = this.translate();
            if let Some(id) = this.s.pick_done(&answer, &t) {
                this.select(&id, cx);
            }
        });
    }

    /// Бросили файлы (`on_drop::<ExternalPaths>`).
    pub(super) fn drop_files(&mut self, paths: &ExternalPaths, cx: &mut Context<Self>) {
        let paths: Vec<String> = paths
            .paths()
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let t = self.translate();
        let Some((paths, extra)) = self.s.drop_paths(&paths, &t) else {
            cx.notify();
            return;
        };
        self.call(
            "demo-add-paths",
            vec![json!(paths)],
            cx,
            move |this, answer, cx| {
                let t = this.translate();
                if let Some(id) = this.s.add_paths_done(&answer, extra, &t) {
                    this.select(&id, cx);
                }
            },
        );
    }

    pub(super) fn remove_item(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.s.remove_begin() else {
            return;
        };
        self.call(
            "demo-remove",
            vec![json!(id)],
            cx,
            move |this, answer, cx| {
                let t = this.translate();
                let next = this.s.remove_done(&id, &answer, &t);
                this.follow(next, cx);
            },
        );
    }

    pub(super) fn move_segment(&mut self, index: usize, delta: isize, cx: &mut Context<Self>) {
        let Some((id, order)) = self.s.reorder_begin(index, delta) else {
            return;
        };
        self.call(
            "demo-reorder",
            vec![json!(id), json!(order)],
            cx,
            |this, answer, _| {
                let t = this.translate();
                this.s.reorder_done(&answer, &t);
            },
        );
    }

    pub(super) fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if tab == self.s.tab {
            return;
        }
        // Видео другой вкладки не играет за кадром.
        let leaving = match self.s.tab {
            Tab::Demos => &self.player,
            Tab::Rendered => &self.rendered_player,
        };
        leaving.update(cx, |player, cx| player.pause(cx));
        if self.s.set_tab(tab) {
            self.load_rendered(cx);
        }
    }

    pub(super) fn toggle_card(&mut self) {
        self.s.toggle_card();
    }

    // ---------- плеер, предпросмотр, экспорт ----------

    /// Переход по метке карточки: время демо.
    pub(super) fn jump_to(&mut self, demo_time: f64, cx: &mut Context<Self>) {
        self.player
            .update(cx, |player, cx| player.seek_demo(demo_time, cx));
    }

    /// Переход по метке видео «Отрендеренных»: время клипа.
    pub(super) fn jump_rendered(&mut self, t: f64, cx: &mut Context<Self>) {
        self.rendered_player
            .update(cx, |player, cx| player.seek(t, cx));
    }

    /// `startPreview(force)`: долгий предпросмотр (оценка выше порога) — сначала окно экспорта.
    pub fn start_preview(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(item_id) = self.s.preview_check() else {
            return;
        };
        if force {
            self.preview_go(item_id, cx);
            return;
        }
        let args = self.s.estimate_args(&item_id);
        self.call("demo-estimate", args, cx, move |this, answer, cx| {
            if this.s.estimate_done(&item_id, &answer) {
                this.preview_go(item_id, cx);
            }
        });
    }

    fn preview_go(&mut self, item_id: String, cx: &mut Context<Self>) {
        let args = self.s.preview_begin(&item_id);
        self.call("demo-preview-start", args, cx, move |this, answer, _| {
            this.s.job_started(JobKind::Preview, &item_id, &answer);
        });
    }

    /// `startExport(settings)` (окно экспорта, G13).
    pub fn start_export(&mut self, settings: Value, cx: &mut Context<Self>) {
        let Some(args) = self.s.export_begin(settings) else {
            return;
        };
        let item_id = args[0].as_str().unwrap_or_default().to_owned();
        self.call("demo-export-start", args, cx, move |this, answer, _| {
            this.s.job_started(JobKind::Export, &item_id, &answer);
        });
        cx.notify();
    }

    /// `cancelJob`: без jobId (ответ на старт ещё не пришёл) главный процесс гасит текущее.
    pub fn cancel_job(&mut self, cx: &mut Context<Self>) {
        if let Some(args) = self.s.cancel_args() {
            fire(&self.services, "demo-cancel", args, cx);
        }
    }

    /// `openExport`.
    pub fn open_export(&mut self, cx: &mut Context<Self>) {
        self.s.set_export_open(true);
        cx.notify();
    }

    /// Окно экспорта закрыто (`@close`).
    pub fn close_export(&mut self, cx: &mut Context<Self>) {
        self.s.set_export_open(false);
        cx.notify();
    }

    /// «Всё равно предпросмотр» (`@preview-anyway`).
    pub fn preview_anyway(&mut self, cx: &mut Context<Self>) {
        self.s.preview_anyway();
        self.start_preview(true, cx);
        cx.notify();
    }

    /// Интервал из окна экспорта (`@range`).
    pub fn set_range(&mut self, range: Option<ClipRange>, cx: &mut Context<Self>) {
        self.s.range = range;
        cx.notify();
    }

    /// «Открыть» итог экспорта (`@open-result`).
    pub fn open_result(&mut self, result: Value, cx: &mut Context<Self>) {
        if let Some(id) = self.s.open_result(result) {
            self.select(&id, cx);
        }
        cx.notify();
    }

    /// «Ещё раз» (`@again`): итог экспорта забыт.
    pub fn export_again(&mut self, cx: &mut Context<Self>) {
        self.s.export_outcome = None;
        cx.notify();
    }

    pub(super) fn close_media(&mut self, cx: &mut Context<Self>) {
        self.player.update(cx, |player, cx| player.pause(cx));
        self.s.close_media();
    }

    /// Вид предпросмотра из «Дополнительных настроек».
    pub(super) fn set_view(&mut self, key: &str, on: bool, cx: &mut Context<Self>) {
        if let Some(view) = self.s.preview_view.as_object_mut() {
            view.insert(key.to_owned(), json!(on));
        }
        cx.notify();
    }

    // ---------- FFmpeg и bxt-rs ----------

    fn load_ffmpeg(&mut self, cx: &mut Context<Self>) {
        self.call("demo-ffmpeg-status", Vec::new(), cx, |this, answer, _| {
            this.s.ffmpeg_status_done(&answer)
        });
    }

    fn load_bxtrs(&mut self, cx: &mut Context<Self>) {
        self.call("demo-bxtrs-status", Vec::new(), cx, |this, answer, _| {
            this.s.bxtrs_status_done(&answer)
        });
    }

    /// «Скачать» FFmpeg или bxt-rs (`installFfmpeg`, `installBxtrs`).
    pub fn install_tool(&mut self, bxtrs: bool, cx: &mut Context<Self>) {
        if !self.s.install_begin(bxtrs) {
            return;
        }
        let channel = if bxtrs {
            "demo-bxtrs-install"
        } else {
            "demo-ffmpeg-install"
        };
        self.call(channel, Vec::new(), cx, move |this, answer, cx| {
            if this.s.install_done(bxtrs, &answer) {
                if bxtrs {
                    this.load_bxtrs(cx);
                } else {
                    this.load_ffmpeg(cx);
                }
            }
            // Готовность рендера HL1 зависит от bxt-rs: обновить выбранную запись.
            if bxtrs && let Some(id) = this.s.selected_id.clone() {
                this.inspect(id, cx);
            }
        });
        cx.notify();
    }

    /// «Отмена» загрузки FFmpeg или bxt-rs.
    pub fn cancel_tool(&mut self, bxtrs: bool, cx: &mut Context<Self>) {
        let channel = if bxtrs {
            "demo-bxtrs-cancel"
        } else {
            "demo-ffmpeg-cancel"
        };
        fire(&self.services, channel, Vec::new(), cx);
    }

    /// Ссылка карточки bxt-rs по ключу (`open-external-link`).
    pub(super) fn open_link(&self, key: String, cx: &mut App) {
        fire(&self.services, "open-external-link", vec![json!(key)], cx);
    }

    // ---------- отрендеренные ----------

    fn load_rendered(&mut self, cx: &mut Context<Self>) {
        self.s.rendered_list_begin();
        self.call("demo-rendered-list", Vec::new(), cx, |this, answer, _| {
            this.s.rendered_list_done(&answer)
        });
    }

    pub(super) fn open_rendered(&mut self, name: String, cx: &mut Context<Self>) {
        if !self.s.open_rendered_begin(&name) {
            return;
        }
        self.rendered_player
            .update(cx, |player, cx| player.pause(cx));
        self.call(
            "demo-rendered-open",
            vec![json!(name)],
            cx,
            |this, answer, cx| {
                let t = this.translate();
                if this.s.open_rendered_done(&answer, &t) {
                    this.load_rendered(cx);
                }
            },
        );
    }

    pub(super) fn reveal_rendered(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.s.open_name() else {
            return;
        };
        self.call(
            "demo-rendered-reveal",
            vec![json!(name)],
            cx,
            |this, answer, cx| {
                let t = this.translate();
                if this.s.reveal_done(&answer, &t) {
                    this.load_rendered(cx);
                }
            },
        );
    }

    /// «Экспорт» готового видео: окно экспорта его исходной демки с тем же отрезком и скоростью.
    pub(super) fn export_from_rendered(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.s.export_from_rendered_begin() else {
            return;
        };
        self.rendered_player
            .update(cx, |player, cx| player.pause(cx));
        if self.s.select(&source) {
            self.player.update(cx, |player, cx| player.pause(cx));
            self.s.inspect_begin();
            self.call(
                "demo-inspect",
                vec![json!(source)],
                cx,
                move |this, answer, _| {
                    let t = this.translate();
                    this.s.inspect_done(&source, &answer, &t);
                    this.s.export_from_rendered_finish(&source);
                },
            );
        } else {
            self.s.export_from_rendered_finish(&source);
        }
    }

    pub(super) fn ask_delete(&mut self) {
        self.s.ask_delete();
    }

    fn delete_resolved(&mut self, accepted: bool, cx: &mut Context<Self>) {
        let Some(name) = self.s.delete_resolved(accepted) else {
            cx.notify();
            return;
        };
        // Плееры отпускают файл до Корзины: иначе Windows держит его открытым.
        for player in [self.player.clone(), self.rendered_player.clone()] {
            player.update(cx, |player, cx| player.pause(cx));
        }
        let args = vec![json!(name)];
        self.call("demo-rendered-delete", args, cx, move |this, answer, cx| {
            let t = this.translate();
            let (toast, reload) = this.s.delete_done(&name, &answer, &t);
            if let Some((text, icon)) = toast {
                notify(&text, icon, cx);
            }
            if reload {
                this.load_rendered(cx);
            }
        });
    }

    // ---------- вид ----------

    /// Обработчик щелчка, который зовёт метод этой страницы.
    pub(super) fn act(
        &self,
        cx: &Context<Self>,
        f: impl Fn(&mut DemoPanel, &mut Window, &mut Context<DemoPanel>) + 'static,
    ) -> OnClick {
        super::act(&cx.weak_entity(), f)
    }

    fn head(&self, low: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let weak = cx.weak_entity();
        let busy = self.s.list_busy();
        let refresh = icon_btn(
            "dmp-refresh",
            if busy {
                "fas fa-sync-alt fa-spin"
            } else {
                "fas fa-sync-alt"
            },
            40.,
            self.t("#L_DEMO_REFRESH", &[]),
            busy,
            false,
            window,
            cx,
            super::act(&weak, |this, _, cx| this.refresh(cx)),
        );
        let add = (self.s.tab == Tab::Demos).then(|| {
            btn(
                "dmp-add",
                BtnLook::PLAIN,
                false,
                None,
                vec![
                    btn_icon("fas fa-plus", theme.color("l-text")),
                    self.t("#L_DEMO_ADD", &[]).into_any_element(),
                ],
                window,
                cx,
                super::act(&weak, |this, _, cx| this.pick_files(cx)),
            )
        });
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap_x(u(20.))
            .gap_y(u(12.))
            .mb(u(if low { 12. } else { 16. }))
            .child(spaced(
                self.t("#L_DEMO_TITLE", &[]).to_uppercase(),
                32.,
                40.,
                FontWeight::BOLD,
                0.08,
                gpui::white(),
            ))
            .child(
                div()
                    .mt(u(4.))
                    .text_px(13.5, 20.)
                    .text_color(theme.color("l-text-dim"))
                    .child(self.t("#L_DEMO_LEAD", &[])),
            )
            .child(div().flex_1())
            .child(crate::snap::target("dmp-refresh", refresh))
            .children(add.map(|add| crate::snap::target("dmp-add", add)))
            .into_any_element()
    }

    fn tabs(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let weak = cx.weak_entity();
        let mut tab = |id: Tab, key: &str, name: &'static str| {
            let active = self.s.tab == id;
            let label = self.t(key, &[]);
            let (dim, text, accent) = (
                theme.color("l-text-dim"),
                theme.color("l-text"),
                theme.color("accent"),
            );
            let rendered = (id == Tab::Rendered && !self.s.rendered.items.is_empty())
                .then(|| count(self.s.rendered.items.len(), &theme));
            let on_click = super::act(&weak, move |this, _, cx| this.set_tab(id, cx));
            let inner = SharedString::from(format!("{name}-tab"));
            let super::Focus {
                handle,
                ring: focus,
            } = super::focus(&inner.clone().into(), 0., -2., window, cx);
            let el = crate::motion::hover(name, crate::motion::Timing::fast(), move |t| {
                div()
                    .id(inner)
                    .role(Role::Tab)
                    .button_text()
                    .relative()
                    .track_focus(&handle)
                    .children(focus)
                    .flex()
                    .items_center()
                    .gap(u(8.))
                    .h(u(36.))
                    .px(u(2.))
                    .cursor_pointer()
                    .text_px(13.5, 20.)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if active {
                        text
                    } else {
                        crate::motion::mix(dim, text, t)
                    })
                    .child(label)
                    .children(rendered)
                    .when(active, |el| {
                        el.child(
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .bottom(u(-1.))
                                .h(u(2.))
                                .rounded_t(u(2.))
                                .bg(accent),
                        )
                    })
                    .on_click(move |event, window, cx| on_click(event, window, cx))
            });
            crate::snap::target(name, el)
        };
        div()
            .flex_none()
            .flex()
            .gap(u(24.))
            .mt(u(-4.))
            .mb(u(14.))
            .border_b_1()
            .border_color(theme.color("l-border"))
            .child(tab(Tab::Demos, "#L_DEMO_TAB_DEMOS", "dmp-tab-demos"))
            .child(tab(
                Tab::Rendered,
                "#L_DEMO_TAB_RENDERED",
                "dmp-tab-rendered",
            ))
            .into_any_element()
    }

    fn notice(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let notice = self.s.notice.as_ref()?;
        let theme = cx.global::<Theme>().clone();
        let weak = cx.weak_entity();
        let zoom = crate::scale::zoom(window);
        let color = if notice.error {
            theme.color("l-level-error")
        } else {
            theme.color("l-text-dim")
        };
        let mut text = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(u(2.))
            .pt(u(4.));
        if !notice.text.is_empty() {
            text = text.child(div().child(notice.text.clone()));
        }
        if !notice.rejected.is_empty() {
            text =
                text.child(div().child(self.t("#L_DEMO_REJECTED", &[])))
                    .child(div().flex().flex_col().pl(u(18.)).children(
                        notice.rejected.iter().map(|(name, reason)| {
                            div()
                                .relative()
                                .text_color(theme.color("l-text-dim"))
                                .child(div().absolute().left(u(-12.)).child("•"))
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .child(
                                            div()
                                                .text_color(theme.color("l-text"))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child(name.clone()),
                                        )
                                        .child(format!(": {reason}")),
                                )
                        }),
                    ));
        }
        let super::Focus {
            handle,
            ring: focus,
        } = super::focus(&"dmp-notice-close-btn".into(), 6., 0., window, cx);
        let close = crate::motion::hover("dmp-notice-close", crate::motion::Timing::fast(), {
            let (dim, bright) = (theme.color("l-text-dim"), theme.color("l-text"));
            let label = self.t("#L_CLOSE", &[]);
            let on_click = super::act(&weak, |this, _, _| this.s.notice = None);
            move |t| {
                div()
                    .id("dmp-notice-close-btn")
                    .role(Role::Button)
                    .button_text()
                    .relative()
                    .track_focus(&handle)
                    .children(focus)
                    .aria_label(SharedString::from(label))
                    .flex_none()
                    .size(u(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(u(6.))
                    .bg(crate::kit::button::white(0.08 * t))
                    .cursor_pointer()
                    .child(
                        crate::icons::fa_glyph("fas fa-times", 13.)
                            .text_color(crate::motion::mix(dim, bright, t)),
                    )
                    .on_click(move |event, window, cx| on_click(event, window, cx))
            }
        });
        Some(
            div()
                .flex_none()
                .flex()
                .items_start()
                .gap(u(12.))
                .mb(u(12.))
                .pt(u(8.))
                .pr(u(8.))
                .pb(u(8.))
                .pl(u(12.))
                .rounded(theme.rem("l-radius-sm"))
                .bg(theme.color("l-note-bg"))
                .shadow(vec![crate::paint::ring(zoom, 1., theme.color("l-border"))])
                .text_color(color)
                .text_px(13., 13. * 1.45)
                .child(text)
                .child(crate::snap::target("dmp-notice-close", close))
                .into_any_element(),
        )
    }

    /// Окно подтверждения удаления видео (ConfirmModal 0.4.9).
    fn confirm_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let pending = self.s.rendered.confirm.clone();
        if pending.is_some() != self.confirm.is_open() {
            cx.defer_in(window, |this, window, cx| {
                if this.s.rendered.confirm.is_some() {
                    this.confirm.open(window, cx);
                } else {
                    this.confirm.close(window, cx);
                }
                cx.notify();
            });
        }
        let card = pending.map(|name| {
            let request = Confirm {
                title: self.t("#L_DEMO_RENDERED_DELETE_TITLE", &[]),
                text: self.t("#L_DEMO_RENDERED_DELETE_TEXT", &[]),
                items: State::delete_items(&name),
                confirm_label: Some(self.t("#L_DEMO_RENDERED_DELETE_OK", &[])),
                danger: true,
                ..Confirm::default()
            };
            let weak = cx.weak_entity();
            let on_answer: OnAnswer = Rc::new(move |answer, _, cx| {
                weak.update(cx, |this, cx| {
                    this.delete_resolved(answer == Answer::Accept, cx)
                })
                .ok();
            });
            ConfirmCard::new(request, "")
                .render(&self.confirm_cancel, on_answer, window, cx)
                .into_any_element()
        });
        let on_close = listen(cx, |this: &mut DemoPanel, _, cx| {
            this.delete_resolved(false, cx)
        });
        self.confirm
            .render(window, cx, on_close, move || {
                card.unwrap_or_else(|| div().into_any_element())
            })
            .into_any_element()
    }
}

/// Без ответа (`ipcRenderer.invoke(…).catch(() => {})`).
fn fire(services: &Services, channel: &str, args: Vec<Value>, cx: &App) {
    let reply = services.invoke(channel, args);
    cx.background_spawn(async move {
        let _ = reply.await;
    })
    .detach();
}

/// `$emit("notify", { text, icon })` — тост оболочки.
fn notify(text: &str, icon: &str, cx: &App) {
    if let Some(shell) = ShellContext::get(cx) {
        shell.shell.notify(text, icon);
    }
}

impl Render for DemoPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::live::rendered(cx.entity_id());
        let theme = cx.global::<Theme>().clone();
        let low = viewport(window).1 <= LOW_HEIGHT;
        // Пропсы плееров (`:src`, `:video`, `:markers`, `:duration`, `:range`) и их экраны.
        let input = self.s.player();
        let screen = self.s.screen();
        let demos = PlayerProps {
            src: input.src,
            video: input.video,
            markers: input.markers,
            duration: self.s.duration(),
            range: self.s.range,
            marking: true,
            overlay: screen.map(|_| self.screens.clone().into()),
        };
        self.player.update(cx, |player, _| player.set_props(demos));
        let open = self.s.rendered.open.clone();
        let field = |name: &str| open.as_ref().and_then(|open| open.get(name)).cloned();
        let rendered = PlayerProps {
            src: field("url").and_then(|url| url.as_str().map(str::to_owned)),
            video: field("video"),
            markers: field("markers")
                .and_then(|markers| markers.as_array().cloned())
                .unwrap_or_default(),
            duration: 0.,
            range: None,
            marking: false,
            overlay: (open.is_none() || self.s.rendered.media_error)
                .then(|| self.rendered_screens.clone().into()),
        };
        self.rendered_player
            .update(cx, |player, _| player.set_props(rendered));

        let head = self.head(low, window, cx);
        let tabs = self.tabs(window, cx);
        let notice = self.notice(window, cx);
        let body = match self.s.tab {
            Tab::Demos => self.demos_body(low, window, cx),
            Tab::Rendered => self.rendered_body(low, window, cx),
        };
        let dragging = self.s.dragging && cx.has_active_drag();
        let drop = super::drop::overlay(dragging, self, &theme);
        let confirm = self.confirm_modal(window, cx);
        let export = self.export.clone();
        div()
            .id("dmp")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .font_family(RUBIK)
            .text_size(u(14.))
            // body { line-height: 20px } (base.css 0.4.9) — у текста без своей высоты строки.
            .line_height(u(20.))
            .text_color(theme.color("l-text"))
            .pt(u(theme.len("l-topbar-h") + 24.))
            .pr(u(theme.len("l-edge-pad")))
            .pb(u(theme.len("l-edge-pad")))
            .pl(u(theme.len("l-hero-pad")))
            .on_drag_move::<ExternalPaths>(cx.listener(super::drop::on_drag_move))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.drop_files(paths, cx)))
            .child(head)
            .child(tabs)
            .children(notice)
            .child(body)
            .child(drop)
            .child(export)
            .child(confirm)
    }
}
