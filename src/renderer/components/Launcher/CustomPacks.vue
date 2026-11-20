<template>
  <!-- Моды игры (src/main/game-delivery/custom-packs.js): HL1 и Ghosting. Нажатие на превью
       или переключатель сразу кладёт файлы мода в игру или убирает их: «Сохранить» здесь не нужен. -->
  <section id="gx-panel" class="gx-pane cpk" role="tabpanel" aria-labelledby="gx-tab-mods">
    <header class="gx-pane__head">
      <h3 class="gx-pane__title">{{ $t(title) }}</h3>
      <p class="gx-pane__note" :title="$t(noteKey)">{{ $t(noteKey) }}</p>
    </header>

    <div class="cpk__list">
      <p v-if="!packs" class="cpk__state">
        <i class="fas fa-spinner fa-spin" aria-hidden="true"></i> {{ $t("#L_GS_LOADING") }}
      </p>
      <p v-if="error && !errorPack" class="cpk__error" role="alert">{{ $t(error) }}</p>

      <!-- Плитки в сетке: ряды растягиваются на всю высоту панели, превью забирает свободное место. -->
      <div class="cpk__grid">
      <article v-for="pack in packs || []" :key="pack.id" class="cpk-card" :class="{ 'is-on': pack.enabled, 'is-off': !pack.available }">
        <!-- Превью — тоже переключатель: нажатие включает или выключает мод. -->
        <button
          type="button"
          class="cpk-card__preview"
          :aria-pressed="pack.enabled ? 'true' : 'false'"
          :aria-label="$t('#L_CPK_PREVIEW', $t(titleKey(pack)))"
          :aria-describedby="'cpk-' + pack.id + '-desc'"
          :disabled="!pack.available || !!switching"
          @click="toggle(pack, !pack.enabled)"
        >
          <!-- Сцена — контейнер размера: по её пропорциям кадры «до / после» идут рядом или друг под другом. -->
          <span class="cpk-card__stage">
            <span v-if="SHOTS[pack.id]" class="cpk-card__shots">
              <span v-for="shot in SHOTS[pack.id]" :key="shot.src" class="cpk-shot">
                <img :src="shot.src" alt="" loading="lazy" decoding="async" :style="{ objectPosition: shot.focus }" />
                <span v-if="shot.label" class="cpk-shot__label">{{ $t(shot.label) }}</span>
              </span>
            </span>
            <span v-else class="cpk-card__icon" aria-hidden="true"><i class="fas" :class="KIND_ICONS[pack.kind]"></i></span>
          </span>
          <span v-if="pack.enabled" class="cpk-card__check" aria-hidden="true"><i class="fas fa-check"></i></span>
        </button>

        <div class="cpk-card__body">
          <div class="cpk-card__head">
            <div class="cpk-card__titles">
              <h4 :id="'cpk-' + pack.id" class="cpk-card__title">{{ $t(titleKey(pack)) }}</h4>
              <span class="cpk-card__author">{{ pack.author ? $t("#L_CPK_AUTHOR", pack.author) : $t("#L_CPK_AUTHOR_UNKNOWN") }}</span>
            </div>
            <label class="gx-switch" :class="{ 'is-busy': switching === pack.id }">
              <input
                type="checkbox"
                :checked="pack.enabled"
                :disabled="!pack.available || !!switching"
                :aria-labelledby="'cpk-' + pack.id"
                :aria-describedby="'cpk-' + pack.id + '-desc'"
                @change="toggle(pack, $event.target.checked, $event.target)"
              />
              <span class="gx-switch__track" aria-hidden="true"><span class="gx-switch__thumb"></span></span>
            </label>
          </div>

          <p :id="'cpk-' + pack.id + '-desc'" class="cpk-card__desc">{{ $t(descKey(pack)) }}</p>
          <p class="cpk-card__meta">
            <i class="fas" :class="KIND_ICONS[pack.kind]" aria-hidden="true"></i>
            {{ $t("#L_CPK_KIND_" + pack.kind.toUpperCase(), pack.files.length) }}
          </p>
          <p v-if="!pack.available" class="cpk-card__needs">{{ $t(!installed ? "#L_CPK_NOT_INSTALLED" : isHl1 ? "#L_CPK_NOT_AVAILABLE" : "#L_CPK_NOT_AVAILABLE_GHOSTING") }}</p>
          <!-- Ошибка — в карточке своего мода: верх списка может быть прокручен. -->
          <p v-if="error && errorPack === pack.id" class="cpk__error" role="alert">{{ $t(error) }}</p>

          <details class="cpk-files">
            <summary>{{ $t("#L_CPK_FILES", pack.files.length) }}</summary>
            <ul class="cpk-files__list">
              <li v-for="file in pack.files" :key="file" class="cpk-files__row">
                <code class="cpk-files__name">{{ file }}</code>
                <template v-if="pack.kind === 'sounds' && pack.available">
                  <button
                    v-for="which in ['original', 'pack']"
                    :key="which"
                    type="button"
                    class="gx-btn"
                    :aria-pressed="isPlaying(pack, file, which) ? 'true' : 'false'"
                    :aria-label="$t(which === 'pack' ? '#L_CPK_PLAY_PACK' : '#L_CPK_PLAY_ORIGINAL') + ': ' + file"
                    @click="play(pack, file, which)"
                  >
                    <i class="fas" :class="isPlaying(pack, file, which) ? 'fa-stop' : 'fa-play'" aria-hidden="true"></i>
                    {{ $t(which === "pack" ? "#L_CPK_PLAY_PACK" : "#L_CPK_PLAY_ORIGINAL") }}
                  </button>
                </template>
              </li>
            </ul>
          </details>
        </div>
      </article>
      </div>
    </div>
  </section>
