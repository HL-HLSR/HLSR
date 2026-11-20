/* Общее для прокси d3d9.dll и opengl32.dll (proxy-d3d9.c, proxy-gl.c).
 *
 * Лаунчер кладёт прокси в папку игры под именем системной DLL, и игра
 * загружает её сама, как обёртки вроде ReShade или DXVK: загрузчик ищет DLL
 * сначала рядом с программой, а d3d9.dll и opengl32.dll не входят в
 * KnownDLLs. Под Wine нужна ещё замена WINEDLLOVERRIDES=<имя>=n,b. Прокси
 * передаёт вызовы настоящей DLL из системной папки, а кадр перед показом
 * рисует оверлей (overlay.h). Ни в какой другой процесс прокси не пишет.
 *
 * Папку данных оверлея (overlay.ini, overlay.log) прокси узнаёт из файла
 * hlsr-overlay.path рядом с собой: в нём путь в UTF-8. Нет файла — прокси
 * просто передаёт вызовы, игра работает как без неё.
 *
 * Функции без перехвата — переходники PROXY_THUNK: первый вызов находит
 * функцию в настоящей DLL, следующие идут прямо в неё. Адреса экспорта и
 * номера задаёт .def (его пишет build.sh).
 *
 * Перед включением определить PROXY_NAME (L"d3d9.dll") и PROXY_NAME_A.
 *
 * Основная сборка — clang (build.sh). Ветки _MSC_VER нужны для сборки MSVC
 * на Windows (build-msvc.cmd): атрибуты GCC пустые, переходники на
 * встроенном ассемблере MSVC, без CRT нужен _fltused. */

#if defined(_MSC_VER) && !defined(__clang__)
#define __attribute__(x)
#define __builtin_sqrtf(x) _mm_cvtss_f32(_mm_sqrt_ss(_mm_set_ss(x)))
#pragma warning(disable : 4068 4100 4152)
#pragma comment(linker, "/INCLUDE:_g_hlsrProxyMarker")
#include <intrin.h>
#include <xmmintrin.h>
int _fltused;
/* CopyMemory и ZeroMemory MSVC превращает в вызовы memcpy и memset. */
#pragma function(memcpy, memset)
void *__cdecl memcpy(void *to, const void *from, size_t size) {
  __movsb((unsigned char *)to, (const unsigned char *)from, size);
  return to;
}
void *__cdecl memset(void *to, int value, size_t size) {
  __stosb((unsigned char *)to, (unsigned char)value, size);
  return to;
}
#else
#pragma clang diagnostic ignored "-Wunused-function"
#endif

#include <windows.h>

/* Метка прокси лаунчера: по этой строке livesplit-overlay.js отличает свою
 * d3d9.dll или opengl32.dll в папке игры от чужой (DXVK, ReShade) и чужую
 * не трогает. */
__attribute__((used)) const char g_hlsrProxyMarker[] = "HLSR-OVERLAY-PROXY-1";

static HINSTANCE g_self;
static HMODULE g_real;
static volatile LONG g_realState; /* 0 — не загружали, 1 — загружается, 2 — готово */

/* Для OutputDebugString (DebugView, wine +debugstr): журнала оверлея к
 * моменту ошибок прокси может ещё не быть. */
static void proxy_debug(const char *format, ...) {
  char line[512];
  va_list args;
  va_start(args, format);
  int length = wvsprintfA(line, format, args);
  va_end(args);
  line[length++] = '\n';
  line[length] = 0;
  OutputDebugStringA(line);
}

#ifdef HLSR_D3D9
/* Vulkan для Ghosting: при HLSR_D3D9_BACKEND=vulkan настоящей d3d9.dll
 * служит DXVK из backend\dxvk_d3d9.dll рядом с программой игры (hl2.exe;
 * сам прокси может лежать и в bin\). Переменную задаёт лаунчер, когда в
 * «Параметрах запуска» включён Vulkan. 0 — Vulkan не просили или DXVK не
 * загрузился: тогда, как и без переменной, системная d3d9.dll (DirectX 9). */
