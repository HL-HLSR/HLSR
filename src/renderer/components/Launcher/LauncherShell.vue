<template>
  <div
    class="launcher-root"
    :class="{ 'no-anim': !prefs.animations, 'rail-open': railOpen }"
    :data-theme="themeId"
    :style="rootStyle"
  >
    <!-- Преломление для темы Liquid Glass: launcher.css ссылается на эти
         фильтры из backdrop-filter. Крупный сглаженный шум сдвигает кадр
         под стеклом, как толща выпуклого стекла. Chromium понимает
         url() в backdrop-filter, а display:none у svg отключил бы фильтры. -->
    <svg v-if="themeId === 'liquid-glass'" class="lg-filters" aria-hidden="true" focusable="false">
      <filter id="lg-refract" x="0%" y="0%" width="100%" height="100%" color-interpolation-filters="sRGB">
        <feTurbulence type="fractalNoise" baseFrequency="0.002 0.006" numOctaves="1" seed="17" result="noise" />
        <feGaussianBlur in="noise" stdDeviation="3" result="map" />
        <feDisplacementMap in="SourceGraphic" in2="map" scale="70" xChannelSelector="R" yChannelSelector="G" />
      </filter>
      <filter id="lg-refract-strong" x="0%" y="0%" width="100%" height="100%" color-interpolation-filters="sRGB">
        <feTurbulence type="fractalNoise" baseFrequency="0.004 0.012" numOctaves="1" seed="17" result="noise" />
        <feGaussianBlur in="noise" stdDeviation="2" result="map" />
        <feDisplacementMap in="SourceGraphic" in2="map" scale="120" xChannelSelector="R" yChannelSelector="G" />
      </filter>
    </svg>
    <div class="launcher-frame" :key="langRev">

      <LauncherBackground
        :src="backgroundSrc"
        :foreground="foregroundSrc"
        :front-layout="foregroundLayout"
        :parallax="prefs.animations"
        :paused="overlayOpen"
        :tool-background="section === 'games' && activeTab === 'tools'"
      />

      <GameSidebar
        :games="games"
        :selectedId="selectedGameId"
        :online="onlineByGame"
        :activeTab="section === 'games' ? activeTab : ''"
        :toolItems="tools"
        :selectedToolId="selectedToolId"
        :tipsEnabled="!overlayOpen"
        :expanded="railOpen"
        @toggle="toggleRail"
        @select="onSelectGame"
        @select-tool="onSelectTool"
        @select-demo="onSelectDemo"
        @tool="onTool"
      />

      <HomePanel v-if="section === 'home' && official" @open-profile="openRunnerProfile($event || '')" @overlay="homeImageOpen = $event" />
      <OfficialStub v-else-if="section === 'home'" class="launcher-home-stub" text="#L_OFFICIAL_HOME" />

      <!-- keep-alive: несохранённые черновики «Персонализации»
           переживают переход на другие вкладки шапки. -->
      <keep-alive>
        <GameConfigsPanel
          v-if="section === 'configs'"
          ref="configsPanel"
          :games="configGames"
          :initialGameId="configsGameId"
          :openToken="configsOpenToken"
          :initialSection="configsSection"
          :importRequested="configsImportRequested"
          @import-started="configsImportRequested = false"
          @saved="onGameSettingsSaved"
          @open-games="section = 'games'"
          @help="onConfigsHelp"
        />
      </keep-alive>

      <!-- Переход между игрой и инструментом — тот же l-value, что при смене
           игры (launcher.css): старое и новое растворяются друг в друге на
           месте, в такт с фоном. Они стоят в одной ячейке сетки
           (launcher-stage, launcher-dock) и не сдвигают друг друга. -->
      <!-- «Демо» живёт в keep-alive: идущий экспорт переживает переход на
           другие вкладки, предпросмотр страница снимает сама. -->
      <keep-alive>
        <DemoPanel v-if="section === 'games' && activeTab === 'demo'" @notify="notify($event.text, $event.icon)" />
      </keep-alive>

      <div v-if="section === 'games' && activeTab !== 'demo'" class="launcher-main" :class="{ 'launcher-main--tool': activeTab === 'tools' }">
        <div class="launcher-stage">
        <transition name="l-value">
          <ToolPanel
            v-if="activeTab === 'tools'"
            :key="'tool-' + currentTool.toolId"
            :tool="currentTool"
            hero-only
          />
          <HeroPanel
            v-else
            key="games"
            :game="currentGame"
            :stats="currentStats"
            :delivery="currentDelivery"
          />
        </transition>
        </div>

        <div class="launcher-dock">
        <transition name="l-value">
        <ToolPanel
          v-if="activeTab === 'tools'"
          key="tool-controls"
          :tool="currentTool"
          controls-only
          @install="onToolInstall"
          @cancel="onToolCancel"
          @launch="onToolLaunch"
          @stop="onToolStop"
          @remove="onToolRemove"
          @help="onToolHelp"
          @folder="onToolFolder"
        />

          <DownloadBar
            v-else
            key="download-bar"
            :game="currentGame"
            :buildValue="currentBuild"
            :delivery="currentDelivery"
            @select-build="onSelectBuild"
            @play="onPlay"
            @stop="onStopGame"
            @install="onInstall"
            @install-episode="onInstallEpisode"
            @update="onApplyUpdate"
            @pause="onPauseDownload"
            @cancel="onCancelDownload"
            @resume="onResumeOperation"
            @retry="onResumeOperation"
            @repair="onMenuAction('repair')"
            @explain="unavailableOpen = true"
            @menu-action="onMenuAction"
          />
        </transition>
        </div>
      </div>

      <!-- Шапка: логотип и разделы слева, уведомления и аккаунт справа.
           Лежит поверх колонки значков и кадра игры; перетаскивает окно
           всё, кроме кнопок. -->
      <div class="launcher-topbar">
        <div class="launcher-topbar__brand" aria-hidden="true">
          <!-- Логотип из старой версии: точка растёт под курсором, двойной
               щелчок перекрашивает его на полторы секунды, тройной — пасхалка. -->
          <span
            class="launcher-topbar__logo-mark"
            :style="logoColor ? { '--logo-color': logoColor } : null"
            @click="onLogoClick"
            @dblclick="onLogoDoubleClick"
          >
            <span class="launcher-topbar__logo-dot"></span>
          </span>
          <span class="launcher-topbar__logo-text">HLSR</span>
          <span class="launcher-topbar__logo-beta">BETA</span>
        </div>

        <nav class="launcher-topbar__tabs" role="tablist">
          <button
            v-for="tab in sections"
            :key="tab.id"
            type="button"
            role="tab"
            v-grid-width
            class="launcher-topbar__tab"
            :data-tour="tab.id === 'configs' ? 'configs-tab' : null"
            :class="{ active: section === tab.id }"
            :aria-selected="section === tab.id ? 'true' : 'false'"
            @click="section = tab.id"
          >
            {{ $t(tab.title) }}
          </button>
        </nav>

        <!-- Поиск прижат к колокольчику; вкладкам слева остаётся место. -->
        <div v-if="official" class="launcher-topbar__search">
          <RunnerSearch @open-profile="openRunnerProfile" />
        </div>

        <div class="launcher-topbar__right">
          <SponsorsButton v-if="official" :active="sponsorsOpen" @open="sponsorsOpen = true" />
          <NotificationCenter
            :suspended="overlayOpen"
            @open-dump="onOpenCrashDump"
            @export-diagnostics="openDiagnostics"
            @open-discord="onOpenDiscord"
            @whats-new="onWhatsNew"
            @run-update="onRunUpdateAction"
          />
          <span class="launcher-topbar__sep" aria-hidden="true"></span>
          <AccountMenu v-if="official" @action="onAccountAction" />
          <OfficialStub v-else compact text="#L_OFFICIAL_LOGIN" />
          <WindowControls class="launcher-topbar__window" :fullscreenButton="prefs.fullscreenButton !== false" />
        </div>
      </div>

      <!-- Выбор папки ждёт, пока человек ответит про профиль: два
           блокирующих окна разом означали бы, что он отвечает на
           верхнее, не прочитав нижнее. -->
      <LibrarySetupModal
        v-if="librarySetupOpen"
        :initialPath="installPath"
        @confirmed="onLibraryConfirmed"
      />


      <transition name="l-modal">
        <LiveSplitLaunchModal
          v-if="liveSplitLaunch"
          :games="liveSplitLaunch.games"
          :defaultGame="liveSplitLaunch.defaultGame"
          :chosen="liveSplitSplits"
          @resolve="onLiveSplitLaunchResolved"
        />
      </transition>

      <transition name="l-modal">
        <ConfirmModal
          v-if="confirmRequest"
          :title="confirmRequest.title"
          :text="confirmRequest.text"
          :items="confirmRequest.items"
          :note="confirmRequest.note"
          :confirmLabel="confirmRequest.confirmLabel"
          :cancelLabel="confirmRequest.cancelLabel"
          :danger="confirmRequest.danger"
          :choices="confirmRequest.choices"
          :defaultChoice="confirmRequest.defaultChoice"
          @resolve="onConfirmResolved"
        />
      </transition>

      <!-- «Демо» без FFmpeg: до входа в раздел — предложение скачать; отказ оставляет где были. -->
      <transition name="l-modal">
        <ConfirmModal
          v-if="ffmpegGate"
          :title="$t('#L_DEMO_GATE_TITLE')"
          :text="$t('#L_DEMO_GATE_TEXT')"
          :items="ffmpegGateItems"
          :confirmLabel="$t('#L_DEMO_GATE_OK')"
          :cancelLabel="$t(ffmpegGate.installing ? '#L_DEMO_GATE_CANCEL' : '#L_DEMO_GATE_LATER')"
          :confirmDisabled="ffmpegGate.installing"
          @resolve="onFfmpegGate"
        >
          <div v-if="ffmpegGate.installing || ffmpegGate.error" class="ffgate" role="status">
            <span v-if="ffmpegGate.error" class="ffgate__error" role="alert">{{ ffmpegGate.error }}</span>
            <template v-if="ffmpegGate.installing">
              <span class="ffgate__head"><span>{{ $t("#L_DEMO_FFMPEG_INSTALLING") }}</span><span>{{ ffmpegGate.percent }}%</span></span>
              <span class="ffgate__bar"><span class="ffgate__fill" :style="{ width: ffmpegGate.percent + '%' }"></span></span>
            </template>
          </div>
        </ConfirmModal>
      </transition>

      <transition name="l-modal">
        <RepairModal v-if="repairRequest" :files="repairRequest.files" @resolve="onRepairResolved" />
      </transition>

      <UpdateCard
        v-if="!updateDismissed"
        :status="updateStatus"
        @download="startUpdateDownload"
        @cancel="cancelUpdateDownload"
        @install-now="confirmInstallNow"
        @install-on-quit="installOnQuit"
        @retry="checkForUpdates"
        @dismiss="updateDismissed = true"
        @whats-new="onWhatsNew({ target: 'launcher', version: updateStatus.version })"
      />

      <transition name="l-modal">
        <SettingsModal
          v-if="settingsOpen"
          :prefs="prefs"
          :installPath="installPath"
          :initialSection="settingsSection"
          @close="closeSettings"
          @theme="setTheme"
          @apply="applyPrefs"
          @reset="notify($t('#L_SETTINGS_RESET'), 'fas fa-undo')"
          @stub="notify($t('#L_STUB_FEATURE'), 'fas fa-info-circle')"
        />
      </transition>

      <transition name="l-pop">
        <FriendsPanel v-if="friendsOpen" @close="friendsOpen = false" @open-profile="openRunnerProfile" />
      </transition>

      <!-- Ключ — чей профиль: при переходе к другому окно открывается
           заново, а не смешивает данные двух профилей. -->
      <transition name="l-modal" mode="out-in">
        <ProfileModal
          v-if="profileOpen"
          :key="(profileTarget || 'self') + profileSection"
          :targetSteamId="profileTarget"
          :initialSection="profileSection"
          :hideSteamId="prefs.hideSteamId === true"
          @close="onCloseProfile"
          @toggle-id="onToggleSteamId"
          @server-hide-id="onServerHideSteamId"
          @notify="notify($t($event), 'fas fa-check')"
          @open-sponsors="onOpenSponsors"
        />
      </transition>

      <transition name="l-modal">
        <ConfigSetupModal
          v-if="configSetup"
          @choose="onConfigSetupChoose"
          @close="onConfigSetupChoose(null)"
        />
      </transition>

      <ConfigTour
        v-if="configTour"
        :steps="configTour.steps"
        :section="section"
        @section="section = $event"
        @page-section="onConfigTourPageSection"
        @done="onConfigTourDone"
      />

      <transition name="l-modal">
        <UnavailableModal
          v-if="unavailableOpen"
          :reason="unavailableReason"
          :fileRepair="!!(currentDelivery && currentDelivery.fileRepair)"
          @close="unavailableOpen = false"
          @action="onUnavailableAction"
        />
      </transition>


      <transition name="l-modal">
        <GameCrashModal
          v-if="crashReport"
          :report="crashReport"
          @close="crashReport = null"
          @open-folder="onOpenCrashDump"
          @open-discord="onOpenDiscord"
          @export-diagnostics="crashReport = null; openDiagnostics()"
        />
      </transition>

      <transition name="l-modal">
        <SupportThanksModal v-if="thanksNotice" :notice="thanksNotice" @close="thanksNotice = null" />
      </transition>

      <transition name="l-modal">
        <SponsorsModal v-if="sponsorsOpen" @close="sponsorsOpen = false" @open-profile="sponsorsOpen = false; openRunnerProfile($event)" />
      </transition>

      <transition name="l-modal">
        <ChangelogModal
          v-if="changelogOpen"
          :initialVersion="changelogVersion"
          :extraRelease="changelogExtra"
          @close="closeChangelog"
        />
      </transition>


      <transition name="l-modal">
        <GameNotesModal
          v-if="gameNotesOpen"
          :gameTitle="currentGameFullTitle"
          :version="gameNotesVersion"
          :notes="gameNotesData"
          @close="gameNotesOpen = false"
        />
      </transition>

      <transition name="l-modal">
        <WelcomeModal
          v-if="welcomeOpen"
          :version="updateStatus.currentVersion || ''"
          @signed-in="onWelcomeSignedIn"
          @later="onWelcomeLater"
        />
      </transition>

      <ToastContainer :toasts="toasts" @dismiss="dismissToast" />

      <!-- Пасхалки: плашка HESOYAM под кнопкой «Закрыть» и пони из
           «Назад в прошлое!». Поверх всего и прозрачны для мыши. -->
      <transition name="egg-cheat">
        <div v-if="cheatShown" class="egg-cheat" aria-hidden="true">{{ $t("#L_CHEAT_ACTIVATED") }}</div>
      </transition>
      <transition name="egg-pony">
        <img v-if="ponyShown" class="egg-pony" src="@/assets/launcher/pony.webp" alt="" aria-hidden="true" />
      </transition>
    </div>
  </div>
