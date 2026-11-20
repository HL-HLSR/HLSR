<template>
  <div class="settings-modal" @mousedown.self="$emit('close')">
    <div
      class="settings-modal__window"
      role="dialog"
      aria-modal="true"
      :aria-label="$t('#UI_SETTINGS')"
      ref="window"
    >
      <button
        type="button"
        class="settings-modal__close"
        :title="$t('#L_CLOSE')"
        :aria-label="$t('#L_CLOSE')"
        @click="$emit('close')"
      >
        <i class="fas fa-times" aria-hidden="true"></i>
      </button>

      <!-- Шапка как у профиля: плитка со значком, крупный заголовок и
           вкладки под ним. -->
      <header class="settings-modal__hero">
        <div class="settings-modal__identity">
          <div class="settings-modal__badge" aria-hidden="true">
            <i class="fas fa-cog"></i>
          </div>
          <div class="settings-modal__who">
            <h2 class="settings-modal__title">{{ $t("#UI_SETTINGS") }}</h2>
            <div class="settings-modal__meta">
              <span class="settings-modal__chip">HLSR v{{ version }} · {{ isLinux ? "Linux" : "Windows" }}</span>
            </div>
          </div>
        </div>

        <!-- Вкладки сверху, как в профиле: подсветка перетекает к нажатой
             вкладке (tab-ink.js), стрелки переключают вкладки. -->
        <nav
          ref="tabbar"
          class="settings-modal__tabs"
          role="tablist"
          :aria-label="$t('#UI_SETTINGS')"
          @keydown="onTabKey"
        >
          <button
            v-for="s in sections"
            :key="s.id"
            :ref="(el) => setTabRef(s.id, el)"
            type="button"
            role="tab"
            class="settings-modal__tab"
            :class="{ 'is-active': section === s.id }"
            :aria-selected="section === s.id ? 'true' : 'false'"
            :tabindex="section === s.id ? 0 : -1"
            @click="section = s.id"
          >
            <i :class="s.icon" aria-hidden="true"></i>
            <span>{{ $t(s.title) }}</span>
          </button>
          <span v-if="ink" class="settings-modal__tab-ink l-ink" :class="'is-' + inkMotion" :style="inkStyle" aria-hidden="true"></span>
        </nav>
      </header>

      <!-- Раздел уезжает в сторону, противоположную нажатой вкладке, новый
           приезжает с её стороны (l-slide-*, launcher.css). Оба стоят в
           одной ячейке сетки, у каждого своя прокрутка. -->
      <div class="settings-modal__body">
        <transition :name="'l-slide-' + slideDir">
          <div v-if="section === 'general'" key="general" class="settings-modal__section" role="tabpanel">
            <h4 class="settings-group">{{ $t("#L_S_LAUNCHER_GROUP") }}</h4>

            <!-- Язык переехал сюда из шапки лаунчера -->
            <div class="settings-row">
              <div class="settings-row__text">
                <span class="settings-row__label">{{ $t("#L_S_LANGUAGE") }}</span>
                <span class="settings-row__hint">{{ $t("#L_S_LANGUAGE_HINT") }}</span>
              </div>
              <UiDropdown
                :items="languages"
                :value="language"
                align="right"
                @input="language = $event"
              >
                <template #toggle="{ open }">
                  <span class="settings-select" :class="{ open: open }">
                    <img class="settings-lang__flag" :src="flags[language]" alt="" />
                    <span>{{ languageLabel }}</span>
                    <i class="fas fa-chevron-down"></i>
                  </span>
                </template>
                <template #item="{ item }">
                  <img class="settings-lang__flag" :src="flags[item.id]" alt="" />
                  <span class="ui-dropdown__label">{{ item.label }}</span>
                </template>
              </UiDropdown>
            </div>

            <h4 class="settings-group">{{ $t("#L_S_WHILE_PLAYING") }}</h4>

            <!-- Окно возвращается при любом завершении игры, в том числе
                 при падении. Пока оно спрятано, в области уведомлений
                 висит значок с пунктом «Показать HLSR». -->
            <div class="settings-row">
              <div class="settings-row__text">
                <span class="settings-row__label">{{ $t("#L_S_HIDE_PLAYING") }}</span>
                <span class="settings-row__hint">{{ $t("#L_S_HIDE_PLAYING_HINT") }}</span>
              </div>
              <label class="settings-switch">
                <input type="checkbox" v-model="draft.hideWhilePlaying" :aria-label="$t('#L_S_HIDE_PLAYING')" />
                <span class="settings-switch__track"
                  ><span class="settings-switch__thumb"></span
                ></span>
              </label>
            </div>



            <h4 class="settings-group">{{ $t("#L_S_INTEGRATIONS") }}</h4>

            <div class="settings-row">
              <div class="settings-row__text">
                <span class="settings-row__label">{{
                  $t("#L_S_DISCORD_RPC")
                }}</span>
                <span class="settings-row__hint">{{
                  $t("#L_S_DISCORD_RPC_HINT")
                }}</span>
                <span class="settings-row__hint">{{ discordRpcStatusText }}</span>
              </div>
              <label class="settings-switch">
                <input
                  type="checkbox"
                  :aria-label="$t('#L_S_DISCORD_RPC')"
                  :checked="discordRpc.enabled"
                  @change="setDiscordRpc($event.target.checked)"
                />
                <span class="settings-switch__track"
                  ><span class="settings-switch__thumb"></span
                ></span>
              </label>
            </div>
          </div>


          <div v-else-if="section === 'downloads'" key="downloads" class="settings-modal__section" role="tabpanel">
            <div class="settings-row column">
              <div class="settings-row__text">
                <span class="settings-row__label">{{
                  $t("#L_S_INSTALL_PATH")
                }}</span>
              </div>
              <div class="settings-path" :class="{ invalid: pathError }">
                <input
                  type="text"
                  class="settings-path__input"
                  name="installPath"
                  autocomplete="off"
                  :aria-label="$t('#L_S_INSTALL_PATH')"
                  v-model="draft.installPath"
                  spellcheck="false"
                  @input="pathError = null; pathCyrillic = false"
                />
                <button
                  type="button"
                  class="settings-path__browse"
                  :title="$t('#L_S_BROWSE')"
                  :aria-label="$t('#L_S_BROWSE')"
                  @click="browseInstallPath"
                >
                  <svg
                    class="settings-path__icon"
                    viewBox="0 0 24 24"
                    aria-hidden="true"
                  >
                    <path
                      d="M10 4H4c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V8c0-1.1-.9-2-2-2h-8l-2-2z"
                      fill="currentColor"
                    />
                  </svg>
                </button>
              </div>
              <span v-if="pathError" class="settings-path__error">{{
                pathError
              }}</span>
              <span v-else-if="pathCyrillic" class="settings-path__warning">{{
                $t("#UI_FOUND_CYRILLIC")
              }}</span>
            </div>

            <div class="settings-row">
              <div class="settings-row__text">
                <span class="settings-row__label">{{
                  $t("#L_S_SPEED_LIMIT")
                }}</span>
                <span class="settings-row__hint">{{ $t("#L_S_SPEED_LIMIT_HINT") }}</span>
                <span v-if="downloadError" class="settings-path__error" role="alert">{{ downloadError }}</span>
              </div>
              <UiDropdown
                :items="speedItems"
                :value="draft.speedLimit"
                align="right"
                @input="draft.speedLimit = $event; downloadError = null"
              >
                <template #toggle="{ open }">
                  <span class="settings-select" :class="{ open: open }">
                    <span>{{ speedLabel(draft.speedLimit) }}</span>
                    <i class="fas fa-chevron-down" aria-hidden="true"></i>
                  </span>
                </template>
              </UiDropdown>
            </div>

          </div>


          <!-- Раздел только для Linux. Оформлен теми же строками настроек, что и
               остальные: свой крупный заголовок, моноширинная пометка и
               «маршрутная» полоса выбивались из остального окна и повторяли
               подсказку про общий префикс. -->
          <div v-else-if="section === 'proton'" key="proton" class="settings-modal__section proton" role="tabpanel">
            <div class="settings-row column">
              <div class="settings-row__text">
                <span class="settings-row__label">{{ $t('#L_S_PROTON_TITLE') }}</span>
                <span class="settings-row__hint">{{ $t('#L_S_PROTON_HINT') }}</span>
              </div>

              <div class="proton__choices" role="radiogroup" :aria-label="$t('#L_S_PROTON')">
                <label
                  v-for="item in protonStatus.installed"
                  :key="item.path"
                  class="proton__choice"
                  :class="{ selected: protonChoice === item.path }"
                >
                  <input type="radio" name="proton" :value="item.path" v-model="protonChoice" />
                  <span class="proton__choice-icon"><i class="fas fa-flask" aria-hidden="true"></i></span>
                  <span class="proton__choice-body">
                    <strong class="proton__choice-title">
                      <span class="proton__choice-name" :title="item.name">{{ item.name }}</span>
                      <span v-if="isFreshProton(item)" class="proton__badge">{{ $t('#L_S_PROTON_UPDATED_BADGE') }}</span>
                    </strong>
                    <small>{{ item.recommended ? $t('#L_S_PROTON_RECOMMENDED') : $t('#L_S_PROTON_SYSTEM') }}</small>
                    <code :title="item.path">{{ item.path }}</code>
                  </span>
                  <span class="proton__check"><i class="fas fa-check" aria-hidden="true"></i></span>
                </label>
                <p v-if="!protonStatus.installed.length" class="proton__empty">{{ $t('#L_S_PROTON_EMPTY') }}</p>
              </div>
            </div>

            <!-- Итог проверки обновлений Proton Wineland за этот запуск. -->
            <p v-if="protonUpdateText" class="proton__update" :class="protonUpdateTone">
              <i :class="protonUpdateTone === 'warn' ? 'fas fa-exclamation-triangle' : protonUpdateTone === 'fresh' ? 'fas fa-arrow-circle-up' : 'fas fa-check-circle'" aria-hidden="true"></i>
              <span>{{ protonUpdateText }}</span>
            </p>

            <div v-if="protonBusy" class="proton__progress" role="status">
              <span>{{ protonPhaseText }}</span><b>{{ protonStatus.percent }}%</b>
              <div class="proton__progress-track"><span :style="{ width: protonStatus.percent + '%' }"></span></div>
            </div>
            <p v-if="protonError || protonStatus.error" class="proton__error" role="alert">
              {{ $t('#L_S_PROTON_ERROR') }}: {{ protonError || protonStatus.error }}
            </p>

            <div class="proton__actions">
              <button v-if="protonBusy" type="button" class="settings-btn ghost" @click="cancelProton">
                <i class="fas fa-times" aria-hidden="true"></i>{{ $t('#L_CANCEL') }}
              </button>
              <button v-if="protonUpdate.available" type="button" class="settings-btn ghost" :disabled="protonBusy" @click="updateProton">
                <i class="fas fa-arrow-circle-down" aria-hidden="true"></i>{{ $t('#L_S_PROTON_UPDATE') }}
              </button>
              <button type="button" class="settings-btn ghost" :disabled="protonBusy" @click="downloadProton">
                <i class="fas fa-download" aria-hidden="true"></i>{{ $t('#L_S_PROTON_DOWNLOAD') }}
              </button>
              <button type="button" class="settings-btn ghost" :disabled="protonBusy" @click="addCustomProton">
                <i class="fas fa-folder-plus" aria-hidden="true"></i>{{ $t('#L_S_PROTON_ADD') }}
              </button>
              <button
                type="button"
                class="settings-btn ghost proton__refresh"
                :title="$t('#L_S_PROTON_REFRESH')"
                :aria-label="$t('#L_S_PROTON_REFRESH')"
                @click="refreshProtons"
              >
                <i class="fas fa-sync-alt" aria-hidden="true"></i>
              </button>
            </div>
          </div>

          <div v-else-if="section === 'interface'" key="interface" class="settings-modal__section" role="tabpanel">
            <!-- Тема применяется сразу, без «Сохранить»; черновик следит за ней,
                 чтобы «Сохранить» не вернул прежнюю. -->
            <div class="settings-row column">
              <div class="settings-row__text">
                <span class="settings-row__label">{{ $t("#L_PZ_THEMES") }}</span>
              </div>
              <LauncherThemePicker :current-theme="draft.launcherTheme" :current-accent="draft.paperAccent" @theme="onTheme" />
            </div>

            <!-- Масштаб: окно и интерфейс растут вместе, раскладка не
                 меняется (src/main/window-size.js, scaleLayout). -->
            <div class="settings-row">
              <div class="settings-row__text">
                <span class="settings-row__label">{{ $t("#L_S_UI_SCALE") }}</span>
                <span class="settings-row__hint">{{ $t("#L_S_UI_SCALE_HINT") }}</span>
              </div>
              <UiDropdown
                :items="uiScaleItems"
                :value="uiScale"
                align="right"
                @input="uiScale = $event"
              >
                <template #toggle="{ open }">
                  <span class="settings-select" :class="{ open: open }">
                    <span>{{ uiScaleLabel }}</span>
                    <i class="fas fa-chevron-down"></i>
                  </span>
                </template>
              </UiDropdown>
            </div>

            <div class="settings-row">
              <div class="settings-row__text">
                <span class="settings-row__label">{{ $t("#L_S_FULLSCREEN_BUTTON") }}</span>
                <span class="settings-row__hint">{{ $t("#L_S_FULLSCREEN_BUTTON_HINT") }}</span>
              </div>
              <label class="settings-switch">
                <input type="checkbox" v-model="draft.fullscreenButton" :aria-label="$t('#L_S_FULLSCREEN_BUTTON')" />
                <span class="settings-switch__track"
                  ><span class="settings-switch__thumb"></span
                ></span>
              </label>
            </div>

            <div class="settings-row">
              <div class="settings-row__text">
                <span class="settings-row__label">{{
                  $t("#L_S_ANIMATIONS")
                }}</span>
                <span class="settings-row__hint">{{
                  $t("#L_S_ANIMATIONS_HINT")
                }}</span>
              </div>
              <label class="settings-switch">
                <input type="checkbox" v-model="draft.animations" :aria-label="$t('#L_S_ANIMATIONS')" />
                <span class="settings-switch__track"
                  ><span class="settings-switch__thumb"></span
                ></span>
              </label>
            </div>

            <!-- Аппаратное ускорение отрисовки. Применяется только при
                 следующем запуске: Electron принимает выключение до
                 готовности приложения, и об этом сказано прямо в подсказке —
                 иначе человек щёлкает, не видит разницы и щёлкает обратно. -->
            <div class="settings-row">
              <div class="settings-row__text">
                <span class="settings-row__label">{{ $t("#L_S_GPU") }}</span>
                <span class="settings-row__hint">{{ $t("#L_S_GPU_HINT") }}</span>
                <span v-if="gpuError" class="settings-path__error" role="alert">{{ gpuError }}</span>
              </div>
              <label class="settings-switch">
                <input type="checkbox" v-model="gpuAcceleration" :aria-label="$t('#L_S_GPU')" @change="gpuError = null" />
                <span class="settings-switch__track"
                  ><span class="settings-switch__thumb"></span
                ></span>
              </label>
            </div>
          </div>


          <div v-else-if="section === 'diagnostics'" key="diagnostics" class="settings-modal__section diag" role="tabpanel">
            <div class="diag__toolbar">

              <button
                type="button"
                class="settings-btn primary diag__run"
                :disabled="diagRunning"
                @click="runDiagnostics"
              >
                <i
                  :class="diagRunning ? 'fas fa-circle-notch fa-spin' : 'fas fa-play'"
                  aria-hidden="true"
                ></i>
                {{ diagRunning ? $t("#L_DG_RUNNING") : diagFinished ? $t("#L_DG_AGAIN") : $t("#L_DG_RUN") }}
              </button>
              <span class="settings-modal__footer-spacer"></span>
              <!-- Пока идёт проверка, выгружать нечего: на месте экспорта
                   стоит отмена, и три кнопки не теснятся в одном ряду. -->
              <button
                v-if="diagRunning"
                type="button"
                class="settings-btn"
                @click="cancelDiagnostics"
              >
                <i class="fas fa-times" aria-hidden="true"></i>
                {{ $t("#L_DG_CANCEL") }}
              </button>
              <button v-else type="button" class="settings-btn ghost" @click="openExportPreview">
                <i class="fas fa-file-export" aria-hidden="true"></i>
                {{ $t("#L_S_EXPORT_DIAGNOSTICS") }}
              </button>
            </div>


            <div
              v-if="exportPreviewOpen"
              ref="exportPanel"
              tabindex="-1"
              class="diag__export"
            >
              <span class="diag__export-title">{{ $t("#L_DG_EXPORT_TITLE") }}</span>
              <p class="diag__export-text">{{ $t("#L_DG_EXPORT_INCLUDES") }}</p>
              <p class="diag__export-text muted">{{ $t("#L_DG_EXPORT_EXCLUDES") }}</p>
              <label v-if="crashInfo && crashInfo.dumpPath" class="diag__export-dump">
                <input type="checkbox" v-model="exportIncludeDump" />
                <span>{{ $t("#L_DG_EXPORT_DUMP") }}</span>
              </label>
              <div class="diag__export-actions">
                <button type="button" class="settings-btn" @click="exportPreviewOpen = false">
                  {{ $t("#L_CANCEL") }}
                </button>
                <button
                  type="button"
                  class="settings-btn primary"
                  :disabled="exportBusy"
                  @click="exportZip"
                >
                  {{ $t("#L_DG_EXPORT_GO") }}
                </button>
              </div>
            </div>

            <p v-if="diagError" class="data__error" role="alert">{{ diagError }}</p>

            <div v-if="!diagChecks.length && !diagRunning && !diagError" class="diag__empty">
              {{ $t("#L_DG_EMPTY") }}
            </div>


            <transition-group name="diag-in" tag="div" class="diag__groups">
            <div v-for="group in diagGroups" :key="group.id" class="diag__group">
              <span class="diag__group-title">{{ $t(group.title) }}</span>
              <div
                v-for="item in group.items"
                :key="item.id"
                class="diag__row"
                :class="item.status"
              >
                <i class="diag__row-icon" :class="statusIcon(item.status)" aria-hidden="true"></i>
                <span class="diag__row-name">{{ diagName(item.id) }}</span>
                <span class="diag__row-value">
                  {{ diagDetail(item) }}
                </span>
                <details v-if="item.code" class="diag__row-details">
                  <summary>{{ $t("#L_UA_DETAILS") }}</summary>
                  <code>{{ item.code }}</code>
                  <p class="diag__row-hint">{{ diagHint(item.code) }}</p>
                </details>
              </div>
            </div>
            </transition-group>

            <div class="diag__links">
              <button type="button" class="settings-about__link" @click="openLogs">
                <i class="fas fa-folder"></i>
                <span>{{ $t("#L_S_OPEN_LOGS") }}</span>
              </button>
              <button type="button" class="settings-about__link" @click="openDiscord">
                <i class="fab fa-discord"></i>
                <span>Discord</span>
              </button>
            </div>

          </div>


          <div v-else key="about" class="settings-modal__section about" role="tabpanel">

            <div class="settings-about__head">
              <img
                class="settings-about__logo"
                :src="logoSrc"
                alt="HLSR"
                width="56"
                height="56"
              />
              <div class="settings-about__titles">
                <span class="settings-about__name"><span class="settings-about__word">HLSR</span><span class="settings-about__beta">BETA</span></span>
                <span class="settings-about__version">v{{ version }}</span>
              </div>

              <!-- Ссылки — в строке с логотипом, справа: отдельный блок с
                   заголовком «Ссылки» не давал вкладке поместиться без
                   прокрутки. -->
              <div class="settings-about__links" role="group" :aria-label="$t('#L_S_LINKS')">
                <button
                  type="button"
                  class="settings-about__link"
                  :title="$t('#L_S_WEBSITE')"
                  @click="openLink('site')"
                >
                  <i class="fas fa-globe" aria-hidden="true"></i>
                  <span>{{ $t("#L_S_WEBSITE") }}</span>
                </button>
                <button
                  type="button"
                  class="settings-about__link"
                  :title="$t('#L_S_GITHUB')"
                  @click="openLink('github')"
                >
                  <i class="fab fa-github" aria-hidden="true"></i>
                  <span>{{ $t("#L_S_GITHUB") }}</span>
                </button>
                <button
                  type="button"
                  class="settings-about__link"
                  title="Discord"
                  @click="openLink('discord')"
                >
                  <i class="fab fa-discord" aria-hidden="true"></i>
                  <span>Discord</span>
                </button>
              </div>
            </div>
            <p class="settings-about__text">{{ $t("#L_S_ABOUT_TEXT") }}</p>

            <section class="settings-team" aria-labelledby="settings-team-title">
              <h2 class="settings-group" id="settings-team-title">{{ $t("#L_S_AUTHORS") }}</h2>
              <ul class="settings-team__list">
                <li v-for="person in team" :key="person.login">
                  <button
                    type="button"
                    class="settings-team__person"
                    :title="$t('#L_S_AUTHOR_GITHUB', person.login)"
                    @click="openLink(person.linkKey)"
                  >
                    <img class="settings-team__avatar" :src="person.avatar" alt="" width="44" height="44" />
                    <span class="settings-team__identity">
                      <span class="settings-team__name">{{ person.name || person.login }}</span>
                      <span class="settings-team__role">{{ $t(person.role) }}</span>
                    </span>
                    <i class="fas fa-external-link-alt settings-team__external" aria-hidden="true"></i>
                  </button>
                </li>
              </ul>
            </section>
          </div>
        </transition>
      </div>

      <footer class="settings-modal__footer">
        <button
          type="button"
          class="settings-btn ghost"
          @click="resetDraft"
        >
          {{ $t("#L_RESET") }}
        </button>
        <span class="settings-modal__footer-spacer"></span>
        <button type="button" class="settings-btn" @click="$emit('close')">
          {{ $t("#L_CANCEL") }}
        </button>
        <button type="button" class="settings-btn primary" @click="submit">
          {{ $t("#L_SAVE") }}
        </button>
      </footer>
    </div>
  </div>
