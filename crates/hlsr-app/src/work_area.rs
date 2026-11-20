//! Рабочая область монитора (без панелей) в физических пикселях.

/// Прямоугольник в физических пикселях рабочего стола.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkArea {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl WorkArea {
    #[cfg(test)]
    pub const EMPTY: WorkArea = WorkArea {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
    };

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// Рабочая область монитора `monitor` (его прямоугольник от winit; `screen.getDisplayMatching(
/// bounds).workArea` 0.4.7). Linux — `_NET_WORKAREA` текущего рабочего стола (общий
/// прямоугольник всех мониторов), Windows — `rcWork` этого монитора (`MonitorFromRect` +
/// `GetMonitorInfoW`; монитора нет — `SPI_GETWORKAREA` основного); в пределах `monitor`.
pub fn of_monitor(monitor: WorkArea) -> WorkArea {
    choose(platform::work_area(monitor), monitor)
}

/// Что ответила ОС и монитор → рабочая область: ответ ОС в пределах монитора; монитора нет —
/// ответ ОС как есть; ответа нет или он мимо монитора — весь монитор.
fn choose(area: Option<WorkArea>, monitor: WorkArea) -> WorkArea {
    match area {
        Some(area) if monitor.is_empty() => area,
        Some(area) => intersect(area, monitor).unwrap_or(monitor),
        None => monitor,
    }
}

/// Пересечение прямоугольников; `None`, если они не пересекаются.
pub fn intersect(a: WorkArea, b: WorkArea) -> Option<WorkArea> {
    let left = i64::from(a.x).max(i64::from(b.x));
    let top = i64::from(a.y).max(i64::from(b.y));
    let right = (i64::from(a.x) + i64::from(a.width)).min(i64::from(b.x) + i64::from(b.width));
    let bottom = (i64::from(a.y) + i64::from(a.height)).min(i64::from(b.y) + i64::from(b.height));
    if right <= left || bottom <= top {
        return None;
    }
    Some(WorkArea {
        x: left as i32,
        y: top as i32,
        width: (right - left) as u32,
        height: (bottom - top) as u32,
    })
}

#[cfg(target_os = "linux")]
mod platform {
    use super::WorkArea;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

    /// `_NET_WORKAREA` — по четыре числа на каждый рабочий стол; общий
    /// прямоугольник всех мониторов за вычетом панелей.
    pub fn work_area(_monitor: WorkArea) -> Option<WorkArea> {
        let (conn, screen) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots.get(screen)?.root;
        let atom = |name: &[u8]| Some(conn.intern_atom(false, name).ok()?.reply().ok()?.atom);
        let desktop = atom(b"_NET_CURRENT_DESKTOP")
            .and_then(|current| {
                conn.get_property(false, root, current, AtomEnum::CARDINAL, 0, 1)
                    .ok()?
                    .reply()
                    .ok()?
                    .value32()?
                    .next()
            })
            .unwrap_or(0);
        let reply = conn
            .get_property(
                false,
                root,
                atom(b"_NET_WORKAREA")?,
                AtomEnum::CARDINAL,
                desktop * 4,
                4,
            )
            .ok()?
            .reply()
            .ok()?;
        let v: Vec<u32> = reply.value32()?.collect();
        let [x, y, width, height] = v[..] else {
            return None;
        };
        Some(WorkArea {
            x: x as i32,
            y: y as i32,
            width,
            height,
        })
        .filter(|area| !area.is_empty())
    }
}

