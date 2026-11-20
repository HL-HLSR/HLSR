"use strict";

// Исправление курсора в меню для RInput 1.31 без замены DLL.
//
// RInput перехватывает GetCursorPos и всегда отдаёт игре позицию, собранную
// из raw input. В обзоре мышью так и нужно, но в меню (курсор показан) игра
// получает не то место, где курсор на экране, и клики промахиваются.
// Патч в памяти hl.exe: пока курсор показан, GetCursorPos идёт в настоящую
// функцию через трамплин RInput; пока скрыт — RInput работает как раньше.
//
// Адреса ниже — смещения внутри RInput.dll 1.31 (sequential edition). Перед
// записью байты сверяются с ожидаемыми, другую версию патч не трогает.
//
// Сам доступ к памяти зависит от ОС: в Linux это /proc/<pid>/mem
// (linux/rinput-menu-fix.js), в Windows — ReadProcessMemory/WriteProcessMemory
// (rinput-menu-fix-win.js). Здесь только логика патча над объектом памяти
// { moduleBase(name), read(address, length), write(address, buf) };
// его методы могут быть и синхронными, и асинхронными.

const HOOK = 0x1530;          // CRawInput::hGetCursorPos
const STUB = 0x154a;          // int3-выравнивание за его ret 4 (6 байт)
const TRAMPOLINE = 0x12b0;    // TrmpGetCursorPos: настоящая GetCursorPos
const RAW_X = 0x12b74;        // CRawInput::x, CRawInput::y
const CAVE = 0xc180;          // нули в конце последней страницы .text
const CAVE_SIZE = 64;

function u32(value) {
  const buf = Buffer.alloc(4);
  buf.writeUInt32LE(value >>> 0);
  return buf;
}

function rel32(from, to) { return u32(to - from); }

// Исходный hGetCursorPos с учётом того, куда загрузчик сдвинул DLL.
function originalHook(base) {
  return Buffer.concat([
    Buffer.from("8b442404" + "8b0d", "hex"), u32(base + RAW_X),
    Buffer.from("8b15", "hex"), u32(base + RAW_X + 4),
    Buffer.from("890889500433c0c20400", "hex"),
  ]);
}

// stdcall BOOL hook(POINT *p): CURSORINFO на стеке, затем выбор пути.
function buildCave(base, getCursorInfo) {
  const at = base + CAVE;
  const head = Buffer.concat([
    Buffer.from("83ec14", "hex"),             // sub esp, 20
    Buffer.from("c7042414000000", "hex"),     // mov [esp], 20 (cbSize)
    Buffer.from("54", "hex"),                 // push esp
    Buffer.from("b8", "hex"), u32(getCursorInfo), // mov eax, GetCursorInfo
    Buffer.from("ffd0", "hex"),               // call eax
    Buffer.from("85c0", "hex"),               // test eax, eax
    Buffer.from("7416", "hex"),               // jz virtual
    Buffer.from("f644240401", "hex"),         // test byte [esp+4], CURSOR_SHOWING
    Buffer.from("740f", "hex"),               // jz virtual
    Buffer.from("837c240800", "hex"),         // cmp dword [esp+8], 0 (hCursor)
    Buffer.from("7408", "hex"),               // je virtual
    Buffer.from("83c414", "hex"),             // add esp, 20
  ]);
  const real = Buffer.concat([Buffer.from("e9", "hex"), rel32(at + head.length + 5, base + TRAMPOLINE)]);
  const virtualStart = head.length + real.length;
  const virtualPath = Buffer.concat([
    Buffer.from("83c414", "hex"),             // add esp, 20
    Buffer.from("8b442404", "hex"),           // mov eax, [esp+4] — первая инструкция хука
    Buffer.from("e9", "hex"), rel32(at + virtualStart + 12, base + HOOK + 4),
  ]);
  return Buffer.concat([head, real, virtualPath]);
}

// Таблица экспорта PE прямо из памяти процесса.
async function exportAddress(mem, base, name) {
  const pe = base + (await mem.read(base + 0x3c, 4)).readUInt32LE(0);
  const dirRva = (await mem.read(pe + 0x78, 4)).readUInt32LE(0);
  const dirSize = (await mem.read(pe + 0x7c, 4)).readUInt32LE(0);
  const dir = await mem.read(base + dirRva, 40);
  const count = dir.readUInt32LE(24);
  const functions = await mem.read(base + dir.readUInt32LE(28), dir.readUInt32LE(20) * 4);
  const names = await mem.read(base + dir.readUInt32LE(32), count * 4);
  const ordinals = await mem.read(base + dir.readUInt32LE(36), count * 2);
  for (let i = 0; i < count; i++) {
    const text = await mem.read(base + names.readUInt32LE(i * 4), name.length + 1);
    if (text.toString("latin1", 0, name.length) !== name || text[name.length] !== 0) continue;
    const rva = functions.readUInt32LE(ordinals.readUInt16LE(i * 2) * 4);
    if (rva >= dirRva && rva < dirRva + dirSize) return null; // форвард в другую DLL
    return base + rva;
  }
  return null;
}

// "patched" — патч записан, "already" — уже стоит, "waiting" — RInput ещё не
// поставил свои перехваты, "unsupported" — не та версия RInput.
async function patchMemory(mem) {
  const base = await mem.moduleBase("rinput.dll");
  const user32 = await mem.moduleBase("user32.dll");
  if (base == null || user32 == null) return "waiting";
  const hook = await mem.read(base + HOOK, 2);
  if (hook[0] === 0xeb && hook[1] === STUB - HOOK - 2) return "already";
  const getCursorPos = await exportAddress(mem, user32, "GetCursorPos");
  const getCursorInfo = await exportAddress(mem, user32, "GetCursorInfo");
  if (getCursorPos == null || getCursorInfo == null) return "unsupported";
  // Перехват стоит, когда GetCursorPos начинается с jmp на hGetCursorPos.
  const entry = await mem.read(getCursorPos, 5);
  if (entry[0] !== 0xe9 || ((getCursorPos + 5 + entry.readInt32LE(1)) >>> 0) !== base + HOOK) return "waiting";
  const original = originalHook(base);
  if (!(await mem.read(base + HOOK, original.length)).equals(original)) return "unsupported";
  if ((await mem.read(base + STUB, 6)).some((b) => b !== 0xcc)) return "unsupported";
  if ((await mem.read(base + CAVE, CAVE_SIZE)).some((b) => b !== 0)) return "unsupported";
  // Сначала код, потом переход на него: двухбайтовый jmp ставится последним.
  await mem.write(base + CAVE, buildCave(base, getCursorInfo));
  await mem.write(base + STUB, Buffer.concat([Buffer.from("e9", "hex"), rel32(base + STUB + 5, base + CAVE)]));
  await mem.write(base + HOOK, Buffer.from([0xeb, STUB - HOOK - 2]));
  return "patched";
}

// Несколько процессов игры: "patched"/"already" важнее "waiting".
function mergeStatus(result, status) {
  return status === "patched" || status === "already" || result === "waiting" ? status : result;
}

module.exports = {
  patchMemory,
  mergeStatus,
  buildCave,
  originalHook,
  exportAddress,
  CAVE,
  CAVE_SIZE,
  HOOK,
  STUB,
  TRAMPOLINE,
};
