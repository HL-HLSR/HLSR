<template>
  <!-- Обучение по вкладке «Персонализация»: подсвечивает элемент с data-tour и ведёт по шагам.
       Щит ловит нажатия, чтобы игрок не менял вкладку под подсказкой; выйти можно
       всегда: «Пропустить» или Esc. -->
  <div class="tour" :class="{ idle: !step }" ref="root" @mousedown.stop>
    <div v-if="step" class="tour__ring" :style="ringStyle"></div>
    <div
      v-if="step"
      ref="card"
      class="tour__card"
      :style="cardStyle"
      role="dialog"
      aria-modal="true"
      :aria-label="$t(step.title)"
    >
      <h2 class="tour__title">{{ $t(step.title) }}</h2>
      <p class="tour__text">{{ $t(step.text) }}</p>
      <div class="tour__actions">
        <button type="button" class="tour__btn ghost" @click="finish">{{ $t("#L_TOUR_SKIP") }}</button>
        <span class="tour__spacer"></span>
        <button type="button" class="tour__btn" :disabled="!canGoBack" @click="go(index - 1, -1)">{{ $t("#L_TOUR_BACK") }}</button>
        <button type="button" class="tour__btn primary" ref="next" @click="go(index + 1, 1)">
          {{ $t(index === steps.length - 1 ? "#L_TOUR_DONE" : "#L_TOUR_NEXT") }}
        </button>
      </div>
    </div>
  </div>
</template>

<script>
import { trapFocus } from "./focus-trap";

const PAD = 6; // поля подсветки вокруг элемента
const GAP = 12; // отступ карточки от подсветки
const EDGE = 12; // отступ карточки от края окна
const WAIT_MS = 400; // сколько ждать якорь, когда раздел не менялся
const WAIT_SWITCH_MS = 1200; // после смены раздела вкладки окна появляются не сразу

// steps — результат config-tour.stepsFor; section — текущий раздел шапки.
// Раздел шага другой — просим оболочку переключить; нет якоря на экране — шаг пропускаем.
export default {
  name: "ConfigTour",
  props: {
    steps: { type: Array, required: true },
    section: { type: String, default: "configs" },
  },
  emits: ["section", "page-section", "done"],
  data() {
    return { index: -1, ring: null, cardPos: { left: 0, top: 0 } };
  },
  computed: {
    step() {
      return this.index >= 0 ? this.steps[this.index] : null;
    },
    canGoBack() {
      return this.index > 0;
    },
    ringStyle() {
      if (!this.ring) return { opacity: 0 };
      return {
        left: this.ring.left + "px",
        top: this.ring.top + "px",
        width: this.ring.width + "px",
        height: this.ring.height + "px",
      };
    },
    cardStyle() {
      return { left: this.cardPos.left + "px", top: this.cardPos.top + "px" };
    },
  },
  methods: {
    anchorEl(step) {
      const el = document.querySelector(`[data-tour="${step.anchor}"]`);
      return el && (el.offsetWidth || el.offsetHeight) ? el : null;
    },
    // Ждёт якорь после смены раздела; null — не появился.
    async waitFor(step, token, switched) {
      const until = Date.now() + (switched ? WAIT_SWITCH_MS : WAIT_MS);
      for (;;) {
        await this.$nextTick();
        if (token !== this.token) return null;
        const el = this.anchorEl(step);
        if (el || Date.now() > until) return el;
        await new Promise((resolve) => setTimeout(resolve, 40));
      }
    },
    // Переход к шагу i; вперёд или назад пропускает шаги без якоря.
    async go(i, dir) {
      const token = ++this.token;
      for (; i >= 0 && i < this.steps.length; i += dir) {
        const step = this.steps[i];
        const switched = !!step.section && step.section !== this.section;
        if (switched) this.$emit("section", step.section);
        // Раздел списка слева (импорт и экспорт видны не во всех): страница открывает его до поиска якоря.
        if (step.pageSection) this.$emit("page-section", step.pageSection);
        const el = await this.waitFor(step, token, switched);
        if (token !== this.token) return;
        if (!el) continue;
        this.index = i;
        await this.$nextTick();
        this.place();
        if (this.$refs.next) this.$refs.next.focus();
        return;
      }
      // Вперёд шагов не осталось — конец. Назад — остаёмся на текущем.
      if (dir > 0) this.finish();
    },
    place() {
      const root = this.$refs.root;
      const card = this.$refs.card;
      if (!this.step || !root || !card) return;
      const el = this.anchorEl(this.step);
      if (!el) return;

      const base = root.getBoundingClientRect();
      const r = el.getBoundingClientRect();
      const ring = {
        left: r.left - base.left - PAD,
        top: r.top - base.top - PAD,
        width: r.width + PAD * 2,
        height: r.height + PAD * 2,
      };
      this.ring = ring;

      const cw = card.offsetWidth;
      const ch = card.offsetHeight;
      const below = ring.top + ring.height + GAP;
      const above = ring.top - GAP - ch;
      let top;
      if (below + ch <= base.height - EDGE) top = below;
      else if (above >= EDGE) top = above;
      else top = base.height - ch - EDGE; // подсветка почти на весь экран: карточка внутри, у нижнего края
      const left = Math.max(EDGE, Math.min(ring.left, base.width - cw - EDGE));
      this.cardPos = { left, top: Math.max(EDGE, top) };
    },
    // Якорь может сдвинуться и без resize: страница едет вслед за боковой панелью,
    // раскрывается раздел. Каждый кадр сверяем его место и переставляем подсветку.
    track() {
      const el = this.step && this.anchorEl(this.step);
      if (el) {
        const r = el.getBoundingClientRect();
        const key = `${r.left},${r.top},${r.width},${r.height}`;
        if (key !== this.lastRect) {
          this.lastRect = key;
          this.place();
        }
      }
      this.frame = requestAnimationFrame(this.track);
    },
    finish() {
      this.token += 1;
      this.$emit("done");
    },
    onKeydown(e) {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        this.finish();
      } else if (e.key === "Tab") {
        trapFocus(this.$refs.card, e);
      }
    },
  },
  mounted() {
    this.token = 0;
    this.previousFocus = document.activeElement;
    document.addEventListener("keydown", this.onKeydown, true);
    window.addEventListener("resize", this.place);
    window.addEventListener("scroll", this.place, true);
    this.frame = requestAnimationFrame(this.track);
    this.go(0, 1);
  },
  beforeUnmount() {
    this.token += 1;
    document.removeEventListener("keydown", this.onKeydown, true);
    window.removeEventListener("resize", this.place);
    window.removeEventListener("scroll", this.place, true);
    cancelAnimationFrame(this.frame);
    const back = this.previousFocus;
    if (back && back.focus && document.contains(back)) back.focus();
  },
};
</script>

