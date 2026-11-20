/* Ядро оверлея LiveSplit внутри игры: поток снимков окна LiveSplit, маски и
 * отрисовка кадра поверх игры. Подключается к прокси d3d9.dll (proxy-d3d9.c,
 * HLSR_D3D9) или opengl32.dll (proxy-gl.c, HLSR_GL); устройство прокси —
 * в proxy.h.
 *
 * Отдельный поток раз в ~33 мс снимает окно LiveSplit, обрезает его и
 * вырезает фон маской; кадр игры перед показом получает эту картинку поверх.
 * Рисуем внутри игры, поэтому оверлей виден и в полноэкранном режиме.
 *
 * Под Wine окно другого процесса снять нельзя: там поток берёт кадр из общей
 * памяти, куда его кладёт hlsr-share64.dll изнутри LiveSplit (share.c), и
 * через hlsr-overlay64.exe просит внедрить её в LiveSplit (только Linux).
 *
 * Настройки лежат в папке данных оверлея в overlay.ini; лаунчер переписывает
 * файл при сохранении персонализации, и поток подхватывает его на лету.
 * Журнал — overlay.log там же. */

#define COBJMACROS
#define CINTERFACE
#include "common.h"
#include <d3d9.h>

#define RS_ALLOC(n) HeapAlloc(GetProcessHeap(), 0, (n))
#define RS_FREE(p) HeapFree(GetProcessHeap(), 0, (p))
#include "resample.h"

/* ---- Журнал. ---- */

static WCHAR g_dir[MAX_PATH];
static HANDLE g_log = INVALID_HANDLE_VALUE;

static void log_line(const char *format, ...) {
  if (g_log == INVALID_HANDLE_VALUE) return;
  char line[512];
  SYSTEMTIME now;
  GetLocalTime(&now);
  int length = wsprintfA(line, "%02d:%02d:%02d.%03d ", now.wHour, now.wMinute, now.wSecond, now.wMilliseconds);
  va_list args;
  va_start(args, format);
  length += wvsprintfA(line + length, format, args);
  va_end(args);
  line[length++] = '\r';
  line[length++] = '\n';
  DWORD written;
  WriteFile(g_log, line, length, &written, 0);
}

static void path_in_dir(WCHAR *out, const WCHAR *name) {
  lstrcpyW(out, g_dir);
  lstrcatW(out, name);
}

/* ---- Общее состояние потока снимков и потока игры. ---- */

static CRITICAL_SECTION g_lock;
static OverlayConfig g_config;
static DWORD *g_frame;            /* предумноженный BGRA */
static int g_frameWidth, g_frameHeight;
/* Размер снимка до масштабирования: по нему считается место на экране.
 * g_frame уже пересчитан под это место, если поток знал размер экрана. */
static int g_frameSourceWidth, g_frameSourceHeight;
static volatile LONG g_screenWidth, g_screenHeight; /* последний кадр игры */
static LONG g_frameSeq;

/* Состояние запуска: см. overlay_start. */
#define READY_NEW 0
#define READY_STARTING 1
#define READY_RUNNING 2
#define READY_OFF 3
static volatile LONG g_ready;

/* Раз в секунду из потока снимков: прокси делает свои дела, которым нужна
 * игра уже запущенной (proxy-gl.c подменяет SwapBuffers в модулях игры). */
static void proxy_poll(void);
static DWORD g_frameTick;         /* когда снимок последний раз удался */
static volatile LONG g_d3dFrames __attribute__((unused)); /* кадры, нарисованные через Direct3D 9 */

/* Снимок старше этого не показываем: LiveSplit закрыт или свёрнут. */
#define STALE_MS 1500

typedef struct { int left, top, width, height; } Placement;

/* Высота — доля высоты экрана, ширина — по пропорциям снимка. x и y — где
 * стоит картинка в свободном месте: 0 — у левого (верхнего) края, 100 — у
 * правого (нижнего). Так положение не зависит от разрешения. */
static int place(const OverlayConfig *c, int screenWidth, int screenHeight, int width, int height, Placement *out) {
  if (screenWidth < 16 || screenHeight < 16 || width < 1 || height < 1) return 0;
  float h = screenHeight * c->height / 100, w = h * width / height;
  if (w > screenWidth) { w = (float)screenWidth; h = w * height / width; }
  out->width = (int)(w + 0.5f);
  out->height = (int)(h + 0.5f);
  out->left = (int)((screenWidth - out->width) * c->x / 100 + 0.5f);
  out->top = (int)((screenHeight - out->height) * c->y / 100 + 0.5f);
  return out->width > 0 && out->height > 0;
}

/* Что рисовать в этом кадре. Под замком: копия настроек, размер картинки и
 * размер снимка, по которому считается место на экране. */
