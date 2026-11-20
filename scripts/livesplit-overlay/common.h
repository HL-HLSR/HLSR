/* Общее для оверлея в прокси (overlay.h) и помощников снимка (inject.c,
 * share.c): настройки оверлея, поиск окна LiveSplit, снимок его клиентской
 * части и маски фона.
 *
 * Маски повторяют фильтры OBS «Цветовой ключ» и «Яркостный ключ» и
 * src/shared/livesplit-overlay.js (keyAlpha): предпросмотр в лаунчере и
 * игра должны вырезать одни и те же пиксели. CRT не используется: DLL живёт
 * в чужом процессе и не должна тянуть за собой свою библиотеку C. */

#ifdef __clang__
#pragma clang diagnostic ignored "-Wunused-function"
#endif

#include <windows.h>
#include <tlhelp32.h>

#define KEY_NONE 0
#define KEY_COLOR 1
#define KEY_LUMA 2

/* Холст перед PrintWindow заливается этим цветом (BGRX): что осталось им
 * залито, окно не нарисовало. В src/shared/livesplit-overlay.js — SENTINEL. */
#define SENTINEL 0x5A00FF5Au

/* Больше LiveSplit не бывает; ограничение держит буферы в разумных рамках. */
#define MAX_SIDE 2048

typedef struct {
  int enabled;
  float x, y, height, opacity;            /* проценты */
  int key;
  float keyR, keyG, keyB;                 /* 0..1 */
  float similarity, smoothness;           /* 0..1 (в настройках 1..1000, как в OBS) */
  float lumaMin, lumaMinSmooth, lumaMax, lumaMaxSmooth;
  int cropTop, cropRight, cropBottom, cropLeft;
} OverlayConfig;

static void config_defaults(OverlayConfig *c) {
  c->enabled = 0;
  c->x = 100; c->y = 50; c->height = 45; c->opacity = 100;
  c->key = KEY_NONE;
  c->keyR = c->keyG = c->keyB = 0;
  c->similarity = 0.08f; c->smoothness = 0.05f;
  c->lumaMin = 0.1f; c->lumaMinSmooth = 0.05f; c->lumaMax = 1; c->lumaMaxSmooth = 0;
  c->cropTop = c->cropRight = c->cropBottom = c->cropLeft = 0;
}

static float clampf(float v, float lo, float hi) { return v < lo ? lo : v > hi ? hi : v; }

/* ---- Разбор overlay.ini: строки key=value, числа в десятичной записи. ---- */

static int same_key(const char *a, int length, const char *b) {
  int i = 0;
  for (; i < length && b[i]; i++) if (a[i] != b[i]) return 0;
  return i == length && !b[i];
}

static float parse_float(const char *s, int length) {
  float value = 0, scale = 0;
  int i = 0, negative = 0;
  if (i < length && (s[i] == '-' || s[i] == '+')) negative = s[i++] == '-';
  for (; i < length; i++) {
    char ch = s[i];
    if (ch == '.') { if (scale) break; scale = 1; continue; }
    if (ch < '0' || ch > '9') break;
    if (scale) { scale /= 10; value += (ch - '0') * scale; }
    else value = value * 10 + (ch - '0');
  }
  return negative ? -value : value;
}

static int hex_digit(char ch) {
  if (ch >= '0' && ch <= '9') return ch - '0';
  if (ch >= 'a' && ch <= 'f') return ch - 'a' + 10;
  if (ch >= 'A' && ch <= 'F') return ch - 'A' + 10;
  return -1;
}

