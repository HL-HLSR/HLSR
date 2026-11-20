<template>
  <!-- Колонка значков. Свёрнутая — узкая: названия игр, «скоро» и онлайн
       показывает подсказка справа от значка. Выдвинутая (кнопка панели
       внизу колонки) — подписи стоят прямо в колонке, подсказки не нужны.
       Значки при этом не двигаются: колонка растёт вправо. -->
  <aside class="game-rail" :class="{ 'is-open': expanded }" ref="rail">

    <nav class="game-rail__list" ref="list" @scroll="hideTip">
      <template v-for="game in games" :key="game.id">
        <button
          ref="cards"
          type="button"
          class="rail-item"
          :class="{
            active: activeTab === 'games' && game.id === selectedId,
            unavailable: !game.available,
          }"
          :aria-label="labelFor(game)"
          :aria-current="activeTab === 'games' && game.id === selectedId ? 'page' : null"
          :aria-disabled="game.available ? null : 'true'"
          @click="selectItem('select', game.id)"
          @mouseenter="showTip($event, tipFor(game))"
          @mouseleave="hideTip"
          @focus="showFocusTip($event, tipFor(game))"
          @blur="hideTip"
        >
          <span class="rail-item__icon">
            <span
              v-if="game.logo"
              class="rail-item__badge"
              :style="{ '--rail-logo': 'url(' + game.logo + ')' }"
            ></span>
            <HL v-else class="rail-item__hl" iconColor="currentColor" />
          </span>
          <span class="rail-item__text" aria-hidden="true">
            <span class="rail-item__title">{{ game.shortName }}</span>
            <span v-if="tips[game.id].note" class="rail-item__note" :class="'is-' + tips[game.id].tone">
              <span v-if="tips[game.id].tone === 'online'" class="rail-item__dot"></span>
              {{ tips[game.id].note }}
            </span>
          </span>
        </button>
      </template>

      <span class="game-rail__rule" role="separator"></span>

      <template v-if="toolItems.length">
        <button
          v-for="tool in toolItems"
          :key="tool.toolId"
          type="button"
          class="rail-item"
          :class="{ active: activeTab === 'tools' && tool.toolId === selectedToolId }"
          :aria-label="tool.shortName"
          :aria-current="activeTab === 'tools' && tool.toolId === selectedToolId ? 'page' : null"
          @click="selectItem('select-tool', tool.toolId)"
          @mouseenter="showTip($event, { title: tool.shortName })"
          @mouseleave="hideTip"
          @focus="showFocusTip($event, { title: tool.shortName })"
          @blur="hideTip"
        >
          <span class="rail-item__icon">
            <span
              v-if="toolLogo(tool)"
              class="rail-item__badge"
              :style="{ '--rail-logo': 'url(' + toolLogo(tool) + ')' }"
            ></span>
            <i v-else class="rail-item__glyph" :class="tool.icon" aria-hidden="true"></i>
          </span>
          <span class="rail-item__text" aria-hidden="true">
            <span class="rail-item__title">{{ tool.shortName }}</span>
          </span>
        </button>
      </template>

      <!-- «Демо» — свой пункт, не из tool-list: там только скачиваемые
           инструменты, а у страницы демо нет манифеста. -->
      <button
        type="button"
        class="rail-item"
        :class="{ active: activeTab === 'demo' }"
        :aria-label="$t('#L_DEMO_TITLE')"
        :aria-current="activeTab === 'demo' ? 'page' : null"
        @click="selectItem('select-demo')"
        @mouseenter="showTip($event, { title: $t('#L_DEMO_TITLE') })"
        @mouseleave="hideTip"
        @focus="showFocusTip($event, { title: $t('#L_DEMO_TITLE') })"
        @blur="hideTip"
      >
        <span class="rail-item__icon">
          <i class="rail-item__glyph rail-item__glyph--demo fas fa-film" aria-hidden="true"></i>
        </span>
        <span class="rail-item__text" aria-hidden="true">
          <span class="rail-item__title">{{ $t("#L_DEMO_TITLE") }}</span>
        </span>
      </button>
    </nav>

    <div class="game-rail__footer">
      <!-- Переключатель колонки — такая же кнопка, как соседние: значок
           панели, стрелка в нём показывает, куда колонка поедет. -->
      <button
        type="button"
        class="game-rail__tool game-rail__collapse"
        :aria-expanded="expanded ? 'true' : 'false'"
        :aria-label="$t(railToggleTitle)"
        @click="toggle"
        @mouseenter="showTip($event, { title: $t(railToggleTitle) })"
        @mouseleave="hideTip"
        @focus="showFocusTip($event, { title: $t(railToggleTitle) })"
        @blur="hideTip"
      >
        <svg class="game-rail__tool-icon game-rail__collapse-icon" viewBox="0 0 24 24" aria-hidden="true">
          <rect x="3" y="4" width="18" height="16" rx="3" fill="none" stroke="currentColor" stroke-width="2" />
          <path d="M9 4v16" stroke="currentColor" stroke-width="2" />
          <path class="game-rail__collapse-chev" d="M13.5 9.5 16 12l-2.5 2.5" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" />
        </svg>
        <span class="game-rail__tool-label" aria-hidden="true">{{ $t(railToggleTitle) }}</span>
      </button>
      <span class="game-rail__rule game-rail__rule--footer" role="separator"></span>
      <button
        v-for="tool in tools"
        :key="tool.id"
        type="button"
        class="game-rail__tool"
        :aria-label="$t(tool.title)"
        @click="selectItem('tool', tool.id)"
        @mouseenter="showTip($event, { title: $t(tool.title) })"
        @mouseleave="hideTip"
        @focus="showFocusTip($event, { title: $t(tool.title) })"
        @blur="hideTip"
      >
        <i v-if="tool.icon" class="game-rail__tool-fa" :class="tool.icon" aria-hidden="true"></i>
        <svg v-else class="game-rail__tool-icon" :viewBox="tool.box" aria-hidden="true">
          <path :d="tool.path" fill="currentColor" />
        </svg>
        <span class="game-rail__tool-label" aria-hidden="true">{{ $t(tool.title) }}</span>
      </button>
    </div>

    <!-- Подсказка живёт вне прокручиваемого списка: внутри него её обрезал
         бы overflow. Позиция считается в координатах колонки через
         offsetTop, а не getBoundingClientRect: под CSS zoom тот отдаёт
         экранные пиксели, а top задаётся в пикселях раскладки. -->
    <transition name="rail-tip">
      <div v-if="tip" v-grid-width class="rail-tip" :style="{ top: tip.top + 'px' }" aria-hidden="true">
        <span class="rail-tip__title">{{ tip.title }}</span>
        <span v-if="tip.note" class="rail-tip__note" :class="'is-' + tip.tone">
          <span v-if="tip.tone === 'online'" class="rail-tip__dot"></span>
          {{ tip.note }}
        </span>
      </div>
    </transition>
  </aside>