</template>

<script>
import GameSidebar from "./GameSidebar";
import HeroPanel from "./HeroPanel";
import ToolPanel from "./ToolPanel";
import DemoPanel from "./DemoPanel";
import ProfileModal from "./ProfileModal";
import RunnerSearch from "./RunnerSearch";
import LauncherBackground from "./LauncherBackground";
import DownloadBar from "./DownloadBar";
import NotificationCenter from "./NotificationCenter";
import SponsorsButton from "./SponsorsButton";
import AccountMenu from "./AccountMenu";
import HomePanel from "./HomePanel";
import OfficialStub from "./OfficialStub";
import { OFFICIAL } from "../../official";
import { themeById, themeVars } from "./launcher-themes";
import gridWidth from "./grid-width";
import UnavailableModal from "./UnavailableModal";
import ConfigSetupModal from "./ConfigSetupModal";
import ConfigTour from "./ConfigTour";
import WelcomeModal from "./WelcomeModal";
import WindowControls from "./WindowControls";
import SettingsModal from "./SettingsModal";
import GameConfigsPanel from "./GameConfigsPanel";
import ChangelogModal from "./ChangelogModal";
import SponsorsModal from "./SponsorsModal";
import SupportThanksModal from "./SupportThanksModal";
import GameCrashModal from "./GameCrashModal";
import GameNotesModal from "./GameNotesModal";
import ToastContainer from "./ToastContainer";
import UpdateCard from "./UpdateCard";
import LibrarySetupModal from "./LibrarySetupModal";
import FriendsPanel from "./FriendsPanel";
import ConfirmModal from "./ConfirmModal";
import RepairModal from "./RepairModal";
import LiveSplitLaunchModal from "./LiveSplitLaunchModal";
import { formatSize } from "./file-format";
import { errorMessage as demoErrorMessage } from "./demo-tool";

import launcherData, { licenseGameOf, DELIVERY_IDS, GHOSTING_GAME_ID } from "./launcher-data";
const configTourSteps = require("./config-tour");
const bindPresets = require("../../../shared/bind-presets");
import prefsStore, { ACCENT, DEFAULT_INSTALL_PATH } from "./launcher-prefs";
import availability from "../../../shared/game-availability";
import progressStates from "../../../shared/progress-states";
import GameList from "../../../games.config";
import Store from "@/utils/Store.js";
import StoreDefaults from "@/utils/StoreDefaults.js";
import settingsStore from "@/utils/settingsStore";

import "@/assets/css/launcher.css";

const { ipcRenderer } = window.hlsrElectron;

const library = new Store({
  configName: "library",
  defaults: StoreDefaults.library,
});

const settings = settingsStore;

// Сплиты LiveSplit из настроек: { gameId: id набора }. До 0.4.3 там лежал
// один набор на весь LiveSplit ({ livesplit: "chapters" | "maps" }), он
// относился к Ghosting.
function liveSplitChoicesOf(prefs) {
  const saved = Object.assign({}, prefs && prefs.toolSplits);
  if (typeof saved.livesplit === "string") {
    if (!saved["ghosting-renewed"]) saved["ghosting-renewed"] = saved.livesplit;
    delete saved.livesplit;
  }
  return saved;
}

const SECRET_SEQUENCE = ["ArrowUp", "ArrowUp", "ArrowDown", "ArrowDown", "ArrowLeft", "ArrowRight", "ArrowLeft", "ArrowRight", "KeyB", "KeyA"];
// Чит из GTA: San Andreas, набирается в главном окне (секретное достижение hesoyam).
const CHEAT_SEQUENCE = ["KeyH", "KeyE", "KeyS", "KeyO", "KeyY", "KeyA", "KeyM"];
const CHEAT_SOUND = require("@/assets/launcher/gta-ui.mp3");

const fallbackHours = {
  "70": "36,5",
  "50": "8,2",
  "130": "0",
  "220": "124",
};