static int frame_state(OverlayConfig *config, int *width, int *height, int *sourceWidth, int *sourceHeight, LONG *seq) {
  if (g_ready != READY_RUNNING) return 0;
  EnterCriticalSection(&g_lock);
  *config = g_config;
  *width = g_frameWidth;
  *height = g_frameHeight;
  *sourceWidth = g_frameSourceWidth;
  *sourceHeight = g_frameSourceHeight;
  *seq = g_frameSeq;
  int fresh = g_frame && GetTickCount() - g_frameTick < STALE_MS;
  LeaveCriticalSection(&g_lock);
  return config->enabled && fresh && config->opacity > 0;
}

#ifdef HLSR_D3D9

/* ---- Direct3D 9. ---- */

typedef struct { float x, y, z, rhw, u, v; } Vertex;

static IDirect3DDevice9 *g_d3dDevice;
static IDirect3DTexture9 *g_d3dTexture;
static IDirect3DStateBlock9 *g_d3dState;
static int g_d3dTextureWidth, g_d3dTextureHeight, g_d3dDynamic;
static LONG g_d3dSeq = -1;
static int g_d3dFailed;

static void d3d_release(void) {
  if (g_d3dTexture) IDirect3DTexture9_Release(g_d3dTexture);
  if (g_d3dState) IDirect3DStateBlock9_Release(g_d3dState);
  g_d3dTexture = 0;
  g_d3dState = 0;
  g_d3dSeq = -1;
}

static int d3d_upload(IDirect3DDevice9 *device, int width, int height, LONG seq) {
  if (g_d3dTexture && (g_d3dTextureWidth != width || g_d3dTextureHeight != height)) {
    IDirect3DTexture9_Release(g_d3dTexture);
    g_d3dTexture = 0;
  }
  if (!g_d3dTexture) {
    g_d3dDynamic = 1;
    HRESULT hr = IDirect3DDevice9_CreateTexture(device, width, height, 1, D3DUSAGE_DYNAMIC, D3DFMT_A8R8G8B8, D3DPOOL_DEFAULT, &g_d3dTexture, 0);
    if (FAILED(hr)) {
      g_d3dDynamic = 0;
      hr = IDirect3DDevice9_CreateTexture(device, width, height, 1, 0, D3DFMT_A8R8G8B8, D3DPOOL_MANAGED, &g_d3dTexture, 0);
    }
    if (FAILED(hr)) {
      g_d3dTexture = 0;
      if (!g_d3dFailed++) log_line("d3d9: текстура %dx%d не создана (0x%08lx)", width, height, hr);
      return 0;
    }
    g_d3dTextureWidth = width;
    g_d3dTextureHeight = height;
    g_d3dSeq = -1;
  }
  if (g_d3dSeq == seq) return 1;
  D3DLOCKED_RECT locked;
  if (FAILED(IDirect3DTexture9_LockRect(g_d3dTexture, 0, &locked, 0, g_d3dDynamic ? D3DLOCK_DISCARD : 0))) return 0;
  EnterCriticalSection(&g_lock);
  if (g_frame && g_frameWidth == width && g_frameHeight == height) {
    for (int y = 0; y < height; y++)
      CopyMemory((BYTE *)locked.pBits + y * locked.Pitch, g_frame + y * width, width * 4);
    g_d3dSeq = g_frameSeq;
  }
  LeaveCriticalSection(&g_lock);
  IDirect3DTexture9_UnlockRect(g_d3dTexture, 0);
  return 1;
}

