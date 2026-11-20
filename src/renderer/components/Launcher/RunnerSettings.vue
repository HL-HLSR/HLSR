<template>
  <div class="runner-settings">
    <!-- ================= Отображение ================= -->
    <h4 class="settings-group">{{ $t("#L_PROFILE_VIEW") }}</h4>

    <div class="settings-row">
      <div class="settings-row__text">
        <span class="settings-row__label">{{ $t("#L_RUNNER_SHOW_ID") }}</span>
        <span class="settings-row__hint">{{ $t("#L_RUNNER_SHOW_ID_HINT") }}</span>
      </div>
      <label class="settings-switch">
        <input
          type="checkbox"
          :checked="!hideSteamId"
          :aria-label="$t('#L_RUNNER_SHOW_ID')"
          @change="$emit('toggle-id')"
        />
        <span class="settings-switch__track"><span class="settings-switch__thumb"></span></span>
      </label>
    </div>

    <div v-if="account.steamId64" class="settings-row">
      <div class="settings-row__text">
        <span class="settings-row__label">{{ $t("#L_RUNNER_STEAM_ACCOUNT") }}</span>
        <span class="settings-row__hint">
          SteamID64:
          <!-- Скрытый SteamID64: глаз слева переключает, номер или точки той
               же длины стоят на одном месте, без анимаций. Настоящий номер
               попадает в разметку только когда он открыт. -->
          <span class="runner-steamid" :class="{ 'is-open': idRevealed }">
            <button
              type="button"
              class="runner-steamid__toggle"
              :aria-pressed="idRevealed ? 'true' : 'false'"
              :aria-label="$t(idRevealed ? '#L_RUNNER_ID_HIDE' : '#L_RUNNER_ID_REVEAL')"
              :title="$t(idRevealed ? '#L_RUNNER_ID_HIDE' : '#L_RUNNER_ID_REVEAL')"
              @click="idRevealed = !idRevealed"
            >
              <i :class="idRevealed ? 'fas fa-eye' : 'fas fa-eye-slash'" aria-hidden="true"></i>
            </button>
            <!-- Ширину задаёт невидимый образец из цифр той же длины: номер
                 и точки занимают ровно её, без пустоты справа. -->
            <span class="runner-steamid__slot">
              <span class="runner-steamid__sizer" aria-hidden="true">{{ idSizer }}</span>
              <span v-if="idRevealed" class="runner-steamid__value">{{ account.steamId64 }}</span>
              <span v-else class="runner-steamid__value is-masked" aria-hidden="true">
                <span v-for="n in idLength" :key="n">•</span>
              </span>
            </span>
          </span>
        </span>
      </div>
      <div class="runner-settings__buttons">
        <button type="button" class="settings-btn" @click="$emit('copy', account.steamId64)">
          <i class="fas fa-copy" aria-hidden="true"></i>
          {{ $t("#L_RUNNER_COPY") }}
        </button>
        <button type="button" class="settings-btn" @click="$emit('open-steam')">
          <i class="fab fa-steam" aria-hidden="true"></i>
          {{ $t("#L_PROFILE_OPEN_STEAM") }}
        </button>
      </div>
    </div>

    <!-- ================= Приватность =================
         Флаги хранятся на сервере: по ним он не показывает профиль в
         списках игроков и прячет от других текущую игру. -->
    <h4 class="settings-group">{{ $t("#L_RUNNER_PRIVACY_GROUP") }}</h4>

    <div v-for="item in privacyItems" :key="item.flag" class="settings-row">
      <div class="settings-row__text">
        <span class="settings-row__label">{{ $t(item.label) }}</span>
        <span v-if="privacyError === item.flag" class="settings-row__hint is-error" role="alert">{{ $t("#L_RUNNER_FORM_ERROR") }}</span>
        <span v-else class="settings-row__hint">{{ $t(item.hint) }}</span>
      </div>
      <label class="settings-switch">
        <input
          type="checkbox"
          :checked="!hidden(item.flag)"
          :disabled="privacyPending[item.flag] !== undefined"
          :aria-label="$t(item.label)"
          @change="setPrivacy(item.flag, !$event.target.checked)"
        />
        <span class="settings-switch__track"><span class="settings-switch__thumb"></span></span>
      </label>
    </div>

    <!-- ================= О себе =================
         Поля и ссылки сохраняются одной кнопкой: сервер проверяет их
         вместе, и частично сохранённая форма запутала бы больше, чем
         помогла. -->
    <h4 class="settings-group">{{ $t("#L_RUNNER_ABOUT") }}</h4>

    <div class="settings-row">
      <div class="settings-row__text">
        <label class="settings-row__label" for="runner-status">{{ $t("#L_RUNNER_STATUS") }}</label>
        <span v-if="mute" class="settings-row__hint is-error" role="status">
          {{ mute.expiresAt ? $t("#L_MOD_MUTED_UNTIL", muteUntil) : $t("#L_MOD_MUTED_FOREVER") }}<template v-if="mute.reason"> {{ $t("#L_MOD_REASON", mute.reason) }}</template>
        </span>
        <span v-else class="settings-row__hint" :class="{ 'is-error': statusLength > 80 || statusHasLink }" :role="statusHasLink ? 'alert' : null">
          {{ statusHasLink ? $t("#L_RUNNER_STATUS_LINKS") : $t("#L_RUNNER_STATUS_HINT", statusLength) }}
        </span>
      </div>
      <input
        id="runner-status"
        v-model="form.status"
        :disabled="!!mute"
        class="runner-input"
        maxlength="160"
        autocomplete="off"
        :placeholder="$t('#L_RUNNER_STATUS_PLACEHOLDER')"
        @input="touch"
      />
    </div>

    <div class="settings-row">
      <div class="settings-row__text">
        <span class="settings-row__label">{{ $t("#L_RUNNER_PRONOUNS") }}</span>
        <span class="settings-row__hint">{{ $t("#L_RUNNER_PRONOUNS_HINT") }}</span>
      </div>
      <UiDropdown :items="pronounItems" :value="form.pronouns" align="right" @input="setPronouns">
        <template #toggle="{ open }">
          <span class="settings-select runner-settings__select" :class="{ open: open }">
            <span class="runner-settings__select-label">{{ pronounsLabel }}</span>
            <i class="fas fa-chevron-down" aria-hidden="true"></i>
          </span>
        </template>
      </UiDropdown>
    </div>

    <div class="settings-row">
      <div class="settings-row__text">
        <span class="settings-row__label">{{ $t("#L_RUNNER_COUNTRY") }}</span>
        <span class="settings-row__hint">{{ $t("#L_RUNNER_COUNTRY_HINT") }}</span>
      </div>
      <UiDropdown :items="countries" :value="form.country" align="right" @input="setCountry">
        <template #toggle="{ open }">
          <span class="settings-select runner-settings__select" :class="{ open: open }">
            <span class="runner-settings__select-label">{{ countryLabel }}</span>
            <i class="fas fa-chevron-down" aria-hidden="true"></i>
          </span>
        </template>
      </UiDropdown>
    </div>

    <h4 class="settings-group">{{ $t("#L_RUNNER_LINKS") }}</h4>

    <div v-for="link in links" :key="link.id" class="settings-row">
      <div class="settings-row__text">
        <label class="settings-row__label runner-settings__brand" :for="'runner-' + link.id">
          <i :class="link.icon" aria-hidden="true"></i>
          {{ $t(link.label) }}
        </label>
        <span v-if="link.hint" class="settings-row__hint" :class="{ 'is-error': linkInvalid[link.id] }" :role="linkInvalid[link.id] ? 'alert' : null">
          {{ $t(linkInvalid[link.id] ? link.bad : link.hint) }}
        </span>
      </div>
      <input
        :id="'runner-' + link.id"
        v-model="form[link.id]"
        class="runner-input"
        :maxlength="link.max"
        autocomplete="off"
        spellcheck="false"
        :placeholder="link.placeholder"
        @input="touch"
        @blur="tidyLink(link.id)"
      />
    </div>

    <div class="runner-settings__save">
      <span v-if="formMessage" class="runner-settings__message" :class="formTone" role="status">
        <i :class="formTone === 'ok' ? 'fas fa-check' : 'fas fa-exclamation-circle'" aria-hidden="true"></i>
        {{ $t(formMessage) }}
      </span>
      <button type="button" class="settings-btn ghost" :disabled="busy || !dirty" @click="resetForm">
        {{ $t("#L_RUNNER_REVERT") }}
      </button>
      <button
        type="button"
        class="settings-btn primary"
        :disabled="busy || !dirty || statusLength > 80 || statusHasLink || linkInvalid.youtube || linkInvalid.twitch"
        @click="save"
      >
        {{ $t("#L_RUNNER_SAVE") }}
      </button>
    </div>

    <!-- ================= Подсветка ника и рамка =================
         Заготовки «подсветка ника + рамка аватара» выбирают поддержавшие
         проект (StylePresetPicker). Сервер на PHP заготовок не знает: список
         пуст, и группы нет, если в профиле нет ни прав, ни подписки, ни
         привязки Telegram. -->
    <template v-if="styleGroup">
      <h4 class="settings-group">{{ $t("#L_RUNNER_STYLE_GROUP") }}</h4>

      <StylePresetPicker
        :perks="perks"
        :subscriberBadge="subscriberBadge"
        :current="stylePreset"
        :presets="stylePresets"
        :account="account"
        @saved="$emit('updated', $event)"
        @stale="loadPresets(true)"
        @open-sponsors="$emit('open-sponsors')"
      />

      <!-- Telegram привязывается в боте по коду, как Discord: «Привязать»
           берёт код у сервера и открывает бота с ним (/start <код>). Сервер
           узнаёт о привязке сразу, а лаунчер — при следующей синхронизации
           профиля. «Обновить» синхронизирует сейчас (без опроса в фоне).
           «Отвязать» снимает все Telegram, привязанные к профилю.
           Привязка Telegram — только в официальной сборке. -->
      <div v-if="official" class="settings-row">
        <div class="settings-row__text">
          <span class="settings-row__label runner-settings__brand">
            <i class="fab fa-telegram" aria-hidden="true"></i>
            {{ telegramLabel }}
          </span>
          <span v-if="telegramNote" class="settings-row__hint" :class="{ 'is-error': telegramTone === 'error' }" :role="telegramTone === 'error' ? 'alert' : 'status'">
            {{ $t(telegramNote) }}
          </span>
          <template v-else-if="!telegram.linked && telegramCode">
            <span class="settings-row__hint" role="status">{{ $t("#L_RUNNER_TELEGRAM_CODE", telegramCode) }}</span>
            <span class="settings-row__hint">{{ $t("#L_RUNNER_DISCORD_UNTIL", discordTime(telegramExpires)) }}</span>
          </template>
          <span v-else class="settings-row__hint">
            {{ $t(telegram.linked ? "#L_RUNNER_TELEGRAM_HINT" : "#L_RUNNER_TELEGRAM_LINK_HINT") }}
          </span>
        </div>
        <div class="runner-settings__buttons is-row">
          <button type="button" class="settings-btn ghost" :disabled="telegramBusy" @click="refreshTelegram">
            <i class="fas fa-sync-alt" :class="{ 'fa-spin': telegramBusy }" aria-hidden="true"></i>
            {{ $t("#L_RUNNER_TELEGRAM_REFRESH") }}
          </button>
          <button v-if="!telegram.linked" type="button" class="settings-btn" :disabled="telegramBusy" @click="linkTelegram">
            <i class="fab fa-telegram" aria-hidden="true"></i>
            {{ $t("#L_RUNNER_TELEGRAM_LINK") }}
          </button>
          <button v-else type="button" class="settings-btn" :disabled="telegramBusy" @click="unlinkTelegram">
            {{ $t("#L_RUNNER_TELEGRAM_UNLINK") }}
          </button>
        </div>
      </div>
    </template>

    <!-- ================= speedrun.com =================
         Ключ живёт только в поле ввода и уходит на сервер один раз:
         ни лаунчер, ни сервер его не хранят. -->
    <h4 class="settings-group">speedrun.com</h4>

    <template v-if="!details.speedrun">
      <div class="settings-row column">
        <div class="settings-row__text">
          <label class="settings-row__label" for="runner-sr-key">{{ $t("#L_RUNNER_SR_KEY") }}</label>
          <span class="settings-row__hint">{{ $t("#L_RUNNER_SR_HELP") }}</span>
          <button type="button" class="settings-link" @click="openLink('https://www.speedrun.com/settings/api')">
            {{ $t("#L_RUNNER_SR_AUTH") }}
          </button>
        </div>
        <div class="runner-settings__inline">
          <input
            id="runner-sr-key"
            v-model="speedrunKey"
            class="runner-input"
            type="password"
            maxlength="250"
            autocomplete="off"
            spellcheck="false"
            :disabled="busy"
            @keydown.enter.prevent="linkSpeedrun"
          />
          <button
            type="button"
            class="settings-btn primary"
            :disabled="busy || !speedrunKey.trim()"
            @click="linkSpeedrun"
          >
            {{ $t("#L_RUNNER_SR_LINK") }}
          </button>
        </div>
      </div>
    </template>

    <template v-else>
      <div class="settings-row">
        <div class="settings-row__text">
          <span class="settings-row__label runner-settings__brand">
            <i class="fas fa-check-circle runner-settings__ok" aria-hidden="true"></i>
            {{ details.speedrun.name }}
          </span>
          <span class="settings-row__hint">
            {{ details.pinnedRun ? $t("#L_RUNNER_SR_PINNED", pinnedText) : $t("#L_RUNNER_SR_LINKED") }}
          </span>
        </div>
        <div class="runner-settings__buttons">
          <button type="button" class="settings-btn" :disabled="busy" @click="loadRuns">
            <i class="fas fa-stopwatch" aria-hidden="true"></i>
            {{ $t("#L_RUNNER_SR_BESTS") }}
          </button>
          <button v-if="details.pinnedRun" type="button" class="settings-btn ghost" :disabled="busy" @click="unpinRun">
            <i class="fas fa-thumbtack" aria-hidden="true"></i>
            {{ $t("#L_RUNNER_SR_UNPIN") }}
          </button>
          <button type="button" class="settings-btn ghost" :disabled="busy" @click="unlinkSpeedrun">
            {{ $t("#L_RUNNER_SR_UNLINK") }}
          </button>
        </div>
      </div>

      <ul v-if="runs.length" class="runner-runs">
        <li v-for="run in runs" :key="run.id" class="runner-runs__item">
          <span class="runner-runs__text">
            <span class="runner-runs__game">{{ run.game }}</span>
            <span class="runner-runs__category">{{ run.category }}</span>
          </span>
          <RunPlace :place="run.place" />
          <span class="runner-runs__time">{{ formatRun(run.seconds) }}</span>
          <button
            v-if="isPinned(run)"
            type="button"
            class="settings-btn ghost runner-runs__unpin"
            :disabled="busy"
            :title="$t('#L_RUNNER_SR_UNPIN')"
            @click="unpinRun"
          >
            <i class="fas fa-thumbtack" aria-hidden="true"></i>
            <span class="runner-runs__unpin-on">{{ $t("#L_RUNNER_SR_PINNED_BADGE") }}</span>
            <span class="runner-runs__unpin-off">{{ $t("#L_RUNNER_SR_UNPIN") }}</span>
          </button>
          <button v-else type="button" class="settings-btn" :disabled="busy" @click="pinRun(run.id)">
            {{ $t("#L_RUNNER_SR_PIN") }}
          </button>
        </li>
      </ul>
      <p v-else-if="runsLoaded" class="runner-settings__empty">{{ $t("#L_RUNNER_SR_EMPTY") }}</p>
    </template>

    <p v-if="speedrunMessage" class="runner-settings__message is-error runner-settings__block" role="alert">
      <i class="fas fa-exclamation-circle" aria-hidden="true"></i>
      {{ $t(speedrunMessage) }}
    </p>


    <!-- ================= Discord =================
         Код выдаёт сервер; игрок вводит его в Discord командой бота. Привязку
         лаунчер узнаёт при следующей синхронизации — её запускает возврат
         фокуса в окно, пока код на экране. -->
    <h4 class="settings-group">Discord</h4>

    <div class="settings-row">
      <div class="settings-row__text">
        <span class="settings-row__label runner-settings__brand">
          <i class="fab fa-discord" aria-hidden="true"></i>
          {{ discordLink.linked ? (discordLink.username ? $t("#L_RUNNER_DISCORD_LINKED", discordLink.username) : $t("#L_RUNNER_DISCORD_LINKED_PLAIN")) : $t("#L_RUNNER_DISCORD_NOT_LINKED") }}
        </span>
        <span v-if="discordLink.linked && discordLink.boosterSince" class="settings-row__hint">
          {{ $t("#L_RUNNER_DISCORD_BOOST", discordDate(discordLink.boosterSince)) }}
        </span>
        <span v-else-if="!discordLink.linked && discordCode" class="settings-row__hint">{{ $t("#L_RUNNER_DISCORD_UNTIL", discordTime(discordExpires)) }}</span>
        <span v-else-if="!discordLink.linked" class="settings-row__hint">{{ $t("#L_RUNNER_DISCORD_HINT") }}</span>
      </div>
      <div class="runner-settings__buttons is-row">
        <button v-if="discordLink.linked" type="button" class="settings-btn" :disabled="discordBusy" @click="unlinkDiscord">
          {{ $t("#L_RUNNER_DISCORD_UNLINK") }}
        </button>
        <template v-else>
          <button v-if="discordCode" type="button" class="settings-btn" @click="$emit('copy', discordCommand)">
            {{ $t("#L_RUNNER_DISCORD_COPY") }}
          </button>
          <button v-else type="button" class="settings-btn" :disabled="discordBusy" @click="requestDiscordCode">
            <i class="fab fa-discord" aria-hidden="true"></i>
            {{ $t("#L_RUNNER_DISCORD_LINK") }}
          </button>
        </template>
      </div>
    </div>

    <!-- Пошаговая подсказка по щелчку на «Как привязать»: команду бота легко
         отправить обычным сообщением, и тогда привязка не сработает. -->
    <div v-if="!discordLink.linked" class="runner-settings__block runner-settings__steps">
      <button type="button" class="settings-link runner-settings__steps-title" :aria-expanded="discordHelp ? 'true' : 'false'" @click="discordHelp = !discordHelp">
        <i class="fas" :class="discordHelp ? 'fa-chevron-down' : 'fa-chevron-right'" aria-hidden="true"></i>
        {{ $t("#L_RUNNER_DISCORD_STEPS") }}
      </button>
      <ol v-if="discordHelp" role="status">
        <li v-if="!discordCode">{{ $t("#L_RUNNER_DISCORD_STEP_CODE") }}</li>
        <li>
          {{ $t("#L_RUNNER_DISCORD_STEP_OPEN") }}
          <button type="button" class="settings-link" @click="openLink(discordInvite)">{{ $t("#L_RUNNER_DISCORD_OPEN") }}</button>
        </li>
        <li>{{ $t("#L_RUNNER_DISCORD_STEP_TYPE", discordCommandText) }}</li>
        <li>{{ $t("#L_RUNNER_DISCORD_STEP_CHECK") }}</li>
        <li>{{ $t("#L_RUNNER_DISCORD_STEP_BACK") }}</li>
      </ol>
    </div>

    <p v-if="discordMessage" class="runner-settings__message is-error runner-settings__block" role="alert">
      <i class="fas fa-exclamation-circle" aria-hidden="true"></i>
      {{ $t(discordMessage) }}
    </p>

    <!-- ================= Данные ================= -->
    <h4 class="settings-group">{{ $t("#L_RUNNER_DATA") }}</h4>

    <div class="settings-row">
      <div class="settings-row__text">
        <span class="settings-row__label">{{ $t("#L_RUNNER_TELEMETRY") }}</span>
        <span class="settings-row__hint">{{ $t("#L_RUNNER_TELEMETRY_HINT") }}</span>
        <button type="button" class="settings-link" @click="$emit('privacy')">{{ $t("#L_RUNNER_PRIVACY") }}</button>
      </div>
    </div>

    <div class="settings-row">
      <div class="settings-row__text">
        <span class="settings-row__label">{{ $t("#L_RUNNER_LOGOUT") }}</span>
        <span class="settings-row__hint">{{ $t("#L_RUNNER_LOGOUT_HINT") }}</span>
      </div>
      <button type="button" class="settings-btn" :disabled="busy" @click="logout">
        <i class="fas fa-sign-out-alt" aria-hidden="true"></i>
        {{ $t("#L_RUNNER_LOGOUT_SHORT") }}
      </button>
    </div>

    <div class="settings-row">
      <div class="settings-row__text">
        <span class="settings-row__label">{{ $t("#L_RUNNER_LOGOUT_ALL") }}</span>
        <span class="settings-row__hint">{{ $t("#L_RUNNER_LOGOUT_ALL_HINT") }}</span>
      </div>
      <button type="button" class="settings-btn" :disabled="busy" @click="logoutAll">
        <i class="fas fa-sign-out-alt" aria-hidden="true"></i>
        {{ $t("#L_RUNNER_LOGOUT_ALL_SHORT") }}
      </button>
    </div>

    <!-- ================= Удаление =================
         Плашка раскрывается на месте строки: человек видит, что будет
         удалено, ставит отметку, а кнопка ждёт 20 секунд. Ожидание
         отсчитывает сервер — здесь только его отображение. -->
    <div v-if="deletionAt" class="settings-row">
      <div class="settings-row__text">
        <span class="settings-row__label runner-settings__danger-text">{{ $t("#L_RUNNER_DELETE_SCHEDULED") }}</span>
        <span class="settings-row__hint">{{ $t("#L_RUNNER_DELETE_PENDING", deletionDate) }}</span>
      </div>
      <button type="button" class="settings-btn" :disabled="busy" @click="cancelDelete">
        {{ $t("#L_RUNNER_DELETE_CANCEL") }}
      </button>
    </div>

    <div v-else-if="!deleteConfirmation" class="settings-row">
      <div class="settings-row__text">
        <span class="settings-row__label">{{ $t("#L_RUNNER_DELETE") }}</span>
        <span class="settings-row__hint">{{ $t("#L_RUNNER_DELETE_HINT") }}</span>
      </div>
      <button type="button" class="settings-btn danger" :disabled="busy" @click="prepareDelete">
        <i class="fas fa-trash-alt" aria-hidden="true"></i>
        {{ $t("#L_RUNNER_DELETE_SHORT") }}
      </button>
    </div>

    <div v-else class="runner-delete" role="group" :aria-label="$t('#L_RUNNER_DELETE')">
      <span class="runner-delete__title">
        <i class="fas fa-exclamation-triangle" aria-hidden="true"></i>
        {{ $t("#L_RUNNER_DELETE") }}
      </span>
      <ul class="runner-delete__list">
        <li>{{ $t("#L_RUNNER_DELETE_ITEM_DATA") }}</li>
        <li>{{ $t("#L_RUNNER_DELETE_ITEM_WAIT") }}</li>
        <li>{{ $t("#L_RUNNER_DELETE_ITEM_FINAL") }}</li>
      </ul>
      <label class="runner-delete__agree">
        <input v-model="deleteAgreed" type="checkbox" />
        <span>{{ $t("#L_RUNNER_DELETE_AGREE") }}</span>
      </label>
      <div class="runner-delete__actions">
        <button type="button" class="settings-btn ghost" :disabled="busy" @click="closeDelete">
          {{ $t("#L_RUNNER_CANCEL") }}
        </button>
        <button
          type="button"
          class="settings-btn danger"
          :disabled="busy || !deleteAgreed || countdown > 0"
          @click="confirmDelete"
        >
          {{ $t("#L_RUNNER_DELETE_CONFIRM") }}
          <span v-if="countdown > 0" class="runner-delete__count">{{ countdown }}</span>
        </button>
      </div>
    </div>

    <p v-if="accountMessage" class="runner-settings__message is-error runner-settings__block" role="alert">
      <i class="fas fa-exclamation-circle" aria-hidden="true"></i>
      {{ $t(accountMessage) }}
    </p>
  </div>