</template>

<script>
import UiDropdown from "./UiDropdown";
import LauncherThemePicker from "./LauncherThemePicker";
import uiState from "./ui-state";
import { trapFocus } from "./focus-trap";
import { inkBox, direction } from "./tab-ink";
import prefsStore, { DEFAULT_INSTALL_PATH } from "./launcher-prefs";
import gpuPreference from "./gpu-preference";
import uiScalePreference from "./ui-scale";
import { TEAM } from "./launcher-credits";
const { SPEED_LIMITS } = require("../../../shared/download-settings");

const { ipcRenderer } = window.hlsrElectron;

// Proton и umu есть только в Linux; в Windows вместо них нативный модуль Steam.
const IS_LINUX = window.hlsrElectron.platform === "linux";

export default {
  name: "SettingsModal",
  components: { UiDropdown, LauncherThemePicker },
  emits: ["apply", "close", "reset", "toast", "theme"],
  props: {
    prefs: {
      type: Object,
      required: true,
    },
    installPath: {
      type: String,
      required: true,
    },
    initialSection: {
      type: String,
      default: "general",
    },
  },
  data() {
    return {
      section: this.initialSection === "data" ? "general" : this.initialSection,
      language: Object.keys(this.$localization.locales)[this.$localization.lang] || "en",
      languages: [
        { id: "ru", label: "Русский" },
        { id: "en", label: "English" },
      ],
      flags: {
        ru: require("@/assets/launcher/flag-ru.svg"),
        en: require("@/assets/launcher/flag-en.svg"),
      },
      draft: Object.assign({}, this.prefs, {
        installPath: this.installPath,
      }),
      gpuAcceleration: gpuPreference.read(),
      uiScale: uiScalePreference.read(),
      gpuError: null,
      discordRpc: { enabled: false, state: "disabled", code: null },
      pathError: null,
      downloadError: null,
      pathCyrillic: false,
      exportingDiagnostics: false,
      version: ipcRenderer.sendSync("app-version"),
      logoSrc: require("@/assets/logo.png"),
      team: TEAM,
      sections: [
        { id: "general", icon: "fas fa-sliders-h", title: "#L_S_GENERAL" },
        { id: "downloads", icon: "fas fa-download", title: "#L_S_DOWNLOADS" },
        ...(IS_LINUX ? [{ id: "proton", icon: "fas fa-flask", title: "#L_S_PROTON" }] : []),
        { id: "interface", icon: "fas fa-palette", title: "#L_S_INTERFACE" },
        { id: "diagnostics", icon: "fas fa-stethoscope", title: "#L_S_DIAGNOSTICS" },
        { id: "about", icon: "fas fa-info-circle", title: "#L_S_ABOUT" },
      ],
      diagChecks: [],
      diagRunning: false,
      diagError: null,
      diagFinished: false,
      diagRunId: null,
      exportPreviewOpen: false,
      exportIncludeDump: false,
      exportBusy: false,
      exportOpening: false,
      crashInfo: null,
      lastFocused: null,
      isLinux: IS_LINUX,
      protonStatus: { installed: [], selected: null, phase: "idle", percent: 0, error: null },
      protonChoice: null,
      protonError: null,
      ink: null,
      inkMotion: "still",
      slideDir: "next",
    };
  },
  watch: {
    section(next, previous) {
      this.slideDir = direction(this.sections.map((item) => item.id), previous, next);
      this.$nextTick(() => this.measureInk(this.slideDir));
      if (next === "proton") this.refreshProtons();
    },
  },
  computed: {
    uiScaleItems() {
      return uiScalePreference.SCALES.map((scale) => ({
        id: scale,
        label: `${Math.round(scale * 100)} %` + (scale === uiScalePreference.DEFAULT_SCALE ? ` (${this.$t("#L_S_UI_SCALE_DEFAULT")})` : ""),
      }));
    },
    uiScaleLabel() {
      return `${Math.round(this.uiScale * 100)} %`;
    },
    inkStyle() {
      return { left: this.ink.left + "px", right: this.ink.right + "px" };
    },
    languageLabel() {
      const item = this.languages.find((entry) => entry.id === this.language);
      return item ? item.label : this.language;
    },
    protonBusy() {
      return ["checking", "downloading", "extracting"].includes(this.protonStatus.phase);
    },
    protonUpdate() {
      return this.protonStatus.update || {};
    },
    protonUpdateTone() {
      const update = this.protonUpdate;
      if (update.available || update.updatedTo) return "fresh";
      if (update.error) return "warn";
      return "ok";
    },
    protonUpdateText() {
      const update = this.protonUpdate;
      const short = (tag) => String(tag || "").replace(/^wineland-/, "");
      if (update.available) return this.$t("#L_S_PROTON_UPDATE_AVAILABLE", short(update.latest));
      if (update.updatedTo)
        return this.$t("#L_S_PROTON_UPDATED", short(update.updatedFrom), short(update.updatedTo));
      if (update.error) return this.$t("#L_S_PROTON_UPDATE_FAILED");
      if (update.checked && update.latest) return this.$t("#L_S_PROTON_UP_TO_DATE", short(update.latest));
      return "";
    },
    protonPhaseText() {
      if (this.protonStatus.phase === "extracting") return this.$t("#L_S_PROTON_EXTRACTING");
      if (this.protonStatus.phase === "checking") return this.$t("#L_S_PROTON_CHECKING");
      return this.$t("#L_S_PROTON_DOWNLOADING");
    },
    discordRpcStatusText() {
      if (!this.discordRpc.enabled) return this.$t("#L_S_DISCORD_RPC_OFF");
      const state = this.discordRpc.state;
      if (state === "connected") return this.$t("#L_S_DISCORD_RPC_CONNECTED");
      if (state === "connecting") return this.$t("#L_S_DISCORD_RPC_CONNECTING");
      if (state === "unavailable") return this.$t("#L_S_DISCORD_RPC_UNAVAILABLE");
      if (state === "retry_wait" || state === "disconnected")
        return this.$t("#L_S_DISCORD_RPC_ERROR");
      return this.$t("#L_S_DISCORD_RPC_CONNECTING");
    },
    speedItems() {
      return SPEED_LIMITS.map((n) => ({
        id: n,
        label: this.speedLabel(n),
      }));
    },
    diagGroups() {
      const order = [
        { id: "hlsr", title: "#L_DG_G_HLSR" },
        { id: "graphics", title: "#L_DG_G_GRAPHICS" },
        { id: "downloads", title: "#L_DG_G_DOWNLOADS" },
        IS_LINUX ? { id: "linux", title: "#L_DG_G_LINUX" } : { id: "windows", title: "#L_DG_G_WINDOWS" },
        { id: "library", title: "#L_DG_G_LIBRARY" },
        { id: "build", title: "#L_DG_G_BUILD" },
        { id: "profile", title: "#L_DG_G_PROFILE" },
        { id: "steam", title: "#L_DG_G_STEAM" },
        { id: "network", title: "#L_DG_G_NETWORK" },
      ];

      return order
        .map((group) => ({
          ...group,
          items: this.diagChecks.filter((c) => c.group === group.id),
        }))
        .filter((group) => group.items.length);
    },

  },
  methods: {
    isFreshProton(item) {
      const to = this.protonUpdate.updatedTo;
      return !!to && item.name === `proton-${to}-x86_64`;
    },
    onProtonState(event, state) {
      const previous = this.protonStatus.selected;
      this.protonStatus = state;
      if (!this.protonChoice || this.protonChoice === previous ||
          !state.installed.some((item) => item.path === this.protonChoice)) this.protonChoice = state.selected;
    },
    async refreshProtons() {
      try {
        const state = await ipcRenderer.invoke("proton-status");
        this.protonStatus = state;
        if (!this.protonChoice || !state.installed.some((item) => item.path === this.protonChoice))
          this.protonChoice = state.selected;
      } catch (error) { this.protonError = error.message; }
    },
    async downloadProton() {
      this.protonError = null;
      try {
        await ipcRenderer.invoke("proton-download");
        await this.refreshProtons();
      } catch (error) { this.protonError = error.message; }
    },
    async updateProton() {
      this.protonError = null;
      try {
        await ipcRenderer.invoke("proton-update");
        await this.refreshProtons();
      } catch (error) { this.protonError = error.message; }
    },
    cancelProton() {
      ipcRenderer.invoke("proton-cancel");
    },
    async addCustomProton() {
      this.protonError = null;
      try {
        const added = await ipcRenderer.invoke("proton-add");
        if (!added) return;
        await this.refreshProtons();
        this.protonChoice = added.path;
      } catch (error) { this.protonError = error.message; }
    },
    async runDiagnostics() {
      if (this.diagRunning) return;
      this.diagRunning = true;
      this.diagError = null;
      this.diagFinished = false;
      this.diagChecks = [];

      try {
        const result = await ipcRenderer.invoke("diagnostics-run");
        if (result && result.checks) {
          this.diagRunId = result.runId || null;
          this.diagChecks = result.checks;
        }
        this.diagFinished = true;
      } catch (error) {
        this.diagError = this.$t("#L_DG_RUN_FAILED");
      } finally {
        this.diagRunning = false;
      }
    },
    cancelDiagnostics() {
      ipcRenderer.invoke("diagnostics-cancel");
    },
    onDiagProgress(e, item) {
      if (!this.diagRunning) return;
      const index = this.diagChecks.findIndex((c) => c.id === item.id);
      if (index === -1) this.diagChecks.push(item);
      else this.diagChecks[index] = item;
    },
    statusIcon(status) {
      if (status === "ok") return "fas fa-check-circle";
      if (status === "warn") return "fas fa-exclamation-triangle";
      if (status === "fail") return "fas fa-times-circle";
      return "fas fa-minus-circle";
    },
    statusText(status) {
      if (status === "ok") return this.$t("#L_DG_OK");
      if (status === "warn") return this.$t("#L_DG_WARN");
      if (status === "fail") return this.$t("#L_DG_FAIL");
      return this.$t("#L_DG_SKIP");
    },
    diagName(id) {
      const key = "#L_DG_" + String(id).toUpperCase().replace(/-/g, "_");
      const text = this.$t(key);
      if (text === key && /^steam-owns-/.test(id)) return this.$t("#L_DG_STEAM_LICENSE");
      return text === key ? String(id).replace(/-/g, " ") : text;
    },
    diagDetail(item) {
      return item.detailKey
        ? this.$t(item.detailKey, ...(item.detailArgs || []))
        : item.detail || this.statusText(item.status);
    },
    diagHint(code) {
      const key = "#L_DGH_" + code;
      const text = this.$t(key);
      return text === key ? "" : text;
    },
    async openExportPreview() {
      if (this.exportOpening) return;
      if (this.exportPreviewOpen) {
        this.revealExportPanel();
        return;
      }

      this.exportOpening = true;
      try {
        this.crashInfo = await ipcRenderer.invoke("game-crash-info", null);
        this.exportIncludeDump = false;
        this.exportPreviewOpen = true;

        await this.$nextTick();
        this.revealExportPanel();
      } finally {
        this.exportOpening = false;
      }
    },
    revealExportPanel() {
      const panel = this.$refs.exportPanel;
      if (!panel) return;

      const container = panel.closest(".settings-modal__section");
      if (container) {
        const top =
          panel.getBoundingClientRect().top -
          container.getBoundingClientRect().top +
          container.scrollTop -
          8;

        const instant =
          window.matchMedia("(prefers-reduced-motion: reduce)").matches ||
          this.draft.animations === false;

        container.scrollTo({
          top: Math.max(0, top),
          behavior: instant ? "auto" : "smooth",
        });
      }

      panel.focus({ preventScroll: true });
    },
    async exportZip() {
      if (this.exportBusy) return;
      this.exportBusy = true;

      try {
        const result = await ipcRenderer.invoke("export-diagnostics-zip", {
          includeDump: this.exportIncludeDump === true,
        });
        this.$emit(
          "toast",
          result && result.ok
            ? this.$t("#L_S_EXPORT_DIAGNOSTICS_OK")
            : result && result.cancelled
              ? this.$t("#L_DG_EXPORT_CANCELLED")
              : this.$t("#L_S_EXPORT_DIAGNOSTICS_FAIL")
        );
        if (result && result.ok) this.exportPreviewOpen = false;
      } finally {
        this.exportBusy = false;
      }
    },
    openDiscord() {
      this.openLink("discord");
    },
    openLink(key) {
      ipcRenderer.invoke("open-external-link", key);
    },
    speedLabel(n) {
      return n === 0 ? this.$t("#L_S_NO_LIMIT") : this.$t("#L_S_MBPS", n);
    },
    openLogs() {
      ipcRenderer.invoke("open-logs-folder");
    },
    async exportDiagnostics() {
      if (this.exportingDiagnostics) return;
      this.exportingDiagnostics = true;

      try {
        const result = await ipcRenderer.invoke("export-diagnostics");
        this.$emit(
          "toast",
          result && result.ok
            ? this.$t("#L_S_EXPORT_DIAGNOSTICS_OK")
            : this.$t("#L_S_EXPORT_DIAGNOSTICS_FAIL")
        );
      } catch (e) {
        this.$emit("toast", this.$t("#L_S_EXPORT_DIAGNOSTICS_FAIL"));
      } finally {
        this.exportingDiagnostics = false;
      }
    },
    async browseInstallPath() {
      let selected = await ipcRenderer.invoke(
        "select-install-folder",
        this.draft.installPath
      );

      if (!selected) return;

      this.draft.installPath = selected;
      this.pathError = null;
      this.checkPath(selected);
    },
    async checkPath(target) {
      try {
        const check = await ipcRenderer.invoke("validate-install-path", target);
        this.pathCyrillic = !!(check && check.ok && check.hasCyrillic);
      } catch (e) {
        this.pathCyrillic = false;
      }
    },
    pathErrorText(code) {
      switch (code) {
        case "no-access":
          return this.$t("#L_S_PATH_NO_ACCESS");
        default:
          return this.$t("#L_S_PATH_INVALID");
      }
    },
    async submit() {
      let check = await ipcRenderer.invoke(
        "validate-install-path",
        this.draft.installPath
      );

      if (!check.ok) {
        this.section = "downloads";
        this.pathError = this.pathErrorText(check.error);
        return;
      }

      this.pathCyrillic = !!check.hasCyrillic;

      if (this.protonChoice && this.protonChoice !== this.protonStatus.selected) {
        try {
          await ipcRenderer.invoke("proton-select", this.protonChoice);
        } catch (error) {
          this.section = "proton";
          this.protonError = error.message;
          return;
        }
      }

      const previousScale = uiScalePreference.read();
      if (previousScale !== this.uiScale) {
        uiScalePreference.write(this.uiScale);
        await uiScalePreference.apply(this.uiScale);
      }

      const previousGpu = gpuPreference.read();
      const gpuChanged = previousGpu !== this.gpuAcceleration;
      if (gpuChanged && !gpuPreference.write(this.gpuAcceleration)) {
        this.section = "interface";
        this.gpuError = this.$t("#L_S_GPU_SAVE_FAILED");
        return;
      }

      try {
        const result = await ipcRenderer.invoke("download-settings-set", this.draft.speedLimit);
        if (!result || !result.ok) throw new Error("Download settings were not saved");
      } catch (error) {
        this.section = "downloads";
        if (gpuChanged) gpuPreference.write(previousGpu);
        this.downloadError = this.$t("#L_S_SPEED_SAVE_FAILED");
        return;
      }

      this.$emit("apply", Object.assign({}, this.draft, { installPath: check.path }), {
        language: this.language,
        gpuChanged,
      });
    },

    formatDate(value) {
      const date = new Date(value);
      return isNaN(date.getTime()) ? "—" : date.toLocaleString();
    },
    async loadDiscordRpc() {
      try {
        const state = await ipcRenderer.invoke("discord-rpc-get");
        if (state && typeof state === "object") this.discordRpc = state;
      } catch (e) {
        /* main ещё не поднял канал — оставляем значения по умолчанию */
      }
    },
    async setDiscordRpc(enabled) {
      this.discordRpc = Object.assign({}, this.discordRpc, { enabled: enabled === true });
      try {
        const state = await ipcRenderer.invoke("discord-rpc-set", enabled === true);
        if (state && typeof state === "object") this.discordRpc = state;
      } catch (e) {
        this.loadDiscordRpc();
      }
    },
    onDiscordRpcState(event, state) {
      if (state && typeof state === "object") this.discordRpc = state;
    },
    onTheme(id, accent) {
      this.draft.launcherTheme = id;
      if (accent) this.draft.paperAccent = accent;
      this.$emit("theme", id, accent);
    },
    resetDraft() {
      this.draft = Object.assign({}, prefsStore.defaults(), {
        toolSplits: Object.assign({}, this.prefs.toolSplits),
        liveSplitGame: this.prefs.liveSplitGame,
        // Тема применяется сразу и в сброс не входит: выбранная в списке — применённая.
        launcherTheme: this.prefs.launcherTheme,
        paperAccent: this.prefs.paperAccent,
        installPath: DEFAULT_INSTALL_PATH,
      });
      this.pathError = null;
      this.downloadError = null;
      this.gpuError = null;
      this.gpuAcceleration = gpuPreference.DEFAULT_ENABLED;
      this.uiScale = uiScalePreference.DEFAULT_SCALE;
      this.$emit("reset");
    },
    setTabRef(id, el) {
      if (el) this.tabEls[id] = el;
      else delete this.tabEls[id];
    },
    // Полоса под активной вкладкой перетекает к новой (tab-ink.js). При
    // изменении размеров окна — без анимации.
    measureInk(motion) {
      const box = inkBox(this.$refs.tabbar, this.tabEls[this.section]);
      if (!box) return;
      this.inkMotion = this.ink && typeof motion === "string" ? motion : "still";
      this.ink = box;
    },
    // Стрелки, Home и End переключают вкладки, фокус идёт за выбранной.
    onTabKey(event) {
      const ids = this.sections.map((item) => item.id);
      let index = ids.indexOf(this.section);
      if (event.key === "ArrowRight") index = (index + 1) % ids.length;
      else if (event.key === "ArrowLeft") index = (index - 1 + ids.length) % ids.length;
      else if (event.key === "Home") index = 0;
      else if (event.key === "End") index = ids.length - 1;
      else return;
      event.preventDefault();
      this.section = ids[index];
      this.$nextTick(() => this.tabEls[this.section] && this.tabEls[this.section].focus());
    },
    onKeydown(e) {
      if (e.key === "Escape") {
        if (uiState.openDropdowns > 0) return;
        e.preventDefault();
        this.$emit("close");
      } else if (e.key === "Tab") {
        trapFocus(this.$refs.window, e);
      }
    },
  },
  created() {
    this.tabEls = {};
  },
  mounted() {
    this.lastFocused = document.activeElement;
    this.measureInk("still");
    window.addEventListener("resize", this.measureInk);
    document.addEventListener("keydown", this.onKeydown);
    ipcRenderer.on("diagnostics-progress", this.onDiagProgress);
    ipcRenderer.on("discord-rpc-state", this.onDiscordRpcState);
    if (IS_LINUX) {
      ipcRenderer.on("proton-state", this.onProtonState);
      this.refreshProtons();
    }
    this.loadDiscordRpc();
    this.$nextTick(() => {
      const tab = this.tabEls[this.section];
      if (tab) tab.focus();
    });
  },
  beforeUnmount() {
    document.removeEventListener("keydown", this.onKeydown);
    window.removeEventListener("resize", this.measureInk);
    ipcRenderer.removeListener("diagnostics-progress", this.onDiagProgress);
    ipcRenderer.removeListener("discord-rpc-state", this.onDiscordRpcState);
    if (IS_LINUX) ipcRenderer.removeListener("proton-state", this.onProtonState);
    if (this.lastFocused && this.lastFocused.focus) this.lastFocused.focus();
  },
};
</script>

