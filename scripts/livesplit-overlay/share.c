/* hlsr-share64.dll — снимок окна LiveSplit изнутри LiveSplit.exe (Wine).
 *
 * Под Wine окно рисуется в память своего процесса: снимок из игры или из
 * помощника выходит пустым, а из самого LiveSplit — верным, даже если окно
 * закрыто полноэкранной игрой. hlsr-overlay64.exe внедряет эту DLL в
 * LiveSplit.exe; она раз в ~33 мс снимает главное окно и кладёт кадр в общую
 * память (SHARE_NAME в common.h), откуда его берут оверлей в игре и
 * предпросмотр в лаунчере. Без читателей снимает раз в полсекунды. */

#include "common.h"

/* Главное окно своего процесса: самое большое видимое без владельца. */
static HWND own_window(void) {
  WindowSearch search;
  search.count = 1;
  search.pids[0] = GetCurrentProcessId();
  search.best = 0;
  search.bestArea = 0;
  EnumWindows(search_window, (LPARAM)&search);
  return search.best;
}

static DWORD WINAPI worker(LPVOID unused) {
  (void)unused;
  HANDLE mapping = CreateFileMappingW(INVALID_HANDLE_VALUE, 0, PAGE_READWRITE, 0, (DWORD)SHARE_BYTES, SHARE_NAME);
  if (!mapping) return 1;
  ShareHeader *h = (ShareHeader *)MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, 0);
  if (!h) return 1;
  h->pid = GetCurrentProcessId();
  h->magic = SHARE_MAGIC;
  Canvas canvas;
  ZeroMemory(&canvas, sizeof canvas);
  for (;;) {
    DWORD now = GetTickCount();
    int reading = now - h->readTick < 3000;
    HWND window = own_window();
    RECT rc;
    if (window && GetClientRect(window, &rc) && rc.right >= 16 && rc.bottom >= 16) {
      int width = rc.right > MAX_SIDE ? MAX_SIDE : rc.right, height = rc.bottom > MAX_SIDE ? MAX_SIDE : rc.bottom;
      HDC source = GetDC(window);
      if (source && canvas_ensure(&canvas, width, height) &&
          BitBlt(canvas.dc, 0, 0, width, height, source, 0, 0, SRCCOPY)) {
        GdiFlush();
        InterlockedIncrement(&h->seq);
        MemoryBarrier();
        h->width = width;
        h->height = height;
        CopyMemory((BYTE *)(h + 1), canvas.bits, (SIZE_T)width * height * 4);
        h->pid = GetCurrentProcessId();
        h->tick = GetTickCount();
        MemoryBarrier();
        InterlockedIncrement(&h->seq);
      }
      if (source) ReleaseDC(window, source);
    }
    Sleep(reading ? 33 : 500);
  }
  return 0;
}

BOOL WINAPI DllMain(HINSTANCE instance, DWORD reason, LPVOID reserved) {
  (void)reserved;
  if (reason != DLL_PROCESS_ATTACH) return TRUE;
  DisableThreadLibraryCalls(instance);
  HANDLE thread = CreateThread(0, 0, worker, 0, 0, 0);
  if (thread) CloseHandle(thread);
  return TRUE;
}