</template>

<script>
import UiDropdown from "./UiDropdown";
import RunPlace from "./RunPlace";
import StylePresetPicker from "./StylePresetPicker";
import { formatRun } from "./run-time";
import { OFFICIAL } from "../../official";

// Та же проверка стоит на сервере: обойти её, изменив лаунчер, нельзя.
const { containsLink } = require("../../../shared/link-guard");
// Ссылка на канал вводится как удобно — ником или адресом с https или
// без, — а на сервер уходит полный адрес канала.
const profileLinks = require("../../../shared/profile-links");
const SITE_LINKS = require("../../../shared/links");
const { pronounList, pronounLabel, normalizePronouns } = require("../../../shared/pronouns");

const { ipcRenderer } = window.hlsrElectron;

const FIELDS = ["status", "pronouns", "country", "discord", "youtube", "twitch"];

const LINKS = [
  { id: "discord", icon: "fab fa-discord", label: "#L_RUNNER_DISCORD", max: 32, placeholder: "username" },
  { id: "twitch", icon: "fab fa-twitch", label: "#L_RUNNER_TWITCH", max: 256, placeholder: "twitch.tv/…",
    hint: "#L_RUNNER_TWITCH_HINT", bad: "#L_RUNNER_TWITCH_BAD" },
  { id: "youtube", icon: "fab fa-youtube", label: "#L_RUNNER_YOUTUBE", max: 256, placeholder: "youtube.com/@…",
    hint: "#L_RUNNER_YOUTUBE_HINT", bad: "#L_RUNNER_YOUTUBE_BAD" },
];