static void d3d_draw(IDirect3DDevice9 *device) {
  OverlayConfig config;
  int width, height, sourceWidth, sourceHeight;
  LONG seq;
  if (device != g_d3dDevice) {
    /* Новое устройство (_restart, пересоздание). Текстура и блок состояний
     * держат по ссылке на своё устройство, так что прежнее ещё живо: отпускаем
     * их, и оно освобождается. Если их просто забыть, каждое новое устройство
     * оставляло бы в памяти старое целиком, с буферами. */
    d3d_release();
    g_d3dDevice = device;
    log_line("d3d9: устройство %p, таблица %p", device, *(void **)device);
  }
  if (!frame_state(&config, &width, &height, &sourceWidth, &sourceHeight, &seq)) return;

  IDirect3DSurface9 *backBuffer = 0;
  if (FAILED(IDirect3DDevice9_GetBackBuffer(device, 0, 0, D3DBACKBUFFER_TYPE_MONO, &backBuffer)) || !backBuffer) return;
  D3DSURFACE_DESC desc;
  Placement at;
  if (FAILED(IDirect3DSurface9_GetDesc(backBuffer, &desc)) ||
      !place(&config, desc.Width, desc.Height, sourceWidth, sourceHeight, &at) ||
      !d3d_upload(device, width, height, seq)) {
    IDirect3DSurface9_Release(backBuffer);
    return;
  }
  if (!g_d3dState && FAILED(IDirect3DDevice9_CreateStateBlock(device, D3DSBT_ALL, &g_d3dState))) {
    g_d3dState = 0;
    if (!g_d3dFailed++) log_line("d3d9: блок состояний не создан");
    IDirect3DSurface9_Release(backBuffer);
    return;
  }
  IDirect3DStateBlock9_Capture(g_d3dState);
  IDirect3DSurface9 *previousTarget = 0;
  IDirect3DDevice9_GetRenderTarget(device, 0, &previousTarget);
  IDirect3DDevice9_SetRenderTarget(device, 0, backBuffer);

  InterlockedExchange(&g_screenWidth, (LONG)desc.Width);
  InterlockedExchange(&g_screenHeight, (LONG)desc.Height);
  /* Картинка уже пересчитана под место на экране: рисуем пиксель в пиксель. */
  DWORD filter = width == at.width && height == at.height ? D3DTEXF_POINT : D3DTEXF_LINEAR;
  if (SUCCEEDED(IDirect3DDevice9_BeginScene(device))) {
    /* Через int: float → DWORD у MSVC x86 — вызов __ftol3 из CRT. */
    DWORD opacity = (DWORD)(int)(config.opacity * 2.55f + 0.5f);
    IDirect3DDevice9_SetVertexShader(device, 0);
    IDirect3DDevice9_SetPixelShader(device, 0);
    IDirect3DDevice9_SetFVF(device, D3DFVF_XYZRHW | D3DFVF_TEX1);
    IDirect3DDevice9_SetTexture(device, 0, (IDirect3DBaseTexture9 *)g_d3dTexture);
    IDirect3DDevice9_SetRenderState(device, D3DRS_ZENABLE, D3DZB_FALSE);
    IDirect3DDevice9_SetRenderState(device, D3DRS_ZWRITEENABLE, FALSE);
    IDirect3DDevice9_SetRenderState(device, D3DRS_ALPHATESTENABLE, FALSE);
    IDirect3DDevice9_SetRenderState(device, D3DRS_STENCILENABLE, FALSE);
    IDirect3DDevice9_SetRenderState(device, D3DRS_SCISSORTESTENABLE, FALSE);
    IDirect3DDevice9_SetRenderState(device, D3DRS_CLIPPLANEENABLE, 0);
    IDirect3DDevice9_SetRenderState(device, D3DRS_FOGENABLE, FALSE);
    IDirect3DDevice9_SetRenderState(device, D3DRS_LIGHTING, FALSE);
    IDirect3DDevice9_SetRenderState(device, D3DRS_CULLMODE, D3DCULL_NONE);
    IDirect3DDevice9_SetRenderState(device, D3DRS_FILLMODE, D3DFILL_SOLID);
    IDirect3DDevice9_SetRenderState(device, D3DRS_SRGBWRITEENABLE, FALSE);
    IDirect3DDevice9_SetRenderState(device, D3DRS_COLORWRITEENABLE, 0xF);
    IDirect3DDevice9_SetRenderState(device, D3DRS_ALPHABLENDENABLE, TRUE);
    IDirect3DDevice9_SetRenderState(device, D3DRS_SEPARATEALPHABLENDENABLE, FALSE);
    IDirect3DDevice9_SetRenderState(device, D3DRS_BLENDOP, D3DBLENDOP_ADD);
    IDirect3DDevice9_SetRenderState(device, D3DRS_SRCBLEND, D3DBLEND_ONE);
    IDirect3DDevice9_SetRenderState(device, D3DRS_DESTBLEND, D3DBLEND_INVSRCALPHA);
    IDirect3DDevice9_SetRenderState(device, D3DRS_TEXTUREFACTOR, D3DCOLOR_ARGB(opacity, opacity, opacity, opacity));
    IDirect3DDevice9_SetTextureStageState(device, 0, D3DTSS_COLOROP, D3DTOP_MODULATE);
    IDirect3DDevice9_SetTextureStageState(device, 0, D3DTSS_COLORARG1, D3DTA_TEXTURE);
    IDirect3DDevice9_SetTextureStageState(device, 0, D3DTSS_COLORARG2, D3DTA_TFACTOR);
    IDirect3DDevice9_SetTextureStageState(device, 0, D3DTSS_ALPHAOP, D3DTOP_MODULATE);
    IDirect3DDevice9_SetTextureStageState(device, 0, D3DTSS_ALPHAARG1, D3DTA_TEXTURE);
    IDirect3DDevice9_SetTextureStageState(device, 0, D3DTSS_ALPHAARG2, D3DTA_TFACTOR);
    IDirect3DDevice9_SetTextureStageState(device, 0, D3DTSS_TEXCOORDINDEX, 0);
    IDirect3DDevice9_SetTextureStageState(device, 0, D3DTSS_TEXTURETRANSFORMFLAGS, D3DTTFF_DISABLE);
    IDirect3DDevice9_SetTextureStageState(device, 1, D3DTSS_COLOROP, D3DTOP_DISABLE);
    IDirect3DDevice9_SetTextureStageState(device, 1, D3DTSS_ALPHAOP, D3DTOP_DISABLE);
    IDirect3DDevice9_SetSamplerState(device, 0, D3DSAMP_MINFILTER, filter);
    IDirect3DDevice9_SetSamplerState(device, 0, D3DSAMP_MAGFILTER, filter);
    IDirect3DDevice9_SetSamplerState(device, 0, D3DSAMP_MIPFILTER, D3DTEXF_NONE);
    IDirect3DDevice9_SetSamplerState(device, 0, D3DSAMP_ADDRESSU, D3DTADDRESS_CLAMP);
    IDirect3DDevice9_SetSamplerState(device, 0, D3DSAMP_ADDRESSV, D3DTADDRESS_CLAMP);
    IDirect3DDevice9_SetSamplerState(device, 0, D3DSAMP_SRGBTEXTURE, FALSE);

    /* Центры текселей Direct3D 9 сдвинуты на полпикселя. */
    float l = at.left - 0.5f, t = at.top - 0.5f, r = l + at.width, b = t + at.height;
    Vertex quad[4] = {
      { l, t, 0, 1, 0, 0 }, { r, t, 0, 1, 1, 0 },
      { l, b, 0, 1, 0, 1 }, { r, b, 0, 1, 1, 1 },
    };
    IDirect3DDevice9_DrawPrimitiveUP(device, D3DPT_TRIANGLESTRIP, 2, quad, sizeof(Vertex));
    IDirect3DDevice9_EndScene(device);
    if (InterlockedIncrement(&g_d3dFrames) == 1) log_line("d3d9: первый кадр с оверлеем, %ux%u", desc.Width, desc.Height);
  }

  if (previousTarget) {
    IDirect3DDevice9_SetRenderTarget(device, 0, previousTarget);
    IDirect3DSurface9_Release(previousTarget);
  }
  IDirect3DStateBlock9_Apply(g_d3dState);
  IDirect3DSurface9_Release(backBuffer);
}