<style lang="scss" scoped>
.settings-modal {
  position: absolute;
  inset: 0;
  background: var(--l-modal-scrim);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 50;

  /* Окно того же размера и устройства, что профиль (ProfileModal.vue). */
  &__window {
    position: relative;
    width: min(1040px, calc(100% - 64px));
    height: min(720px, calc(100% - 64px));
    display: flex;
    flex-direction: column;
    background: var(--l-modal-bg);
    border: none;
    border-radius: var(--l-radius);
    box-shadow: inset 0 0 0 1px var(--l-modal-border), 0 24px 72px rgba(0, 0, 0, 0.6);
    overflow: hidden;
  }

  /* Правый край крестика — на линии правого края содержимого (32 px). */
  &__close {
    position: absolute;
    top: 24px;
    right: 32px;
    z-index: 4;
    width: 40px;
    height: 40px;
    padding: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    border: none;
    border-radius: var(--l-radius-sm);
    background: rgba(0, 0, 0, 0.32);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: var(--l-text-dim);
    font-size: 15px;
    cursor: pointer;
    transition: background var(--l-fast), color var(--l-fast);

    &:hover {
      background: rgba(255, 255, 255, 0.1);
      color: var(--l-text);
    }

    &:focus-visible {
      outline: 2px solid var(--l-accent);
      outline-offset: 2px;
    }
  }

  /* ---- Шапка ---- */

  &__hero {
    position: relative;
    flex: none;
    padding: 0 32px;
    box-shadow: inset 0 -1px 0 var(--l-modal-border);
  }

  &__identity {
    position: relative;
    display: flex;
    align-items: center;
    gap: 24px;
    padding-top: 24px;
    padding-right: 56px;
  }

  &__badge {
    flex: 0 0 88px;
    width: 88px;
    height: 88px;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: 18px;
    background:
      radial-gradient(100% 100% at 30% 20%, color-mix(in srgb, var(--l-accent) 22%, transparent) 0%, transparent 70%),
      var(--l-modal-bg);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: var(--l-accent);
    font-size: 36px;
  }

  &__who {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
  }

  &__title {
    margin: 0;
    color: var(--l-text);
    font-size: 28px;
    font-weight: 700;
    line-height: 36px;
    letter-spacing: 0.01em;
  }

  &__meta {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 12px;
    color: var(--l-text-faint);
    font-size: 13px;
    line-height: 20px;
  }

  &__chip {
    padding: 4px 10px;
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.05);
    color: var(--l-text-dim);
    font-size: 12.5px;
    font-variant-numeric: tabular-nums;
  }

  &__tabs {
    position: relative;
    display: flex;
    gap: 8px;
    margin: 20px 0 0 -12px;
  }

  &__tab {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    height: 48px;
    padding: 0 12px;
    border: 0;
    background: none;
    color: var(--l-text-dim);
    font-family: inherit;
    font-size: 13.5px;
    font-weight: 600;
    line-height: 20px;
    white-space: nowrap;
    cursor: pointer;
    transition: color var(--l-fast);

    i {
      font-size: 12px;
      color: var(--l-text-faint);
      transition: color var(--l-fast);
    }

    &:hover { color: var(--l-text); }

    &:focus-visible {
      outline: 2px solid var(--l-accent);
      outline-offset: -4px;
      border-radius: var(--l-radius-sm);
    }

    &.is-active {
      color: var(--l-text);

      i { color: var(--l-accent); }
    }
  }

  /* Перетекает между вкладками (.l-ink в launcher.css). */
  &__tab-ink {
    bottom: 0;
    height: 2px;
    border-radius: 2px 2px 0 0;
    background: var(--l-accent);
    box-shadow: 0 0 12px color-mix(in srgb, var(--l-accent) 60%, transparent);
  }

  /* Разделы стоят в одной ячейке сетки и при смене уезжают на одинаковую
     ширину (l-slide-*); выезжающее за край обрезается. У каждого своя
     прокрутка: прокрутка одного не сдвигает другой. */
  &__body {
    position: relative;
    flex: 1 1 auto;
    min-height: 0;
    display: grid;
    grid-template: minmax(0, 1fr) / minmax(0, 1fr);
    overflow: hidden;
  }

  &__section {
    grid-area: 1 / 1;
    min-height: 0;
    min-width: 0;
    box-sizing: border-box;
    padding: 28px 32px 32px;
    overflow-y: auto;
    overscroll-behavior: contain;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  /* Без отдельной плашки: та же поверхность, что у окна, отделена линией. */
  &__footer {
    display: flex;
    align-items: center;
    flex: none;
    gap: 12px;
    padding: 16px 32px;
    box-shadow: inset 0 1px 0 var(--l-modal-border);
  }

  &__footer-spacer {
    flex: 1;
  }
}

