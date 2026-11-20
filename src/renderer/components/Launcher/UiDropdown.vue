<template>
  <div class="ui-dropdown" ref="root">
    <button
      type="button"
      ref="toggle"
      class="ui-dropdown__toggle"
      :class="{ open: open }"
      :id="toggleId"
      :aria-describedby="describedBy"
      aria-haspopup="listbox"
      :aria-expanded="open ? 'true' : 'false'"
      @click="toggle"
      @keydown.down.prevent="openAndFocus(0)"
      @keydown.up.prevent="openAndFocus(items.length - 1, -1)"
      @keydown="onToggleKeydown"
    >
      <slot name="toggle" :open="open"></slot>
    </button>

    <transition name="l-pop">
      <div
        v-if="open"
        v-dropdown-layer
        ref="menu"
        class="ui-dropdown__menu"
        :class="[menuDirection, align]"
        :style="{ visibility: positioned ? 'visible' : 'hidden' }"
        role="listbox"
      >
        <div v-if="heading" class="ui-dropdown__heading" aria-hidden="true">{{ heading }}</div>
        <button
          v-for="(item, i) in items"
          :key="item.id"
          type="button"
          role="option"
          ref="options"
          class="ui-dropdown__item"
          :class="{ selected: item.id === value, separated: item.separated }"
          :style="{ '--i': i }"
          :disabled="item.disabled"
          :aria-selected="item.id === value ? 'true' : 'false'"
          :aria-disabled="item.disabled ? 'true' : 'false'"
          :title="item.hint || item.label"
          @click="select(item)"
          @keydown="onItemKeydown($event, i)"
        >
          <slot name="item" :item="item">
            <span class="ui-dropdown__label">{{ item.label }}</span>
          </slot>
          <span v-if="item.badge" class="ui-dropdown__badge">{{ item.badge }}</span>
          <!-- Выбранный нескачанный эпизод: и размер, и галочка. -->
          <i v-if="item.id === value" class="fas fa-check ui-dropdown__check" aria-hidden="true"></i>
        </button>
      </div>
    </transition>
  </div>
</template>

<script>
import orderedRefs from "@/utils/orderedRefs";
import uiState from "./ui-state";
import { TYPEAHEAD_RESET_MS, findTypeaheadIndex, isTypeaheadKey } from "./dropdown-typeahead.mjs";