#endif /* HLSR_D3D9 */

#ifdef HLSR_GL

/* ---- OpenGL. ---- */

typedef unsigned int GLenum, GLuint, GLbitfield;
typedef int GLint, GLsizei;
typedef float GLfloat;
typedef double GLdouble;
typedef unsigned char GLboolean;

#define GL_TEXTURE_2D 0x0DE1
#define GL_BLEND 0x0BE2
#define GL_DEPTH_TEST 0x0B71
#define GL_CULL_FACE 0x0B44
#define GL_ALPHA_TEST 0x0BC0
#define GL_FOG 0x0B60
#define GL_LIGHTING 0x0B50
#define GL_SCISSOR_TEST 0x0C11
#define GL_STENCIL_TEST 0x0B90
#define GL_TEXTURE_GEN_S 0x0C60
#define GL_TEXTURE_GEN_T 0x0C61
#define GL_COLOR_LOGIC_OP 0x0BF2
#define GL_FRAMEBUFFER_SRGB 0x8DB9
#define GL_ONE 1
#define GL_ONE_MINUS_SRC_ALPHA 0x0303
#define GL_PROJECTION 0x1701
#define GL_MODELVIEW 0x1700
#define GL_TEXTURE 0x1702
#define GL_TEXTURE_ENV 0x2300
#define GL_TEXTURE_ENV_MODE 0x2200
#define GL_MODULATE 0x2100
#define GL_TEXTURE_MIN_FILTER 0x2801
#define GL_TEXTURE_MAG_FILTER 0x2800
#define GL_TEXTURE_WRAP_S 0x2802
#define GL_TEXTURE_WRAP_T 0x2803
#define GL_LINEAR 0x2601
#define GL_NEAREST 0x2600
#define GL_CLAMP_TO_EDGE 0x812F
#define GL_RGBA8 0x8058
#define GL_BGRA 0x80E1
#define GL_UNSIGNED_BYTE 0x1401
#define GL_UNPACK_ALIGNMENT 0x0CF5
#define GL_UNPACK_ROW_LENGTH 0x0CF2
#define GL_UNPACK_SKIP_ROWS 0x0CF3
#define GL_UNPACK_SKIP_PIXELS 0x0CF4
#define GL_QUADS 0x0007
#define GL_FRONT_AND_BACK 0x0408
#define GL_FILL 0x1B02
#define GL_ALL_ATTRIB_BITS 0x000FFFFF
#define GL_CLIENT_ALL_ATTRIB_BITS 0xFFFFFFFF
#define GL_TEXTURE0 0x84C0
#define GL_CURRENT_PROGRAM 0x8B8D
#define GL_PIXEL_UNPACK_BUFFER 0x88EC
#define GL_PIXEL_UNPACK_BUFFER_BINDING 0x88EF
#define GL_MAX_TEXTURE_UNITS 0x84E2

