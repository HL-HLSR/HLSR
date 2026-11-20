//! Страница «Игроки» (`HomePlayers.vue` 0.4.9): «В сети» со счётчиком и сводкой (опрос раз в
//! 30 с, пока вкладка открыта, вошёл и окно не спрятано) и «Рейтинг» (сортировки, фильтр игры у
//! часов и запусков, «Показать ещё»). Строка — место (кроме «Новых»), аватар в рамке, ник со
//! значком подписчика, присутствие, значение; щелчок открывает профиль. Гостю — приглашение
//! войти. Логика — [`PlayersState`]; вкладку, вход и «Обновить» отдаёт каркас ([`HomePlayers::sync`]).
//! Строятся только строки у видимой области ([`crate::rows`]).

use super::panel::{HomePanel, SCOPE, open_profile};
use super::records::Chip;
use crate::common::{avatar_frame, nick_name};
use crate::icons::fa_glyph;
use crate::image::{Pictures, device_size};
use crate::kit::button::{focus_handle, focus_visible, white};
use crate::layer::TipExt;
use crate::motion::{AnimGate, Timing, hover, mix};
use crate::paint::{ring, shadow};
use crate::rows::{Rows, Slot};
use crate::scale::{u, zoom};
use crate::scroll::{Bar, Scroll, ScrollArea};
use crate::text::TextExt;
use crate::theme::Theme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, ClickEvent, Context, ElementId, FontWeight, IntoElement, ObjectFit, Render,
    RenderImage, Role, Subscription, Task, Window, div, img, prelude::*, px, rgb,
};
use hlsr_ui::controllers::home_players::{
    Effect, Mode, ONLINE_REFRESH, PlayersState, PlayersView, Row, Tag,
};
use hlsr_ui::images::{Fit, ImageRequest, ImageSource};
use hlsr_ui::services::{Reply, Services};
use std::sync::Arc;

/// `#4ad07a`: в игре, в сети.
const GREEN: u32 = 0x4ad07a;

/// Страница вкладки в каркасе — вид игроков (`<HomePlayers>` живёт с разделом).
pub(super) fn page(
    panel: &mut HomePanel,
    _: &mut Window,
    _: &mut Context<HomePanel>,
) -> AnyElement {
    panel.players.clone().into_any_element()
}

pub struct HomePlayers {
    services: Services,
    state: PlayersState,
    pictures: Pictures,
    scroll: Scroll,
    /// Окно видимости строк.
    rows: Rows,
    /// `resetScroll()` до следующей отрисовки (нужно окно).
    reset: bool,
    /// Опрос «В сети» (`setInterval`): задача живёт, пока опрос включён.
    poll: Option<Task<()>>,
    /// `document.hidden`: окно спрятано (`AnimGate`).
    hidden: bool,
    _gate: Subscription,
}

impl HomePlayers {
    pub fn new(services: &Services, pictures: Pictures, cx: &mut Context<Self>) -> HomePlayers {
        let gate = cx.observe_global::<AnimGate>(|this: &mut Self, cx| {
            let hidden = AnimGate::get(cx).hidden;
            if hidden != this.hidden {
                this.hidden = hidden;
                let effects = this.state.set_hidden(hidden);
                this.run(effects, cx);
            }
        });
        let mut players = HomePlayers {
            services: services.clone(),
            state: PlayersState::new(),
            pictures,
            scroll: Scroll::new(),
            rows: Rows::default(),
            reset: false,
            poll: None,
            hidden: AnimGate::get(cx).hidden,
            _gate: gate,
        };
        let mut effects = players.state.set_hidden(players.hidden);
        effects.extend(players.state.mounted());
        players.run(effects, cx);
        players
    }

    /// `active`, `signedIn` и `me` из каркаса: вкладка открыта, вход, SteamID.
    pub fn sync(&mut self, active: bool, signed_in: bool, me: &str, cx: &mut Context<Self>) {
        let mut effects = self.state.set_signed_in(signed_in, me);
        effects.extend(self.state.set_active(active));
        self.run(effects, cx);
    }