// Служебные коды ISO 3166 без отдельной страны: Евросоюз, ООН и
// зарезервированные под частное использование.
const NOT_COUNTRIES = ["EU", "EZ", "UN", "QO", "XA", "XB", "ZZ"];

const countryCache = {};

// Список стран строится из Intl, а не хранится в коде: так названия
// совпадают с языком интерфейса. Устаревшие коды (SU, YU и т. п.) Intl
// переводит в новые — такие отбрасываются, иначе страна была бы дважды.
function countryList(locale) {
  if (countryCache[locale]) return countryCache[locale];

  const names = new Intl.DisplayNames([locale], { type: "region" });
  const list = [];

  for (let a = 65; a <= 90; a++) {
    for (let b = 65; b <= 90; b++) {
      const code = String.fromCharCode(a, b);
      if (NOT_COUNTRIES.indexOf(code) !== -1) continue;
      if (new Intl.Locale("und-" + code).maximize().region !== code) continue;

      const name = names.of(code);
      if (name && name !== code) list.push({ id: code, label: name });
    }
  }

  countryCache[locale] = list.sort((x, y) => x.label.localeCompare(y.label, locale));
  return countryCache[locale];
}

export default {
  name: "RunnerSettings",
  components: { UiDropdown, RunPlace, StylePresetPicker },
  emits: ["copy", "open-steam", "privacy", "toggle-id", "updated", "open-sponsors"],
  props: {
    account: { type: Object, default: () => ({}) },
    details: { type: Object, default: () => ({}) },
    deletionAt: { type: String, default: null },
    hideSteamId: { type: Boolean, default: false },
    // Раздел настроек открыт; при уходе на другую вкладку SteamID64 снова скрывается.
    active: { type: Boolean, default: true },
    // Запрет статуса из админки: { reason, expiresAt } или null.
    mute: { type: Object, default: null },
    // Поддержавшим проект (src/main/online-profile/profile-state.js): права,
    // выбранная заготовка подсветки и привязка Telegram.
    perks: { type: Object, default: () => ({ styles: false, subscriber: null }) },
    // Подписка активна сейчас (subscriber профиля): значок у ника в
    // предпросмотре заготовок.
    subscriberBadge: { type: Boolean, default: false },
    stylePreset: { type: Number, default: null },
    telegram: { type: Object, default: () => ({ linked: false, username: null }) },
    discordLink: { type: Object, default: () => ({ linked: false, username: null, boosterSince: null }) },
  },
  data() {
    return {
      official: OFFICIAL,
      // SteamID64 под «Показывать SteamID» открывается нажатием и снова
      // прячется, когда настройки открывают заново.
      idRevealed: false,
      form: {},
      dirty: false,
      busy: false,
      formMessage: null,
      formTone: "ok",
      speedrunKey: "",
      speedrunMessage: null,
      runs: [],
      runsLoaded: false,
      accountMessage: null,
      deleteConfirmation: null,
      deleteAgreed: false,
      countdown: 20,
      countdownTimer: null,
      privacyItems: [
        { flag: "hideFromPlayers", label: "#L_RUNNER_SHOW_IN_PLAYERS", hint: "#L_RUNNER_SHOW_IN_PLAYERS_HINT" },
        { flag: "hideGame", label: "#L_RUNNER_SHOW_GAME", hint: "#L_RUNNER_SHOW_GAME_HINT" },
      ],
      // Значение переключателя, пока запрос в пути; ключа нет — берётся с сервера.
      privacyPending: {},
      privacyError: null,
      // Заготовки подсветки (public-style-presets-get): на сервере на PHP — пусто.
      stylePresets: [],
      telegramBusy: false,
      discordBusy: false,
      discordCode: null,
      discordHelp: false,
      discordExpires: 0,
      discordMessage: null,
      telegramMessage: null,
      telegramTone: "ok",
      telegramCode: null,
      telegramExpires: 0,
    };
  },
  computed: {
    discordCommand() {
      return "/" + this.$t("#L_RUNNER_DISCORD_COMMAND") + " " + this.discordCodeShown;
    },
    // В русском тексте обе формы: при английском Discord код не должен уйти обычным сообщением.
    // До выдачи кода в подсказке стоит слово «код».
    discordCodeShown() {
      return this.discordCode || this.$t("#L_RUNNER_DISCORD_CODE_WORD");
    },
    discordInvite() {
      return SITE_LINKS.discord;
    },
    discordCommandText() {
      const alt = "/link " + this.discordCodeShown;
      return this.discordCommand === alt ? alt : this.discordCommand + " (" + this.$t("#L_RUNNER_DISCORD_OR") + " " + alt + ")";
    },
    idLength() {
      return String(this.account.steamId64 || "").length || 17;
    },
    idSizer() {
      return "0".repeat(this.idLength);
    },
    links() {
      return LINKS;
    },
    locale() {
      return this.$t("#L_RUNNER_LOCALE");
    },
    statusLength() {
      return Array.from(this.form.status || "").length;
    },
    statusHasLink() {
      return containsLink(this.form.status || "");
    },
    linkInvalid() {
      return {
        youtube: profileLinks.normalize("youtube", this.form.youtube || "") === null,
        twitch: profileLinks.normalize("twitch", this.form.twitch || "") === null,
      };
    },
    countries() {
      return [{ id: "", label: this.$t("#L_RUNNER_NOT_SET") }].concat(countryList(this.locale));
    },
    muteUntil() {
      if (!this.mute || !this.mute.expiresAt) return "";
      return new Date(this.mute.expiresAt).toLocaleString(this.locale === "ru" ? "ru-RU" : "en-GB", { day: "numeric", month: "long", hour: "2-digit", minute: "2-digit" });
    },
    pronounItems() {
      return [{ id: "", label: this.$t("#L_RUNNER_NOT_SET") }].concat(pronounList(this.locale));
    },
    pronounsLabel() {
      return pronounLabel(this.form.pronouns, this.locale) || this.$t("#L_RUNNER_NOT_SET");
    },
    countryLabel() {
      const item = this.countries.find((country) => country.id === (this.form.country || ""));
      return item ? item.label : this.form.country;
    },
    pinnedText() {
      const run = this.details.pinnedRun;
      return run ? run.game + " · " + run.category + " · " + formatRun(run.seconds) : "";
    },
    deletionDate() {
      return this.deletionAt ? new Date(this.deletionAt).toLocaleString(this.locale) : "";
    },
    // Группа «Подсветка ника и рамка»: есть заготовки (новый сервер) или
    // профиль сам говорит о поддержке — право, подписка, привязка Telegram.
    styleGroup() {
      const perks = this.perks || {};
      return this.stylePresets.length > 0 || perks.styles === true || !!perks.subscriber || !!(this.telegram && this.telegram.linked);
    },
    telegramLabel() {
      const telegram = this.telegram || {};
      if (!telegram.linked) return this.$t("#L_RUNNER_TELEGRAM_NOT_LINKED");
      return telegram.username ? this.$t("#L_RUNNER_TELEGRAM_LINKED_AS", "@" + telegram.username) : this.$t("#L_RUNNER_TELEGRAM_LINKED");
    },
    // Подсказка под строкой Telegram. «Привязка пока не найдена» рядом с
    // «Telegram: привязан» была бы неправдой — пока привязан, её нет.
    telegramNote() {
      if (this.telegramMessage === "#L_RUNNER_TELEGRAM_NOT_YET" && this.telegram && this.telegram.linked) return null;
      return this.telegramMessage;
    },
  },
  watch: {
    // Привязку принесла фоновая синхронизация (после «Обновить» с ответом
    // «не найдена»): прежняя подсказка больше не к месту.
    // Привязка пришла (синхронизация по фокусу): код больше не нужен.
    "discordLink.linked"(linked) {
      if (linked) this.dropDiscordCode();
    },
    "telegram.linked"(linked) {
      if (linked) {
        this.telegramMessage = null;
        this.telegramCode = null;
      }
    },
    active(value) {
      if (!value) this.idRevealed = false;
    },
    details: {
      immediate: true,
      handler() {
        if (!this.dirty) this.resetForm();
      },
    },
  },
  methods: {
    hideId() {
      this.idRevealed = false;
    },
    formatRun,

    touch() {
      this.dirty = true;
      this.formMessage = null;
    },

    setPronouns(id) {
      this.form.pronouns = normalizePronouns(id);
      this.touch();
    },
    setCountry(code) {
      this.form.country = code;
      this.touch();
    },

    resetForm() {
      const form = {};
      for (const field of FIELDS) form[field] = this.details[field] || "";
      form.pronouns = normalizePronouns(form.pronouns);

      this.form = form;
      this.dirty = false;
    },

    // Ответ main: снимок профиля при успехе, { status: "error" } при отказе
    // сервера. Исключение означает, что до сервера не дошли вовсе.
    // Форма — реактивный Proxy Vue, а IPC копирует аргументы structured
    // clone, который Proxy не принимает: без копии в простой объект любое
    // сохранение падало ещё в лаунчере и показывало ошибку сети.
    hidden(flag) {
      return flag in this.privacyPending ? this.privacyPending[flag] : this.details[flag] === true;
    },
    async setPrivacy(flag, value) {
      this.privacyPending = Object.assign({}, this.privacyPending, { [flag]: value });
      this.privacyError = null;
      const result = await this.request("runner-customize", { [flag]: value });
      if (result.error) this.privacyError = flag;
      const pending = Object.assign({}, this.privacyPending);
      delete pending[flag];
      this.privacyPending = pending;
    },

    async request(channel, value) {
      this.busy = true;

      try {
        const plain = value && typeof value === "object" ? JSON.parse(JSON.stringify(value)) : value;
        const result = await ipcRenderer.invoke(channel, plain);
        if (result && result.code === "RUNNER_SPEEDRUN_TAKEN") return { error: "#L_RUNNER_SR_TAKEN" };
        if (!result || result.status === "error") return { error: "#L_RUNNER_FORM_ERROR" };
        if (result.snapshot) this.$emit("updated", result.snapshot);
        return result;
      } catch (error) {
        return { error: "#L_RUNNER_NETWORK" };
      } finally {
        this.busy = false;
      }
    },

    // После ввода поле показывает, что уйдёт на сервер: полный адрес канала.
    tidyLink(id) {
      const url = profileLinks.normalize(id, this.form[id] || "");
      if (url !== null && url !== this.form[id]) this.form[id] = url;
    },

    async save() {
      const form = Object.assign({}, this.form);
      for (const id of ["youtube", "twitch"]) {
        const url = profileLinks.normalize(id, form[id] || "");
        if (url === null) return;
        form[id] = url;
      }
      const result = await this.request("runner-customize", form);

      if (result.error) {
        this.formTone = "is-error";
        this.formMessage = result.error;
        return;
      }

      this.dirty = false;
      this.formTone = "ok";
      this.formMessage = "#L_RUNNER_SAVED";
    },

    openLink(url) {
      ipcRenderer.invoke("runner-open-link", url).catch(() => {});
    },

    async linkSpeedrun() {
      const key = this.speedrunKey.trim();
      if (!key || this.busy) return;

      this.speedrunKey = "";
      this.speedrunMessage = null;

      const result = await this.request("runner-speedrun-link", key);
      if (result.error) this.speedrunMessage = result.error === "#L_RUNNER_NETWORK" || result.error === "#L_RUNNER_SR_TAKEN" ? result.error : "#L_RUNNER_SR_KEY_ERROR";
    },

    async unlinkSpeedrun() {
      const result = await this.request("runner-speedrun-unlink");
      this.speedrunMessage = result.error || null;
      if (result.error) return;

      this.runs = [];
      this.runsLoaded = false;
    },

    async loadRuns() {
      const result = await this.request("runner-speedrun-bests");
      this.speedrunMessage = result.error || null;
      if (result.error) return;

      this.runs = Array.isArray(result.runs) ? result.runs : [];
      this.runsLoaded = true;
    },

    async pinRun(id) {
      const result = await this.request("runner-speedrun-pin", id);
      this.speedrunMessage = result.error || null;
    },

    async unpinRun() {
      const result = await this.request("runner-speedrun-unpin");
      this.speedrunMessage = result.error || null;
    },

    isPinned(run) {
      return !!this.details.pinnedRun && this.details.pinnedRun.id === run.id;
    },

    async logout() {
      const result = await this.request("runner-logout");
      if (result.error) this.accountMessage = result.error;
      else this.$emit("updated", result);
    },

    async logoutAll() {
      const result = await this.request("runner-logout-all");
      if (result.error) this.accountMessage = result.error;
      else this.$emit("updated", result);
    },

    async prepareDelete() {
      this.accountMessage = null;

      const result = await this.request("runner-delete-challenge");
      if (result.error) {
        this.accountMessage = result.error;
        return;
      }

      this.deleteConfirmation = result;
      this.deleteAgreed = false;
      this.tick();

      if (this.countdownTimer) clearInterval(this.countdownTimer);
      this.countdownTimer = setInterval(this.tick, 250);
    },

    tick() {
      if (!this.deleteConfirmation) return;

      const left = Math.ceil((Date.parse(this.deleteConfirmation.readyAt) - Date.now()) / 1000);
      this.countdown = Math.max(0, left);

      if (!this.countdown && this.countdownTimer) {
        clearInterval(this.countdownTimer);
        this.countdownTimer = null;
      }
    },

    closeDelete() {
      if (this.countdownTimer) clearInterval(this.countdownTimer);
      this.countdownTimer = null;
      this.deleteConfirmation = null;
    },

    async confirmDelete() {
      const result = await this.request("runner-delete-request", {
        challenge: this.deleteConfirmation.challenge,
        confirmed: this.deleteAgreed,
      });

      if (result.error) this.accountMessage = result.error;
      else this.closeDelete();
    },

    async cancelDelete() {
      const result = await this.request("runner-delete-cancel");
      this.accountMessage = result.error || null;
    },

    // Заготовки подсветки: кеш main — 5 минут; force — заготовку скрыли или
    // удалили (выбор ответил PRESET_UNKNOWN). При сбое — последний список main.
    async loadPresets(force) {
      try {
        const result = force
          ? await ipcRenderer.invoke("public-style-presets-get", { force: true })
          : await ipcRenderer.invoke("public-style-presets-get");
        if (result && Array.isArray(result.presets)) this.stylePresets = result.presets;
      } catch (error) {
        // Без списка группа держится на данных профиля.
      }
    },

    // «Привязать»: код у сервера (живёт 10 минут, как у Discord), затем бот
    // из списка спонсоров с этим кодом (support-open, только https://t.me).
    // Пока код жив, кнопка снова открывает бота с ним же — если привязки ещё
    // нет: игрок мог уже привязаться этим кодом, а синхронизации по фокусу у
    // Telegram нет, и бот со старым кодом не нужен.
    async linkTelegram() {
      this.telegramMessage = null;
      if (this.telegramCode && Date.now() / 1000 < this.telegramExpires) {
        let snapshot = null;
        try { snapshot = await ipcRenderer.invoke("runner-sync"); } catch (error) { /* откроем бота с кодом */ }
        if (snapshot && snapshot.account) this.$emit("updated", snapshot);
        if (snapshot && snapshot.telegram && snapshot.telegram.linked) { this.telegramCode = null; return; }
      } else {
        this.telegramCode = null;
        const result = await this.codeAction("runner-telegram-code", "telegramBusy", "telegramMessage");
        if (result && (!/^\d{6}$/.test(result.code) || !Number.isSafeInteger(result.expiresAt))) this.telegramMessage = "#L_RUNNER_FORM_ERROR";
        if (this.telegramMessage) { this.telegramTone = "error"; return; }
        this.telegramCode = result.code;
        this.telegramExpires = result.expiresAt;
        // Срок вышел — код пропадает из подсказки, следующий «Привязать» возьмёт новый.
        clearTimeout(this.telegramTimer);
        this.telegramTimer = setTimeout(() => { this.telegramCode = null; }, Math.max(0, result.expiresAt * 1000 - Date.now()));
      }
      try {
        const result = await ipcRenderer.invoke("support-open", "bot", this.telegramCode);
        if (result && result.status === "ok") return;
      } catch (error) {
        // Ниже — тот же текст, что при отказе main.
      }
      this.telegramTone = "error";
      this.telegramMessage = "#L_RUNNER_TELEGRAM_OPEN_ERROR";
    },

    // «Отвязать»: сервер снимает все Telegram этого профиля; новый снимок
    // профиля приходит в ответе.
    async unlinkTelegram() {
      this.telegramCode = null;
      if (!await this.codeAction("runner-telegram-unlink", "telegramBusy", "telegramMessage")) this.telegramTone = "error";
    },

    discordDate(seconds) {
      return new Date(seconds * 1000).toLocaleDateString(this.locale === "ru" ? "ru-RU" : "en-GB", { day: "numeric", month: "long", year: "numeric" });
    },
    discordTime(seconds) {
      return new Date(seconds * 1000).toLocaleTimeString(this.locale === "ru" ? "ru-RU" : "en-GB", { hour: "2-digit", minute: "2-digit" });
    },
    dropDiscordCode() {
      clearTimeout(this.discordTimer);
      this.discordCode = null;
      window.removeEventListener("focus", this.syncDiscord);
    },
    // Фокус вернулся в окно, пока код на экране: игрок мог уже ввести его в
    // Discord. Привязку (или конец срока кода) подхватит watcher/проверка ниже.
    async syncDiscord() {
      try {
        const snapshot = await ipcRenderer.invoke("runner-sync");
        if (snapshot && snapshot.account) this.$emit("updated", snapshot);
      } catch (error) { /* следующая синхронизация повторит */ }
      if (this.discordCode && Date.now() / 1000 >= this.discordExpires) this.dropDiscordCode();
    },
    // Напрямую, а не через request(): тот теряет код ошибки, а 429 нужен.
    // busy и message — имена полей строки (Discord или Telegram).
    async codeAction(channel, busy, message) {
      this[busy] = true;
      this[message] = null;
      try {
        const result = await ipcRenderer.invoke(channel);
        if (result && result.status === "error") {
          this[message] = result.code === "RUNNER_HTTP_429" ? "#L_RUNNER_DISCORD_RATE" : result.code === "RUNNER_NETWORK" ? "#L_RUNNER_NETWORK" : "#L_RUNNER_FORM_ERROR";
          return null;
        }
        if (!result) throw new Error("RUNNER_NETWORK");
        if (result.snapshot) this.$emit("updated", result.snapshot);
        return result;
      } catch (error) {
        this[message] = "#L_RUNNER_NETWORK";
        return null;
      } finally {
        this[busy] = false;
      }
    },
    discordAction(channel) {
      return this.codeAction(channel, "discordBusy", "discordMessage");
    },
    async requestDiscordCode() {
      const result = await this.discordAction("runner-discord-code");
      if (!result || !/^\d{6}$/.test(result.code) || !Number.isSafeInteger(result.expiresAt)) { this.discordMessage = "#L_RUNNER_FORM_ERROR"; return; }
      this.discordCode = result.code;
      this.discordExpires = result.expiresAt;
      window.removeEventListener("focus", this.syncDiscord);
      window.addEventListener("focus", this.syncDiscord);
      // Срок вышел — код пропадает, кнопка «Привязать» снова доступна.
      clearTimeout(this.discordTimer);
      this.discordTimer = setTimeout(() => this.dropDiscordCode(), Math.max(0, result.expiresAt * 1000 - Date.now()));
    },
    async unlinkDiscord() {
      this.dropDiscordCode();
      await this.discordAction("runner-discord-unlink");
    },
    // «Обновить»: синхронизация профиля сейчас. Снимок уходит в окно
    // профиля; привязки всё ещё нет — подсказка, что делать дальше.
    async refreshTelegram() {
      if (this.telegramBusy) return;
      this.telegramBusy = true;
      this.telegramMessage = null;
      try {
        const snapshot = await ipcRenderer.invoke("runner-sync");
        if (!snapshot || !snapshot.account) throw new Error("RUNNER_NETWORK");
        this.$emit("updated", snapshot);
        if (snapshot.error) {
          this.telegramTone = "error";
          this.telegramMessage = "#L_RUNNER_NETWORK";
        } else if (!(snapshot.telegram && snapshot.telegram.linked)) {
          this.telegramTone = "ok";
          this.telegramMessage = "#L_RUNNER_TELEGRAM_NOT_YET";
        }
      } catch (error) {
        this.telegramTone = "error";
        this.telegramMessage = "#L_RUNNER_NETWORK";
      } finally {
        this.telegramBusy = false;
      }
    },
  },
  async created() {
    await this.loadPresets(false);
  },
  // Открытый SteamID64 прячется и когда окно лаунчера уходит на задний
  // план или сворачивается: вернувшись, номер снова под спойлером.
  mounted() {
    window.addEventListener("blur", this.hideId);
    document.addEventListener("visibilitychange", this.hideId);
  },
  beforeUnmount() {
    window.removeEventListener("blur", this.hideId);
    window.removeEventListener("focus", this.syncDiscord);
    clearTimeout(this.discordTimer);
    clearTimeout(this.telegramTimer);
    document.removeEventListener("visibilitychange", this.hideId);
    this.speedrunKey = "";
    if (this.countdownTimer) clearInterval(this.countdownTimer);
  },
};
</script>