/* Подзаголовок группы внутри раздела. Не карточка и не отдельная страница:
   строка-разделитель в том же непрозрачном стиле, что и остальные настройки. */
.settings-group {
  margin: 32px 0 4px;
  font-size: 12px;
  font-weight: 600;
  line-height: 16px;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--l-text-dim, rgba(255, 255, 255, 0.5));
}

.settings-group:first-child {
  margin-top: 0;
}

.settings-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 20px;
  padding: 16px 0;
  box-shadow: inset 0 -1px 0 rgba(255, 255, 255, 0.045);

  &:last-child {
    box-shadow: none;
  }

  &:first-child {
    padding-top: 0;
  }

  &.column {
    flex-direction: column;
    align-items: stretch;
    gap: 12px;
  }

  &__text {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }

  &__label {
    color: var(--l-text);
    font-size: 14px;
    font-weight: 500;
    line-height: 24px;
  }

  &__hint {
    color: var(--l-text-faint);
    font-size: 12px;
    line-height: 20px;
  }
}

/* Кнопка в конце строки не сжимается в столбик: «Сбросить идентификатор
   статистики» занимала три строки в 170 px. Шире 240 px она не растёт и
   переносится на вторую строку. */
.settings-row > .settings-btn {
  flex: none;
  width: max-content;
  max-width: 240px;
  text-align: center;
}

