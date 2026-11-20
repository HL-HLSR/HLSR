//! Windows: доступ к памяти чужого процесса (`rinput-menu-fix-win.js` → прямой WinAPI) над
//! собственным дочерним процессом теста: чтение и запись известных байтов (постоянная область —
//! только через `VirtualProtectEx`), поиск процесса по имени, снимок модулей и весь патч курсора
//! над «RInput.dll» и «user32.dll» в его памяти — те же проверки, что в самотесте `hl1-selftest.js`
//! («патч курсора RInput идёт через помощника с памятью процесса»). Дочерний процесс — копия
//! этого же тестового бинарника под своим именем, запущенная на один тест [`probe_child`] (там же
//! раскладка памяти). Настоящий 32-битный `hl.exe` с RInput 1.31 — в живой проверке.

#![cfg(windows)]

use hlsr_platform::rinput_menu_fix::{
    CAVE, HOOK, MenuSession, PatchStatus, ProcessMemory, TRAMPOLINE, build_cave, original_hook,
    patch_memory,
};
use hlsr_platform::rinput_menu_fix_win::{
    WinProcessMemory, module_base_in, open_session, process_ids, snapshot_modules,
};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

const PAGE_READONLY: u32 = 0x02;
const PAGE_READWRITE: u32 = 0x04;
const PAGE_EXECUTE_READ: u32 = 0x20;

const READONLY: [u8; 16] = [
    0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
];
const WRITABLE: [u8; 16] = [
    0xde, 0xad, 0xbe, 0xef, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x10, 0x20, 0x30, 0x40,
];

/// Дочерний процесс закрывается при выходе из области видимости.
struct Probe {
    child: Child,
    pid: u32,
    image: String,
    readonly: u64,
    writable: u64,
    rinput: u64,
    user32: u64,
    _dir: tempfile::TempDir,
}

impl Drop for Probe {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

const PROBE_ENV: &str = "HLSR_RINPUT_PROBE";
static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Копия тестового бинарника под своим именем (поиск по имени находит только её), запущенная на
/// один тест [`probe_child`]; адреса — строки `PROBE <число>` в её выводе.
fn start_probe() -> Probe {
    let dir = tempfile::tempdir().expect("временная папка");
    let number = COUNTER.fetch_add(1, Ordering::SeqCst);
    let image = format!("hlsr-rp-{}-{number}.exe", std::process::id());
    let exe = dir.path().join(&image);
    std::fs::copy(std::env::current_exe().expect("путь теста"), &exe).expect("копия теста");
    let mut child = Command::new(&exe)
        .args(["probe_child", "--exact", "--nocapture", "--test-threads=1"])
        .env(PROBE_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("запуск вспомогательного процесса");
    let pid = child.id();
    let mut reader = BufReader::new(child.stdout.take().expect("stdout копии"));
    let mut addresses = Vec::new();
    while addresses.len() < 4 {
        let mut line = String::new();
        let read = reader.read_line(&mut line).expect("вывод копии");
        assert!(read > 0, "копия завершилась, не напечатав адреса");
        // libtest пишет «test probe_child ... » в ту же строку, что и первый адрес.
        if let Some((_, value)) = line.trim().split_once("PROBE ") {
            addresses.push(value.parse::<u64>().expect("адрес — число"));
        }
    }
    Probe {
        child,
        pid,
        image,
        readonly: addresses[0],
        writable: addresses[1],
        rinput: addresses[2],
        user32: addresses[3],
        _dir: dir,
    }
}

#[test]
fn reads_and_writes_child_memory_with_virtual_protect() {
    let probe = start_probe();
    let mut memory = WinProcessMemory::open(probe.pid).expect("открыт дочерний процесс");

    assert_eq!(memory.read(probe.readonly, 16).unwrap(), READONLY);
    assert_eq!(memory.read(probe.writable, 16).unwrap(), WRITABLE);

    let patch = [0xca, 0xfe, 0xba, 0xbe];
    memory.write(probe.writable, &patch).unwrap();
    assert_eq!(memory.read(probe.writable, 4).unwrap(), patch);

    // Постоянная страница: запись проходит только с VirtualProtectEx, потом защита возвращается.
    let ro_patch = [0x13, 0x37, 0x42, 0x99];
    memory.write(probe.readonly, &ro_patch).unwrap();
    assert_eq!(memory.read(probe.readonly, 4).unwrap(), ro_patch);
    assert_eq!(protection_of(&probe, probe.readonly), PAGE_READONLY);
}

/// «Память hl.exe»: настоящие чтение и запись в дочернем процессе; базы модулей — те, что он
/// напечатал (снимок модулей нашёл бы настоящие DLL, а их в 64-битном процессе выше 4 ГиБ).
struct ChildGame<'a> {
    probe: &'a Probe,
    memory: WinProcessMemory,
    writes: Vec<u64>,
}

impl ProcessMemory for ChildGame<'_> {
    fn module_base(&mut self, name: &str) -> Result<Option<u64>, String> {
        Ok(match name {
            "rinput.dll" => Some(self.probe.rinput),
            "user32.dll" => Some(self.probe.user32),
            _ => None,
        })
    }