</template>

<script>
import { GOLDSRC_GAME_IDS } from "./launcher-data";

const { ipcRenderer } = window.hlsrElectron;

// Снимки «без мода / с модом». HL1: из HLSR 0.2.9 (HL-HLSR/HLSR-Legacy);
// Ghosting: кадры игры (d2_coast_01). У ножа один кадр без подписи: нож в руке на фоне неба
// (взгляд вверх через cl_pitchdown -75, нож взят командой use weapon_crowbar после появления).
// focus — точка кадра, которая остаётся видна, когда превью обрезает кадр под свой размер.
const SHOTS = {
  "item-sprites": [
    { src: require("@/assets/launcher/custom-packs/item-sprites-off.jpg"), label: "#L_CPK_SHOT_OFF", focus: "50% 62%" },
    { src: require("@/assets/launcher/custom-packs/item-sprites-on.jpg"), label: "#L_CPK_SHOT_ON", focus: "50% 62%" },
  ],
  "condensed-hud": [
    { src: require("@/assets/launcher/custom-packs/condensed-hud-off.jpg"), label: "#L_CPK_SHOT_MOD_OFF", focus: "0% 100%" },
    { src: require("@/assets/launcher/custom-packs/condensed-hud-on.jpg"), label: "#L_CPK_SHOT_MOD_ON", focus: "0% 100%" },
  ],
  "m9-bayonet": [
    { src: require("@/assets/launcher/custom-packs/m9-bayonet.jpg"), focus: "64% 35%" },
  ],
  // Перекраски Eold: снимки автора со страниц GameBanana.
  "signed-smg": [
    { src: require("@/assets/launcher/custom-packs/signed-smg.jpg"), focus: "55% 45%" },
  ],
  "signed-shotgun": [
    { src: require("@/assets/launcher/custom-packs/signed-shotgun.jpg"), focus: "40% 50%" },
  ],
  "signed-rpg": [
    { src: require("@/assets/launcher/custom-packs/signed-rpg.jpg"), focus: "55% 40%" },
  ],
};
const KIND_ICONS = { models: "fa-crosshairs", skins: "fa-spray-can", sprites: "fa-shapes", sounds: "fa-volume-up", hud: "fa-gauge-high" };
// Ответы custom-packs-set → строки; код настроек показывает те же, когда переключает моды из кода.
export const ERRORS = {
  GAME_RUNNING: "#L_CPK_ERR_RUNNING",
  BUSY: "#L_CPK_ERR_BUSY",
  FILE_LOCKED: "#L_CPK_ERR_LOCKED",
  CUSTOM_PACK_NOT_AVAILABLE: "#L_CPK_NOT_AVAILABLE",
  NOT_INSTALLED: "#L_CPK_NOT_INSTALLED",
};
const keyOf = (pack) => pack.id.replace(/-/g, "_").toUpperCase();

