/* d3d9.dll — прокси Direct3D 9 для оверлея LiveSplit (HL2 2707/4260,
 * Ghosting). Общее устройство — в proxy.h, отрисовка — в overlay.h.
 *
 * Direct3DCreate9 и Direct3DCreate9Ex отдают объект настоящей d3d9.dll с
 * нашими методами в таблице: CreateDevice и CreateDeviceEx ставят их и
 * созданному устройству, а там Present, PresentEx, Reset и ResetEx сначала
 * зовут оверлей. Код настоящей DLL не меняется; объект остаётся тем же,
 * поэтому QueryInterface и подсчёт ссылок работают как раньше.
 *
 * Как ставятся наши методы, зависит от того, где лежит таблица объекта:
 *  - в своей памяти процесса, доступной для записи (в Windows 10/11 у
 *    каждого устройства своя таблица в куче): прямо в неё, остальные места
 *    остаются за d3d9.dll;
 *  - в образе DLL, только для чтения (IDirect3D9 в Windows, Wine, DXVK):
 *    объект получает нашу копию таблицы. Копируется вся таблица, а не только
 *    методы интерфейса: d3d9.dll через неё же зовёт свои внутренние
 *    виртуальные методы, которые стоят после методов IDirect3DDevice9Ex
 *    (в Windows 11 у устройства 175 мест, у интерфейса 134; короткая копия
 *    роняла игру на первом же таком вызове).
 *
 * Настоящий метод перехват берёт из записи о таблице (Table.saved). Запись
 * ищется по объекту (g_bindings), потом по таблице объекта: если ещё кто-то
 * подменит таблицу своей копией нашей, вызов всё равно дойдёт до настоящего
 * метода. */

#define PROXY_NAME L"d3d9.dll"
#define PROXY_NAME_A "d3d9.dll"
#define HLSR_D3D9
#define COBJMACROS
#define CINTERFACE
#include "proxy.h"
#include "overlay.h"

/* Методов в интерфейсах и номера тех, что перехватываются. */
#define D3D9_METHODS 17        /* IDirect3D9 */
#define D3D9EX_METHODS 22      /* IDirect3D9Ex */
#define DEVICE_METHODS 119     /* IDirect3DDevice9 */
#define DEVICEEX_METHODS 134   /* IDirect3DDevice9Ex */
#define CREATE_DEVICE 16
#define CREATE_DEVICE_EX 20
#define RESET 16
#define PRESENT 17
#define PRESENT_EX 121
#define RESET_EX 132
/* Дальше конец таблицы не ищем. */
#define MAX_METHODS 1024

/* Перехваты: биты в Table.hooks, места — g_slots, наши методы — g_hooks. */
#define HOOK_CREATE_DEVICE 0
#define HOOK_CREATE_DEVICE_EX 1
#define HOOK_RESET 2
#define HOOK_PRESENT 3
#define HOOK_PRESENT_EX 4
#define HOOK_RESET_EX 5
#define HOOKS 6
#define BIT(hook) (1 << (hook))

typedef HRESULT (WINAPI *CreateDeviceFn)(IDirect3D9 *, UINT, D3DDEVTYPE, HWND, DWORD, D3DPRESENT_PARAMETERS *, IDirect3DDevice9 **);
typedef HRESULT (WINAPI *CreateDeviceExFn)(IDirect3D9Ex *, UINT, D3DDEVTYPE, HWND, DWORD, D3DPRESENT_PARAMETERS *, D3DDISPLAYMODEEX *, IDirect3DDevice9Ex **);
typedef HRESULT (WINAPI *PresentFn)(IDirect3DDevice9 *, const RECT *, const RECT *, HWND, const RGNDATA *);
typedef HRESULT (WINAPI *ResetFn)(IDirect3DDevice9 *, D3DPRESENT_PARAMETERS *);
typedef HRESULT (WINAPI *PresentExFn)(IDirect3DDevice9Ex *, const RECT *, const RECT *, HWND, const RGNDATA *, DWORD);
typedef HRESULT (WINAPI *ResetExFn)(IDirect3DDevice9Ex *, D3DPRESENT_PARAMETERS *, D3DDISPLAYMODEEX *);
typedef IDirect3D9 *(WINAPI *CreateFn)(UINT);
typedef HRESULT (WINAPI *CreateExFn)(UINT, IDirect3D9Ex **);