<style lang="scss" scoped>
/* Строки, переключатели и кнопки повторяют настройки лаунчера
   (SettingsModal.vue): профиль — такое же окно настроек, а не отдельная
   форма со своим оформлением. */

.settings-group {
  margin: 32px 0 4px;
  font-size: 12px;
  font-weight: 600;
  line-height: 16px;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--l-text-dim);

  &:first-child {
    margin-top: 16px;
  }
}

.settings-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 20px;
  padding: 16px 0;
  box-shadow: inset 0 -1px 0 rgba(255, 255, 255, 0.045);

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

    &.is-error {
      color: #ff6b78;
    }
  }
}

.settings-link {
  align-self: flex-start;
  margin-top: 2px;
  padding: 0;
  border: 0;
  background: none;
  color: var(--l-accent);
  font-family: inherit;
  font-size: 12px;
  line-height: 20px;
  text-decoration: underline;
  text-underline-offset: 3px;
  cursor: pointer;

  &:hover {
    filter: brightness(1.15);
  }

  &:focus-visible {
    outline: 2px solid var(--l-accent);
    outline-offset: 2px;
  }
}

/* Поле ввода — как путь к библиотеке в настройках загрузок */
.runner-input {
  flex: 0 0 280px;
  width: 280px;
  min-width: 0;
  box-sizing: border-box;
  padding: 9px 14px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--l-border);
  border-radius: var(--l-radius-sm);
  color: var(--l-text);
  font-family: inherit;
  font-size: 13px;
  line-height: 20px;
  transition: background var(--l-fast), border-color var(--l-fast);

  &::placeholder {
    color: var(--l-text-faint);
  }

  &:hover {
    background: rgba(255, 255, 255, 0.07);
  }

  &:focus {
    outline: none;
    border-color: var(--accent);
  }

  &:disabled {
    opacity: 0.45;
  }
}