#define GL_FUNCTIONS(X) \
  X(void, glEnable, (GLenum)) X(void, glDisable, (GLenum)) \
  X(void, glGetIntegerv, (GLenum, GLint *)) \
  X(void, glPushAttrib, (GLbitfield)) X(void, glPopAttrib, (void)) \
  X(void, glPushClientAttrib, (GLbitfield)) X(void, glPopClientAttrib, (void)) \
  X(void, glMatrixMode, (GLenum)) X(void, glPushMatrix, (void)) X(void, glPopMatrix, (void)) \
  X(void, glLoadIdentity, (void)) X(void, glOrtho, (GLdouble, GLdouble, GLdouble, GLdouble, GLdouble, GLdouble)) \
  X(void, glViewport, (GLint, GLint, GLsizei, GLsizei)) \
  X(void, glBlendFunc, (GLenum, GLenum)) X(void, glColor4f, (GLfloat, GLfloat, GLfloat, GLfloat)) \
  X(void, glColorMask, (GLboolean, GLboolean, GLboolean, GLboolean)) X(void, glPolygonMode, (GLenum, GLenum)) \
  X(void, glTexEnvi, (GLenum, GLenum, GLint)) X(void, glGenTextures, (GLsizei, GLuint *)) \
  X(void, glBindTexture, (GLenum, GLuint)) X(void, glTexParameteri, (GLenum, GLenum, GLint)) \
  X(void, glTexImage2D, (GLenum, GLint, GLint, GLsizei, GLsizei, GLint, GLenum, GLenum, const void *)) \
  X(void, glPixelStorei, (GLenum, GLint)) X(void, glBegin, (GLenum)) X(void, glEnd, (void)) \
  X(void, glTexCoord2f, (GLfloat, GLfloat)) X(void, glVertex2f, (GLfloat, GLfloat)) \
  X(HGLRC, wglGetCurrentContext, (void)) X(PROC, wglGetProcAddress, (LPCSTR))

#define GL_DECLARE(ret, name, args) static ret (WINAPI *p_##name) args;
GL_FUNCTIONS(GL_DECLARE)
static void (WINAPI *p_glActiveTexture)(GLenum);
static void (WINAPI *p_glUseProgram)(GLuint);
static void (WINAPI *p_glBindBuffer)(GLenum, GLuint);

static int gl_load(HMODULE gl) {
  int ok = 1;
#define GL_LOAD(ret, name, args) if (!(*(FARPROC *)&p_##name = GetProcAddress(gl, #name))) ok = 0;
  GL_FUNCTIONS(GL_LOAD)
  return ok;
}

static HGLRC g_glContext;
static GLuint g_glTexture;
static int g_glTextureWidth, g_glTextureHeight;
static LONG g_glSeq = -1;
static LONG g_glFrames;
static DWORD g_swapThread;

static void gl_extensions(void) {
  *(PROC *)&p_glActiveTexture = p_wglGetProcAddress("glActiveTexture");
  if (!p_glActiveTexture) *(PROC *)&p_glActiveTexture = p_wglGetProcAddress("glActiveTextureARB");
  *(PROC *)&p_glUseProgram = p_wglGetProcAddress("glUseProgram");
  *(PROC *)&p_glBindBuffer = p_wglGetProcAddress("glBindBuffer");
}