    /// «Обновить» шапки на этой вкладке.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let effects = self.state.refresh(true);
        self.run(effects, cx);
    }

    fn run(&mut self, effects: Vec<Effect>, cx: &mut Context<Self>) {
        for effect in effects {
            match effect {
                Effect::Invoke { tag, channel, args } => {
                    let reply = self.services.invoke(channel, args);
                    cx.spawn(async move |this, cx| {
                        let reply = reply.await;
                        this.update(cx, |this, cx| this.answer(tag, reply, cx)).ok();
                    })
                    .detach();
                }
                // `clearInterval` + `setInterval`: включённый опрос начинается заново.
                Effect::Poll(on) => {
                    self.poll = on.then(|| {
                        cx.spawn(async move |this, cx| {
                            loop {
                                cx.background_executor().timer(ONLINE_REFRESH).await;
                                let polled = this.update(cx, |this, cx| {
                                    let effects = this.state.poll();
                                    this.run(effects, cx);
                                });
                                if polled.is_err() {
                                    break;
                                }
                            }
                        })
                    });
                }
                Effect::OpenProfile(id) => open_profile(&id, cx),
                Effect::ResetScroll => self.reset = true,
            }
        }
        cx.notify();
    }

    fn answer(&mut self, tag: Tag, reply: Reply, cx: &mut Context<Self>) {
        match tag {
            Tag::Online(request) => self.state.online_reply(request, reply),
            Tag::Top(request) => self.state.top_reply(request, reply),
        }
        cx.notify();
    }

    fn head(&self, view: &PlayersView, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let group = |chips: Vec<AnyElement>| div().flex().flex_wrap().gap(u(6.)).children(chips);
        let modes: Vec<AnyElement> = view
            .modes
            .iter()
            .map(|chip| {
                let mode = if chip.id == "top" {
                    Mode::Top
                } else {
                    Mode::Online
                };
                let mut content = vec![chip.label.clone().into_any_element()];
                if !chip.count.is_empty() {
                    // `.players__chip-count`: 18 px, поля 5, радиус 9, чёрный .22, 11/18.
                    content.push(
                        div()
                            .min_w(u(18.))
                            .px(u(5.))
                            .rounded(u(9.))
                            .bg(gpui::black().opacity(0.22))
                            .text_px(11., 18.)
                            .text_center()
                            .child(chip.count.clone())
                            .into_any_element(),
                    );
                }
                Chip {
                    name: format!("players-mode-{}", chip.id),
                    content,
                    active: chip.active,
                    title: None,
                    inset_focus: true,
                }
                .render(
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let effects = this.state.set_mode(mode);
                        this.run(effects, cx);
                    }),
                    window,
                    cx,
                )
            })
            .collect();
        let sorts: Vec<AnyElement> = view
            .sorts
            .iter()
            .map(|chip| {
                let sort = chip.id.clone();
                Chip {
                    name: format!("players-sort-{}", chip.id),
                    content: vec![
                        fa_glyph(&chip.icon, 11.).into_any_element(),
                        chip.label.clone().into_any_element(),
                    ],
                    active: chip.active,
                    title: None,
                    inset_focus: true,
                }
                .render(
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let effects = this.state.set_sort(&sort);
                        this.run(effects, cx);
                    }),
                    window,
                    cx,
                )
            })
            .collect();
        let games: Vec<AnyElement> = view
            .games
            .iter()
            .map(|chip| {
                let game = chip.id.clone();
                let name = if chip.id.is_empty() { "all" } else { &chip.id };
                Chip {
                    name: format!("players-game-{name}"),
                    content: vec![chip.label.clone().into_any_element()],
                    active: chip.active,
                    title: None,
                    inset_focus: true,
                }
                .render(
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let effects = this.state.set_game(&game);
                        this.run(effects, cx);
                    }),
                    window,
                    cx,
                )
            })
            .collect();
        let summary = (!view.summary.is_empty()).then(|| {
            div()
                .text_px(12., 16.)
                .text_color(theme.color("l-text-faint"))
                .child(view.summary.clone())
        });
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(u(10.))
            .child(group(modes))
            .when(!sorts.is_empty(), |el| el.child(group(sorts)))
            .when(!games.is_empty(), |el| el.child(group(games)))
            .children(summary)
            .into_any_element()
    }

    /// `.players__more`: отступ сверху 12, 32 px, поля 16, `--l-radius-sm`, белый .06 (наведение
    /// .1), 13/600; `disabled` — `--l-text-faint`.
    fn more(
        &self,
        name: &str,
        label: String,
        disabled: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>();
        let (text, faint, accent) = (
            theme.color("l-text"),
            theme.color("l-text-faint"),
            theme.color("l-accent"),
        );
        let radius = theme.len("l-radius-sm");
        let zoom = zoom(window);
        let id = ElementId::Name(name.to_owned().into());
        let focus = focus_handle(&id, window, cx);
        let ring_on = focus_visible(&focus, window);
        let inner = id.clone();
        let button = hover(id, Timing::fast(), move |t| {
            let t = if disabled { 0. } else { t };
            div()
                .id(inner)
                .role(Role::Button)
                .button_text()
                .track_focus(&focus)
                .flex()
                .flex_none()
                .items_center()
                .mt(u(12.))
                .h(u(32.))
                .px(u(16.))
                .rounded(u(radius))
                .bg(white(0.06 + 0.04 * t))
                .text_px(13., 32.)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if disabled { faint } else { text })
                .when(ring_on, |el| el.shadow(vec![ring(zoom, 2., accent)]))
                .when(!disabled, |el| el.cursor_pointer().on_click(on_click))
                .child(label)
        });
        crate::snap::target(name.to_owned(), button)
    }

    /// Аватар строки 36 × 36 (готов — сразу, иначе загрузка идёт и вид перерисуется).
    fn photo(
        &self,
        avatar: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Arc<RenderImage>> {
        let source = ImageSource::parse(avatar)?;
        let (w, h) = device_size(window, 36., 36.);
        let request = ImageRequest::new(source, SCOPE).with_target(w, h, Fit::Cover);
        self.pictures.get(request, window, cx).and_then(Result::ok)
    }

    /// `.player`: строка-кнопка 10/16, `--l-radius`, `--l-surface-solid` и кольцо; свой —
    /// кольцо акцентом; наведение — `--l-border-strong` и белый .03 поверх.
    fn row(
        &mut self,
        index: usize,
        row: &Row,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let zoom = zoom(window);
        let (solid, border, strong, accent) = (
            theme.color("l-surface-solid"),
            theme.color("l-border"),
            theme.color("l-border-strong"),
            theme.color("l-accent"),
        );
        let (text, dim, faint) = (
            theme.color("l-text"),
            theme.color("l-text-dim"),
            theme.color("l-text-faint"),
        );
        let small = theme.len("l-radius-sm");
        let id = ElementId::Name(format!("player-{}", row.id).into());
        let focus = focus_handle(&id, window, cx);
        if focus.is_focused(window) {
            self.rows.focus(index);
        }
        let ring_on = focus_visible(&focus, window);
        let label = HomePanel::tr(cx)("#L_HOME_AUTHOR_OPEN", &[]);

        let rank = (row.rank > 0).then(|| {
            let color = match row.rank {
                1 => rgb(0xffd166).into(),
                2 => rgb(0xcfd8e3).into(),
                3 => rgb(0xe0a370).into(),
                _ => faint,
            };
            div()
                .flex_none()
                .w(u(28.))
                .text_center()
                .text_px(14., 20.)
                .font_weight(FontWeight::BOLD)
                .tnum()
                .text_color(color)
                .child(row.rank.to_string())
        });
        let photo = self.photo(&row.avatar, window, cx);
        // Буква под картинкой: видна, пока аватар грузится или если он битый.
        let avatar = div()
            .relative()
            .size(u(36.))
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .rounded(u(small))
            .bg(white(0.08))
            .text_px(15., 15.)
            .font_weight(FontWeight::BOLD)
            .text_color(dim)
            .child(row.letter.clone())
            .children(photo.map(|photo| {
                img(photo)
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .rounded(u(small))
                    .object_fit(ObjectFit::Cover)
            }));
        let framed = avatar_frame(ElementId::named_usize("player-frame", index), 36., avatar)
            .css(row.frame_css.clone())
            .width(3.)
            .radius(small)
            .hole(solid);
        let nick = nick_name(
            ElementId::named_usize("player-nick", index),
            row.name.clone(),
        )
        .look(row.nick_style.clone(), row.nick_css.clone())
        .subscriber(row.subscriber, None);
        let green = rgb(GREEN).into();
        let (status_color, dot) = match row.presence {
            "playing" => (green, green),
            "online" => (dim, green),
            _ => (faint, rgb(0x8b939e).into()),
        };
        let playing = row.presence == "playing";
        let status = div()
            .flex()
            .items_center()
            .gap(u(6.))
            .min_w_0()
            .text_px(12., 16.)
            .whitespace_nowrap()
            .text_color(status_color)
            .child(
                div()
                    .flex_none()
                    .size(u(7.))
                    .rounded_full()
                    .bg(dot)
                    // `round-dot-glow(4px, rgba(74, 208, 122, .7))`.
                    .when(playing, |el| {
                        el.shadow(vec![shadow(
                            zoom,
                            0.,
                            0.,
                            4.,
                            0.,
                            gpui::Hsla::from(rgb(GREEN)).opacity(0.7),
                        )])
                    }),
            )
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(row.presence_text.clone()),
            );
        let names = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(u(2.))
            // `.player__name`: поля 4/6 под свечение значка съедены отступом −4/−6.
            .child(
                div()
                    .mx(u(-6.))
                    .my(u(-4.))
                    .px(u(6.))
                    .py(u(4.))
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_px(14., 20.)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(text)
                    .child(nick),
            )
            .child(status);
        let value = (!row.value.is_empty()).then(|| {
            div()
                .flex_none()
                .flex()
                .flex_col()
                .items_end()
                .gap(u(2.))
                .child(
                    div()
                        .text_px(15., 20.)
                        .font_weight(FontWeight::BOLD)
                        .tnum()
                        .text_color(text)
                        .child(row.value.clone()),
                )
                .when(!row.unit.is_empty(), |el| {
                    el.child(
                        div()
                            .text_px(11., 14.)
                            .text_color(faint)
                            .child(row.unit.clone()),
                    )
                })
        });
        let me = row.me;
        let steam_id = row.id.clone();
        let on_click = cx.listener(move |this, _: &ClickEvent, _, cx| {
            let effects = this.state.open_profile(&steam_id);
            this.run(effects, cx);
        });
        let inner = id.clone();
        let button = hover(id, Timing::fast(), move |t| {
            let edge = if ring_on {
                ring(zoom, 2., accent)
            } else if me {
                ring(zoom, 1., accent)
            } else {
                ring(zoom, 1., mix(border, strong, t))
            };
            div()
                .id(inner)
                .role(Role::Button)
                .button_text()
                .aria_label(label.clone())
                .tip(label)
                .track_focus(&focus)
                .flex()
                .items_center()
                .gap(u(12.))
                .w_full()
                .px(u(16.))
                .py(u(10.))
                .rounded(theme.rem("l-radius"))
                .bg(mix(solid, gpui::white(), 0.03 * t))
                .shadow(vec![edge])
                .cursor_pointer()
                .on_click(on_click)
                .children(rank)
                .child(framed)
                .child(names)
                .children(value)
        });
        crate::snap::target(format!("player-{index}"), button)
    }
}

