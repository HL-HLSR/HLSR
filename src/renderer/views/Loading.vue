<template>
  <div class="boot" :class="{ 'boot-colored': coloredBoot }">
    <Logo :absolute="true" />
    <div id="content">
      <LoadingIndicator size="55px" background="rgba(255, 255, 255, .2)" />


      <span class="loading-status">{{ statusText }}</span>


      <span v-if="phase === 'checking' && !done" class="loading-substatus">{{
        $t("#L_U_CHECKING")
      }}</span>


      <div v-if="phase === 'downloading'" class="boot-progress">
        <div class="boot-progress__bar">
          <div
            class="boot-progress__fill"
            :style="{ width: progressPercent + '%' }"
          ></div>
        </div>
        <div class="boot-progress__meta">
          <span>{{ progressPercent }}%</span>
          <span v-if="progressBytes">{{ progressBytes }}</span>
          <span v-if="progressSpeed">{{ progressSpeed }}</span>
        </div>
      </div>


      <div v-if="phase === 'blocked'" class="boot-blocked">
        <span class="boot-blocked__text">{{ $t("#L_B_BLOCKED") }}</span>
        <span v-if="bootError" class="boot-blocked__detail">{{ bootError }}</span>
        <button type="button" class="boot-blocked__retry" @click="retry">
          {{ $t("#L_B_RETRY") }}
        </button>
      </div>
    </div>
  </div>
</template>

<script>
import LoadingIndicator from "@/components/LoadingIndicator";
import Logo from "@/components/Logo";
import prefsStore from "@/components/Launcher/launcher-prefs";
import { formatNumber, formatSize } from "@/components/Launcher/file-format";

const { ipcRenderer } = window.hlsrElectron;

const MIN_SPLASH_MS = 1200;
const FAILSAFE_MS = 20000;

export default {
  name: "LoadingScreen",
  components: { Logo, LoadingIndicator },
  data() {
    return {
      splash: "",
      coloredBoot: prefsStore.load().coloredBoot !== false,
      phase: "checking",
      updateVersion: null,
      progress: null,
      bootError: null,
      mountedAt: Date.now(),
      failsafeTimer: null,
      done: false,
    };
  },
  computed: {
    statusText() {
      switch (this.phase) {
        case "downloading":
          return this.updateVersion
            ? this.$t("#L_B_DOWNLOADING", this.updateVersion)
            : this.$t("#L_B_DOWNLOADING", "");
        case "verifying":
          return this.$t("#L_B_VERIFYING");
        case "installing":
          return this.$t("#L_B_INSTALLING_UPDATE");
        case "blocked":
          return this.$t("#L_B_BLOCKED_TITLE");
        default:
          return this.splash;
      }
    },
    progressPercent() {
      if (!this.progress || !this.progress.percent) return 0;
      return Math.max(0, Math.min(100, Math.round(this.progress.percent)));
    },
    progressBytes() {
      if (!this.progress || !this.progress.total) return "";
      return `${formatSize(this.progress.transferred || 0, this.$t)} / ${formatSize(this.progress.total, this.$t)}`;
    },
    progressSpeed() {
      if (!this.progress || !this.progress.bytesPerSecond) return "";
      return this.$t("#L_U_SPEED", formatNumber(this.progress.bytesPerSecond / (1024 * 1024), 1, this.$t));
    },
  },
  methods: {
    onUpdaterState(e, status) {
      if (!status) return;

      if (status.version) this.updateVersion = status.version;

      switch (status.state) {
        case "downloading":
          this.phase = "downloading";
          this.progress = status.progress;
          break;
        case "verifying":
          this.phase = "verifying";
          break;
      }
    },
    async runGate(options) {
      let decision = null;

      try {
        decision = await ipcRenderer.invoke("boot-gate-run", options || {});
      } catch (err) {
        decision = { action: "proceed", reason: "ipc-error" };
      }

      this.applyDecision(decision);
    },
    applyDecision(decision) {
      if (!decision) return this.proceed();

      if (decision.action === "installing") {
        this.phase = "installing";
        return;
      }

      if (decision.action === "blocked") {
        this.phase = "blocked";
        this.bootError = decision.error || null;
        return;
      }

      this.proceed();
    },
    proceed() {
      if (this.done) return;
      this.done = true;

      let elapsed = Date.now() - this.mountedAt;
      let wait = Math.max(0, MIN_SPLASH_MS - elapsed);

      setTimeout(() => this.$router.push({ name: "launcher" }), wait);
    },
    retry() {
      this.phase = "checking";
      this.bootError = null;
      this.progress = null;
      this.runGate({ retry: true });
    },
  },
  mounted() {
    if (localStorage.getItem("wasLaunched"))
      this.splash = this.$t("#UI_WELCOME_BACK");
    else this.splash = this.$t("#UI_WELCOME_HLSR");

    localStorage.setItem("wasLaunched", true);

    ipcRenderer.on("updater-state", this.onUpdaterState);

    this.failsafeTimer = setTimeout(() => {
      if (this.phase === "checking") this.proceed();
    }, FAILSAFE_MS);

    this.runGate();
  },
  beforeUnmount() {
    ipcRenderer.removeListener("updater-state", this.onUpdaterState);
    if (this.failsafeTimer) clearTimeout(this.failsafeTimer);
  },
};
</script>

