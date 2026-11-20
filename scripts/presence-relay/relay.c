/* hlsr-presence-relay.exe — мост статуса игры для Discord под Proton.
 *
 *   hlsr-presence-relay.exe --pipe \\.\pipe\hlsr-presence-<hex> --port <n>
 *                           --name hl2.exe [--status <путь>] [--timeout 180]
 *
 * client.dll Ghosting отправляет карту, FPS и таймер забега в именованный
 * канал из HLSR_PRESENCE_PIPE (src/main/game-presence). Под Proton такой
 * канал живёт внутри wineserver, и лаунчер на хосте его не видит. Помощник
 * запускается в префиксе игры до неё, создаёт этот канал и пересылает байты
 * в обе стороны на 127.0.0.1:<port>, где их принимает лаунчер. Содержимое
 * он не разбирает: рукопожатие с токеном и проверку сообщений делает
 * PresenceBridgeServer.
 *
 * Помощник сам завершается, когда процесс игры (--name) закрылся, или если
 * игра не появилась за --timeout секунд.
 *
 * Коды выхода: 0 — игра закрылась, 2 — игра не появилась, 3 — канал не
 * создан, 5 — неверные аргументы. С --status <путь> в файл пишется 0, как
 * только канал создан: под Proton сессия umu не завершается, пока работает
 * игра, поэтому лаунчер на Linux ждёт файл, а не выход. */

#include <winsock2.h>
#include <windows.h>
#include <tlhelp32.h>
#include <shellapi.h>

#define BUFFER_SIZE 2048
#define PIPE_PREFIX L"\\\\.\\pipe\\hlsr-presence-"