#[cfg(windows)]
mod platform {
    use super::WorkArea;
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETWORKAREA, SystemParametersInfoW};

    const EMPTY_RECT: RECT = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };

    fn area_of(r: RECT) -> Option<WorkArea> {
        Some(WorkArea {
            x: r.left,
            y: r.top,
            width: (r.right - r.left).max(0) as u32,
            height: (r.bottom - r.top).max(0) as u32,
        })
        .filter(|area| !area.is_empty())
    }

    /// `rcWork` монитора, на который больше всего приходится `monitor` (`MonitorFromRect`, как
    /// `getDisplayMatching` Chromium); пустой `monitor` — основной (`SPI_GETWORKAREA`).
    pub fn work_area(monitor: WorkArea) -> Option<WorkArea> {
        if monitor.is_empty() {
            return primary();
        }
        let rect = RECT {
            left: monitor.x,
            top: monitor.y,
            right: monitor.x.saturating_add_unsigned(monitor.width),
            bottom: monitor.y.saturating_add_unsigned(monitor.height),
        };
        // SAFETY: прямоугольник передаётся по ссылке на время вызова; с NEAREST ответ — монитор.
        let handle = unsafe { MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST) };
        if handle.is_null() {
            return primary();
        }
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            rcMonitor: EMPTY_RECT,
            rcWork: EMPTY_RECT,
            dwFlags: 0,
        };
        // SAFETY: `info` — MONITORINFO с заполненным `cbSize`; `handle` получен выше.
        let ok = unsafe { GetMonitorInfoW(handle, &mut info) };
        if ok == 0 {
            return primary();
        }
        area_of(info.rcWork)
    }

    /// `SPI_GETWORKAREA`: рабочая область основного монитора.
    fn primary() -> Option<WorkArea> {
        let mut r = EMPTY_RECT;
        // SAFETY: SPI_GETWORKAREA пишет один RECT по переданному указателю.
        let ok = unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, (&raw mut r).cast(), 0) };
        if ok == 0 {
            return None;
        }
        area_of(r)
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod platform {
    pub fn work_area(_monitor: super::WorkArea) -> Option<super::WorkArea> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_area_is_clipped_to_primary_monitor() {
        // _NET_WORKAREA на двух мониторах — общий прямоугольник рабочего стола.
        let net = WorkArea {
            x: 0,
            y: 0,
            width: 4480,
            height: 1390,
        };
        let monitor = WorkArea {
            x: 1920,
            y: 0,
            width: 2560,
            height: 1440,
        };
        assert_eq!(
            intersect(net, monitor),
            Some(WorkArea {
                x: 1920,
                y: 0,
                width: 2560,
                height: 1390
            })
        );
        let elsewhere = WorkArea {
            x: 5000,
            y: 0,
            width: 100,
            height: 100,
        };
        assert_eq!(intersect(net, elsewhere), None);
    }

    /// Окно на втором мониторе получает рабочую область этого монитора (прежде —
    /// `SPI_GETWORKAREA` основного, мимо монитора, то есть весь монитор вместе с панелью задач).
    #[test]
    fn work_area_of_the_window_monitor_is_chosen() {
        let second = WorkArea {
            x: 1920,
            y: -200,
            width: 2560,
            height: 1440,
        };
        let second_work = WorkArea {
            x: 1920,
            y: -200,
            width: 2560,
            height: 1392,
        };
        assert_eq!(choose(Some(second_work), second), second_work);
        // Основной монитор — как прежде: его рабочая область.
        let primary = WorkArea {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let primary_work = WorkArea {
            x: 0,
            y: 0,
            width: 1920,
            height: 1032,
        };
        assert_eq!(choose(Some(primary_work), primary), primary_work);
        // Ответ ОС мимо монитора или его нет — весь монитор; монитора нет — ответ ОС.
        assert_eq!(choose(Some(primary_work), second), second);
        assert_eq!(choose(None, second), second);
        assert_eq!(choose(Some(primary_work), WorkArea::EMPTY), primary_work);
        assert_eq!(choose(None, WorkArea::EMPTY), WorkArea::EMPTY);
    }

    /// Каждый монитор этой машины: рабочая область лежит внутри него; у основного она та же,
    /// что `SPI_GETWORKAREA` (прежний ответ).
    #[cfg(windows)]
    #[test]
    fn every_monitor_has_its_own_work_area() {
        use windows_sys::Win32::Foundation::{LPARAM, RECT};
        use windows_sys::Win32::Graphics::Gdi::{
            EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;
        use windows_sys::core::BOOL;

        unsafe extern "system" fn collect(
            handle: HMONITOR,
            _dc: HDC,
            _rect: *mut RECT,
            data: LPARAM,
        ) -> BOOL {
            // SAFETY: `data` — указатель на вектор из теста ниже, живой на время перечисления.
            let found = unsafe { &mut *(data as *mut Vec<(WorkArea, bool)>) };
            let empty = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            let mut info = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                rcMonitor: empty,
                rcWork: empty,
                dwFlags: 0,
            };
            // SAFETY: `handle` выдан перечислением, `cbSize` заполнен.
            if unsafe { GetMonitorInfoW(handle, &mut info) } != 0 {
                let r = info.rcMonitor;
                found.push((
                    WorkArea {
                        x: r.left,
                        y: r.top,
                        width: (r.right - r.left) as u32,
                        height: (r.bottom - r.top) as u32,
                    },
                    info.dwFlags & MONITORINFOF_PRIMARY != 0,
                ));
            }
            1
        }

        let mut found: Vec<(WorkArea, bool)> = Vec::new();
        // SAFETY: без DC и отсечения — все мониторы; обратный вызов пишет в `found`.
        unsafe {
            EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null(),
                Some(collect),
                (&raw mut found) as LPARAM,
            )
        };
        if found.is_empty() {
            eprintln!("пропуск: мониторов нет");
            return;
        }
        for (monitor, primary) in found {
            let work = of_monitor(monitor);
            assert_eq!(intersect(work, monitor), Some(work), "{monitor:?}");
            if primary {
                let old = choose(platform::work_area(WorkArea::EMPTY), monitor);
                assert_eq!(work, old, "основной монитор");
            }
        }
    }
}