</template>

<script>
import orderedRefs from "@/utils/orderedRefs";
import HL from "@/components/Icons/HL";
import gridWidth from "./grid-width";
import { toolArt } from "./launcher-data";

export default {
  name: "GameSidebar",
  components: { HL },
  directives: { gridWidth },
  emits: ["select", "select-tool", "select-demo", "tool", "toggle"],
  props: {
    games: {
      type: Array,
      required: true,
    },
    selectedId: {
      type: String,
      required: true,
    },
    online: {
      type: Object,
      default: () => ({}),
    },
    activeTab: {
      type: String,
      default: "games",
    },
    toolItems: {
      type: Array,
      default: () => [],
    },
    selectedToolId: {
      type: String,
      default: "",
    },
    tipsEnabled: { type: Boolean, default: true },
    expanded: { type: Boolean, default: false },
  },
  data() {
    return {
      tip: null,
      tools: [
        {
          id: "discord",
          title: "#L_DISCORD",
          box: "-2 -2 28 28",
          path: "M20.317 4.3698a19.7913 19.7913 0 00-4.8851-1.5152.0741.0741 0 00-.0785.0371c-.211.3753-.4447.8648-.6083 1.2495-1.8447-.2762-3.68-.2762-5.4868 0-.1636-.3933-.4058-.8742-.6177-1.2495a.077.077 0 00-.0785-.037 19.7363 19.7363 0 00-4.8852 1.515.0699.0699 0 00-.0321.0277C.5334 9.0458-.319 13.5799.0992 18.0578a.0824.0824 0 00.0312.0561c2.0528 1.5076 4.0413 2.4228 5.9929 3.0294a.0777.0777 0 00.0842-.0276c.4616-.6304.8731-1.2952 1.226-1.9942a.076.076 0 00-.0416-.1057c-.6528-.2476-1.2743-.5495-1.8722-.8923a.077.077 0 01-.0076-.1277c.1258-.0943.2517-.1923.3718-.2914a.0743.0743 0 01.0776-.0105c3.9278 1.7933 8.18 1.7933 12.0614 0a.0739.0739 0 01.0785.0095c.1202.099.246.1981.3728.2924a.077.077 0 01-.0066.1276 12.2986 12.2986 0 01-1.873.8914.0766.0766 0 00-.0407.1067c.3604.698.7719 1.3628 1.225 1.9932a.076.076 0 00.0842.0286c1.961-.6067 3.9495-1.5219 6.0023-3.0294a.077.077 0 00.0313-.0552c.5004-5.177-.8382-9.6739-3.5485-13.6604a.061.061 0 00-.0312-.0286zM8.02 15.3312c-1.1825 0-2.1569-1.0857-2.1569-2.419 0-1.3332.9555-2.4189 2.157-2.4189 1.2108 0 2.1757 1.0952 2.1568 2.419 0 1.3332-.9555 2.4189-2.1569 2.4189zm7.9748 0c-1.1825 0-2.1569-1.0857-2.1569-2.419 0-1.3332.9554-2.4189 2.1569-2.4189 1.2108 0 2.1757 1.0952 2.1568 2.419 0 1.3332-.946 2.4189-2.1568 2.4189z",
        },
        {
          id: "folder",
          title: "#L_GAME_FOLDER",
          box: "0 0 24 24",
          path: "M10 4H4c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V8c0-1.1-.9-2-2-2h-8l-2-2z",
        },
        {
          id: "changelog",
          title: "#L_CHANGELOG",
          box: "-1 -1 26 26",
          path: "M14 2H6c-1.1 0-2 .9-2 2v16c0 1.1.9 2 2 2h12c1.1 0 2-.9 2-2V8l-6-6zm2 16H8v-2h8v2zm0-4H8v-2h8v2zm-3-5V3.5L18.5 9H13z",
        },
        {
          id: "settings",
          title: "#L_LAUNCHER_SETTINGS",
          icon: "fas fa-cog",
        },
      ],
    };
  },
  computed: {
    railToggleTitle() {
      return this.expanded ? "#L_RAIL_COLLAPSE" : "#L_RAIL_EXPAND";
    },
    // Подписи выдвинутой колонки — те же, что в подсказке.
    tips() {
      const map = {};
      for (const game of this.games) map[game.id] = this.tipFor(game);
      return map;
    },
  },
  watch: {
    selectedId() { this.hideTip(); this.revealActiveCard(); },
    activeTab: "hideTip",
    selectedToolId: "hideTip",
    tipsEnabled(enabled) { if (!enabled) this.hideTip(); },
    expanded: "hideTip",
  },
  methods: {
    onlineFor(gameId) {
      const entry = this.online && this.online[gameId];
      if (!entry) return null;

      if (entry.unknown || typeof entry.online !== "number")
        return { tone: "unknown", note: this.$t("#L_ONLINE_UNKNOWN") };

      return { tone: "online", note: this.$t("#L_RAIL_ONLINE", entry.online) };
    },
    tipFor(game) {
      if (!game.available)
        return { title: game.shortName, note: this.$t("#L_GAME_COMING_SOON"), tone: "soon" };

      return Object.assign({ title: game.shortName }, this.onlineFor(game.id));
    },
    toolLogo(tool) {
      const art = toolArt[tool.toolId];
      return art ? art.logo : "";
    },
    labelFor(game) {
      const tip = this.tipFor(game);
      return tip.note ? tip.title + " — " + tip.note : tip.title;
    },
    toggle() {
      this.hideTip();
      this.$emit("toggle");
    },
    showTip(event, tip) {
      // В выдвинутой колонке подписи и так видны.
      if (!this.tipsEnabled || this.expanded) return;
      const target = event.currentTarget;
      const rail = this.$refs.rail;
      if (!target || !rail) return;

      // offsetTop кнопки списка считается от колонки (она ближайший
      // позиционированный предок), а прокрутку списка надо вычесть отдельно
      let top = target.offsetTop + target.offsetHeight / 2;
      if (this.$refs.list && this.$refs.list.contains(target)) top -= this.$refs.list.scrollTop;

      this.tip = Object.assign({ note: "", tone: "" }, tip, { top });
    },
    showFocusTip(event, tip) {
      if (event.currentTarget.matches(":focus-visible")) this.showTip(event, tip);
    },
    selectItem(eventName, id) {
      this.hideTip();
      this.$emit(eventName, id);
    },
    hideTip() {
      this.tip = null;
    },
    revealActiveCard() {
      this.$nextTick(() => {
        const index = this.games.findIndex((g) => g.id === this.selectedId);
        const card = orderedRefs(this.$refs.cards)[index];
        if (card) card.scrollIntoView({ block: "nearest" });
      });
    },
  },
  mounted() {
    this.revealActiveCard();
  },
};
</script>

