<template>

  <div class="launcher-bg" aria-hidden="true">
    <transition name="launcher-bg-fade">
      <div
        v-if="shownSrc"
        :key="shownSrc"
        class="launcher-bg__image"
        :style="{ backgroundImage: 'url(' + shownSrc + ')' }"
      ></div>
    </transition>

    <transition name="launcher-bg-fade">
      <!-- Шарики — тайная игра «повтори цвета» (onOrbClick): ни курсор, ни
           вид не выдают, что по ним можно щёлкать. -->
      <div v-if="toolBackground" class="launcher-bg__tool">
        <span class="launcher-bg__orb launcher-bg__orb--red" :class="{ 'is-lit': litOrb === 0 || litOrb === ALL_ORBS, 'is-miss': memoryMiss }" @click="onOrbClick(0)"></span>
        <span class="launcher-bg__orb launcher-bg__orb--blue" :class="{ 'is-lit': litOrb === 1 || litOrb === ALL_ORBS, 'is-miss': memoryMiss }" @click="onOrbClick(1)"></span>
        <span class="launcher-bg__orb launcher-bg__orb--green" :class="{ 'is-lit': litOrb === 2 || litOrb === ALL_ORBS, 'is-miss': memoryMiss }" @click="onOrbClick(2)"></span>
      </div>
    </transition>

    <div class="launcher-bg__scrim"></div>

    <!-- Передний план: вырезанные фигуры поверх затемнения. Лежит ПОСЛЕ
         scrim намеренно — фон уходит в темноту, а герои остаются яркими,
         и между ними появляется воздух. Без картинки слоя нет вовсе. -->
    <transition name="launcher-bg-fade">
      <div
        v-if="shownFront"
        :key="shownFront"
        class="launcher-bg__front"
        :style="frontStyle"
      ></div>
    </transition>
  </div>
</template>

<script>
const memoryGame = require("./memory-game");

const { ipcRenderer } = window.hlsrElectron;

// «Повтори цвета»: три шарика; пауза перед показом, шаг показа и время
// подсветки одного шарика, мс.
const ORB_COUNT = 3;
// Пауза перед показом с запасом после вспышки своего щелчка: иначе они
// сливались, и щелчок принимали за первый цвет показа.
const MEMORY_PAUSE = 1100;
const MEMORY_STEP = 600;
const MEMORY_LIT = 400;
// Пауза без щелчков, после которой следующий щелчок начинает новую игру, мс:
// брошенная игра не ждёт ответа вечно и не принимает «старт» за ошибку.
const MEMORY_IDLE = 5000;
// Ошибка: шарики гаснут на это время, мс.
const MEMORY_MISS = 700;
// litOrb: горят все три (старт игры).
const ALL_ORBS = 3;

const PARALLAX_OVERSCAN = 0.05;
const PARALLAX_ZOOM = 1 + PARALLAX_OVERSCAN;
const PARALLAX_EDGE_MARGIN = PARALLAX_OVERSCAN / 2;
const PARALLAX_AMPLITUDE_OF_EDGE_MARGIN = 0.72;
const PARALLAX_AMPLITUDE = PARALLAX_EDGE_MARGIN * PARALLAX_AMPLITUDE_OF_EDGE_MARGIN;
const PARALLAX_MIN_SAFE_WINDOW_PX = 240;
const PARALLAX_MIN_AMPLITUDE_PX = PARALLAX_MIN_SAFE_WINDOW_PX * PARALLAX_EDGE_MARGIN;
// Время, за которое слой почти догоняет курсор. Сглаживание считается по
// реальному времени кадра, а не долей за кадр: иначе на 144 Гц слой ехал
// бы вдвое быстрее, чем на 60 Гц.
const PARALLAX_SMOOTH_TIME = 0.32;
const PARALLAX_MAX_DT = 0.05;
const PARALLAX_EPSILON = 0.05;
// Фигуры ближе к зрителю, чем город за ними, и ходят вдвое дальше
const FRONT_DEPTH = 2;
const FRONT_TILT_DEG = 1.3;