static void gl_draw(HDC hdc) {
  OverlayConfig config;
  int width, height, sourceWidth, sourceHeight;
  LONG seq;
  if (!frame_state(&config, &width, &height, &sourceWidth, &sourceHeight, &seq)) return;
  HGLRC context = p_wglGetCurrentContext();
  if (!context) return;
  if (context != g_glContext) {
    g_glContext = context;
    g_glTexture = 0;
    g_glSeq = -1;
    gl_extensions();
    log_line("opengl: контекст %p", context);
  }
  HWND window = WindowFromDC(hdc);
  RECT rc;
  Placement at;
  if (!window || !GetClientRect(window, &rc) || !place(&config, rc.right, rc.bottom, sourceWidth, sourceHeight, &at)) return;
  InterlockedExchange(&g_screenWidth, rc.right);
  InterlockedExchange(&g_screenHeight, rc.bottom);

  GLint program = 0, unpackBuffer = 0, units = 1;
  if (p_glUseProgram) p_glGetIntegerv(GL_CURRENT_PROGRAM, &program);
  if (p_glBindBuffer) p_glGetIntegerv(GL_PIXEL_UNPACK_BUFFER_BINDING, &unpackBuffer);
  if (p_glActiveTexture) p_glGetIntegerv(GL_MAX_TEXTURE_UNITS, &units);
  p_glPushAttrib(GL_ALL_ATTRIB_BITS);
  p_glPushClientAttrib(GL_CLIENT_ALL_ATTRIB_BITS);
  p_glMatrixMode(GL_TEXTURE); p_glPushMatrix(); p_glLoadIdentity();
  p_glMatrixMode(GL_PROJECTION); p_glPushMatrix(); p_glLoadIdentity();
  p_glOrtho(0, rc.right, rc.bottom, 0, -1, 1);
  p_glMatrixMode(GL_MODELVIEW); p_glPushMatrix(); p_glLoadIdentity();
  if (program && p_glUseProgram) p_glUseProgram(0);
  if (unpackBuffer && p_glBindBuffer) p_glBindBuffer(GL_PIXEL_UNPACK_BUFFER, 0);
  if (p_glActiveTexture) {
    for (int unit = units > 8 ? 7 : units - 1; unit > 0; unit--) {
      p_glActiveTexture(GL_TEXTURE0 + unit);
      p_glDisable(GL_TEXTURE_2D);
    }
    p_glActiveTexture(GL_TEXTURE0);
  }
  p_glViewport(0, 0, rc.right, rc.bottom);
  p_glDisable(GL_DEPTH_TEST); p_glDisable(GL_CULL_FACE); p_glDisable(GL_ALPHA_TEST);
  p_glDisable(GL_FOG); p_glDisable(GL_LIGHTING); p_glDisable(GL_SCISSOR_TEST);
  p_glDisable(GL_STENCIL_TEST); p_glDisable(GL_TEXTURE_GEN_S); p_glDisable(GL_TEXTURE_GEN_T);
  p_glDisable(GL_COLOR_LOGIC_OP); p_glDisable(GL_FRAMEBUFFER_SRGB);
  p_glColorMask(1, 1, 1, 1);
  p_glPolygonMode(GL_FRONT_AND_BACK, GL_FILL);
  p_glEnable(GL_TEXTURE_2D);
  p_glEnable(GL_BLEND);
  p_glBlendFunc(GL_ONE, GL_ONE_MINUS_SRC_ALPHA);
  p_glTexEnvi(GL_TEXTURE_ENV, GL_TEXTURE_ENV_MODE, GL_MODULATE);

  if (!g_glTexture) {
    p_glGenTextures(1, &g_glTexture);
    g_glTextureWidth = g_glTextureHeight = 0;
  }
  p_glBindTexture(GL_TEXTURE_2D, g_glTexture);
  if (g_glSeq != seq || g_glTextureWidth != width || g_glTextureHeight != height) {
    p_glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    p_glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
    p_glPixelStorei(GL_UNPACK_ALIGNMENT, 4);
    p_glPixelStorei(GL_UNPACK_ROW_LENGTH, 0);
    p_glPixelStorei(GL_UNPACK_SKIP_ROWS, 0);
    p_glPixelStorei(GL_UNPACK_SKIP_PIXELS, 0);
    EnterCriticalSection(&g_lock);
    if (g_frame && g_frameWidth == width && g_frameHeight == height) {
      p_glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA8, width, height, 0, GL_BGRA, GL_UNSIGNED_BYTE, g_frame);
      g_glTextureWidth = width;
      g_glTextureHeight = height;
      g_glSeq = g_frameSeq;
    }
    LeaveCriticalSection(&g_lock);
  }
  if (g_glTextureWidth == width && g_glTextureHeight == height) {
    /* Картинка уже пересчитана под место на экране: рисуем пиксель в пиксель. */
    GLint filter = width == at.width && height == at.height ? GL_NEAREST : GL_LINEAR;
    p_glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, filter);
    p_glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, filter);
    float o = config.opacity / 100;
    float l = (float)at.left, t = (float)at.top, r = l + at.width, b = t + at.height;
    p_glColor4f(o, o, o, o);
    p_glBegin(GL_QUADS);
    p_glTexCoord2f(0, 0); p_glVertex2f(l, t);
    p_glTexCoord2f(1, 0); p_glVertex2f(r, t);
    p_glTexCoord2f(1, 1); p_glVertex2f(r, b);
    p_glTexCoord2f(0, 1); p_glVertex2f(l, b);
    p_glEnd();
    if (++g_glFrames == 1) log_line("opengl: первый кадр с оверлеем, %ldx%ld", rc.right, rc.bottom);
  }

  p_glMatrixMode(GL_MODELVIEW); p_glPopMatrix();
  p_glMatrixMode(GL_PROJECTION); p_glPopMatrix();
  p_glMatrixMode(GL_TEXTURE); p_glPopMatrix();
  p_glPopClientAttrib();
  p_glPopAttrib();
  if (program && p_glUseProgram) p_glUseProgram(program);
  if (unpackBuffer && p_glBindBuffer) p_glBindBuffer(GL_PIXEL_UNPACK_BUFFER, unpackBuffer);
}