/* Свои копии: заголовки d3d9.h объявляют их только для d3d9.lib. */
static const GUID IID_D3D9Ex = { 0x02177241, 0x69fc, 0x400c, { 0x8f, 0xf1, 0x93, 0xa4, 0x4d, 0xf6, 0x86, 0x1d } };
static const GUID IID_Device9Ex = { 0xb18b10ce, 0x2649, 0x405a, { 0x87, 0x0f, 0x95, 0xf7, 0x77, 0xd4, 0x31, 0x3a } };

/* ---- Таблицы методов. ---- */

/* Запись о таблице, в которую поставлены наши методы. У копии за записью
 * лежат два служебных слова из-под настоящей таблицы (RTTI у C++: с ними
 * dynamic_cast внутри DLL видит то же), затем count мест. */
typedef struct Table {
  struct Table *next;
  void **methods;      /* таблица, на которую смотрит объект */
  void **original;     /* настоящая таблица объекта */
  int count;           /* мест в таблице */
  int copy;            /* 1 — наша копия, 0 — методы поставлены в саму таблицу */
  int hooks;           /* какие перехваты стоят (биты HOOK_*) */
  void *saved[HOOKS];  /* что стояло на их местах */
} Table;

/* Объект игры и запись о его таблице. */
typedef struct { void *object; Table *table; } Binding;

#define BINDINGS 64

static CRITICAL_SECTION g_tableLock;
static Table *g_tables;
static Binding g_bindings[BINDINGS];
static int g_bindingCount;
static const int g_slots[HOOKS] = { CREATE_DEVICE, CREATE_DEVICE_EX, RESET, PRESENT, PRESENT_EX, RESET_EX };
static void *g_hooks[HOOKS];

static void proxy_attach(void) {
  InitializeCriticalSection(&g_tableLock);
}

static void proxy_poll(void) {}

static int query(const void *at, MEMORY_BASIC_INFORMATION *info) {
  return VirtualQuery(at, info, sizeof *info) && info->State == MEM_COMMIT && !(info->Protect & (PAGE_NOACCESS | PAGE_GUARD));
}

static int readable(const void *at, SIZE_T size) {
  MEMORY_BASIC_INFORMATION info;
  return query(at, &info) && (const BYTE *)at + size <= (const BYTE *)info.BaseAddress + info.RegionSize;
}

/* Мест в таблице: сколько записей подряд указывают на исполняемый код. */
static int count_methods(void **table) {
  const BYTE *tableEnd = 0, *codeStart = 0, *codeEnd = 0;
  MEMORY_BASIC_INFORMATION info;
  int count = 0;
  for (; count < MAX_METHODS; count++) {
    const BYTE *slot = (const BYTE *)(table + count);
    if (slot + sizeof(void *) > tableEnd) {
      if (!query(slot, &info) || slot + sizeof(void *) > (const BYTE *)info.BaseAddress + info.RegionSize) break;
      tableEnd = (const BYTE *)info.BaseAddress + info.RegionSize;
    }
    const BYTE *entry = (const BYTE *)table[count];
    if (entry >= codeStart && entry < codeEnd) continue;
    if (!entry || !query(entry, &info) ||
        !(info.Protect & (PAGE_EXECUTE | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY))) break;
    codeStart = (const BYTE *)info.BaseAddress;
    codeEnd = codeStart + info.RegionSize;
  }
  return count;
}

/* Таблица в своей памяти процесса (куча), доступной для записи, а не в
 * образе DLL. */
static int writable(void **table, int count) {
  MEMORY_BASIC_INFORMATION info;
  if (!query(table, &info) || info.Type != MEM_PRIVATE) return 0;
  if (!(info.Protect & (PAGE_READWRITE | PAGE_EXECUTE_READWRITE))) return 0;
  return (const BYTE *)(table + count) <= (const BYTE *)info.BaseAddress + info.RegionSize;
}

