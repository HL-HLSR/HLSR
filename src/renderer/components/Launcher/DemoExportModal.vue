<template>
  <div class="dex" @mousedown.self="close">
    <div ref="window" class="dex__window" role="dialog" aria-modal="true" :aria-label="$t('#L_DEMO_EX_TITLE')">
      <header class="dex__head">
        <div class="dex__heading">
          <span class="dex__title">{{ $t("#L_DEMO_EX_TITLE") }}</span>
          <span class="dex__name" :title="item.name">{{ item.name }}</span>
        </div>
        <button ref="closeButton" type="button" class="dex__close" :title="$t('#L_CLOSE')" :aria-label="$t('#L_CLOSE')" @click="close">
          <i class="fas fa-times" aria-hidden="true"></i>
        </button>
      </header>

      <!-- Тело листается, шапка и кнопки внизу всегда на виду (Full HD: окно ниже содержимого). -->
      <div class="dex__body">
      <!-- Итог задания -->
      <div v-if="result" class="dex__outcome" role="status">
        <span class="dex__outcome-icon is-ok"><i class="fas fa-check" aria-hidden="true"></i></span>
        <div class="dex__outcome-body">
          <span class="dex__outcome-title">{{ $t("#L_DEMO_EX_DONE") }}</span>
          <span class="dex__outcome-file" :title="result.file">{{ result.fileName }}</span>
          <span class="dex__outcome-meta">{{ resultMeta }}</span>
          <span v-if="result.attempts > 1" class="dex__outcome-meta">{{ $t("#L_DEMO_EX_ATTEMPTS", String(result.attempts)) }}</span>
          <span v-for="key in resultWarnings" :key="key" class="dex__warn"><i class="fas fa-exclamation-triangle" aria-hidden="true"></i>{{ $t(key) }}</span>
        </div>
      </div>
      <div v-else-if="jobError" class="dex__outcome" role="alert">
        <span class="dex__outcome-icon is-error"><i class="fas fa-exclamation" aria-hidden="true"></i></span>
        <div class="dex__outcome-body">
          <span class="dex__outcome-title">{{ $t("#L_DEMO_EX_FAILED") }}</span>
          <span class="dex__outcome-meta">{{ errorText(jobError) }}</span>
          <div v-if="fixes.length" class="dex__fixes">
            <button v-for="fix in fixes" :key="fix.id" type="button" class="gx-btn" @click="fix.apply">{{ fix.label }}</button>
          </div>
        </div>
      </div>

      <template v-else>
        <p v-if="cancelled && !job" class="dex__notice" role="status">
          <i class="fas fa-ban" aria-hidden="true"></i><span>{{ $t("#L_DEMO_EXPORT_CANCELLED") }}</span>
        </p>
        <!-- Рендер HL1 ждёт bxt-rs: вместо строки «не готово» — карточка скачивания. -->
        <DemoFfmpegCard v-if="bxtrsNeeded" class="dex__ffmpeg" kind="bxtrs" :status="bxtrs" :error="bxtrsError"
          @install="$emit('install-bxtrs')" @cancel="$emit('cancel-bxtrs')" />
        <p v-else-if="renderBlocked" class="dex__notice">
          <i class="fas fa-info-circle" aria-hidden="true"></i><span>{{ errorText({ code: options.render.code }) }}</span>
        </p>

        <DemoFfmpegCard v-if="ffmpeg && ffmpeg.state !== 'ready'" class="dex__ffmpeg" :status="ffmpeg" :error="ffmpegError"
          @install="$emit('install-ffmpeg')" @cancel="$emit('cancel-ffmpeg')" />

        <div v-if="loading" class="dex__loading"><i class="fas fa-spinner fa-spin" aria-hidden="true"></i>{{ $t("#L_DEMO_EX_LOADING") }}</div>
        <p v-else-if="optionsError" class="dex__notice is-error"><span>{{ optionsError }}</span></p>

        <div v-else-if="options" class="dex__form" :inert="job ? true : null">
          <!-- Открыто вместо долгого предпросмотра: сколько он займёт и совет сразу отрендерить видео. -->
          <div v-if="longPreview && !job" class="dex__long" role="status">
            <i class="fas fa-hourglass-half" aria-hidden="true"></i>
            <div class="dex__long-body">
              <b>{{ $t("#L_DEMO_LONG_TITLE", durationLabel(longPreview.seconds)) }}</b>
              <span>{{ longPreview.segments > 1
                ? $t("#L_DEMO_LONG_TEXT", clock(longPreview.range), String(longPreview.segments))
                : $t("#L_DEMO_LONG_TEXT_ONE", clock(longPreview.range)) }}</span>
            </div>
          </div>

          <div class="dex__row">
            <span class="dex__label">{{ $t("#L_DEMO_EX_PRESET") }}</span>
            <div class="dex__seg" role="radiogroup" :aria-label="$t('#L_DEMO_EX_PRESET')">
              <button v-for="preset in presets" :key="preset.id" type="button" role="radio" class="dex__chip"
                :class="{ active: settings.preset === preset.id }" :aria-checked="settings.preset === preset.id ? 'true' : 'false'"
                @click="settings.preset = preset.id">
                <i :class="preset.icon" aria-hidden="true"></i>{{ $t(preset.label) }}
                <span v-if="estimates && estimates[preset.id]" class="dex__chip-time">{{ durationLabel(estimates[preset.id].seconds) }}</span>
              </button>
            </div>
          </div>

          <div class="dex__row">
            <span class="dex__label">{{ $t("#L_DEMO_EX_VIDEO") }}</span>
            <div class="dex__line">
              <div class="dex__seg" role="radiogroup" :aria-label="$t('#L_DEMO_EX_RES')">
                <button v-for="res in resolutionChoices" :key="res.height" type="button" role="radio" class="dex__chip"
                  :class="{ active: settings.height === res.height }" :disabled="!res.allowed"
                  :aria-checked="settings.height === res.height ? 'true' : 'false'" @click="pickHeight(res.height)">{{ res.label }}</button>
              </div>
              <div class="dex__seg" role="radiogroup" :aria-label="$t('#L_DEMO_EX_FPS')">
                <button v-for="rate in fpsChoices" :key="rate.fps" type="button" role="radio" class="dex__chip"
                  :class="{ active: settings.fps === rate.fps }" :disabled="!rate.allowed"
                  :aria-checked="settings.fps === rate.fps ? 'true' : 'false'" @click="settings.fps = rate.fps">{{ rate.fps }} FPS</button>
              </div>
            </div>
          </div>
          <p v-if="options.capabilities.maxHeight < 2160" class="dex__subhint">{{ $t("#L_DEMO_EX_RES_LIMIT") }}</p>

          <div class="dex__row">
            <span class="dex__label">{{ $t("#L_DEMO_EX_ENCODER") }}</span>
            <div class="dex__line">
              <div class="dex__seg" role="radiogroup" :aria-label="$t('#L_DEMO_EX_ENCODER')">
                <button v-for="mode in encoderModes" :key="mode.id" type="button" role="radio" class="dex__chip"
                  :class="{ active: settings.encoder === mode.id }" :disabled="!mode.available"
                  :title="mode.available ? null : $t('#L_DEMO_ERR_NO_GPU')"
                  :aria-checked="settings.encoder === mode.id ? 'true' : 'false'" @click="pickEncoder(mode.id)">{{ $t(mode.label) }}</button>
              </div>
              <span v-if="encoderLabel" class="dex__hint">{{ $t("#L_DEMO_EX_ENC_WILL", encoderLabel) }}</span>
            </div>
          </div>

          <div class="dex__row">
            <span class="dex__label">{{ $t("#L_DEMO_EX_SOUND") }}</span>
            <div class="dex__line">
              <label class="gx-switch" :class="{ 'is-off': !options.capabilities.audio }">
                <input type="checkbox" :checked="settings.audio && options.capabilities.audio" :disabled="!options.capabilities.audio"
                  :aria-label="$t('#L_DEMO_EX_SOUND')" @change="settings.audio = $event.target.checked" />
                <span class="gx-switch__track"><span class="gx-switch__thumb"></span></span>
              </label>
              <span v-if="!options.capabilities.audio" class="dex__hint">{{ $t("#L_DEMO_EX_AUDIO_NA") }}</span>
            </div>
          </div>

          <!-- Надпись проверки данных демки: только если адаптер её умеет (capabilities.integrity, сейчас Ghosting). -->
          <div v-if="integrityAvailable" class="dex__row">
            <span class="dex__label">GH DEMO DATA</span>
            <div class="dex__line">
              <label class="gx-switch">
                <input type="checkbox" :checked="settings.integrity" :aria-label="$t('#L_DEMO_EX_INTEGRITY')"
                  aria-describedby="dex-integrity-hint" @change="settings.integrity = $event.target.checked" />
                <span class="gx-switch__track"><span class="gx-switch__thumb"></span></span>
              </label>
            </div>
          </div>
          <p v-if="integrityAvailable" id="dex-integrity-hint" class="dex__subhint">{{ $t("#L_DEMO_EX_INTEGRITY_HINT") }}</p>

          <!-- Как у предпросмотра: KeyHUD, HUD игры, конфиг и HUD игрока — где игра их умеет. -->
          <DemoViewSettings v-model="view" class="dex__view" :caps="options.capabilities" :game="item.game" />

          <div class="dex__row">
            <span class="dex__label">{{ $t("#L_DEMO_EX_RANGE") }}</span>
            <div class="dex__line">
              <input :key="'from' + rangeRev" class="gx-input dex__time" :class="{ invalid: invalid.from }" :value="rangeText.from" placeholder="0:00.0"
                :aria-label="$t('#L_DEMO_EX_FROM')" :title="$t('#L_DEMO_EX_FROM')" @change="editRange('from', $event.target.value)" />
              <i class="fas fa-long-arrow-alt-right dex__dash" aria-hidden="true"></i>
              <input :key="'to' + rangeRev" class="gx-input dex__time" :class="{ invalid: invalid.to }" :value="rangeText.to" :placeholder="clock(duration)"
                :aria-label="$t('#L_DEMO_EX_TO')" :title="$t('#L_DEMO_EX_TO')" @change="editRange('to', $event.target.value)" />
              <button type="button" class="gx-btn" :disabled="!range" @click="$emit('range', null)">{{ $t("#L_DEMO_RANGE_RESET") }}</button>
            </div>
          </div>
          <p v-if="invalid.from || invalid.to" class="dex__subhint is-error">{{ $t(invalid.from || invalid.to, clock(duration)) }}</p>

          <div class="dex__row">
            <span class="dex__label">{{ $t("#L_DEMO_EX_SPEED") }}</span>
            <div class="dex__seg" role="radiogroup" :aria-label="$t('#L_DEMO_EX_SPEED')" :title="$t('#L_DEMO_EX_SPEED_HINT')">
              <button v-for="speed in speeds" :key="speed" type="button" role="radio" class="dex__chip dex__chip--num"
                :class="{ active: settings.speed === speed }" :aria-checked="settings.speed === speed ? 'true' : 'false'"
                @click="settings.speed = speed">{{ speedText(speed) }}</button>
            </div>
          </div>
          <p class="dex__subhint">{{ $t("#L_DEMO_EX_SPEED_HINT") }}</p>

          <div class="dex__row">
            <span class="dex__label">{{ $t("#L_DEMO_EX_FOLDER") }}</span>
            <div class="dex__line">
              <span class="dex__path" :title="outputDir"><bdi>{{ outputDir }}</bdi></span>
              <button type="button" class="gx-btn" @click="pickFolder"><i class="fas fa-folder-open" aria-hidden="true"></i>{{ $t("#L_DEMO_EX_FOLDER_CHANGE") }}</button>
            </div>
          </div>

          <div v-if="plan || planError || currentEstimate" class="dex__row dex__row--top">
            <span class="dex__label">{{ $t("#L_DEMO_EX_ESTIMATE") }}</span>
            <div class="dex__estimate">
              <span v-if="plan" class="dex__estimate-main">{{ estimateText }}</span>
              <span v-if="currentEstimate" class="dex__hint">{{ $t("#L_DEMO_EST_TIME", durationLabel(currentEstimate.seconds)) }}</span>
              <span v-if="plan && plan.mode === 'bitrate'" class="dex__hint">{{ bitrateText }}</span>
              <span v-for="key in planWarnings" :key="key" class="dex__warn"><i class="fas fa-exclamation-triangle" aria-hidden="true"></i>{{ $t(key) }}</span>
              <span v-if="planError" class="dex__warn is-error" role="alert"><i class="fas fa-exclamation-circle" aria-hidden="true"></i>{{ errorText(planError) }}</span>
              <div v-if="fixes.length" class="dex__fixes">
                <button v-for="fix in fixes" :key="fix.id" type="button" class="gx-btn" @click="fix.apply">{{ fix.label }}</button>
              </div>
            </div>
          </div>
        </div>

        <div v-if="job" class="dex__job" role="status" aria-live="polite">
          <div class="dex__job-head">
            <span class="dex__job-phase">{{ jobText }}</span>
            <span class="dex__job-percent">{{ Math.round(job.percent || 0) }}%</span>
          </div>
          <div class="dex__bar"><div class="dex__fill" :style="{ width: (job.percent || 0) + '%' }"></div></div>
        </div>
      </template>

      <p v-if="actionError" class="dex__notice is-error" role="alert"><span>{{ actionError }}</span></p>

      <p v-if="steamNote" class="dex__steam"><i class="fab fa-steam" aria-hidden="true"></i>{{ $t("#L_DEMO_STEAM_NOTE") }}</p>
      </div>

      <footer class="dex__actions">
        <template v-if="result">
          <button type="button" class="dex__btn" @click="$emit('again')"><i class="fas fa-redo" aria-hidden="true"></i>{{ $t("#L_DEMO_EX_AGAIN") }}</button>
          <span class="dex__spacer"></span>
          <button type="button" class="dex__btn" @click="reveal"><i class="fas fa-folder-open" aria-hidden="true"></i>{{ $t("#L_DEMO_EX_REVEAL") }}</button>
          <button type="button" class="dex__btn primary" @click="$emit('open-result', result)"><i class="fas fa-play" aria-hidden="true"></i>{{ $t("#L_DEMO_EX_OPEN_PLAYER") }}</button>
        </template>
        <template v-else-if="jobError">
          <span class="dex__spacer"></span>
          <button type="button" class="dex__btn" @click="close">{{ $t("#L_CLOSE") }}</button>
          <button type="button" class="dex__btn primary" @click="$emit('again')"><i class="fas fa-redo" aria-hidden="true"></i>{{ $t("#L_DEMO_EX_RETRY") }}</button>
        </template>
        <template v-else>
          <span class="dex__spacer"></span>
          <button type="button" class="dex__btn" @click="close">{{ $t(job ? "#L_CLOSE" : "#L_CANCEL") }}</button>
          <button v-if="job" type="button" class="dex__btn danger" @click="$emit('cancel-job')"><i class="fas fa-stop" aria-hidden="true"></i>{{ $t("#L_DEMO_JOB_CANCEL") }}</button>
          <button v-if="longPreview && !job" type="button" class="dex__btn" @click="$emit('preview-anyway')">
            <i class="fas fa-play" aria-hidden="true"></i>{{ $t("#L_DEMO_LONG_PREVIEW") }}
          </button>
          <button v-if="!job" type="button" class="dex__btn primary" :disabled="!canStart" :title="startBlocker" @click="start">
            <i class="fas fa-file-video" aria-hidden="true"></i>{{ $t(longPreview ? "#L_DEMO_LONG_RENDER" : "#L_DEMO_EX_START") }}
          </button>
        </template>
      </footer>
    </div>
  </div>