static int g_glLoaded;

/* Перед настоящим обменом буферов. 1 — внешний вызов: после обмена надо
 * позвать overlay_gl_after_swap. SwapBuffers из gdi32 может звать
 * wglSwapBuffers, поэтому рисуем только во внешнем вызове. */
static int overlay_gl_before_swap(HDC hdc, HMODULE realGl) {
  DWORD thread = GetCurrentThreadId();
  if (g_swapThread == thread) return 0;
  g_swapThread = thread;
  if (!g_glLoaded) {
    g_glLoaded = gl_load(realGl) ? 1 : -1;
    if (g_glLoaded < 0) log_line("opengl: нет нужных функций opengl32");
  }
  if (g_glLoaded > 0) gl_draw(hdc);
  return 1;
}

static void overlay_gl_after_swap(int outer) {
  if (outer) g_swapThread = 0;
}

#endif /* HLSR_GL */

/* ---- Поток снимков. ---- */

static FILETIME g_configTime;

static void reload_config(void) {
  WCHAR path[MAX_PATH];
  path_in_dir(path, L"overlay.ini");
  WIN32_FILE_ATTRIBUTE_DATA info;
  if (!GetFileAttributesExW(path, GetFileExInfoStandard, &info)) return;
  if (CompareFileTime(&info.ftLastWriteTime, &g_configTime) == 0) return;
  HANDLE file = CreateFileW(path, GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, 0, OPEN_EXISTING, 0, 0);
  if (file == INVALID_HANDLE_VALUE) return;
  static char text[4096];  /* только поток снимков; на стеке понадобился бы __chkstk */
  DWORD size = 0;
  BOOL ok = ReadFile(file, text, sizeof text, &size, 0);
  CloseHandle(file);
  if (!ok) return;
  OverlayConfig config;
  config_parse(&config, text, (int)size);
  g_configTime = info.ftLastWriteTime;
  EnterCriticalSection(&g_lock);
  g_config = config;
  LeaveCriticalSection(&g_lock);
  log_line("настройки: %s, высота %d%%, место %d/%d, маска %d", config.enabled ? "включён" : "выключен",
           (int)config.height, (int)config.x, (int)config.y, config.key);
}

/* Под Wine: внедрить hlsr-share64.dll в LiveSplit (attach в inject.c).
 * Помощник 64-битный, как LiveSplit; ждать его не нужно. Файлы лежат в
 * папке данных оверлея. */
static void attach_livesplit(void) {
  WCHAR helper[MAX_PATH], dll[MAX_PATH], command[MAX_PATH * 2 + 64];
  path_in_dir(helper, L"hlsr-overlay64.exe");
  path_in_dir(dll, L"hlsr-share64.dll");
  wsprintfW(command, L"\"%s\" attach --dll \"%s\"", helper, dll);
  STARTUPINFOW startup;
  PROCESS_INFORMATION info;
  ZeroMemory(&startup, sizeof startup);
  startup.cb = sizeof startup;
  if (CreateProcessW(helper, command, 0, 0, FALSE, CREATE_NO_WINDOW, 0, g_dir, &startup, &info)) {
    CloseHandle(info.hThread);
    CloseHandle(info.hProcess);
    log_line("LiveSplit: включаем снимок изнутри LiveSplit");
  } else {
    log_line("LiveSplit: помощник не запущен (ошибка %lu)", GetLastError());
  }
}