export default {
  name: "LauncherShell",
  directives: { gridWidth },
  components: {
    LiveSplitLaunchModal,
    GameSidebar,
    HeroPanel,
    ToolPanel,
    DemoPanel,
    ProfileModal,
    RunnerSearch,
    LauncherBackground,
    DownloadBar,
    NotificationCenter,
    SponsorsButton,
    AccountMenu,
    HomePanel,
    OfficialStub,
    UnavailableModal,
    ConfigSetupModal,
    ConfigTour,
    WelcomeModal,
    WindowControls,
    SettingsModal,
    GameConfigsPanel,
    ChangelogModal,
    SponsorsModal,
    SupportThanksModal,
    GameCrashModal,
    GameNotesModal,
    ToastContainer,
    UpdateCard,
    LibrarySetupModal,
    FriendsPanel,
    ConfirmModal,
    RepairModal,
  },
  data() {
    return {
      selectedGameId: availability.DEFAULT_GAME_ID,
      profileOpen: false,
      profileSection: "profile",
      // Номер запроса «Скрыть SteamID» на сервер, 0 — запросов нет.
      hideIdSequence: 0,
      homeImageOpen: false,
      profileTarget: "",
      // Раздел в шапке. Внутри «Игр» activeTab ещё различает игру и
      // инструмент: их выбирают из одной колонки значков.
      section: "games",
      sections: [
        { id: "home", title: "#L_TAB_HOME" },
        { id: "games", title: "#L_TAB_GAMES" },
        { id: "configs", title: "#L_TAB_CONFIGS" },
      ],
      activeTab: "games",
      tools: [],
      selectedToolId: "",
      logoColor: "",
      keySequence: [],
      // Пасхалки: плашка «Чит активирован» и пони у нижнего края.
      cheatShown: false,
      ponyShown: false,
      // Сплиты LiveSplit по играм: { gameId: id набора }.
      liveSplitSplits: liveSplitChoicesOf(prefsStore.load()),
      // Открытое окно запуска LiveSplit: { games, defaultGame, resolve }.
      liveSplitLaunch: null,
      selectedBuilds: {},
      settingsOpen: false,
      settingsSection: "general",
      // Игра, на которой открыта страница «Персонализация»; пусто — вкладку ещё не открывали.
      configsGameId: "",
      configsOpenToken: 0,
      // Раздел, на котором шестерёнка, опрос и обучение открывают игру; по вкладке шапки — первый.
      configsSection: "",
      // Опрос первого запуска просит начать импорт на странице «Персонализация»; панель принимает запрос один раз.
      configsImportRequested: false,
      // Идёт проверка или открыт опрос: повторный «Играть» не начинает второй.
      configSetupBusy: false,
      // Опрос первого запуска: { resolve } пока окно открыто.
      configSetup: null,
      // «С нуля» в опросе: обучение страницы «Персонализация» (задача 11) показывается и сбрасывает флаг.
      configTourRequested: false,
      // Идущее обучение: { steps } или null.
      configTour: null,
      // Проверка окна приветствия закончилась: до неё нельзя понять, свободна ли оболочка.
      welcomeChecked: false,
      changelogOpen: false,
      sponsorsOpen: false,
      official: OFFICIAL,
      thanksNotice: null,
      // Окно о падении игры (game-crash-report), null — закрыто.
      crashReport: null,
      changelogVersion: null,
      changelogExtra: null,
      gameNotesOpen: false,
      gameNotesVersion: "",
      gameNotesData: null,
      unavailableOpen: false,
      welcomeOpen: false,
      stats: {},
      toasts: [],
      railPeek: false,
      toastSeq: 0,
      langRev: 0,
      prefs: prefsStore.load(),
      installPath: library.get("libraryPath") || DEFAULT_INSTALL_PATH,
      updateStatus: {
        state: "idle",
        enabled: false,
        currentVersion: ipcRenderer.sendSync("app-version"),
        version: null,
        releaseNotes: null,
        progress: null,
        error: null,
      },
      updateDismissed: false,
      librarySetupOpen: false,
      friendsOpen: false,
      confirmRequest: null,
      // Окно FFmpeg перед входом в «Демо»: { status, installing, percent, error, resolve }.
      ffmpegGate: null,
      repairRequest: null,
      autoUpdateStarted: {},
      downloadSettingsReady: false,
      delivery: {},
      onlineSnapshot: { status: "unknown", launcherOnline: null, games: {} },
    };
  },
  computed: {
    rpcActivity() {
      if (this.section === "home") return "home";
      if (this.section === "configs") return "personalization";
      if (this.section === "games" && this.activeTab === "demo") return "demo";
      if (this.section === "games" && this.activeTab === "tools") return "livesplit";
      return "games";
    },
    // Темам, которым мало переменных (Liquid Glass), launcher.css дописывает
    // правила по [data-theme].
    themeId() {
      return themeById(this.prefs.launcherTheme).id;
    },
    overlayOpen() {
      return this.homeImageOpen || this.settingsOpen || this.profileOpen || this.changelogOpen || this.sponsorsOpen || !!this.thanksNotice ||
        this.gameNotesOpen || this.friendsOpen || this.unavailableOpen || this.librarySetupOpen || this.welcomeOpen || !!this.confirmRequest || !!this.repairRequest || !!this.configTour || !!this.ffmpegGate;
    },
    // Что и откуда скачается: источник манифеста (demo-ffmpeg-status: origins), сборка, размер.
    ffmpegGateItems() {
      const status = this.ffmpegGate.status;
      const origins = (status.origins || []).filter(Boolean);
      const items = [];
      if (origins[0])
        items.push(origins[0].release ? this.$t("#L_DEMO_GATE_FROM", origins[0].site, origins[0].release) : this.$t("#L_DEMO_GATE_FROM_SITE", origins[0].site));
      if (origins[1]) items.push(this.$t("#L_DEMO_GATE_SPARE", origins[1].site));
      items.push(this.$t("#L_DEMO_GATE_BUILD", status.pinnedVersion || "", status.build || ""));
      items.push(this.$t("#L_DEMO_GATE_SIZE", this.formatBytes(status.sizeBytes)));
      return items;
    },
    // Обучение после обновления пора показать: игра установлена, в оболочке нет других окон.
    // Без установленной игры оно ждёт первой, configTourSeen не ставится.
    configTourDue() {
      return !this.prefs.configTourSeen && this.welcomeChecked && this.configGames.length > 0 &&
        !this.configSetup && !this.configSetupBusy && !this.overlayOpen;
    },
    showsGame() {
      return this.section === "games" && this.activeTab === "games";
    },
    backgroundSrc() {
      if (!this.showsGame) return null;
      return (this.currentEpisodeId && this.currentGame.episodesHero) || this.currentGame.hero;
    },
    foregroundSrc() {
      // У эпизодов свой фон без героев Ghosting поверх.
      if (!this.showsGame || (this.currentEpisodeId && this.currentGame.episodesHero)) return null;
      return this.currentGame.heroFront || null;
    },
    foregroundLayout() {
      return this.showsGame ? this.currentGame.heroFrontLayout || null : null;
    },
    currentTool() {
      return (
        this.tools.find((tool) => tool.toolId === this.selectedToolId) ||
        this.tools[0] || { toolId: "", title: "", state: "unknown", icon: "fas fa-toolbox" }
      );
    },
    onlineByGame() {
      let result = {};
      let fresh = this.onlineSnapshot && this.onlineSnapshot.status === "ok";
      let games = (this.onlineSnapshot && this.onlineSnapshot.games) || {};

      for (let launcherId of Object.keys(launcherData.DELIVERY_IDS)) {
        const ids = launcherId === "220" ? ["hl2-2707", "hl2-4260", "hl2-5288"] : [launcherData.DELIVERY_IDS[launcherId]];
        const entries = ids.map((id) => games[id]);
        const known = fresh && entries.every((entry) => entry && typeof entry.online === "number");
        result[launcherId] = known ? { online: entries.reduce((sum, entry) => sum + entry.online, 0), unknown: false }
          : { online: null, unknown: true };
      }

      return result;
    },

    games() {
      let mapped = GameList.map((game) => {
        let art = launcherData.gameArt[game.id] || {};
        let demo = launcherData.demoState[game.id] || {};
        let title = this.splitTitle(game.name);

        return {
          id: game.id,
          name: game.name,
          shortName: launcherData.shortNames[game.id] || game.name,
          heroTitle: title.main,
          heroSubtitle: launcherData.heroSubtitles[game.id] || title.sub,
          hero: art.hero,
          episodesHero: art.episodesHero || null,
          heroFront: art.heroFront || null,
          heroFrontLayout: art.heroFrontLayout || null,
          logo: art.logo,
          accent: art.accent,
          accentSoft: art.accentSoft,
          demo: demo,
          playtimeHours: this.getPlayTime(game.id),
          available: availability.isGameAvailable(game.id),
        };
      });

      return availability.orderGames(mapped);
    },
    railOpen() {
      return this.section === "games" ? this.prefs.railExpanded === true : this.railPeek;
    },
    currentGame() {
      return (
        this.games.find((t) => t.id === this.selectedGameId) || this.games[0]
      );
    },
    currentBuild() {
      const builds = this.currentGame.demo.builds;
      // Вариант Ghosting (эпизод) помнит лаунчер; сохранённый эпизод, которого нет в списке (Linux), — к первому.
      if (this.currentGame.id === GHOSTING_GAME_ID)
        return builds.indexOf(this.prefs.ghostingBuild) !== -1 ? this.prefs.ghostingBuild : builds[0];
      return (
        this.selectedBuilds[this.selectedGameId] ||
        builds[0]
      );
    },
    // Выбранный эпизод Ghosting ("ep1" | "ep2") или null.
    currentEpisodeId() {
      return (this.currentGame.demo.episodes || {})[this.currentBuild] || null;
    },
    // Карточка Ghosting с эпизодами на экране: тогда нужен их размер из манифестов.
    episodeCardShown() {
      return this.showsGame && Object.keys(this.currentGame.demo.episodes || {}).length > 0;
    },
    currentDelivery() {
      return this.delivery[this.selectedGameId] || null;
    },
    deliveryBusy() {
      return Object.keys(this.delivery).some((id) => {
        const status = this.delivery[id];
        return Boolean(status) && progressStates.isBusy(status.state);
      });
    },
    currentLanguage() {
      // LocalizationHelper — обычный объект; ревизия сбрасывает кэш Vue
      // после каждого переключения, включая возврат к исходному языку.
      void this.langRev;
      let keys = Object.keys(this.$localization.locales);
      return keys[this.$localization.lang] || "en";
    },
    unavailableReason() {
      const button = this.currentDelivery ? this.currentDelivery.mainButton : null;
      return (button && button.reason) || "UNKNOWN_UNAVAILABLE";
    },
    currentStats() {
      return this.stats[this.selectedGameId] || null;
    },
    currentGameFullTitle() {
      let game = this.currentGame;
      if (!game) return "";
      return game.heroSubtitle
        ? `${game.heroTitle}: ${game.heroSubtitle}`
        : game.heroTitle;
    },
    // Установленные игры для страницы «Персонализация», в порядке библиотеки.
    configGames() {
      return this.games
        .filter((game) => game.available && this.delivery[game.id] && this.delivery[game.id].installedVersion)
        .map((game) => ({
          id: game.id,
          name: game.shortName,
          title: game.heroSubtitle ? `${game.heroTitle}: ${game.heroSubtitle}` : game.heroTitle,
        }));
    },
    rootStyle() {
      const palette = ACCENT;

      let appImage = `url(${require("@/assets/gradient.jpg")})`;

      return {
        "--accent": palette.accent,
        "--accent-rgb": palette.rgb,
        "--accent-hover": palette.hover,
        "--accent-pressed": palette.pressed,
        "--accent-soft": palette.soft,
        "--accent-border": palette.border,
        "--accent-focus": palette.focus,
        "--accent-contrast-text": palette.contrastText,
        "--accent-color": palette.accent,
        "--l-app-image": appImage,
        ...themeVars(this.prefs.launcherTheme, this.prefs.paperAccent),
      };
    },
  },
  watch: {
    // Discord Rich Presence: где игрок сейчас (discord-rpc/presence-model.js, ACTIVITY_KINDS).
    rpcActivity: {
      immediate: true,
      handler(activity) {
        ipcRenderer.send("discord-rpc-activity", activity);
      },
    },
    // Раскрытая на «Главной» или в «Персонализации» колонка закрывается
    // при смене вкладки.
    section(id) {
      this.railPeek = false;
      // Первый заход на «Персонализацию» по вкладке — на выбранной игре.
      if (id === "configs" && !this.configsGameId) this.configsGameId = this.selectedGameId;
    },
    configTourDue(due) {
      if (!due) return;
      // Небольшая задержка: за окном приветствия или опросом сразу идёт другое окно.
      clearTimeout(this.configTourTimer);
      this.configTourTimer = setTimeout(() => {
        if (!this.configTourDue) return;
        const game = this.configGames.find((item) => item.id === this.selectedGameId) || this.configGames[0];
        // Первый шаг подсвечивает вкладку в том разделе, где игрок сейчас; на «Персонализацию» тур уходит сам.
        const back = this.section;
        this.configsGameId = game.id;
        this.configsSection = "configs";
        this.configsOpenToken += 1;
        this.startConfigTour(game.id, back);
      }, 1200);
    },
    // «С нуля» в опросе: страница «Персонализация» уже открыта на игре.
    configTourRequested(on) {
      if (!on) return;
      this.configTourRequested = false;
      this.startConfigTour(this.configsGameId);
    },
    changelogOpen(open) {
      if (open) this.unlockAchievement("chronicler");
    },
    episodeCardShown: {
      immediate: true,
      handler(shown) {
        if (shown) this.refreshEpisodes();
      },
    },
  },
  methods: {
    // Пасхалка из старой версии: двойной щелчок по логотипу перекрашивает
    // его в случайный цвет.
    onLogoDoubleClick() {
      const channel = () => Math.floor(Math.random() * 250);
      this.logoColor = `rgb(${channel()}, ${channel()}, ${channel()})`;
      clearTimeout(this.logoTimer);
      this.logoTimer = setTimeout(() => { this.logoColor = ""; }, 1500);
    },
    // Три щелчка подряд, каждый не позже 650 мс после предыдущего (двойной
    // до третьего не дотягивает): секретное достижение «Назад в прошлое!» и
    // пони снизу. В старой версии тут открывалась тема My Little Pony.
    onLogoClick(event) {
      this.logoClicks = event.timeStamp - this.logoClickAt <= 650 ? this.logoClicks + 1 : 1;
      this.logoClickAt = event.timeStamp;
      if (this.logoClicks < 3) return;
      this.logoClicks = 0;
      this.unlockBackToPast();
      this.showPony();
    },
    // Новая тема перетекает из старой: браузер снимает кадр до и после
    // смены и плавно сменяет один другим (View Transitions, launcher.css).
    // Без анимаций интерфейса — сразу.
    // accent — вариант акцента темы «Бумага»; без него остаётся прежний.
    setTheme(id, accent) {
      const next = themeById(id).id;
      const paperAccent = accent || this.prefs.paperAccent;
      if (next === this.prefs.launcherTheme && paperAccent === this.prefs.paperAccent) return;
      const apply = () => {
        this.prefs = Object.assign({}, this.prefs, { launcherTheme: next, paperAccent });
        prefsStore.save(this.prefs);
      };
      const still = !this.prefs.animations || window.matchMedia("(prefers-reduced-motion: reduce)").matches;
      if (still || typeof document.startViewTransition !== "function") return apply();
      document.startViewTransition(async () => {
        apply();
        await this.$nextTick();
      });
    },
    unlockBackToPast() {
      this.unlockAchievement("back-to-past");
    },
    // Пони выезжает снизу (0,45 с), стоит секунду и падает обратно
    // (переход egg-pony). Пока он на экране, новый не начинается.
    showPony() {
      if (this.ponyShown) return;
      this.ponyShown = true;
      this.ponyTimer = setTimeout(() => { this.ponyShown = false; }, 1450);
    },
    // HESOYAM: плашка «Чит активирован» со звуком из GTA. Повторный ввод
    // показывает её заново, достижение main выдаёт один раз.
    showCheat() {
      this.cheatShown = true;
      clearTimeout(this.cheatTimer);
      this.cheatTimer = setTimeout(() => { this.cheatShown = false; }, 2600);
      if (!this.cheatSound) this.cheatSound = new Audio(CHEAT_SOUND);
      this.cheatSound.volume = 0.35;
      this.cheatSound.currentTime = 0;
      this.cheatSound.play().catch(() => {});
      this.unlockAchievement("hesoyam");
    },
    // Поздравление приходит событием achievement-unlocked — одно на все
    // достижения, открытые и интерфейсом, и main.
    unlockAchievement(id) {
      ipcRenderer.invoke("runner-secret-unlock", id).catch(() => {});
    },
    onAchievementUnlocked(e, item) {
      if (!item || typeof item.title !== "string" || item.title.indexOf("#L_ACH_") !== 0) return;
      this.notify(this.$t("#L_ACH_UNLOCKED", this.$t(item.title)), typeof item.icon === "string" ? item.icon : "fas fa-trophy");
    },
    // «Спасибо за поддержку»: одно окно на все пришедшие оплаты (первая — самая свежая).
    onSupportThanks(e, notice) {
      if (!notice || typeof notice.id !== "string" || (notice.kind !== "subscription" && notice.kind !== "donation")) return;
      if (!this.thanksNotice) this.thanksNotice = { kind: notice.kind, until: Number.isSafeInteger(notice.until) ? notice.until : null, styles: notice.styles === true };
    },
    // Последовательность клавиш в главном окне, пока не открыто ни одно окно
    // поверх (в том числе выбор игры LiveSplit, опрос конфига и окно о
    // падении — они не в overlayOpen) и фокус не в поле ввода. Буквы — по физическим клавишам, чтобы
    // работало и в русской раскладке.
    onGlobalKeydown(event) {
      const target = event.target;
      if (this.overlayOpen || this.liveSplitLaunch || this.configSetup || this.crashReport || (target && (target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName)))) {
        this.keySequence = [];
        return;
      }
      this.keySequence = this.keySequence.concat(event.code).slice(-SECRET_SEQUENCE.length);
      if (this.keySequence.join(" ") === SECRET_SEQUENCE.join(" ")) {
        this.keySequence = [];
        this.unlockAchievement("wake-up");
      } else if (this.keySequence.slice(-CHEAT_SEQUENCE.length).join(" ") === CHEAT_SEQUENCE.join(" ")) {
        this.keySequence = [];
        this.showCheat();
      }
    },
    splitTitle(name) {
      let parts = name.split(":");
      return {
        main: parts[0].trim(),
        sub: parts[1] ? parts[1].trim() : null,
      };
    },
    getPlayTime(gameId) {
      let config = library.get("config") || {};

      if (config[gameId] && config[gameId].inGameTime)
        return (config[gameId].inGameTime / 3600000)
          .toFixed(1)
          .replace(".", ",");

      return fallbackHours[gameId] || "0";
    },
    onSelectGame(id) {
      if (!availability.isGameAvailable(id)) {
        this.notify(this.$t("#L_GAME_COMING_SOON"), "fas fa-clock");
        return;
      }

      this.section = "games";
      this.activeTab = "games";
      this.selectedGameId = id;
      ipcRenderer.invoke("game-delivery-selected", id);
      this.loadStats(id);
    },
    async onSelectBuild(build) {
      // У Ghosting одна сборка, эпизоды — её дополнения: main сборок для них не знает.
      if (this.selectedGameId === GHOSTING_GAME_ID) {
        this.prefs = Object.assign({}, this.prefs, { ghostingBuild: build });
        prefsStore.save(this.prefs);
        return;
      }

      const result = await ipcRenderer.invoke("game-select-build", this.selectedGameId, build);

      if (result && result.status === "unknown-build") return;

      if (result && result.status === "busy") {
        this.notify(this.$t("#L_D_BUILD_LOCKED"), "fas fa-lock");
        return;
      }

      this.selectedBuilds[this.selectedGameId] = build;

      if (result && result.delivery) this.onDeliveryStatus(null, result.delivery);
    },

    async onPlay() {
      if (!this.currentDelivery) {
        this.notify(this.$t("#L_STUB_PLAY"), "fas fa-play");
        return;
      }

      let gameId = this.currentDelivery.gameId;

      if (await this.offerConfigSetup(gameId)) return;
      if (await this.offerTimer()) return;

      // «Играть» с выбранным эпизодом есть, только когда он скачан (episode-button.js).
      const episode = this.currentEpisodeId;
      let result = await ipcRenderer.invoke("game-process-launch", gameId, episode ? { episode } : {});

      if (!result || result.status === "error") {
        this.notifyDeliveryError(result);
        this.offerLaunchFix(result && result.error ? result.error.code : null, result && result.error);
        return;
      }

      if (result.status === "already-running")
        this.notify(this.$t("#L_P_ALREADY_RUNNING"), "fas fa-info-circle");

      this.notifyDependencies(result.dependencies);
      if (result.packs && result.packs.status === "failed")
        this.notify(this.$t("#L_P_PACKS_FAILED"), "fas fa-exclamation-triangle");
    },

    // Перед запуском HL1 лаунчер обновляет Bunnymod XT и инжектор с GitHub.
    notifyDependencies(dependencies) {
      for (const dep of dependencies || []) {
        if (dep.status === "updated")
          this.notify(this.$t("#L_P_DEP_UPDATED", dep.title, dep.to), "fas fa-sync-alt");
        else if (dep.status === "failed")
          this.notify(this.$t("#L_P_DEP_FAILED", dep.title), "fas fa-exclamation-triangle");
      }
    },

    async offerLaunchFix(code, error) {
      if (code === "LIBRARY_PATH_CASE_CONFLICT") {
        const details = (error && error.details) || {};
        const copy = await this.askConfirm({
          title: this.$t("#L_C_CASE_CONFLICT_TITLE"),
          text: this.$t("#L_C_CASE_CONFLICT_TEXT", details.folder || "", details.twin || ""),
          confirmLabel: this.$t("#L_C_COPY_PATH"),
        });

        if (copy && details.twin) {
          await window.hlsrElectron.clipboard.writeText(details.twin);
          this.notify(this.$t("#L_C_PATH_COPIED"), "fas fa-clipboard-check");
        }
        return;
      }

      if (code === "STEAM_APP_NOT_OWNED") {
        const license = licenseGameOf(this.selectedGameId);
        const open = await this.askConfirm({
          title: this.$t("#L_C_STEAM_LICENSE_TITLE"),
          text: this.$t("#L_C_STEAM_LICENSE_TEXT", license.title),
          confirmLabel: this.$t("#L_C_STEAM_STORE"),
        });

        if (open) ipcRenderer.invoke("open-external-link", license.storeLink);
        return;
      }

      if (code === "STEAM_ELEVATED") {
        const restart = await this.askConfirm({
          title: this.$t("#L_UA_STEAM_ELEVATED"),
          text: this.$t("#L_UA_STEAM_ELEVATED_T"),
          confirmLabel: this.$t("#L_C_RESTART_ELEVATED"),
        });

        if (restart) ipcRenderer.invoke("game-launch-restart-elevated");
        return;
      }

      if (code === "STEAM_NOT_RUNNING" || code === "STEAM_PROCESS_MISMATCH") {
        const open = await this.askConfirm({
          title: this.$t("#L_C_STEAM_TITLE"),
          text: this.$t("#L_P_OPEN_STEAM_PROMPT"),
          confirmLabel: this.$t("#L_UA_ACT_OPEN_STEAM"),
        });

        if (open) ipcRenderer.invoke("game-launch-open-steam");
        return;
      }

      if (code === "SOURCE_SDK_NOT_INSTALLED") {
        const open = await this.askConfirm({
          title: this.$t("#L_C_SDK_TITLE"),
          text: this.$t("#L_P_INSTALL_SDK_PROMPT"),
          confirmLabel: this.$t("#L_C_OPEN"),
        });

        if (open) ipcRenderer.invoke("game-launch-install-sdk");
      }
    },

    async onStopGame() {
      let gameId = this.currentDelivery ? this.currentDelivery.gameId : null;
      if (!gameId) return;

      let result = await ipcRenderer.invoke("game-process-stop", gameId);

      if (!result || result.status === "error") {
        this.notifyDeliveryError(result);
        return;
      }

      if (result.status !== "timeout") return;

      const force = await this.askConfirm({
        title: this.$t("#L_C_STOP_TITLE"),
        text: this.$t("#L_P_STOP_TIMEOUT"),
        confirmLabel: this.$t("#L_C_STOP_FORCE"),
        danger: true,
      });

      if (!force) return;

      let forced = await ipcRenderer.invoke("game-process-stop", gameId, { force: true });

      if (!forced || forced.status === "error")
        this.notify(this.$t("#L_P_STOP_FAILED"), "fas fa-exclamation-triangle");
    },

    onProcessRunning() {
      this.notify(this.$t("#L_P_RUNNING"), "fas fa-play");
      if (new Date().getHours() === 3 && this.prefs.launcherTheme === "midnight") this.unlockAchievement("night-shift");
      this.loadStats(this.selectedGameId);
    },
    onProcessExited() {
      this.notify(this.$t("#L_P_EXITED"), "fas fa-check");
      this.loadStats(this.selectedGameId);
    },
    onProcessCrashed(e, payload) {
      // Настоящее падение показывает окно о краше (game-crash-report) после
      // поиска дампа; здесь — только остановка, которую попросил игрок.
      if (!payload || !payload.requested) return;

      let code = payload && payload.exitCode != null ? String(payload.exitCode) : null;

      this.notify(
        code ? this.$t("#L_P_CRASHED_CODE", code) : this.$t("#L_P_CRASHED"),
        "fas fa-exclamation-triangle"
      );

      setTimeout(() => this.notify(this.$t("#L_P_CRASH_HINT"), "fas fa-clipboard-check"), 600);
    },

    onDeliveryStatus(e, status) {
      if (!status || !status.launcherId) return;
      this.delivery[String(status.launcherId)] = status;
      this.maybeAutoUpdate(status);
    },
    maybeAutoUpdate(status) {
      if (!this.downloadSettingsReady) return;
      const update = status.update;
      if (!update || !update.updateAvailable || update.stale || update.unknown) return;
      if (!status.mainButton || status.mainButton.action !== "update") return;
      if (status.process && status.process.running) return;
      if (!update.availableVersion || !status.gameId) return;

      const key = `${status.gameId}:${update.availableVersion}`;
      if (this.autoUpdateStarted[key]) return;
      this.autoUpdateStarted[key] = true;

      ipcRenderer.invoke("game-menu-apply-update", status.gameId).catch(() => {
        /* ошибка придёт обычным статусом доставки */
      });
    },
    async loadDeliveryStatuses() {
      let info = await ipcRenderer.invoke("game-delivery-info");
      if (!info) return;

      let statuses = await ipcRenderer.invoke("game-delivery-get-status", null);
      for (const status of statuses || []) this.onDeliveryStatus(null, status);

      if (info.pending && info.pending.length)
        this.notify(this.$t("#L_D_PENDING"), "fas fa-exclamation-circle");

      if (info.legacyLayout && info.legacyLayout.length)
        this.offerLibraryMigration(info.legacyLayout);
    },
    async offerLibraryMigration(entries) {
      let names = entries.map((g) => g.gameId).join(", ");

      const move = await this.askConfirm({
        title: this.$t("#L_C_MIGRATE_TITLE"),
        text: this.$t("#L_MIGRATE_CONFIRM", names),
        items: entries.map((g) => g.gameId),
        confirmLabel: this.$t("#L_C_MIGRATE"),
      });

      if (!move) return;

      let result = await ipcRenderer.invoke("game-library-migrate");

      if (result && result.status === "ok")
        this.notify(this.$t("#L_MIGRATE_DONE"), "fas fa-check");
      else this.notifyDeliveryError(result);
    },
    async onInstall() {
      let status = this.currentDelivery;
      if (!status) return;

      if (!status.installable) {
        this.notify(this.$t("#L_D_UNAVAILABLE"), "fas fa-info-circle");
        return;
      }

      if (status.requiresLicense) {
        const license = licenseGameOf(this.selectedGameId);
        const go = await this.askConfirm({
          title: this.$t("#L_C_LICENSE_TITLE", license.title),
          text: this.$t("#L_C_LICENSE_TEXT", license.title),
          confirmLabel: this.$t("#L_C_LICENSE_GO"),
        });

        if (!go) return;
      }

      let result = await ipcRenderer.invoke(
        "game-delivery-install",
        this.selectedGameId
      );

      if (result && result.status === "error") await this.explainDeliveryError(result);
    },

    async explainDeliveryError(result) {
      const code = result && result.error ? result.error.code : null;

      if (code === "STEAM_APP_NOT_OWNED") {
        const license = licenseGameOf(this.selectedGameId);
        const open = await this.askConfirm({
          title: this.$t("#L_C_STEAM_LICENSE_TITLE"),
          text: this.$t("#L_C_STEAM_LICENSE_TEXT", license.title),
          confirmLabel: this.$t("#L_C_STEAM_STORE"),
        });

        if (open) ipcRenderer.invoke("open-external-link", license.storeLink);
        return;
      }

      if (["STEAM_NOT_RUNNING", "STEAM_TIMEOUT", "LICENSE_CHECK_UNAVAILABLE"].indexOf(code) !== -1) {
        const license = licenseGameOf(this.selectedGameId);
        const retry = await this.askConfirm({
          title: this.$t("#L_C_LICENSE_UNCHECKED_TITLE", license.title),
          text: this.$t("#L_C_LICENSE_UNCHECKED_TEXT", license.title),
          confirmLabel: this.$t("#L_C_LICENSE_RETRY"),
        });

        if (retry) this.onInstall();
        return;
      }

      this.notifyDeliveryError(result);
    },
    onPauseDownload() {
      ipcRenderer.invoke("game-delivery-pause");
    },
    onResumeOperation() {
      if (this.currentDelivery && this.currentDelivery.operation === "update")
        return this.onApplyUpdate();

      return this.onInstall();
    },
    // «Скачать» / «Продолжить» эпизода на кнопке карточки: какой бы эпизод ни был выбран, качаются
    // оба сразу: Episode Two main ставит вместе с недостающим Episode One.
    async onInstallEpisode() {
      const result = await ipcRenderer.invoke("game-episode-install", "ep2").catch(() => null);
      if (!result || result.status === "error") this.notifyDeliveryError(result);
      this.refreshEpisodes();
    },
    // Размер эпизодов (из манифестов) main кладёт в статус Ghosting и рассылает его обычным событием.
    refreshEpisodes() {
      ipcRenderer.invoke("game-episodes-status").catch(() => {});
    },
    async onRemoveEpisode(id) {
      const episodes = (this.currentDelivery && this.currentDelivery.episodes) || {};
      // Episode Two монтирует Episode One: без него он не запустится и уходит следом.
      const withTwo = id === "ep1" && !!(episodes.ep2 && episodes.ep2.installed);
      const confirmed = await this.askConfirm({
        title: this.$t("#L_GM_UNINSTALL_EPISODE"),
        text: this.$t(withTwo ? "#L_GM_EPISODE_REMOVE_BOTH" : "#L_GM_EPISODE_REMOVE", this.currentBuild),
        note: this.$t("#L_GM_EPISODE_REMOVE_NOTE"),
        confirmLabel: this.$t("#L_C_UNINSTALL"),
        danger: true,
      });
      if (!confirmed) return;

      const result = await ipcRenderer.invoke("game-episode-remove", id).catch(() => null);
      if (!result || result.status === "error") this.notifyDeliveryError(result);
      else this.notify(this.$t("#L_GM_EPISODE_REMOVED"), "fas fa-check");
      this.refreshEpisodes();
    },
    async onApplyUpdate() {
      let gameId = this.currentDelivery ? this.currentDelivery.gameId : null;
      if (!gameId) return;

      let result = await ipcRenderer.invoke("game-menu-apply-update", gameId);

      if (!result || result.status === "error") {
        this.notifyDeliveryError(result);
        return;
      }

      if (result.status === "up-to-date")
        this.notify(this.$t("#L_U_G_UP_TO_DATE"), "fas fa-check");
    },

    async onMenuAction(action) {
      let gameId = this.currentDelivery ? this.currentDelivery.gameId : null;

      switch (action) {
        case "settings":
          this.openGameSettings();
          return;
        case "shortcut":
          return this.runMenuAction("game-menu-create-shortcut", gameId, "#L_GM_SHORTCUT_DONE");
        case "screenshots":
          return this.runMenuAction("game-menu-open-screenshots", gameId);
        case "folder":
          return this.runMenuAction("game-menu-open-folder", gameId);
        case "verify":
          return this.onVerifyFiles(gameId);
        case "repair":
          return this.onRepairGame(gameId);
        case "uninstall":
          return this.currentEpisodeId ? this.onRemoveEpisode(this.currentEpisodeId) : this.onUninstallGame(gameId);
        case "exportBuild":
          return this.onExportBuild(gameId);
        case "exportSettings":
          return this.onExportSettings(gameId);
        case "importBuild":
          return this.onImportBuild(gameId);
      }
    },
    buildTransferError(result) {
      const code = result && result.code;
      const known = ["NOT_INSTALLED", "BAD_FORMAT", "NEWER_FORMAT", "UNKNOWN_GAME", "CORRUPT", "TOO_LARGE", "PATH_INVALID", "GAME_RUNNING", "BUSY"];
      const pack = ["PACK_BAD_FORMAT", "PACK_NEWER_FORMAT", "PACK_UNKNOWN_GAME", "PACK_VERSION_UNKNOWN", "PACK_ORIGINALS_UNAVAILABLE", "PACK_TOO_LARGE", "PACK_DEPOT_BUILD"];
      if (pack.includes(code)) return this.notify(this.$t("#L_BT_ERR_" + code), "fas fa-exclamation-triangle");
      if (known.includes(code)) return this.notify(this.$t("#L_BT_ERR_" + code), "fas fa-exclamation-triangle");
      if (code === "FILE_LOCKED") return this.notify(this.$t("#L_BT_ERR_FILE_LOCKED", result.file || ""), "fas fa-exclamation-triangle");
      // Остальное — ошибки доставки: место, права, антивирус, 7-Zip.
      if (result && result.error) return this.notifyDeliveryError(result);
      this.notify(this.$t("#L_BT_ERR_IO"), "fas fa-exclamation-triangle");
    },
    // Полная сборка в архив: на карточке идёт прогресс, итог — уведомлением.
    async onExportBuild(gameId) {
      let result = await ipcRenderer.invoke("game-build-export", gameId).catch(() => null);
      if (result && result.status === "confirmation-required") {
        result = await this.confirmPackExport(result);
        if (!result) return;
      }
      if (result && result.status === "canceled") return;
      if (!result || result.status !== "ok") return this.buildTransferError(result);
      if (result.kind === "pack") this.notify(this.$t("#L_BT_PACK_EXPORTED", this.formatBytes(result.size)), "fas fa-file-archive");
      else this.notify(this.$t("#L_BT_EXPORTED", String(result.files)), "fas fa-file-export");
    },
    async onExportSettings(gameId) {
      const result = await ipcRenderer.invoke("game-build-export-settings", gameId).catch(() => null);
      if (result && result.status === "canceled") return;
      if (!result || result.status !== "ok") return this.buildTransferError(result);
      this.notify(this.$t("#L_BT_EXPORTED", String(result.files)), "fas fa-file-export");
    },
    // В сборке изменены файлы (кастомный HUD поверх файлов игры): архив сделать
    // можно, при импорте они переедут, а оригиналы скачаются с сервера.
    async confirmPackExport(request) {
      const go = await this.askConfirm({
        title: this.$t("#L_BT_PACK_MODIFIED_TITLE"),
        text: this.$t(request.custom ? "#L_BT_PACK_MODIFIED_CUSTOM" : "#L_BT_PACK_MODIFIED_BACKUP", String(request.modified)),
        note: this.$t("#L_BT_PACK_MODIFIED_NOTE"),
        confirmLabel: this.$t("#L_BT_PACK_MODIFIED_OK"),
      });
      if (!go) return null;
      return ipcRenderer.invoke("game-build-export-confirm", request.token).catch(() => null);
    },
    async confirmPackImport(request) {
      const replace = await this.askConfirm({
        title: this.$t("#L_BT_PACK_REPLACE_TITLE"),
        text: this.$t("#L_BT_PACK_REPLACE", request.installedVersion, request.version),
        note: this.$t("#L_BT_PACK_REPLACE_NOTE"),
        confirmLabel: this.$t("#L_BT_PACK_REPLACE_OK"),
        danger: true,
      });
      if (!replace) return null;
      return ipcRenderer.invoke("game-build-import-confirm", request.token).catch(() => null);
    },
    // После импорта сборка проверяет обновление и сразу ставит его: cfg
    // игрока обновление не трогает. Не установленную сборку игрок ставит
    // сам (лицензия, место), cfg лягут после установки.
    async onImportBuild(gameId) {
      let result = await ipcRenderer.invoke("game-build-import", gameId).catch(() => null);
      if (result && result.status === "confirmation-required") result = await this.confirmPackImport(result);
      if (!result || result.status === "canceled") return;
      if (result.status !== "ok") return this.buildTransferError(result);
      if (result.launcherId && availability.isGameAvailable(result.launcherId)) {
        this.selectedGameId = result.launcherId;
        ipcRenderer.invoke("game-delivery-selected", result.launcherId).catch(() => {});
      }

      if (result.kind === "pack") {
        const parts = [this.$t("#L_BT_PACK_IMPORTED", result.version)];
        // Изменённые файлы сборки: сколько, куда легли, сколько оригиналов скачано (importPack).
        const modified = result.modified;
        if (modified) {
          parts.push(this.$t("#L_BT_PACK_RESTORED", String(modified.changed), String(modified.restored)));
          if (modified.custom) parts.push(this.$t("#L_BT_PACK_MOVED_CUSTOM", String(modified.custom), modified.customDir));
          if (modified.backup) parts.push(this.$t("#L_BT_PACK_MOVED_BACKUP", String(modified.backup), modified.backupDir));
          if (modified.rejected) parts.push(this.$t("#L_BT_PACK_REJECTED", String(modified.rejected)));
        }
        if (result.dropped) parts.push(this.$t("#L_BT_PACK_DROPPED", String(result.dropped)));
        this.notify(parts.join(" "), "fas fa-file-archive", modified ? 15000 : 0);
        // Перенесённую версию не обновляем сами: архив могли взять ради
        // неё (или компьютер без интернета). Кнопка «Обновить» на месте.
        return;
      }

      if (!result.installed) {
        this.notify(this.$t("#L_BT_IMPORT_PENDING"), "fas fa-file-import");
        return;
      }
      if (result.kind !== "pack") this.notify(this.$t("#L_BT_IMPORTED", String(result.files)), "fas fa-file-import");

      const check = await ipcRenderer.invoke("game-menu-check-updates", result.gameId).catch(() => null);
      const update = check && check.status !== "error" ? check.update : null;
      if (!update || !update.updateAvailable || !check.plan || check.plan.kind === "up-to-date") return;
      this.notify(this.$t("#L_BT_UPDATING", update.availableVersion || ""), "fas fa-download");
      const applied = await ipcRenderer.invoke("game-menu-apply-update", result.gameId).catch(() => null);
      if (!applied || applied.status === "error") this.notifyDeliveryError(applied);
    },
    onBuildImportApplied(e, payload) {
      if (!payload) return;
      if (payload.error) this.buildTransferError({ code: payload.error });
      else this.notify(this.$t("#L_BT_IMPORT_APPLIED", String(payload.files)), "fas fa-check");
    },
    async runMenuAction(channel, gameId, successKey) {
      let result;
      try {
        result = await ipcRenderer.invoke(channel, gameId);
      } catch (error) {
        this.notifyDeliveryError(null);
        return null;
      }

      if (!result || result.status === "error") {
        this.notifyDeliveryError(result);
        return null;
      }

      if (successKey) this.notify(this.$t(successKey), "fas fa-check");
      return result;
    },
    notifyDeliveryError(result) {
      let code = result && result.error ? result.error.code : null;
      let key = code ? "#L_D_ERR_" + code : null;
      let text = key ? this.$t(key) : null;

      this.notify(
        text && text !== key ? text : this.$t("#L_D_ERROR"),
        "fas fa-exclamation-triangle"
      );
    },
    async onVerifyFiles(gameId) {
      let result = await this.runMenuAction("game-menu-verify-files", gameId);
      if (!result) return;

      // Разница в стоковых cfg — не повреждение: о ней мягкая строка.
      const cfg = this.stockCfgParts(result);
      let text = this.$t("#L_GM_NO_DAMAGE");
      if (!result.ok) text = this.$t("#L_D_FILES_DAMAGED_N", String(result.damaged)) + this.stockCfgSentence(cfg);
      else if (cfg) text = this.$t("#L_GM_FILES_OK_CFG", cfg);
      this.notify(text, result.ok ? "fas fa-check" : "fas fa-exclamation-circle");
    },
    // Стоковые cfg — только ненулевые части: «2 нет, 1 отличаются».
    stockCfgParts(integrity) {
      const parts = [];
      if (integrity.stockMissing) parts.push(this.$t("#L_GM_CFG_MISSING_N", String(integrity.stockMissing)));
      if (integrity.configChanged) parts.push(this.$t("#L_GM_CFG_DIFFER_N", String(integrity.configChanged)));
      return parts.join(", ");
    },
    stockCfgSentence(parts) {
      return parts ? `. ${this.$t("#L_GM_CFG_DIFF_N", parts)}.` : "";
    },
    // Ошибки ремонта: по файлу — с его именем, «обнови игру», отмена — молча.
    notifyRepairError(result) {
      const error = (result && result.error) || {};
      const details = error.details || {};
      if (error.code === "CANCELLED") return;
      if (details.reason === "UPDATE_AVAILABLE")
        return this.notify(this.$t("#L_GM_REPAIR_UPDATE_FIRST"), "fas fa-sync-alt");
      if (details.path && error.code === "REPAIR_SOURCE_MISSING")
        return this.notify(this.$t("#L_GM_REPAIR_FILE_UNAVAILABLE", details.path), "fas fa-exclamation-triangle");
      if (details.path && (error.code === "CHECKSUM_MISMATCH" || error.code === "SIZE_MISMATCH"))
        return this.notify(this.$t("#L_GM_REPAIR_FILE_BROKEN", details.path), "fas fa-exclamation-triangle");
      this.notifyDeliveryError(result);
    },
    askRepair(files) {
      if (this.repairRequest) this.onRepairResolved(false);
      return new Promise((resolve) => {
        this.repairRequest = { files, resolve };
      });
    },
    onRepairResolved(chosen) {
      const request = this.repairRequest;
      this.repairRequest = null;
      if (request) request.resolve(chosen);
    },
    // Проверка, окно с галочками, загрузка только отмеченных файлов с зеркала
    // и итог по повторной проверке.
    async onRepairGame(gameId) {
      const check = await ipcRenderer.invoke("game-menu-repair", gameId).catch(() => null);
      if (!check || check.status === "error") return this.notifyRepairError(check);

      if (check.status === "no-damage") {
        this.notify(this.$t("#L_GM_NO_DAMAGE"), "fas fa-check");
        return;
      }

      if (check.status !== "confirmation-required") return;

      const files = await this.askRepair(check.files || []);
      if (!files || !files.length) return;

      const repaired = await ipcRenderer
        .invoke("game-menu-repair", gameId, { confirmed: true, recheck: false, files })
        .catch(() => null);

      if (!repaired || repaired.status === "error") return this.notifyRepairError(repaired);

      const done = String((repaired.repaired || []).length);
      const remaining = repaired.remaining || [];
      if (!remaining.length) {
        const cfg = this.stockCfgSentence(this.stockCfgParts(repaired.integrity || {}));
        return this.notify(this.$t("#L_GM_REPAIR_DONE_N", done) + cfg, "fas fa-check");
      }

      const again = await this.askConfirm({
        title: this.$t("#L_C_REPAIR_TITLE"),
        text: this.$t("#L_GM_REPAIR_RESULT", done, String(remaining.length)),
        items: remaining,
        confirmLabel: this.$t("#L_GM_REPAIR_AGAIN"),
        cancelLabel: this.$t("#L_CLOSE"),
      });
      if (again) return this.onRepairGame(gameId);
    },
    async onUninstallGame(gameId) {
      const plan = await this.runMenuAction("game-menu-uninstall", gameId);
      if (!plan || plan.status !== "confirmation-required") return;

      const confirmed = await this.askConfirm({
        title: this.$t("#L_C_UNINSTALL_TITLE"),
        text: this.$t("#L_GM_UNINSTALL_CONFIRM", this.formatBytes(plan.totalBytes)),
        note: this.$t("#L_GM_UNINSTALL_NOTE"),
        confirmLabel: this.$t("#L_C_UNINSTALL"),
        danger: true,
      });

      if (!confirmed) return;

      const result = await ipcRenderer.invoke("game-menu-uninstall", gameId, { confirmed: true });

      if (!result || result.status === "error") {
        this.notifyDeliveryError(result);
        return;
      }

      if (result.status === "partial") {
        this.notify(this.$t("#L_GM_UNINSTALL_PARTIAL"), "fas fa-exclamation-circle");
        return;
      }

      this.notify(
        this.$t("#L_GM_UNINSTALL_DONE", this.formatBytes(result.freedBytes)),
        "fas fa-check"
      );
    },
    onSelectGameFromArgs(e, payload) {
      if (!payload || !payload.launcherId) return;

      let id = String(payload.launcherId);
      if (!availability.isGameAvailable(id)) return;
      this.selectedGameId = id;
      ipcRenderer.invoke("game-delivery-selected", id).catch(() => {});
    },
    async onTool(id) {
      switch (id) {
        case "settings":
          this.settingsOpen = true;
          return;

        case "changelog":
          this.changelogOpen = true;
          return;

        case "discord":
          ipcRenderer.invoke("open-external-link", "discord");
          return;

        case "folder": {
          const result = await ipcRenderer.invoke("open-games-folder");
          if (!result || result.status !== "ok")
            this.notify(
              this.$t("#L_FOLDER_ERROR", (result && result.code) || "UNKNOWN"),
              "fas fa-exclamation-triangle"
            );
          return;
        }

        default:
          this.notify(this.$t("#L_STUB_FEATURE"), "fas fa-info-circle");
      }
    },
    // Шестерёнка у игры открывает страницу «Персонализация» на выбранной игре.
    openGameSettings() {
      this.configsGameId = this.selectedGameId;
      this.section = "configs";
      this.configsOpenToken += 1;
      this.configsSection = "configs";
    },
    // «?» на странице «Персонализация» повторяет обучение на открытой игре.
    onConfigsHelp(gameId) {
      this.startConfigTour(gameId || this.configsGameId);
    },
    // back — раздел, в который вернуть игрока по окончании (по «?» и после «С нуля» он уже на странице).
    startConfigTour(gameId, back = "configs") {
      if (this.configTour) return;
      const game = this.configGames.find((item) => item.id === gameId) || this.configGames[0];
      if (!game) return;
      const bindsId = DELIVERY_IDS[game.id];
      // pageSection — раздел страницы до обучения: по окончании игрок возвращается в него.
      const panel = this.$refs.configsPanel;
      this.configTour = { steps: configTourSteps.stepsFor(bindPresets.engineFor(bindsId), bindsId), back, pageSection: panel ? panel.currentPageSection() : "" };
    },
    // Шаг обучения просит раздел списка слева; панель могла только что появиться — ждём отрисовки.
    onConfigTourPageSection(id) {
      this.$nextTick(() => {
        const panel = this.$refs.configsPanel;
        if (panel) panel.selectPageSection(id);
      });
    },
    onConfigTourDone() {
      const back = this.configTour ? this.configTour.back : "configs";
      const pageSection = this.configTour ? this.configTour.pageSection : "";
      this.configTour = null;
      this.section = back;
      if (pageSection) this.onConfigTourPageSection(pageSection);
      const prefs = prefsStore.load();
      prefs.configTourSeen = true;
      prefsStore.save(prefs);
      this.prefs.configTourSeen = true;
    },
    openDiagnostics() {
      this.settingsSection = "diagnostics";
      this.settingsOpen = true;
    },
    async onUnavailableAction(actionId) {
      const gameId = this.selectedGameId;

      switch (actionId) {
        case "recheck":
          await ipcRenderer.invoke("game-delivery-selected", gameId);
          this.notify(this.$t("#L_UA_RECHECKED"), "fas fa-sync-alt");
          return;
        case "open-steam":
          ipcRenderer.invoke("game-launch-open-steam");
          return;
        case "install-sdk":
          ipcRenderer.invoke("game-launch-install-sdk");
          return;
        case "open-library-settings":
          this.unavailableOpen = false;
          this.settingsSection = "downloads";
          this.settingsOpen = true;
          return;
        case "verify-files":
          this.unavailableOpen = false;
          return this.onMenuAction("verify");
        case "repair":
          this.unavailableOpen = false;
          return this.onMenuAction("repair");
        case "open-diagnostics":
          this.unavailableOpen = false;
          return this.openDiagnostics();
        case "open-discord":
          ipcRenderer.invoke("open-discord");
          return;
        case "check-launcher-update":
          this.unavailableOpen = false;
          ipcRenderer.invoke("updater-check");
          this.settingsSection = "about";
          this.settingsOpen = true;
          this.notify(this.$t("#L_UA_LAUNCHER_UPDATE_CHECK"), "fas fa-arrow-up");
          return;
      }
    },

    closeChangelog() {
      this.changelogOpen = false;
      this.changelogVersion = null;
      this.changelogExtra = null;
    },

    async onWhatsNew(payload) {
      const target = payload && payload.target;
      const version = payload && typeof payload.version === "string" ? payload.version : null;

      if (target === "launcher") {
        this.changelogVersion = version;
        this.changelogExtra =
          version &&
          this.updateStatus.version === version &&
          this.updateStatus.releaseNotes
            ? {
                version,
                items: String(this.updateStatus.releaseNotes)
                  .split(/\n+/)
                  .map((line) => line.trim())
                  .filter(Boolean),
              }
            : null;
        this.changelogOpen = true;
        return;
      }

      if (target === "game" && version) {
        const gameId = payload.gameId || (this.currentDelivery && this.currentDelivery.gameId);
        const notes = await ipcRenderer.invoke("game-release-notes", gameId, version);

        this.gameNotesVersion = version;
        this.gameNotesData = notes || null;
        this.gameNotesOpen = true;
      }
    },

    onRunUpdateAction(payload) {
      const target = payload && payload.target;

      if (target === "proton") {
        this.settingsSection = "proton";
        this.settingsOpen = true;
        ipcRenderer.invoke("proton-update").catch((error) => {
          this.notify(this.$t("#L_S_PROTON_ERROR") + ": " + error.message, "fas fa-exclamation-triangle");
        });
        return;
      }

      if (target === "launcher") {
        this.updateDismissed = false;
        this.startUpdateDownload();
        return;
      }

      if (target === "game") {
        const gameId =
          (payload && payload.gameId) ||
          (this.currentDelivery && this.currentDelivery.gameId);
        if (!gameId) return;

        ipcRenderer.invoke("game-menu-apply-update", gameId).then((result) => {
          if (!result || result.status === "error") this.notifyDeliveryError(result);
        });
      }
    },

    // gameId — игра из уведомления о падении; в старых уведомлениях его нет,
    // и берётся последнее падение.
    async onOpenCrashDump(gameId) {
      const result = await ipcRenderer.invoke("game-crash-open-dump", gameId || null);
      if (!result || result.status !== "ok")
        this.notify(this.$t("#L_CR_NO_DUMP"), "fas fa-info-circle");
    },
    onOpenDiscord() {
      ipcRenderer.invoke("open-discord");
    },
    onCrashReport(e, report) {
      if (report && report.gameId) this.crashReport = report;
    },
    onProfileImported(result) {
      for (const game of this.games) this.loadStats(game.id);

      if (result && result.preferences) {
        const prefs = Object.assign({}, this.prefs, {
          animations: result.preferences.animations !== false,
        });
        this.prefs = prefs;
        prefsStore.save(prefs);

        this.setLanguage(result.preferences.language);
      }

      this.notify(this.$t("#L_S_DATA_IMPORTED"), "fas fa-file-import");
    },
    onRunnerProfileUpdated() { this.loadStats(this.selectedGameId); },
    onPublicOnline(event, snapshot) {
      if (snapshot && typeof snapshot === "object") this.onlineSnapshot = snapshot;
    },

    async loadStats(gameId) {
      const summary = await ipcRenderer.invoke("game-stats-get", gameId);
      this.stats[gameId] = summary || { available: false };
    },
    onGameSettingsSaved() {
      this.notify(this.$t("#L_GS_SAVED"), "fas fa-check");
    },
    setLanguage(id) {
      const keys = Object.keys(this.$localization.locales);
      if (id === this.currentLanguage || keys.indexOf(id) === -1) return;

      const config = settings.get("config") || {};
      config.language = keys.indexOf(id);
      settings.set("config", config);
      this.$localization.update();
      this.onLanguageChange();
    },
    onLanguageChange() {
      this.langRev++;
      this.syncUpdaterLanguage();
    },
    syncUpdaterLanguage() {
      ipcRenderer.invoke("updater-set-language", this.currentLanguage);
    },
    closeSettings() {
      this.settingsOpen = false;
      this.settingsSection = "general";
    },

    // Игра без конфига: спросить, как его готовить. true — запуск не продолжать
    // (отмена, «С нуля» и «Свой» уводят на страницу «Персонализация»; игрок жмёт «Играть» сам).
    async offerConfigSetup(gameId) {
      // Двойной клик по «Играть»: второй вызов не открывает второй опрос и не запускает игру.
      if (this.configSetupBusy) return true;
      this.configSetupBusy = true;
      try {
        let state = null;
        try {
          state = await ipcRenderer.invoke("player-config-setup-state", gameId);
        } catch (error) {
          return false;
        }
        if (!state || !state.needed) return false;

        const choice = await new Promise((resolve) => {
          this.configSetup = { resolve };
        });
        if (!choice) return true;

        // Не записалось (папка недоступна) — игрока не задерживаем: запуск сам покажет причину.
        try {
          await ipcRenderer.invoke("player-config-setup-done", gameId, choice);
        } catch (error) {
          console.error("[player-config] ответ на опрос не записан", error);
        }
        if (choice === "skip") return false;

        this.configsGameId = gameId;
        this.section = "configs";
        this.configsOpenToken += 1;
        this.configsSection = "configs";
        if (choice === "scratch") this.configTourRequested = true;
        else this.configsImportRequested = true;
        return true;
      } finally {
        this.configSetupBusy = false;
      }
    },
    onConfigSetupChoose(choice) {
      const request = this.configSetup;
      this.configSetup = null;
      if (request) request.resolve(choice);
    },

    async offerTimer() {
      if (this.prefs.timerOffered === true) return false;

      const timer = this.tools.find((tool) => tool.toolId === "livesplit");
      if (!timer) return false;

      if (timer.state === "ready") {
        this.rememberTimerOffered();
        return false;
      }

      const wanted = await this.askConfirm({
        title: this.$t("#L_TOOL_OFFER_TITLE"),
        text: this.$t("#L_TOOL_OFFER_TEXT"),
        confirmLabel: this.$t("#L_C_YES"),
        cancelLabel: this.$t("#L_C_NO"),
      });

      this.rememberTimerOffered();

      if (!wanted) return false;

      this.onSelectTool(timer.toolId);
      return true;
    },
    rememberTimerOffered() {
      const prefs = Object.assign({}, this.prefs, { timerOffered: true });
      this.prefs = prefs;
      prefsStore.save(prefs);
    },
    // Выдвинутая колонка значков запоминается между запусками.
    // В играх колонка запоминает своё положение. На «Главной» и в
    // «Персонализации» она свёрнута и раскрывается только до ухода с вкладки:
    // сохранённое положение не меняется и вернётся вместе с «Играми».
    toggleRail() {
      if (this.section !== "games") {
        this.railPeek = !this.railPeek;
        return;
      }
      const prefs = Object.assign({}, this.prefs, {
        railExpanded: this.prefs.railExpanded !== true,
      });

      this.prefs = prefs;
      prefsStore.save(prefs);
    },
    onToggleSteamId() {
      const prefs = Object.assign({}, this.prefs, {
        hideSteamId: this.prefs.hideSteamId !== true,
      });

      this.prefs = prefs;
      prefsStore.save(prefs);
      this.pushHideSteamId(prefs.hideSteamId);
    },
    // «Скрыть SteamID» хранится и на сервере: по нему лаунчеры других
    // игроков не показывают ID в профиле и поиске. Переключатель меняется
    // сразу, запрос уходит следом; пока он в пути, ответы сервера со старым
    // значением настройку не откатывают.
    async pushHideSteamId(value) {
      const sequence = (this.hideIdSequence = (this.hideIdSequence || 0) + 1);
      try {
        await ipcRenderer.invoke("runner-customize", { hideSteamId: value });
      } catch (error) {
        // без сети: отправится при следующем открытии профиля
      } finally {
        if (sequence === this.hideIdSequence) this.hideIdSequence = 0;
      }
    },
    onServerHideSteamId(value) {
      if (this.hideIdSequence) return;
      const local = this.prefs.hideSteamId === true;
      // У старых профилей флага на сервере нет: отправляется локальный.
      if (typeof value !== "boolean") {
        if (local) this.pushHideSteamId(true);
        return;
      }
      if (value === local) return;
      const prefs = Object.assign({}, this.prefs, { hideSteamId: value });
      this.prefs = prefs;
      prefsStore.save(prefs);
    },
    onSelectTool(toolId) {
      this.section = "games";
      this.activeTab = "tools";
      this.selectedToolId = toolId;
      this.loadTools();
    },
    async onSelectDemo() {
      if (this.ffmpegGate || !(await this.demoFfmpegReady())) return;
      this.section = "games";
      this.activeTab = "demo";
    },
    // FFmpeg не скачан: окно с предложением; true — можно входить (уже есть,
    // скачался, Linux с ffmpeg из PATH, сломан или ставится — тогда скажет сама страница).
    async demoFfmpegReady() {
      if (this.section === "games" && this.activeTab === "demo") return true;
      const answer = await ipcRenderer.invoke("demo-ffmpeg-status").catch(() => null);
      const status = answer && answer.status === "ok" ? answer.ffmpeg : null;
      if (!status || status.state !== "not-installed" || !status.installable) return true;
      return new Promise((resolve) => {
        this.ffmpegGate = { status, installing: false, percent: 0, error: null, resolve };
      });
    },
    closeFfmpegGate(entered) {
      const gate = this.ffmpegGate;
      this.ffmpegGate = null;
      if (gate) gate.resolve(entered);
    },
    async onFfmpegGate(accepted) {
      const gate = this.ffmpegGate;
      if (!gate) return;
      // «Отменить загрузку» и Esc во время загрузки: окно закроет ответ установки (cancelled).
      if (!accepted) {
        if (gate.installing) ipcRenderer.invoke("demo-ffmpeg-cancel").catch(() => {});
        else this.closeFfmpegGate(false);
        return;
      }
      if (gate.installing) return;
      Object.assign(gate, { installing: true, percent: 0, error: null });
      const answer = await ipcRenderer.invoke("demo-ffmpeg-install").catch(() => null);
      gate.installing = false;
      if (answer && answer.status === "ok") return this.closeFfmpegGate(true);
      if (answer && answer.status === "cancelled") return this.closeFfmpegGate(false);
      const message = demoErrorMessage(answer || { code: "DEMO_FAILED" });
      gate.error = this.$t(message.key, ...message.args);
    },
    onFfmpegProgress(event, payload) {
      if (this.ffmpegGate && this.ffmpegGate.installing && payload)
        this.ffmpegGate.percent = Math.max(0, Math.min(100, Math.round(Number(payload.percent) || 0)));
    },
    // Свой профиль из рейтинга, поиска или списка спонсоров открывается как
    // свой: без голосования за себя и с вкладкой настроек.
    async openRunnerProfile(id) {
      // Профили игроков, свой профиль и друзья — только в официальной сборке.
      if (!this.official) return;
      let target = id || "";
      if (target) {
        try {
          const state = await ipcRenderer.invoke("runner-state");
          if (state && state.account && state.account.steamId64 === target) target = "";
        } catch (error) {
          // Без состояния входа открываем как чужой: сервер голос за себя всё равно не примет.
        }
      }
      this.profileTarget = target;
      this.profileSection = "profile";
      this.friendsOpen = false;
      this.profileOpen = true;
    },
    onAccountAction(id) {
      if (!this.official) return;
      if (id === "open-profile") this.openRunnerProfile("");
      else if (id === "profile-settings") {
        this.openRunnerProfile("");
        this.profileSection = "settings";
      }
      else if (id === "friends") {
        // Друзья открываются вместо профиля, а не поверх него.
        if (!this.friendsOpen) this.profileOpen = false;
        this.friendsOpen = !this.friendsOpen;
      }
    },
    syncToolSplits() {
      ipcRenderer.invoke("tool-splits-set", "livesplit", Object.assign({}, this.liveSplitSplits)).catch(() => {});
    },
    async loadTools() {
      try {
        const list = await ipcRenderer.invoke("tool-list");
        if (Array.isArray(list)) {
          this.tools = list;
          if (!this.selectedToolId && list.length) this.selectedToolId = list[0].toolId;
          this.syncToolSplits();
        }
      } catch (ignored) {
      }
    },
    onToolProgress(event, payload) {
      if (!payload || !payload.toolId) return;

      const index = this.tools.findIndex((tool) => tool.toolId === payload.toolId);
      if (index < 0) return;

      this.tools[index] = Object.assign({}, this.tools[index], { state: "installing", percent: payload.percent });
    },
    onToolStatus(event, card) {
      if (!card || !card.toolId) return;

      const index = this.tools.findIndex((tool) => tool.toolId === card.toolId);
      if (index < 0) this.tools.push(card);
      else this.tools[index] = card;
    },
    async onToolInstall(toolId) {
      const result = await ipcRenderer.invoke("tool-install", toolId);
      if (result && result.status === "cancelled") return;

      if (result && result.status === "error") {
        const code = result.error && result.error.code ? ` (${result.error.code})` : "";
        this.notify(this.$t("#L_TOOL_INSTALL_FAILED") + code, "fas fa-exclamation-triangle");
        return;
      }

      this.notify(this.$t("#L_TOOL_INSTALLED"), "fas fa-check");
      this.loadTools();
    },
    onToolCancel(toolId) {
      ipcRenderer.invoke("tool-cancel", toolId);
    },
    // episode — эпизод на паузе: main убирает архив только его.
    onCancelDownload(episode) {
      ipcRenderer.invoke("game-delivery-cancel", this.currentDelivery.gameId, episode ? { episode } : undefined);
    },
    cancelUpdateDownload() {
      ipcRenderer.invoke("updater-cancel");
    },
    async onToolLaunch(toolId) {
      let gameChoice = "";
      let preset = null;
      if (toolId === "livesplit") {
        const picked = await this.askLiveSplitLaunch();
        if (!picked) return;
        gameChoice = picked.game;
        preset = picked.preset;
        this.rememberLiveSplit(picked);
      }
      let result = await ipcRenderer.invoke("tool-launch", toolId, preset, gameChoice);

      if (result && result.status === "already-running" && toolId === "livesplit") {
        if (result.same) return this.notify(this.$t("#L_TOOL_LIVESPLIT_ALREADY"), "fas fa-stopwatch");
        const restart = await this.askConfirm({
          title: this.$t("#L_TOOL_LIVESPLIT_RESTART_TITLE"),
          text: this.$t("#L_TOOL_LIVESPLIT_RESTART_TEXT"),
          confirmLabel: this.$t("#L_TOOL_LIVESPLIT_RESTART"),
        });
        if (!restart) return;
        result = await ipcRenderer.invoke("tool-launch", toolId, preset, gameChoice, { restart: true });
        if (result && result.status === "stop-timeout")
          return this.notify(this.$t("#L_TOOL_STOP_FAILED"), "fas fa-exclamation-triangle");
      }

      if (result && result.status === "error")
        this.notify(this.$t("#L_TOOL_LAUNCH_FAILED"), "fas fa-exclamation-triangle");
    },
    // Окно запуска LiveSplit: игра и её сплиты. На Linux LiveSplit идёт в
    // префикс Proton этой игры (иначе автосплиттер её не видит), на Windows
    // к этой игре привязывается оверлей LiveSplit. Сплиты у каждой игры
    // свои, наборы отдаёт main (livesplit-splits.js).
    async askLiveSplitLaunch() {
      const games = availability.orderGames(GameList.filter((game) => availability.isGameAvailable(game.id)));
      let described = [];
      try {
        described = await ipcRenderer.invoke("tool-splits-presets", games.map((game) => String(game.id)));
      } catch (ignored) {
      }
      const presetsById = {};
      for (const entry of described || []) if (entry && entry.id) presetsById[entry.id] = entry;
      const choices = games.map((game) => {
        const entry = presetsById[String(game.id)] || {};
        return {
          id: String(game.id),
          gameId: entry.gameId || null,
          label: launcherData.shortNames[game.id] || game.name,
          presets: entry.presets || [],
        };
      });
      if (!choices.length) return null;

      const running = Object.keys(this.delivery).find((id) => {
        const status = this.delivery[id];
        return status && status.process && status.process.running;
      });
      const runningChoice = running && choices.find((choice) => choice.gameId === running);
      const defaultGame = runningChoice
        ? runningChoice.id
        : this.prefs.liveSplitGame || (this.selectedGameId != null ? String(this.selectedGameId) : null);

      if (this.liveSplitLaunch) this.onLiveSplitLaunchResolved(false);
      return new Promise((resolve) => {
        this.liveSplitLaunch = { games: choices, defaultGame, resolve };
      });
    },
    onLiveSplitLaunchResolved(picked) {
      const request = this.liveSplitLaunch;
      this.liveSplitLaunch = null;
      if (request) request.resolve(picked || null);
    },
    rememberLiveSplit(picked) {
      if (picked.gameId && picked.preset)
        this.liveSplitSplits = Object.assign({}, this.liveSplitSplits, { [picked.gameId]: picked.preset });
      this.prefs = Object.assign({}, this.prefs, { liveSplitGame: picked.game, toolSplits: this.liveSplitSplits });
      prefsStore.save(this.prefs);
      this.syncToolSplits();
    },
    async onToolStop(toolId) {
      const result = await ipcRenderer.invoke("tool-stop", toolId);
      if (!result || result.status === "error") {
        this.notify(this.$t("#L_TOOL_STOP_FAILED"), "fas fa-exclamation-triangle");
        return;
      }
      if (result.status !== "timeout") return;

      const force = await this.askConfirm({
        title: this.$t("#L_TOOL_STOP_TIMEOUT_TITLE"),
        text: this.$t("#L_TOOL_STOP_TIMEOUT_TEXT"),
        confirmLabel: this.$t("#L_C_STOP_FORCE"),
        danger: true,
      });
      if (!force) return;
      const forced = await ipcRenderer.invoke("tool-stop", toolId, { force: true });
      if (!forced || forced.status !== "stopped")
        this.notify(this.$t("#L_TOOL_STOP_FAILED"), "fas fa-exclamation-triangle");
    },
    async onToolRemove(toolId) {
      const confirmed = await this.askConfirm({
        title: this.$t("#L_TOOL_REMOVE"),
        text: this.$t("#L_TOOL_REMOVE_TEXT"),
        confirmLabel: this.$t("#L_TOOL_REMOVE"),
      });

      if (!confirmed) return;

      await ipcRenderer.invoke("tool-remove", toolId);
      this.notify(this.$t("#L_TOOL_REMOVED"), "fas fa-check");
      this.loadTools();
    },
    onToolFolder(toolId) {
      ipcRenderer.invoke("tool-open-folder", toolId).then((result) => {
        if (result && result.status !== "ok")
          this.notify(this.$t("#L_TOOL_FOLDER_FAILED"), "fas fa-exclamation-triangle");
      }, () => {});
    },
    onToolHelp(toolId) {
      this.notify(this.$t("#L_TOOL_HELP_HINT"), "fas fa-question-circle");
      ipcRenderer.invoke("tool-open-help", toolId).catch(() => {});
    },
    syncTrayPreference() {
      ipcRenderer
        .invoke("tray-hide-while-playing-set", this.prefs.hideWhilePlaying !== false)
        .catch(() => {});
    },
    async initializeDownloadSettings() {
      try {
        const state = await ipcRenderer.invoke("download-settings-get");
        if (state && state.initialized) {
          this.prefs.speedLimit = state.speedLimit;
          prefsStore.save(this.prefs);
        } else {
          const result = await ipcRenderer.invoke("download-settings-set", this.prefs.speedLimit);
          if (!result || !result.ok) throw new Error("Download settings were not saved");
        }
      } catch (error) {
        this.notify(this.$t("#L_S_SPEED_SAVE_FAILED"), "fas fa-exclamation-triangle");
      }
    },
    async applyPrefs(draft, extra) {
      let installPath = draft.installPath;
      let prefs = Object.assign({}, draft);
      delete prefs.installPath;

      this.prefs = prefs;
      prefsStore.save(prefs);
      this.syncTrayPreference();

      this.liveSplitSplits = liveSplitChoicesOf(prefs);
      this.syncToolSplits();

      if (installPath !== this.installPath) {
        let result = null;
        try {
          result = await ipcRenderer.invoke("set-library-path", installPath);
        } catch (error) {
          result = { ok: false };
        }

        if (!result || !result.ok) {
          this.notify(this.$t(result && result.error === "busy" ? "#L_S_PATH_BUSY" : "#L_S_PATH_INVALID"), "fas fa-exclamation-triangle");
          return;
        }

        this.installPath = result.path || installPath;
      }

      // После «Сохранить» окно настроек остаётся открытым (решение владельца 14.11).
      // Закрывается только при смене языка, и язык меняется последним, когда окно
      // уже закрыто: смена языка пересоздаёт весь интерфейс (:key="langRev"), и
      // открытое окно пересоздалось бы вместе с ним посреди сохранения
      if (extra && extra.language && extra.language !== this.currentLanguage) {
        this.closeSettings();
        this.setLanguage(extra.language);
      }
      if (extra && extra.gpuChanged)
        this.notify(this.$t("#L_S_GPU_RESTART"), "fas fa-info-circle");
      this.notify(this.$t("#L_SETTINGS_SAVED"), "fas fa-check");
    },
    // Размер на языке окна (file-format.js); неизвестный — пустая строка.
    formatBytes(bytes) {
      const value = Number(bytes);
      if (!isFinite(value) || value <= 0) return "";
      return formatSize(value, this.$t);
    },
    askConfirm(request) {
      if (this.confirmRequest) this.onConfirmResolved(false);

      return new Promise((resolve) => {
        this.confirmRequest = Object.assign({ items: [], note: null, danger: false, choices: [], defaultChoice: null }, request, {
          resolve,
        });
      });
    },
    onConfirmResolved(accepted) {
      let request = this.confirmRequest;
      this.confirmRequest = null;
      if (!request) return;
      // С вариантами ответ — id выбранного или false.
      if (request.choices.length) request.resolve(accepted || false);
      else request.resolve(!!accepted);
    },
    // ms — сколько держать уведомление: длинное, с путями к папкам, — дольше.
    notify(text, icon, ms) {
      let id = ++this.toastSeq;
      this.toasts.push({ id, text, icon: icon || "fas fa-info-circle" });

      if (this.toasts.length > 3) this.toasts.shift();

      setTimeout(() => this.dismissToast(id), ms || 2800);
    },
    dismissToast(id) {
      let index = this.toasts.findIndex((t) => t.id === id);
      if (index >= 0) this.toasts.splice(index, 1);
    },

    async checkLibrarySetup() {
      try {
        let info = await ipcRenderer.invoke("get-library-path");
        this.installPath = info.path;

        if (!info.confirmed) this.librarySetupOpen = true;
        else await this.checkWelcome();
      } catch (error) {
        console.error("[launcher] проверка библиотеки не удалась", error);
      } finally {
        // Сбой проверки не должен навсегда лишить игрока обучения; открытое окно выбора папки держит overlayOpen.
        if (!this.librarySetupOpen) this.welcomeChecked = true;
      }
    },
    onLibraryConfirmed(confirmedPath) {
      this.installPath = confirmedPath;
      this.librarySetupOpen = false;
      this.notify(this.$t("#L_SETTINGS_SAVED"), "fas fa-check");
      this.checkWelcome();
    },
    // «HLSR — Новая эра!»: один раз после обновления до 0.4 и на новой
    // установке, если игрок ещё не вошёл через Steam. Новой установке окно
    // показывается после выбора папки библиотеки.
    async checkWelcome() {
      try {
        await this.offerWelcome();
      } finally {
        this.welcomeChecked = true;
      }
    },
    async offerWelcome() {
      if (this.prefs.welcomeSeen) return;
      try {
        const state = await ipcRenderer.invoke("runner-state");
        if (state && state.account && state.account.signedIn) {
          this.markWelcomeSeen();
          return;
        }
      } catch (error) {
        return;
      }
      this.welcomeOpen = true;
    },
    markWelcomeSeen() {
      const prefs = prefsStore.load();
      prefs.welcomeSeen = true;
      prefsStore.save(prefs);
      this.prefs.welcomeSeen = true;
    },
    // Вход начат: код подтверждения и ожидание браузера показывает профиль.
    onWelcomeSignedIn() {
      this.markWelcomeSeen();
      this.welcomeOpen = false;
      this.openRunnerProfile("");
    },
    onWelcomeLater() {
      this.markWelcomeSeen();
      this.welcomeOpen = false;
      // «Войти можно через аватар в углу» — только там, где вход есть.
      if (this.official) this.notify(this.$t("#L_WELCOME_LATER_TOAST"), "fas fa-user");
    },

    onCloseProfile() {
      this.profileOpen = false;
    },

    // «Поддержать проект» в настройках профиля: окно «Спонсорство» вместо
    // профиля (профиль лежит выше и закрыл бы его), как переход из окна
    // спонсоров к профилю.
    onOpenSponsors() {
      if (!this.official) return;
      this.profileOpen = false;
      this.sponsorsOpen = true;
    },

    onUpdaterState(e, status) {
      if (status.state === "checking" && status.checkSource === "manual")
        this.updateDismissed = false;
      if (status.state === "available" && status.version !== this.updateStatus.version)
        this.updateDismissed = false;
      if (
        status.state === "downloaded" ||
        (status.state === "error" && status.checkSource === "manual")
      )
        this.updateDismissed = false;
      this.updateStatus = status;
    },
    checkForUpdates() {
      ipcRenderer.invoke("updater-check");
    },
    async startUpdateDownload() {
      let started = false;

      try {
        started = await ipcRenderer.invoke("updater-download");
      } catch (error) {
        started = false;
      }

      if (started) return;

      this.notify(this.$t("#L_U_CHECKING"), "fas fa-sync");
      ipcRenderer.invoke("updater-check").catch(() => {});
    },
    async confirmInstallNow() {
      let busy = this.deliveryBusy;
      let question = busy
        ? this.$t("#L_U_CONFIRM_RESTART_BUSY")
        : this.$t("#L_U_CONFIRM_RESTART");

      const restart = await this.askConfirm({
        title: this.$t("#L_C_RESTART_TITLE"),
        text: question,
        confirmLabel: this.$t("#L_C_RESTART"),
        danger: busy,
      });

      if (!restart) return;

      ipcRenderer.invoke("updater-install-now");
    },
    installOnQuit() {
      ipcRenderer.invoke("updater-install-on-quit");
      this.updateDismissed = true;
      this.notify(this.$t("#L_U_WILL_INSTALL_ON_QUIT"), "fas fa-check");
    },
  },
  mounted() {
    this.selectedGameId = availability.resolveStartupGame(
      library.get("lastLaunched")
    );
    // Выбранная при запуске игра — и для Discord: иначе до первого щелчка её подменял статус любой игры.
    ipcRenderer.invoke("game-delivery-selected", this.selectedGameId).catch(() => {});

    this.syncUpdaterLanguage();
    this.syncTrayPreference();
    this.checkLibrarySetup();
    this.initializeDownloadSettings().then(() => {
      this.downloadSettingsReady = true;
      this.loadTools();
      this.loadDeliveryStatuses();
    });
    for (const game of this.games) this.loadStats(game.id);

    ipcRenderer.on("game-delivery-status", this.onDeliveryStatus);
    ipcRenderer.on("game-build-import-applied", this.onBuildImportApplied);
    ipcRenderer.on("select-game", this.onSelectGameFromArgs);
    ipcRenderer.on("game-process-running", this.onProcessRunning);
    ipcRenderer.on("game-process-exited", this.onProcessExited);
    ipcRenderer.on("game-process-crashed", this.onProcessCrashed);
    ipcRenderer.on("game-crash-report", this.onCrashReport);
    ipcRenderer.on("tool-progress", this.onToolProgress);
    ipcRenderer.on("tool-status", this.onToolStatus);
    ipcRenderer.on("demo-ffmpeg-progress", this.onFfmpegProgress);
    ipcRenderer.on("updater-state", this.onUpdaterState);
    ipcRenderer.invoke("updater-get-state").then((status) => {
      if (status) this.updateStatus = status;
    });

    ipcRenderer.on("public-online", this.onPublicOnline);
    ipcRenderer.on("runner-profile-state", this.onRunnerProfileUpdated);
    ipcRenderer.on("achievement-unlocked", this.onAchievementUnlocked);
    ipcRenderer.on("support-thanks", this.onSupportThanks);
    ipcRenderer.send("support-thanks-ready");
    window.addEventListener("keydown", this.onGlobalKeydown);
    ipcRenderer.invoke("public-online-get").then((snapshot) => {
      if (snapshot) this.onlineSnapshot = snapshot;
    });
  },
  beforeUnmount() {
    clearTimeout(this.logoTimer);
    clearTimeout(this.configTourTimer);
    clearTimeout(this.ponyTimer);
    clearTimeout(this.cheatTimer);
    ipcRenderer.removeListener("public-online", this.onPublicOnline);
    ipcRenderer.removeListener("runner-profile-state", this.onRunnerProfileUpdated);
    ipcRenderer.removeListener("achievement-unlocked", this.onAchievementUnlocked);
    ipcRenderer.removeListener("support-thanks", this.onSupportThanks);
    window.removeEventListener("keydown", this.onGlobalKeydown);
    ipcRenderer.removeListener("updater-state", this.onUpdaterState);
    ipcRenderer.removeListener("game-delivery-status", this.onDeliveryStatus);
    ipcRenderer.removeListener("game-build-import-applied", this.onBuildImportApplied);
    ipcRenderer.removeListener("select-game", this.onSelectGameFromArgs);
    ipcRenderer.removeListener("game-process-running", this.onProcessRunning);
    ipcRenderer.removeListener("game-process-exited", this.onProcessExited);
    ipcRenderer.removeListener("game-process-crashed", this.onProcessCrashed);
    ipcRenderer.removeListener("game-crash-report", this.onCrashReport);
    ipcRenderer.removeListener("tool-progress", this.onToolProgress);
    ipcRenderer.removeListener("tool-status", this.onToolStatus);
    ipcRenderer.removeListener("demo-ffmpeg-progress", this.onFfmpegProgress);
  },
};
</script>