</template>

<script>
import DemoFfmpegCard from "./DemoFfmpegCard";
import DemoViewSettings from "./DemoViewSettings";
import { trapFocus } from "./focus-trap";
import { formatSize, formatNumber } from "./file-format";
import {
  CLIP_SPEEDS,
  RESOLUTIONS,
  combosFor,
  heightsOf,
  fpsFor,
  pickCombo,
  formatClock,
  parseClock,
  resolutionLabel,
  durationText,
  encoderName,
  errorMessage,
  warningKeys,
} from "./demo-tool";

const { ipcRenderer } = window.hlsrElectron;

// Окно экспорта клипа. Настройки и план — здесь; задание запускает и
// ведёт DemoPanel (события задания приходят и при закрытом окне).
export default {
  name: "DemoExportModal",
  components: { DemoFfmpegCard, DemoViewSettings },
  emits: ["close", "start", "cancel-job", "install-ffmpeg", "cancel-ffmpeg", "install-bxtrs", "cancel-bxtrs", "range", "open-result", "again", "preview-anyway"],
  props: {
    item: { type: Object, required: true },
    // Окно открыто вместо долгого предпросмотра: { seconds, range, segments } из demo-estimate.
    longPreview: { type: Object, default: null },
    // Стартовые значения поверх запомненных (экспорт готового видео ещё раз): { speed }.
    initial: { type: Object, default: null },
    duration: { type: Number, default: 0 },
    // Интервал во времени демо или null (весь ран).
    range: { type: Object, default: null },
    ffmpeg: { type: Object, default: null },
    ffmpegError: { type: String, default: null },
    bxtrs: { type: Object, default: null },
    bxtrsError: { type: String, default: null },
    job: { type: Object, default: null },
    result: { type: Object, default: null },
    jobError: { type: Object, default: null },
    // Идёт другое задание (предпросмотр): экспорт ждёт его.
    busy: { type: Boolean, default: false },
    // Последнее задание этой демки отменили: строка «Экспорт отменён».
    cancelled: { type: Boolean, default: false },
  },
  data() {
    return {
      loading: true,
      options: null,
      optionsError: null,
      settings: { preset: "quality", height: 1080, fps: 60, audio: true, keyhud: false, integrity: false, encoder: "auto", speed: 1,
        hud: true, ownConfig: true, ownHud: true },
      plan: null,
      planError: null,
      // Оценка времени рендера по пресетам: { quality, discord } (demo-estimate).
      estimates: null,
      outputDir: "",
      // Ключ текста ошибки поля интервала или null.
      invalid: { from: null, to: null },
      // Ревизия полей интервала: после правки DOM-значения берутся заново.
      rangeRev: 0,
      // Ошибка действия в окне («Показать в папке», выбор папки).
      actionError: null,
      presets: [
        { id: "quality", label: "#L_DEMO_EX_QUALITY", icon: "fas fa-gem" },
        { id: "discord", label: "#L_DEMO_EX_DISCORD", icon: "fab fa-discord" },
      ],
    };
  },
  computed: {
    renderBlocked() {
      return !!(this.options && this.options.render && !this.options.render.ok);
    },
    bxtrsNeeded() {
      return !!(this.bxtrs && this.renderBlocked && this.options.render.code === "BXTRS_MISSING");
    },
    combos() {
      return combosFor(this.options, this.settings.encoder);
    },
    resolutionChoices() {
      const heights = heightsOf(this.combos);
      const offered = (this.options && this.options.resolutions) || RESOLUTIONS;
      return offered.map((res) => ({ height: res.height, width: res.width, label: res.label || resolutionLabel(res.height), allowed: heights.includes(res.height) }));
    },
    fpsChoices() {
      const caps = (this.options && this.options.capabilities && this.options.capabilities.fps) || [30, 60, 120];
      const allowed = fpsFor(this.combos, this.settings.height);
      return caps.map((fps) => ({ fps, allowed: allowed.includes(fps) }));
    },
    encoderModes() {
      const modes = this.options && this.options.modes;
      return [
        { id: "auto", label: "#L_DEMO_EX_ENC_AUTO" },
        { id: "gpu", label: "#L_DEMO_EX_ENC_GPU" },
        { id: "cpu", label: "#L_DEMO_EX_ENC_CPU" },
      ].map((mode) => Object.assign(mode, { available: !modes || !!modes[mode.id] }));
    },
    encoderLabel() {
      const modes = this.options && this.options.modes;
      const entry = modes && modes[this.settings.encoder];
      return entry ? encoderName(entry.encoder) : "";
    },
    keyhudAvailable() {
      return !!(this.options && this.options.keyhud && this.options.keyhud.available);
    },
    // Переключатели «Дополнительных настроек» — часть настроек экспорта.
    view: {
      get() {
        const { keyhud, hud, ownConfig, ownHud } = this.settings;
        return { keyhud: keyhud && this.keyhudAvailable, hud, ownConfig, ownHud };
      },
      set(value) {
        Object.assign(this.settings, value);
      },
    },
    integrityAvailable() {
      return !!(this.options && this.options.capabilities && this.options.capabilities.integrity);
    },
    speeds() {
      const limits = (this.options && this.options.speed) || { min: 0.25, max: 4 };
      return CLIP_SPEEDS.filter((speed) => speed >= limits.min && speed <= limits.max);
    },
    rangeText() {
      return {
        from: this.range ? this.clock(this.range.from) : this.clock(0),
        to: this.range ? this.clock(this.range.to) : this.clock(this.duration),
      };
    },
    payload() {
      return Object.assign({}, this.settings, {
        audio: this.settings.audio && !!(this.options && this.options.capabilities.audio),
        keyhud: this.settings.keyhud && this.keyhudAvailable,
        integrity: this.settings.integrity && this.integrityAvailable,
        range: this.range ? { from: this.range.from, to: this.range.to } : null,
      });
    },
    currentEstimate() {
      return this.estimates ? this.estimates[this.settings.preset] || null : null;
    },
    estimateText() {
      const plan = this.plan;
      const size = formatSize(plan.estimatedBytes, this.$t);
      const parts = [this.$t("#L_DEMO_EX_LENGTH", this.clock(plan.outputDuration)), `${plan.width}×${plan.height}, ${plan.fps} FPS`,
        this.$t(plan.mode === "bitrate" ? "#L_DEMO_EX_SIZE_UPTO" : "#L_DEMO_EX_SIZE_ABOUT", size)];
      return parts.join(" · ");
    },
    bitrateText() {
      const video = formatNumber(this.plan.videoBps / 1e6, 1, this.$t);
      if (!this.plan.audioBps) return this.$t("#L_DEMO_EX_BITRATE_MUTE", video);
      return this.$t("#L_DEMO_EX_BITRATE", video, String(Math.round(this.plan.audioBps / 1000)));
    },
    planWarnings() {
      return this.plan ? warningKeys(this.plan.warnings) : [];
    },
    resultWarnings() {
      return this.result ? warningKeys(this.result.warnings) : [];
    },
    resultMeta() {
      const video = this.result.video || {};
      const parts = [formatSize(this.result.size, this.$t)];
      if (video.width) parts.push(`${video.width}×${video.height}, ${video.fps} FPS`);
      if (video.duration) parts.push(this.clock(video.duration));
      parts.push(this.$t("#L_DEMO_EX_ENC_USED", encoderName(this.result.encoder)));
      return parts.join(" · ");
    },
    // Быстрые исправления для «слишком длинно для Discord»: из плана или из
    // итога задания (после повторов кодирования); во втором случае окно
    // сначала возвращается к форме.
    fixes() {
      // Игра отрисовала меньше (масштаб экрана): наибольшее разрешение, которое влезает.
      if (this.jobError && this.jobError.code === "RESOLUTION_LIMITED") {
        const limit = this.jobError;
        const fit = this.resolutionChoices.filter((res) => res.allowed && res.height <= Number(limit.height) &&
          (!res.width || res.width <= Number(limit.width))).pop();
        return fit ? [{ id: "fit", label: this.$t("#L_DEMO_EX_FIX_RES", fit.label), apply: () => {
          this.$emit("again");
          this.pickHeight(fit.height);
        } }] : [];
      }
      const fromJob = !!(this.jobError && this.jobError.code === "DISCORD_TOO_LONG");
      const error = fromJob ? this.jobError : this.planError;
      if (!error || error.code !== "DISCORD_TOO_LONG") return [];
      const run = (act) => () => {
        if (fromJob) this.$emit("again");
        act();
      };
      const out = [];
      if (error.suggestedHeight && this.resolutionChoices.some((res) => res.height === error.suggestedHeight && res.allowed))
        out.push({ id: "res", label: this.$t("#L_DEMO_EX_FIX_RES", resolutionLabel(error.suggestedHeight)), apply: run(() => this.pickHeight(error.suggestedHeight)) });
      const seconds = Math.floor(Number(error.recommendedMaxSeconds) || 0);
      if (seconds >= 1) {
        const from = this.range ? this.range.from : 0;
        out.push({ id: "cut", label: this.$t("#L_DEMO_EX_FIX_CUT", String(seconds)),
          apply: run(() => this.$emit("range", { from, to: Math.min(this.duration, from + seconds * this.settings.speed) })) });
      }
      return out;
    },
    jobText() {
      const job = this.job;
      const phase = this.$t({ prepare: "#L_DEMO_PHASE_PREPARE", render: "#L_DEMO_PHASE_RENDER", encode: "#L_DEMO_PHASE_ENCODE",
        verify: "#L_DEMO_PHASE_VERIFY", done: "#L_DEMO_PHASE_DONE" }[job.phase] || "#L_DEMO_PHASE_PREPARE");
      // Повтор Discord: полоса кодирования снова идёт с 50 %, подпись говорит почему.
      const retry = /^retry-(\d+)$/.exec(job.message || "");
      const parts = [retry ? this.$t("#L_DEMO_JOB_RETRY", String(Number(retry[1]) + 1)) : phase];
      if (job.segment && job.segment.count > 1) parts.push(this.$t("#L_DEMO_JOB_SEGMENT", String(job.segment.index), String(job.segment.count)));
      return parts.join(" · ");
    },
    startBlocker() {
      if (this.busy && !this.job) return this.$t("#L_DEMO_ERR_BUSY");
      if (this.renderBlocked) return this.errorText({ code: this.options.render.code });
      if (this.ffmpeg && this.ffmpeg.state !== "ready") return this.$t("#L_DEMO_ERR_FFMPEG_MISSING");
      if (this.planError) return this.errorText(this.planError);
      return null;
    },
    // Запись идёт игрой: Steam на это время покажет «в игре».
    steamNote() {
      return !this.result && !this.jobError && !!this.options && !this.renderBlocked;
    },
    canStart() {
      return !!(this.options && this.plan && !this.planError && !this.renderBlocked && !this.job && !this.busy && !this.loading &&
        this.ffmpeg && this.ffmpeg.state === "ready");
    },
  },
  watch: {
    payload: {
      deep: true,
      handler() {
        clearTimeout(this.planTimer);
        this.planTimer = setTimeout(() => {
          this.loadPlan();
          this.loadEstimates();
        }, 150);
      },
    },
    "ffmpeg.state"(state, previous) {
      if (state === "ready" && previous !== "ready") this.loadOptions();
    },
    "bxtrs.state"(state, previous) {
      if (state === "ready" && previous !== "ready") this.loadOptions();
    },
    // Интервал изменили снаружи (плеер, «Взять весь ран», быстрые исправления).
    range() {
      this.invalid = { from: null, to: null };
      this.rangeRev++;
    },
  },
  created() {
    this.loadOptions();
    this.loadOutputDir();
  },
  mounted() {
    this.lastFocused = document.activeElement;
    document.addEventListener("keydown", this.onKeydown);
    this.$nextTick(() => this.$refs.closeButton && this.$refs.closeButton.focus());
  },
  beforeUnmount() {
    clearTimeout(this.planTimer);
    document.removeEventListener("keydown", this.onKeydown);
    if (this.lastFocused && this.lastFocused.focus) this.lastFocused.focus();
  },
  methods: {
    clock(seconds) {
      return formatClock(seconds, 1);
    },
    durationLabel(seconds) {
      const text = durationText(seconds);
      return this.$t(text.key, ...text.args);
    },
    // Время рендера для «Лучшего качества» и Discord при текущих разрешении, FPS и интервале.
    async loadEstimates() {
      if (!this.options) return;
      const ticket = (this.estimateTicket = (this.estimateTicket || 0) + 1);
      const variants = this.presets.map((preset) => Object.assign({}, this.payload, { preset: preset.id }));
      const answer = await ipcRenderer.invoke("demo-estimate", this.item.id, variants).catch(() => null);
      if (ticket !== this.estimateTicket) return;
      this.estimates = answer && answer.status === "ok"
        ? Object.fromEntries(this.presets.map((preset, index) => [preset.id, answer.items[index]]))
        : null;
    },
    speedText(speed) {
      return String(speed).replace(".", this.$t("#L_DECIMAL")) + "×";
    },
    errorText(error) {
      const message = errorMessage(error);
      return this.$t(message.key, ...message.args);
    },
    async loadOptions() {
      this.loading = true;
      this.optionsError = null;
      try {
        const answer = await ipcRenderer.invoke("demo-options", this.item.id);
        if (!answer || answer.status !== "ok") throw answer || {};
        this.options = answer;
        // Запомненные настройки (главный процесс хранит их после экспорта).
        const saved = Object.assign({}, answer.defaults, this.initial);
        for (const key of Object.keys(this.settings)) if (saved[key] !== undefined) this.settings[key] = saved[key];
        // «preview» — пресет предпросмотра, окно экспорта его не шлёт.
        if (!this.presets.some((preset) => preset.id === this.settings.preset)) this.settings.preset = "quality";
        if (this.encoderModes.some((mode) => mode.id === this.settings.encoder && !mode.available)) this.settings.encoder = "auto";
        this.fitCombo();
        this.loadPlan();
        this.loadEstimates();
      } catch (error) {
        this.optionsError = this.errorText(error && error.code ? error : { code: "DEMO_FAILED" });
      } finally {
        this.loading = false;
      }
    },
    async loadOutputDir() {
      try {
        const answer = await ipcRenderer.invoke("demo-settings-get");
        if (answer && answer.status === "ok") this.outputDir = answer.outputDir;
      } catch (ignored) {
        // Папку покажет следующий выбор.
      }
    },
    async loadPlan() {
      if (!this.options) return;
      const ticket = (this.planTicket = (this.planTicket || 0) + 1);
      try {
        const answer = await ipcRenderer.invoke("demo-plan", this.item.id, this.payload);
        if (ticket !== this.planTicket) return;
        if (answer && answer.status === "ok") {
          this.plan = answer.plan;
          this.planError = null;
        } else {
          this.plan = null;
          // Без FFmpeg плана нет: про это уже говорит карточка FFmpeg.
          this.planError = answer && /^FFMPEG_/.test(answer.code || "") ? null : answer || { code: "DEMO_FAILED" };
        }
      } catch (error) {
        if (ticket === this.planTicket) this.planError = { code: "DEMO_FAILED" };
      }
    },
    fitCombo() {
      const picked = pickCombo(this.combos, this.settings.height, this.settings.fps);
      if (picked) Object.assign(this.settings, picked);
    },
    pickHeight(height) {
      this.settings.height = height;
      this.fitCombo();
    },
    pickEncoder(id) {
      this.settings.encoder = id;
      this.fitCombo();
    },
    // Свой текст на каждую ошибку: формат, вне демки, конец раньше начала.
    editRange(which, text) {
      const value = parseClock(text);
      const current = this.range || { from: 0, to: this.duration };
      const next = Object.assign({}, current, { [which]: value });
      let problem = null;
      if (value === null) problem = "#L_DEMO_EX_RANGE_BAD";
      else if (value > this.duration + 0.05) problem = "#L_DEMO_EX_RANGE_OUT";
      else if (next.to <= next.from) problem = "#L_DEMO_ERR_RANGE";
      this.invalid = Object.assign({}, this.invalid, { [which]: problem });
      if (problem) return;
      next.to = Math.min(next.to, this.duration);
      const range = next.from <= 0 && next.to >= this.duration ? null : next;
      if (JSON.stringify(range) === JSON.stringify(this.range)) {
        this.invalid = { from: null, to: null };
        this.rangeRev++;
      } else this.$emit("range", range);
    },
    async pickFolder() {
      this.actionError = null;
      const answer = await ipcRenderer.invoke("demo-output-pick").catch(() => ({ status: "error", code: "DIALOG_UNAVAILABLE" }));
      if (answer && answer.status === "ok") this.outputDir = answer.outputDir;
      else if (answer && answer.status === "error") this.actionError = this.errorText(answer);
    },
    async reveal() {
      this.actionError = null;
      const answer = await ipcRenderer.invoke("demo-reveal", this.result.id).catch(() => null);
      if (!answer || answer.status !== "ok") this.actionError = this.errorText(answer || {});
    },
    start() {
      if (this.canStart) this.$emit("start", this.payload);
    },
    close() {
      this.$emit("close");
    },
    onKeydown(event) {
      if (event.key === "Escape") {
        event.preventDefault();
        this.close();
      } else if (event.key === "Tab") trapFocus(this.$refs.window, event);
    },
  },
};
</script>