static void say(const char *format, ...) {
  char line[512];
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

static int starts_with(const WCHAR *text, const WCHAR *prefix) {
  for (; *prefix; text++, prefix++) if (*text != *prefix) return 0;
  return 1;
}

static DWORD parse_number(const WCHAR *text) {
  DWORD value = 0;
  for (; *text >= '0' && *text <= '9'; text++) value = value * 10 + (*text - '0');
  return value;
}

static int game_running(const WCHAR *name) {
  HANDLE snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
  if (snapshot == INVALID_HANDLE_VALUE) return 1;
  PROCESSENTRY32W entry;
  entry.dwSize = sizeof entry;
  int found = 0;
  for (BOOL ok = Process32FirstW(snapshot, &entry); ok && !found; ok = Process32NextW(snapshot, &entry))
    found = same_name(entry.szExeFile, name);
  CloseHandle(snapshot);
  return found;
}

/* Код пишется во временный файл и переименовывается: лаунчер не прочитает
 * его наполовину. */
static void write_status(const WCHAR *path, int code) {
  WCHAR temporary[MAX_PATH];
  if (!path || lstrlenW(path) + 5 >= MAX_PATH) return;
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

/* Канал открыт с FILE_FLAG_OVERLAPPED: иначе чтение и запись из двух
 * потоков ждали бы друг друга. */
static BOOL pipe_io(HANDLE pipe, BOOL write, void *buffer, DWORD length, DWORD *done) {
  OVERLAPPED overlapped;
  ZeroMemory(&overlapped, sizeof overlapped);
  overlapped.hEvent = CreateEventW(0, TRUE, FALSE, 0);
  if (!overlapped.hEvent) return FALSE;
  BOOL ok = write ? WriteFile(pipe, buffer, length, 0, &overlapped)
                  : ReadFile(pipe, buffer, length, 0, &overlapped);
  if (ok || GetLastError() == ERROR_IO_PENDING) ok = GetOverlappedResult(pipe, &overlapped, done, TRUE);
  CloseHandle(overlapped.hEvent);
  return ok;
}

typedef struct {
  HANDLE pipe;
  SOCKET socket;
} Link;

static DWORD WINAPI pipe_to_socket(void *param) {
  Link *link = param;
  char buffer[BUFFER_SIZE];
  DWORD read;
  while (pipe_io(link->pipe, FALSE, buffer, sizeof buffer, &read) && read) {
    for (DWORD sent = 0; sent < read;) {
      int n = send(link->socket, buffer + sent, (int)(read - sent), 0);
      if (n <= 0) return 0;
      sent += (DWORD)n;
    }
  }
  return 0;
}

static DWORD WINAPI socket_to_pipe(void *param) {
  Link *link = param;
  char buffer[BUFFER_SIZE];
  int n;
  while ((n = recv(link->socket, buffer, sizeof buffer, 0)) > 0) {
    for (DWORD written = 0, chunk; written < (DWORD)n; written += chunk)
      if (!pipe_io(link->pipe, TRUE, buffer + written, (DWORD)n - written, &chunk) || !chunk) return 0;
  }
  return 0;
}

static SOCKET connect_launcher(DWORD port) {
  SOCKET s = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
  if (s == INVALID_SOCKET) return s;
  struct sockaddr_in address;
  ZeroMemory(&address, sizeof address);
  address.sin_family = AF_INET;
  address.sin_port = htons((u_short)port);
  address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
  BOOL on = TRUE;
  setsockopt(s, IPPROTO_TCP, TCP_NODELAY, (const char *)&on, sizeof on);
  if (connect(s, (struct sockaddr *)&address, sizeof address) != 0) {
    closesocket(s);
    return INVALID_SOCKET;
  }
  return s;
}

/* Пересылает байты, пока одна из сторон не закроется. 1 — игра закрылась. */
static int relay(HANDLE pipe, SOCKET s, const WCHAR *name) {
  Link link = { pipe, s };
  HANDLE threads[2];
  threads[0] = CreateThread(0, 0, pipe_to_socket, &link, 0, 0);
  threads[1] = CreateThread(0, 0, socket_to_pipe, &link, 0, 0);
  int gone = 0;
  if (threads[0] && threads[1]) {
    for (;;) {
      DWORD wait = WaitForMultipleObjects(2, threads, FALSE, 2000);
      if (wait != WAIT_TIMEOUT) break;
      if (!game_running(name)) { gone = 1; break; }
    }
  }
  /* Будим оба потока: recv прервётся на закрытом сокете, чтение канала —
   * на отключённом клиенте. */
  shutdown(s, SD_BOTH);
  closesocket(s);
  CancelIoEx(pipe, 0);
  DisconnectNamedPipe(pipe);
  for (int i = 0; i < 2; i++) {
    if (!threads[i]) continue;
    WaitForSingleObject(threads[i], 5000);
    CloseHandle(threads[i]);
  }
  return gone;
}

static int run(const WCHAR *pipeName, DWORD port, const WCHAR *name, const WCHAR *status, DWORD timeout) {
  WSADATA wsa;
  if (WSAStartup(MAKEWORD(2, 2), &wsa) != 0) { say("сеть Windows недоступна"); return 3; }
  HANDLE pipe = CreateNamedPipeW(pipeName,
    PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
    1, BUFFER_SIZE, BUFFER_SIZE, 0, 0);
  if (pipe == INVALID_HANDLE_VALUE) {
    say("канал не создан: ошибка %lu", GetLastError());
    return 3;
  }
  write_status(status, 0);
  say("канал готов, порт лаунчера %lu, ждём %ls", port, name);

  HANDLE event = CreateEventW(0, TRUE, FALSE, 0);
  DWORD started = GetTickCount();
  int seen = 0;
  for (;;) {
    OVERLAPPED overlapped;
    ZeroMemory(&overlapped, sizeof overlapped);
    ResetEvent(event);
    overlapped.hEvent = event;
    BOOL connected = ConnectNamedPipe(pipe, &overlapped);
    DWORD error = connected ? ERROR_PIPE_CONNECTED : GetLastError();
    if (error == ERROR_IO_PENDING) {
      for (;;) {
        if (WaitForSingleObject(event, 1000) == WAIT_OBJECT_0) { error = ERROR_PIPE_CONNECTED; break; }
        if (game_running(name)) seen = 1;
        else if (seen) { CancelIoEx(pipe, &overlapped); say("игра закрылась"); return 0; }
        else if (GetTickCount() - started > timeout * 1000) { CancelIoEx(pipe, &overlapped); say("игра не появилась"); return 2; }
      }
    }
    if (error != ERROR_PIPE_CONNECTED) {
      say("подключение к каналу не удалось: ошибка %lu", error);
      DisconnectNamedPipe(pipe);
      Sleep(500);
      continue;
    }
    seen = 1;
    SOCKET s = connect_launcher(port);
    if (s == INVALID_SOCKET) {
      say("лаунчер не принимает соединение: ошибка %d", WSAGetLastError());
      DisconnectNamedPipe(pipe);
      Sleep(1000);
      continue;
    }
    say("игра подключилась");
    if (relay(pipe, s, name)) { say("игра закрылась"); return 0; }
    say("соединение закрыто, ждём новое");
  }
}

void __stdcall start(void) {
  int argc = 0;
  WCHAR **argv = CommandLineToArgvW(GetCommandLineW(), &argc);
  const WCHAR *pipeName = 0, *name = 0, *status = 0;
  DWORD port = 0, timeout = 180;
  for (int i = 1; argv && i + 1 < argc; i += 2) {
    if (same_name(argv[i], L"--pipe")) pipeName = argv[i + 1];
    else if (same_name(argv[i], L"--port")) port = parse_number(argv[i + 1]);
    else if (same_name(argv[i], L"--name")) name = argv[i + 1];
    else if (same_name(argv[i], L"--status")) status = argv[i + 1];
    else if (same_name(argv[i], L"--timeout")) timeout = parse_number(argv[i + 1]);
  }
  int code = 5;
  if (pipeName && starts_with(pipeName, PIPE_PREFIX) && port > 0 && port < 65536 && name)
    code = run(pipeName, port, name, status, timeout);
  else say("использование: hlsr-presence-relay.exe --pipe \\\\.\\pipe\\hlsr-presence-<hex> --port <n> --name <exe> [--status <путь>]");
  if (code != 0) write_status(status, code);
  ExitProcess(code);
}