static void config_parse(OverlayConfig *c, const char *text, int size) {
  config_defaults(c);
  int pos = 0;
  while (pos < size) {
    int start = pos;
    while (pos < size && text[pos] != '\n') pos++;
    int end = pos++;
    while (end > start && (text[end - 1] == '\r' || text[end - 1] == ' ')) end--;
    int eq = start;
    while (eq < end && text[eq] != '=') eq++;
    if (eq >= end) continue;
    const char *k = text + start, *v = text + eq + 1;
    int kl = eq - start, vl = end - eq - 1;
    float f = parse_float(v, vl);
    if (same_key(k, kl, "enabled")) c->enabled = f != 0;
    else if (same_key(k, kl, "x")) c->x = clampf(f, 0, 100);
    else if (same_key(k, kl, "y")) c->y = clampf(f, 0, 100);
    else if (same_key(k, kl, "height")) c->height = clampf(f, 5, 100);
    else if (same_key(k, kl, "opacity")) c->opacity = clampf(f, 0, 100);
    else if (same_key(k, kl, "key")) c->key = (int)f;
    else if (same_key(k, kl, "key_color") && vl >= 6) {
      int d[6], ok = 1;
      for (int i = 0; i < 6; i++) if ((d[i] = hex_digit(v[i])) < 0) ok = 0;
      if (ok) { c->keyR = (d[0] * 16 + d[1]) / 255.0f; c->keyG = (d[2] * 16 + d[3]) / 255.0f; c->keyB = (d[4] * 16 + d[5]) / 255.0f; }
    }
    else if (same_key(k, kl, "similarity")) c->similarity = clampf(f, 1, 1000) / 1000;
    else if (same_key(k, kl, "smoothness")) c->smoothness = clampf(f, 1, 1000) / 1000;
    else if (same_key(k, kl, "luma_min")) c->lumaMin = clampf(f, 0, 1);
    else if (same_key(k, kl, "luma_min_smooth")) c->lumaMinSmooth = clampf(f, 0, 1);
    else if (same_key(k, kl, "luma_max")) c->lumaMax = clampf(f, 0, 1);
    else if (same_key(k, kl, "luma_max_smooth")) c->lumaMaxSmooth = clampf(f, 0, 1);
    else if (same_key(k, kl, "crop_top")) c->cropTop = (int)clampf(f, 0, MAX_SIDE);
    else if (same_key(k, kl, "crop_right")) c->cropRight = (int)clampf(f, 0, MAX_SIDE);
    else if (same_key(k, kl, "crop_bottom")) c->cropBottom = (int)clampf(f, 0, MAX_SIDE);
    else if (same_key(k, kl, "crop_left")) c->cropLeft = (int)clampf(f, 0, MAX_SIDE);
  }
  if (c->key != KEY_COLOR && c->key != KEY_LUMA) c->key = KEY_NONE;
}

/* ---- Маски. ---- */

static float smoothstep(float e0, float e1, float x) {
  if (e1 <= e0) return x < e0 ? 0.0f : 1.0f;
  float t = clampf((x - e0) / (e1 - e0), 0, 1);
  return t * t * (3 - 2 * t);
}

/* Непрозрачность пикселя 0..1 после маски. */
static float key_alpha(const OverlayConfig *c, float r, float g, float b) {
  if (c->key == KEY_COLOR) {
    float dr = r - c->keyR, dg = g - c->keyG, db = b - c->keyB;
    float distance = __builtin_sqrtf(dr * dr + dg * dg + db * db);
    float base = distance - c->similarity;
    if (base <= 0) return 0;
    return clampf(base / c->smoothness, 0, 1);
  }
  if (c->key == KEY_LUMA) {
    float luma = 0.2126f * r + 0.7152f * g + 0.0722f * b;
    float low = smoothstep(c->lumaMin, c->lumaMin + c->lumaMinSmooth, luma);
    float high = c->lumaMaxSmooth > 0
      ? 1 - smoothstep(c->lumaMax - c->lumaMaxSmooth, c->lumaMax, luma)
      : (luma > c->lumaMax ? 0.0f : 1.0f);
    return low * high;
  }
  return 1;
}

/* Обрезка из настроек, но не меньше 8 пикселей с каждой стороны картинки. */
static void crop_rect(const OverlayConfig *c, int width, int height, RECT *out) {
  int left = c->cropLeft, right = c->cropRight, top = c->cropTop, bottom = c->cropBottom;
  if (left + right > width - 8) { left = 0; right = 0; }
  if (top + bottom > height - 8) { top = 0; bottom = 0; }
  out->left = left; out->top = top; out->right = width - right; out->bottom = height - bottom;
}