<style lang="scss" scoped>
@import "./settings-fields";

.dex {
  position: absolute;
  inset: 0;
  z-index: 70;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--l-modal-scrim);

  &__window {
    display: flex;
    flex-direction: column;
    gap: 14px;
    width: min(680px, calc(100% - 80px));
    max-height: calc(100% - 60px);
    overflow: hidden;
    padding: 22px 24px 20px;
    border: 1px solid var(--l-modal-border);
    border-radius: var(--l-radius);
    background: var(--l-modal-bg);
    box-shadow: 0 24px 72px rgba(0, 0, 0, 0.6);
    color: var(--l-text);
    color-scheme: dark;
  }

  /* Полоса прокрутки — у края окна, а не у края полей. */
  &__body {
    flex: 1 1 auto;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 14px;
    margin: 0 -24px;
    padding: 0 24px;
    overflow-y: auto;
    overscroll-behavior: contain;
    @include gx-scrollbar;
  }

  &__head {
    display: flex;
    align-items: flex-start;
    gap: 12px;
  }

  &__heading {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  &__title {
    font-size: 15px;
    font-weight: 600;
    letter-spacing: 0.09em;
    text-transform: uppercase;
  }

  &__name {
    overflow: hidden;
    color: var(--l-text-dim);
    font-size: 13px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  &__close {
    flex: none;
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    margin: -4px -6px 0 0;
    padding: 0;
    border: 0;
    border-radius: var(--l-radius-sm);
    background: transparent;
    color: var(--l-text-dim);
    cursor: pointer;

    &:hover { background: rgba(255, 255, 255, 0.08); color: var(--l-text); }
    &:focus-visible { outline: 2px solid var(--l-accent); outline-offset: 0; }
  }

  &__notice {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    margin: 0;
    padding: 10px 12px;
    border-radius: var(--l-radius-sm);
    background: var(--l-note-bg);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: var(--l-text-dim);
    font-size: 13px;
    line-height: 1.45;

    i { margin-top: 2px; color: var(--accent); }
    &.is-error { color: var(--l-level-error); }
  }

  &__ffmpeg { max-width: none; }

  &__loading {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 24px 0;
    color: var(--l-text-dim);
    font-size: 13px;
  }

  &__form {
    display: flex;
    flex-direction: column;
    gap: 12px;
    transition: opacity var(--l-fast);

    &[inert] { opacity: 0.55; }
  }

  &__row {
    display: grid;
    grid-template-columns: 136px minmax(0, 1fr);
    align-items: center;
    gap: 12px;

    &--top { align-items: start; }
  }

  &__label {
    color: var(--l-text-dim);
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
  }

  &__row--top &__label { padding-top: 2px; }

  &__line {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px 10px;
    min-width: 0;
  }

  &__seg {
    justify-self: start;
    display: inline-flex;
    flex-wrap: wrap;
    gap: 3px;
    padding: 3px;
    border-radius: var(--l-radius-sm);
    background: var(--l-chip-bg);
    box-shadow: inset 0 0 0 1px var(--l-border);
  }

  &__chip-time {
    color: var(--l-text-faint);
    font-weight: 500;
    font-variant-numeric: tabular-nums;
  }

  &__chip.active &__chip-time { color: inherit; opacity: 0.75; }

  /* Окно вместо долгого предпросмотра. */
  &__long {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    padding: 12px 14px;
    border-radius: var(--l-radius-sm);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 35%, transparent);
    color: var(--l-text-dim);
    font-size: 13px;
    line-height: 1.45;

    i { margin-top: 3px; color: var(--accent); }
    b { color: var(--l-text); font-size: 14px; font-weight: 600; }
  }

  &__long-body {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }

  &__chip {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 30px;
    padding: 0 12px;
    border: 0;
    border-radius: 7px;
    background: transparent;
    color: var(--l-text-dim);
    font: inherit;
    font-size: 12.5px;
    font-weight: 600;
    white-space: nowrap;
    cursor: pointer;
    transition: background var(--l-fast), color var(--l-fast);

    i { font-size: 12px; }
    &--num { min-width: 46px; justify-content: center; padding: 0 8px; font-variant-numeric: tabular-nums; }
    &:hover:not(.active):not(:disabled) { background: var(--l-chip-hover); color: var(--l-text); }
    &:focus-visible { outline: 2px solid var(--l-accent); outline-offset: 1px; }

    &.active {
      background: var(--accent-soft);
      box-shadow: inset 0 0 0 1px var(--accent-border);
      color: var(--l-text);

      i { color: var(--accent); }
    }

    &:disabled { opacity: 0.35; cursor: not-allowed; }
  }

  &__hint {
    color: var(--l-text-faint);
    font-size: 12px;
  }

  &__view {
    align-items: flex-start;
    margin-left: 148px;
    width: auto;
  }

  &__subhint {
    margin: -6px 0 0 148px;
    color: var(--l-text-faint);
    font-size: 12px;
    line-height: 1.4;

    &.is-error { color: var(--l-level-error); }
  }

  .gx-switch.is-off { opacity: 0.45; cursor: default; }

  &__time {
    width: 104px;
    text-align: center;
    font-variant-numeric: tabular-nums;
  }

  &__dash { color: var(--l-text-faint); font-size: 13px; }

  /* Длинный путь режется слева: конец пути (сама папка) важнее начала. */
  &__path {
    flex: 1 1 200px;
    min-width: 0;
    overflow: hidden;
    color: var(--l-text-dim);
    direction: rtl;
    font-size: 12.5px;
    text-align: left;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  &__estimate {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }

  &__estimate-main {
    color: var(--l-text);
    font-size: 13.5px;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }

  &__warn {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    color: var(--l-level-warning);
    font-size: 12.5px;
    line-height: 1.4;

    i { flex: none; margin-top: 2px; font-size: 11px; }
    &.is-error { color: var(--l-level-error); }
  }

  &__fixes {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  &__job {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px 14px;
    border-radius: var(--l-radius-sm);
    background: var(--l-modal-bar);
    box-shadow: inset 0 0 0 1px var(--l-border);
  }

  &__job-head {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    font-size: 13px;
  }

  &__job-phase { color: var(--l-text); }
  &__job-percent { color: var(--l-text-dim); font-variant-numeric: tabular-nums; }

  &__bar {
    height: 6px;
    border-radius: 3px;
    background: rgba(255, 255, 255, 0.1);
    overflow: hidden;
  }

  &__fill {
    height: 100%;
    border-radius: 3px;
    background: var(--accent);
    transition: width var(--l-med);
  }

  &__outcome {
    display: flex;
    gap: 14px;
    padding: 14px 16px;
    border-radius: var(--l-radius-sm);
    background: var(--l-modal-bar);
    box-shadow: inset 0 0 0 1px var(--l-border);
  }

  &__outcome-icon {
    flex: none;
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    border-radius: 50%;
    font-size: 15px;

    &.is-ok { background: color-mix(in srgb, var(--l-level-ok) 18%, transparent); color: var(--l-level-ok); }
    &.is-error { background: color-mix(in srgb, var(--l-level-error) 18%, transparent); color: var(--l-level-error); }
  }

  &__outcome-body {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  &__outcome-title { font-size: 15px; font-weight: 600; }

  &__outcome-file {
    overflow: hidden;
    color: var(--l-text);
    font-size: 13px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  &__outcome-meta {
    color: var(--l-text-dim);
    font-size: 12.5px;
    line-height: 1.45;
  }

  &__steam {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    color: var(--l-text-faint);
    font-size: 12px;
    line-height: 1.4;

    i { font-size: 14px; }
  }

  &__actions {
    flex: none;
    display: flex;
    align-items: center;
    gap: 10px;
    padding-top: 14px;
    border-top: 1px solid var(--l-border);
  }

  &__spacer { flex: 1; }

  &__btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    min-height: 40px;
    padding: 0 18px;
    border: 1px solid var(--l-border-strong);
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.05);
    color: var(--l-text);
    font: inherit;
    font-size: 13.5px;
    font-weight: 600;
    cursor: pointer;
    transition: background var(--l-fast), transform var(--l-fast);

    i { font-size: 12px; }
    &:hover:not(:disabled) { background: rgba(255, 255, 255, 0.1); }
    &:active:not(:disabled) { transform: scale(0.97); }
    &:focus-visible { outline: 2px solid var(--l-accent); outline-offset: 2px; }
    &:disabled { opacity: 0.45; cursor: not-allowed; }

    &.primary {
      border-color: transparent;
      background: var(--accent);
      color: var(--accent-contrast-text);
      &:hover:not(:disabled) { background: var(--accent-hover); }
    }

    &.danger:hover { color: #ff8796; }
  }
}
</style>