.settings-link {
  align-self: flex-start;
  margin-top: 2px;
  padding: 0;
  border: 0;
  background: none;
  color: var(--l-accent, #ff7a1c);
  font-size: 12px;
  line-height: 1.4;
  text-decoration: underline;
  cursor: pointer;

  &:hover {
    filter: brightness(1.15);
  }

  &:focus-visible {
    outline: 2px solid var(--l-accent, #ff7a1c);
    outline-offset: 2px;
  }
}

.settings-lang__flag {
  display: block;
  width: 21px;
  height: 14px;
  border-radius: 3px;
  object-fit: cover;
}

.settings-select {
  display: flex;
  align-items: center;
  gap: 12px;
  min-width: 150px;
  justify-content: space-between;
  padding: 9px 14px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--l-border);
  border-radius: var(--l-radius-sm);
  color: var(--l-text);
  font-size: 13px;
  font-weight: 500;
  transition: background var(--l-fast), border-color var(--l-fast);

  i {
    font-size: 10px;
    color: var(--l-text-dim);
    transition: transform var(--l-fast);
  }

  &:hover,
  &.open {
    background: rgba(255, 255, 255, 0.08);
    border-color: var(--l-border-strong);
  }

  &.open i {
    transform: rotate(180deg);
  }
}

.settings-path {
  display: flex;
  align-items: stretch;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--l-border);
  border-radius: var(--l-radius-sm);
  overflow: hidden;
  transition: border-color var(--l-fast), background var(--l-fast);

  &:hover {
    background: rgba(255, 255, 255, 0.07);
  }

  &:focus-within {
    border-color: var(--accent);
  }

  &.invalid {
    border-color: #ff4d5e;
  }

  &__input {
    flex: 1;
    min-width: 0;
    padding: 11px 14px;
    background: transparent;
    border: none;
    color: var(--l-text);
    font-family: inherit;
    font-size: 13px;

    &:focus {
      outline: none;
    }
  }

  &__browse {
    flex: 0 0 42px;
    width: 42px;
    border: none;
    border-left: 1px solid var(--l-border);
    background: transparent;
    color: var(--l-text-dim);
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    transition: background var(--l-fast), color var(--l-fast);

    &:hover {
      background: rgba(255, 255, 255, 0.08);
      color: var(--l-text);
    }

    &:active {
      background: rgba(255, 255, 255, 0.12);
    }

    &:focus-visible {
      outline: 2px solid var(--accent-focus);
      outline-offset: -2px;
    }
  }

  &__icon {
    display: block;
    width: 18px;
    height: 18px;
  }

  &__error {
    color: #ff6b78;
    font-size: 12px;
    line-height: 1.4;
  }

  &__warning {
    color: #ffb37a;
    font-size: 12px;
    line-height: 1.4;
  }
}

