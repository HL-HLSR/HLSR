//! Общий лимит скорости загрузок (`DownloadRateLimiter.js`): игры, LiveSplit, сейвы и
//! зеркало ремонта делят одну очередь.
//!
//! Очередь FIFO: кредит за время простоя не копится, чтобы начало загрузки не давало
//! неограниченный скачок скорости. Таймер ждёт не дольше секунды; смена лимита будит
//! текущие ожидания, лимит 0 отпускает всех. Отмена ожидания (`AbortSignal` в 0.4.7) —
//! сброс будущего [`DownloadRateLimiter::acquire`]: место в очереди освобождается сразу.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::Notify;

#[derive(Debug)]
pub struct DownloadRateLimiter {
    state: Mutex<State>,
    changed: Notify,
}

#[derive(Debug)]
struct State {
    bytes_per_second: f64,
    queue: VecDeque<Waiter>,
    last_tick: Instant,
    next_id: u64,
}

#[derive(Debug)]
struct Waiter {
    id: u64,
    remaining: f64,
}

/// `Number.isFinite(x) ? Math.max(0, x) : 0`.
fn sanitize(bytes_per_second: f64) -> f64 {
    if bytes_per_second.is_finite() {
        bytes_per_second.max(0.0)
    } else {
        0.0
    }
}

impl State {
    /// Бюджет за прошедшее время отдаётся очереди по порядку; остаток уходит голове.
    /// `true` — кто-то отпущен.
    fn tick(&mut self) -> bool {
        let now = Instant::now();
        let mut budget = (now - self.last_tick).as_secs_f64() * self.bytes_per_second;
        self.last_tick = now;
        let mut released = false;
        while let Some(head) = self.queue.front() {
            if budget < head.remaining {
                break;
            }
            budget -= head.remaining;
            self.queue.pop_front();
            released = true;
        }
        if let Some(head) = self.queue.front_mut() {
            head.remaining -= budget;
        }
        released
    }

    fn waiting(&self, id: u64) -> bool {
        self.queue.iter().any(|waiter| waiter.id == id)
    }
}

/// Место в очереди: сброшенное ожидание (`onAbort` в 0.4.7) уходит из очереди.
struct Ticket<'a> {
    limiter: &'a DownloadRateLimiter,
    id: u64,
    released: bool,
}

impl Drop for Ticket<'_> {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let mut state = self.limiter.lock();
        state.tick();
        if let Some(index) = state.queue.iter().position(|w| w.id == self.id) {
            state.queue.remove(index);
        }
        drop(state);
        self.limiter.changed.notify_waiters();
    }
}

impl DownloadRateLimiter {
    /// Лимит в байтах в секунду; 0 — без ограничения.
    pub fn new(bytes_per_second: f64) -> DownloadRateLimiter {
        DownloadRateLimiter {
            state: Mutex::new(State {
                bytes_per_second: sanitize(bytes_per_second),
                queue: VecDeque::new(),
                last_tick: Instant::now(),
                next_id: 0,
            }),
            changed: Notify::new(),
        }
    }

    pub fn bytes_per_second(&self) -> f64 {
        self.lock().bytes_per_second
    }

    /// `setLimit`: действует сразу, в том числе на ожидающих.
    pub fn set_limit(&self, bytes_per_second: f64) {
        let mut state = self.lock();
        state.tick();
        state.bytes_per_second = sanitize(bytes_per_second);
        if state.bytes_per_second == 0.0 {
            state.queue.clear();
        }
        drop(state);
        self.changed.notify_waiters();
    }

    /// `acquire(bytes)`: дождаться права пропустить `bytes` байт.
    pub async fn acquire(&self, bytes: usize) {
        let mut ticket = {
            let mut state = self.lock();
            if state.tick() {
                self.changed.notify_waiters();
            }
            if state.bytes_per_second == 0.0 {
                return;
            }
            let id = state.next_id;
            state.next_id += 1;
            state.queue.push_back(Waiter {
                id,
                remaining: bytes as f64,
            });
            Ticket {
                limiter: self,
                id,
                released: false,
            }
        };
        self.changed.notify_waiters();

        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            // Голова очереди ждёт таймер, остальные — изменения очереди.
            let wait = {
                let state = self.lock();
                if !state.waiting(ticket.id) {
                    ticket.released = true;
                    return;
                }
                let head = state.queue.front().expect("в очереди есть ожидающий");
                (head.id == ticket.id).then(|| {
                    let ms = head.remaining * 1000.0 / state.bytes_per_second;
                    Duration::from_secs_f64(ms.clamp(1.0, 1000.0) / 1000.0)
                })
            };
            match wait {
                Some(delay) => {
                    tokio::select! {
                        () = tokio::time::sleep(delay) => {
                            if self.lock().tick() {
                                self.changed.notify_waiters();
                            }
                        }
                        () = &mut changed => {}
                    }
                }
                None => changed.await,
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }
}