static HMODULE proxy_vulkan(void) {
  static const WCHAR backend[] = L"backend\\dxvk_d3d9.dll";
  WCHAR mode[8];
  DWORD size = GetEnvironmentVariableW(L"HLSR_D3D9_BACKEND", mode, ARRAYSIZE(mode));
  if (size != 6 || lstrcmpW(mode, L"vulkan") != 0) return 0;
  WCHAR path[MAX_PATH];
  DWORD length = GetModuleFileNameW(0, path, MAX_PATH);
  if (!length || length >= MAX_PATH) return 0;
  while (length > 0 && path[length - 1] != '\\' && path[length - 1] != '/') length--;
  if (length + ARRAYSIZE(backend) > MAX_PATH) return 0;
  CopyMemory(path + length, backend, sizeof backend);
  HMODULE real = LoadLibraryW(path);
  if (!real) proxy_debug("hlsr: %ls не загрузилась (ошибка %lu), будет DirectX 9", path, GetLastError());
  return real;
}
#endif

/* Настоящая DLL из системной папки (у 32-битной игры на 64-битной Windows
 * это SysWOW64), у d3d9.dll — или DXVK (proxy_vulkan). Грузится при первом
 * вызове, а не в DllMain: под замком загрузчика LoadLibrary звать нельзя. */
static HMODULE proxy_real(void) {
  if (g_realState == 2) return g_real;
  if (InterlockedCompareExchange(&g_realState, 1, 0) != 0) {
    while (g_realState != 2) Sleep(0);
    return g_real;
  }
  /* Игра выгружает d3d9.dll и opengl32.dll при выходе и при _restart (вместе
   * с shaderapidx9.dll или hw.dll), а поток оверлея и перехваты в таблицах
   * методов остаются: после выгрузки они попадали бы в пустую память, и игра
   * падала на выходе. Прокси остаётся в процессе до его конца, как и
   * настоящая DLL, которую он не освобождает; новая загрузка после _restart
   * получает тот же экземпляр. */
  HMODULE pinned;
  if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN,
                          (LPCWSTR)(void *)proxy_real, &pinned))
    proxy_debug("hlsr: прокси не закреплён в процессе (ошибка %lu)", GetLastError());
  WCHAR path[MAX_PATH];
  UINT length = GetSystemDirectoryW(path, MAX_PATH - 32);
  HMODULE real = 0;
  /* opengl32.dll собирается из того же кода байт в байт: Vulkan только у d3d9. */
#ifdef HLSR_D3D9
  real = proxy_vulkan();
  if (!real)
#endif
  if (length && length < MAX_PATH - 32) {
    lstrcatW(path, L"\\" PROXY_NAME);
    real = LoadLibraryW(path);
  }
  if (real == g_self) {
    /* Загрузчик вернул нас же: вызовы ушли бы по кругу. */
    FreeLibrary(real);
    real = 0;
  }
  if (!real) proxy_debug("hlsr: системная %ls не загрузилась (ошибка %lu)", PROXY_NAME, GetLastError());
  g_real = real;
  InterlockedExchange(&g_realState, 2);
  return real;
}

/* Функция настоящей DLL по имени; 0 — её там нет. */
static void *proxy_function(const char *name) {
  HMODULE real = proxy_real();
  return real ? (void *)GetProcAddress(real, name) : 0;
}

/* Заглушка для функций, которых нет в настоящей DLL. */
static int proxy_missing(void) { return 0; }

/* Зовёт переходник при первом вызове функции: кладёт адрес в слот. */
__attribute__((used)) void *WINAPI proxy_resolve(void **slot, const char *name) {
  void *fn = proxy_function(name);
  if (!fn) {
    proxy_debug("hlsr: в системной %ls нет %s", PROXY_NAME, name);
    fn = (void *)proxy_missing;
  }
  *slot = fn;
  return fn;
}

/* Переходник: слот с адресом настоящей функции и её имя. Стек не
 * трогается, так что соглашение о вызове не важно; ecx и edx сохраняются на
 * случай, если функция получает аргументы в них. */