export default {
  name: "UiDropdown",
  directives: {
    dropdownLayer: {
      mounted(el, binding) {
        binding.instance.mountMenu(el);
      },
    },
  },
  emits: ["input", "select"],
  props: {
    items: {
      type: Array,
      required: true,
    },
    value: {
      type: [String, Number],
      default: null,
    },
    direction: {
      type: String,
      default: "down",
    },
    align: {
      type: String,
      default: "left",
    },
    // Для <label for> и подсказки поля, когда список стоит в форме вместо
    // системного <select>.
    toggleId: {
      type: String,
      default: null,
    },
    describedBy: {
      type: String,
      default: null,
    },
    // Подпись над пунктами, как заголовок раздела в профиле.
    heading: {
      type: String,
      default: null,
    },
  },
  data() {
    return {
      open: false,
      // Видимость реактивная: смена направления перерисовывает меню, и
      // статический style вернул бы visibility: hidden после позиционирования.
      positioned: false,
      menuDirection: this.direction,
    };
  },
  methods: {
    toggle() {
      this.open ? this.close() : this.show();
    },
    show() {
      if (this.open) return;
      this.menuDirection = this.direction;
      this.positioned = false;
      this.open = true;
      uiState.openDropdowns++;
      document.addEventListener("mousedown", this.onDocumentDown, true);
      document.addEventListener("keydown", this.onDocumentKey, true);
      document.addEventListener("scroll", this.onViewportChange, true);
      window.addEventListener("resize", this.onViewportChange);
      this.$nextTick(this.positionMenu);
    },
    mountMenu(menu) {
      // Отдельный слой сохраняет тему и zoom лаунчера, но не участвует
      // в scrollHeight настроек даже в первый кадр открытия и анимации.
      this.menuHost = this.$refs.root.closest(".launcher-root") || document.body;
      this.layerMenu = menu;
      this.menuHost.appendChild(menu);
    },
    positionMenu() {
      if (!this.open || !this.$refs.menu) return;
      const root = this.$refs.root;
      const launcher = root.closest(".launcher-root");
      const zoom = (launcher && parseFloat(getComputedStyle(launcher).zoom)) || 1;
      const anchor = this.$refs.toggle.getBoundingClientRect();
      const menu = this.$refs.menu;
      const host = this.menuHost.getBoundingClientRect();
      let top = 0;
      let bottom = window.innerHeight;
      let left = 8 * zoom;
      let right = window.innerWidth - 8 * zoom;
      // Меню должно помещаться не только в окно, но и в прокручиваемую
      // область настроек: её нижнюю часть закрывает панель кнопок.
      for (let parent = root.parentElement; parent; parent = parent.parentElement) {
        if (!/^(auto|scroll|hidden|clip)$/.test(getComputedStyle(parent).overflowY)) continue;
        const rect = parent.getBoundingClientRect();
        top = Math.max(top, rect.top);
        bottom = Math.min(bottom, rect.bottom);
        left = Math.max(left, rect.left + 8 * zoom);
        right = Math.min(right, rect.right - 8 * zoom);
      }
      let up = Math.max(0, (anchor.top - top) / zoom - 8);
      let down = Math.max(0, (bottom - anchor.bottom) / zoom - 8);
      menu.style.maxHeight = "320px";
      const maxWidth = Math.max(0, Math.min(360, (right - left) / zoom));
      menu.style.minWidth = Math.min(anchor.width / zoom, maxWidth) + "px";
      menu.style.maxWidth = maxWidth + "px";
      menu.style.width = "max-content";
      // Ширина по содержимому дробная, а offsetWidth округляет её до целого
      // и вниз тоже: меню выходило на долю пикселя уже текста, и подпись
      // обрезалась многоточием («Episode T…» при ширине 224.15 px).
      // getComputedStyle даёт точное значение в px CSS, без учёта zoom
      // и анимации появления.
      const natural = parseFloat(getComputedStyle(menu).width) || menu.offsetWidth;
      menu.style.width = Math.min(maxWidth, Math.ceil(natural / 4) * 4) + "px";
      const wanted = Math.min(320, menu.scrollHeight);
      // Низкое окно (Full HD): в прокручиваемой области места нет ни вверх, ни вниз —
      // меню выходит за неё (поверх панели кнопок), в пределах окна, и не листается.
      if (up < wanted && down < wanted) {
        up = Math.max(0, anchor.top / zoom - 8);
        down = Math.max(0, (window.innerHeight - anchor.bottom) / zoom - 8);
      }
      this.menuDirection = this.direction;
      if (this.direction === "down" && down < wanted && up > down) this.menuDirection = "up";
      else if (this.direction === "up" && up < wanted && down > up) this.menuDirection = "down";
      const space = this.menuDirection === "up" ? up : down;
      menu.style.maxHeight = Math.floor(Math.min(320, space) / 4) * 4 + "px";
      // Полоса прокрутки занимает место подписей: ширина больше на неё, без многоточия.
      if (menu.scrollHeight > menu.clientHeight) menu.style.width = Math.min(maxWidth, parseFloat(menu.style.width) + menu.offsetWidth - menu.clientWidth) + "px";
      const size = menu.getBoundingClientRect();
      const x = this.align === "right" ? anchor.right - size.width : anchor.left;
      const y = this.menuDirection === "up" ? anchor.top - size.height - 8 * zoom : anchor.bottom + 8 * zoom;
      menu.style.left = (Math.max(left, Math.min(x, right - size.width)) - host.left) / zoom + "px";
      menu.style.top = (y - host.top) / zoom + "px";
      this.positioned = true;
    },
    onViewportChange(event) {
      if (this.$refs.menu && this.$refs.menu.contains(event.target)) return;
      this.close();
    },
    close(focusToggle) {
      if (!this.open) return;
      this.open = false;
      uiState.openDropdowns = Math.max(0, uiState.openDropdowns - 1);
      document.removeEventListener("mousedown", this.onDocumentDown, true);
      document.removeEventListener("keydown", this.onDocumentKey, true);
      document.removeEventListener("scroll", this.onViewportChange, true);
      window.removeEventListener("resize", this.onViewportChange);
      if (focusToggle) this.$refs.toggle.focus({ preventScroll: true });
    },
    openAndFocus(index, step) {
      this.show();
      this.$nextTick(() => this.$nextTick(() => this.focusOption(index, step)));
    },
    // Ближайший доступный пункт от index в сторону step (по кругу).
    enabledIndex(index, step) {
      const count = this.items.length;
      for (let n = 0; n < count; n++) {
        const i = (((index + n * step) % count) + count) % count;
        if (!this.items[i].disabled) return i;
      }
      return -1;
    },
    focusOption(index, step) {
      index = this.enabledIndex(index, step || 1);
      if (index === -1) return;
      let options = orderedRefs(this.$refs.options);
      if (options && options[index]) {
        options[index].focus({ preventScroll: true });
        // Прокручиваем только меню, не родительскую область настроек.
        const menu = this.$refs.menu;
        const option = options[index];
        if (option.offsetTop < menu.scrollTop) menu.scrollTop = option.offsetTop;
        else if (option.offsetTop + option.offsetHeight > menu.scrollTop + menu.clientHeight)
          menu.scrollTop = option.offsetTop + option.offsetHeight - menu.clientHeight;
      }
    },
    select(item) {
      if (item.disabled) return;
      this.$emit("input", item.id);
      this.$emit("select", item);
      this.close(true);
    },
    onDocumentDown(e) {
      if (!this.$refs.root.contains(e.target) && !(this.$refs.menu && this.$refs.menu.contains(e.target))) this.close();
    },
    onDocumentKey(e) {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        this.close(true);
      }
    },
    onToggleKeydown(e) {
      if (!isTypeaheadKey(e)) return;
      e.preventDefault();
      const current = this.items.findIndex((item) => item.id === this.value);
      const index = this.typeahead(e.key, current);
      if (index === -1) return;
      if (this.open) this.focusOption(index);
      else this.openAndFocus(index);
    },
    typeahead(key, currentIndex) {
      clearTimeout(this.typeaheadTimer);
      this.typeaheadQuery = (this.typeaheadQuery || "") + key;
      this.typeaheadTimer = setTimeout(() => {
        this.typeaheadQuery = "";
      }, TYPEAHEAD_RESET_MS);
      return findTypeaheadIndex(this.items, this.typeaheadQuery, currentIndex);
    },
    onItemKeydown(e, index) {
      let last = this.items.length - 1;

      if (isTypeaheadKey(e)) {
        e.preventDefault();
        const found = this.typeahead(e.key, index);
        if (found !== -1) this.focusOption(found);
        return;
      }

      if (e.key === "ArrowDown") {
        e.preventDefault();
        this.focusOption(index >= last ? 0 : index + 1, 1);
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        this.focusOption(index <= 0 ? last : index - 1, -1);
      } else if (e.key === "Home") {
        e.preventDefault();
        this.focusOption(0, 1);
      } else if (e.key === "End") {
        e.preventDefault();
        this.focusOption(last, -1);
      } else if (e.key === "Tab") {
        this.close(true);
      }
    },
  },
  beforeUnmount() {
    clearTimeout(this.typeaheadTimer);
    this.close();
    if (this.layerMenu) this.layerMenu.remove();
  },
};
</script>