.settings-select {
  display: flex;
  align-items: center;
  gap: 12px;
  justify-content: space-between;
  padding: 9px 14px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--l-border);
  border-radius: var(--l-radius-sm);
  color: var(--l-text);
  font-size: 13px;
  font-weight: 500;
  line-height: 20px;
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

.runner-settings__select {
  box-sizing: border-box;
  width: 280px;
}

.runner-settings__select-label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.settings-switch {
  position: relative;
  display: inline-flex;
  flex: none;
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
    box-sizing: border-box;
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

  input:disabled + .settings-switch__track {
    opacity: 0.45;
  }
}

.settings-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex: none;
  gap: 8px;
  min-height: 40px;
  padding: 8px 16px;
  border: none;
  box-shadow: inset 0 0 0 1px var(--l-border-strong);
  border-radius: var(--l-radius-sm);
  background: rgba(255, 255, 255, 0.05);
  color: var(--l-text);
  font-family: inherit;
  font-size: 13px;
  font-weight: 600;
  line-height: 24px;
  white-space: nowrap;
  cursor: pointer;
  transition: background var(--l-fast), transform var(--l-fast);

  i {
    font-size: 12px;
    opacity: 0.9;
  }

  &:hover {
    background: rgba(255, 255, 255, 0.1);
  }

  &:active {
    transform: scale(0.97);
  }

  &:focus-visible {
    outline: 2px solid var(--accent-focus);
    outline-offset: 2px;
  }

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
    box-shadow: none;
    color: var(--l-text-dim);

    &:hover {
      color: var(--l-text);
      background: rgba(255, 255, 255, 0.06);
    }

    &:disabled:hover {
      background: transparent;
      color: var(--l-text-dim);
    }
  }

  &.primary {
    background: var(--accent);
    box-shadow: none;
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

  &.danger {
    box-shadow: inset 0 0 0 1px rgba(224, 69, 90, 0.45);
    color: #ff8796;

    &:hover {
      background: rgba(224, 69, 90, 0.12);
    }

    &:disabled:hover {
      background: rgba(255, 255, 255, 0.05);
    }
  }
}