<style lang="scss" scoped>
@use "./round-dot" as *;

/* Колонка начинается под шапкой: логотип и вкладки лежат в верхней полосе
   LauncherShell поверх неё. Заливка чуть светлее затемнённого кадра справа,
   граница едва заметна — колонка читается, но не режет экран линией. */
.game-rail {
  position: relative;
  width: var(--l-rail-w);
  min-width: var(--l-rail-w);
  height: 100%;
  padding-top: var(--l-topbar-h);
  display: flex;
  flex-direction: column;
  align-items: stretch;
  /* Матовое стекло над кадром игры: под ним тот же кадр с тем же
     затемнением, только размытый. Заливка нейтральная и без подкраски
     насыщенности — иначе на краю колонка уходила в синеву и не совпадала
     со сценой справа. */
  background-color: var(--l-rail-fill);
  -webkit-backdrop-filter: blur(var(--l-rail-blur));
  backdrop-filter: blur(var(--l-rail-blur));
  z-index: 2;
  /* Колонка выдвигается тем же свайпом, что и смена вкладок. */
  transition: width var(--l-dur-swipe) var(--l-ease-swipe), min-width var(--l-dur-swipe) var(--l-ease-swipe),
    background-color var(--l-dur-swipe) var(--l-ease-swipe), box-shadow var(--l-dur-swipe) var(--l-ease-swipe);

  /* Выдвинутая колонка лежит над сценой: стекло прозрачнее, а мягкая тень
     справа отделяет её от кадра без линии на краю. */
  &.is-open {
    width: var(--l-rail-open-w);
    min-width: var(--l-rail-open-w);
    background-color: var(--l-rail-open-fill);
    box-shadow: 24px 0 48px -32px rgba(0, 0, 0, 0.7);
  }

  /* Стрелка в значке панели разворачивается, когда колонка выдвинута:
     показывает, куда колонка поедет по щелчку. */
  &__collapse-chev {
    transform-box: fill-box;
    transform-origin: center;
    transition: transform var(--l-dur-swipe) var(--l-ease-swipe);
  }

  &.is-open &__collapse-chev {
    transform: rotate(180deg);
  }

  /* Линия между переключателем и остальными кнопками: как линия перед
     инструментами в списке, места в колонке не занимает. */
  &__rule--footer {
    margin: 0 0 -1px;
  }

  /* Значки стоят на месте при любой ширине: поля списка — как в узкой
     колонке, кнопки растягиваются вправо вместе с ней. */
  &__list {
    flex: 1 1 auto;
    min-height: 0;
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: var(--l-rail-gap);
    padding: var(--l-rail-gap) calc((var(--l-rail-w) - var(--l-rail-item)) / 2);
    overflow-y: auto;
    overflow-x: hidden;
    /* Полоса прокрутки в колонке шириной со значок съела бы его край */
    &::-webkit-scrollbar {
      display: none;
    }
  }

  /* Линия не занимает места в колонке (высота 1 px съедена отступом):
     иначе LiveSplit под ней стоял бы на пиксель ниже сетки */
  &__rule {
    flex: none;
    display: block;
    align-self: center;
    width: 40px;
    height: 1px;
    margin-bottom: -1px;
    background: var(--l-border-strong);
    opacity: 0.6;
    transition: width var(--l-dur-swipe) var(--l-ease-swipe);
  }

  &.is-open &__rule {
    width: 100%;
  }

  &__footer {
    flex: none;
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: var(--l-rail-tool-gap);
    padding: var(--l-rail-gap) calc((var(--l-rail-w) - var(--l-rail-tool)) / 2) var(--l-rail-bottom);
  }

  /* Значок — в квадрате слева, подпись за ним; в узкой колонке кнопка
     ровно в квадрат, и подпись обрезана. */
  &__tool {
    flex: none;
    width: 100%;
    height: var(--l-rail-tool);
    padding: 0 0 0 calc((var(--l-rail-tool) - 24px) / 2);
    border: none;
    border-radius: var(--l-radius-sm);
    background: transparent;
    color: var(--l-rail-icon);
    display: flex;
    align-items: center;
    justify-content: flex-start;
    gap: calc((var(--l-rail-tool) - 24px) / 2 + 4px);
    overflow: hidden;
    font-family: inherit;
    cursor: pointer;
    transition: background var(--l-fast), color var(--l-fast);

    &:hover {
      background: rgba(255, 255, 255, 0.06);
      color: var(--l-text);
    }

    &:active {
      background: rgba(255, 255, 255, 0.1);
    }

    &:focus-visible {
      outline: 2px solid var(--l-accent);
      outline-offset: 0;
    }
  }

  &__tool-icon {
    flex: none;
    display: block;
    width: 24px;
    height: 24px;
  }

  &__tool-fa {
    flex: none;
    width: 24px;
    font-size: 22px;
    line-height: 24px;
    text-align: center;
  }

  &__tool-label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
    font-weight: 500;
    line-height: 20px;
  }
}

