//! Исправление курсора в меню для RInput 1.31 без замены DLL (`rinput-menu-fix.js`).
//!
//! RInput перехватывает `GetCursorPos` и всегда отдаёт игре позицию из raw input. В обзоре мышью
//! так и нужно, но в меню (курсор показан) игра получает не то место, где курсор на экране, и
//! клики промахиваются. Патч в памяти `hl.exe`: пока курсор показан, `GetCursorPos` идёт в
//! настоящую функцию через трамплин RInput; пока скрыт — RInput работает как раньше.
//!
//! Адреса — смещения внутри `RInput.dll` 1.31 (sequential edition). Перед записью байты сверяются
//! с ожидаемыми, другую версию патч не трогает. Доступ к памяти зависит от ОС (трейт
//! [`ProcessMemory`]): Linux — `/proc/<pid>/mem` (`linux::rinput_menu_fix`), Windows —
//! `ReadProcessMemory`/`WriteProcessMemory` (`rinput_menu_fix_win`). Здесь — только логика патча.

/// `HOOK`: `CRawInput::hGetCursorPos`.
pub const HOOK: u64 = 0x1530;
/// `STUB`: int3-выравнивание за его `ret 4` (6 байт).
pub const STUB: u64 = 0x154a;
/// `TRAMPOLINE`: `TrmpGetCursorPos` — настоящая `GetCursorPos`.
pub const TRAMPOLINE: u64 = 0x12b0;
/// `RAW_X`: `CRawInput::x`, `CRawInput::y`.
pub const RAW_X: u64 = 0x12b74;
/// `CAVE`: нули в конце последней страницы `.text`.
pub const CAVE: u64 = 0xc180;
/// `CAVE_SIZE`.
pub const CAVE_SIZE: usize = 64;

/// Итог [`patch_memory`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchStatus {
    /// Патч записан.
    Patched,
    /// Уже стоит.
    Already,
    /// RInput ещё не поставил свои перехваты.
    Waiting,
    /// Не та версия RInput.
    Unsupported,
}

impl PatchStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            PatchStatus::Patched => "patched",
            PatchStatus::Already => "already",
            PatchStatus::Waiting => "waiting",
            PatchStatus::Unsupported => "unsupported",
        }
    }
}

/// Память процесса: база модуля по имени, чтение и запись по адресу. Методы могут открывать
/// ресурсы лениво (Linux — файл при первом чтении), поэтому `&mut self`.
pub trait ProcessMemory {
    /// `moduleBase(name)`: база загруженного модуля или `None`.
    fn module_base(&mut self, name: &str) -> Result<Option<u64>, String>;
    /// `read(address, length)`: ровно `length` байт.
    fn read(&mut self, address: u64, length: usize) -> Result<Vec<u8>, String>;
    /// `write(address, buf)`.
    fn write(&mut self, address: u64, buf: &[u8]) -> Result<(), String>;
}

fn u32_le(value: u32) -> [u8; 4] {
    value.to_le_bytes()
}

/// `rel32(from, to)`: 32-битное смещение перехода (`to - from`), как `>>> 0`.
fn rel32(from: u64, to: u64) -> [u8; 4] {
    u32_le((to as u32).wrapping_sub(from as u32))
}

/// `originalHook(base)`: исходный `hGetCursorPos` с учётом сдвига DLL.
pub fn original_hook(base: u64) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0x8b, 0x44, 0x24, 0x04, 0x8b, 0x0d]);
    out.extend_from_slice(&u32_le((base + RAW_X) as u32));
    out.extend_from_slice(&[0x8b, 0x15]);
    out.extend_from_slice(&u32_le((base + RAW_X + 4) as u32));
    out.extend_from_slice(&[0x89, 0x08, 0x89, 0x50, 0x04, 0x33, 0xc0, 0xc2, 0x04, 0x00]);
    out
}

/// `buildCave(base, getCursorInfo)`: `stdcall BOOL hook(POINT *p)` — `CURSORINFO` на стеке, затем
/// выбор пути (настоящая `GetCursorPos`, если курсор показан; иначе код RInput).
pub fn build_cave(base: u64, get_cursor_info: u64) -> Vec<u8> {
    let at = base + CAVE;
    let mut head = Vec::new();
    head.extend_from_slice(&[0x83, 0xec, 0x14]); // sub esp, 20
    head.extend_from_slice(&[0xc7, 0x04, 0x24, 0x14, 0x00, 0x00, 0x00]); // mov [esp], 20
    head.push(0x54); // push esp
    head.push(0xb8); // mov eax, GetCursorInfo
    head.extend_from_slice(&u32_le(get_cursor_info as u32));
    head.extend_from_slice(&[0xff, 0xd0]); // call eax
    head.extend_from_slice(&[0x85, 0xc0]); // test eax, eax
    head.extend_from_slice(&[0x74, 0x16]); // jz virtual
    head.extend_from_slice(&[0xf6, 0x44, 0x24, 0x04, 0x01]); // test byte [esp+4], 1
    head.extend_from_slice(&[0x74, 0x0f]); // jz virtual
    head.extend_from_slice(&[0x83, 0x7c, 0x24, 0x08, 0x00]); // cmp dword [esp+8], 0
    head.extend_from_slice(&[0x74, 0x08]); // je virtual
    head.extend_from_slice(&[0x83, 0xc4, 0x14]); // add esp, 20

    let mut real = vec![0xe9];
    real.extend_from_slice(&rel32(at + head.len() as u64 + 5, base + TRAMPOLINE));

    let virtual_start = head.len() + real.len();
    let mut virtual_path = Vec::new();
    virtual_path.extend_from_slice(&[0x83, 0xc4, 0x14]); // add esp, 20
    virtual_path.extend_from_slice(&[0x8b, 0x44, 0x24, 0x04]); // mov eax, [esp+4]
    virtual_path.push(0xe9);
    virtual_path.extend_from_slice(&rel32(at + virtual_start as u64 + 12, base + HOOK + 4));

    let mut out = head;
    out.extend_from_slice(&real);
    out.extend_from_slice(&virtual_path);
    out
}