.runner-settings {
  padding-bottom: 24px;

  &__buttons {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 8px;

    // Две короткие кнопки рядом с длинной подсказкой: в строку, а
    // переносится подсказка.
    &.is-row {
      flex: none;
      flex-wrap: nowrap;
    }
  }

  &__inline {
    display: flex;
    gap: 8px;

    .runner-input {
      flex: 1 1 auto;
      width: auto;
    }
  }

  &__mono {
    font-variant-numeric: tabular-nums;
    letter-spacing: 0.02em;
  }

  &__brand {
    display: inline-flex;
    align-items: center;
    gap: 12px;

    i {
      width: 16px;
      text-align: center;
      color: var(--l-text-dim);
      font-size: 14px;
    }
  }

  &__ok {
    color: #4ad07a !important;
  }

  &__save {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
    padding-top: 16px;
  }

  &__message {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    margin-right: auto;
    color: #6fd38a;
    font-size: 12px;
    line-height: 20px;

    &.is-error {
      color: #ff6b78;
    }
  }

  &__steps {
    /* __block ниже задаёт flex в строку: шаги — столбиком под заголовком. */
    flex-direction: column;
    color: var(--l-text-dim);
    font-size: 13px;
    line-height: 20px;

    ol {
      margin: 6px 0 0;
      padding-left: 20px;
    }

    li + li {
      margin-top: 4px;
    }
  }

  &__steps-title {
    align-self: flex-start;
    font-weight: 600;

    i {
      width: 12px;
      font-size: 10px;
    }
  }

  &__block {
    display: flex;
    margin: 12px 0 0;
  }

  &__empty {
    margin: 0;
    padding: 12px 0 16px;
    color: var(--l-text-faint);
    font-size: 12px;
    line-height: 20px;
  }

  &__danger-text {
    color: #ff8796;
  }
}