/* Подписи проявляются, когда колонка уже почти раскрыта, и гаснут сразу
   при сворачивании: обрезанный на лету текст не мелькает. */
.game-rail__tool-label,
.rail-item__text {
  opacity: 0;
  transform: translateX(-8px);
  transition: opacity var(--l-dur-fast) var(--l-ease-in), transform var(--l-dur-fast) var(--l-ease-in);
}

.game-rail.is-open .game-rail__tool-label,
.game-rail.is-open .rail-item__text {
  opacity: 1;
  transform: none;
  transition: opacity var(--l-dur) var(--l-ease) 0.14s, transform var(--l-dur) var(--l-ease) 0.14s;
}

.no-anim .game-rail,
.no-anim .game-rail__collapse-chev,
.no-anim .game-rail__rule,
.no-anim .game-rail__tool-label,
.no-anim .rail-item__text {
  transition: none !important;
}

/* Значок игры. Квадрат кнопки один и тот же у выбранной и обычной игры,
   меняется только то, что в нём нарисовано: при смене игры колонка не
   прыгает. */
.rail-item {
  flex: none;
  position: relative;
  width: 100%;
  height: var(--l-rail-item);
  padding: 0;
  border: none;
  border-radius: var(--l-radius);
  box-shadow: inset 0 0 0 1px transparent;
  background: transparent;
  display: flex;
  align-items: center;
  justify-content: flex-start;
  overflow: hidden;
  color: var(--l-rail-game);
  font-family: inherit;
  text-align: left;
  cursor: pointer;
  transition: color var(--l-fast), background-color var(--l-fast), box-shadow var(--l-fast);

  /* Квадрат значка того же размера, что кнопка в узкой колонке:
     кружок в нём по центру, как и был. */
  &__icon {
    flex: none;
    width: var(--l-rail-item);
    height: var(--l-rail-item);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  &__text {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding-right: 12px;
  }

  &__title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--l-text);
    font-size: 13px;
    font-weight: 600;
    line-height: 18px;
  }

  &__note {
    display: flex;
    align-items: center;
    gap: 6px;
    /* overflow нужен для многоточия, но обрезал бы свечение точки. Запас
       под свечение внутри, отрицательный margin возвращает текст на место. */
    padding: 4px 0 4px 6px;
    margin: -4px 0 -4px -6px;
    overflow: hidden;
    white-space: nowrap;
    color: var(--l-text-faint);
    font-size: 11.5px;
    line-height: 16px;

    &.is-online { color: #5fd17f; }
  }

  &__dot {
    flex: none;
    width: 6px;
    height: 6px;
    @include round-dot;
    @include round-dot-glow(3px);
  }

  /* Логотип игры рисуется маской: так он перекрашивается в цвет состояния
     (серый, акцентный) без второй копии картинки. Размер в px, а не в
     процентах от кружка: 58 % давали 27.84 px. */
  &__badge {
    width: var(--l-rail-logo);
    height: var(--l-rail-logo);
    background-color: currentColor;
    -webkit-mask: var(--rail-logo) center / contain no-repeat;
    mask: var(--rail-logo) center / contain no-repeat;
  }

  &__hl {
    width: var(--l-rail-logo);
    height: var(--l-rail-logo);
  }

  &__glyph {
    width: 24px;
    font-size: 20px;
    line-height: 24px;
    text-align: center;
  }

  /* Плёнка рядом с логотипами (32 px) выглядела мелкой. */
  &__glyph--demo {
    width: var(--l-rail-logo);
    font-size: 26px;
    line-height: var(--l-rail-logo);
  }

  &:hover {
    color: var(--l-text);
  }

  &:focus-visible {
    outline: 2px solid var(--l-accent);
    outline-offset: 0;
  }

  &.active {
    color: var(--l-accent);
    cursor: default;
  }

  &.unavailable {
    cursor: not-allowed;

    .rail-item__icon,
    .rail-item__title {
      opacity: 0.5;
    }

    &:hover {
      color: var(--l-rail-game);
    }
  }
}