impl Render for HomePlayers {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.reset) {
            self.scroll.scroll_to(px(0.), false, window, cx);
        }
        let view = self.state.view(HomePanel::language(cx), &HomePanel::tr(cx));
        let theme = cx.global::<Theme>().clone();
        let faint = theme.color("l-text-faint");
        let head = self.head(&view, window, cx);

        let mut body: Vec<AnyElement> = Vec::new();
        if !view.message.is_empty() {
            let sign_in = view.sign_in_button.then(|| {
                let label = HomePanel::tr(cx)("#L_HOME_SIGN_IN", &[]);
                self.more(
                    "players-sign-in",
                    label,
                    false,
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        let effects = this.state.open_profile("");
                        this.run(effects, cx);
                    }),
                    window,
                    cx,
                )
            });
            body.push(
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(u(12.))
                    .text_px(14., 20.)
                    .text_color(faint)
                    .child(view.message.clone())
                    .children(sign_in)
                    .into_any_element(),
            );
        } else {
            let keys = view.rows.iter().map(|row| row.id.clone()).collect();
            let slots = self.rows.plan(&self.scroll, keys, 6., window, cx);
            let mut built = vec![false; view.rows.len()];
            let mut rows: Vec<AnyElement> = Vec::with_capacity(slots.len());
            for slot in slots {
                rows.push(match slot {
                    Slot::Row(index) => {
                        built[index] = true;
                        self.row(index, &view.rows[index], window, cx)
                    }
                    Slot::Space(height) => div().flex_none().h(height).into_any_element(),
                });
            }
            // В 0.4.9 аватары всего списка грузятся сразу: строка за краем просит свой заранее.
            // Загрузку `Pictures::get` не повторяет (кеш загрузчика и `waiting`): за кадр — поиск
            // по кешу на строку, как до окна видимости.
            for (row, _) in view.rows.iter().zip(&built).filter(|(_, built)| !**built) {
                self.photo(&row.avatar, window, cx);
            }
            body.push(
                self.rows
                    .measure(
                        &self.scroll,
                        div().flex().flex_col().gap(u(6.)).children(rows),
                    )
                    .into_any_element(),
            );
        }
        if view.more {
            let more = self.more(
                "players-more",
                view.more_label.clone(),
                view.more_loading,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    let effects = this.state.load_top(true);
                    this.run(effects, cx);
                }),
                window,
                cx,
            );
            // Кнопка — по ширине подписи, как <button> в потоке.
            body.push(div().flex().child(more).into_any_element());
        }
        // shortcut: края — заливкой цвета фона, как у ленты (S6).
        let list = ScrollArea::new("home-players", &self.scroll)
            .bar(Bar::List)
            .contain()
            .fade(theme.color("l-shade-rgb"))
            .flex_1()
            .min_h_0()
            .mr(u(-4.))
            .child(div().flex().flex_col().pr(u(4.)).pb(u(32.)).children(body));
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(u(16.))
            .child(head)
            .child(list)
    }
}

#[cfg(test)]
mod tests;