export default {
  name: "LauncherBackground",
  props: {
    toolBackground: { type: Boolean, default: false },
    src: {
      type: String,
      default: null,
    },
    foreground: {
      type: String,
      default: null,
    },
    parallax: {
      type: Boolean,
      default: true,
    },
    // Поверх открыто окно (профиль, настройки…): фон под ним замирает.
    paused: {
      type: Boolean,
      default: false,
    },
    // Раскладка вырезки: CSS-переменные --front-*. Без неё действуют
    // значения по умолчанию, подобранные под Гордона и Аликс из Ghosting.
    frontLayout: {
      type: Object,
      default: null,
    },
  },
  data() {
    return {
      shownSrc: null,
      shownFront: null,
      // Раскладка той картинки, что видна сейчас. Новая раскладка приходит
      // сразу при смене игры, а картинка — только после загрузки, и старая
      // картинка успевала прыгнуть на место новой (из центра у HL1 вбок
      // у Ghosting и обратно). Поэтому они меняются вместе.
      shownLayout: null,
      // Подсвеченный шарик «повтори цвета», -1 — ни один.
      litOrb: -1,
      // Ошибка в «повтори цвета»: шарики коротко гаснут.
      memoryMiss: false,
      ALL_ORBS,
    };
  },
  computed: {
    frontStyle() {
      return Object.assign({ backgroundImage: "url(" + this.shownFront + ")" }, this.shownLayout || {});
    },
  },
  watch: {
    src: {
      immediate: true,
      handler(value) {
        this.preload(value);
      },
    },
    foreground: {
      immediate: true,
      handler(value) {
        this.preloadFront(value);
      },
    },
    frontLayout(value) {
      if (this.foreground === this.shownFront) this.shownLayout = value;
    },
    parallax() {
      this.applyParallaxMode();
    },
    paused(value) {
      if (value) this.freeze();
    },
    toolBackground(value) {
      if (!value) this.resetMemory();
    },
  },
  methods: {
    // Пасхалка «Хорошая память!»: щелчок по любому шарику начинает игру
    // (все три вспыхивают), дальше игрок повторяет показанные цвета. Ошибка
    // гасит шарики и
    // заканчивает игру, восьмой раунд открывает секретное достижение.
    onOrbClick(index) {
      if (this.memoryPlaying) return;
      const now = performance.now();
      if (!this.memory || now - this.memoryLast > MEMORY_IDLE) {
        // Любой шарик начинает игру; старт видно: вспыхивают все три.
        this.memory = memoryGame.start(ORB_COUNT);
        this.memoryMiss = false;
        this.flashOrb(ALL_ORBS);
        this.playMemory();
        return;
      }
      this.memoryLast = now;
      const { game, result } = memoryGame.press(this.memory, index, ORB_COUNT);
      this.memory = game;
      if (result === "wrong") {
        // Ошибку видно: шарики гаснут; следующий щелчок — новая игра.
        clearTimeout(this.litTimer);
        this.litOrb = -1;
        this.memoryMiss = true;
        this.memoryTimers.push(setTimeout(() => { this.memoryMiss = false; }, MEMORY_MISS));
        return;
      }
      this.flashOrb(index);
      if (result === "won") ipcRenderer.invoke("runner-secret-unlock", "good-memory").catch(() => {});
      if (result === "round") this.playMemory();
    },
    playMemory() {
      this.memoryPlaying = true;
      const sequence = this.memory.sequence;
      sequence.forEach((orb, i) => this.memoryTimers.push(setTimeout(() => this.flashOrb(orb), MEMORY_PAUSE + i * MEMORY_STEP)));
      // Ввод открыт с последней вспышки: иначе быстрый щелчок сразу после
      // показа тихо терялся, и повтор того же цвета считался ошибкой.
      this.memoryTimers.push(setTimeout(() => {
        this.memoryPlaying = false;
        this.memoryLast = performance.now();
      }, MEMORY_PAUSE + (sequence.length - 1) * MEMORY_STEP));
    },
    flashOrb(index) {
      clearTimeout(this.litTimer);
      if (this.litOrb === index) {
        // Тот же шарик ещё горит (быстрый повтор): гасим на миг, чтобы каждый
        // щелчок был отдельной вспышкой, а не одной длинной.
        this.litOrb = -1;
        this.litTimer = setTimeout(() => this.flashOrb(index), 60);
        return;
      }
      this.litOrb = index;
      this.litTimer = setTimeout(() => { this.litOrb = -1; }, MEMORY_LIT);
    },
    resetMemory() {
      this.memoryTimers.forEach(clearTimeout);
      clearTimeout(this.litTimer);
      this.memoryTimers = [];
      this.memory = null;
      this.memoryPlaying = false;
      this.memoryMiss = false;
      this.litOrb = -1;
    },
    preload(value) {
      this.release();

      if (!value) {
        this.shownSrc = null;
        return;
      }

      if (value === this.shownSrc) return;

      const image = new Image();
      this.pending = image;

      image.onload = () => {
        if (this.src === value) this.shownSrc = value;
        this.release();
      };

      image.onerror = () => {
        if (this.src === value) this.shownSrc = null;
        this.release();
      };

      image.src = value;
    },

    preloadFront(value) {
      if (this.pendingFront) {
        this.pendingFront.onload = null;
        this.pendingFront.onerror = null;
        this.pendingFront = null;
      }

      if (!value) {
        this.shownFront = null;
        this.shownLayout = null;
        return;
      }

      if (value === this.shownFront) {
        this.shownLayout = this.frontLayout;
        return;
      }

      const image = new Image();
      this.pendingFront = image;

      image.onload = () => {
        if (this.foreground === value) {
          this.shownFront = value;
          this.shownLayout = this.frontLayout;
        }
        this.pendingFront = null;
      };

      image.onerror = () => {
        if (this.foreground === value) this.shownFront = null;
        this.pendingFront = null;
      };

      image.src = value;
    },
    release() {
      if (!this.pending) return;

      this.pending.onload = null;
      this.pending.onerror = null;
      this.pending = null;
    },

    parallaxEnabled() {
      return this.parallax && !this.reducedMotion;
    },

    applyParallaxMode() {
      const root = this.$el;
      if (!root || !root.style) return;

      if (!this.parallaxEnabled()) {
        this.stopLoop();
        this.targetX = this.targetY = this.currentX = this.currentY = 0;
        this.velocityX = this.velocityY = 0;
        root.style.removeProperty("--l-bg-par-zoom");
        root.style.setProperty("--l-bg-par-x", "0px");
        root.style.setProperty("--l-bg-par-y", "0px");
        root.style.setProperty("--l-bg-front-x", "0px");
        root.style.setProperty("--l-bg-front-y", "0px");
        root.style.setProperty("--l-bg-front-rx", "0deg");
        root.style.setProperty("--l-bg-front-ry", "0deg");
        return;
      }

      root.style.setProperty("--l-bg-par-zoom", String(PARALLAX_ZOOM));
      this.measure();
      this.startLoop();
    },

    measure() {
      const root = this.$el;
      if (!root) return;

      const width = root.clientWidth || window.innerWidth || 0;
      const height = root.clientHeight || window.innerHeight || 0;

      this.ampX = Math.max(PARALLAX_MIN_AMPLITUDE_PX, width * PARALLAX_AMPLITUDE);
      this.ampY = Math.max(PARALLAX_MIN_AMPLITUDE_PX, height * PARALLAX_AMPLITUDE);
    },

    onPointerMove(event) {
      // Chromium присылает mousemove и неактивному окну, когда курсор
      // проходит над ним: без фокуса фон стоит на месте.
      if (!this.parallaxEnabled() || !this.tracking()) return;

      const width = window.innerWidth || 1;
      const height = window.innerHeight || 1;

      const nx = (event.clientX / width) * 2 - 1;
      const ny = (event.clientY / height) * 2 - 1;

      this.targetX = -clamp(nx, -1, 1) * this.ampX;
      this.targetY = -clamp(ny, -1, 1) * this.ampY;
      this.startLoop();
    },

    onPointerLeave() {
      if (!this.tracking()) return;
      this.targetX = 0;
      this.targetY = 0;
      this.startLoop();
    },

    startLoop() {
      if (this.frame != null) return;
      if (!this.parallaxEnabled()) return;
      if (!this.tracking() || !this.windowVisible || document.hidden) return;

      this.frame = window.requestAnimationFrame(this.step);
    },

    stopLoop() {
      this.lastFrameTime = null;
      if (this.frame == null) return;
      window.cancelAnimationFrame(this.frame);
      this.frame = null;
    },

    step(now) {
      this.frame = null;

      const last = this.lastFrameTime;
      this.lastFrameTime = now;
      // Первый кадр после паузы считается обычным, а не скачком за всё
      // время простоя
      const dt = last == null ? 1 / 60 : Math.min(Math.max((now - last) / 1000, 0), PARALLAX_MAX_DT);

      const x = smoothDamp(this.currentX, this.targetX, this.velocityX, dt);
      const y = smoothDamp(this.currentY, this.targetY, this.velocityY, dt);
      this.currentX = x.value;
      this.velocityX = x.velocity;
      this.currentY = y.value;
      this.velocityY = y.velocity;

      const settled =
        Math.abs(this.targetX - this.currentX) < PARALLAX_EPSILON &&
        Math.abs(this.targetY - this.currentY) < PARALLAX_EPSILON &&
        Math.abs(this.velocityX) < PARALLAX_EPSILON &&
        Math.abs(this.velocityY) < PARALLAX_EPSILON;

      if (settled) {
        this.currentX = this.targetX;
        this.currentY = this.targetY;
        this.velocityX = this.velocityY = 0;
        this.lastFrameTime = null;
        this.write();
        return;
      }

      this.write();
      this.startLoop();
    },

    write() {
      const root = this.$el;
      if (!root || !root.style) return;

      root.style.setProperty("--l-bg-par-x", this.currentX.toFixed(2) + "px");
      root.style.setProperty("--l-bg-par-y", this.currentY.toFixed(2) + "px");

      root.style.setProperty("--l-bg-front-x", (this.currentX * FRONT_DEPTH).toFixed(2) + "px");
      root.style.setProperty("--l-bg-front-y", (this.currentY * FRONT_DEPTH).toFixed(2) + "px");

      const tiltY = this.ampX ? (this.currentX / this.ampX) * FRONT_TILT_DEG : 0;
      const tiltX = this.ampY ? (-this.currentY / this.ampY) * FRONT_TILT_DEG : 0;

      root.style.setProperty("--l-bg-front-ry", tiltY.toFixed(3) + "deg");
      root.style.setProperty("--l-bg-front-rx", tiltX.toFixed(3) + "deg");
    },

    onResize() {
      if (!this.parallaxEnabled()) return;
      this.measure();
      this.targetX = clamp(this.targetX, -this.ampX, this.ampX);
      this.targetY = clamp(this.targetY, -this.ampY, this.ampY);
      this.startLoop();
    },

    onWindowVisibility(event, visible) {
      this.windowVisible = visible === true;
      if (this.windowVisible) this.startLoop();
      else this.stopLoop();
    },
    // Фон следует за курсором, только пока окно лаунчера в фокусе и
    // поверх не открыто другое окно.
    tracking() {
      return this.focused && !this.paused;
    },
    // Фон замирает там, где был, без возврата в центр, и потом снова
    // следует за курсором с того же места.
    freeze() {
      this.velocityX = this.velocityY = 0;
      this.targetX = this.currentX;
      this.targetY = this.currentY;
      this.stopLoop();
    },
    onWindowBlur() {
      this.focused = false;
      this.freeze();
    },
    onWindowFocus() {
      this.focused = true;
    },
    onVisibilityChange() {
      if (document.hidden) this.stopLoop();
      else this.startLoop();
    },
  },
  created() {
    this.pending = null;
    this.windowVisible = true;
    this.focused = typeof document.hasFocus === "function" ? document.hasFocus() : true;
    this.frame = null;
    this.targetX = 0;
    this.targetY = 0;
    this.currentX = 0;
    this.currentY = 0;
    this.velocityX = 0;
    this.velocityY = 0;
    this.lastFrameTime = null;
    this.ampX = PARALLAX_MIN_AMPLITUDE_PX;
    this.ampY = PARALLAX_MIN_AMPLITUDE_PX;
    this.memory = null;
    this.memoryPlaying = false;
    this.memoryLast = 0;
    this.memoryTimers = [];

    const query =
      typeof window.matchMedia === "function"
        ? window.matchMedia("(prefers-reduced-motion: reduce)")
        : null;

    this.reducedMotion = !!(query && query.matches);
  },
  mounted() {
    ipcRenderer.on("window-visibility", this.onWindowVisibility);
    document.addEventListener("visibilitychange", this.onVisibilityChange);
    window.addEventListener("mousemove", this.onPointerMove, { passive: true });
    window.addEventListener("blur", this.onWindowBlur);
    window.addEventListener("focus", this.onWindowFocus);
    document.addEventListener("mouseleave", this.onPointerLeave);
    window.addEventListener("resize", this.onResize);

    this.applyParallaxMode();
  },
  beforeUnmount() {
    this.release();
    this.stopLoop();
    this.resetMemory();
    ipcRenderer.removeListener("window-visibility", this.onWindowVisibility);
    document.removeEventListener("visibilitychange", this.onVisibilityChange);
    window.removeEventListener("mousemove", this.onPointerMove);
    window.removeEventListener("blur", this.onWindowBlur);
    window.removeEventListener("focus", this.onWindowFocus);
    document.removeEventListener("mouseleave", this.onPointerLeave);
    window.removeEventListener("resize", this.onResize);
  },
};