/* В выдвинутой колонке строка — карточкой, как пункты окна профиля:
   наведение подсвечивает фон, выбранная игра — в цвете акцента. */
.game-rail.is-open .rail-item {
  &:hover:not(.active) {
    background: rgba(255, 255, 255, 0.05);
    box-shadow: inset 0 0 0 1px var(--l-border);
  }

  &.active {
    background: linear-gradient(90deg, rgba(var(--accent-rgb), 0.18), rgba(var(--accent-rgb), 0.06));
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--l-accent) 40%, transparent);
  }

  &.unavailable:hover {
    background: rgba(255, 255, 255, 0.02);
  }
}

/* Высота подсказки — 40 px с одной строкой и 64 px с двумя, обе кратны 8:
   сдвиг на половину высоты (центр против значка) остаётся целым числом
   шагов сетки. Рамка — тенью: border добавил бы по пикселю к высоте.
   Ширину доводит до сетки v-grid-width. */
.rail-tip {
  position: absolute;
  left: calc(100% + 12px);
  transform: translateY(-50%);
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 10px 16px;
  border-radius: var(--l-radius-sm);
  background: var(--l-surface-solid);
  box-shadow: inset 0 0 0 1px var(--l-border), 0 10px 28px rgba(0, 0, 0, 0.45);
  white-space: nowrap;
  pointer-events: none;
  z-index: 30;

  /* Хвостик в сторону значка */
  &::before {
    content: "";
    position: absolute;
    left: -6px;
    top: 50%;
    width: 10px;
    height: 10px;
    background: inherit;
    border-left: 1px solid var(--l-border);
    border-bottom: 1px solid var(--l-border);
    transform: translateY(-50%) rotate(45deg);
  }

  &__title {
    color: var(--l-text);
    font-size: 13px;
    font-weight: 600;
    line-height: 20px;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  &__note {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--l-text-dim);
    font-size: 12px;
    line-height: 20px;

    &.is-online {
      color: #5fd17f;
    }
  }

  &__dot {
    flex: none;
    width: 8px;
    height: 8px;
    @include round-dot;
  }
}

.rail-tip-enter-active,
.rail-tip-leave-active {
  transition: opacity var(--l-fast), transform var(--l-fast);
}

.rail-tip-enter-from,
.rail-tip-leave-to {
  opacity: 0;
  transform: translate(-4px, -50%);
}
</style>