/* BGRX из снимка → BGRA с предумноженной альфой. */
static void key_pixels(const OverlayConfig *c, const DWORD *source, int stride, const RECT *crop, DWORD *target) {
  int width = crop->right - crop->left;
  for (int y = crop->top; y < crop->bottom; y++) {
    const DWORD *row = source + y * stride + crop->left;
    DWORD *out = target + (y - crop->top) * width;
    for (int x = 0; x < width; x++) {
      DWORD p = row[x];
      int r = (p >> 16) & 255, g = (p >> 8) & 255, b = p & 255;
      /* Место, которое PrintWindow не закрасил, прозрачно при любой маске. */
      if (p == SENTINEL) { out[x] = 0; continue; }
      if (c->key == KEY_NONE) { out[x] = 0xFF000000u | (p & 0xFFFFFF); continue; }
      float a = key_alpha(c, r / 255.0f, g / 255.0f, b / 255.0f);
      int ia = (int)(a * 255 + 0.5f);
      if (ia <= 0) { out[x] = 0; continue; }
      out[x] = ((DWORD)ia << 24) | ((DWORD)((r * ia + 127) / 255) << 16) | ((DWORD)((g * ia + 127) / 255) << 8) | (DWORD)((b * ia + 127) / 255);
    }
  }
}

/* ---- Окно LiveSplit. ---- */

static int is_livesplit_name(const WCHAR *name) {
  static const WCHAR wanted[] = L"livesplit.exe";
  int i = 0;
  for (; name[i] && wanted[i]; i++) {
    WCHAR ch = name[i];
    if (ch >= 'A' && ch <= 'Z') ch += 32;
    if (ch != wanted[i]) return 0;
  }
  return !name[i] && !wanted[i];
}

typedef struct { DWORD pids[16]; int count; HWND best; int bestArea; } WindowSearch;

static BOOL CALLBACK search_window(HWND hwnd, LPARAM param) {
  WindowSearch *search = (WindowSearch *)param;
  if (!IsWindowVisible(hwnd) || GetWindow(hwnd, GW_OWNER) || IsIconic(hwnd)) return TRUE;
  DWORD pid = 0;
  GetWindowThreadProcessId(hwnd, &pid);
  for (int i = 0; i < search->count; i++) {
    if (search->pids[i] != pid) continue;
    RECT rc;
    if (!GetClientRect(hwnd, &rc)) break;
    int area = rc.right * rc.bottom;
    if (area > search->bestArea) { search->best = hwnd; search->bestArea = area; }
    break;
  }
  return TRUE;
}

/* Главное окно LiveSplit: самое большое видимое окно верхнего уровня без
 * владельца у процесса LiveSplit.exe (окна настроек принадлежат главному). */
static HWND find_livesplit(void) {
  WindowSearch search;
  search.count = 0; search.best = 0; search.bestArea = 0;
  HANDLE snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
  if (snapshot == INVALID_HANDLE_VALUE) return 0;
  PROCESSENTRY32W entry;
  entry.dwSize = sizeof entry;
  for (BOOL ok = Process32FirstW(snapshot, &entry); ok && search.count < 16; ok = Process32NextW(snapshot, &entry))
    if (is_livesplit_name(entry.szExeFile)) search.pids[search.count++] = entry.th32ProcessID;
  CloseHandle(snapshot);
  if (!search.count) return 0;
  EnumWindows(search_window, (LPARAM)&search);
  return search.best;
}

/* ---- Снимок. ---- */

#define METHOD_PRINT 1
#define METHOD_BLT 2

typedef struct {
  HDC dc;
  HBITMAP bitmap;
  DWORD *bits;
  int width, height;
} Canvas;

static void canvas_free(Canvas *canvas) {
  if (canvas->dc) DeleteDC(canvas->dc);
  if (canvas->bitmap) DeleteObject(canvas->bitmap);
  canvas->dc = 0; canvas->bitmap = 0; canvas->bits = 0; canvas->width = canvas->height = 0;
}

