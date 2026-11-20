<template>
  <div class="tool-panel" :class="{ 'tool-panel--hero': heroOnly, 'tool-panel--controls': controlsOnly }">
    <div v-if="!controlsOnly" class="tool-panel__hero">
      <h1 class="tool-panel__title">
        {{ tool.title }}
        <span class="tool-panel__subtitle" v-if="tool.subtitle">{{ $t(tool.subtitle) }}</span>
      </h1>

      <!-- Вместо статистики игры (часы, дата сборки) — описание: у
           инструмента нет ни того, ни другого, и пустые поля выглядели бы
           поломкой. -->
      <p class="tool-panel__description" v-if="tool.description">{{ $t(tool.description) }}</p>

      <p class="tool-panel__size" v-if="tool.sizeBytes && !installed">
        {{ $t("#L_TOOL_SIZE", sizeText) }}
      </p>
    </div>

    <div v-if="!heroOnly" class="tool-panel__bar">
      <span class="tool-panel__spacer"></span>
      <button v-if="tool.state === 'installing'" type="button" class="tool-panel__help"
        :title="$t('#L_CANCEL')" :aria-label="$t('#L_CANCEL')" @click="$emit('cancel', tool.toolId)">
        <i class="fas fa-times" aria-hidden="true"></i>
      </button>

      <!-- Вопросительный знак: вопрос «как настроить таймер» — самый частый.
           Стоит в одном ряду с папкой и удалением, а не в другом конце
           полосы: это всё действия над инструментом. -->
      <button
        v-if="tool.hasHelp"
        type="button"
        class="tool-panel__help"
        :title="$t('#L_TOOL_HELP')"
        :aria-label="$t('#L_TOOL_HELP')"
        @click="$emit('help', tool.toolId)"
      >
        <i class="fas fa-question" aria-hidden="true"></i>
      </button>

      <!-- Папка инструмента вместо шестерёнки. Настраивается таймер в самом
           таймере, а вот добраться до его сплитов и конфигов через проводник
           нужно постоянно — своих настроек у лаунчера тут нет. -->
      <button
        v-if="installed"
        type="button"
        class="tool-panel__folder"
        :title="$t('#L_TOOL_FOLDER')"
        :aria-label="$t('#L_TOOL_FOLDER')"
        :disabled="busy"
        @click="$emit('folder', tool.toolId)"
      >
        <i class="fas fa-folder-open" aria-hidden="true"></i>
      </button>

      <button
        v-if="installed"
        type="button"
        class="tool-panel__remove"
        :title="$t('#L_TOOL_REMOVE')"
        :aria-label="$t('#L_TOOL_REMOVE')"
        :disabled="busy || running"
        @click="$emit('remove', tool.toolId)"
      >
        <i class="fas fa-trash" aria-hidden="true"></i>
      </button>

      <button type="button" class="tool-panel__action"
        :class="{ 'is-progress': !running && (!installed || tool.updateAvailable) }"
        v-grid-width :disabled="busy" @click="onAction">
        <span class="tool-panel__action-label" v-grid-width:8>
          <i :class="actionIcon" aria-hidden="true"></i>
          <span>{{ actionLabel }}</span>
        </span>
      </button>
    </div>

    <div class="tool-panel__progress" v-if="busy && !heroOnly">
      <div class="tool-panel__progress-fill" :style="{ width: percent + '%' }"></div>
    </div>
  </div>
</template>

<script>
import gridWidth from "./grid-width";
import { formatSize } from "./file-format";

