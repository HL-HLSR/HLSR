//! «Повтори цвета» на шариках фона LiveSplit (секретное достижение `good-memory`): правила —
//! `memory-game.js` 0.4.9 ([`start`], [`press`]), щелчки, показ и подсветка с таймингами —
//! `onOrbClick`/`playMemory`/`flashOrb`/`resetMemory` из `LauncherBackground.vue` 0.4.9
//! ([`MemoryGame`]). Каждый раунд добавляет один случайный цвет, ошибка сбрасывает игру,
//! восьмой раунд без ошибки — победа.

use crate::env::Env;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

/// Раундов до победы (`ROUNDS`).
pub const ROUNDS: usize = 8;
/// Шариков (`ORB_COUNT`).
pub const ORB_COUNT: usize = 3;
/// Пауза перед показом с запасом после вспышки своего щелчка, мс (`MEMORY_PAUSE`).
pub const PAUSE_MS: u64 = 1100;
/// Шаг показа, мс (`MEMORY_STEP`).
pub const STEP_MS: u64 = 600;
/// Подсветка одного шарика, мс (`MEMORY_LIT`).
pub const LIT_MS: u64 = 400;
/// Тишина, после которой щелчок начинает новую игру, мс (`MEMORY_IDLE`).
pub const IDLE_MS: f64 = 5000.0;
/// Ошибка: шарики гаснут на это время, мс (`MEMORY_MISS`).
pub const MISS_MS: u64 = 700;
/// Тот же шарик ещё горит: гаснет на миг перед новой вспышкой, мс.
const REFLASH_MS: u64 = 60;

/// Игра: номера шариков и сколько из них игрок уже повторил.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    pub sequence: Vec<usize>,
    pub step: usize,
}

/// Итог щелчка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    /// Ошибка — сброс (игры больше нет).
    Wrong,
    /// Ждём следующий щелчок.
    Ok,
    /// Раунд пройден, в `sequence` уже новый цвет.
    Round,
    /// Пройден последний раунд (игры больше нет).
    Won,
}

/// Случайное число `[0, 1)`, как `Math.random`.
pub type Random = Box<dyn FnMut() -> f64>;

fn pick(count: usize, random: &mut dyn FnMut() -> f64) -> usize {
    ((random() * count as f64).floor().max(0.0) as usize).min(count - 1)
}

/// `start(count, random)`: раунд 1 — один случайный цвет.
pub fn start(count: usize, random: &mut dyn FnMut() -> f64) -> Game {
    Game {
        sequence: vec![pick(count, random)],
        step: 0,
    }
}

/// `press(game, index, count, random)`: щелчок по шарику `index`.
pub fn press(
    game: Option<&Game>,
    index: usize,
    count: usize,
    random: &mut dyn FnMut() -> f64,
) -> (Option<Game>, Press) {
    let Some(game) = game.filter(|game| game.sequence.get(game.step) == Some(&index)) else {
        return (None, Press::Wrong);
    };
    let step = game.step + 1;
    if step < game.sequence.len() {
        let next = Game {
            sequence: game.sequence.clone(),
            step,
        };
        return (Some(next), Press::Ok);
    }
    if game.sequence.len() >= ROUNDS {
        return (None, Press::Won);
    }
    let mut sequence = game.sequence.clone();
    sequence.push(pick(count, random));
    (Some(Game { sequence, step: 0 }), Press::Round)
}

/// `Math.random` без своей зависимости: хеш ОС-случайного ключа и счётчика.
pub fn os_random() -> Random {
    use std::hash::{BuildHasher, Hasher};
    let state = std::collections::hash_map::RandomState::new();
    let mut counter = 0u64;
    Box::new(move || {
        counter += 1;
        let mut hasher = state.build_hasher();
        hasher.write_u64(counter);
        (hasher.finish() >> 11) as f64 / (1u64 << 53) as f64
    })
}

/// Что горит.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lit {
    None,
    One(usize),
    /// Все три (старт игры, `ALL_ORBS`).
    All,
}