<style lang="scss" scoped>
/* Фирменная подложка старого HLSR (assets/gradient.jpg).
   Картинку подставляет rootStyle через --l-app-image (путь резолвит webpack),
   поверх неё лежит затемняющий слой --l-app-scrim: оригинал заметно светлее,
   и без затемнения панели и текст теряют контраст.
   Фон игры (картинка или видео) рисуется ПОВЕРХ подложки и ничем не
   закрывается.

   Раньше подложка включалась отдельным правилом под переключателем
   «Цветной фон интерфейса». Переключателя больше нет, поэтому правило
   одно: два правила с одним селектором держались только на порядке
   следования и ломались от любой перестановки. */
.launcher-root {
  position: absolute;
  inset: 0;
  font-family: Rubik, sans-serif;
  overflow: hidden;
  z-index: 10;

  background-image: var(--l-app-scrim), var(--l-app-image, none);
  background-size: cover, cover;
  background-position: center, center;
  background-repeat: no-repeat, no-repeat;
  /* Фон статичен: не анимируется и не перерисовывается каждый кадр */
  background-color: var(--l-bg);
}

/* Колонка значков и страница стоят в одной строке во всех разделах:
   выдвинутая колонка сдвигает и сужает страницу (как в «Демо»), а не
   ложится поверх неё (просьба владельца 11.11). Фон и окна — абсолютные
   слои и от ширины колонки не зависят. */