static int canvas_ensure(Canvas *canvas, int width, int height) {
  if (canvas->dc && canvas->width == width && canvas->height == height) return 1;
  canvas_free(canvas);
  BITMAPINFO info;
  ZeroMemory(&info, sizeof info);
  info.bmiHeader.biSize = sizeof info.bmiHeader;
  info.bmiHeader.biWidth = width;
  info.bmiHeader.biHeight = -height;
  info.bmiHeader.biPlanes = 1;
  info.bmiHeader.biBitCount = 32;
  void *bits = 0;
  canvas->dc = CreateCompatibleDC(0);
  canvas->bitmap = CreateDIBSection(canvas->dc, &info, DIB_RGB_COLORS, &bits, 0, 0);
  if (!canvas->dc || !canvas->bitmap || !bits) { canvas_free(canvas); return 0; }
  SelectObject(canvas->dc, canvas->bitmap);
  canvas->bits = (DWORD *)bits;
  canvas->width = width;
  canvas->height = height;
  return 1;
}

/* PrintWindow не тронул картинку: под Wine он не рисует окна чужого процесса. */
static int untouched(const Canvas *canvas) {
  int total = canvas->width * canvas->height;
  for (int i = 0; i < 64; i++) if (canvas->bits[total / 64 * i] != SENTINEL) return 0;
  return 1;
}

/* Размер окна снимается в DPI-контексте самого LiveSplit. Иначе при
 * масштабе Windows больше 100% GetClientRect из игры с другим режимом DPI
 * отдаёт размер в своих единицах, а PrintWindow рисует окно в его
 * собственных: картинка занимает только часть холста, а полосы справа и
 * снизу остаются залитыми SENTINEL. Функции есть с Windows 10 1607, поэтому
 * берутся через GetProcAddress. Возвращает прежний контекст потока или 0. */
typedef void *(WINAPI *GetWindowDpiContextFn)(HWND);
typedef void *(WINAPI *SetThreadDpiContextFn)(void *);

static SetThreadDpiContextFn dpi_setter(void) {
  static SetThreadDpiContextFn set;
  static int loaded;
  if (!loaded) {
    HMODULE user32 = GetModuleHandleW(L"user32.dll");
    set = user32 ? (SetThreadDpiContextFn)(void *)GetProcAddress(user32, "SetThreadDpiAwarenessContext") : 0;
    loaded = 1;
  }
  return set;
}

static void *dpi_enter(HWND hwnd) {
  SetThreadDpiContextFn set = dpi_setter();
  HMODULE user32 = GetModuleHandleW(L"user32.dll");
  GetWindowDpiContextFn get = user32 ? (GetWindowDpiContextFn)(void *)GetProcAddress(user32, "GetWindowDpiAwarenessContext") : 0;
  void *context = get ? get(hwnd) : 0;
  return set && context ? set(context) : 0;
}

static void dpi_leave(void *previous) {
  SetThreadDpiContextFn set = dpi_setter();
  if (set && previous) set(previous);
}

static int capture_client(HWND hwnd, Canvas *canvas, int *method) {
  RECT rc;
  if (!GetClientRect(hwnd, &rc) || rc.right < 16 || rc.bottom < 16) return 0;
  int width = rc.right > MAX_SIDE ? MAX_SIDE : rc.right, height = rc.bottom > MAX_SIDE ? MAX_SIDE : rc.bottom;
  if (!canvas_ensure(canvas, width, height)) return 0;
  if (*method != METHOD_BLT) {
    for (int i = 0; i < width * height; i++) canvas->bits[i] = SENTINEL;
    /* PW_CLIENTONLY | PW_RENDERFULLCONTENT */
    if (PrintWindow(hwnd, canvas->dc, 1 | 2) && !untouched(canvas)) { *method = METHOD_PRINT; return 1; }
    if (*method == METHOD_PRINT) return 0;
  }
  HDC source = GetDC(hwnd);
  if (!source) return 0;
  BOOL ok = BitBlt(canvas->dc, 0, 0, width, height, source, 0, 0, SRCCOPY);
  ReleaseDC(hwnd, source);
  if (!ok) return 0;
  *method = METHOD_BLT;
  return 1;
}