struct Inner {
    env: Rc<dyn Env>,
    random: RefCell<Random>,
    game: RefCell<Option<Game>>,
    /// Идёт показ: щелчки не считаются (`memoryPlaying`).
    playing: Cell<bool>,
    /// Последний щелчок или конец показа, мс (`memoryLast`).
    last: Cell<f64>,
    lit: Cell<Lit>,
    miss: Cell<bool>,
    /// `memoryTimers`: сброс отменяет их, сдвигая поколение.
    epoch: Cell<u64>,
    /// `litTimer`: новая вспышка отменяет прежний.
    lit_epoch: Cell<u64>,
    on_change: RefCell<Option<Rc<dyn Fn()>>>,
    on_won: RefCell<Option<Rc<dyn Fn()>>>,
}

/// Щелчки, показ и подсветка «повтори цвета»; клон — та же игра.
#[derive(Clone)]
pub struct MemoryGame {
    inner: Rc<Inner>,
}

impl MemoryGame {
    pub fn new(env: Rc<dyn Env>, random: Random) -> MemoryGame {
        MemoryGame {
            inner: Rc::new(Inner {
                env,
                random: RefCell::new(random),
                game: RefCell::default(),
                playing: Cell::new(false),
                last: Cell::new(0.0),
                lit: Cell::new(Lit::None),
                miss: Cell::new(false),
                epoch: Cell::new(0),
                lit_epoch: Cell::new(0),
                on_change: RefCell::default(),
                on_won: RefCell::default(),
            }),
        }
    }

    /// Кому сообщать, что подсветка или ошибка сменились: вид перерисовывается.
    pub fn on_change(&self, listener: impl Fn() + 'static) {
        *self.inner.on_change.borrow_mut() = Some(Rc::new(listener));
    }

    /// Победа: `runner-secret-unlock("good-memory")`.
    pub fn on_won(&self, listener: impl Fn() + 'static) {
        *self.inner.on_won.borrow_mut() = Some(Rc::new(listener));
    }

    /// Горит ли шарик `orb` (`is-lit`).
    pub fn lit(&self, orb: usize) -> bool {
        match self.inner.lit.get() {
            Lit::None => false,
            Lit::One(index) => index == orb,
            Lit::All => true,
        }
    }

    /// Шарики погашены ошибкой (`is-miss`).
    pub fn miss(&self) -> bool {
        self.inner.miss.get()
    }

    /// Игра (тесты).
    pub fn game(&self) -> Option<Game> {
        self.inner.game.borrow().clone()
    }

    /// Идёт показ (тесты).
    pub fn playing(&self) -> bool {
        self.inner.playing.get()
    }

    fn changed(&self) {
        let listener = self.inner.on_change.borrow().clone();
        if let Some(listener) = listener {
            listener();
        }
    }

    /// `setTimeout`, который снимает сброс игры (`memoryTimers`).
    fn later(&self, ms: u64, action: impl FnOnce(&MemoryGame) + 'static) {
        let epoch = self.inner.epoch.get();
        let weak = Rc::downgrade(&self.inner);
        self.inner.env.after(
            Duration::from_millis(ms),
            Box::new(move || {
                if let Some(inner) = weak.upgrade()
                    && inner.epoch.get() == epoch
                {
                    action(&MemoryGame { inner });
                }
            }),
        );
    }

    /// `onOrbClick(index)`.
    pub fn orb_clicked(&self, index: usize) {
        let inner = &self.inner;
        if inner.playing.get() {
            return;
        }
        let now = inner.env.now();
        if inner.game.borrow().is_none() || now - inner.last.get() > IDLE_MS {
            // Любой шарик начинает игру; старт видно: вспыхивают все три.
            let game = start(ORB_COUNT, &mut *inner.random.borrow_mut());
            *inner.game.borrow_mut() = Some(game);
            inner.miss.set(false);
            self.flash(Lit::All);
            self.play();
            return;
        }
        inner.last.set(now);
        let (game, result) = {
            let current = inner.game.borrow();
            press(
                current.as_ref(),
                index,
                ORB_COUNT,
                &mut *inner.random.borrow_mut(),
            )
        };
        *inner.game.borrow_mut() = game;
        if result == Press::Wrong {
            // Ошибку видно: шарики гаснут; следующий щелчок — новая игра.
            inner.lit_epoch.set(inner.lit_epoch.get() + 1);
            inner.lit.set(Lit::None);
            inner.miss.set(true);
            self.later(MISS_MS, |this| {
                this.inner.miss.set(false);
                this.changed();
            });
            self.changed();
            return;
        }
        self.flash(Lit::One(index));
        if result == Press::Won {
            let listener = inner.on_won.borrow().clone();
            if let Some(listener) = listener {
                listener();
            }
        }
        if result == Press::Round {
            self.play();
        }
    }

