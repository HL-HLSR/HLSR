use std::{
    cell::RefCell,
    sync::LazyLock,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use gpui::{FrameRequestSource, PlatformDisplay};
use gpui_util::ResultExt;
use windows::Win32::{
    Foundation::HWND,
    Graphics::{
        Dwm::{DWM_TIMING_INFO, DwmFlush, DwmGetCompositionTimingInfo},
        Dxgi::{CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput},
        Gdi::HMONITOR,
    },
    System::Performance::QueryPerformanceFrequency,
};

static QPC_TICKS_PER_SECOND: LazyLock<u64> = LazyLock::new(|| {
    let mut frequency = 0;
    // On systems that run Windows XP or later, the function will always succeed and
    // will thus never return zero.
    unsafe { QueryPerformanceFrequency(&mut frequency).unwrap() };
    frequency as u64
});

const VSYNC_INTERVAL_THRESHOLD: Duration = Duration::from_millis(1);
const DEFAULT_VSYNC_INTERVAL: Duration = Duration::from_micros(16_666); // ~60Hz

/// HLSR: монитор без выхода DXGI (или с ошибкой ожидания) ищется заново не чаще этого.
const OUTPUT_RETRY: Duration = Duration::from_secs(1);

pub(crate) struct VSyncProvider {
    interval: Duration,
    f: Box<dyn Fn() -> bool>,
    /// HLSR: монитор окон, его выход DXGI (`None` — нет, ждать DWM), кадр монитора и когда выход
    /// найден.
    target: RefCell<Option<(HMONITOR, Option<IDXGIOutput>, Duration, Instant)>>,
}

impl VSyncProvider {
    pub(crate) fn new() -> Self {
        let interval = get_dwm_interval()
            .context("Failed to get DWM interval")
            .log_err()
            .unwrap_or(DEFAULT_VSYNC_INTERVAL);
        let f = Box::new(|| unsafe { DwmFlush().is_ok() });
        Self {
            interval,
            f,
            target: RefCell::default(),
        }
    }

    /// HLSR: `monitor` — монитор, на котором окна (`None` — окон нет или они на разных): ждать
    /// его vblank (`IDXGIOutput::WaitForVBlank`), как rAF Chromium, а не композицию DWM — она идёт
    /// с частотой самого быстрого монитора, и окно на мониторе 120 Гц рядом с 280 Гц будилось
    /// 280 раз в секунду неровно (2 или 3 будильника на кадр монитора). Без монитора — DWM.
    ///
    /// HLSR: ожидание длится `gpui::frame_divisor` кадров монитора (или композиции DWM) — сейчас
    /// один: окна будятся каждым vblank, с частотой монитора (решение владельца 22.11; прежде
    /// предел 60 к/с). Будят окна только отсюда, так что шаг общий для всего: переходы,
    /// бесконечные анимации, анимации сцены GPUI, ответ на ввод.
    pub(crate) fn wait_for_vsync(&self, monitor: Option<HMONITOR>) -> FrameRequestSource {
        let vsync_start = Instant::now();
        let (output, frame) = match monitor.and_then(|monitor| self.output(monitor)) {
            Some((output, frame)) => (Some(output), frame),
            None => (None, self.interval),
        };
        let mut wait_succeeded = true;
        for _ in 0..gpui::frame_divisor(frame) {
            wait_succeeded = match &output {
                Some(output) => {
                    let waited = unsafe { output.WaitForVBlank() };
                    if let Err(error) = &waited {
                        log::debug!("WaitForVBlank: {error}");
                        // Выход пропал (смена мониторов): DWM, пока он не найдётся снова. Ранний
                        // возврат (монитор спит) — как у DWM ниже: сон на интервал.
                        *self.target.borrow_mut() =
                            monitor.map(|m| (m, None, Duration::ZERO, Instant::now()));
                    }
                    waited.is_ok()
                }
                None => (self.f)(),
            };
            if !wait_succeeded {
                break;
            }
        }
        let elapsed = vsync_start.elapsed();
        // DwmFlush and DCompositionWaitForCompositorClock returns very early
        // instead of waiting until vblank when the monitor goes to sleep or is
        // unplugged (nothing to present due to desktop occlusion). We use 1ms as
        // a threshold for the duration of the wait functions and fallback to
        // Sleep() if it returns before that. This could happen during normal
        // operation for the first call after the vsync thread becomes non-idle,
        // but it shouldn't happen often.
        if !wait_succeeded || elapsed < VSYNC_INTERVAL_THRESHOLD {
            log::trace!("VSyncProvider::wait_for_vsync() took less time than expected");
            // HLSR: и сон — тот же шаг (`gpui::frame_divisor`).
            std::thread::sleep(self.interval * gpui::frame_divisor(self.interval));
            FrameRequestSource::LocalSchedule
        } else {
            FrameRequestSource::NativeCallback
        }
    }

    /// HLSR: выход DXGI монитора `monitor` и кадр монитора: тот же монитор — прежние, другой —
    /// перечислить выходы всех видеокарт (монитор может висеть на встроенной, а рисовать
    /// дискретная). Кадр перечитывается раз в `OUTPUT_RETRY`: частоту меняют и без смены выхода
    /// (120 → 60 Гц в настройках Windows), а делитель по старой дал бы 30 кадров/с.
    fn output(&self, monitor: HMONITOR) -> Option<(IDXGIOutput, Duration)> {
        let mut target = self.target.borrow_mut();
        if let Some((known, output, frame, since)) = target.as_mut()
            && *known == monitor
        {
            if since.elapsed() < OUTPUT_RETRY {
                return output.clone().map(|output| (output, *frame));
            }
            if let Some(output) = output {
                *frame = monitor_frame(monitor);
                *since = Instant::now();
                return Some((output.clone(), *frame));
            }
        }
        let output = find_output(monitor).log_err().flatten();
        let frame = monitor_frame(monitor);
        *target = Some((monitor, output.clone(), frame, Instant::now()));
        output.map(|output| (output, frame))
    }
}

/// HLSR: кадр текущего режима монитора. Частота неизвестна — 1/60 с, то есть каждый vblank, как
/// до предела: кадр композиции DWM здесь не годится — он у самого быстрого монитора, и окно на
/// 120 Гц рядом с 280 Гц рисовало бы каждый пятый свой vblank (24 кадра/с). Переходы и движение
/// лаунчера и так держат 60 кадров/с своей сеткой (`crates/hlsr-gpui/src/motion/driver.rs`).
fn monitor_frame(monitor: HMONITOR) -> Duration {
    crate::WindowsDisplay::new(crate::WindowsDisplay::display_id_for_monitor(monitor))
        .and_then(|display| display.refresh_interval())
        .unwrap_or(DEFAULT_VSYNC_INTERVAL)
}

/// HLSR: выход DXGI, который выводит монитор `monitor`.
fn find_output(monitor: HMONITOR) -> Result<Option<IDXGIOutput>> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }?;
    for adapter in (0..).map_while(|i| unsafe { factory.EnumAdapters1(i) }.ok()) {
        for output in (0..).map_while(|i| unsafe { adapter.EnumOutputs(i) }.ok()) {
            if unsafe { output.GetDesc() }?.Monitor == monitor {
                return Ok(Some(output));
            }
        }
    }
    Ok(None)
}