#if defined(_MSC_VER) && !defined(__clang__)
/* MSVC: меток в макросе нет — слот сразу смотрит на resolve_<имя>, тот
 * находит функцию, кладёт её в слот и прыгает в неё. */
#define PROXY_THUNK(name)                                                      \
  const char g_name_##name[] = #name;                                          \
  void resolve_##name(void);                                                   \
  void *g_slot_##name = (void *)resolve_##name;                                \
  __declspec(naked) void resolve_##name(void) {                                \
    __asm push ecx                                                             \
    __asm push edx                                                             \
    __asm push offset g_name_##name                                            \
    __asm push offset g_slot_##name                                            \
    __asm call proxy_resolve                                                   \
    __asm pop edx                                                              \
    __asm pop ecx                                                              \
    __asm jmp eax                                                              \
  }                                                                            \
  __declspec(naked) void thunk_##name(void) {                                  \
    __asm jmp dword ptr [g_slot_##name]                                        \
  }
#else
#define PROXY_THUNK(name)                                                      \
  __attribute__((used)) void *g_slot_##name;                                   \
  __attribute__((used)) const char g_name_##name[] = #name;                    \
  __attribute__((naked, used)) void thunk_##name(void) {                       \
    __asm__ volatile(                                                          \
      "movl _g_slot_" #name ", %eax\n\t"                                       \
      "testl %eax, %eax\n\t"                                                   \
      "jnz 1f\n\t"                                                             \
      "pushl %ecx\n\t"                                                         \
      "pushl %edx\n\t"                                                         \
      "pushl $_g_name_" #name "\n\t"                                           \
      "pushl $_g_slot_" #name "\n\t"                                           \
      "calll _proxy_resolve@8\n\t"                                             \
      "popl %edx\n\t"                                                          \
      "popl %ecx\n"                                                            \
      "1:\n\t"                                                                 \
      "jmpl *%eax");                                                           \
  }
#endif

/* ---- Папка данных оверлея. ---- */

/* Из hlsr-overlay.path рядом с прокси, с замыкающей косой чертой. */
static int proxy_data_dir(WCHAR *out, int capacity) {
  WCHAR sidecar[MAX_PATH];
  DWORD length = GetModuleFileNameW(g_self, sidecar, MAX_PATH);
  if (!length || length >= MAX_PATH) return 0;
  while (length > 0 && sidecar[length - 1] != '\\' && sidecar[length - 1] != '/') length--;
  sidecar[length] = 0;
  if (length + 20 >= MAX_PATH) return 0;
  lstrcatW(sidecar, L"hlsr-overlay.path");
  HANDLE file = CreateFileW(sidecar, GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, 0, OPEN_EXISTING, 0, 0);
  if (file == INVALID_HANDLE_VALUE) return 0;
  char text[MAX_PATH * 3];
  DWORD size = 0;
  BOOL ok = ReadFile(file, text, sizeof text - 1, &size, 0);
  CloseHandle(file);
  if (!ok) return 0;
  while (size > 0 && (text[size - 1] == '\r' || text[size - 1] == '\n' || text[size - 1] == ' ')) size--;
  if (size >= 3 && (BYTE)text[0] == 0xEF && (BYTE)text[1] == 0xBB && (BYTE)text[2] == 0xBF) {
    for (DWORD i = 3; i < size; i++) text[i - 3] = text[i];
    size -= 3;
  }
  if (!size) return 0;
  int written = MultiByteToWideChar(CP_UTF8, 0, text, (int)size, out, capacity - 2);
  if (written <= 0) return 0;
  if (out[written - 1] != '\\' && out[written - 1] != '/') out[written++] = '\\';
  out[written] = 0;
  return 1;
}

/* Свои дела прокси при загрузке (proxy-d3d9.c: замок таблиц). */
static void proxy_attach(void);

BOOL WINAPI DllMain(HINSTANCE instance, DWORD reason, LPVOID reserved) {
  (void)reserved;
  if (reason == DLL_PROCESS_ATTACH) {
    g_self = instance;
    DisableThreadLibraryCalls(instance);
    proxy_attach();
  }
  return TRUE;
}