    /// `playMemory()`: показ последовательности; ввод открыт с последней вспышки.
    fn play(&self) {
        self.inner.playing.set(true);
        let sequence = self
            .inner
            .game
            .borrow()
            .as_ref()
            .map(|game| game.sequence.clone())
            .unwrap_or_default();
        for (i, orb) in sequence.iter().copied().enumerate() {
            self.later(PAUSE_MS + i as u64 * STEP_MS, move |this| {
                this.flash(Lit::One(orb))
            });
        }
        let end = PAUSE_MS + sequence.len().saturating_sub(1) as u64 * STEP_MS;
        self.later(end, |this| {
            this.inner.playing.set(false);
            this.inner.last.set(this.inner.env.now());
        });
    }

    /// `flashOrb(index)`: тот же шарик ещё горит — гаснет на миг, чтобы каждый щелчок был
    /// отдельной вспышкой.
    fn flash(&self, lit: Lit) {
        let inner = &self.inner;
        let lit_epoch = inner.lit_epoch.get() + 1;
        inner.lit_epoch.set(lit_epoch);
        // По сроку: вспыхнуть снова (`again`) или погаснуть.
        let (ms, again) = if inner.lit.get() == lit {
            inner.lit.set(Lit::None);
            (REFLASH_MS, Some(lit))
        } else {
            inner.lit.set(lit);
            (LIT_MS, None)
        };
        let weak = Rc::downgrade(inner);
        inner.env.after(
            Duration::from_millis(ms),
            Box::new(move || {
                if let Some(inner) = weak.upgrade()
                    && inner.lit_epoch.get() == lit_epoch
                {
                    let this = MemoryGame { inner };
                    match again {
                        Some(lit) => this.flash(lit),
                        None => {
                            this.inner.lit.set(Lit::None);
                            this.changed();
                        }
                    }
                }
            }),
        );
        self.changed();
    }

