/* Масштабирование снимка LiveSplit под размер на экране.
 *
 * Игра рисует картинку ровно в её размере, без растяжения текстуры: иначе
 * билинейный фильтр размывает текст при увеличении и рвёт его при
 * уменьшении (без мипмапов он пропускает пиксели). Фильтр Catmull-Rom,
 * разделимый: сначала строки, потом столбцы. При уменьшении ядро
 * расширяется на коэффициент, и каждый пиксель усредняет всю свою площадь.
 *
 * Пиксели — BGRA с предумноженной альфой; так фильтр не тянет цвет
 * прозрачного фона в края букв. Без CRT: память даёт RS_ALLOC/RS_FREE. */

#ifndef HLSR_RESAMPLE_H
#define HLSR_RESAMPLE_H

#ifndef RS_ALLOC
#error "RS_ALLOC и RS_FREE задаёт тот, кто подключает resample.h"
#endif

typedef struct {
  int start, count;      /* первый пиксель источника и число отсчётов */
  int offset;            /* где лежат их веса в общем массиве */
} RsTap;

typedef struct {
  float *rows;           /* промежуточный проход: dstWidth x srcHeight x 4 */
  int rowsCapacity;
  RsTap *taps;
  int tapsCapacity;
  float *weights;
  int weightsCapacity;
} Resampler;

static int rs_floor(float v) {
  int i = (int)v;
  return v < (float)i ? i - 1 : i;
}

static float rs_kernel(float x) {
  if (x < 0) x = -x;
  if (x < 1) return (1.5f * x - 2.5f) * x * x + 1;
  if (x < 2) return ((-0.5f * x + 2.5f) * x - 4) * x + 2;
  return 0;
}

static int rs_grow(void **buffer, int *capacity, int needed, int size) {
  if (needed <= *capacity) return 1;
  if (*buffer) RS_FREE(*buffer);
  *buffer = RS_ALLOC((unsigned)needed * (unsigned)size);
  *capacity = *buffer ? needed : 0;
  return *buffer != 0;
}

static float rs_support(int src, int dst) {
  float ratio = (float)src / dst;
  return ratio > 1 ? 2 * ratio : 2;
}

static int rs_per_tap(int src, int dst) {
  return rs_floor(rs_support(src, dst) * 2) + 2;
}

/* Отсчёты одной оси: taps[0..dst), их веса подряд с weights. Возвращает,
 * сколько весов занято. */
static int rs_axis(int src, int dst, RsTap *taps, float *weights) {
  float ratio = (float)src / dst;
  float support = rs_support(src, dst);
  float stretch = ratio > 1 ? 1 / ratio : 1;
  int perTap = rs_per_tap(src, dst);
  int used = 0;
  for (int i = 0; i < dst; i++) {
    float center = (i + 0.5f) * ratio - 0.5f;
    int first = rs_floor(center - support) + 1, last = rs_floor(center + support);
    if (first < 0) first = 0;
    if (last > src - 1) last = src - 1;
    if (last - first + 1 > perTap) last = first + perTap - 1;
    float sum = 0;
    float *w = weights + used;
    for (int s = first; s <= last; s++) sum += w[s - first] = rs_kernel((s - center) * stretch);
    if (sum == 0) {
      first = rs_floor(center + 0.5f);
      if (first < 0) first = 0;
      if (first > src - 1) first = src - 1;
      last = first;
      w[0] = sum = 1;
    }
    for (int s = 0; s <= last - first; s++) w[s] /= sum;
    taps[i].start = first;
    taps[i].count = last - first + 1;
    taps[i].offset = used;
    used += last - first + 1;
  }
  return used;
}

static unsigned rs_pack(const float *p) {
  float a = p[3] < 0 ? 0 : p[3] > 255 ? 255 : p[3];
  /* Через int: float → unsigned у MSVC x86 — вызов __ftol3 из CRT. */
  unsigned out = (unsigned)(int)(a + 0.5f) << 24;
  for (int c = 0; c < 3; c++) {
    /* Отрицательные лепестки ядра дают выбросы: цвет не выходит за альфу. */
    float v = p[c] < 0 ? 0 : p[c] > a ? a : p[c];
    out |= (unsigned)(int)(v + 0.5f) << (c * 8);
  }
  return out;
}

/* src (srcWidth x srcHeight) → dst (dstWidth x dstHeight). 0 — нет памяти. */
static int resample(Resampler *r, const unsigned *src, int srcWidth, int srcHeight,
                    unsigned *dst, int dstWidth, int dstHeight) {
  if (srcWidth < 1 || srcHeight < 1 || dstWidth < 1 || dstHeight < 1) return 0;
  int weights = dstWidth * rs_per_tap(srcWidth, dstWidth) + dstHeight * rs_per_tap(srcHeight, dstHeight);
  if (!rs_grow((void **)&r->taps, &r->tapsCapacity, dstWidth + dstHeight, sizeof(RsTap)) ||
      !rs_grow((void **)&r->weights, &r->weightsCapacity, weights, sizeof(float)) ||
      !rs_grow((void **)&r->rows, &r->rowsCapacity, dstWidth * srcHeight * 4, sizeof(float)))
    return 0;
  RsTap *xTaps = r->taps, *yTaps = r->taps + dstWidth;
  float *xWeights = r->weights;
  float *yWeights = xWeights + rs_axis(srcWidth, dstWidth, xTaps, xWeights);
  rs_axis(srcHeight, dstHeight, yTaps, yWeights);

  for (int y = 0; y < srcHeight; y++) {
    const unsigned *row = src + y * srcWidth;
    float *out = r->rows + y * dstWidth * 4;
    for (int x = 0; x < dstWidth; x++, out += 4) {
      const RsTap *tap = xTaps + x;
      const float *w = xWeights + tap->offset;
      float b = 0, g = 0, rd = 0, a = 0;
      for (int k = 0; k < tap->count; k++) {
        unsigned p = row[tap->start + k];
        b += w[k] * (float)(p & 255);
        g += w[k] * (float)((p >> 8) & 255);
        rd += w[k] * (float)((p >> 16) & 255);
        a += w[k] * (float)(p >> 24);
      }
      out[0] = b; out[1] = g; out[2] = rd; out[3] = a;
    }
  }
  for (int y = 0; y < dstHeight; y++) {
    const RsTap *tap = yTaps + y;
    const float *w = yWeights + tap->offset;
    unsigned *out = dst + y * dstWidth;
    for (int x = 0; x < dstWidth; x++) {
      float p[4] = { 0, 0, 0, 0 };
      for (int k = 0; k < tap->count; k++) {
        const float *q = r->rows + ((tap->start + k) * dstWidth + x) * 4;
        p[0] += w[k] * q[0]; p[1] += w[k] * q[1]; p[2] += w[k] * q[2]; p[3] += w[k] * q[3];
      }
      out[x] = rs_pack(p);
    }
  }
  return 1;
}

#endif