export default {
  name: "CustomPacks",
  props: {
    gameId: { type: String, required: true },
    title: { type: String, required: true },
  },
  // packs — список с диска после загрузки и каждого переключения (предпросмотр HUD знает о моде).
  emits: ["packs"],
  data() {
    return { SHOTS, KIND_ICONS, packs: null, installed: true, error: "", errorPack: "", switching: "", playing: "" };
  },
  computed: {
    isHl1() { return GOLDSRC_GAME_IDS.includes(this.gameId); },
    noteKey() { return this.isHl1 ? "#L_CPK_NOTE_HL1" : "#L_CPK_NOTE_GHOSTING"; },
  },
  mounted() {
    this.load();
  },
  beforeUnmount() {
    this.stop();
  },
  methods: {
    titleKey(pack) { return "#L_CPK_TITLE_" + keyOf(pack); },
    descKey(pack) { return "#L_CPK_DESC_" + keyOf(pack); },
    async load() {
      const result = await ipcRenderer.invoke("custom-packs-get", this.gameId).catch(() => null);
      if (result && result.status === "ok") {
        this.packs = result.packs;
        this.$emit("packs", result.packs);
        this.installed = result.installed !== false;
      } else {
        this.packs = [];
        this.error = "#L_CPK_ERR_FAILED";
      }
    },
    // input — переключатель, если нажали его: после ответа он показывает то, что на диске.
    async toggle(pack, enabled, input) {
      if (this.switching) {
        if (input) input.checked = pack.enabled;
        return;
      }
      this.switching = pack.id;
      this.error = "";
      this.errorPack = pack.id;
      this.stop();
      try {
        const result = await ipcRenderer.invoke("custom-packs-set", this.gameId, pack.id, !!enabled);
        if (result && result.packs) {
          this.packs = result.packs;
          this.$emit("packs", result.packs);
        }
        if (!result || result.status !== "ok") this.error = ERRORS[result && result.code] || "#L_CPK_ERR_FAILED";
      } catch (error) {
        this.error = "#L_CPK_ERR_FAILED";
      } finally {
        this.switching = "";
        const fresh = (this.packs || []).find((item) => item.id === pack.id);
        if (input) input.checked = !!(fresh && fresh.enabled);
      }
    },
    isPlaying(pack, file, which) { return this.playing === `${pack.id}/${file}/${which}`; },
    stop() {
      if (this.audio) this.audio.pause();
      this.audio = null;
      this.playing = "";
    },
    async play(pack, file, which) {
      const id = `${pack.id}/${file}/${which}`;
      const again = this.playing === id;
      this.stop();
      if (again) return;
      this.playing = id;
      const result = await ipcRenderer.invoke("custom-packs-sound", this.gameId, pack.id, file, which).catch(() => null);
      if (this.playing !== id) return;
      if (!result || result.status !== "ok") {
        this.playing = "";
        this.error = "#L_CPK_ERR_SOUND";
        this.errorPack = pack.id;
        return;
      }
      this.audio = new Audio(result.dataUrl);
      this.audio.onended = () => { if (this.playing === id) this.playing = ""; };
      this.audio.play().catch(() => { if (this.playing === id) this.playing = ""; });
    },
  },
};
</script>

<style lang="scss" scoped>
@import "./settings-fields";

/* Прокрутка панели; в маленьком окне и при крупном масштабе прокручиваются плитки. */
.cpk__list {
  container: cpk / inline-size;
  display: flex;
  flex: 1;
  flex-direction: column;
  gap: 12px;
  min-height: 0;
  padding: 14px;
  overflow-y: auto;
  overscroll-behavior: contain;
  @include gx-scrollbar;
}