/* Объект отвечает на QueryInterface(iid) самим собой: у него таблица Ex. */
static int has_interface(void *object, const GUID *iid) {
  IUnknown *unknown = (IUnknown *)object;
  void *other = 0;
  if (FAILED(IUnknown_QueryInterface(unknown, iid, &other)) || !other) return 0;
  IUnknown_Release((IUnknown *)other);
  return other == object;
}

/* Под замком. */
static Table *table_of(void **methods) {
  for (Table *t = g_tables; t; t = t->next) if (t->methods == methods) return t;
  return 0;
}

static Table *copy_of(void **original) {
  for (Table *t = g_tables; t; t = t->next) if (t->copy && t->original == original) return t;
  return 0;
}

static Table *new_table(void **original, int count, int copy) {
  SIZE_T size = sizeof(Table) + (copy ? (2 + (SIZE_T)count) * sizeof(void *) : 0);
  Table *t = (Table *)HeapAlloc(GetProcessHeap(), HEAP_ZERO_MEMORY, size);
  if (!t) return 0;
  t->original = original;
  t->count = count;
  t->copy = copy;
  t->methods = original;
  if (copy) {
    void **words = (void **)(t + 1);
    if (readable(original - 2, 2 * sizeof(void *))) {
      words[0] = original[-2];
      words[1] = original[-1];
    }
    CopyMemory(words + 2, original, count * sizeof(void *));
    t->methods = words + 2;
  }
  t->next = g_tables;
  g_tables = t;
  return t;
}

/* Ставит перехваты hooks в таблицу записи. Где уже стоит наш метод, ничего
 * не делает. Если место после нашей записи сменилось: на нём снова прежний
 * метод — d3d9.dll переписала таблицу, ставим заново; чужой метод не
 * трогаем — он сам дойдёт до нашего, а замена дала бы вызов по кругу. */
static void install(Table *t, int hooks) {
  for (int h = 0; h < HOOKS; h++) {
    if (!(hooks & BIT(h)) || g_slots[h] >= t->count) continue;
    void **slot = t->methods + g_slots[h];
    void *current = *slot;
    if (current == g_hooks[h]) continue;
    if (t->hooks & BIT(h)) {
      if (current != t->saved[h]) continue;
    } else {
      t->saved[h] = current;
      t->hooks |= BIT(h);
    }
    InterlockedExchangePointer((PVOID volatile *)slot, g_hooks[h]);
  }
}

static void bind_object(void *object, Table *t) {
  for (int i = 0; i < g_bindingCount; i++)
    if (g_bindings[i].object == object) { g_bindings[i].table = t; return; }
  if (g_bindingCount < BINDINGS) {
    g_bindings[g_bindingCount].object = object;
    g_bindings[g_bindingCount].table = t;
    g_bindingCount++;
  }
}

/* Ставит наши методы объекту. device — устройство (иначе IDirect3D9); ex:
 * 1 — вариант Ex, -1 — не Ex, 0 — спросить у объекта. */
static void wrap(void *object, int device, int ex) {
  if (!readable(object, sizeof(void *))) return;
  if (!ex) ex = has_interface(object, device ? &IID_Device9Ex : &IID_D3D9Ex) ? 1 : -1;
  int needed = device ? (ex > 0 ? DEVICEEX_METHODS : DEVICE_METHODS) : (ex > 0 ? D3D9EX_METHODS : D3D9_METHODS);
  int hooks = device ? BIT(HOOK_RESET) | BIT(HOOK_PRESENT) | (ex > 0 ? BIT(HOOK_PRESENT_EX) | BIT(HOOK_RESET_EX) : 0)
                     : BIT(HOOK_CREATE_DEVICE) | (ex > 0 ? BIT(HOOK_CREATE_DEVICE_EX) : 0);
  const char *what = device ? "устройство" : "IDirect3D9";
  EnterCriticalSection(&g_tableLock);
  void **methods = *(void ***)object;
  Table *t = table_of(methods);
  int fresh = !t;
  if (!t) t = copy_of(methods);
  if (!t) {
    int count = count_methods(methods);
    if (count < needed) {
      LeaveCriticalSection(&g_tableLock);
      log_line("d3d9: %s %p: в таблице %p мест %d из %d, оверлей не ставится", what, object, methods, count, needed);
      return;
    }
    t = new_table(methods, count, !writable(methods, needed));
  }
  if (t) {
    install(t, hooks);
    if (t->methods != methods) InterlockedExchangePointer((PVOID volatile *)object, t->methods);
    bind_object(object, t);
  }
  LeaveCriticalSection(&g_tableLock);
  if (t && fresh)
    log_line("d3d9: %s%s %p: таблица %p, мест %d, методы %s %p", what, ex > 0 ? " Ex" : "", object, methods, t->count,
             t->copy ? "в копии" : "в самой таблице", t->methods);
}

