//! Машина состояний оболочки (`LauncherShell.vue` 0.4.9): тосты (`notify` — в слой окна или в
//! свою очередь), окно
//! подтверждения (`askConfirm`), тексты по кодам ошибок, смена языка с пересборкой интерфейса
//! (`langRev`), секретные достижения. Вид читает состояние и узнаёт о переменах через
//! [`ShellController::on_change`].
//!
//! Живёт в потоке интерфейса. Сценарий ждёт ответа окна так:
//! `env.spawn_local(Box::pin(async move { if shell.ask_confirm(req).await.accepted() { … } }))`.

use crate::env::Env;
use crate::overlay::ToastInput;
use hlsr_core::language::Language;
use hlsr_core::settings::Settings;
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};
use std::time::Duration;

/// Сколько живёт тост.
pub const TOAST_LIFETIME: Duration = Duration::from_millis(2800);
/// Сколько тостов видно одновременно: четвёртый вытесняет самый старый.
pub const MAX_TOASTS: usize = 3;
/// Значок тоста по умолчанию.
pub const DEFAULT_TOAST_ICON: &str = "fas fa-info-circle";
/// Значок тоста об ошибке.
pub const ERROR_ICON: &str = "fas fa-exclamation-triangle";

/// Перевод: язык, ключ, аргументы → строка (`$t`). Нет ключа — сам ключ.
pub type Translator = Rc<dyn Fn(Language, &str, &[&str]) -> String>;

/// Запрос окна подтверждения (`askConfirm(request)`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Confirm {
    pub title: String,
    pub text: String,
    pub items: Vec<String>,
    pub note: Option<String>,
    pub confirm_label: Option<String>,
    pub cancel_label: Option<String>,
    pub danger: bool,
    /// Варианты `(id, подпись)`: тогда ответ — id выбранного.
    pub choices: Vec<(String, String)>,
    pub default_choice: Option<String>,
}

/// Ответ окна подтверждения.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmAnswer {
    /// «Подтвердить» в окне без вариантов (`true`).
    Accepted,
    /// «Отмена», затемнение, Esc или окно заменил новый запрос (`false`).
    Declined,
    /// Выбранный вариант (`id`).
    Choice(String),
}

impl ConfirmAnswer {
    /// Ответ «да» (вариант выбран или подтверждено).
    pub fn accepted(&self) -> bool {
        !matches!(self, ConfirmAnswer::Declined)
    }
}

/// Ожидание ответа окна подтверждения.
pub struct ConfirmFuture {
    rx: tokio::sync::oneshot::Receiver<ConfirmAnswer>,
}

impl ConfirmFuture {
    /// Ответ и его ожидание (окно подтверждения вне `ShellController`: тесты, сценарии).
    pub fn pair() -> (tokio::sync::oneshot::Sender<ConfirmAnswer>, ConfirmFuture) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        (tx, ConfirmFuture { rx })
    }

    /// Ответ, если он уже есть (без ожидания).
    pub fn try_answer(&mut self) -> Option<ConfirmAnswer> {
        match self.rx.try_recv() {
            Ok(answer) => Some(answer),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => None,
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => Some(ConfirmAnswer::Declined),
        }
    }
}

impl Future for ConfirmFuture {
    type Output = ConfirmAnswer;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<ConfirmAnswer> {
        Pin::new(&mut self.rx)
            .poll(cx)
            .map(|answer| answer.unwrap_or(ConfirmAnswer::Declined))
    }
}

struct PendingConfirm {
    request: Confirm,
    reply: tokio::sync::oneshot::Sender<ConfirmAnswer>,
}

type AchievementListener = Rc<dyn Fn(&str)>;
/// Слой тостов окна: текст, значок, `ms` (0 — по умолчанию).
type ToastSink = Rc<dyn Fn(&str, &str, u64)>;

struct Inner {
    env: Rc<dyn Env>,
    toasts: RefCell<Vec<ToastInput>>,
    toast_seq: Cell<i32>,
    toast_lifetime: Cell<Option<Duration>>,
    confirm: RefCell<Option<PendingConfirm>>,
    language: Cell<Language>,
    lang_rev: Cell<i32>,
    translator: RefCell<Translator>,
    on_achievement: RefCell<Option<AchievementListener>>,
    on_toast: RefCell<Option<ToastSink>>,
    on_change: RefCell<Option<Rc<dyn Fn()>>>,
}

/// Контроллер оболочки; клон — тот же контроллер.
#[derive(Clone)]
pub struct ShellController {
    inner: Rc<Inner>,
}

/// Перевод по умолчанию — строки `tr.rs`.
pub fn default_translator() -> Translator {
    Rc::new(crate::tr)
}

