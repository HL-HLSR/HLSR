//! Прогресс на кнопке панели задач (`BrowserWindow.setProgressBar(value)` Electron →
//! `ITaskbarList3`): `value < 0` — убрать, `value > 1` — бегущая полоса, иначе — доля `value`
//! (`SetProgressValue(value * 100, 100)`), как `TaskbarHost::SetProgressBar` Electron. Зовётся из
//! потока окна (там COM уже поднят Winit для перетаскивания; на всякий случай поднимается и здесь).

/// Что показать на кнопке.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Progress {
    /// Без полосы (`TBPF_NOPROGRESS`).
    None,
    /// Бегущая полоса (`TBPF_INDETERMINATE`).
    Indeterminate,
    /// Доля 0..=1 (`TBPF_NORMAL`), в сотых.
    Normal(u64),
}

/// `setProgressBar(value)` → состояние кнопки.
pub fn progress_of(value: f64) -> Progress {
    if value.is_nan() || value < 0.0 {
        Progress::None
    } else if value > 1.0 {
        Progress::Indeterminate
    } else {
        // `static_cast<int>(progress * 100)`.
        Progress::Normal((value * 100.0) as u64)
    }
}

/// Показать прогресс на кнопке окна `hwnd`.
#[cfg(windows)]
pub fn set_progress(hwnd: isize, value: f64) -> Result<(), String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    };
    use windows::Win32::UI::Shell::{
        ITaskbarList3, TBPF_INDETERMINATE, TBPF_NOPROGRESS, TBPF_NORMAL, TaskbarList,
    };

    let window = HWND(hwnd as *mut std::ffi::c_void);
    // SAFETY: COM-вызовы в потоке окна; `CoInitializeEx` повторно безопасен (S_FALSE или
    // RPC_E_CHANGED_MODE, если поток уже в COM), объект `TaskbarList` живёт до конца функции,
    // `window` — окно этого процесса.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let list: ITaskbarList3 = CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| format!("TaskbarList: {error}"))?;
        list.HrInit().map_err(|error| format!("HrInit: {error}"))?;
        match progress_of(value) {
            Progress::None => list.SetProgressState(window, TBPF_NOPROGRESS),
            Progress::Indeterminate => list.SetProgressState(window, TBPF_INDETERMINATE),
            Progress::Normal(percent) => list
                .SetProgressState(window, TBPF_NORMAL)
                .and_then(|()| list.SetProgressValue(window, percent, 100)),
        }
        .map_err(|error| format!("SetProgress: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_map_like_electron() {
        assert_eq!(progress_of(-1.0), Progress::None);
        assert_eq!(progress_of(f64::NAN), Progress::None);
        assert_eq!(progress_of(0.0), Progress::Normal(0));
        assert_eq!(progress_of(0.505), Progress::Normal(50));
        assert_eq!(progress_of(1.0), Progress::Normal(100));
        assert_eq!(progress_of(2.0), Progress::Indeterminate);
    }
}