/* Настоящий метод перехвата hook у объекта. */
static void *saved_method(void *object, int hook) {
  Table *t = 0;
  EnterCriticalSection(&g_tableLock);
  for (int i = 0; i < g_bindingCount; i++)
    if (g_bindings[i].object == object) { t = g_bindings[i].table; break; }
  if (!t || !(t->hooks & BIT(hook))) t = table_of(*(void ***)object);
  /* Таблица чужая (её подменили после нас): метод на этом месте у всех
   * наших записей один и тот же. */
  if (!t || !(t->hooks & BIT(hook)))
    for (t = g_tables; t && !(t->hooks & BIT(hook)); t = t->next) {}
  void *method = t ? t->saved[hook] : 0;
  LeaveCriticalSection(&g_tableLock);
  return method;
}

/* После Reset: d3d9.dll могла переписать таблицу устройства или завести новую. */
static void rewrap(void *device) {
  int ex = -1;
  EnterCriticalSection(&g_tableLock);
  for (int i = 0; i < g_bindingCount; i++)
    if (g_bindings[i].object == device && (g_bindings[i].table->hooks & BIT(HOOK_PRESENT_EX))) ex = 1;
  LeaveCriticalSection(&g_tableLock);
  wrap(device, 1, ex);
}

/* ---- Перехваты. ---- */

static HRESULT WINAPI hook_CreateDevice(IDirect3D9 *d3d, UINT adapter, D3DDEVTYPE type, HWND window, DWORD flags,
    D3DPRESENT_PARAMETERS *params, IDirect3DDevice9 **out) {
  CreateDeviceFn real = (CreateDeviceFn)saved_method(d3d, HOOK_CREATE_DEVICE);
  if (!real) return D3DERR_INVALIDCALL;
  HRESULT hr = real(d3d, adapter, type, window, flags, params, out);
  if (SUCCEEDED(hr) && out && *out) wrap(*out, 1, 0);
  return hr;
}

static HRESULT WINAPI hook_CreateDeviceEx(IDirect3D9Ex *d3d, UINT adapter, D3DDEVTYPE type, HWND window, DWORD flags,
    D3DPRESENT_PARAMETERS *params, D3DDISPLAYMODEEX *mode, IDirect3DDevice9Ex **out) {
  CreateDeviceExFn real = (CreateDeviceExFn)saved_method(d3d, HOOK_CREATE_DEVICE_EX);
  if (!real) return D3DERR_INVALIDCALL;
  HRESULT hr = real(d3d, adapter, type, window, flags, params, mode, out);
  if (SUCCEEDED(hr) && out && *out) wrap(*out, 1, 1);
  return hr;
}

/* Один Present может звать другой: рисуем только во внешнем вызове. */
static volatile DWORD g_presentThread;

static int present_enter(IDirect3DDevice9 *device) {
  DWORD thread = GetCurrentThreadId();
  if (g_presentThread == thread) return 0;
  g_presentThread = thread;
  overlay_start();
  d3d_draw(device);
  return 1;
}

static void present_leave(int outer) {
  if (outer) g_presentThread = 0;
}