// Критически демпфированная пружина: слой плавно разгоняется и плавно
// тормозит у цели, без перелёта. Приближение экспоненты взято из
// Game Programming Gems 4 (то же, что Mathf.SmoothDamp в Unity).
function smoothDamp(current, target, velocity, dt) {
  const omega = 2 / PARALLAX_SMOOTH_TIME;
  const x = omega * dt;
  const decay = 1 / (1 + x + 0.48 * x * x + 0.235 * x * x * x);
  const change = current - target;
  const temp = (velocity + omega * change) * dt;

  return {
    value: target + (change + temp) * decay,
    velocity: (velocity - omega * temp) * decay,
  };
}

function clamp(value, min, max) {
  return value < min ? min : value > max ? max : value;
}
</script>

<style lang="scss" scoped>
/* Один слой на всю клиентскую площадь окна: кадр игры идёт под sidebar,
   Hero и нижней панелью и читается сквозь них. Второй копии кадра нигде
   нет.
   Геометрия интерфейса при этом не меняется: слой абсолютный и в потоке
   не участвует. */
.launcher-bg {
  position: absolute;
  inset: var(--l-bg-inset, 0);
  overflow: hidden;
  pointer-events: none;
  z-index: 0;
  /* Виден, пока картинка грузится, и если она не загрузилась вовсе */
  background: var(--l-bg-fallback);
  /* Единицы cqw/cqh у слоёв ниже отсчитываются от этой рамки */
  container-type: size;

  &__image {
    position: absolute;
    inset: 0;
    background-size: cover;
    background-position: center top;
    background-repeat: no-repeat;
    pointer-events: none;
    /* Порядок важен: сдвиг оформления и сдвиг параллакса складываются в
       экранных пикселях, а увеличение применяется после них — иначе
       амплитуда параллакса умножалась бы на масштаб и запас, посчитанный
       под 1.8%, перестал бы быть запасом.
       Значения по умолчанию (0px, 0px, 1) дают в точности тот кадр, что
       был до появления параллакса: выключенная настройка ничего не двигает. */
    transform:
      translateX(var(--l-bg-shift-x, 0))
      translate3d(var(--l-bg-par-x, 0px), var(--l-bg-par-y, 0px), 0)
      scale(var(--l-bg-zoom, 1))
      scale(var(--l-bg-par-zoom, 1));
    /* Слой едет каждый кадр — на своём слое композитора, без перерисовки */
    will-change: transform;
  }

  &__scrim {
    position: absolute;
    inset: 0;
    background: var(--l-bg-scrim);
    pointer-events: none;
  }

  /* Передний план. Лежит поверх затемнения, поэтому фигуры остаются яркими
     на притушенном фоне — из этого контраста и берётся глубина.

     perspective + rotate дают наклон «как у карточки». */
  &__front {
    /* Запас слоя за краями окна. Слой едет и наклоняется вместе с курсором,
       а картинка рисуется только внутри его рамки: при рамке ровно по окну
       её нижний край въезжал в кадр, и фигуры обрывались прямой линией над
       низом окна. Запас больше любого хода: сдвиг до ~4% ширины и высоты
       плюс наклон. Верх не расширяется — над головами в картинке
       прозрачное поле, и рамка там не видна. */
    --front-bleed-x: 12cqw;
    --front-bleed-y: 30cqh;

    position: absolute;
    top: 0;
    left: calc(-1 * var(--front-bleed-x));
    right: calc(-1 * var(--front-bleed-x));
    bottom: calc(-1 * var(--front-bleed-y));
    /* Размеры и сдвиги заданы в долях ОКНА (cqw/cqh от .launcher-bg), а не
       рамки слоя, поэтому запас выше не меняет кадр. 50% по горизонтали
       тоже остаётся центром окна: рамка расширена симметрично.

       Сдвиг вправо — в долях ширины окна. Проценты в background-position
       здесь бесполезны: при картинке шириной с окно они дают нулевое
       смещение, а фигуры в вырезке стоят левее, чем нужно. Рука Аликс при
       этом уходит за правый край — так и было на прежней склейке.

       Крупный план: картинка почти вдвое выше окна, привязана по ВЕРХУ, и
       низ уходит далеко за нижний край. Вырезка нарисована в полный рост,
       но целиком в окне фигуры становятся мелкими и висят посреди экрана,
       поэтому она увеличена и намеренно кадрируется низом окна. */
    background-size: var(--front-size, auto 195cqh);
    background-position: var(--front-pos, calc(50% + 20cqw) 3cqh);
    background-repeat: no-repeat;
    pointer-events: none;
    /* Точка поворота — у самих фигур (72% ширины и 85% высоты окна), иначе
       наклон читается как перекос всего экрана */
    transform-origin: calc(var(--front-bleed-x) + var(--front-origin-x, 72cqw)) var(--front-origin-y, 85cqh);
    transform:
      perspective(1200px)
      translate3d(var(--l-bg-front-x, 0px), var(--l-bg-front-y, 0px), 0)
      rotateX(var(--l-bg-front-rx, 0deg))
      rotateY(var(--l-bg-front-ry, 0deg))
      scale(var(--l-bg-front-zoom, 1));
    will-change: transform;
  }
}