static DWORD WINAPI worker(LPVOID unused) {
  (void)unused;
  Canvas canvas;
  ZeroMemory(&canvas, sizeof canvas);
  ShareView share = { 0, 0 };
  int wine = is_wine();
  HWND window = 0;
  int method = 0, shared = 0;
  DWORD lastSearch = 0, lastConfig = 0, lastHook = 0, lastAttach = 0;
  DWORD *keyed = 0, *scaled = 0;
  int keyedCapacity = 0, scaledCapacity = 0;
  Resampler resampler;
  ZeroMemory(&resampler, sizeof resampler);
  if (wine) log_line("Wine: кадр LiveSplit берётся из общей памяти");

  for (;;) {
    DWORD now = GetTickCount();
    if (now - lastConfig >= 500 || !lastConfig) { lastConfig = now; reload_config(); }
    if (now - lastHook >= 1000 || !lastHook) { lastHook = now; proxy_poll(); }

    EnterCriticalSection(&g_lock);
    OverlayConfig config = g_config;
    LeaveCriticalSection(&g_lock);
    if (!config.enabled) { Sleep(250); continue; }

    int captured = 0;
    if (wine) {
      captured = share_read(&share, &canvas, STALE_MS);
      if (captured != shared) { shared = captured; log_line(captured ? "LiveSplit: кадры идут" : "LiveSplit: кадров нет"); }
      /* Кадров нет, а окно LiveSplit есть: DLL в нём ещё не внедрена. */
      if (!captured && now - lastAttach >= 5000 && find_livesplit()) { lastAttach = now; attach_livesplit(); }
    } else {
      if (window && !IsWindow(window)) window = 0;
      if (!window && now - lastSearch >= 1000) {
        lastSearch = now;
        window = find_livesplit();
        method = 0;
        if (window) log_line("LiveSplit: окно %p", window);
      }
      captured = window && capture_window(window, &canvas, &method);
      if (!captured && window && method == 0) window = 0;
    }
    if (captured) {
      RECT crop;
      crop_rect(&config, canvas.width, canvas.height, &crop);
      int width = crop.right - crop.left, height = crop.bottom - crop.top;
      if (width * height > keyedCapacity) {
        if (keyed) HeapFree(GetProcessHeap(), 0, keyed);
        keyedCapacity = width * height;
        keyed = (DWORD *)HeapAlloc(GetProcessHeap(), 0, keyedCapacity * 4);
        if (!keyed) keyedCapacity = 0;
      }
      if (keyed) {
        key_pixels(&config, canvas.bits, canvas.width, &crop, keyed);
        /* Пересчитываем под место на экране здесь, а не текстурой в игре. */
        DWORD **out = &keyed;
        int *outCapacity = &keyedCapacity, outWidth = width, outHeight = height;
        Placement at;
        if (place(&config, (int)g_screenWidth, (int)g_screenHeight, width, height, &at) &&
            (at.width != width || at.height != height) && at.width <= MAX_SIDE && at.height <= MAX_SIDE) {
          if (at.width * at.height > scaledCapacity) {
            if (scaled) HeapFree(GetProcessHeap(), 0, scaled);
            scaledCapacity = at.width * at.height;
            scaled = (DWORD *)HeapAlloc(GetProcessHeap(), 0, scaledCapacity * 4);
            if (!scaled) scaledCapacity = 0;
          }
          if (scaled && resample(&resampler, (const unsigned *)keyed, width, height, (unsigned *)scaled, at.width, at.height)) {
            out = &scaled;
            outCapacity = &scaledCapacity;
            outWidth = at.width;
            outHeight = at.height;
          }
        }
        EnterCriticalSection(&g_lock);
        /* Поток игры читает g_frame под этим же замком; меняемся буферами. */
        DWORD *previous = g_frame;
        int previousCapacity = g_frame ? g_frameWidth * g_frameHeight : 0;
        g_frame = *out;
        g_frameWidth = outWidth;
        g_frameHeight = outHeight;
        g_frameSourceWidth = width;
        g_frameSourceHeight = height;
        g_frameSeq++;
        g_frameTick = GetTickCount();
        LeaveCriticalSection(&g_lock);
        *out = previous;
        *outCapacity = previousCapacity;
      }
    }
    Sleep(33);
  }
  return 0;
}

/* Запуск из прокси при первом вызове игры. Папку данных (overlay.ini,
 * overlay.log, помощники Wine) прокси берёт у лаунчера; без неё оверлей
 * выключен, и игра работает как без него. */
static void overlay_start(void) {
  if (InterlockedCompareExchange(&g_ready, READY_STARTING, READY_NEW) != READY_NEW) return;
  if (!proxy_data_dir(g_dir, MAX_PATH)) { InterlockedExchange(&g_ready, READY_OFF); return; }
  WCHAR logPath[MAX_PATH];
  path_in_dir(logPath, L"overlay.log");
  g_log = CreateFileW(logPath, GENERIC_WRITE, FILE_SHARE_READ, 0, CREATE_ALWAYS, 0, 0);
  InitializeCriticalSection(&g_lock);
  config_defaults(&g_config);
  log_line("hlsr-overlay (%s) загружен в процесс %lu", PROXY_NAME_A, GetCurrentProcessId());
  InterlockedExchange(&g_ready, READY_RUNNING);
  HANDLE thread = CreateThread(0, 0, worker, 0, 0, 0);
  if (thread) CloseHandle(thread);
}