/* Рекорды speedrun.com — тем же списком, что и выбор Proton */
.runner-runs {
  list-style: none;
  margin: 0;
  padding: 8px 0 16px;
  display: flex;
  flex-direction: column;
  gap: 8px;

  &__item {
    display: flex;
    align-items: center;
    gap: 16px;
    min-height: 56px;
    padding: 8px 8px 8px 16px;
    box-sizing: border-box;
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.03);
    box-shadow: inset 0 0 0 1px var(--l-border);
  }

  &__text {
    display: flex;
    flex-direction: column;
    flex: 1 1 auto;
    min-width: 0;
  }

  &__game {
    overflow: hidden;
    color: var(--l-text);
    font-size: 13px;
    font-weight: 600;
    line-height: 20px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  &__category {
    color: var(--l-text-faint);
    font-size: 12px;
    line-height: 20px;
  }

  &__time {
    color: var(--l-text);
    font-size: 14px;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }

  // Закреплённый забег: «В профиле», при наведении — «Открепить».
  &__unpin {
    color: var(--l-accent);

    .runner-runs__unpin-off {
      display: none;
    }

    &:hover:not(:disabled),
    &:focus-visible {
      color: var(--l-danger, #ff6b6b);

      .runner-runs__unpin-on {
        display: none;
      }

      .runner-runs__unpin-off {
        display: inline;
      }
    }
  }
}

.runner-delete {
  margin-top: 16px;
  padding: 16px 20px 20px;
  border-radius: var(--l-radius-sm);
  background: rgba(224, 69, 90, 0.06);
  box-shadow: inset 0 0 0 1px rgba(224, 69, 90, 0.35);

  &__title {
    display: flex;
    align-items: center;
    gap: 12px;
    color: var(--l-text);
    font-size: 14px;
    font-weight: 600;
    line-height: 24px;

    i {
      color: #ff8796;
      font-size: 13px;
    }
  }

  &__list {
    margin: 8px 0 16px;
    padding-left: 20px;
    color: var(--l-text-dim);
    font-size: 12.5px;
    line-height: 20px;

    li + li {
      margin-top: 4px;
    }
  }

  &__agree {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    color: var(--l-text);
    font-size: 13px;
    line-height: 20px;
    cursor: pointer;

    input {
      flex: none;
      width: 16px;
      height: 16px;
      margin: 2px 0 0;
      accent-color: #e0455a;
      cursor: pointer;
    }
  }

  &__actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 16px;
  }

  /* Цифры моноширинные: иначе кнопка дёргается каждую секунду */
  &__count {
    min-width: 20px;
    padding: 0 6px;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.08);
    font-variant-numeric: tabular-nums;
  }
}