.launcher-frame {
  position: absolute;
  inset: 0;
  display: flex;
}

.no-anim .launcher-topbar__logo-mark,
.no-anim .launcher-topbar__logo-dot,
.no-anim .launcher-topbar__logo-text,
.no-anim .launcher-topbar__brand {
  transition: none;
}

/* Колонка выдвинута: «HLSR» прилипает к кружку, и логотип с текстом
   стоят одним знаком внутри колонки. Блок логотипа — ровно по её
   ширине, поэтому вкладки шапки начинаются за её краем. */
.rail-open .launcher-topbar__brand {
  width: var(--l-rail-open-w);
}

.rail-open .launcher-topbar__logo-mark {
  margin-right: 0;
}

.rail-open .launcher-topbar__logo-text {
  margin-left: 12px;
  /* Разрядка меньше — и блок уже на столько же (4 буквы × 3 px), иначе
     «BETA» отрывается от «R». Ширина едет вместе с разрядкой. */
  width: 96px;
  letter-spacing: 5px;
}

.launcher-main {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;

  /* Содержимое обязано лежать ВЫШЕ фонового слоя.
     Без собственного позиционирования это не так: .launcher-bg спозиционирован
     и имеет z-index 0, а непозиционированные элементы рисуются ниже
     позиционированных независимо от порядка в разметке. Из-за этого затемняющая
     подложка фона ложилась поверх заголовка и описания — и у игр, и у
     инструмента. */
  position: relative;
  z-index: 1;
}