export default {
  name: "ToolPanel",
  directives: { gridWidth },
  emits: ["cancel", "folder", "help", "install", "launch", "remove", "stop"],
  props: {
    heroOnly: { type: Boolean, default: false },
    controlsOnly: { type: Boolean, default: false },
    tool: {
      type: Object,
      required: true,
    },
  },
  computed: {
    installed() {
      return this.tool.state === "ready";
    },
    running() {
      return this.tool.running === true;
    },
    busy() {
      return this.tool.state === "installing" || this.tool.stopping === true;
    },
    percent() {
      return Math.max(0, Math.min(100, this.tool.percent || 0));
    },
    actionLabel() {
      if (this.tool.stopping) return this.$t("#L_TOOL_STOPPING");
      if (this.running) return this.$t("#L_CLOSE");
      if (this.busy) return this.$t("#L_TOOL_INSTALLING");
      if (this.tool.updateAvailable) return this.$t("#L_TOOL_UPDATE");
      return this.$t(this.installed ? "#L_TOOL_RUN" : "#L_TOOL_INSTALL");
    },
    actionIcon() {
      if (this.busy) return "fas fa-spinner fa-spin";
      if (this.running) return "fas fa-stop";
      if (this.tool.updateAvailable) return "fas fa-arrow-up";
      return this.installed ? "fas fa-play" : "fas fa-download";
    },
    sizeText() {
      return formatSize(this.tool.sizeBytes || 0, this.$t);
    },
  },
  methods: {
    onAction() {
      if (this.busy) return;
      if (this.running) return this.$emit("stop", this.tool.toolId);
      if (this.installed && !this.tool.updateAvailable) this.$emit("launch", this.tool.toolId);
      else this.$emit("install", this.tool.toolId);
    },
  },
};
</script>