fn get_dwm_interval() -> Result<Duration> {
    let mut timing_info = DWM_TIMING_INFO {
        cbSize: std::mem::size_of::<DWM_TIMING_INFO>() as u32,
        ..Default::default()
    };
    unsafe { DwmGetCompositionTimingInfo(HWND::default(), &mut timing_info) }?;
    let interval = retrieve_duration(timing_info.qpcRefreshPeriod, *QPC_TICKS_PER_SECOND);
    // Check for interval values that are impossibly low. A 29 microsecond
    // interval was seen (from a qpcRefreshPeriod of 60).
    if interval < VSYNC_INTERVAL_THRESHOLD {
        Ok(retrieve_duration(
            timing_info.rateRefresh.uiDenominator as u64,
            timing_info.rateRefresh.uiNumerator as u64,
        ))
    } else {
        Ok(interval)
    }
}

#[inline]
fn retrieve_duration(counts: u64, ticks_per_second: u64) -> Duration {
    let ticks_per_microsecond = ticks_per_second / 1_000_000;
    Duration::from_micros(counts / ticks_per_microsecond)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_compositor_wait_reports_native_callback() {
        let provider = VSyncProvider {
            interval: Duration::ZERO,
            f: Box::new(|| {
                std::thread::sleep(VSYNC_INTERVAL_THRESHOLD * 2);
                true
            }),
            target: RefCell::default(),
        };

        assert_eq!(
            provider.wait_for_vsync(None),
            FrameRequestSource::NativeCallback
        );
    }

    #[test]
    fn failed_compositor_wait_reports_local_schedule_even_after_threshold() {
        let provider = VSyncProvider {
            interval: Duration::ZERO,
            f: Box::new(|| {
                std::thread::sleep(VSYNC_INTERVAL_THRESHOLD * 2);
                false
            }),
            target: RefCell::default(),
        };

        assert_eq!(
            provider.wait_for_vsync(None),
            FrameRequestSource::LocalSchedule
        );
    }

    /// HLSR: одно ожидание длится `frame_divisor` кадров (сейчас один на любой частоте),
    /// неудачное ожидание обрывает счёт.
    #[test]
    fn one_wait_spans_frame_divisor_frames() {
        for (hz, ok, waits) in [(60, true, 1), (120, true, 1), (280, true, 1), (280, false, 1)] {
            let calls = std::rc::Rc::new(std::cell::Cell::new(0));
            let counted = calls.clone();
            let provider = VSyncProvider {
                interval: Duration::from_secs(1) / hz,
                f: Box::new(move || {
                    counted.set(counted.get() + 1);
                    std::thread::sleep(VSYNC_INTERVAL_THRESHOLD);
                    ok
                }),
                target: RefCell::default(),
            };
            provider.wait_for_vsync(None);
            assert_eq!(calls.get(), waits, "{hz} Гц, ожидание удачно: {ok}");
        }
    }

    /// HLSR: у каждого монитора такт — его кадр, а не кадр самого быстрого (композиции DWM):
    /// на 120 Гц — 8,33 мс, на 280 Гц — 3,57 мс.
    #[test]
    fn each_monitor_paces_at_its_own_refresh() {
        let provider = VSyncProvider::new();
        for display in crate::WindowsDisplay::displays() {
            let Some(refresh) = display.refresh_interval() else {
                continue;
            };
            let frame = refresh * gpui::frame_divisor(refresh);
            let monitor = HMONITOR(u64::from(display.id()) as _);
            // Без выхода DXGI (удалённый сеанс) — такт DWM, сверять нечего.
            if provider.output(monitor).is_none() {
                continue;
            }
            provider.wait_for_vsync(Some(monitor));
            let mut gaps: Vec<Duration> = (0..30)
                .map(|_| {
                    let start = Instant::now();
                    provider.wait_for_vsync(Some(monitor));
                    start.elapsed()
                })
                .collect();
            gaps.sort();
            let median = gaps[gaps.len() / 2];
            assert!(
                median > frame * 3 / 4 && median < frame * 5 / 4,
                "кадр {median:?}, у монитора {frame:?}"
            );
        }
    }
}