impl ShellController {
    /// Оболочка с языком `language`; сроки тостов идут по часам `env`.
    pub fn new(env: Rc<dyn Env>, language: Language) -> ShellController {
        ShellController {
            inner: Rc::new(Inner {
                env,
                toasts: RefCell::default(),
                toast_seq: Cell::new(0),
                toast_lifetime: Cell::new(Some(TOAST_LIFETIME)),
                confirm: RefCell::new(None),
                language: Cell::new(language),
                lang_rev: Cell::new(0),
                translator: RefCell::new(default_translator()),
                on_achievement: RefCell::new(None),
                on_toast: RefCell::new(None),
                on_change: RefCell::new(None),
            }),
        }
    }

    /// Кому сообщать о переменах (тосты, окно подтверждения, язык): вид перерисовывается.
    pub fn on_change(&self, listener: impl Fn() + 'static) {
        *self.inner.on_change.borrow_mut() = Some(Rc::new(listener));
    }

    fn changed(&self) {
        let listener = self.inner.on_change.borrow().clone();
        if let Some(listener) = listener {
            listener();
        }
    }

    /// Текущий язык интерфейса.
    pub fn language(&self) -> Language {
        self.inner.language.get()
    }

    /// Заменить перевод (`Locale.tr` задачи 8).
    pub fn set_translator(&self, translator: Translator) {
        *self.inner.translator.borrow_mut() = translator;
    }

    /// `$t(key, ...args)` на текущем языке.
    pub fn tr(&self, key: &str, args: &[&str]) -> String {
        let translator = self.inner.translator.borrow().clone();
        translator(self.language(), key, args)
    }

    /// Слой тостов окна: с ним `notify` идёт туда (очередь и сроки — у слоя), а не в очередь
    /// контроллера.
    pub fn on_toast(&self, sink: impl Fn(&str, &str, u64) + 'static) {
        *self.inner.on_toast.borrow_mut() = Some(Rc::new(sink));
    }

    /// `notify(text, icon)`: тост в конец очереди; больше трёх — самый старый уходит; через
    /// 2 800 мс тост закрывается сам. Пустой значок — `fas fa-info-circle`.
    pub fn notify(&self, text: &str, icon: &str) {
        self.notify_for(text, icon, 0);
    }

    /// `notify(text, icon, ms)` 0.4.9: `ms` — сколько держать (0 — 2 800; длинные с путями —
    /// 15 000). Без слоя тостов `ms` не играет: очередь контроллера — для тестов сценариев.
    pub fn notify_for(&self, text: &str, icon: &str, ms: u64) {
        let sink = self.inner.on_toast.borrow().clone();
        if let Some(sink) = sink {
            let icon = if icon.is_empty() {
                DEFAULT_TOAST_ICON
            } else {
                icon
            };
            sink(text, icon, ms);
            return;
        }
        let inner = &self.inner;
        let id = inner.toast_seq.get() + 1;
        inner.toast_seq.set(id);
        {
            let mut toasts = inner.toasts.borrow_mut();
            toasts.push(ToastInput {
                id,
                text: text.to_owned(),
                icon: if icon.is_empty() {
                    DEFAULT_TOAST_ICON.to_owned()
                } else {
                    icon.to_owned()
                },
            });
            if toasts.len() > MAX_TOASTS {
                toasts.remove(0);
            }
        }
        self.toasts_changed();
        let Some(lifetime) = inner.toast_lifetime.get() else {
            return;
        };
        let weak = Rc::downgrade(inner);
        inner.env.after(
            lifetime,
            Box::new(move || {
                if let Some(inner) = weak.upgrade() {
                    ShellController { inner }.dismiss_toast(id);
                }
            }),
        );
    }

    /// Срок жизни новых тостов: `None` — живут, пока их не закроют щелчком или не вытеснит
    /// четвёртый (окно сцен с `timerCeilingMs` эталона не больше [`TOAST_LIFETIME`]: там
    /// `setTimeout(…, 2800)` оболочки не срабатывает, и тост стоит на снимке).
    pub fn set_toast_lifetime(&self, lifetime: Option<Duration>) {
        self.inner.toast_lifetime.set(lifetime);
    }

    /// `dismissToast(id)` (щелчок по тосту, конец срока); уже закрытый — ничего.
    pub fn dismiss_toast(&self, id: i32) {
        let removed = {
            let mut toasts = self.inner.toasts.borrow_mut();
            let index = toasts.iter().position(|toast| toast.id == id);
            index.map(|index| toasts.remove(index)).is_some()
        };
        if removed {
            self.toasts_changed();
        }
    }

    /// Тосты в очереди по порядку (`toasts` оболочки; уходящие на экране в неё не входят).
    pub fn toasts(&self) -> Vec<ToastInput> {
        self.inner.toasts.borrow().clone()
    }

    /// Очередь изменилась.
    fn toasts_changed(&self) {
        self.changed();
    }