.launcher-bg__tool {
  position: absolute;
  inset: 0;
  background:
    radial-gradient(ellipse 46% 52% at 70% 40%, rgba(255,255,255,0.05), transparent 70%),
    repeating-linear-gradient(45deg, rgba(255,255,255,0.022) 0 2px, transparent 2px 6px),
    linear-gradient(160deg, #0a0d12 0%, #0b1016 55%, #0d131a 100%);
}
.launcher-bg__orb {
  position: absolute; width: 176px; height: 176px; border-radius: 50%;
  background: radial-gradient(circle at 32% 25%, color-mix(in srgb, var(--orb) 72%, white), var(--orb) 65%);
  box-shadow: inset 0 0 0 1px rgba(255,255,255,0.14), inset -12px -16px 32px rgba(0,0,0,0.18),
    0 24px 56px color-mix(in srgb, var(--orb) 24%, transparent);
  animation: tool-orbit 6s ease-in-out infinite alternate;
  /* Слой фона прозрачен для мыши, а шарики ловят щелчки «повтори цвета».
     Курсор не меняется: игра тайная. */
  pointer-events: auto;
  transition: filter 0.12s ease-out, box-shadow 0.12s ease-out;
}
/* Подсветка — свечение цветом самого шарика. Без анимаций (правило в
   launcher.css) она включается и гаснет сразу, но так же видна. */
.launcher-bg__orb.is-lit {
  filter: brightness(1.5) saturate(1.15);
  box-shadow: inset 0 0 0 1px rgba(255,255,255,0.3), 0 0 40px 12px color-mix(in srgb, var(--orb) 80%, transparent),
    0 0 110px 36px color-mix(in srgb, var(--orb) 45%, transparent);
}
/* Ошибка в «повтори цвета»: шарики коротко гаснут. */
.launcher-bg__orb.is-miss {
  filter: brightness(0.35) saturate(0.4);
}
.launcher-bg__orb--red { --orb: #cf3247; left: calc(72% - 88px); top: calc(26% - 88px); }
.launcher-bg__orb--blue { --orb: #2f5fd8; left: calc(64% - 88px); top: calc(50% - 88px); animation-delay: -2s; }
.launcher-bg__orb--green { --orb: #2f9a4a; left: calc(81% - 88px); top: calc(50% - 88px); animation-delay: -4s; }
@keyframes tool-orbit {
  from { transform: translate3d(0, -6px, 0); }
  to { transform: translate3d(0, 8px, 0); }
}
.no-anim .launcher-bg__orb { animation: none !important; }
@media (prefers-reduced-motion: reduce) {
  .launcher-bg__orb { animation: none !important; }
}

/* Кроссфейд при смене игры. Оба слоя абсолютные и лежат друг на друге,
   поэтому промежуточного пустого состояния не возникает: старый кадр
   гаснет только по мере проявления нового.
   Длительность сводится к нулю глобальными правилами
   prefers-reduced-motion и .no-anim в launcher.css. */
.launcher-bg-fade-enter-active,
.launcher-bg-fade-leave-active {
  transition: opacity var(--l-dur-fade) var(--l-ease-fade);
}

.launcher-bg-fade-enter-from,
.launcher-bg-fade-leave-to {
  opacity: 0;
}
</style>