<style lang="scss" scoped>
.ui-dropdown {
  position: relative;
  -webkit-app-region: no-drag;

  &__toggle {
    display: flex;
    align-items: center;
    border: none;
    background: none;
    padding: 0;
    color: inherit;
    font: inherit;
    cursor: pointer;
  }

  /* Размеры и положение рассчитываются относительно доступной области;
     рамка тенью не меняет размеры меню. Поверхность — как у окна профиля
     и меню игры. */
  &__menu {
    position: absolute;
    width: max-content;
    max-width: min(360px, calc(100vw - 48px));
    max-height: min(320px, calc(100vh - var(--l-topbar-h) - 96px));
    overflow-y: auto;
    overscroll-behavior: contain;
    padding: 8px;
    background: var(--l-modal-bg);
    border-radius: var(--l-radius);
    box-shadow: inset 0 0 0 1px var(--l-modal-border), var(--l-shadow-pop);
    display: flex;
    flex-direction: column;
    gap: 4px;
    z-index: 1000;
    -webkit-app-region: no-drag;

    &::-webkit-scrollbar { width: 8px; }
    &::-webkit-scrollbar-thumb { border: 2px solid transparent; border-radius: 4px; background: rgba(255, 255, 255, 0.14) padding-box; }
  }

  /* Меню над кнопкой растёт снизу вверх, под кнопкой — сверху вниз. */
  &__menu.down { transform-origin: 50% 0; }
  &__menu.up { transform-origin: 50% 100%; }

  /* Пункты появляются друг за другом; у длинных списков задержка не
     копится дальше восьмого пункта. */
  &__item {
    flex: none;
    display: flex;
    align-items: center;
    gap: 12px;
    border: none;
    background: none;
    box-shadow: inset 0 0 0 1px transparent;
    text-align: left;
    padding: 8px 8px 8px 12px;
    border-radius: var(--l-radius-sm);
    color: var(--l-text-dim);
    font-family: inherit;
    font-size: 13px;
    font-weight: 500;
    line-height: 24px;
    cursor: pointer;
    transition: background-color var(--l-fast), box-shadow var(--l-fast), color var(--l-fast);
    animation: l-rise-in var(--l-dur) var(--l-ease) both;
    animation-delay: calc(min(var(--i, 0), 8) * 16ms);

    &:hover:not(:disabled),
    &:focus-visible {
      background: rgba(255, 255, 255, 0.05);
      box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--l-accent) 45%, var(--l-border));
      color: var(--l-text);
      outline: none;
    }

    &:active:not(:disabled) {
      background: rgba(255, 255, 255, 0.08);
    }

    &.selected {
      background: var(--l-accent-soft);
      color: var(--l-text);
    }

    &:disabled {
      cursor: default;
      color: var(--l-text-faint);
    }

    /* Линия над пунктом — псевдоэлементом в отступе, высота пункта та же. */
    &.separated {
      position: relative;
      margin-top: 8px;

      &::before {
        content: "";
        position: absolute;
        left: 4px;
        right: 4px;
        top: -7px;
        height: 1px;
        background: var(--l-modal-border);
        pointer-events: none;
      }
    }
  }

  &__heading {
    flex: none;
    padding: 4px 12px 4px;
    color: var(--l-text-faint);
    font-size: 11px;
    font-weight: 700;
    line-height: 16px;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    animation: l-rise-in var(--l-dur) var(--l-ease) both;
  }

  /* Пометка «скоро» — как у будущего выпуска в списке изменений. */
  &__badge {
    flex: none;
    padding: 0 8px;
    border-radius: 6px;
    background: rgba(255, 179, 122, 0.12);
    box-shadow: inset 0 0 0 1px rgba(255, 179, 122, 0.35);
    color: #ffb37a;
    font-size: 10.5px;
    font-weight: 700;
    line-height: 20px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  &__label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Высота значка — строка пункта: с line-height 1 глиф 11 px стоял на
     полпикселя ниже центра */
  &__check {
    flex: none;
    width: 24px;
    text-align: center;
    font-size: 11px;
    line-height: 24px;
    color: var(--accent);
  }
}
</style>
