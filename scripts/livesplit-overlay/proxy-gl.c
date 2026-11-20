/* opengl32.dll — прокси OpenGL для оверлея LiveSplit (HL1 WON, GoldSrc).
 * Общее устройство — в proxy.h, отрисовка — в overlay.h.
 *
 * Все функции gl* и wgl* — переходники к настоящей opengl32.dll
 * (proxy-gl-exports.inc). wglSwapBuffers сначала зовёт оверлей.
 *
 * GoldSrc меняет буферы не wglSwapBuffers, а SwapBuffers из gdi32, который
 * в настоящую opengl32.dll не заходит. Поэтому поток оверлея раз в секунду
 * находит в таблицах импорта модулей из папки игры (hw.dll, sw.dll) адрес
 * gdi32!SwapBuffers и ставит туда наш. Код gdi32 и чужие модули не
 * меняются: правится только ячейка в импорте своих модулей игры. */

#define PROXY_NAME L"opengl32.dll"
#define PROXY_NAME_A "opengl32.dll"
#define HLSR_GL
#include "proxy.h"
#include "overlay.h"

static void proxy_attach(void) {}

/* ---- wglSwapBuffers. ---- */

BOOL WINAPI hook_wglSwapBuffers(HDC hdc) {
  static BOOL (WINAPI *real)(HDC);
  if (!real) real = (BOOL (WINAPI *)(HDC))proxy_function("wglSwapBuffers");
  if (!real) return FALSE;
  overlay_start();
  int outer = overlay_gl_before_swap(hdc, proxy_real());
  BOOL result = real(hdc);
  overlay_gl_after_swap(outer);
  return result;
}

/* ---- SwapBuffers из gdi32 в импорте модулей игры. ---- */

static BOOL (WINAPI *volatile o_SwapBuffers)(HDC);

static BOOL WINAPI hook_gdi_SwapBuffers(HDC hdc) {
  overlay_start();
  int outer = overlay_gl_before_swap(hdc, proxy_real());
  BOOL result = o_SwapBuffers(hdc);
  overlay_gl_after_swap(outer);
  return result;
}

/* Заменяет в импорте из gdi32.dll ячейки с адресом real на hook. Сколько
 * ячеек заменено. */
static int patch_imports(HMODULE module, void *real, void *hook) {
  BYTE *base = (BYTE *)module;
  IMAGE_DOS_HEADER *dos = (IMAGE_DOS_HEADER *)base;
  if (dos->e_magic != IMAGE_DOS_SIGNATURE) return 0;
  IMAGE_NT_HEADERS *nt = (IMAGE_NT_HEADERS *)(base + dos->e_lfanew);
  if (nt->Signature != IMAGE_NT_SIGNATURE) return 0;
  IMAGE_DATA_DIRECTORY dir = nt->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT];
  if (!dir.VirtualAddress) return 0;
  int patched = 0;
  for (IMAGE_IMPORT_DESCRIPTOR *d = (IMAGE_IMPORT_DESCRIPTOR *)(base + dir.VirtualAddress); d->Name; d++) {
    if (lstrcmpiA((const char *)(base + d->Name), "gdi32.dll")) continue;
    for (void **slot = (void **)(base + d->FirstThunk); *slot; slot++) {
      if (*slot != real) continue;
      DWORD old;
      if (!VirtualProtect(slot, sizeof *slot, PAGE_READWRITE, &old)) continue;
      InterlockedExchangePointer((PVOID volatile *)slot, hook);
      VirtualProtect(slot, sizeof *slot, old, &old);
      patched++;
    }
  }
  return patched;
}

/* Путь начинается с каталога dir (без учёта регистра). */
static int under_dir(const WCHAR *path, const WCHAR *dir, int dirLength) {
  for (int i = 0; i < dirLength; i++) {
    WCHAR a = path[i], b = dir[i];
    if (a >= 'A' && a <= 'Z') a += 32;
    if (b >= 'A' && b <= 'Z') b += 32;
    if (a != b) return 0;
  }
  return 1;
}

static int g_swapPatched;

/* Раз в секунду из потока оверлея: модули игры загружаются не сразу. */
static void proxy_poll(void) {
  HMODULE gdi = GetModuleHandleW(L"gdi32.dll");
  void *real = gdi ? (void *)GetProcAddress(gdi, "SwapBuffers") : 0;
  if (!real) return;
  WCHAR exe[MAX_PATH];
  DWORD length = GetModuleFileNameW(0, exe, MAX_PATH);
  if (!length || length >= MAX_PATH) return;
  while (length > 0 && exe[length - 1] != '\\' && exe[length - 1] != '/') length--;
  HANDLE snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, GetCurrentProcessId());
  if (snapshot == INVALID_HANDLE_VALUE) return;
  MODULEENTRY32W entry;
  entry.dwSize = sizeof entry;
  /* Адрес настоящей функции запоминаем до первой замены: хук сразу ею пользуется. */
  if (!o_SwapBuffers) o_SwapBuffers = (BOOL (WINAPI *)(HDC))real;
  for (BOOL ok = Module32FirstW(snapshot, &entry); ok; ok = Module32NextW(snapshot, &entry)) {
    if (entry.hModule == g_self || !under_dir(entry.szExePath, exe, (int)length)) continue;
    int patched = patch_imports(entry.hModule, real, (void *)hook_gdi_SwapBuffers);
    if (patched) {
      g_swapPatched += patched;
      log_line("opengl: SwapBuffers в %ls заменён (%d, всего %d)", entry.szModule, patched, g_swapPatched);
    }
  }
  CloseHandle(snapshot);
}

/* ---- Экспорт. ---- */

#define X(name) PROXY_THUNK(name)
#include "proxy-gl-exports.inc"
#undef X