.settings-switch {
  position: relative;
  display: inline-flex;
  cursor: pointer;

  input {
    position: absolute;
    opacity: 0;
    width: 100%;
    height: 100%;
    margin: 0;
    cursor: pointer;
  }

  &__track {
    width: 42px;
    height: 24px;
    background: rgba(255, 255, 255, 0.12);
    border-radius: 12px;
    display: flex;
    align-items: center;
    padding: 3px;
    transition: background var(--l-med);
  }

  &__thumb {
    width: 18px;
    height: 18px;
    background: #fff;
    border-radius: 50%;
    transform: translateX(0);
    transition: transform var(--l-med);
  }

  input:checked + .settings-switch__track {
    background: var(--accent);
  }

  input:checked + .settings-switch__track .settings-switch__thumb {
    background: var(--l-switch-thumb-on, #fff);
    transform: translateX(18px);
  }

  input:focus-visible + .settings-switch__track {
    outline: 2px solid var(--accent-focus);
    outline-offset: 2px;
  }
}

.settings-about {
  /* Логотип с названием слева, ссылки справа. В узком окне ссылки
     переносятся под логотип. */
  &__head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 12px 16px;
    margin-bottom: 4px;
  }

  /* Настоящий логотип проекта; собственный знак из CSS больше не рисуется */
  &__logo {
    width: 56px;
    height: 56px;
    border-radius: 12px;
    flex: none;
    object-fit: contain;
  }

  /* «BETA» как в логотипе на сайте (Brand.tsx): кегль — 10/22 кегля
     «HLSR», верх букв чуть выше верха «HLSR», начало — через 0.38 em
     после «R» (за вычетом разрядки «HLSR»). */
  &__beta {
    align-self: flex-start;
    margin: 2px 0 0 3.5px;
    color: var(--l-text-dim);
    font-size: calc(20px * 10 / 22);
    font-weight: 700;
    letter-spacing: 0.08em;
    line-height: 1;
  }

  &__titles {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  &__name {
    display: inline-flex;
    align-items: center;
    align-self: flex-start;
    color: var(--l-text);
    font-size: 20px;
    line-height: 24px;
    font-weight: 700;
    letter-spacing: 0.2em;
  }

  &__version {
    color: var(--l-text-dim);
    font-size: 12.5px;
  }

  &__text {
    color: var(--l-text-dim);
    font-size: 13.5px;
    line-height: 1.6;
    margin: 12px 0 0;
  }

  &__status {
    display: flex;
    align-items: center;
    gap: 9px;
    margin-bottom: 18px;
    padding: 10px 14px;
    border-radius: var(--l-radius-sm);
    background: rgba(255, 122, 28, 0.1);
    border: 1px solid rgba(255, 122, 28, 0.35);
    color: #ffb37a;
    font-size: 12.5px;

    svg {
      display: block;
      width: 16px;
      height: 16px;
      min-width: 16px;
    }

    &.ok {
      background: rgba(55, 200, 113, 0.1);
      border-color: rgba(55, 200, 113, 0.35);
      color: #7fe0a6;
    }
  }

  &__links {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    margin-left: auto;
  }

  &__link {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 9px 16px;
    background: rgba(255, 255, 255, 0.05);
    border: 1px solid var(--l-border);
    border-radius: var(--l-radius-sm);
    color: var(--l-text);
    font-family: inherit;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: background var(--l-fast), border-color var(--l-fast);

    i {
      color: var(--l-text-dim);
    }

    &:hover {
      background: rgba(255, 255, 255, 0.09);
      border-color: var(--l-border-strong);
    }

    &:active {
      background: rgba(255, 255, 255, 0.12);
    }
  }
}

.settings-team {
  margin-top: 20px;

  &__list {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(208px, 1fr));
    gap: 12px;
    list-style: none;
    margin: 12px 0 0;
    padding: 0;
  }

  &__person {
    position: relative;
    display: flex;
    align-items: flex-start;
    gap: 12px;
    width: 100%;
    height: 100%;
    padding: 12px 14px;
    border: 0;
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.04);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: var(--l-text);
    font-family: inherit;
    text-align: left;
    cursor: pointer;
    transition: background var(--l-fast), box-shadow var(--l-fast);

    &:hover {
      background: rgba(255, 255, 255, 0.07);
      box-shadow: inset 0 0 0 1px var(--l-border-strong);
    }

    &:focus-visible {
      outline: 2px solid var(--l-accent);
      outline-offset: 2px;
    }
  }

  &__avatar {
    flex: none;
    width: 44px;
    height: 44px;
    border-radius: var(--l-radius-sm);
    object-fit: cover;
  }

  &__identity {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    padding-right: 8px;
  }

  &__name {
    font-size: 14px;
    font-weight: 600;
    line-height: 20px;
    overflow-wrap: anywhere;
  }

  &__role {
    color: var(--l-text-dim);
    font-size: 12px;
    line-height: 18px;
  }

  &__external {
    position: absolute;
    top: 12px;
    right: 12px;
    color: var(--l-text-faint);
    font-size: 9px;
  }
}