<style lang="scss" scoped>
.tour {
  position: absolute;
  inset: 0;
  z-index: 60;

  // Пока нет ни одной карточки, щит не перехватывает нажатия.
  &.idle {
    pointer-events: none;
  }
}

.tour__ring {
  position: absolute;
  border-radius: var(--l-radius-sm);
  border: 2px solid var(--accent);
  // Тень вокруг подсветки затемняет всё остальное окно.
  box-shadow: 0 0 0 9999px rgba(0, 0, 0, 0.62);
  pointer-events: none;
  transition: left 0.25s ease, top 0.25s ease, width 0.25s ease, height 0.25s ease, opacity 0.2s ease;
}

.tour__card {
  position: absolute;
  width: 340px;
  max-width: calc(100% - 24px);
  padding: 16px 18px;
  display: flex;
  flex-direction: column;
  gap: 8px;
  background: var(--l-modal-bg);
  border: 1px solid var(--l-modal-border);
  border-radius: var(--l-radius);
  box-shadow: 0 18px 48px rgba(0, 0, 0, 0.55);
  color: var(--l-text);
  transition: left 0.25s ease, top 0.25s ease;
}

.tour__title {
  margin: 0;
  font-size: 15px;
  font-weight: 600;
}

.tour__text {
  margin: 0;
  font-size: 13px;
  line-height: 1.45;
  color: var(--l-text-dim);
}

.tour__actions {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 6px;
}

.tour__spacer {
  flex: 1;
}

.tour__btn {
  padding: 7px 14px;
  border: 1px solid var(--l-border-strong);
  border-radius: var(--l-radius-sm);
  background: rgba(255, 255, 255, 0.05);
  color: var(--l-text);
  font-family: inherit;
  font-size: 13px;
  font-weight: 500;
  cursor: pointer;
  transition: background var(--l-fast);

  &:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.1);
  }

  &:disabled {
    opacity: 0.4;
    cursor: default;
  }

  &.primary {
    background: rgba(255, 255, 255, 0.14);
  }

  &.ghost {
    background: transparent;
    border-color: transparent;
    color: var(--l-text-dim);
  }
}

// Без анимаций (настройка лаунчера или системная) подсветка и карточка перескакивают.
.no-anim .tour__ring,
.no-anim .tour__card,
.no-anim .tour__btn {
  transition: none;
}

@media (prefers-reduced-motion: reduce) {
  .tour__ring,
  .tour__card,
  .tour__btn {
    transition: none;
  }
}
</style>
