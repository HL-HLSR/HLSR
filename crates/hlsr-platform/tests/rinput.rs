//! Патч курсора RInput в меню (`rinput-menu-fix.js`, `linux/rinput-menu-fix.js`): точные байты
//! `originalHook`/`buildCave` против фикстуры 0.4.7 и патч над поддельным `/proc` — те же случаи,
//! что в самотесте `scripts/hl1-selftest.js` («HL1 Linux: патч курсора RInput пишет переход в
//! обход только знакомой DLL»). Логика `/proc` собирается на всех ОС, поэтому идёт и здесь.

mod support;

use hlsr_platform::linux::rinput_menu_fix::{
    MemFile, ProcFs, image_base, parse_maps, patch_running_game,
};
use hlsr_platform::rinput_menu_fix::{
    CAVE, CAVE_SIZE, HOOK, PatchStatus, STUB, TRAMPOLINE, build_cave, merge_status, original_hook,
};
use std::cell::RefCell;
use support::fixture;

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn hook_bytes_match_reference() {
    let data = fixture("launch.json");
    let rinput = &data["rinput"];
    assert_eq!(rinput["HOOK"].as_u64(), Some(HOOK));
    assert_eq!(rinput["STUB"].as_u64(), Some(STUB));
    assert_eq!(rinput["TRAMPOLINE"].as_u64(), Some(TRAMPOLINE));
    assert_eq!(rinput["CAVE"].as_u64(), Some(CAVE));
    assert_eq!(rinput["CAVE_SIZE"].as_u64(), Some(CAVE_SIZE as u64));

    let base = rinput["base"].as_u64().unwrap();
    let get_cursor_info = rinput["getCursorInfo"].as_u64().unwrap();
    assert_eq!(
        to_hex(&original_hook(base)),
        rinput["originalHook"].as_str().unwrap()
    );
    assert_eq!(
        to_hex(&build_cave(base, get_cursor_info)),
        rinput["buildCave"].as_str().unwrap()
    );
}

/// Память `hl.exe` в миниатюре: `RInput.dll`, сдвинутая загрузчиком, и `user32` с экспортами, а
/// вокруг — `/proc` с процессом 4242 (`fakeGame` самотеста: `comm`, `maps`, `mem`).
struct FakeGame {
    rinput: RefCell<Vec<u8>>,
    user32: RefCell<Vec<u8>>,
    writes: RefCell<Vec<u64>>,
}

const B: u64 = 0x642c0000;
const U: u64 = 0x7b5b0000;

fn put_u32(buf: &mut [u8], at: usize, value: u32) {
    buf[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

impl FakeGame {
    fn new(hooked: bool, hook_bytes: Option<&[u8]>) -> FakeGame {
        let mut rinput = vec![0u8; 0x13000];
        let hook = hook_bytes.map_or_else(|| original_hook(B), <[u8]>::to_vec);
        rinput[HOOK as usize..HOOK as usize + hook.len()].copy_from_slice(&hook);
        rinput[0x154a..0x1550].fill(0xcc);

        let mut user32 = vec![0u8; 0x3000];
        put_u32(&mut user32, 0x3c, 0x80);
        put_u32(&mut user32, 0x80 + 0x78, 0x1000);
        put_u32(&mut user32, 0x80 + 0x7c, 0x300);
        for (offset, value) in [(20, 2), (24, 2), (28, 0x1100), (32, 0x1120), (36, 0x1140)] {
            put_u32(&mut user32, 0x1000 + offset, value);
        }
        put_u32(&mut user32, 0x1100, 0x2000);
        put_u32(&mut user32, 0x1104, 0x2100);
        put_u32(&mut user32, 0x1120, 0x1200);
        put_u32(&mut user32, 0x1124, 0x1220);
        user32[0x1142..0x1144].copy_from_slice(&1u16.to_le_bytes());
        user32[0x1200..0x1200 + 14].copy_from_slice(b"GetCursorInfo\0");
        user32[0x1220..0x1220 + 13].copy_from_slice(b"GetCursorPos\0");
        if hooked {
            user32[0x2100] = 0xe9;
            let rel = ((B + HOOK) as u32).wrapping_sub((U + 0x2105) as u32);
            put_u32(&mut user32, 0x2101, rel);
        } else {
            user32[0x2100..0x2105].copy_from_slice(&[0x8b, 0xff, 0x55, 0x8b, 0xec]);
        }
        FakeGame {
            rinput: RefCell::new(rinput),
            user32: RefCell::new(user32),
            writes: RefCell::new(Vec::new()),
        }
    }

    /// Область и смещение в ней; адрес вне областей — ошибка.
    fn region(&self, address: u64) -> Result<(&RefCell<Vec<u8>>, usize), String> {
        for (start, buf) in [(B, &self.rinput), (U, &self.user32)] {
            let len = buf.borrow().len() as u64;
            if address >= start && address < start + len {
                return Ok((buf, (address - start) as usize));
            }
        }
        Err(format!("адрес {address:#x} вне известных областей"))
    }

    fn rinput(&self) -> Vec<u8> {
        self.rinput.borrow().clone()
    }

    fn writes(&self) -> Vec<u64> {
        self.writes.borrow().clone()
    }
}

struct FakeMem<'a>(&'a FakeGame);

impl MemFile for FakeMem<'_> {
    fn read_at(&mut self, buf: &mut [u8], position: u64) -> Result<(), String> {
        let (region, offset) = self.0.region(position)?;
        buf.copy_from_slice(&region.borrow()[offset..offset + buf.len()]);
        Ok(())
    }

    fn write_at(&mut self, buf: &[u8], position: u64) -> Result<(), String> {
        self.0.writes.borrow_mut().push(position);
        let (region, offset) = self.0.region(position)?;
        region.borrow_mut()[offset..offset + buf.len()].copy_from_slice(buf);
        Ok(())
    }
}