<style lang="scss" scoped>
.tool-panel {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  justify-content: flex-start;
  gap: 24px;
  position: relative;
  overflow: hidden;

  &--controls {
    flex: none;
    gap: 0;
    overflow: visible;
    background: transparent;
    z-index: 6;
    &::before {
      content: "";
      position: absolute;
      left: 0; right: 0; bottom: 0;
      top: calc(-1 * var(--l-footer-feather));
      background: var(--l-footer-fill);
      -webkit-mask-image: linear-gradient(90deg, transparent 0, #000 var(--l-footer-lead-in));
      mask-image: linear-gradient(90deg, transparent 0, #000 var(--l-footer-lead-in));
      pointer-events: none;
      z-index: -1;
    }
    .tool-panel__progress { position: absolute; left: 0; right: 0; bottom: 0; }
  }

  /* Название, описание и нижняя полоса стоят НАД фоном. Без этого фоновый
     слой ложился поверх текста и гасил его — на скриншоте было видно, как
     заголовок ушёл в темноту. */
  &__hero,
  &__bar,
  &__progress {
    position: relative;
    z-index: 1;
  }

  /* Блок стоит от низа окна, поэтому сетку держат высоты строк: все в px,
     кратных 4 */
  &__hero {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: var(--l-hero-gap);
    padding: var(--l-hero-top) var(--l-hero-pad) 0;
    max-width: 640px;

  }

  &__title {
    margin: 0;
    font-size: var(--l-hero-title);
    font-weight: 700;
    line-height: var(--l-hero-title-lh);
    letter-spacing: 0.14em;
    text-transform: uppercase;
    color: #fff;
    text-shadow: 0 5px 14px rgba(0, 0, 0, 0.75);
    display: flex;
    flex-direction: column;
  }

  &__subtitle {
    display: block;
    margin-top: 8px;
    font-size: 0.4em;
    font-weight: 600;
    line-height: var(--l-hero-subtitle-lh);
    letter-spacing: 0.3em;
    color: rgba(255, 255, 255, 0.85);
    text-shadow: 0 4px 10px rgba(0, 0, 0, 0.7);
    white-space: nowrap;
  }

  &__description {
    margin: 0;
    font-size: 14px;
    line-height: 24px;
    color: var(--l-text-dim);
  }

  &__size {
    margin: 0;
    font-size: 13px;
    line-height: 20px;
    color: var(--l-text-faint);
  }

  /* Уже 1600 px описание лежит на синем шарике фона (LauncherBackground):
     под ним размытая тёмная подложка. На широком окне шарик правее текста. */
  @media (max-width: 1599px) {
    &__description {
      position: relative;
      isolation: isolate;

      &::before {
        content: "";
        position: absolute;
        inset: -10px -16px;
        z-index: -1;
        border-radius: 16px;
        background: rgba(10, 13, 18, 0.8);
        filter: blur(12px);
        pointer-events: none;
      }
    }
  }

  &__bar {
    display: flex;
    align-items: center;
    gap: var(--l-control-gap);
    height: var(--l-bottombar-h);
    min-height: var(--l-bottombar-h);
    padding: 0 var(--l-edge-pad) 0 var(--l-hero-pad);
  }

  &__spacer {
    flex: 1 1 auto;
  }

  /* Кнопки слева — как меню игры рядом с «Играть» (GameMenu): той же
     высоты, что «Установить» / «Запустить», 64 px в ширину. При наведении
     только светлеют, на месте. */
  &__help,
  &__folder,
  &__remove {
    display: flex;
    align-items: center;
    justify-content: center;
    flex: 0 0 64px;
    width: 64px;
    height: var(--l-play-h);
    padding: 0;
    border: none;
    background: var(--l-surface-solid);
    border-radius: var(--l-radius);
    color: var(--l-text-dim);
    font-size: 18px;
    cursor: pointer;
    transition: background var(--l-fast), color var(--l-fast);

    > i {
      width: 24px;
      line-height: 24px;
      text-align: center;
    }

    &:hover:not(:disabled) {
      background: linear-gradient(rgba(255, 255, 255, 0.08), rgba(255, 255, 255, 0.08)), var(--l-surface-solid);
      color: var(--l-text);
    }

    &:active:not(:disabled) {
      background: linear-gradient(rgba(var(--accent-rgb, 0, 171, 255), 0.16), rgba(var(--accent-rgb, 0, 171, 255), 0.16)), var(--l-surface-solid);
    }

    &:disabled {
      opacity: 0.5;
      cursor: default;
    }
  }

  &__action {
    --action-bg: var(--btn-ready-bg, var(--l-accent));
    --action-hover: var(--btn-ready-hover, var(--l-accent-hover));
    --action-pressed: var(--btn-ready-pressed, var(--l-accent));
    --action-text: var(--btn-ready-text, #fff);
    --action-glow: var(--btn-ready-glow, var(--l-glow));
    display: flex;
    align-items: center;
    min-width: var(--l-play-w);
    justify-content: center;
    height: var(--l-play-h);
    padding: 0 28px;
    border: none;
    border-radius: var(--l-radius);
    background: var(--action-bg);
    color: var(--action-text);
    font-family: inherit;
    font-size: var(--l-play-font);
    font-weight: 700;
    line-height: 24px;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    cursor: pointer;
    /* Как «Играть» (DownloadBar): при наведении меняется только цвет. */
    box-shadow: 0 6px 22px var(--action-glow);
    transition: background var(--l-fast), transform var(--l-fast), box-shadow var(--l-med);

    &.is-progress {
      --action-bg: var(--btn-progress-bg);
      --action-hover: var(--btn-progress-hover);
      --action-pressed: var(--btn-progress-pressed);
      --action-text: var(--btn-progress-text, #fff);
      --action-glow: var(--btn-progress-glow);
    }

    &:hover:not(:disabled) {
      background: var(--action-hover);
      box-shadow: 0 8px 26px var(--action-glow);
    }

    &:active:not(:disabled) {
      background: var(--action-pressed);
      transform: scale(0.98);
    }

    &:disabled {
      opacity: 0.6;
      cursor: default;
    }
  }

  /* Как у «Играть»: ширина центрируемой пары кратна 8. */
  &__action-label {
    display: flex;
    align-items: center;
    gap: 12px;

    i {
      width: 24px;
      line-height: 24px;
      text-align: center;
    }
  }

  &__progress {
    height: 4px;
    background: rgba(255, 255, 255, 0.08);
    overflow: hidden;
  }

  &__progress-fill {
    height: 100%;
    background: var(--l-accent, #00abff);
    transition: width var(--l-fast);
  }
}

</style>
