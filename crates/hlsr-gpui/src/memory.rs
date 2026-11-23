//! Управление памятью процесса на Linux (glibc) и выбор аппаратного Vulkan.
//!
//! 1. В glibc по умолчанию создаётся до `8 × CPU` отдельных арен `malloc` по 64 МиБ и при
//!    освобождении крупных растров (6–16 МиБ) динамический порог `M_MMAP_THRESHOLD` раздувается
//!    до 32 МиБ. При декодировании картинок в фоновых потоках GPUI/декодера и освобождении их в
//!    потоке окна это удерживает 150–400 МиБ уже освобождённых страниц в RSS процесса.
//!    Фиксация `M_ARENA_MAX = 2` и `M_MMAP_THRESHOLD = 128 КиБ` до запуска потоков возвращает
//!    крупные растры ядру сразу через `munmap`, а [`trim_heap`] отдаёт свободные страницы кучи
//!    после выгрузки тяжёлых экранов и кэшей.
//! 2. На Linux загрузчик Vulkan при наличии Mesa Lavapipe (`lvp_icd*.json`) подгружает
//!    `libvulkan_lvp.so` + `libLLVM.so` (~29 МиБ RSS) даже на машине с аппаратным GPU.
//!    [`prefer_hardware_vulkan`] отключает программный ICD только когда в системе есть аппаратный
//!    узел `/dev/dri/renderD*` и не-`lvp` ICD, не трогая пользовательские переменные окружения.

/// Настроить аллокатор glibc до создания фоновых потоков. Вызывать в самом начале `main`.
pub fn tune_allocator() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // Значения из `<malloc.h>` glibc.
        const M_TRIM_THRESHOLD: std::ffi::c_int = -1;
        const M_MMAP_THRESHOLD: std::ffi::c_int = -3;
        const M_ARENA_MAX: std::ffi::c_int = -8;
        unsafe extern "C" {
            fn mallopt(param: std::ffi::c_int, value: std::ffi::c_int) -> std::ffi::c_int;
        }
        unsafe {
            // Если пользователь сам задал MALLOC_ARENA_MAX / MALLOC_MMAP_THRESHOLD_, не мешаем.
            if std::env::var_os("MALLOC_ARENA_MAX").is_none() {
                mallopt(M_ARENA_MAX, 2);
            }
            if std::env::var_os("MALLOC_MMAP_THRESHOLD_").is_none() {
                mallopt(M_MMAP_THRESHOLD, 128 * 1024);
            }
            if std::env::var_os("MALLOC_TRIM_THRESHOLD_").is_none() {
                mallopt(M_TRIM_THRESHOLD, 128 * 1024);
            }
        }
    }
}

/// Отдать освобождённые страницы кучи обратно ОС (вызывать после сброса тяжёлых кэшей, не в
/// кадрах анимации).
pub fn trim_heap() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> std::ffi::c_int;
        }
        unsafe {
            malloc_trim(0);
        }
    }
}

/// То же в фоновом потоке, чтобы системный вызов `malloc_trim(0)` не задерживал кадр UI.
pub fn trim_heap_async(cx: &gpui::App) {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        cx.background_executor().spawn(async { trim_heap() }).detach();
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    {
        let _ = cx;
    }
}

/// На Linux не подгружать программный Vulkan ICD Lavapipe (`libLLVM.so`, ~29 МиБ), если в
/// системе есть аппаратный DRM render-узел и аппаратный Vulkan ICD. Вызывать до запуска потоков.
pub fn prefer_hardware_vulkan() {
    #[cfg(target_os = "linux")]
    {
        for key in [
            "VK_LOADER_DRIVERS_DISABLE",
            "VK_ICD_FILENAMES",
            "VK_DRIVER_FILES",
            "VK_LOADER_DRIVERS_SELECT",
        ] {
            if std::env::var_os(key).is_some() {
                return;
            }
        }
        let has_drm_gpu = std::fs::read_dir("/dev/dri").is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("renderD"))
            })
        });
        if !has_drm_gpu {
            return;
        }
        let has_hw_icd = ["/etc/vulkan/icd.d", "/usr/share/vulkan/icd.d"]
            .into_iter()
            .any(|dir| {
                std::fs::read_dir(dir).is_ok_and(|entries| {
                    entries.flatten().any(|entry| {
                        entry.file_name().to_str().is_some_and(|name| {
                            name.ends_with(".json") && !name.starts_with("lvp_")
                        })
                    })
                })
            });
        if has_hw_icd {
            // SAFETY: вызывается в однопоточном начале `main()` до запуска потоков.
            unsafe {
                std::env::set_var("VK_LOADER_DRIVERS_DISABLE", "*lvp*");
            }
        }
    }
}
