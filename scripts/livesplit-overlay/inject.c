/* Помощник оверлея LiveSplit для лаунчера, только Linux (Wine).
 *
 *   hlsr-overlay64.exe capture --out ... --dll hlsr-share64.dll [--status ...]
 *   hlsr-overlay64.exe attach --dll C:\...\hlsr-share64.dll
 *     Под Wine окно рисуется в память процесса LiveSplit, поэтому в
 *     LiveSplit.exe внедряется hlsr-share64.dll (share.c), а снимок берётся из
 *     общей памяти: заголовок "HLSRLS1\0", ширина и высота (uint32 LE), затем
 *     строки BGRX сверху вниз. Нужна та же разрядность, что у LiveSplit, поэтому
 *     сборка 64-битная. На Windows этого файла нет: там окно снимает сам
 *     лаунчер (src/main/game-delivery/livesplit-window-capture.js).
 *
 * Оверлей в игру этот помощник не внедряет: игра сама загружает прокси
 * d3d9.dll или opengl32.dll из своей папки (proxy.h).
 *
 * Коды выхода: 0 — готово, 2 — LiveSplit не найден, 3 — нет доступа к
 * процессу или снимок не получился, 4 — DLL не загрузилась, 5 — неверные
 * аргументы.
 *
 * С --status <путь> код выхода ещё и записывается в этот файл текстом. Под
 * Proton сессия umu не завершается, пока в префиксе работает что-то ещё
 * (LiveSplit, игра), поэтому лаунчер на Linux ждёт этот файл, а не выход. */

#include "common.h"
#include <shellapi.h>

static void say(const char *format, ...) {
  char line[1024];
  va_list args;
  va_start(args, format);
  int length = wvsprintfA(line, format, args);
  va_end(args);
  line[length++] = '\r';
  line[length++] = '\n';
  DWORD written;
  WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), line, length, &written, 0);
}

static int same_name(const WCHAR *a, const WCHAR *b) {
  for (;; a++, b++) {
    WCHAR x = *a, y = *b;
    if (x >= 'A' && x <= 'Z') x += 32;
    if (y >= 'A' && y <= 'Z') y += 32;
    if (x != y) return 0;
    if (!x) return 1;
  }
}

static const WCHAR *base_name(const WCHAR *path) {
  const WCHAR *name = path;
  for (const WCHAR *p = path; *p; p++) if (*p == '\\' || *p == '/') name = p + 1;
  return name;
}

static int has_module(DWORD pid, const WCHAR *name) {
  HANDLE snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid);
  if (snapshot == INVALID_HANDLE_VALUE) return -1;
  MODULEENTRY32W entry;
  entry.dwSize = sizeof entry;
  int found = 0;
  for (BOOL ok = Module32FirstW(snapshot, &entry); ok; ok = Module32NextW(snapshot, &entry))
    if (same_name(entry.szModule, name)) { found = 1; break; }
  CloseHandle(snapshot);
  return found;
}