.cpk__state { margin: 0; color: var(--l-text-dim); font-size: 13px; }
.cpk__error { margin: 0; color: #ff8a96; font-size: 12.5px; line-height: 1.4; }

/* Плитки: колонок столько, сколько влезает по 320 px (2 мода Ghosting — две колонки,
   3 пака HL1 — три в ряд). Ряды не ниже 380 px и растягиваются на всю высоту панели
   (align-content: stretch для auto-рядов), поэтому снизу не остаётся пустого места.
   Высокое содержимое (раскрытый список файлов) растит свой ряд, а не налезает на соседний. */
.cpk__grid {
  display: grid;
  flex: 1 0 auto;
  grid-template-columns: repeat(auto-fit, minmax(min(100%, 320px), 1fr));
  grid-auto-rows: minmax(380px, auto);
  align-content: stretch;
  gap: 14px;
}

/* Две колонки и нечётное число плиток (3 пака HL1): последняя — по центру второго ряда. */
@container cpk (min-width: 654px) and (max-width: 987px) {
  .cpk-card:last-child:nth-child(odd) {
    grid-column: 1 / -1;
    justify-self: center;
    width: calc((100% - 14px) / 2);
  }
}

/* Плитка: сверху крупное превью (оно же кнопка), ниже название, переключатель и что меняется.
   Включённый мод подсвечен светлым цветом текста темы: в тёмных темах это белый,
   в «Бумаге» — тёплый светлый. */
.cpk-card {
  display: flex;
  flex-direction: column;
  gap: 12px;
  min-width: 0;
  padding: 12px;
  border: 1px solid var(--l-border);
  border-radius: var(--l-radius-sm);
  background: rgba(255, 255, 255, 0.025);
  transition: border-color var(--l-fast), box-shadow var(--l-med);

  &.is-on {
    border-color: color-mix(in srgb, var(--l-text) 42%, transparent);
    box-shadow: 0 0 22px -10px color-mix(in srgb, var(--l-text) 60%, transparent);
  }

  /* Превью забирает всё свободное место плитки. */
  &__preview {
    position: relative;
    display: flex;
    flex: 1 1 auto;
    min-height: 150px;
    padding: 0;
    border: 0;
    border-radius: 10px;
    background: transparent;
    color: inherit;
    font: inherit;
    cursor: pointer;
    transition: box-shadow var(--l-med), transform 90ms var(--l-ease, ease-out), filter var(--l-fast), opacity var(--l-fast);

    &:hover:not(:disabled) { filter: brightness(1.1); }
    &:active:not(:disabled) { transform: scale(0.99); }
    &:focus-visible { outline: 2px solid var(--accent-focus, var(--l-accent)); outline-offset: 4px; }
    &:disabled { cursor: default; opacity: 0.5; }
  }

  /* Выбранный: светлая обводка и мягкое свечение за превью. */
  &.is-on &__preview {
    box-shadow:
      0 0 0 2px var(--l-text),
      0 0 18px 2px color-mix(in srgb, var(--l-text) 38%, transparent);
  }

  &__stage {
    display: flex;
    flex: 1;
    min-width: 0;
    container: cpk-stage / size;
  }

  /* Кадры «до / после» друг под другом; в широкой сцене (шире 2:1) — рядом. */
  &__shots { display: flex; flex: 1; flex-direction: column; gap: 6px; min-width: 0; }

  /* Пак без снимков: крупный значок на мягкой подложке цвета акцента темы. */
  &__icon {
    display: grid;
    flex: 1;
    place-items: center;
    border: 1px solid color-mix(in srgb, var(--accent, var(--l-accent)) 22%, var(--l-border));
    border-radius: 10px;
    background:
      radial-gradient(120% 90% at 50% 35%, color-mix(in srgb, var(--accent, var(--l-accent)) 24%, transparent), transparent 70%),
      linear-gradient(160deg, color-mix(in srgb, var(--accent, var(--l-accent)) 10%, transparent), rgba(255, 255, 255, 0.02));
    color: color-mix(in srgb, var(--accent, var(--l-accent)) 70%, var(--l-text));
    font-size: clamp(40px, 34cqmin, 128px);

    i { filter: drop-shadow(0 6px 18px color-mix(in srgb, var(--accent, var(--l-accent)) 40%, transparent)); }
  }

  &__check {
    position: absolute;
    top: -7px;
    right: -7px;
    display: grid;
    width: 22px;
    height: 22px;
    place-items: center;
    border-radius: 50%;
    background: var(--l-text);
    color: #15181c;
    font-size: 11px;
    box-shadow: 0 1px 4px rgba(0, 0, 0, 0.45);
  }

  &__body { display: flex; flex: none; flex-direction: column; gap: 6px; min-width: 0; }
  &__head { display: flex; align-items: flex-start; justify-content: space-between; gap: 12px; }
  &__titles { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  &__title { margin: 0; font-size: 14.5px; font-weight: 600; line-height: 1.3; }
  &__author { color: var(--l-text-dim); font-size: 12px; }
  &__desc { margin: 0; color: var(--l-text); font-size: 13px; line-height: 1.45; text-wrap: pretty; }
  &__meta { display: flex; align-items: center; gap: 7px; margin: 0; color: var(--l-text-dim); font-size: 12px; }
  &__meta i { width: 14px; text-align: center; }
  &__needs { margin: 0; color: var(--l-text-faint); font-size: 12px; line-height: 1.4; }

  &.is-off .gx-switch { opacity: 0.45; cursor: default; }
  &.is-off .gx-switch input { cursor: default; }
}

/* Снимок в превью: скруглённый, подпись сверху (внизу у HUD — сами цифры). */
.cpk-shot {
  position: relative;
  flex: 1 1 0;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  border-radius: 8px;
  background: rgba(0, 0, 0, 0.3);

  img { display: block; width: 100%; height: 100%; object-fit: cover; }

  &__label {
    position: absolute;
    top: 5px;
    left: 5px;
    padding: 2px 6px;
    border-radius: 5px;
    background: rgba(0, 0, 0, 0.62);
    color: #fff;
    font-size: 11.5px;
    font-weight: 600;
    line-height: 1.3;
  }
}

@container cpk-stage (min-aspect-ratio: 2/1) {
  .cpk-card__shots { flex-direction: row; }
}

.gx-switch.is-busy { opacity: 0.6; }

.cpk-files {
  margin-top: 2px;

  summary {
    width: fit-content;
    color: var(--l-text-dim);
    font-size: 12.5px;
    cursor: pointer;
    border-radius: 4px;
  }
  summary:hover { color: var(--l-text); }
  summary:focus-visible { outline: 2px solid var(--accent-focus); outline-offset: 2px; }

  /* Длинный список прокручивается внутри плитки, сетку он не раздувает. */
  &__list {
    display: flex;
    flex-direction: column;
    max-height: 200px;
    margin: 8px 0 0;
    padding: 0 4px 0 0;
    overflow-y: auto;
    list-style: none;
    @include gx-scrollbar;
  }
  &__row {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 30px;
    padding: 3px 0;
    border-top: 1px solid rgba(255, 255, 255, 0.05);
  }
  &__name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    color: var(--l-text-dim);
    font-family: Consolas, monospace;
    font-size: 12px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .gx-btn { height: 26px; padding: 0 9px; }
}

/* Одна колонка (маленькое окно, крупный масштаб): плитка в строку — превью слева,
   текст справа, ряды ниже, панель прокручивается. В конце файла: правила плитки выше
   с той же специфичностью иначе перебили бы эти. */
@container cpk (max-width: 653px) {
  .cpk__grid { grid-auto-rows: minmax(220px, auto); }
}
@container cpk (min-width: 460px) and (max-width: 653px) {
  .cpk-card { flex-direction: row; }
  .cpk-card__preview { flex: 0 0 46%; }
  .cpk-card__body { flex: 1 1 0; }
}
</style>