.launcher-stage {
  flex: 1;
  min-height: 0;
  display: grid;
  grid-template: minmax(0, 1fr) / minmax(0, 1fr);
}

.launcher-dock {
  flex: none;
  margin-top: auto;
  display: grid;
  grid-template-columns: minmax(0, 1fr);
}

.launcher-stage > *,
.launcher-dock > * {
  grid-area: 1 / 1;
}

/* У LiveSplit на сцене только текст: щелчки мимо нижней панели проходят
   к шарикам фона (тайная игра «повтори цвета», LauncherBackground). */
.launcher-main--tool {
  pointer-events: none;
}

.launcher-main--tool .launcher-dock {
  pointer-events: auto;
}

/* HESOYAM: плашка в духе подсказок GTA: San Andreas — тёмный полупрозрачный
   прямоугольник и жирный округлый Rubik. Встаёт на 4 px ниже кнопки
   «Закрыть», правым краем по её правому краю, и выезжает из-за края окна. */
.egg-cheat {
  position: absolute;
  top: calc((var(--l-topbar-h) + var(--l-header-control)) / 2 + 4px);
  right: calc(var(--l-header-gap) - var(--l-header-inset));
  z-index: 70;
  padding: 10px 20px;
  background: rgba(0, 0, 0, 0.62);
  color: #d9dadb;
  font-size: 22px;
  font-weight: 700;
  line-height: 28px;
  letter-spacing: 0.02em;
  white-space: nowrap;
  text-shadow: 0 0 2px #000, 1px 1px 0 #000;
  pointer-events: none;
}