static HRESULT WINAPI hook_Present(IDirect3DDevice9 *device, const RECT *source, const RECT *dest, HWND window, const RGNDATA *dirty) {
  PresentFn real = (PresentFn)saved_method(device, HOOK_PRESENT);
  if (!real) return D3DERR_INVALIDCALL;
  int outer = present_enter(device);
  HRESULT hr = real(device, source, dest, window, dirty);
  present_leave(outer);
  return hr;
}

static HRESULT WINAPI hook_Reset(IDirect3DDevice9 *device, D3DPRESENT_PARAMETERS *params) {
  ResetFn real = (ResetFn)saved_method(device, HOOK_RESET);
  if (!real) return D3DERR_INVALIDCALL;
  /* Ресурсы в D3DPOOL_DEFAULT надо отпустить до Reset, иначе он не пройдёт. */
  if (device == g_d3dDevice) d3d_release();
  HRESULT hr = real(device, params);
  rewrap(device);
  log_line("d3d9: Reset %p: 0x%08lx", device, hr);
  return hr;
}

static HRESULT WINAPI hook_PresentEx(IDirect3DDevice9Ex *device, const RECT *source, const RECT *dest, HWND window, const RGNDATA *dirty, DWORD flags) {
  PresentExFn real = (PresentExFn)saved_method(device, HOOK_PRESENT_EX);
  if (!real) return D3DERR_INVALIDCALL;
  int outer = present_enter((IDirect3DDevice9 *)device);
  HRESULT hr = real(device, source, dest, window, dirty, flags);
  present_leave(outer);
  return hr;
}

static HRESULT WINAPI hook_ResetEx(IDirect3DDevice9Ex *device, D3DPRESENT_PARAMETERS *params, D3DDISPLAYMODEEX *mode) {
  ResetExFn real = (ResetExFn)saved_method(device, HOOK_RESET_EX);
  if (!real) return D3DERR_INVALIDCALL;
  if ((IDirect3DDevice9 *)device == g_d3dDevice) d3d_release();
  HRESULT hr = real(device, params, mode);
  rewrap(device);
  log_line("d3d9: ResetEx %p: 0x%08lx", device, hr);
  return hr;
}

static void *g_hooks[HOOKS] = {
  (void *)hook_CreateDevice, (void *)hook_CreateDeviceEx, (void *)hook_Reset,
  (void *)hook_Present, (void *)hook_PresentEx, (void *)hook_ResetEx,
};

/* ---- Экспорт. ---- */

IDirect3D9 *WINAPI hook_Direct3DCreate9(UINT sdk) {
  CreateFn real = (CreateFn)proxy_function("Direct3DCreate9");
  if (!real) return 0;
  overlay_start();
  IDirect3D9 *d3d = real(sdk);
  if (d3d) wrap(d3d, 0, 0);
  return d3d;
}

HRESULT WINAPI hook_Direct3DCreate9Ex(UINT sdk, IDirect3D9Ex **out) {
  CreateExFn real = (CreateExFn)proxy_function("Direct3DCreate9Ex");
  if (!real) return D3DERR_NOTAVAILABLE;
  overlay_start();
  HRESULT hr = real(sdk, out);
  if (SUCCEEDED(hr) && out && *out) wrap(*out, 0, 1);
  return hr;
}

/* Остальные функции — переходники (список в d3d9.def). */
PROXY_THUNK(Direct3DShaderValidatorCreate9)
PROXY_THUNK(PSGPError)
PROXY_THUNK(PSGPSampleTexture)
PROXY_THUNK(D3DPERF_BeginEvent)
PROXY_THUNK(D3DPERF_EndEvent)
PROXY_THUNK(D3DPERF_GetStatus)
PROXY_THUNK(D3DPERF_QueryRepeatFrame)
PROXY_THUNK(D3DPERF_SetMarker)
PROXY_THUNK(D3DPERF_SetOptions)
PROXY_THUNK(D3DPERF_SetRegion)
PROXY_THUNK(DebugSetLevel)
PROXY_THUNK(DebugSetMute)
PROXY_THUNK(Direct3DCreate9On12)