    /// `askConfirm(request)`: открыть окно подтверждения. Если окно уже открыто, прежний запрос
    /// получает «нет», и его место занимает новый (0.4.7 не ставит запросы в очередь).
    pub fn ask_confirm(&self, request: Confirm) -> ConfirmFuture {
        let pending = self.inner.confirm.borrow_mut().take();
        if let Some(pending) = pending {
            Self::answer(pending, false, "");
        }
        let (reply, rx) = tokio::sync::oneshot::channel();
        *self.inner.confirm.borrow_mut() = Some(PendingConfirm { request, reply });
        self.changed();
        ConfirmFuture { rx }
    }

    /// Запрос открытого окна подтверждения.
    pub fn confirm(&self) -> Option<Confirm> {
        self.inner
            .confirm
            .borrow()
            .as_ref()
            .map(|pending| pending.request.clone())
    }

    /// Открыто ли окно подтверждения.
    pub fn confirm_open(&self) -> bool {
        self.inner.confirm.borrow().is_some()
    }

    /// `onConfirmResolved(accepted)`: ответ окна. С вариантами ответ — `choice` (пустой —
    /// «нет»), без них — `accepted`.
    pub fn resolve_confirm(&self, accepted: bool, choice: &str) {
        let pending = self.inner.confirm.borrow_mut().take();
        if let Some(pending) = pending {
            Self::answer(pending, accepted, choice);
            self.changed();
        }
    }

    fn answer(pending: PendingConfirm, accepted: bool, choice: &str) {
        let answer = match (!pending.request.choices.is_empty(), accepted) {
            (true, true) if !choice.is_empty() => ConfirmAnswer::Choice(choice.to_owned()),
            (false, true) => ConfirmAnswer::Accepted,
            _ => ConfirmAnswer::Declined,
        };
        // Сценарий мог уже не ждать ответа — это не ошибка.
        let _ = pending.reply.send(answer);
    }

    /// Текст по коду ошибки: `prefix + code`, если такой ключ есть (и перевод не пуст), иначе
    /// `fallback` (`notifyDeliveryError`, `#L_BT_ERR_*`).
    pub fn error_text(&self, prefix: &str, code: Option<&str>, fallback: &str) -> String {
        if let Some(code) = code.filter(|code| !code.is_empty()) {
            let key = format!("{prefix}{code}");
            let text = self.tr(&key, &[]);
            if !text.is_empty() && text != key {
                return text;
            }
        }
        self.tr(fallback, &[])
    }

    /// `notifyDeliveryError(result)`: `code` — `result.error.code`.
    pub fn notify_delivery_error(&self, code: Option<&str>) {
        let text = self.error_text("#L_D_ERR_", code, "#L_D_ERROR");
        self.notify(&text, ERROR_ICON);
    }

    /// `onLanguageChange`: новый язык и новое поколение интерфейса ([`ShellController::lang_rev`])
    /// — всё внутри рамки окна строится заново (черновики экранов теряются, «Главная» загружается
    /// снова, меню закрываются), окно подтверждения остаётся, тосты очереди — тоже (на месте, без
    /// повторного входа). Язык программы обновления (`updater-set-language`) передаёт подключение
    /// `UpdateApi`.
    pub fn on_language_change(&self, lang: Language) {
        self.inner.language.set(lang);
        self.inner
            .lang_rev
            .set(self.inner.lang_rev.get().wrapping_add(1));
        self.changed();
    }

    /// Поколение интерфейса (`langRev`): растёт с каждой сменой языка.
    pub fn lang_rev(&self) -> i32 {
        self.inner.lang_rev.get()
    }

    /// `setLanguage(id)`: тот же язык — ничего (`false`); иначе записать в `settings.json` и
    /// перестроить интерфейс.
    pub fn set_language(&self, settings: &mut Settings, lang: Language) -> bool {
        if lang == self.language() {
            return false;
        }
        settings.set_language(lang);
        self.on_language_change(lang);
        true
    }

    /// Куда отправлять секретные достижения (`runner-secret-unlock`, подключение `OnlineApi`).
    pub fn on_unlock_achievement(&self, listener: impl Fn(&str) + 'static) {
        *self.inner.on_achievement.borrow_mut() = Some(Rc::new(listener));
    }

    /// `unlockAchievement(id)`: поздравление приходит событием `achievement-unlocked`.
    pub fn unlock_achievement(&self, id: &str) {
        let listener = self.inner.on_achievement.borrow().clone();
        match listener {
            Some(listener) => listener(id),
            None => tracing::debug!("[shell] достижение {id}: онлайн не подключён"),
        }
    }
}

/// Вариант, выбранный при открытии окна (`data()` ConfirmModal): умолчание, если оно среди
/// вариантов, иначе первый; без вариантов — пусто.
pub fn initial_choice(request: &Confirm) -> &str {
    let ids = || request.choices.iter().map(|(id, _)| id.as_str());
    request
        .default_choice
        .as_deref()
        .filter(|id| ids().any(|known| known == *id))
        .or_else(|| ids().next())
        .unwrap_or("")
}