.egg-cheat-enter-active {
  transition: transform var(--l-dur) var(--l-ease);
}

.egg-cheat-leave-active {
  transition: transform var(--l-dur) var(--l-ease-in), opacity var(--l-dur) var(--l-ease-in);
}

.egg-cheat-enter-from,
.egg-cheat-leave-to {
  transform: translateX(calc(100% + 32px));
}

.egg-cheat-leave-to {
  opacity: 0;
}

/* Пони выглядывает снизу посередине окна: выезжает с замедлением и
   падает обратно с разгоном. Без анимаций — просто появляется и исчезает. */
.egg-pony {
  position: absolute;
  left: 50%;
  bottom: 0;
  z-index: 70;
  width: min(46%, 640px);
  transform: translateX(-50%);
  pointer-events: none;
}

.egg-pony-enter-active {
  transition: transform 0.45s var(--l-ease);
}

.egg-pony-leave-active {
  transition: transform 0.4s var(--l-ease-in);
}

.egg-pony-enter-from,
.egg-pony-leave-to {
  transform: translate(-50%, 100%);
}

.launcher-topbar {
  position: absolute;
  top: 0;
  left: 0;
  right: 0;
  height: var(--l-topbar-h);
  display: flex;
  align-items: center;
  /* От значка «закрыть» до края окна — тот же шаг шапки. */
  padding-right: calc(var(--l-header-gap) - var(--l-header-inset));
  gap: var(--l-header-gap);
  z-index: 20;
  -webkit-app-region: drag;

  /* Кружок занимает колонку значков, текст — фиксированные колонки.
     Никаких отрицательных отступов и измерений ширины шрифта. */
  /* Логотип ведёт себя как в старой версии: под курсором точка
     увеличивается, двойной щелчок плавно перекрашивает кольцо и точку.
     В области перетаскивания окна Electron не присылает наведение и
     двойной щелчок разворачивает окно, поэтому логотип из неё вынут. */
  /* Ширина блока задана явно (кружок с полями колонки + «HLSR» + «BETA»):
     при выдвинутой колонке он растёт до её ширины, и вкладки уезжают
     вместе с её краем. */
  &__brand {
    display: flex;
    align-items: center;
    flex: none;
    width: calc(var(--l-rail-w) + 164px);
    user-select: none;
    -webkit-app-region: no-drag;
    transition: width var(--l-dur-swipe) var(--l-ease-swipe);
  }

  &__logo-mark {
    flex: none;
    width: var(--l-brand-mark);
    height: var(--l-brand-mark);
    margin: 0 calc((var(--l-rail-w) - var(--l-brand-mark)) / 2);
    transition: color 0.5s var(--l-ease), margin var(--l-dur-swipe) var(--l-ease-swipe);
    --logo-color: var(--l-text);
    color: var(--logo-color);
    box-shadow: inset 0 0 0 2px currentColor;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: default;
    /* Свой слой: при перекраске сглаженные края кольца иначе оставляли
       на шапке не стёртые пиксели (видно при дробном масштабе экрана). */
    will-change: transform;
  }

  /* 16 px в кольце 40 px — пропорции старого логотипа (20 в 48);
     под курсором точка растёт так же, как там: 20 → 26. */
  &__logo-dot {
    width: 16px;
    height: 16px;
    background: currentColor;
    border-radius: 50%;
    transition: transform var(--l-dur) var(--l-ease-spring), background-color 0.5s var(--l-ease);
  }

  &__logo-text {
    width: 108px;
    margin-left: 16px;
    transition: margin var(--l-dur-swipe) var(--l-ease-swipe), width var(--l-dur-swipe) var(--l-ease-swipe), letter-spacing var(--l-dur-swipe) var(--l-ease-swipe);
    color: var(--l-text);
    font-size: 28px;
    font-weight: 700;
    line-height: 32px;
    letter-spacing: 8px;
  }

  &__logo-mark:hover &__logo-dot {
    transform: scale(1.3);
  }

  /* «BETA» стоит как в логотипе на сайте (Brand.tsx): кегль — 10/22
     кегля «HLSR», верх букв на 0.11 em выше верха «HLSR», начало — через
     0.38 em после «R». «HLSR» заканчивается разрядкой (8 px), поэтому
     отступ от его блока меньше. Ширина задана так, чтобы блок кончался
     на «A» и на сетке 4 px (244 px от края): по тексту она дробная и
     сдвигала бы вкладки и всё, что правее. */
  &__logo-beta {
    align-self: flex-start;
    width: 38px;
    margin: 6px 0 0 2px;
    overflow: visible;
    color: var(--l-text-dim);
    font-size: 13px;
    font-weight: 700;
    line-height: 1;
    text-align: left;
    letter-spacing: 0.08em;
  }

  &__tabs {
    display: flex;
    align-items: center;
    height: 48px;
    /* Поле вкладки (половина шага) уходит в промежуток: от «BETA» до
       текста «Главной» ровно шаг шапки. */
    margin-left: calc(var(--l-header-gap) / -2);
    -webkit-app-region: no-drag;
  }

  /* Линия под вкладкой — ровно по ширине текста. */
  &__tab {
    position: relative;
    flex: none;
    height: 48px;
    padding: 0 calc(var(--l-header-gap) / 2);
    border: none;
    background: none;
    color: var(--l-text-faint);
    font-family: inherit;
    font-size: 17px;
    font-weight: 500;
    line-height: 24px;
    cursor: pointer;
    transition: color var(--l-fast);

    &::after {
      content: "";
      position: absolute;
      left: calc(var(--l-header-gap) / 2);
      right: calc(var(--l-header-gap) / 2);
      bottom: 0;
      height: 4px;
      border-radius: 2px;
      background: var(--l-accent);
      opacity: 0;
      transform: scaleX(0.4);
      transition: opacity var(--l-fast), transform var(--l-med);
    }

    &:hover {
      color: var(--l-text-dim);
    }

    &.active {
      color: var(--l-text);
      cursor: default;

      &::after {
        opacity: 1;
        transform: none;
      }
    }

    &:focus-visible {
      outline: 2px solid var(--l-accent);
      outline-offset: -8px;
      border-radius: var(--l-radius-sm);
    }
  }

  /* Вкладки шириной по тексту, с полями по 16 px для клика: поля
     компенсируются отступами, и между «Главной», «Играми», поиском и
     значком колокольчика везде ровно 32 px (у кнопки колокольчика 16 px
     до значка внутри). */
  /* Поиск — компактное поле у правого края: место между ним и вкладками
     остаётся под перетаскивание окна. Ширина кратна 4. */
  /* Поиск занимает всё место между вкладками и значками: промежутки по
     обе стороны — тот же шаг (поле вкладки и поле кнопки вычитаются). */
  &__search {
    flex: 1 1 288px;
    min-width: 152px;
    margin-left: calc(var(--l-header-gap) / -2);
    margin-right: calc(var(--l-header-inset) * -1);
    -webkit-app-region: no-drag;
  }

  /* Промежутки справа считаются от значков: между значками шаг минус
     поля двух кнопок, разделитель — посередине шага между колокольчиком
     и аватаром, стрелка меню стоит в 8 px от края своей кнопки. */
  &__right {
    display: flex;
    align-items: center;
    gap: calc(var(--l-header-gap) - var(--l-header-inset) * 2);
    flex: none;
    -webkit-app-region: no-drag;
  }

  /* Линия не занимает места в ряду: иначе колокольчик слева от неё стоял
     бы на пиксель вне сетки */
  &__sep {
    position: relative;
    margin: 0 calc(var(--l-header-inset) * 2 - var(--l-header-gap) / 2) 0 calc(var(--l-header-inset) - var(--l-header-gap) / 2);
    width: 0;
    height: 32px;

    &::after {
      content: "";
      position: absolute;
      inset: 0 auto 0 0;
      width: 1px;
      background: var(--l-border);
    }
  }

  &__window {
    margin-left: calc(var(--l-header-inset) - 8px);
  }
}


/* Загрузка FFmpeg в окне перед «Демо». */
.ffgate {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin-top: 14px;

  &__error { color: var(--l-level-error); font-size: 12.5px; line-height: 1.45; }
  &__head { display: flex; justify-content: space-between; color: var(--l-text-dim); font-size: 12.5px; font-variant-numeric: tabular-nums; }
  &__bar { display: block; height: 6px; border-radius: 3px; background: rgba(255, 255, 255, 0.12); overflow: hidden; }
  &__fill { display: block; height: 100%; background: var(--accent); transition: width var(--l-med); }
}
</style>