const MAPS: [&str; 3] = [
    "642c0000-642c1000 r-xp 00000000 00:21 1 /games/hl1-won/RInput/RInput.dll",
    "642d1000-642d3000 rwxp 0000f000 00:21 1 /games/hl1-won/RInput/RInput.dll",
    "7b5b0000-7b678000 r-xp 00000000 00:21 2 /proton/lib/wine/i386-windows/user32.dll",
];

impl ProcFs for FakeGame {
    fn read_dir(&self, _dir: &str) -> Result<Vec<String>, String> {
        Ok(vec!["self".to_owned(), "4242".to_owned()])
    }

    fn read_to_string(&self, file: &str) -> Result<String, String> {
        match file {
            "/proc/4242/comm" => Ok("hl.exe\n".to_owned()),
            "/proc/4242/maps" => Ok(MAPS.join("\n")),
            other => Err(format!("нет файла {other}")),
        }
    }

    fn open_mem<'a>(&'a self, _file: &str) -> Result<Box<dyn MemFile + 'a>, String> {
        Ok(Box::new(FakeMem(self)))
    }
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

/// «HL1 Linux: патч курсора RInput пишет переход в обход только знакомой DLL».
#[test]
fn patch_writes_trampoline_only_for_known_dll() {
    let game = FakeGame::new(true, None);
    assert_eq!(
        patch_running_game("hl.exe", &game).unwrap(),
        PatchStatus::Patched
    );

    let rinput = game.rinput();
    assert_eq!(
        to_hex(&rinput[0x1530..0x1532]),
        "eb18",
        "хук начинается с короткого перехода"
    );
    assert_eq!(
        to_hex(&rinput[0x1547..0x154a]),
        "c20400",
        "ret 4 исходного хука цел"
    );
    assert_eq!(rinput[0x154a], 0xe9);
    assert_eq!(jump_target(&rinput, 0x154a, B + 0x154a), (B + CAVE) as u32);

    let cave = build_cave(B, U + 0x2000);
    assert_eq!(
        &rinput[CAVE as usize..CAVE as usize + cave.len()],
        cave.as_slice()
    );
    assert_eq!(
        u32::from_le_bytes([cave[12], cave[13], cave[14], cave[15]]),
        (U + 0x2000) as u32,
        "вызывается GetCursorInfo из user32"
    );
    assert_eq!(
        jump_target(&cave, 39, B + CAVE + 39),
        (B + TRAMPOLINE) as u32,
        "курсор показан: настоящая GetCursorPos"
    );
    assert_eq!(
        jump_target(&cave, cave.len() - 5, B + CAVE + cave.len() as u64 - 5),
        (B + 0x1534) as u32,
        "курсор скрыт: дальше код RInput"
    );
    let writes = game.writes();
    assert_eq!(
        writes.iter().position(|&at| at == B + HOOK),
        Some(writes.len() - 1),
        "переход на патч ставится последним"
    );
    assert_eq!(
        patch_running_game("hl.exe", &game).unwrap(),
        PatchStatus::Already
    );

    let early = FakeGame::new(false, None);
    assert_eq!(
        patch_running_game("hl.exe", &early).unwrap(),
        PatchStatus::Waiting,
        "RInput ещё не перехватил курсор"
    );
    assert!(early.writes().is_empty());

    let foreign = FakeGame::new(true, Some(&[0x90u8; 26]));
    assert_eq!(
        patch_running_game("hl.exe", &foreign).unwrap(),
        PatchStatus::Unsupported
    );
    assert!(
        foreign.writes().is_empty(),
        "чужую версию RInput не трогаем"
    );

    // Процесса с таким именем нет — ждём.
    assert_eq!(
        patch_running_game("hl2.exe", &game).unwrap(),
        PatchStatus::Waiting
    );
}

#[test]
fn maps_are_parsed_like_reference() {
    let text = [
        "642c0000-642c1000 r-xp 00000000 00:21 1 /games/My Games/RInput/RInput.dll",
        "",
        "7b5b0000-7b678000 r-xp 0000f000 00:21 2 /wine/user32.dll",
        "7c000000-7c001000 r-xp 00000000 00:21 3 Z:\\wine\\USER32.DLL",
        "7d000000-7d001000 rw-p 00000000 00:00 0",
        "",
    ]
    .join("\n");
    let maps = parse_maps(&text);
    assert_eq!(maps.len(), 4, "пустые строки пропускаются");
    assert_eq!(maps[0].start, 0x642c0000);
    assert_eq!(
        maps[0].file, "/games/My Games/RInput/RInput.dll",
        "пробелы в пути сохраняются"
    );
    assert_eq!(maps[1].offset, 0xf000);
    assert_eq!(maps[3].file, "");
    assert_eq!(
        image_base(&maps, "rinput.dll"),
        Some(0x642c0000),
        "без учёта регистра"
    );
    assert_eq!(
        image_base(&maps, "user32.dll"),
        Some(0x7c000000),
        "только отображение со смещением 0; разделитель \\ тоже"
    );
    assert_eq!(
        image_base(&maps, "input.dll"),
        None,
        "имя целиком, после разделителя"
    );
}

#[test]
fn merge_status_prefers_patched_over_waiting() {
    use PatchStatus::{Already, Patched, Unsupported, Waiting};
    assert_eq!(merge_status(Waiting, Patched), Patched);
    assert_eq!(merge_status(Waiting, Unsupported), Unsupported);
    assert_eq!(merge_status(Patched, Waiting), Patched);
    assert_eq!(merge_status(Unsupported, Already), Already);
    assert_eq!(merge_status(Unsupported, Waiting), Unsupported);
}