/* Снимает клиентскую часть окна. На Windows PrintWindow видит окно даже под
 * полноэкранной игрой; под Wine он ничего не рисует, и тогда берётся BitBlt
 * из DC окна (у Wine он отдаёт содержимое окна, а не экрана).
 * method: 0 — выбрать, METHOD_* — уже выбранный для этого окна. */
static int capture_window(HWND hwnd, Canvas *canvas, int *method) {
  void *previous = dpi_enter(hwnd);
  int ok = capture_client(hwnd, canvas, method);
  dpi_leave(previous);
  return ok;
}

/* ---- Кадр LiveSplit в общей памяти (Wine). ----
 *
 * Под Wine окно рисуется в память своего процесса, и снимок окна LiveSplit
 * из игры или из помощника выходит пустым. Поэтому там в LiveSplit.exe
 * внедряется hlsr-share64.dll (share.c): она снимает своё же окно и кладёт
 * кадр сюда. Писатель держит seq нечётным, пока пишет (seqlock). Читатель
 * отмечает readTick: без читателей DLL снимает окно реже. */

#define SHARE_NAME L"Local\\HlsrLiveSplitFrame"
#define SHARE_MAGIC 0x31534C48u
#define SHARE_PIXELS (MAX_SIDE * MAX_SIDE)

typedef struct {
  DWORD magic;
  volatile LONG seq;
  DWORD width, height;
  volatile DWORD tick;      /* когда записан кадр (GetTickCount) */
  volatile DWORD readTick;  /* когда кадр последний раз читали */
  DWORD pid;
  DWORD reserved;
} ShareHeader;

#define SHARE_BYTES (sizeof(ShareHeader) + SHARE_PIXELS * 4)

typedef struct { HANDLE mapping; ShareHeader *header; } ShareView;

static int is_wine(void) {
  HMODULE ntdll = GetModuleHandleW(L"ntdll.dll");
  return ntdll && GetProcAddress(ntdll, "wine_get_version") != 0;
}

static int share_open(ShareView *view) {
  if (view->header) return 1;
  view->mapping = OpenFileMappingW(FILE_MAP_READ | FILE_MAP_WRITE, FALSE, SHARE_NAME);
  if (!view->mapping) return 0;
  view->header = (ShareHeader *)MapViewOfFile(view->mapping, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, 0);
  if (!view->header) { CloseHandle(view->mapping); view->mapping = 0; return 0; }
  return 1;
}

static void share_close(ShareView *view) {
  if (view->header) UnmapViewOfFile(view->header);
  if (view->mapping) CloseHandle(view->mapping);
  view->header = 0;
  view->mapping = 0;
}

/* Копирует кадр не старше maxAgeMs в canvas. 1 — скопирован, 0 — свежего
 * кадра нет (LiveSplit закрыт, свёрнут или DLL ещё не внедрена). */
static int share_read(ShareView *view, Canvas *canvas, DWORD maxAgeMs) {
  if (!share_open(view)) return 0;
  ShareHeader *h = view->header;
  if (h->magic != SHARE_MAGIC) return 0;
  h->readTick = GetTickCount();
  for (int attempt = 0; attempt < 4; attempt++) {
    LONG before = h->seq;
    MemoryBarrier();
    if (before & 1) { Sleep(1); continue; }
    int width = (int)h->width, height = (int)h->height;
    if (width < 16 || height < 16 || width > MAX_SIDE || height > MAX_SIDE) return 0;
    /* Память живёт, пока её держит хоть один процесс: перезапущенный
     * LiveSplit откроет эту же, так что держать её открытой можно. */
    if (GetTickCount() - h->tick > maxAgeMs) return 0;
    if (!canvas_ensure(canvas, width, height)) return 0;
    CopyMemory(canvas->bits, (BYTE *)(h + 1), (SIZE_T)width * height * 4);
    MemoryBarrier();
    if (h->seq == before) return 1;
  }
  return 0;
}