fn read_u32(mem: &mut dyn ProcessMemory, address: u64) -> Result<u32, String> {
    let bytes = mem.read(address, 4)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// `exportAddress(mem, base, name)`: адрес экспорта из таблицы PE прямо в памяти процесса;
/// `None` — нет имени или форвард в другую DLL.
pub fn export_address(
    mem: &mut dyn ProcessMemory,
    base: u64,
    name: &str,
) -> Result<Option<u64>, String> {
    let pe = base + u64::from(read_u32(mem, base + 0x3c)?);
    let dir_rva = read_u32(mem, pe + 0x78)?;
    let dir_size = read_u32(mem, pe + 0x7c)?;
    let dir = mem.read(base + u64::from(dir_rva), 40)?;
    let le = |offset: usize| {
        u32::from_le_bytes([
            dir[offset],
            dir[offset + 1],
            dir[offset + 2],
            dir[offset + 3],
        ])
    };
    let count = le(24);
    let functions = mem.read(base + u64::from(le(28)), le(20) as usize * 4)?;
    let names = mem.read(base + u64::from(le(32)), count as usize * 4)?;
    let ordinals = mem.read(base + u64::from(le(36)), count as usize * 2)?;
    let name_bytes = name.as_bytes();
    for i in 0..count as usize {
        let name_rva = u32::from_le_bytes([
            names[i * 4],
            names[i * 4 + 1],
            names[i * 4 + 2],
            names[i * 4 + 3],
        ]);
        let text = mem.read(base + u64::from(name_rva), name_bytes.len() + 1)?;
        if &text[..name_bytes.len()] != name_bytes || text[name_bytes.len()] != 0 {
            continue;
        }
        let ordinal = u16::from_le_bytes([ordinals[i * 2], ordinals[i * 2 + 1]]) as usize;
        let rva = u32::from_le_bytes([
            functions[ordinal * 4],
            functions[ordinal * 4 + 1],
            functions[ordinal * 4 + 2],
            functions[ordinal * 4 + 3],
        ]);
        if rva >= dir_rva && rva < dir_rva + dir_size {
            return Ok(None); // форвард в другую DLL
        }
        return Ok(Some(base + u64::from(rva)));
    }
    Ok(None)
}

/// `patchMemory(mem)`: переводит перехваты RInput на трамплин через пещеру кода.
pub fn patch_memory(mem: &mut dyn ProcessMemory) -> Result<PatchStatus, String> {
    let (Some(base), Some(user32)) = (
        mem.module_base("rinput.dll")?,
        mem.module_base("user32.dll")?,
    ) else {
        return Ok(PatchStatus::Waiting);
    };
    let hook = mem.read(base + HOOK, 2)?;
    if hook[0] == 0xeb && u64::from(hook[1]) == STUB - HOOK - 2 {
        return Ok(PatchStatus::Already);
    }
    let (Some(get_cursor_pos), Some(get_cursor_info)) = (
        export_address(mem, user32, "GetCursorPos")?,
        export_address(mem, user32, "GetCursorInfo")?,
    ) else {
        return Ok(PatchStatus::Unsupported);
    };
    // Перехват стоит, когда GetCursorPos начинается с jmp на hGetCursorPos.
    let entry = mem.read(get_cursor_pos, 5)?;
    let rel = i32::from_le_bytes([entry[1], entry[2], entry[3], entry[4]]);
    let target = (get_cursor_pos as u32)
        .wrapping_add(5)
        .wrapping_add(rel as u32);
    if entry[0] != 0xe9 || target != (base + HOOK) as u32 {
        return Ok(PatchStatus::Waiting);
    }
    let original = original_hook(base);
    if mem.read(base + HOOK, original.len())? != original {
        return Ok(PatchStatus::Unsupported);
    }
    if mem.read(base + STUB, 6)?.iter().any(|&b| b != 0xcc) {
        return Ok(PatchStatus::Unsupported);
    }
    if mem.read(base + CAVE, CAVE_SIZE)?.iter().any(|&b| b != 0) {
        return Ok(PatchStatus::Unsupported);
    }
    // Сначала код, потом переход на него: двухбайтовый jmp ставится последним.
    mem.write(base + CAVE, &build_cave(base, get_cursor_info))?;
    let mut stub = vec![0xe9];
    stub.extend_from_slice(&rel32(base + STUB + 5, base + CAVE));
    mem.write(base + STUB, &stub)?;
    mem.write(base + HOOK, &[0xeb, (STUB - HOOK - 2) as u8])?;
    Ok(PatchStatus::Patched)
}

/// `mergeStatus(result, status)`: `patched`/`already` важнее `waiting`.
pub fn merge_status(result: PatchStatus, status: PatchStatus) -> PatchStatus {
    if matches!(status, PatchStatus::Patched | PatchStatus::Already)
        || result == PatchStatus::Waiting
    {
        status
    } else {
        result
    }
}

/// Сессия патча (`openSession()` в Windows): доступ к памяти держится между опросами, пока RInput
/// ставит перехваты. Ошибка — как отказ промиса `session.patch`.
pub trait MenuSession: Send {
    /// `patch(processName)`: все процессы игры с таким именем.
    fn patch(&mut self, process_name: &str) -> Result<PatchStatus, String>;
    /// `close()`.
    fn close(&mut self);
}