    /// `resetMemory()`: ушли со страницы LiveSplit — показ гаснет, игра сначала.
    pub fn reset(&self) {
        let inner = &self.inner;
        inner.epoch.set(inner.epoch.get() + 1);
        inner.lit_epoch.set(inner.lit_epoch.get() + 1);
        *inner.game.borrow_mut() = None;
        inner.playing.set(false);
        inner.miss.set(false);
        inner.lit.set(Lit::None);
        self.changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::ManualEnv;

    fn fixed(value: f64) -> impl FnMut() -> f64 {
        move || value
    }

    #[test]
    fn rules_match_memory_game_js() {
        assert_eq!(ROUNDS, 8);
        assert_eq!(
            start(3, &mut fixed(0.0)),
            Game {
                sequence: vec![0],
                step: 0
            }
        );
        assert_eq!(start(3, &mut fixed(0.9999)).sequence, [2]);
        let mut random = os_random();
        let mut seen: Vec<usize> = (0..200)
            .map(|_| start(3, &mut random).sequence[0])
            .collect();
        seen.sort();
        seen.dedup();
        assert_eq!(seen, [0, 1, 2], "цвета случайные и все три");

        let game = Game {
            sequence: vec![1, 0],
            step: 0,
        };
        let (game, result) = press(Some(&game), 1, 3, &mut fixed(0.5));
        assert_eq!(
            (game.clone(), result),
            (
                Some(Game {
                    sequence: vec![1, 0],
                    step: 1
                }),
                Press::Ok
            )
        );
        let (game, result) = press(game.as_ref(), 0, 3, &mut fixed(0.5));
        assert_eq!(
            (game.clone(), result),
            (
                Some(Game {
                    sequence: vec![1, 0, 1],
                    step: 0
                }),
                Press::Round
            ),
            "новый раунд — тот же порядок плюс один цвет"
        );
        assert_eq!(
            press(game.as_ref(), 2, 3, &mut random),
            (None, Press::Wrong)
        );
        assert_eq!(press(None, 0, 3, &mut random), (None, Press::Wrong));

        let mut results = Vec::new();
        let mut game = start(3, &mut random);
        loop {
            let index = game.sequence[game.step];
            let (next, result) = press(Some(&game), index, 3, &mut random);
            results.push(result);
            match next {
                Some(next) => game = next,
                None => break,
            }
        }
        assert_eq!(results.last(), Some(&Press::Won));
        assert_eq!(
            results.iter().filter(|r| **r == Press::Round).count(),
            ROUNDS - 1,
            "победа после 8-го раунда"
        );
        assert_eq!(results.len(), ROUNDS * (ROUNDS + 1) / 2);
    }

    /// Щелчки по последовательности игры (после показа).
    fn play_round(env: &ManualEnv, memory: &MemoryGame) {
        env.advance(PAUSE_MS as f64 + 8.0 * STEP_MS as f64);
        assert!(!memory.playing());
        for orb in memory.game().unwrap().sequence {
            memory.orb_clicked(orb);
        }
    }

    #[test]
    fn clicks_start_show_miss_idle_and_win_like_on_orb_click() {
        let env = ManualEnv::new();
        let memory = MemoryGame::new(env.clone(), Box::new(fixed(0.5)));
        let won = Rc::new(Cell::new(0));
        let counter = won.clone();
        memory.on_won(move || counter.set(counter.get() + 1));

        // Любой щелчок начинает игру: вспыхивают все три, затем показ.
        memory.orb_clicked(2);
        assert_eq!(memory.game().unwrap().sequence, [1]);
        assert!((0..3).all(|orb| memory.lit(orb)), "старт: горят все три");
        assert!(memory.playing());
        env.advance(LIT_MS as f64);
        assert!(!memory.lit(0) && !memory.lit(1));
        // Пока идёт показ, щелчки не считаются.
        memory.orb_clicked(1);
        assert_eq!(memory.game().unwrap().step, 0);
        env.advance((PAUSE_MS - LIT_MS) as f64);
        assert!(memory.lit(1) && !memory.lit(0), "показ: первый цвет");
        assert!(!memory.playing(), "ввод открыт с последней вспышки");

        // Ошибка: сброс, шарики гаснут на 700 мс, вспышки нет.
        memory.orb_clicked(0);
        assert!(memory.game().is_none() && memory.miss());
        assert!(
            !memory.lit(0) && !memory.lit(1),
            "ошибка шарик не подсвечивает"
        );
        env.advance(MISS_MS as f64);
        assert!(!memory.miss());

        // Новая игра; 5 с тишины — щелчок снова начинает игру, а не отвечает.
        memory.orb_clicked(0);
        assert!(memory.lit(0) && memory.lit(2), "новая игра — снова все три");
        env.advance(PAUSE_MS as f64);
        env.advance(IDLE_MS + 1.0);
        let first = memory.game().unwrap().sequence[0];
        memory.orb_clicked(first);
        assert_eq!(memory.game().unwrap().step, 0, "после 5 с — новая игра");
        assert!(memory.playing());

        // Семь раундов — ещё не победа, восьмой открывает good-memory.
        for _ in 0..7 {
            play_round(&env, &memory);
        }
        assert_eq!(memory.game().unwrap().sequence.len(), 8);
        assert_eq!(won.get(), 0, "семь раундов — ещё не победа");
        play_round(&env, &memory);
        assert_eq!(won.get(), 1);
        assert!(
            memory.game().is_none(),
            "после победы игра начинается заново"
        );
    }

    #[test]
    fn quick_repeat_flashes_twice_and_reset_stops_the_show() {
        let env = ManualEnv::new();
        let memory = MemoryGame::new(env.clone(), Box::new(fixed(0.0)));
        memory.orb_clicked(0);
        env.advance((PAUSE_MS + 10) as f64);
        assert!(!memory.playing() && memory.lit(0));
        // Тот же шарик ещё горит: гаснет на 60 мс и вспыхивает снова.
        memory.orb_clicked(0);
        assert!(!memory.lit(0), "гаснет на миг");
        env.advance(REFLASH_MS as f64);
        assert!(memory.lit(0), "отдельная вспышка");

        // Показ раунда 2 идёт; уход со страницы гасит его и сбрасывает игру.
        env.advance(PAUSE_MS as f64 - 200.0);
        assert!(memory.playing());
        memory.reset();
        assert!(memory.game().is_none() && !memory.playing() && !memory.lit(0));
        env.advance(5000.0);
        assert!((0..3).all(|orb| !memory.lit(orb)), "таймеры показа сняты");
    }
}