/* Вкладка «Данные». Никакого стекла и размытия: обычные непрозрачные
   блоки на поверхности окна настроек. */
.data {
  gap: 18px;

  &__block {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 16px;
    border: 1px solid var(--l-border);
    border-radius: var(--l-radius-sm);
    background: var(--l-bg-raised);
  }

  &__title {
    font-size: 14px;
    font-weight: 600;
    color: var(--l-text);
  }

  &__text {
    margin: 0;
    font-size: 12.5px;
    line-height: 1.5;
    color: var(--l-text-dim);

    &.muted {
      color: var(--l-text-faint);
    }
  }

  &__action {
    align-self: flex-start;
    margin-top: 4px;
  }

  &__preview {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 12px;
    border: 1px solid var(--l-border);
    border-radius: var(--l-radius-sm);
    background: var(--l-surface-solid);
  }

  &__row {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    font-size: 12.5px;
    color: var(--l-text-dim);

    b {
      color: var(--l-text);
      font-weight: 600;
      text-align: right;
    }
  }

  &__warning {
    margin: 6px 0 0;
    font-size: 12.5px;
    line-height: 1.5;
    color: var(--btn-repair-bg);
  }

  &__buttons {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 8px;
  }

  &__error {
    margin: 0;
    font-size: 12.5px;
    color: var(--btn-danger-bg);
  }

  &__code {
    font-family: Consolas, monospace;
    opacity: 0.85;
  }
}