@media (max-width: 760px) {
  .settings-row:not(.column) {
    flex-direction: column;
    align-items: stretch;
  }

  .runner-input,
  .runner-settings__select {
    flex: none;
    width: 100%;
  }
}

// Скрытый SteamID64: капсула с глазом слева. Переходов и анимаций нет —
// номер и точки меняются на месте, ширина капсулы не прыгает.
.runner-steamid {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  margin-left: 4px;
  padding: 1px 9px 1px 1px;
  border: 1px solid rgba(255, 255, 255, 0.08);
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.04);
  vertical-align: baseline;

  &__toggle {
    display: inline-grid;
    place-items: center;
    width: 22px;
    height: 20px;
    padding: 0;
    border: 0;
    border-radius: 999px;
    background: transparent;
    color: var(--l-text-faint);
    font-size: 10px;
    cursor: pointer;

    &:hover { color: var(--l-text); background: rgba(255, 255, 255, 0.08); }
    &:focus-visible { outline: 2px solid var(--accent); outline-offset: 1px; }
  }

  &__slot {
    display: inline-grid;
    font-variant-numeric: tabular-nums;
  }

  &__sizer,
  &__value {
    grid-area: 1 / 1;
  }

  &__sizer {
    visibility: hidden;
  }

  &__value {
    color: var(--l-text);
    user-select: text;

    &.is-masked {
      display: flex;
      justify-content: space-between;
      color: var(--l-text-faint);
      user-select: none;
    }
  }
}

@media (prefers-reduced-motion: reduce) {
  .settings-btn,
  .settings-switch__track,
  .settings-switch__thumb {
    transition: none;
  }
}
</style>