<style scoped>
#loadingscreen {
  display: flex;
  width: 100%;
}

/* Цветной сине-голубой фон из старого HLSR: тёмно-синяя основа с
   голубым радиальным свечением в центре. Реализовано CSS-градиентами —
   без внешних картинок и видео. prefers-reduced-motion отключает только
   анимации, сам фон остаётся. */
/* Корень экрана получает id="router" от <router-view> в App.vue и теряет
   собственный: селектор #wrapper не находил ничего, и голубой фон ниже не
   рисовался ни разу. Поэтому стили привязаны к классу. */
.boot.boot-colored {
  background:
    radial-gradient(
      ellipse 60% 50% at 50% 46%,
      rgba(47, 156, 236, 0.34) 0%,
      rgba(30, 98, 172, 0.18) 38%,
      rgba(12, 36, 74, 0) 72%
    ),
    linear-gradient(155deg, #0d1f3c 0%, #12294e 34%, #163356 55%, #0f2440 100%),
    #0c1e3a;
}

#content {
  width: 100%;
  display: flex;
  justify-content: center;
  align-items: center;
  flex-flow: column;
}

.loading-status {
  position: absolute;
  top: calc(50% + 130px);
  left: 50%;
  transform: translate(-50%, -50%);
  font-size: 1rem;
  font-weight: 500;
  letter-spacing: 0.1em;
  animation: animate 2s linear infinite;
  color: #fff;
  white-space: nowrap;
}

.loading-substatus {
  position: absolute;
  top: calc(50% + 162px);
  left: 50%;
  transform: translateX(-50%);
  font-size: 12px;
  letter-spacing: 0.08em;
  color: rgba(255, 255, 255, 0.35);
  white-space: nowrap;
}

.boot-progress {
  position: absolute;
  top: calc(50% + 170px);
  left: 50%;
  transform: translateX(-50%);
  width: min(420px, 70vw);
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.boot-progress__bar {
  height: 6px;
  border-radius: 3px;
  background: rgba(255, 255, 255, 0.12);
  overflow: hidden;
}

.boot-progress__fill {
  height: 100%;
  border-radius: 3px;
  background: #fff;
  transition: width 0.25s ease;
}

.boot-progress__meta {
  display: flex;
  justify-content: space-between;
  gap: 12px;
  color: rgba(255, 255, 255, 0.55);
  font-size: 12px;
  letter-spacing: 0.05em;
}

.boot-blocked {
  position: absolute;
  top: calc(50% + 170px);
  left: 50%;
  transform: translateX(-50%);
  width: min(520px, 80vw);
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
  text-align: center;
}

.boot-blocked__text {
  color: #ffb3ba;
  font-size: 13.5px;
  line-height: 1.5;
}

.boot-blocked__detail {
  color: rgba(255, 255, 255, 0.45);
  font-size: 12px;
  word-break: break-word;
}

.boot-blocked__retry {
  margin-top: 6px;
  padding: 9px 26px;
  border: 1px solid rgba(255, 255, 255, 0.3);
  border-radius: 8px;
  background: rgba(255, 255, 255, 0.08);
  color: #fff;
  font-family: inherit;
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
  transition: background 0.16s ease;
}

.boot-blocked__retry:hover {
  background: rgba(255, 255, 255, 0.16);
}

/* Подпись мягко «дышит», но остаётся читаемой: прежние 0.2–0.3 делали её
   почти невидимой на любом фоне. */
@keyframes animate {
  0% {
    opacity: 0.9;
  }

  50% {
    opacity: 0.6;
  }

  100% {
    opacity: 0.9;
  }
}
</style>