static int inject(DWORD pid, const WCHAR *dll) {
  HANDLE process = OpenProcess(PROCESS_CREATE_THREAD | PROCESS_QUERY_INFORMATION | PROCESS_VM_OPERATION |
                               PROCESS_VM_WRITE | PROCESS_VM_READ, FALSE, pid);
  if (!process) { say("нет доступа к процессу %lu (ошибка %lu)", pid, GetLastError()); return 3; }
  SIZE_T size = (lstrlenW(dll) + 1) * sizeof(WCHAR);
  void *remote = VirtualAllocEx(process, 0, size, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
  int code = 4;
  if (remote && WriteProcessMemory(process, remote, dll, size, 0)) {
    /* kernel32 у процессов одной разрядности загружен по одному адресу. */
    LPTHREAD_START_ROUTINE load = (LPTHREAD_START_ROUTINE)GetProcAddress(GetModuleHandleW(L"kernel32.dll"), "LoadLibraryW");
    HANDLE thread = CreateRemoteThread(process, 0, 0, load, remote, 0, 0);
    if (thread) {
      DWORD exitCode = 0;
      if (WaitForSingleObject(thread, 20000) == WAIT_OBJECT_0 && GetExitCodeThread(thread, &exitCode) && exitCode) code = 0;
      else say("LoadLibraryW в процессе %lu не удался", pid);
      CloseHandle(thread);
    } else {
      say("поток в процессе %lu не создан (ошибка %lu)", pid, GetLastError());
    }
  } else {
    say("память в процессе %lu не выделена (ошибка %lu)", pid, GetLastError());
  }
  if (remote) VirtualFreeEx(process, remote, 0, MEM_RELEASE);
  CloseHandle(process);
  return code;
}

static int run_attach(const WCHAR *dll) {
  HWND window = find_livesplit();
  if (!window) { say("окно LiveSplit не найдено"); return 2; }
  DWORD pid = 0;
  GetWindowThreadProcessId(window, &pid);
  if (has_module(pid, base_name(dll)) == 1) return 0;
  int code = inject(pid, dll);
  if (code == 0) say("снимок LiveSplit включён в процессе %lu", pid);
  return code;
}

static int run_capture(const WCHAR *out, const WCHAR *dll) {
  Canvas canvas;
  ZeroMemory(&canvas, sizeof canvas);
  int method = METHOD_BLT;
  if (dll) {
    int code = run_attach(dll);
    if (code) return code;
    ShareView view = { 0, 0 };
    DWORD started = GetTickCount();
    while (!share_read(&view, &canvas, 1500)) {
      if (GetTickCount() - started > 5000) { say("LiveSplit не прислал кадр"); return find_livesplit() ? 3 : 2; }
      Sleep(50);
    }
  } else {
    HWND window = find_livesplit();
    if (!window) { say("окно LiveSplit не найдено"); return 2; }
    method = 0;
    if (!capture_window(window, &canvas, &method)) { say("снимок LiveSplit не получился"); return 3; }
  }
  HANDLE file = CreateFileW(out, GENERIC_WRITE, 0, 0, CREATE_ALWAYS, 0, 0);
  if (file == INVALID_HANDLE_VALUE) { say("файл снимка не создан (ошибка %lu)", GetLastError()); return 3; }
  BYTE header[16] = { 'H', 'L', 'S', 'R', 'L', 'S', '1', 0 };
  *(DWORD *)(header + 8) = (DWORD)canvas.width;
  *(DWORD *)(header + 12) = (DWORD)canvas.height;
  DWORD written;
  BOOL ok = WriteFile(file, header, sizeof header, &written, 0) &&
            WriteFile(file, canvas.bits, canvas.width * canvas.height * 4, &written, 0);
  CloseHandle(file);
  say("снимок LiveSplit %dx%d (%s)", canvas.width, canvas.height, dll ? "общая память" : method == METHOD_PRINT ? "PrintWindow" : "BitBlt");
  canvas_free(&canvas);
  if (!ok) { say("снимок не записан"); return 3; }
  return 0;
}

/* Код пишется во временный файл и переименовывается: лаунчер не прочитает
 * его наполовину. */
static void write_status(const WCHAR *path, int code) {
  WCHAR temporary[MAX_PATH];
  if (lstrlenW(path) + 5 >= MAX_PATH) return;
  lstrcpyW(temporary, path);
  lstrcatW(temporary, L".tmp");
  HANDLE file = CreateFileW(temporary, GENERIC_WRITE, 0, 0, CREATE_ALWAYS, 0, 0);
  if (file == INVALID_HANDLE_VALUE) return;
  char text[16];
  int length = wsprintfA(text, "%d", code);
  DWORD written;
  BOOL ok = WriteFile(file, text, length, &written, 0);
  CloseHandle(file);
  if (ok) MoveFileExW(temporary, path, MOVEFILE_REPLACE_EXISTING);
  else DeleteFileW(temporary);
}

void __stdcall start(void) {
  int argc = 0;
  WCHAR **argv = CommandLineToArgvW(GetCommandLineW(), &argc);
  const WCHAR *dll = 0, *out = 0, *status = 0;
  for (int i = 2; argv && i + 1 < argc; i += 2) {
    if (same_name(argv[i], L"--dll")) dll = argv[i + 1];
    else if (same_name(argv[i], L"--out")) out = argv[i + 1];
    else if (same_name(argv[i], L"--status")) status = argv[i + 1];
  }
  int code = 5;
  if (argv && argc >= 2 && same_name(argv[1], L"capture") && out) code = run_capture(out, dll);
  else if (argv && argc >= 2 && same_name(argv[1], L"attach") && dll) code = run_attach(dll);
  else say("использование: hlsr-overlay64.exe capture --out <путь> [--dll <путь>] | attach --dll <путь>");
  if (status) write_status(status, code);
  ExitProcess(code);
}