.diag {
  gap: 14px;

  &__toolbar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 10px;

    .settings-btn { flex: none; white-space: nowrap; }
  }

  &__empty {
    color: var(--l-text-faint);
    font-size: 13px;
    padding: 18px 2px;
  }

  &__group {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  &__group-title {
    color: var(--l-text);
    font-size: 12.5px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    padding: 6px 0;
  }

  &__row {
    display: flex;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 8px;
    padding: 5px 2px 5px 8px;
    border-left: 2px solid var(--l-border);
    font-size: 13px;

    &.ok {
      border-left-color: #2fa84f;
    }
    &.warn {
      border-left-color: #e8a33d;
    }
    &.fail {
      border-left-color: #e0455a;
    }
  }

  &__row-icon {
    font-size: 12px;
    width: 14px;
  }

  .diag__row.ok .diag__row-icon {
    color: #2fa84f;
  }
  .diag__row.warn .diag__row-icon {
    color: #e8a33d;
  }
  .diag__row.fail .diag__row-icon {
    color: #e0455a;
  }

  &__row-name {
    color: var(--l-text);
    font-weight: 500;
  }

  &__row-value {
    color: var(--l-text-dim);
    font-size: 12.5px;
    word-break: break-word;
  }

  &__row-details {
    flex-basis: 100%;

    summary {
      color: var(--l-text-faint);
      font-size: 11.5px;
      cursor: pointer;
    }

    code {
      display: inline-block;
      margin: 4px 0;
      padding: 2px 8px;
      background: rgba(255, 255, 255, 0.05);
      border-radius: 5px;
      color: var(--l-text-dim);
      font-family: Consolas, monospace;
      font-size: 11.5px;
    }
  }

  &__row-hint {
    color: var(--l-text-dim);
    font-size: 12px;
    line-height: 1.45;
    margin: 2px 0 0;
  }

  &__links {
    display: flex;
    gap: 10px;
  }

  &__export {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 14px;
    border: 1px solid var(--l-border-strong);
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.03);

    &:focus:not(:focus-visible) {
      outline: none;
    }
  }

  &__export-title {
    color: var(--l-text);
    font-size: 13.5px;
    font-weight: 600;
  }

  &__export-text {
    color: var(--l-text-dim);
    font-size: 12.5px;
    line-height: 1.5;
    margin: 0;

    &.muted {
      color: var(--l-text-faint);
    }
  }

  &__export-dump {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--l-text);
    font-size: 12.5px;
    cursor: pointer;
  }

  &__export-actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
    margin-top: 4px;
  }
}

.proton {
  gap: 4px;

  &__choices { display: flex; flex-direction: column; gap: 8px; }
  &__choice {
    position: relative; display: flex; align-items: center; gap: 12px; min-width: 0;
    padding: 11px 13px; border: 1px solid var(--l-border-strong);
    border-radius: var(--l-radius-sm); background: rgba(255,255,255,0.025); cursor: pointer;
    transition: border-color var(--l-fast), background var(--l-fast);
  }
  &__choice:hover { background: rgba(255,255,255,0.055); }
  &__choice.selected { border-color: var(--accent); background: var(--accent-soft); }
  &__choice:focus-within { outline: 2px solid var(--accent-focus, #4fc3ff); outline-offset: 2px; }
  &__choice input { position: absolute; opacity: 0; width: 1px; height: 1px; }
  &__choice-icon { display: grid; place-items: center; width: 36px; height: 36px; flex: none;
    border-radius: 9px; background: rgba(var(--accent-rgb, 0, 171, 255), 0.1); color: var(--accent); }
  &__choice-body { display: flex; flex-direction: column; gap: 2px; min-width: 0; flex: 1; }
  &__choice-body strong { color: var(--l-text); font-size: 13.5px; }
  &__choice-title { display: flex; align-items: center; gap: 8px; min-width: 0; }
  &__choice-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
  &__choice-body small { color: var(--l-text-dim); font-size: 11px; }
  &__choice-body code { overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    color: var(--l-text-faint); font: 10px/1.4 monospace; }
  &__check { color: transparent; font-size: 12px; }
  &__choice.selected &__check { color: var(--accent); }
  &__empty { margin: 0; padding: 18px; border: 1px dashed var(--l-border-strong);
    border-radius: var(--l-radius-sm); color: var(--l-text-dim); font-size: 13px; }
  &__progress { display: flex; justify-content: space-between; flex-wrap: wrap; gap: 7px;
    color: var(--l-text); font-size: 12px; }
  &__progress-track { flex-basis: 100%; height: 5px; overflow: hidden; border-radius: 4px;
    background: rgba(255,255,255,0.1); }
  &__progress-track span { display: block; height: 100%; background: var(--accent); transition: width 160ms linear; }
  &__error { margin: 0; color: #ff8787; font-size: 12px; line-height: 1.4; }
  &__badge { display: inline-flex; align-items: center; flex: none; padding: 0 8px; height: 20px; border-radius: 4px;
    background: var(--accent); color: var(--accent-contrast-text); font-size: 10px; font-weight: 700; line-height: 16px;
    letter-spacing: 0.04em; text-transform: uppercase; white-space: nowrap; }
  &__update { display: flex; align-items: center; gap: 8px; margin: 4px 0 0; color: var(--l-text-dim);
    font-size: 12px; line-height: 16px; }
  &__update.fresh { color: var(--l-text); }
  &__update.fresh i { color: var(--accent); }
  &__update.ok i { color: #6fd38a; }
  &__update.warn i { color: #ffc46b; }
  /* Кнопки стоят сеткой, а не переносятся по одной: две колонки по ширине
     подписей, «Указать папку» под первой кнопкой, значок «Обновить список»
     всегда в первом ряду сразу за ними. */
  &__actions { display: grid; grid-template-columns: repeat(2, minmax(0, max-content)) 40px; gap: 8px;
    padding-top: 10px; }
  &__actions .settings-btn { justify-content: flex-start; min-width: 0; min-height: 40px; padding: 8px 10px;
    font-size: 12.5px; line-height: 16px; text-align: left; }
  &__actions .settings-btn i { flex: none; width: 14px; text-align: center; }
  &__actions .proton__refresh { grid-column: 3; grid-row: 1; justify-content: center; padding: 8px 0; }
}

.settings-btn {
  /* inline-flex, а не inline-block: у кнопок с иконкой (экспорт, импорт,
     выгрузка диагностики) иначе нет отступа между значком и подписью, и
     значок съезжает относительно базовой линии текста. */
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  min-height: 48px;
  padding: 12px 20px;
  border: none;
  box-shadow: inset 0 0 0 1px var(--l-border-strong);
  border-radius: var(--l-radius-sm);
  background: rgba(255, 255, 255, 0.05);
  color: var(--l-text);
  font-family: inherit;
  font-size: 13.5px;
  font-weight: 600;
  line-height: 24px;
  cursor: pointer;
  transition: background var(--l-fast), border-color var(--l-fast),
    transform var(--l-fast);

  i {
    font-size: 13px;
    opacity: 0.9;
  }

  &:hover {
    background: rgba(255, 255, 255, 0.1);
  }

  &:active {
    transform: scale(0.97);
  }

  /* Пока main занят экспортом или импортом, кнопки заблокированы: без этого
     состояния «занято» кнопка выглядела активной и приглашала нажать ещё раз */
  &:disabled {
    opacity: 0.45;
    cursor: default;
    transform: none;
  }

  &:disabled:hover {
    background: rgba(255, 255, 255, 0.05);
  }

  &.ghost {
    background: transparent;
    border-color: transparent;
    color: var(--l-text-dim);

    &:hover {
      color: var(--l-text);
      background: rgba(255, 255, 255, 0.06);
    }
  }

  &.primary {
    background: var(--accent);
    border-color: transparent;
    color: var(--accent-contrast-text);

    &:hover {
      background: var(--accent-hover);
    }

    &:active {
      background: var(--accent-pressed);
    }

    &:disabled:hover {
      background: var(--accent);
    }
  }

  &.ghost:disabled:hover {
    background: transparent;
    color: var(--l-text-dim);
  }
}
</style>