    fn read(&mut self, address: u64, length: usize) -> Result<Vec<u8>, String> {
        self.memory.read(address, length)
    }

    fn write(&mut self, address: u64, buf: &[u8]) -> Result<(), String> {
        self.writes.push(address);
        self.memory.write(address, buf)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn jump_target(buf: &[u8], offset: usize, address: u64) -> u32 {
    let rel = i32::from_le_bytes([
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
        buf[offset + 4],
    ]);
    (address as u32).wrapping_add(5).wrapping_add(rel as u32)
}

#[test]
fn patch_runs_over_child_process_memory() {
    let probe = start_probe();
    let (base, user32) = (probe.rinput, probe.user32);
    let mut game = ChildGame {
        probe: &probe,
        memory: WinProcessMemory::open(probe.pid).expect("открыт дочерний процесс"),
        writes: Vec::new(),
    };
    assert_eq!(patch_memory(&mut game).unwrap(), PatchStatus::Patched);

    let mut check = WinProcessMemory::open(probe.pid).unwrap();
    let image = check.read(base, 0xc200).unwrap();
    assert_eq!(
        hex(&image[0x1530..0x1532]),
        "eb18",
        "хук начинается с короткого перехода"
    );
    assert_eq!(
        hex(&image[0x1547..0x154a]),
        "c20400",
        "ret 4 исходного хука цел"
    );
    assert_eq!(image[0x154a], 0xe9);
    assert_eq!(
        jump_target(&image, 0x154a, base + 0x154a),
        (base + CAVE) as u32
    );
    let cave = build_cave(base, user32 + 0x2000);
    assert_eq!(
        &image[CAVE as usize..CAVE as usize + cave.len()],
        cave.as_slice()
    );
    assert_eq!(
        jump_target(&cave, 39, base + CAVE + 39),
        (base + TRAMPOLINE) as u32,
        "курсор показан: настоящая GetCursorPos"
    );
    assert_eq!(
        game.writes.last(),
        Some(&(base + HOOK)),
        "переход на патч ставится последним"
    );
    assert_eq!(
        protection_of(&probe, base + HOOK),
        PAGE_EXECUTE_READ,
        "защита страниц кода возвращена"
    );

    assert_eq!(patch_memory(&mut game).unwrap(), PatchStatus::Already);
}

#[test]
fn session_waits_while_rinput_is_not_loaded() {
    let probe = start_probe();
    // Процесс найден по имени (без учёта регистра), но RInput.dll в нём нет.
    let mut session = open_session();
    assert_eq!(
        session.patch(&probe.image.to_uppercase()).unwrap(),
        PatchStatus::Waiting
    );
    session.close();
}

#[test]
fn process_ids_find_the_child() {
    let probe = start_probe();
    let ids = process_ids(&probe.image).expect("перечисление процессов");
    assert_eq!(ids, vec![probe.pid], "найдена только копия теста");
    let stem = probe.image.trim_end_matches(".exe");
    assert_eq!(
        process_ids(stem).expect("перечисление процессов"),
        vec![probe.pid],
        "имя без .exe тоже находит процесс"
    );
}

#[test]
fn module_snapshot_lists_a_loaded_dll() {
    let probe = start_probe();
    let modules = snapshot_modules(probe.pid).expect("снимок модулей");
    assert!(
        modules
            .iter()
            .any(|(name, base)| name.eq_ignore_ascii_case("kernel32.dll") && *base != 0),
        "kernel32.dll не найден в снимке модулей"
    );
}

#[test]
fn module_base_filters_to_the_32_bit_view() {
    let modules = vec![
        ("rinput.dll".to_owned(), 0x7fff_0000_0000u64),
        ("RINPUT.DLL".to_owned(), 0x642c_0000u64),
        ("user32.dll".to_owned(), 0x7b5b_0000u64),
    ];
    assert_eq!(module_base_in(&modules, "rinput.dll"), Some(0x642c_0000));
    assert_eq!(module_base_in(&modules, "USER32.DLL"), Some(0x7b5b_0000));
    assert_eq!(module_base_in(&modules, "missing.dll"), None);
    assert_eq!(
        module_base_in(&[("rinput.dll".to_owned(), 0x1_0000_0000u64)], "rinput.dll"),
        None,
        "база выше 4 ГиБ не видна 32-битному процессу"
    );
}

/// Защита страницы в дочернем процессе (`VirtualQueryEx`).
fn protection_of(probe: &Probe, address: u64) -> u32 {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQueryEx};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_INFORMATION};
    // SAFETY: открываем свой дочерний процесс; дескриптор закрывается ниже.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION, 0, probe.pid) };
    assert!(!handle.is_null(), "OpenProcess");
    // SAFETY: структура из нулей допустима.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: `info` — локальная структура нужного размера.
    let size = unsafe {
        VirtualQueryEx(
            handle,
            address as *const core::ffi::c_void,
            &mut info,
            std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    // SAFETY: дескриптор получен выше.
    unsafe { CloseHandle(handle) };
    assert!(size != 0, "VirtualQueryEx");
    info.Protect
}

// ------------------------------------------------------------------- дочерний процесс

/// Постоянная область: в `.rdata`, страница только для чтения.
static PROBE_READONLY: [u8; 16] = READONLY;

const RINPUT_SIZE: usize = 0x13000;
const USER32_SIZE: usize = 0x3000;

fn put_u32(buf: &mut [u8], at: usize, value: u32) {
    buf[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

/// Тело дочернего процесса (без переменной [`PROBE_ENV`] сразу выходит). Печатает четыре адреса:
/// 1. постоянная область (запись в неё требует `VirtualProtectEx`);
/// 2. изменяемая область на куче;
/// 3. «RInput.dll» в миниатюре ниже 4 ГиБ: исходный `hGetCursorPos` 1.31, `int3` за ним, нули
///    пещеры; страницы `PAGE_EXECUTE_READ`, как у кода DLL;
/// 4. «user32.dll» ниже 4 ГиБ: таблица экспорта с `GetCursorInfo` и `GetCursorPos`, который уже
///    начинается с перехода на перехват RInput; страницы только для чтения.
///
/// Раскладка — та же, что у `fakeGame` самотеста. Живёт 15 с — тест закрывает его раньше.
#[test]
fn probe_child() {
    use std::hint::black_box;
    use std::io::Write as _;
    use windows_sys::Win32::System::Memory::VirtualProtect;

    if std::env::var_os(PROBE_ENV).is_none() {
        return;
    }
    let writable: &'static mut [u8; 16] = Box::leak(Box::new(WRITABLE));
    let rinput_ptr = alloc_low(RINPUT_SIZE);
    let user32_ptr = alloc_low(USER32_SIZE);
    let base = rinput_ptr as u64;
    let user32_base = user32_ptr as u64;
    {
        // SAFETY: обе области выделены выше, размеры те же.
        let rinput = unsafe { std::slice::from_raw_parts_mut(rinput_ptr, RINPUT_SIZE) };
        // SAFETY: как выше.
        let user32 = unsafe { std::slice::from_raw_parts_mut(user32_ptr, USER32_SIZE) };
        let hook = original_hook(base);
        rinput[HOOK as usize..HOOK as usize + hook.len()].copy_from_slice(&hook);
        rinput[0x154a..0x1550].fill(0xcc);

        put_u32(user32, 0x3c, 0x80);
        put_u32(user32, 0x80 + 0x78, 0x1000);
        put_u32(user32, 0x80 + 0x7c, 0x300);
        for (offset, value) in [(20, 2), (24, 2), (28, 0x1100), (32, 0x1120), (36, 0x1140)] {
            put_u32(user32, 0x1000 + offset, value);
        }
        put_u32(user32, 0x1100, 0x2000);
        put_u32(user32, 0x1104, 0x2100);
        put_u32(user32, 0x1120, 0x1200);
        put_u32(user32, 0x1124, 0x1220);
        user32[0x1142..0x1144].copy_from_slice(&1u16.to_le_bytes());
        user32[0x1200..0x1200 + 14].copy_from_slice(b"GetCursorInfo\0");
        user32[0x1220..0x1220 + 13].copy_from_slice(b"GetCursorPos\0");
        user32[0x2100] = 0xe9;
        let rel = ((base + HOOK) as u32).wrapping_sub((user32_base + 0x2105) as u32);
        put_u32(user32, 0x2101, rel);
    }
    for (ptr, size, protection) in [
        (rinput_ptr, RINPUT_SIZE, PAGE_EXECUTE_READ),
        (user32_ptr, USER32_SIZE, PAGE_READONLY),
    ] {
        let mut old = 0u32;
        // SAFETY: область выделена выше и живёт до конца процесса.
        let ok = unsafe { VirtualProtect(ptr.cast(), size, protection, &mut old) };
        assert!(ok != 0, "VirtualProtect");
    }

    let mut out = std::io::stdout();
    for address in [
        core::ptr::addr_of!(PROBE_READONLY) as u64,
        writable.as_ptr() as u64,
        base,
        user32_base,
    ] {
        let _ = writeln!(out, "PROBE {address}");
    }
    let _ = out.flush();
    for _ in 0..300 {
        black_box(&PROBE_READONLY);
        black_box(&*writable);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Область по свободному адресу ниже 4 ГиБ (как база модуля 32-битного процесса).
fn alloc_low(size: usize) -> *mut u8 {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, VirtualAlloc};
    let mut address: usize = 0x1000_0000;
    while address < 0xf000_0000 {
        // SAFETY: просим свободный диапазон по заданному адресу; занят — вернётся NULL.
        let ptr = unsafe {
            VirtualAlloc(
                address as *const core::ffi::c_void,
                size,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
            )
        };
        if !ptr.is_null() {
            return ptr.cast::<u8>();
        }
        address += 0x0100_0000;
    }
    panic!("нет свободного адреса ниже 4 ГиБ");
}
