<template>
  <div class="settings-modal game-settings" :class="{ 'game-settings--embedded': embedded }" @mousedown.self="embedded || $emit('close')">
    <div
      class="settings-modal__window game-settings__window"
      :role="embedded ? 'region' : 'dialog'"
      :aria-modal="embedded ? null : 'true'"
      :aria-label="$t('#L_GAME_SETTINGS')"
      :inert="keyboardOpen || null"
      ref="window"
    >
      <!-- Крестик — первым в окне: на него встаёт фокус при открытии.
           На странице «Персонализация» (embedded) окно не закрывается. -->
      <button
        v-if="!embedded"
        type="button"
        class="game-settings__close"
        :title="$t('#L_CLOSE')"
        :aria-label="$t('#L_CLOSE')"
        @click="$emit('close')"
      >
        <i class="fas fa-times" aria-hidden="true"></i>
      </button>

      <!-- Шапка как у профиля, но без баннера: название, игра и вкладки
           строкой под ними. Полоса под вкладкой перетекает к выбранной.
           На странице «Персонализация» (embedded) шапка — список разделов слева. -->
      <header class="game-settings__head">
        <div v-if="!embedded" class="game-settings__titles">
          <h2 class="game-settings__title">{{ $t("#L_GAME_SETTINGS") }}</h2>
          <span class="game-settings__game">{{ gameTitle }}</span>
        </div>

        <!-- В окне — вкладки. На странице — навигация: tablist не допускает вложенного списка
             подразделов, поэтому там обычные кнопки с aria-current. -->
        <nav class="game-settings__tabs" :role="embedded ? null : 'tablist'" :aria-label="$t('#L_GAME_SETTINGS')">
          <template v-for="s in sections" :key="s.id">
            <button
              :ref="(el) => setTabRef(s.id, el)"
              type="button"
              :role="embedded ? null : 'tab'"
              class="game-settings__tab"
              :data-tour="'gs-' + s.id"
              :class="{ 'is-active': activeSection === s.id, 'is-window': s.window }"
              :aria-selected="embedded ? null : (activeSection === s.id ? 'true' : 'false')"
              :aria-current="embedded && activeSection === s.id ? 'true' : null"
              @click="s.window ? openKeyboard() : selectSection(s.id)"
            >
              <i :class="s.icon" aria-hidden="true"></i>
              <span>{{ $t(s.title) }}</span>
              <i v-if="s.window" class="fas fa-expand-alt game-settings__tab-open" aria-hidden="true"></i>
            </button>
            <!-- Подразделы редактора внешнего вида — под своим пунктом, пока он открыт (R6).
                 id как у прежнего списка редактора: на них ссылаются его панели (aria-labelledby). -->
            <div
              v-if="s.id === 'personal' && activeSection === 'personal' && personalNav.items.length"
              class="game-settings__subtabs"
              role="group"
              :aria-label="$t(s.title)"
            >
              <button
                v-for="item in personalNav.items"
                :key="item.id"
                :id="'gx-tab-' + item.id"
                type="button"
                class="game-settings__subtab"
                :class="{ 'is-active': personalNav.active === item.id }"
                :aria-current="personalNav.active === item.id ? 'true' : null"
                @click="personalNav.active = item.id"
              >
                <i :class="item.icon" aria-hidden="true"></i>
                <span class="game-settings__subtab-label" :title="$t(item.label)">{{ $t(item.label) }}</span>
                <span v-if="item.changes" class="game-settings__subtab-dot" aria-hidden="true"></span>
                <span v-if="item.changes" class="game-settings__sr">{{ $t('#L_GS_X_CHANGED', item.changes) }}</span>
              </button>
            </div>
          </template>
          <span v-if="ink" class="game-settings__tab-ink l-ink" :class="'is-' + inkMotion" :style="inkStyle" aria-hidden="true"></span>
        </nav>
      </header>


      <!-- Разделы стоят в одной ячейке сетки и сменяются свайпом в сторону
           нажатой вкладки (l-slide-*, launcher.css), как в профиле. -->
      <div class="game-settings__layout">

      <!-- Внешний вид игры: свои «Вернуть» и «Сохранить», нижней строки окна здесь нет. -->
      <transition :name="'l-slide-' + slideDir">
        <div v-if="hasPersonal" class="game-settings__body game-settings__body--personal" v-show="activeSection === 'personal'">
          <GamePersonalization
            ref="personal"
            :game-id="gameId"
            :active="activeSection === 'personal'"
            :section="personalNav.active"
            @dirty="personalDirty = $event"
            @nav="personalNav = $event"
          />
        </div>
      </transition>

      <!-- На странице клавиатура — обычный раздел, без окна поверх; черновик и «Сохранить» общие. -->
      <transition :name="'l-slide-' + slideDir">
        <div v-if="embedded && hasKeyboard" class="game-settings__body game-settings__body--keyboard" v-show="activeSection === 'keyboard'">
          <GameKeyboard
            :game-id="bindsGameId"
            :value="draft.binds"
            :key-binds="shownKeyBinds"
            :saved-value="savedBinds"
            :saved-key-binds="keyBinds"
            :full="keyboardFull"
            @update:full="setKeyboardFull"
            @input="draft.binds = $event"
          />
        </div>
      </transition>

      <transition :name="'l-slide-' + slideDir">
      <div class="game-settings__body" v-show="activeSection === 'launch'">

        <div class="settings-row" v-for="toggle in visibleLaunchToggles" :key="toggle.key">
          <div class="settings-row__text">
            <span class="settings-row__label">{{ $t(toggle.label) }}</span>
            <span class="settings-row__hint">{{
              toggle.hintKey ? $t(toggle.hintKey) : toggle.hint
            }}</span>
          </div>
          <label class="settings-switch">
            <input type="checkbox" v-model="draft[toggle.key]" />
            <span class="settings-switch__track"
              ><span class="settings-switch__thumb"></span
            ></span>
          </label>
        </div>

        <!-- Vulkan (DXVK) у Ghosting на Windows: не аргумент, а окружение запуска
             (src/main/game-delivery/vulkan-backend.js). vulkanAvailable: null — у игры
             переключателя нет, false — DXVK в сборке ещё нет. -->
        <div class="settings-row" v-if="vulkanAvailable !== null && !isLinux">
          <div class="settings-row__text">
            <span class="settings-row__label">{{ $t("#L_GS_VULKAN") }}</span>
            <span class="settings-row__hint" id="game-settings-vulkan-hint">{{
              $t(vulkanAvailable ? "#L_GS_VULKAN_HINT" : "#L_GS_VULKAN_MISSING")
            }}</span>
          </div>
          <label class="settings-switch" :class="{ disabled: !vulkanAvailable }">
            <input
              type="checkbox"
              :checked="vulkanAvailable && draft.vulkan"
              :disabled="!vulkanAvailable"
              :aria-label="$t('#L_GS_VULKAN')"
              aria-describedby="game-settings-vulkan-hint"
              @change="draft.vulkan = $event.target.checked"
            />
            <span class="settings-switch__track"
              ><span class="settings-switch__thumb"></span
            ></span>
          </label>
        </div>

        <!-- HL1: разрешение и частота уходят в -w/-h и -freq. В меню игры с
             RInput клики не работают, поэтому выбирать их удобнее здесь. -->
        <template v-if="isGoldSrc">
          <div class="settings-row">
            <div class="settings-row__text">
              <span class="settings-row__label">{{ $t("#L_GS_RESOLUTION") }}</span>
              <span class="settings-row__hint">{{ $t("#L_GS_RESOLUTION_HINT") }}</span>
            </div>
            <UiDropdown
              :items="resolutionItems"
              :value="resolutionValue"
              align="right"
              @input="setResolution"
            >
              <template #toggle="{ open }">
                <span class="settings-select" :class="{ open: open }">
                  <span>{{ resolutionLabel }}</span>
                  <i class="fas fa-chevron-down" aria-hidden="true"></i>
                </span>
              </template>
            </UiDropdown>
          </div>

          <div class="settings-row">
            <div class="settings-row__text">
              <span class="settings-row__label">{{ $t("#L_GS_REFRESH_RATE") }}</span>
              <span class="settings-row__hint">{{ $t("#L_GS_REFRESH_RATE_HINT") }}</span>
            </div>
            <UiDropdown
              :items="refreshItems"
              :value="draft.refreshRate || 0"
              align="right"
              @input="draft.refreshRate = $event"
            >
              <template #toggle="{ open }">
                <span class="settings-select" :class="{ open: open }">
                  <span>{{ refreshLabel }}</span>
                  <i class="fas fa-chevron-down" aria-hidden="true"></i>
                </span>
              </template>
            </UiDropdown>
          </div>
        </template>

        <!-- Не аргумент запуска, а свойство процесса: маска процессоров
             применяется к уже запущенной игре, поэтому и стоит отдельно
             от переключателей выше. -->
        <div class="settings-row">
          <div class="settings-row__text">
            <span class="settings-row__label">{{ $t("#L_GS_NO_CPU0") }}</span>
            <span class="settings-row__hint">{{ $t("#L_GS_NO_CPU0_HINT") }}</span>
          </div>
          <label class="settings-switch">
            <input type="checkbox" v-model="draft.excludeCpu0" />
            <span class="settings-switch__track"
              ><span class="settings-switch__thumb"></span
            ></span>
          </label>
        </div>

        <!-- Язык игры. Аргументом запуска не передаётся: движок такого
             параметра не знает. Применяется подменой языковых файлов перед
             запуском — подробности в src/main/game-delivery/game-language.js. -->
        <div class="settings-row" v-if="languageAvailable">
          <div class="settings-row__text">
            <span class="settings-row__label">{{ $t("#L_GS_LANG") }}</span>
            <span class="settings-row__hint">{{ $t("#L_GS_LANG_HINT") }}</span>
          </div>
          <UiDropdown
            :items="languageItems"
            :value="draft.language"
            align="right"
            @input="draft.language = $event"
          >
            <template #toggle="{ open }">
              <span class="settings-select" :class="{ open: open }">
                <span>{{ languageLabel }}</span>
                <i class="fas fa-chevron-down" aria-hidden="true"></i>
              </span>
            </template>
          </UiDropdown>
        </div>

        <!-- Тоже не аргумент запуска: таймер поднимается отдельной
             программой уже после того, как процесс игры подтверждён. Если
             таймер не установлен, переключатель просто ничего не сделает. -->
        <div class="settings-row">
          <div class="settings-row__text">
            <span class="settings-row__label">{{ $t("#L_GS_START_TIMER") }}</span>
            <span class="settings-row__hint">{{ $t("#L_GS_START_TIMER_HINT") }}</span>
          </div>
          <label class="settings-switch">
            <input type="checkbox" v-model="draft.startTimer" />
            <span class="settings-switch__track"
              ><span class="settings-switch__thumb"></span
            ></span>
          </label>
        </div>


        <div class="settings-row column">
          <div class="settings-row__text">
            <span class="settings-row__label">{{ $t("#L_GS_ARGS") }}</span>
            <span class="settings-row__hint">{{
              $t(isGoldSrc ? "#L_GS_ARGS_HINT_GOLDSRC" : "#L_GS_ARGS_HINT")
            }}</span>
          </div>
          <input
            type="text"
            class="game-settings__args"
            :class="{ invalid: argsError }"
            v-model="draft.launchArgs"
            spellcheck="false"
            :maxlength="512"
            :placeholder="isGoldSrc ? '-heapsize 524288 +fps_max 100' : '+fps_max 300 -high'"
            :aria-label="$t('#L_GS_ARGS')"
            :aria-invalid="argsError ? 'true' : 'false'"
            @input="argsError = null"
          />
          <span v-if="argsError" class="game-settings__error" role="alert">{{
            argsError
          }}</span>
        </div>

        <!-- Только Linux: переменные окружения и обёртка вокруг Proton, как
             «Параметры запуска» в Steam. Разбор — parseProtonOptions в
             src/main/game-delivery/launch-args.js. -->
        <div class="settings-row column" v-if="isLinux">
          <div class="settings-row__text">
            <span class="settings-row__label">{{ $t("#L_GS_PROTON_OPTIONS") }}</span>
            <span class="settings-row__hint">{{ $t("#L_GS_PROTON_OPTIONS_HINT") }}</span>
          </div>
          <input
            type="text"
            class="game-settings__args"
            :class="{ invalid: protonError }"
            v-model="draft.protonOptions"
            spellcheck="false"
            :maxlength="512"
            placeholder="PROTON_USE_WINED3D=1 mangohud %command%"
            :aria-label="$t('#L_GS_PROTON_OPTIONS')"
            :aria-invalid="protonError ? 'true' : 'false'"
            @input="protonError = null"
          />
          <span v-if="protonError" class="game-settings__error" role="alert">{{
            protonError
          }}</span>
        </div>
      </div>
      </transition>

      <transition :name="'l-slide-' + slideDir">
        <div class="game-settings__body game-settings__body--configs" v-show="activeSection === 'configs'">
          <!-- Строка над редактором: сборка (у Half-Life 2 их три, и конфиги
               у каждой свои), папка конфигов и кнопка, чтобы её открыть. -->
          <div class="game-settings__cfg-bar">
            <UiDropdown
              v-if="configBuildItems.length > 1"
              :items="configBuildItems"
              :value="configBuild"
              @input="onConfigBuild"
            >
              <template #toggle="{ open }">
                <span class="settings-select" :class="{ open: open }" :title="$t('#L_GS_CFG_BUILD_HINT')">
                  <span>{{ $t("#L_GS_CFG_BUILD") }}: {{ configBuildLabel }}</span>
                  <i class="fas fa-chevron-down" aria-hidden="true"></i>
                </span>
              </template>
            </UiDropdown>
            <code class="game-settings__cfg-path" :title="configPathText">{{ configPathText }}</code>
            <button
              v-if="!embedded"
              type="button"
              class="settings-btn"
              :disabled="!configInfo || configInfo.status !== 'ok' || !configInfo.exists"
              @click="openConfigFolder"
            >
              <i class="fas fa-folder-open" aria-hidden="true"></i>
              {{ $t("#L_GS_OPEN_FOLDER") }}
            </button>
          </div>

          <p v-if="configError" class="game-settings__cfg-alert" role="alert">
            <i class="fas fa-exclamation-triangle" aria-hidden="true"></i>{{ configError }}
          </p>
          <p v-else-if="configState && configState.running" class="game-settings__cfg-alert" role="status">
            <i class="fas fa-info-circle" aria-hidden="true"></i>{{ $t("#L_GS_CFG_RUNNING", configState.playerFile) }}
          </p>

          <GameConfigEditor
            v-if="configState && configState.status === 'ok'"
            ref="configEditor"
            :key="configState.gameId"
            class="game-settings__cfg-editor"
            :game-id="configState.gameId"
            :engine="configState.engine"
            :values="configState.values"
            :sources="configState.sources"
            :shadowed="configState.shadowed"
            :player-file="configState.playerFile"
            :value="configChanges"
            :check-counts="ruleCounts"
            :can-leave="leaveCfgFiles"
            @input="configChanges = $event"
          >
            <template #files>
              <GameCfgFiles
                ref="cfgFiles"
                :game-id="configState.gameId"
                :build="configBuild"
                @dirty="cfgFilesDirty = $event"
                @saved="onCfgFileSaved"
              />
            </template>
            <template #check>
              <GameCfgCheck
                :check="ruleCheck"
                :failed="ruleCheckFailed"
                :loading="ruleCheckLoading"
                :category="ruleCategory"
                @category="setRuleCategory"
                @refresh="loadRuleCheck"
                @open-place="showCheckPlace"
              />
            </template>
          </GameConfigEditor>
          <!-- У игры нет понятных настроек, но cfg править можно. -->
          <GameCfgFiles
            v-else-if="configInfo && configInfo.status === 'ok' && configInfo.exists"
            ref="cfgFiles"
            class="game-settings__cfg-editor"
            :game-id="gameId"
            :build="configBuild"
            @dirty="cfgFilesDirty = $event"
            @saved="onCfgFileSaved"
          />
          <div v-else class="game-settings__empty">
            {{ configEmptyText }}
          </div>
        </div>
      </transition>


      <transition :name="'l-slide-' + slideDir">
        <div
          v-if="hasGhosts"
          class="game-settings__body"
          v-show="activeSection === 'ghosts'"
        >
          <div class="game-settings__ghosts-head">
            <span class="settings-row__hint">{{ ghostsSummary }}</span>
            <span class="settings-modal__footer-spacer"></span>
            <button
              type="button"
              class="settings-btn"
              :disabled="runsLoading"
              @click="loadRuns"
            >
              <i class="fas fa-sync-alt" aria-hidden="true"></i>
              {{ $t("#L_GS_REFRESH") }}
            </button>
            <button
              type="button"
              class="settings-btn"
              :disabled="!runsAvailable"
              @click="openRunsFolder"
            >
              <i class="fas fa-folder-open" aria-hidden="true"></i>
              {{ $t("#L_GS_OPEN_RUNS") }}
            </button>
          </div>

          <div v-if="runsLoading" class="game-settings__empty">
            {{ $t("#L_GS_LOADING") }}
          </div>
          <div v-else-if="runsError" class="game-settings__empty">
            {{ runsError }}
          </div>
          <div v-else-if="!runs.length" class="game-settings__empty">
            {{ $t("#L_NO_DATA") }}
          </div>
          <ul v-else class="game-settings__runs">
            <li
              v-for="run in runs"
              :key="run.id"
              class="game-settings__run"
            >
              <button type="button" class="game-settings__run-file" :title="$t('#L_GS_REVEAL')" @click="revealRun(run)">
              <i class="fas fa-ghost" aria-hidden="true"></i>
              <span class="game-settings__run-name">{{ run.displayName || run.name }}</span>
              </button>
              <template v-if="pendingRunDelete !== run.id">
              <span class="game-settings__run-meta">{{ formatSize(run.size) }}</span>
              <span class="game-settings__run-meta">{{ formatDate(run.modifiedAt) }}</span>
              </template>
              <div class="game-settings__run-actions">
                <template v-if="pendingRunDelete === run.id">
                  <span class="game-settings__run-confirm">{{ $t('#L_GS_GHOST_DELETE_CONFIRM') }}</span>
                  <button type="button" class="game-settings__run-delete danger" :disabled="!!deletingRun"
                    :aria-label="$t('#L_GS_GHOST_DELETE_YES')" :title="$t('#L_GS_GHOST_DELETE_YES')" @click="deleteRun(run)">
                    <i :class="deletingRun === run.id ? 'fas fa-spinner fa-spin' : 'fas fa-check'" aria-hidden="true"></i>
                  </button>
                  <button type="button" class="game-settings__run-delete" :disabled="!!deletingRun"
                    :aria-label="$t('#L_CANCEL')" :title="$t('#L_CANCEL')" @click="pendingRunDelete = null">
                    <i class="fas fa-times" aria-hidden="true"></i>
                  </button>
                </template>
                <button v-else type="button" class="game-settings__run-delete" :disabled="!!deletingRun"
                  :aria-label="$t('#L_GS_GHOST_DELETE') + ': ' + (run.displayName || run.name)" :title="$t('#L_GS_GHOST_DELETE')" @click="confirmRunDelete(run)">
                  <i class="fas fa-trash-alt" aria-hidden="true"></i>
                </button>
              </div>
            </li>
          </ul>
          <p v-if="runDeleteError" class="game-settings__run-error" role="alert">{{ runDeleteError }}</p>
        </div>
      </transition>

      <transition :name="'l-slide-' + slideDir">
        <div v-if="hasDemos" class="game-settings__body" v-show="activeSection === 'demos'">
          <GameDemos
            :game-id="gameId"
            :engine="isGoldSrc ? 'goldsrc' : 'ghosting'"
            :active="activeSection === 'demos'"
          />
        </div>
      </transition>

      <!-- Скрипты гайда yavats/hl1-scripts: видео, описание, клавиши. -->
      <transition :name="'l-slide-' + slideDir">
        <div
          v-if="hasScripts"
          v-show="activeSection === 'scripts'"
          class="game-settings__body game-settings__body--personalization"
        >
          <GameScripts
            :value="draft.binds"
            :key-binds="shownKeyBinds"
            :active="activeSection === 'scripts'"
            @input="draft.binds = $event"
          />
        </div>
      </transition>

      <transition :name="'l-slide-' + slideDir">
        <div class="game-settings__body" v-show="activeSection === 'version'">
          <div class="game-settings__facts">
            <div v-for="(fact, index) in versionFacts" :key="fact.label" class="game-settings__fact" :style="{ '--i': index }">
              <span class="game-settings__fact-label">{{ $t(fact.label) }}</span>
              <span class="game-settings__fact-value">{{ fact.value }}</span>
            </div>
          </div>
          <p class="settings-row__hint">{{ $t("#L_GS_VER_SOURCE") }}</p>
        </div>
      </transition>
      </div>

      <footer v-if="activeSection !== 'personal'" class="settings-modal__footer game-settings__footer">
        <button v-if="!embedded" type="button" class="settings-btn ghost" @click="resetDraft">
          {{ $t("#L_RESET") }}
        </button>
        <!-- На странице «Персонализация» сюда встают импорт, экспорт и папка (слот панели). -->
        <slot name="footer-start"></slot>
        <!-- Раздел «Клавиатура» на странице: то, что было в подвале окна клавиатуры.
             Только значок: строка не переносится и в окне 1600 px. -->
        <button
          v-if="activeSection === 'keyboard' && importedCfgActive"
          type="button"
          class="settings-btn ghost game-settings__icon-btn"
          :disabled="importingBinds"
          :title="$t('#L_KB_IMPORT_REMOVE') + ' — ' + $t('#L_KB_IMPORT_REMOVE_HINT')"
          :aria-label="$t('#L_KB_IMPORT_REMOVE')"
          @click="removeImportedCfg"
        >
          <i class="fas fa-file-circle-xmark" aria-hidden="true"></i>
        </button>
        <button
          v-if="!embedded && activeSection === 'configs' && configState && configState.status === 'ok'"
          type="button"
          class="settings-btn ghost"
          :disabled="!!configState.running"
          :aria-busy="importingConfig ? 'true' : null"
          :title="$t('#L_GS_CFG_IMPORT_HINT')"
          @click="importConfig"
        >
          <i class="fas fa-file-import" aria-hidden="true"></i>
          <span>{{ $t("#L_KB_IMPORT") }}</span>
        </button>
        <span class="settings-modal__footer-spacer"></span>
        <div class="game-settings__footer-end">
        <span v-if="fileChanged" class="game-settings__note is-error" role="alert">{{ $t("#L_GS_CFG_CHANGED") }}</span>
        <span
          v-else-if="editedPending"
          class="game-settings__note is-error"
          role="alert"
          :title="$t('#L_GS_SCRIPT_EDITED_TITLE')"
        >{{ $t("#L_GS_SCRIPT_EDITED_TEXT", editedPending.join(", ")) }}</span>
        <span v-else-if="activeSection === 'scripts'" class="game-settings__note">{{ $t("#L_GS_X_APPLY_NOTE") }}</span>
        <span
          v-else-if="activeSection === 'keyboard'"
          class="game-settings__note"
          :class="{ 'is-error': bindsImport && bindsImport.error }"
          :role="bindsImport ? 'status' : null"
          :title="bindsImport && bindsImport.details ? bindsImport.details : null"
        >{{ bindsImport ? bindsImport.text : $t("#L_GS_X_APPLY_NOTE") }}</span>
        <span
          v-else-if="activeSection === 'configs' && configImport"
          class="game-settings__note"
          :class="{ 'is-error': configImport.error }"
          role="status"
          :title="configImport.details || null"
        >{{ configImport.text }}</span>
        <button v-if="fileChanged" type="button" class="settings-btn ghost" :disabled="saving" @click="reloadAfterChange">
          {{ $t("#L_GS_CFG_CHANGED_RELOAD") }}
        </button>
        <template v-else-if="editedPending">
          <button type="button" class="settings-btn danger" :disabled="saving" @click="deleteEdited">
            {{ $t("#L_GS_SCRIPT_EDITED_DELETE") }}
          </button>
          <button type="button" class="settings-btn ghost" :aria-label="$t('#L_CANCEL')" :title="$t('#L_CANCEL')" @click="editedPending = null">
            <i class="fas fa-times" aria-hidden="true"></i>
          </button>
        </template>
        <!-- «Демо», «Призраки» и «Версия» ничего не сохраняют через подвал. -->
        <template v-if="footerSaves">
        <button v-if="embedded" type="button" class="settings-btn ghost" @click="resetDraft">
          {{ $t("#L_RESET") }}
        </button>
        <button type="button" class="settings-btn" @click="onCancel">
          {{ $t("#L_CANCEL") }}
        </button>
        <button
          type="button"
          class="settings-btn primary"
          :disabled="saving || !hasUnsaved()"
          @click="submit()"
        >
          {{ $t("#L_SAVE") }}
        </button>
        </template>
        </div>
      </footer>
    </div>

    <!-- Клавиатура — отдельное окно во весь лаунчер поверх настроек: схеме
         и списку действий нужно место. Окно собрано заранее и только
         скрыто, поэтому открывается одним проявлением без перестройки
         раскладки. Черновик общий с настройками: «Сохранить» сохраняет всё. -->
    <div
      v-if="hasKeyboard && !embedded"
      ref="keyboard"
      class="settings-modal__window game-settings__kb-window"
      :class="{ 'is-open': keyboardOpen }"
      role="dialog"
      aria-modal="true"
      :aria-label="$t('#L_GS_S_KEYBOARD')"
      :inert="!keyboardOpen || null"
    >
      <!-- Шапка окна клавиатуры: «Назад» к остальным настройкам, название
           и игра. Черновик биндов общий: «Сохранить» внизу сохраняет всё. -->
      <header class="game-settings__head game-settings__head--keyboard">
        <button
          ref="back"
          type="button"
          class="game-settings__back"
          :title="$t('#L_BACK')"
          @click="closeKeyboard"
        >
          <i class="fas fa-arrow-left" aria-hidden="true"></i>
          <span>{{ $t("#L_BACK") }}</span>
        </button>
        <span class="game-settings__head-sep" aria-hidden="true"></span>
        <div class="game-settings__titles">
          <h2 class="game-settings__title">{{ $t("#L_GS_S_KEYBOARD") }}</h2>
          <span class="game-settings__game">{{ gameTitle }}</span>
        </div>
      </header>

      <div class="game-settings__keyboard">
        <GameKeyboard
          :game-id="bindsGameId"
          :value="draft.binds"
          :key-binds="shownKeyBinds"
          :saved-value="savedBinds"
          :saved-key-binds="keyBinds"
          :full="keyboardFull"
          @update:full="setKeyboardFull"
          @input="draft.binds = $event"
        />
      </div>

      <footer class="settings-modal__footer game-settings__footer">
        <button type="button" class="settings-btn ghost" @click="resetDraft">
          {{ $t("#L_RESET") }}
        </button>
        <!-- На странице «Персонализация» импорт — кнопкой внизу вкладки (R18): второй поток импорта
             записал бы поверх импорта черновик окна, снятый до него. -->
        <button
          v-if="!embedded"
          type="button"
          class="settings-btn ghost"
          :aria-busy="importingBinds ? 'true' : null"
          :title="$t(wholeCfgImport ? '#L_KB_IMPORT_CFG_HINT' : '#L_KB_IMPORT_HINT')"
          @click="importBinds"
        >
          <i class="fas fa-file-import" aria-hidden="true"></i>
          <span>{{ $t("#L_KB_IMPORT") }}</span>
        </button>
        <button
          v-if="importedCfgActive"
          type="button"
          class="settings-btn ghost"
          :disabled="importingBinds"
          :title="$t('#L_KB_IMPORT_REMOVE_HINT')"
          @click="removeImportedCfg"
        >
          <i class="fas fa-file-circle-xmark" aria-hidden="true"></i>
          <span>{{ $t("#L_KB_IMPORT_REMOVE") }}</span>
        </button>
        <span class="settings-modal__footer-spacer"></span>
        <span v-if="fileChanged" class="game-settings__note is-error" role="alert">{{ $t("#L_GS_CFG_CHANGED") }}</span>
        <span
          v-else-if="editedPending"
          class="game-settings__note is-error"
          role="alert"
          :title="$t('#L_GS_SCRIPT_EDITED_TITLE')"
        >{{ $t("#L_GS_SCRIPT_EDITED_TEXT", editedPending.join(", ")) }}</span>
        <span
          v-else
          class="game-settings__note"
          :class="{ 'is-error': bindsImport && bindsImport.error }"
          :role="bindsImport ? 'status' : null"
          :title="bindsImport && bindsImport.details ? bindsImport.details : null"
        >{{ bindsImport ? bindsImport.text : $t("#L_GS_X_APPLY_NOTE") }}</span>
        <button v-if="fileChanged" type="button" class="settings-btn ghost" :disabled="saving" @click="reloadAfterChange">
          {{ $t("#L_GS_CFG_CHANGED_RELOAD") }}
        </button>
        <template v-else-if="editedPending">
          <button type="button" class="settings-btn danger" :disabled="saving" @click="deleteEdited">
            {{ $t("#L_GS_SCRIPT_EDITED_DELETE") }}
          </button>
          <button type="button" class="settings-btn ghost" :aria-label="$t('#L_CANCEL')" :title="$t('#L_CANCEL')" @click="editedPending = null">
            <i class="fas fa-times" aria-hidden="true"></i>
          </button>
        </template>
        <button type="button" class="settings-btn" @click="onCancel">
          {{ $t("#L_CANCEL") }}
        </button>
        <button
          type="button"
          class="settings-btn primary"
          :disabled="saving"
          @click="submit()"
        >
          {{ $t("#L_SAVE") }}
        </button>
      </footer>
    </div>
    <!-- Перед импортом HL1: окон выбора файла будет несколько (по одному на exec). -->
    <Teleport to=".launcher-root">
      <transition name="l-modal">
        <ConfirmModal
          v-if="importIntro"
          :title="$t('#L_CFG_IMPORT_INTRO_TITLE')"
          :text="$t('#L_CFG_IMPORT_INTRO_TEXT')"
          :items="[$t('#L_CFG_IMPORT_INTRO_STEP1'), $t('#L_CFG_IMPORT_INTRO_STEP2'), $t('#L_CFG_IMPORT_INTRO_STEP3')]"
          :confirm-label="$t('#L_CFG_IMPORT_INTRO_OK')"
          @resolve="onImportIntro"
        />
      </transition>
    </Teleport>
  </div>
</template>

<script>
import uiState from "./ui-state";
import { trapFocus } from "./focus-trap";
import { inkBox, direction } from "./tab-ink";
import UiDropdown from "./UiDropdown";
import ConfirmModal from "./ConfirmModal";
import GameKeyboard from "./GameKeyboard";
import GameCfgFiles from "./GameCfgFiles";
import GameCfgCheck from "./GameCfgCheck";
import GameScripts from "./GameScripts";
import GameDemos from "./GameDemos";
import { formatSize, formatDate } from "./file-format";
import GameConfigEditor from "./GameConfigEditor";
import GamePersonalization from "./GamePersonalization";
import { defaultPersonalization, normalizePersonalization } from "./ghosting-personalization-defaults";
import { GHOSTING_GAME_ID, GOLDSRC_GAME_IDS } from "./launcher-data";
import prefsStore from "./launcher-prefs";
const gameBinds = require("../../../shared/game-binds");
const bindPresets = require("../../../shared/bind-presets");
const cfgImport = require("../../../shared/cfg-import");
const importAdopt = require("../../../shared/goldsrc-import-adopt");
const gameConfig = require("../../../shared/game-config");

const { ipcRenderer } = window.hlsrElectron;
const IS_LINUX = window.hlsrElectron.platform === "linux";

const LAUNCH_TOGGLES = [
  { key: "console", label: "#L_GS_CONSOLE", hint: "-console" },
  { key: "novid", label: "#L_GS_NOVID", hint: "-novid" },
  { key: "borderless", label: "#L_GS_BORDERLESS", hint: "-windowed -noborder" },
  { key: "dx9", label: "#L_GS_DX9", hint: "-dx9" },
  { key: "nocrashdialog", label: "#L_GS_NOCRASHDIALOG", hint: "-nocrashdialog" },
  { key: "noasync", label: "#L_GS_NOASYNC", hint: "-noasync" },
  { key: "noaafonts", label: "#L_GS_NOAAFONTS", hint: "-noaafonts" },
  { key: "high", label: "#L_GS_HIGH", hint: "-high" },
];

// HL1 (WON 2005): только то, что движок действительно читает из командной
// строки. RInput не аргумент: лаунчер запускает его рядом с игрой.
const GOLDSRC_TOGGLES = [
  { key: "rinput", label: "#L_GS_RINPUT", hintKey: "#L_GS_RINPUT_HINT" },
  // Патч курсора в меню (Windows и Linux) имеет смысл только вместе с RInput.
  { key: "rinputMenuFix", label: "#L_GS_RINPUT_MENUFIX", hintKey: "#L_GS_RINPUT_MENUFIX_HINT", requires: "rinput" },
  { key: "windowed", label: "#L_GS_WINDOWED", hint: "-windowed" },
  { key: "console", label: "#L_GS_CONSOLE", hint: "-console" },
  { key: "developer", label: "#L_GS_DEVELOPER", hint: "-dev" },
  { key: "condebug", label: "#L_GS_CONDEBUG", hint: "-condebug" },
  { key: "nojoy", label: "#L_GS_NOJOY", hint: "-nojoy" },
  { key: "noaafonts", label: "#L_GS_NOAAFONTS", hint: "-noaafonts" },
];

const RESOLUTIONS = [
  [640, 480], [800, 600], [1024, 768], [1280, 960], [1440, 1080],
  [1280, 720], [1366, 768], [1600, 900], [1920, 1080], [2560, 1440], [3840, 2160],
  [1680, 1050], [1920, 1200],
];

const REFRESH_RATES = [60, 75, 100, 120, 144, 165, 180, 240, 360];

const EMPTY = LAUNCH_TOGGLES.reduce((acc, toggle) => Object.assign(acc, { [toggle.key]: false }), {
  rinput: true,
  rinputMenuFix: false,
  windowed: false,
  developer: false,
  condebug: false,
  nojoy: false,
  width: 0,
  height: 0,
  refreshRate: 0,
  launchArgs: "",
  protonOptions: "",
  excludeCpu0: false,
  startTimer: false,
  vulkan: false,
  language: "auto",
  personalization: null,
  packs: null,
  bxt: null,
  binds: {},
});

function withPersonalization(settings) {
  const base = Object.assign({}, EMPTY, settings || {});
  base.personalization = normalizePersonalization(base.personalization);
  return base;
}

const LANGUAGE_NAMES = {
  english: "English",
  russian: "Русский",
};

export default {
  name: "GameSettingsModal",
  components: { UiDropdown, ConfirmModal, GameKeyboard, GameScripts, GameDemos, GameConfigEditor, GameCfgFiles, GameCfgCheck, GamePersonalization },
  emits: ["close", "saved", "file-saved", "ready", "load-failed"],
  props: {
    gameId: {
      type: String,
      required: true,
    },
    gameTitle: {
      type: String,
      required: true,
    },
    // Страница «Персонализация»: окно стоит на странице, без подложки и крестика.
    embedded: {
      type: Boolean,
      default: false,
    },
    // Страница «Персонализация» передаёт свой вопрос о несохранённом (сохранить, отбросить, остаться):
    // async () => true, если можно идти дальше. Без него окно спрашивает confirm редактора cfg.
    askUnsaved: {
      type: Function,
      default: null,
    },
    // Раздел, открытый сразу: "configs" у шестерёнки и опроса; пусто — первый в списке.
    initialSection: {
      type: String,
      default: "",
    },
  },
  data() {
    return {
      // Окно-подсказка перед импортом HL1: { resolve } пока открыто (confirmImportIntro).
      importIntro: null,
      ghostingGameId: GHOSTING_GAME_ID,
      draft: withPersonalization({}),
      argsError: null,
      protonError: null,
      isLinux: IS_LINUX,
      saving: false,
      lastFocused: null,
      section: this.initialSection,
      keyboardOpen: false,
      // В разделе «Персонализация» есть несохранённое (событие dirty GamePersonalization).
      personalDirty: false,
      // Подразделы редактора внешнего вида для списка слева: { active, items: [{ id, icon, label, changes }] }.
      personalNav: { active: "", items: [] },
      ink: null,
      inkMotion: "still",
      slideDir: "next",
      languages: [],
      // Переключатель Vulkan: null — у игры его нет, false — в сборке нет DXVK.
      vulkanAvailable: null,
      keyBinds: null,
      bindsGameId: null,
      importingBinds: false,
      bindsImport: null,
      // Конфиг HL1, который подключится после «Сохранить»: { name, text }.
      pendingCfgImport: null,
      // Клавиатура открывается своим окном только вне страницы «Персонализация».
      allSections: [
        { id: "personal", icon: "fas fa-palette", title: "#L_GS_S_PERSONAL", personalOnly: true },
        { id: "configs", icon: "fas fa-file-code", title: "#L_GS_S_CONFIGS" },
        { id: "keyboard", icon: "fas fa-keyboard", title: "#L_GS_S_KEYBOARD", keyboardOnly: true, window: !this.embedded },
        { id: "scripts", icon: "fas fa-scroll", title: "#L_GS_S_SCRIPTS", scriptsOnly: true },
        { id: "launch", icon: "fas fa-play", title: "#L_GS_S_LAUNCH" },
        { id: "demos", icon: "fas fa-film", title: "#L_GS_S_DEMOS", demosOnly: true },
        { id: "ghosts", icon: "fas fa-ghost", title: "#L_GS_S_GHOSTS", ghostingOnly: true },
        { id: "version", icon: "fas fa-code-branch", title: "#L_GS_S_VERSION", ghostingOnly: true },
      ],
      // Проверка конфига HL1 по правилам speedrun.com: ответ player-config-check (status "ok") или null.
      ruleCheck: null,
      ruleCheckLoading: false,
      // Категория проверки (HL1): запоминается в launcher-prefs.
      ruleCategory: prefsStore.ruleCategoryOf(),
      keyboardFull: !!prefsStore.load().keyboardFull,
      // Перепроверка не удалась (сбой main или IPC): прежний итог остаётся, раздел не исчезает.
      ruleCheckFailed: false,
      configInfo: null,
      configBuild: null,
      configState: null,
      configChanges: {},
      // Черновик в виде, в каком он загружен или сохранён: по нему видно несохранённые правки.
      draftSaved: null,
      // Загрузка настроек: "loading" | "ready" | "failed" (страница «Персонализация» читает его, а не только ждёт событие).
      loadState: "loading",
      importingConfig: false,
      // Итог импорта cfg на страницу «Персонализация»: { error, text, details }.
      configImport: null,
      configError: null,
      runs: [],
      runsLoading: false,
      runsError: null,
      runsTruncated: false,
      pendingRunDelete: null,
      deletingRun: null,
      // Блоки скриптов, правленные руками: остались в файле игрока, ждём ответа «Удалить?».
      editedPending: null,
      // Сохранение отклонено: файл игрока изменён вне лаунчера (CONFIG_CHANGED).
      fileChanged: false,
      configSha256: undefined,
      // В редакторе текста cfg (GameCfgFiles) есть несохранённая правка.
      cfgFilesDirty: false,
      importUndoable: false,
      runDeleteError: null,
      delivery: null,
    };
  },
  computed: {
    // Любая игра с файлом игрока импортирует конфиг целиком (§8.1): и alias, и бинды на них.
    wholeCfgImport() {
      return !!this.bindsGameId && !!bindPresets.engineFor(this.bindsGameId);
    },
    importedCfgOnDisk() {
      if (this.importUndoable) return true;
      if (this.keyBinds && Array.isArray(this.keyBinds.imported) && this.keyBinds.imported.length) return true;
      // Импорт прошлых версий лаунчера (hlsr-import.cfg).
      const files = (this.keyBinds && this.keyBinds.files) || [];
      return files.some((name) => String(name).toLowerCase() === gameBinds.IMPORT_FILE);
    },
    importedCfgActive() {
      return this.wholeCfgImport && (!!this.pendingCfgImport || this.importedCfgOnDisk);
    },
    // Бинды для клавиатуры и «Скриптов»: с ещё не сохранённым импортом —
    // как они будут в игре после «Сохранить».
    shownKeyBinds() {
      if (!this.pendingCfgImport) return this.keyBinds;
      return gameBinds.withImport(this.keyBinds, this.pendingCfgImport.text);
    },
    // Переназначения последнего сохранения (не черновик) — для «Пианино».
    savedBinds() {
      return this.draftSaved ? JSON.parse(this.draftSaved).binds || {} : null;
    },
    sections() {
      return this.allSections.filter(
        (item) =>
          (!item.personalOnly || this.hasPersonal) &&
          (!item.ghostingOnly || this.gameId === GHOSTING_GAME_ID) &&
          (!item.demosOnly || this.hasDemos) &&
          (!item.keyboardOnly || this.hasKeyboard) &&
          (!item.scriptsOnly || this.hasScripts)
      );
    },
    // Клавиатура есть у игр с заготовками биндов; у HL2 — по выбранной сборке.
    hasKeyboard() {
      return !!(this.bindsGameId && bindPresets.catalogFor(this.bindsGameId));
    },
    // Скрипты гайда есть только у Half-Life 1.
    hasScripts() {
      return this.bindsGameId === "hl1-won";
    },
    isGoldSrc() {
      return GOLDSRC_GAME_IDS.includes(this.gameId);
    },
    // Внешний вид игры (GamePersonalization) — только на странице и только у Ghosting и HL1.
    hasPersonal() {
      return this.embedded && (this.isGoldSrc || this.gameId === GHOSTING_GAME_ID);
    },
    // Раздел «Демо» есть у HL1 и у Ghosting.
    hasDemos() {
      return this.isGoldSrc || this.gameId === GHOSTING_GAME_ID;
    },
    visibleLaunchToggles() {
      if (!this.isGoldSrc) return LAUNCH_TOGGLES;
      return GOLDSRC_TOGGLES.filter((toggle) =>
        !toggle.requires || this.draft[toggle.requires]);
    },
    resolutionItems() {
      const items = [{ id: "auto", label: this.$t("#L_GS_RESOLUTION_AUTO") }];
      for (const [w, h] of RESOLUTIONS) items.push({ id: `${w}x${h}`, label: `${w} × ${h}` });
      const current = this.resolutionValue;
      if (!items.some((item) => item.id === current))
        items.push({ id: current, label: current.replace("x", " × ") });
      return items;
    },
    resolutionValue() {
      return this.draft.width && this.draft.height
        ? `${this.draft.width}x${this.draft.height}`
        : "auto";
    },
    resolutionLabel() {
      const found = this.resolutionItems.find((item) => item.id === this.resolutionValue);
      return found ? found.label : this.resolutionValue;
    },
    refreshItems() {
      const items = [{ id: 0, label: this.$t("#L_GS_RESOLUTION_AUTO") }];
      for (const hz of REFRESH_RATES) items.push({ id: hz, label: this.$t("#L_GS_HZ", hz) });
      const current = this.draft.refreshRate || 0;
      if (!items.some((item) => item.id === current)) items.push({ id: current, label: this.$t("#L_GS_HZ", current) });
      return items;
    },
    refreshLabel() {
      const found = this.refreshItems.find((item) => item.id === (this.draft.refreshRate || 0));
      return found ? found.label : "";
    },
    inkStyle() {
      return { left: this.ink.left + "px", right: this.ink.right + "px" };
    },
    // Раздел «Проверка» есть только у HL1 и только когда проверка прошла.
    ruleCounts() {
      const check = this.ruleCheck;
      if (check && check.status === "ok") {
        return { errors: check.stats.errors, warnings: check.stats.warnings, failed: this.ruleCheckFailed };
      }
      return this.ruleCheckFailed ? { errors: 0, warnings: 0, failed: true } : null;
    },
    activeSection() {
      return this.sections.some((item) => item.id === this.section && !item.window)
        ? this.section
        : this.sections[0].id;
    },
    footerSaves() {
      return !["demos", "ghosts", "version"].includes(this.activeSection);
    },
    configBuildItems() {
      const builds = (this.configInfo && this.configInfo.builds) || [];

      return builds.map((item) => ({
        id: item.build,
        label: item.installed
          ? item.build
          : `${item.build} (${this.$t("#L_GS_CFG_BUILD_ABSENT")})`,
      }));
    },
    configBuildLabel() {
      const found = this.configBuildItems.find((item) => item.id === this.configBuild);
      return found ? found.label : this.$t("#L_NO_DATA");
    },
    languageItems() {
      const items = [{ id: "auto", label: this.$t("#L_GS_LANG_AUTO") }];

      for (const id of this.languages)
        if (LANGUAGE_NAMES[id]) items.push({ id, label: LANGUAGE_NAMES[id] });

      return items;
    },
    languageAvailable() {
      return this.languageItems.length > 1;
    },
    languageLabel() {
      const found = this.languageItems.find((item) => item.id === this.draft.language);
      return found ? found.label : this.$t("#L_GS_LANG_AUTO");
    },
    configEmptyText() {
      const status = this.configState ? this.configState.status : null;
      if (!status) return this.$t("#L_GS_LOADING");
      if (status === "not-installed") return this.$t("#L_GS_NOT_INSTALLED");
      return this.$t("#L_NO_DATA");
    },
    configPathText() {
      if (!this.configInfo) return this.$t("#L_NO_DATA");
      if (this.configInfo.status === "not-installed") return this.$t("#L_GS_NOT_INSTALLED");
      if (this.configInfo.status !== "ok") return this.$t("#L_NO_DATA");
      return this.configInfo.exists ? this.configInfo.path : this.$t("#L_GS_CFG_MISSING");
    },
    runsAvailable() {
      return !this.runsError && !this.runsLoading;
    },
    // Записи .run есть только у Ghosting; демо живут в GameDemos.
    hasGhosts() {
      return this.sections.some((item) => item.id === "ghosts");
    },
    ghostsSummary() {
      if (this.runsLoading) return this.$t("#L_GS_LOADING");
      if (this.runsError) return this.runsError;
      const suffix = this.runsTruncated ? " +" : "";
      return this.$t("#L_GS_GHOSTS_COUNT", String(this.runs.length) + suffix);
    },
    versionFacts() {
      const d = this.delivery || {};
      const meta = d.buildMetadata || null;
      const none = this.$t("#L_NO_DATA");

      return [
        { label: "#L_GS_V_NAME", value: meta && meta.buildName ? meta.buildName : none },
        { label: "#L_GS_V_GAME", value: d.installedVersion || none },
        { label: "#L_GS_V_BUILD", value: meta ? meta.buildId : none },
        { label: "#L_GS_V_BUILT", value: this.dateText(d.buildDate) },
        { label: "#L_GS_V_INSTALLED", value: this.dateText(d.installedAt) },
        {
          label: "#L_GS_V_TRUST",
          value: this.$t("#L_GS_TRUST_" + String(d.metadataState || "unknown").toUpperCase().replace(/-/g, "_")),
        },
        {
          label: "#L_GS_V_MINLAUNCHER",
          value: meta && meta.minimumLauncherVersion ? meta.minimumLauncherVersion : none,
        },
      ];
    },
  },
  watch: {
    activeSection(id, previous) {
      this.pendingRunDelete = null;
      this.runDeleteError = null;
      if (previous === "keyboard") this.bindsImport = null;
      // «Персонализация» и остальные разделы пишут файл игрока каждая сама: сторона, куда
      // перешли, перечитывает его, если в ней нет правок (как при возврате на вкладку, R18).
      if (id === "personal" && this.$refs.personal) this.$refs.personal.onWindowFocus();
      else if (previous === "personal") this.onWindowFocus();
      this.slideDir = direction(this.sections.map((item) => item.id), previous, id);
      this.$nextTick(() => this.measureInk(this.slideDir));
    },
    // Вкладки клавиатуры и скриптов появляются, когда лаунчер узнал сборку.
    sections() {
      this.$nextTick(() => this.measureInk("still"));
    },
    // Сборка без клавиатуры — окно клавиатуры закрывается само.
    hasKeyboard(value) {
      if (!value) this.keyboardOpen = false;
    },
  },
  created() {
    this.tabEls = {};
  },
  methods: {
    // «Персонализация» сохраняется своей кнопкой, остальные разделы — общей «Сохранить» внизу:
    // переход между ними задаёт вопрос страницы о несохранённом там, откуда уходим.
    // false — игрок остался в разделе.
    async selectSection(id) {
      const from = this.activeSection;
      if (id === from) return true;
      const part = from === "personal" ? "personal" : id === "personal" ? "settings" : null;
      if (part && this.askUnsaved && !(await this.askUnsaved(part))) return false;
      this.section = id;
      return true;
    },
    openKeyboard() {
      this.keyboardOpen = true;
      this.$nextTick(() => this.$refs.back && this.$refs.back.focus({ preventScroll: true }));
    },
    // Назад: окно клавиатуры гаснет над нетронутым окном настроек, фокус
    // возвращается на вкладку «Клавиатура».
    closeKeyboard() {
      this.keyboardOpen = false;
      this.bindsImport = null;
      this.$nextTick(() => this.tabEls.keyboard && this.tabEls.keyboard.focus({ preventScroll: true }));
    },
    // Импорт cfg. Half-Life 1 подключает файлы целиком, под их именами: с ними
    // работают alias, скрипты и перебинды из файла. Ghosting переносит только
    // бинды — обычные переназначения в черновике. Для каждого cfg, который
    // файл вызывает через exec, открывается ещё одно окно выбора с его именем
    // (src/shared/cfg-import.js). До «Сохранить» ничего не записано.
    async importBinds() {
      if (this.importingBinds || !this.bindsGameId) return;
      this.importingBinds = true;
      try {
        if (!(await this.confirmImportIntro(bindPresets.engineFor(this.bindsGameId)))) return;
        const ipc = window.hlsrElectron.ipcRenderer;
        const result = await ipc.invoke("player-config-import-pick", this.bindsGameId, { multiple: true });
        if (!result || result.status === "canceled") return;
        if (result.status !== "ok") {
          const key = result.code === "BINDS_FILE_TOO_LARGE" ? "#L_KB_IMPORT_TOO_LARGE" : "#L_KB_IMPORT_FAILED";
          this.bindsImport = { error: true, text: this.$t(key) };
          return;
        }
        const engine = bindPresets.engineFor(this.bindsGameId);
        const chosen = await this.withCompanions(this.bindsGameId, engine, result.files || [result], result.dir);
        const { root, files: picked } = cfgImport.rootOfPicked(chosen);
        result.name = root.name;
        const { files, missing } = await this.pickImportChain(this.bindsGameId, root, result.dir, engine, !this.wholeCfgImport, picked);
        const built = cfgImport.buildImport({ engine, root, files, missing });
        if (this.wholeCfgImport && built.bytes > gameBinds.MAX_IMPORT_BYTES) {
          this.bindsImport = { error: true, text: this.$t("#L_KB_IMPORT_TOO_LARGE") };
          return;
        }
        const report = this.importReport(built.report);
        if (this.wholeCfgImport) {
          const preview = gameBinds.withImport(null, built.flatText);
          const bindCount = Object.keys(preview.binds).length;
          const aliasCount = Object.keys(preview.aliases).length;
          if (!bindCount && !aliasCount) {
            this.bindsImport = { error: true, text: this.$t("#L_KB_IMPORT_EMPTY", result.name) };
            return;
          }
          // Отмена импорта до «Сохранить» возвращает то, что импорт поменял
          // в черновике; повторный импорт — состояние до первого.
          const undo = (this.pendingCfgImport && this.pendingCfgImport.undo) || {
            binds: Object.assign({}, this.draft.binds),
            personalization: this.draft.personalization,
            bxt: this.draft.bxt,
          };
          // Прицел, HUD и FOV переносятся в персонализацию только у GoldSrc.
          const extra = engine === "goldsrc" ? this.adoptImport(built.flatText) : { short: [], details: "" };
          this.pendingCfgImport = { name: result.name, text: built.flatText, payload: { root, files, missing }, undo };
          this.bindsImport = {
            error: false,
            text: [this.$t("#L_KB_IMPORT_CFG_DONE", result.name, String(bindCount), String(aliasCount))]
              .concat(extra.short, report.short).join(" "),
            details: [report.details, extra.details].filter(Boolean).join("\n"),
          };
          return;
        }
        const keyBinds = this.keyBinds || {};
        const imported = gameBinds.importOverrides(built.flatText, {
          fileBinds: keyBinds.binds,
          aliases: keyBinds.aliases,
          overrides: this.draft.binds,
          knownCommand: (command) => !!bindPresets.presetOf(bindPresets.catalogFor(this.bindsGameId), command),
        });
        if (!imported.found) {
          this.bindsImport = { error: true, text: this.$t("#L_KB_IMPORT_EMPTY", result.name) };
          return;
        }
        this.draft.binds = imported.overrides;
        const skipped = imported.skippedAlias + imported.skippedInvalid + imported.skippedKey;
        const parts = [this.$t("#L_KB_IMPORT_DONE", result.name, String(imported.changed + imported.unbound))];
        if (skipped) parts.push(this.$t("#L_KB_IMPORT_SKIPPED", String(skipped)));
        this.bindsImport = { error: false, text: parts.concat(report.short).join(" "), details: report.details };
      } catch (error) {
        this.bindsImport = { error: true, text: this.$t("#L_KB_IMPORT_FAILED") };
      } finally {
        this.importingBinds = false;
      }
    },
    // Импорт HL1 целиком: что лаунчер пишет после него, не должно молча
    // перекрыть конфиг игрока. Прицел, HUD и поле зрения из конфига
    // становятся персонализацией лаунчера (goldsrc-import-adopt.js), а
    // переназначения лаунчера на клавишах, которые конфиг назначает сам,
    // снимаются. { short: [строки отчёта], details }.
    adoptImport(flatText) {
      const short = [];
      const details = [];
      const adopted = importAdopt.adoptImported(flatText, { personalization: this.draft.personalization, bxt: this.draft.bxt || {} });
      if (adopted.adopted.length) {
        this.draft.personalization = normalizePersonalization(adopted.personalization);
        this.draft.bxt = adopted.bxt;
        short.push(this.$t("#L_KB_IMPORT_ADOPTED", String(adopted.adopted.length)));
        details.push(this.$t("#L_KB_IMPORT_ADOPTED_LIST", adopted.adopted.join(", ")));
      }
      if (adopted.skipped.length) {
        short.push(this.$t("#L_KB_IMPORT_NOT_ADOPTED", String(adopted.skipped.length)));
        details.push(this.$t("#L_KB_IMPORT_NOT_ADOPTED_LIST", adopted.skipped.join(", ")));
      }
      const touched = gameBinds.importTouchedKeys(flatText);
      const binds = Object.assign({}, this.draft.binds);
      const dropped = Object.keys(binds).filter((key) => touched.has(key));
      if (dropped.length) {
        dropped.forEach((key) => { delete binds[key]; });
        this.draft.binds = binds;
        short.push(this.$t("#L_KB_IMPORT_OVERRIDES_DROPPED", dropped.join(", ")));
      }
      return { short, details: details.join("\n") };
    },
    // Импорт HL1 спрашивает каждый файл цепочки exec своим окном выбора: заранее сказать,
    // что «Открыть» придётся нажать несколько раз (просьба владельца 14.11). true — продолжать.
    confirmImportIntro(engine) {
      if (engine !== "goldsrc") return Promise.resolve(true);
      // Второй вызов, пока открыта подсказка (страница и окно импортируют каждый своей
      // кнопкой): прежний ждущий получает «нет», а не зависает с занятым флагом.
      if (this.importIntro) this.importIntro.resolve(false);
      return new Promise((resolve) => {
        this.importIntro = { resolve };
      });
    },
    onImportIntro(answer) {
      const pending = this.importIntro;
      this.importIntro = null;
      if (pending) pending.resolve(answer === true);
    },
    // Файлы игрока, которые движок выполнил бы сам рядом с выбранными
    // (cfgImport.companionsOf): окно выбора открывается сразу на них.
    // Отмена — импорт без этого файла.
    async withCompanions(gameId, engine, picked, dir) {
      if (engine !== "goldsrc") return picked;
      const list = picked.slice();
      for (const want of cfgImport.companionsOf(list.map((file) => file.name))) {
        const chosen = await window.hlsrElectron.ipcRenderer.invoke("player-config-import-pick", gameId, {
          want: { name: want.name, from: "valve.rc" },
          dir,
        });
        if (!chosen || chosen.status !== "ok") continue;
        if (want.before) list.unshift({ name: chosen.name, text: chosen.text });
        else list.push({ name: chosen.name, text: chosen.text });
      }
      return list;
    },
    // Файлы цепочки exec: по одному окну выбора на каждый. Отмена окна —
    // файл не подключается, его exec в конфиге отключается. Импорту только
    // биндов (Ghosting) и настроек (страница «Персонализация») нужны лишь exec,
    // которые выполняются сразу (topLevelOnly), а не по клавише.
    async pickImportChain(gameId, root, dir, engine, topLevelOnly, picked) {
      const files = Object.assign({}, picked);
      const missing = [];
      for (let i = 0; i < cfgImport.MAX_FILES; i += 1) {
        const want = cfgImport.nextWanted({ engine, root, files, missing, topLevelOnly });
        if (!want) break;
        const chosen = await window.hlsrElectron.ipcRenderer.invoke("player-config-import-pick", gameId, {
          want: { name: want.name, from: want.from },
          dir,
        });
        if (chosen && chosen.status === "ok") {
          // target — имя из exec: под ним файл ляжет в папку игры.
          files[want.key] = { name: chosen.name, target: want.name, text: chosen.text };
          dir = chosen.dir || dir;
        } else {
          missing.push(want.key);
        }
      }
      return { files, missing };
    },
    importReport(report) {
      const short = [];
      const details = [];
      const list = (items) => items.slice(0, 4).join(", ") + (items.length > 4 ? ` +${items.length - 4}` : "");
      if (report.files.length) short.push(this.$t("#L_KB_IMPORT_CHAIN", String(report.files.length)));
      if (report.missing.length) {
        short.push(this.$t("#L_KB_IMPORT_CHAIN_MISSING", list(report.missing)));
      }
      if (report.cycles.length) {
        short.push(this.$t("#L_KB_IMPORT_LOOP", list(report.cycles.map(([from, to]) => `${from} → ${to}`))));
      }
      if (report.duplicateFiles.length) {
        short.push(this.$t("#L_KB_IMPORT_DUP_FILES", list(report.duplicateFiles.map(([a, b]) => `${a} = ${b}`))));
      }
      if (report.duplicates.length) {
        const label = (item) => (item.kind === "cvar" ? item.name : `${item.kind} ${item.name}`);
        short.push(this.$t("#L_KB_IMPORT_DUPLICATES", list(report.duplicates.map(label))));
        for (const item of report.duplicates) {
          details.push(`${label(item)}: ${[...new Set(item.files)].join(", ")} → ${item.value || "—"}`);
        }
      }
      return { short, details: details.length ? details.join("\n") : null };
    },
    setTabRef(id, el) {
      if (el) this.tabEls[id] = el;
      else delete this.tabEls[id];
    },
    // Полоса под вкладкой (tab-ink.js), как в профиле. При изменении
    // размеров окна — без анимации.
    measureInk(motion) {
      if (this.embedded) return; // у списка слева полосы нет
      const el = this.tabEls[this.activeSection];
      const box = inkBox(el && el.parentElement, el);
      if (!box) return;
      this.inkMotion = this.ink && typeof motion === "string" ? motion : "still";
      this.ink = box;
    },
    dateText(iso) {
      if (!iso) return this.$t("#L_NO_DATA");
      const date = new Date(iso);
      if (isNaN(date.getTime())) return this.$t("#L_NO_DATA");
      const pad = (n) => String(n).padStart(2, "0");
      return `${pad(date.getDate())}.${pad(date.getMonth() + 1)}.${date.getFullYear()}`;
    },
    formatSize(bytes) { return formatSize(bytes, this.$t); },
    formatDate,
    async loadRuns() {
      if (this.runsLoading) return;
      this.pendingRunDelete = null;
      this.runsLoading = true;
      this.runsError = null;

      try {
        const result = await ipcRenderer.invoke("game-runs-list", this.gameId);
        if (!result || result.status === "not-installed") {
          this.runs = [];
          this.runsError = this.$t("#L_GS_NOT_INSTALLED");
        } else if (result.status === "no-directory") {
          this.runs = [];
        } else if (result.status !== "ok") {
          this.runs = [];
          this.runsError = this.$t("#L_GS_RUNS_ERROR", result.code || "");
        } else {
          this.runs = result.items || [];
          this.runsTruncated = !!result.truncated;
        }
      } finally {
        this.runsLoading = false;
      }
    },
    async loadConfigInfo() {
      this.configInfo = await ipcRenderer.invoke("game-config-info", this.gameId, this.configBuild);

      if (this.configInfo && this.configInfo.build) this.configBuild = this.configInfo.build;
      await this.loadConfig();
    },
    async loadConfig(accept) {
      let state;
      try {
        state = await ipcRenderer.invoke("game-config-read", this.gameId, this.configBuild);
      } catch (error) {
        state = { status: "error" };
      }
      if (accept && !accept()) return false;
      this.configState = state;
      this.loadRuleCheck();
      return true;
    },
    // Переключатель «Scriptless / Scripted» в разделе «Проверка»: запомнить и проверить заново.
    setKeyboardFull(full) {
      this.keyboardFull = full;
      try { prefsStore.setKeyboardFull(full); } catch (error) { /* хранилище недоступно — выбор живёт до закрытия окна */ }
    },
    setRuleCategory(category) {
      if (category === this.ruleCategory) return;
      this.ruleCategory = category === "scripted" ? "scripted" : "scriptless";
      try { prefsStore.setRuleCategory("hl1-won", this.ruleCategory); } catch (error) { /* хранилище недоступно — выбор живёт до закрытия окна */ }
      this.loadRuleCheck();
    },
    // Только чтение файлов: вызывается при открытии, после сохранения, импорта, возврата и по фокусу.
    // Ответ устаревшего запроса отбрасывается.
    async loadRuleCheck() {
      // Id игры в окне — id доставки ("70"), его разбирает main: чужой игре ответит unsupported.
      this.ruleCheckToken = (this.ruleCheckToken || 0) + 1;
      const token = this.ruleCheckToken;
      this.ruleCheckLoading = true;
      let result = null;
      try {
        result = await ipcRenderer.invoke("player-config-check", this.gameId, this.ruleCategory);
      } catch (error) {
        result = null;
      }
      if (token !== this.ruleCheckToken) return;
      this.ruleCheckLoading = false;
      // Сбой не стирает прежний итог; "nothing" и "unsupported" — проверять нечего, раздела нет.
      const failed = !result || result.status === "error";
      this.ruleCheckFailed = failed;
      if (!failed) this.ruleCheck = result.status === "ok" ? result : null;
    },
    // Уход из «Файлов cfg» с несохранённым текстом: вопрос страницы «Персонализация» или confirm редактора.
    async leaveCfgFiles() {
      if (!this.cfgFilesDirty) return true;
      if (this.askUnsaved) return this.askUnsaved();
      const files = this.$refs.cfgFiles;
      return !files || files.confirmLeave();
    },
    // Место из проверки: «Файлы cfg», нужный файл, курсор на строке.
    async showCheckPlace(place) {
      const editor = this.$refs.configEditor;
      if (!place || !place.path || !editor || !(await editor.selectSection("cfg-files"))) return;
      await this.$nextTick();
      if (this.$refs.cfgFiles) await this.$refs.cfgFiles.openAt(place.path, place.line);
    },
    // Для страницы «Персонализация» (строка в итоге импорта): открыть раздел «Проверка».
    async showCheck() {
      if (!(await this.selectSection("configs"))) return;
      if (!this.ruleCounts) await this.loadRuleCheck();
      await this.$nextTick();
      if (this.$refs.configEditor) await this.$refs.configEditor.selectSection("cfg-check");
    },
    // У другой сборки свои cfg: несохранённые правки прежней сборки
    // к ней не относятся.
    onConfigBuild(build) {
      this.configBuild = build;
      this.configChanges = {};
      this.configImport = null;
      this.configError = null;
      this.loadConfigInfo();
    },
    // Импорт cfg на страницу «Персонализация»: значения переменных из файла и его
    // цепочки exec становятся правками в окне. До «Сохранить» ничего не
    // записано; значение, равное текущему, правкой не считается.
    async importConfig() {
      const state = this.configState;
      if (this.importingConfig || !state || state.status !== "ok" || state.running) return;
      this.importingConfig = true;
      try {
        if (!(await this.confirmImportIntro(state.engine))) return;
        const ipc = window.hlsrElectron.ipcRenderer;
        const result = await ipc.invoke("player-config-import-pick", this.gameId, { multiple: true });
        if (!result || result.status === "canceled") return;
        if (result.status !== "ok") {
          const key = result.code === "BINDS_FILE_TOO_LARGE" ? "#L_KB_IMPORT_TOO_LARGE" : "#L_KB_IMPORT_FAILED";
          this.configImport = { error: true, text: this.$t(key) };
          return;
        }
        const chosen = await this.withCompanions(this.gameId, state.engine, result.files || [result], result.dir);
        const { root, files: picked } = cfgImport.rootOfPicked(chosen);
        result.name = root.name;
        const { files, missing } = await this.pickImportChain(this.gameId, root, result.dir, state.engine, true, picked);
        const built = cfgImport.buildImport({ engine: state.engine, root, files, missing });
        const catalog = gameConfig.catalogFor(state.gameId, state.engine);
        const { values, skipped } = gameConfig.importValues(catalog, built.flatText);
        const found = Object.keys(values);
        if (!found.length) {
          this.configImport = { error: true, text: this.$t("#L_GS_CFG_IMPORT_EMPTY", result.name) };
          return;
        }
        const edits = Object.assign({}, this.configChanges);
        let changed = 0;
        for (const cvar of found) {
          const field = catalog.byCvar.get(cvar.toLowerCase());
          const stored = gameConfig.fromCfg(field, state.values[field.cvar]);
          const shown = Object.prototype.hasOwnProperty.call(edits, field.cvar)
            ? edits[field.cvar]
            : stored === undefined ? gameConfig.defaultValue(field) : stored;
          if (values[cvar] !== shown) changed += 1;
          if (values[cvar] === stored || (stored === undefined && values[cvar] === gameConfig.defaultValue(field))) {
            delete edits[field.cvar];
          } else {
            edits[field.cvar] = values[cvar];
          }
        }
        this.configChanges = edits;
        const report = this.importReport(built.report);
        const parts = [changed
          ? this.$t("#L_GS_CFG_IMPORT_DONE", result.name, String(changed))
          : this.$t("#L_GS_CFG_IMPORT_SAME", result.name)];
        if (skipped.length) parts.push(this.$t("#L_GS_CFG_IMPORT_SKIPPED", skipped.join(", ")));
        this.configImport = { error: false, text: parts.concat(report.short).join(" "), details: report.details };
      } catch (error) {
        this.configImport = { error: true, text: this.$t("#L_KB_IMPORT_FAILED") };
      } finally {
        this.importingConfig = false;
      }
    },
    configErrorText(result) {
      const code = result && result.code;
      if (code === "NOT_INSTALLED") return this.$t("#L_GS_NOT_INSTALLED");
      return this.$t("#L_GS_CFG_SAVE_FAILED", code || "");
    },
    // Конфиги пишутся до остальных настроек: если игра запущена, не
    // сохраняется ничего, и окно остаётся открытым на странице «Персонализация».
    async saveConfig() {
      if (!Object.keys(this.configChanges).length) return true;
      this.configError = null;
      let result;
      try {
        const args = [this.gameId, this.configBuild, JSON.parse(JSON.stringify(this.configChanges))];
        const sha = this.configState ? this.configState.sha256 : undefined;
        if (typeof sha === "string") args.push({ expectedSha256: sha });
        result = await ipcRenderer.invoke("game-config-write", ...args);
      } catch (error) {
        result = null;
      }
      if (result && result.status === "ok") {
        // Файл игрока один: отпечаток после записи нужен и остальным настройкам окна.
        // Следующая отправка (если дальнейшее сохранение упадёт) должна нести новый отпечаток.
        if (result.sha256 && this.configState) this.configState.sha256 = result.sha256;
        if (result.sha256 && this.configSha256 !== undefined) this.configSha256 = result.sha256;
        return true;
      }
      this.section = "configs";
      if (result && result.code === "CONFIG_CHANGED") {
        this.fileChanged = true;
        return false;
      }
      this.configError = this.configErrorText(result);
      if (result && result.code === "GAME_RUNNING") await this.loadConfig();
      return false;
    },
    async loadDelivery() {
      try {
        this.delivery = await ipcRenderer.invoke("game-delivery-get-status", this.gameId);
      } catch (error) {
        this.delivery = null;
      }
    },
    openConfigFolder() {
      ipcRenderer.invoke("game-config-open-folder", this.gameId, this.configBuild);
    },
    openRunsFolder() {
      ipcRenderer.invoke("game-runs-open-folder", this.gameId);
    },
    revealRun(run) {
      ipcRenderer.invoke("game-runs-reveal", this.gameId, run.name);
    },
    confirmRunDelete(run) {
      if (this.deletingRun) return;
      this.runDeleteError = null;
      this.pendingRunDelete = run.id;
    },
    async deleteRun(run) {
      if (this.deletingRun || this.pendingRunDelete !== run.id) return;
      this.deletingRun = run.id;
      this.runDeleteError = null;
      try {
        const result = await ipcRenderer.invoke("game-runs-delete", this.gameId, run.name);
        if (!result || result.status !== "ok") {
          this.runDeleteError = result && result.code === "GAME_RUNNING"
            ? this.$t("#L_D_ERR_GAME_RUNNING") : this.$t("#L_GS_GHOST_DELETE_FAILED");
          return;
        }
        this.runs = this.runs.filter((item) => item.id !== run.id);
        this.pendingRunDelete = null;
        if (this.runsTruncated) await this.loadRuns();
      } catch (error) {
        this.runDeleteError = this.$t("#L_GS_GHOST_DELETE_FAILED");
      } finally { this.deletingRun = null; }
    },
    async load(accept) {
      let result = await ipcRenderer.invoke("game-settings-get", this.gameId);
      if (accept && !accept()) return false;

      if (result && result.status === "ok" && result.settings)
        this.draft = withPersonalization(result.settings);
      else this.draft = withPersonalization({});
      this.draftSaved = JSON.stringify(this.draft);

      // Отпечаток файла игрока, по которому построено окно: уходит обратно при сохранении.
      this.configSha256 = result ? result.configSha256 : undefined;
      this.languages = result && Array.isArray(result.languages) ? result.languages : [];
      this.vulkanAvailable = result && typeof result.vulkan === "boolean" ? result.vulkan : null;
      this.keyBinds = result && result.keyBinds ? result.keyBinds : null;
      this.bindsGameId = result && result.bindsGameId ? result.bindsGameId : null;
      return true;
    },
    errorText(error) {
      let code = error ? error.code : null;

      switch (code) {
        case "TOO_LONG":
          return this.$t("#L_GS_ERR_TOO_LONG");
        case "CONTROL_CHARS":
          return this.$t("#L_GS_ERR_CONTROL_CHARS");
        case "UNBALANCED_QUOTES":
          return this.$t("#L_GS_ERR_QUOTES");
        case "FORBIDDEN_PARAM":
          return this.$t("#L_GS_ERR_FORBIDDEN", error.param || "");
        case "PROTECTED_ENV":
          return this.$t("#L_GS_ERR_PROTECTED_ENV", error.param || "");
        case "WRAPPER_INVALID":
          return this.$t("#L_GS_ERR_WRAPPER", error.param || "");
        default:
          return this.$t("#L_GS_ERR_INVALID");
      }
    },
    async removeImportedCfg() {
      if (this.importingBinds || !this.bindsGameId) return;
      if (this.pendingCfgImport) {
        const { undo } = this.pendingCfgImport;
        if (undo) Object.assign(this.draft, { binds: undo.binds, personalization: undo.personalization, bxt: undo.bxt });
        this.pendingCfgImport = null;
        this.bindsImport = null;
        if (!this.importedCfgOnDisk) return;
      }
      this.importingBinds = true;
      try {
        const result = await ipcRenderer.invoke("player-config-import-undo", this.bindsGameId);
        if (!result || result.status !== "ok") {
          this.bindsImport = { error: true, text: this.importErrorText(result) };
          return;
        }
        this.keyBinds = result.keyBinds || null;
        this.importUndoable = false;
        if (result.sha256 !== undefined && this.configSha256 !== undefined) this.configSha256 = result.sha256;
        await this.reloadAfterChange();
        this.bindsImport = { error: false, text: this.$t("#L_KB_IMPORT_REMOVED") };
      } catch (error) {
        this.bindsImport = { error: true, text: this.$t("#L_KB_IMPORT_FAILED") };
      } finally {
        this.importingBinds = false;
      }
    },
    importErrorText(result) {
      const code = result && result.code;
      if (code === "GAME_RUNNING") return this.$t("#L_D_ERR_GAME_RUNNING");
      if (code === "BINDS_FILE_TOO_LARGE") return this.$t("#L_KB_IMPORT_TOO_LARGE");
      return this.$t("#L_KB_IMPORT_WRITE_FAILED");
    },
    // Вернулись в лаунчер: файл игрока могли править руками. Перечитываем, только если
    // на странице нет несохранённых правок; с правками сохранение поймает CONFIG_CHANGED.
    // Ответ применяется, только если за время чтения правок и сохранения не было и запрос последний.
    async onWindowFocus() {
      const idle = () => !this.saving && !this.fileChanged && !this.hasUnsaved();
      if (!idle()) return;
      this.focusToken = (this.focusToken || 0) + 1;
      const token = this.focusToken;
      const accept = () => token === this.focusToken && idle();
      if (!(await this.load(accept))) return;
      await this.loadConfig(accept);
    },
    // Несохранённые правки: черновик отличается от загруженного, есть правки «Конфигов», импорт cfg
    // или текст в редакторе cfg.
    hasUnsaved() {
      return this.draftSaved === null || JSON.stringify(this.draft) !== this.draftSaved
        || Object.keys(this.configChanges).length > 0 || !!this.pendingCfgImport || this.cfgFilesDirty;
    },
    // Сохранён текст cfg в редакторе: файл игрока мог измениться. Без других правок окно
    // перечитывает его (новый отпечаток и значения); с правками старый отпечаток остаётся,
    // и «Сохранить» спросит «Перечитать», а не запишет поверх текста устаревший черновик.
    onCfgFileSaved() {
      this.cfgFilesDirty = false;
      this.loadRuleCheck();
      this.$emit("file-saved");
      return this.onWindowFocus();
    },
    // Окно не закрывается: черновик берётся из сохранённых настроек.
    // Файл игрока изменён вне лаунчера: настройки не сохранены, окно перечитывает их.
    async reloadAfterChange() {
      this.fileChanged = false;
      await this.load();
      if (Object.keys(this.configChanges).length) this.configChanges = {};
      await this.loadConfig();
    },
    // «Отмена»: в окне закрывает его, на странице «Персонализация» отбрасывает правки.
    onCancel() {
      if (this.embedded) this.discardChanges();
      else this.$emit("close");
    },
    // Несохранённое отброшено: настройки и файл игрока читаются заново.
    async discardChanges() {
      this.keyboardOpen = false;
      this.pendingCfgImport = null;
      this.bindsImport = null;
      this.configImport = null;
      this.argsError = null;
      this.protonError = null;
      this.configError = null;
      this.editedPending = null;
      if (this.$refs.cfgFiles) this.$refs.cfgFiles.revert();
      this.cfgFilesDirty = false;
      await this.reloadAfterChange();
    },
    async deleteEdited() {
      this.editedPending = null;
      await this.submit({ removeEdited: true });
    },
    async submit(options) {
      if (this.saving) return;
      this.saving = true;
      this.focusToken = (this.focusToken || 0) + 1; // устаревший ответ перечитывания по фокусу отбрасывается
      this.editedPending = null;
      this.fileChanged = false;

      try {
        if (!(await this.saveConfig())) return;

        if (this.pendingCfgImport) {
          // payload лежит в data и стал реактивным прокси: IPC его не
          // клонирует («An object could not be cloned»).
          const imported = await ipcRenderer.invoke("player-config-import-apply", this.bindsGameId,
            JSON.parse(JSON.stringify(this.pendingCfgImport.payload)));
          if (!imported || imported.status !== "ok") {
            this.argsError = this.importErrorText(imported);
            this.bindsImport = { error: true, text: this.argsError };
            return;
          }
          this.pendingCfgImport = null;
          this.importUndoable = true;
          // Файл игрока изменился: дальше «Сохранить» ждёт уже новый отпечаток.
          if (imported.sha256 !== undefined && this.configSha256 !== undefined) this.configSha256 = imported.sha256;
          this.keyBinds = imported.keyBinds || this.keyBinds;
          // Переназначения в хранилище сняты (файл главный): черновик не должен их вернуть при сохранении.
          this.draft.binds = {};
        }

        const payload = {};
        for (const key of Object.keys(EMPTY)) payload[key] = this.draft[key];
        if (!payload.personalization) payload.personalization = defaultPersonalization();

        // Бинды, паки и персонализация в черновике — реактивные прокси Vue,
        // а IPC их не клонирует («An object could not be cloned»): в main
        // уходит обычная копия.
        const saveArgs = [this.gameId, JSON.parse(JSON.stringify(payload))];
        const saveOptions = {};
        if (options && options.removeEdited) saveOptions.removeEdited = true;
        if (this.configSha256 !== undefined) saveOptions.configSha256 = this.configSha256;
        if (Object.keys(saveOptions).length) saveArgs.push(undefined, saveOptions);
        let result = await ipcRenderer.invoke("game-settings-save", ...saveArgs);

        if (!result || result.status !== "ok") {
          const error = result && result.error;
          if (error && error.code === "CONFIG_CHANGED") this.fileChanged = true;
          else if (error && error.field === "protonOptions") this.protonError = this.errorText(error);
          else this.argsError = this.errorText(error);
          return;
        }

        // Окно остаётся открытым; тост «Настройки игры сохранены» даёт LauncherShell по saved.
        if ("configSha256" in result) {
          this.configSha256 = result.configSha256;
          if (this.configState && this.configState.status === "ok") this.configState.sha256 = result.configSha256;
        }
        this.$emit("saved", result.settings);
        if (result.settings) {
          this.draft = withPersonalization(result.settings);
          this.draftSaved = JSON.stringify(this.draft);
        }
        // Скрипты, которые больше не нужны, но правлены руками, остались в файле игрока.
        if (Array.isArray(result.keptEdited) && result.keptEdited.length) this.editedPending = result.keptEdited;
        // Бинды уже в файле игрока: закрывать игру не нужно, игрок
        // выполняет его в консоли сам.
        this.bindsImport = this.configState && this.configState.running
          ? { error: false, text: this.$t("#L_KB_SAVED_RUNNING", gameBinds.engineOf(bindPresets.engineFor(this.bindsGameId)).execFile) }
          : null;
        this.argsError = null;
        this.protonError = null;
        this.loadRuleCheck();
        if (Object.keys(this.configChanges).length) {
          this.configChanges = {};
          this.configImport = null;
          await this.loadConfig();
        }
      } catch (error) {
        // Без этого любой сбой IPC выглядел как «кнопка не нажимается».
        console.error("[game-settings] сохранение не удалось", error);
        this.argsError = this.errorText(null);
      } finally {
        this.saving = false;
      }
    },
    setResolution(id) {
      const match = /^(\d+)x(\d+)$/.exec(id);
      this.draft.width = match ? Number(match[1]) : 0;
      this.draft.height = match ? Number(match[2]) : 0;
    },
    // Сброс касается только того, что есть в этом окне: персонализацию,
    // паки и BXT сбрасывают во вкладке «Персонализация».
    resetDraft() {
      const binds = gameBinds.resetOverrides(this.draft.binds);
      const { personalization, packs, bxt } = this.draft;
      this.draft = withPersonalization({ personalization, packs, bxt });
      this.draft.binds = binds;
      this.pendingCfgImport = null;
      this.bindsImport = null;
      this.argsError = null;
      this.protonError = null;
      // Конфиги не сбрасываются к значениям по умолчанию: «Сбросить»
      // отменяет только несохранённые правки в них.
      this.configChanges = {};
      this.configImport = null;
      this.configError = null;
    },
    onKeydown(e) {
      // Esc и Tab у подсказки перед импортом — её (ConfirmModal), окно настроек не закрывается.
      if (this.importIntro) return;
      // На странице окно не перехватывает клавиши; свои только у окна клавиатуры.
      if (this.embedded && !this.keyboardOpen) return;
      if (e.key === "Escape") {
        if (this.pendingRunDelete) {
          e.preventDefault();
          if (!this.deletingRun) this.pendingRunDelete = null;
          return;
        }
        if (uiState.openDropdowns > 0) return;
        e.preventDefault();
        // Из окна клавиатуры Esc ведёт назад, а не закрывает настройки.
        if (this.keyboardOpen) this.closeKeyboard();
        else this.$emit("close");
      } else if (e.key === "Tab") {
        trapFocus(this.keyboardOpen ? this.$refs.keyboard : this.$refs.window, e);
      }
    },
  },
  mounted() {
    this.lastFocused = document.activeElement;
    document.addEventListener("keydown", this.onKeydown);
    // Страница «Персонализация» ждёт готовности окна (опрос первого запуска начинает импорт).
    this.load().then(
      () => {
        this.loadState = "ready";
        this.$emit("ready");
      },
      () => {
        this.loadState = "failed";
        this.$emit("load-failed");
      },
    );
    this.loadConfigInfo();
    if (this.hasGhosts) this.loadRuns();
    this.loadDelivery();
    window.addEventListener("resize", this.measureInk);
    window.addEventListener("focus", this.onWindowFocus);
    this.$nextTick(() => {
      this.measureInk("still");
      if (this.embedded) return;
      let close = this.$refs.window.querySelector("button");
      if (close) close.focus();
    });
  },
  beforeUnmount() {
    // Окно пересоздаётся с открытой подсказкой импорта — ждущий получает «нет».
    if (this.importIntro) this.onImportIntro(false);
    document.removeEventListener("keydown", this.onKeydown);
    window.removeEventListener("resize", this.measureInk);
    window.removeEventListener("focus", this.onWindowFocus);
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

  &__window {
    position: relative;
    display: flex;
    flex-direction: column;
    background: var(--l-modal-bg);
    border: none;
    border-radius: var(--l-radius);
    box-shadow: inset 0 0 0 1px var(--l-modal-border), 0 24px 72px rgba(0, 0, 0, 0.6);
    overflow: hidden;
  }

  /* Подвал без отдельной плашки, как в профиле: та же поверхность,
     отделён линией. */
  &__footer {
    display: flex;
    align-items: center;
    flex: none;
    gap: 12px;
    padding: 16px 32px 20px;
    box-shadow: inset 0 1px 0 var(--l-modal-border);
  }

  &__footer-spacer {
    flex: 1;
  }
}

/* На странице «Персонализация» окно занимает отведённое место, без затемнения. */
.settings-modal.game-settings--embedded {
  position: relative;
  inset: auto;
  flex: 1;
  min-height: 0;
  z-index: auto;
  background: none;

  /* Без внешней карточки: рамка, фон, скругление и поля убраны, содержимое
     идёт на всю ширину страницы. Своё оформление остаётся у внутренних панелей.
     Слева список разделов, справа раздел и под ним нижняя строка. */
  .game-settings__window {
    display: grid;
    grid-template: minmax(0, 1fr) auto / clamp(170px, 15vw, 216px) minmax(0, 1fr);
    column-gap: 20px;
    width: 100%;
    height: 100%;
    background: none;
    border-radius: 0;
    box-shadow: none;
    overflow: visible;
  }

  /* Заголовка нет: шапка — это сам список, сверху вровень с содержимым. */
  .game-settings__head {
    grid-row: 1 / -1;
    min-height: 0;
    padding: 12px 0 16px;
    box-shadow: none;
    overflow-y: auto;
  }
  .game-settings__tabs {
    flex-direction: column;
    gap: 2px;
    margin-top: 0;
    overflow: visible;
  }
  .game-settings__tab {
    gap: 10px;
    height: auto;
    min-height: 40px;
    padding: 8px 12px;
    border-radius: var(--l-radius-sm);
    white-space: normal;
    text-align: left;
    transition: background var(--l-fast), color var(--l-fast);

    i { flex: none; width: 16px; font-size: 13px; text-align: center; }
    &:hover { background: var(--l-chip-hover); }
    &:hover i { transform: none; }
    &:focus-visible { outline-offset: -2px; }
    &.is-active { background: var(--accent-soft); }
  }
  .game-settings__layout { grid-column: 2; grid-row: 1; }
  /* Боковой отступ 4 px: кольцо фокуса и тень крайних элементов не обрезаются прокруткой тела. */
  .game-settings__body { padding: 12px 4px 16px; scroll-padding: 4px; }
  /* Свою прокрутку и нижнюю строку разделы ведут сами. */
  .game-settings__body--personal { padding-bottom: 0; overflow: hidden; }
  .game-settings__body--keyboard { overflow: hidden; }
  .game-settings__footer {
    grid-column: 2;
    grid-row: 2;
    flex-wrap: wrap;
    gap: 12px;
    padding: 16px 0 0;
  }
  /* Правая группа забирает всё место справа, заметка в ней сжимается и переносит текст:
     строка остаётся одной (R7), на вторую уходит только в узком окне. */
  .game-settings__footer > .settings-modal__footer-spacer { display: none; }
  .game-settings__footer-end { flex: 1 1 0; margin-left: auto; }
  .game-settings__footer-end > .game-settings__note { flex: 1 1 0; min-width: 120px; }
  .game-settings__icon-btn { width: 48px; padding: 0; }
}

/* Подразделы «Персонализации» (только на странице): мельче, с отступом; название целиком —
   в узкой колонке (Full HD) длинное переносится на вторую строку, а не обрезается многоточием.
   Выбранный — подложкой и значком цвета акцента. Вне блока embedded: тема Liquid Glass
   осветляет неактивные своим правилом в launcher.css. */
.game-settings__subtabs {
  display: flex;
  flex-direction: column;
  gap: 1px;
  margin: 2px 0 6px;
}
.game-settings__subtab {
  display: flex;
  align-items: center;
  gap: 7px;
  min-height: 30px;
  padding: 5px 6px 5px 22px;
  border: 0;
  border-radius: var(--l-radius-sm);
  background: none;
  color: var(--l-text-dim);
  font: inherit;
  font-size: 12.5px;
  line-height: 16px;
  text-align: left;
  cursor: pointer;
  transition: background var(--l-fast), color var(--l-fast);

  i { flex: none; width: 14px; color: var(--l-text-faint); font-size: 11px; text-align: center; }
  &-label { min-width: 0; text-wrap: balance; }
  &:hover { background: var(--l-chip-hover); color: var(--l-text); }
  &:focus-visible { outline: 2px solid var(--l-accent); outline-offset: -2px; }
  &.is-active { background: var(--l-chip-hover); color: var(--l-text); font-weight: 600; }
  &.is-active i { color: var(--l-accent); }
}
.game-settings__subtab-dot { flex: none; width: 6px; height: 6px; margin-left: auto; border-radius: 50%; background: var(--l-accent); }

/* Только для чтения с экрана. */
.game-settings__sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0, 0, 0, 0); white-space: nowrap; }

.game-settings {
  &__validation { color: #e99c9c; font-size: 12px; max-width: 240px; }
  &__note { max-width: 340px; color: var(--l-text-faint); font-size: 12px; line-height: 1.4; text-align: right; text-wrap: pretty; }
  &__note.is-error { color: var(--l-level-error); }
  /* Размер окна одинаков во всех разделах: вкладки не прыгают. Шапка с
     вкладками выше прежней боковой навигации — окно выше на столько же,
     и редактору клавиатуры остаётся та же высота. */
  /* z-index собирает слои окна (крестик и прочее) в один слой: окно
     клавиатуры целиком лежит над ним. */
  &__window {
    z-index: 1;
    width: min(1240px, calc(100% - 24px));
    height: min(760px, calc(100% - 24px));
  }

  /* Окно клавиатуры — во весь лаунчер, с тем же отступом, что у окна
     настроек в низком окне, и целиком закрывает его. Анимируются только
     прозрачность и transform: раскладка внутри не пересчитывается. */
  &__kb-window {
    position: absolute;
    inset: 12px;
    z-index: 2;
    opacity: 0;
    visibility: hidden;
    transform: translateY(8px) scale(0.985);
    transition: opacity var(--l-dur) var(--l-ease), transform var(--l-dur) var(--l-ease),
      visibility 0s linear var(--l-dur);

    &.is-open {
      opacity: 1;
      visibility: visible;
      transform: none;
      transition: opacity var(--l-dur) var(--l-ease), transform var(--l-dur) var(--l-ease), visibility 0s;
    }
  }

  /* Крестик — как в профиле: квадрат с рамкой, правый край на линии
     содержимого (32 px). */
  &__close {
    position: absolute;
    top: 24px;
    right: 32px;
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
    cursor: pointer;
    transition: background var(--l-fast), color var(--l-fast);
    z-index: 4;

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

  &__head {
    position: relative;
    flex: none;
    padding: 24px 32px 0;
    box-shadow: inset 0 -1px 0 var(--l-modal-border);
  }

  &__titles {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
    padding-right: 56px;
    animation: l-rise-in var(--l-dur-slow) var(--l-ease) both;
  }

  &__title {
    margin: 0;
    color: var(--l-text);
    font-size: 24px;
    font-weight: 700;
    line-height: 32px;
    letter-spacing: 0.01em;
  }

  &__game {
    color: var(--l-text-faint);
    font-size: 11px;
    font-weight: 700;
    line-height: 16px;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* Шапка окна клавиатуры: «Назад», черта и название одной строкой. */
  &__head--keyboard {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 12px 24px;

    .game-settings__titles {
      gap: 0;
      padding-right: 0;
      animation: none;
    }

    .game-settings__title {
      font-size: 18px;
      line-height: 24px;
    }
  }

  &__back {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 8px;
    height: 36px;
    padding: 0 14px 0 10px;
    border: none;
    border-radius: var(--l-radius-sm);
    background: rgba(0, 0, 0, 0.32);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: var(--l-text-dim);
    font-family: inherit;
    font-size: 13.5px;
    font-weight: 600;
    cursor: pointer;
    transition: background var(--l-fast), color var(--l-fast), box-shadow var(--l-fast);

    > i {
      width: 16px;
      font-size: 12px;
      text-align: center;
      transition: transform var(--l-dur) var(--l-ease);
    }

    &:hover {
      background: rgba(255, 255, 255, 0.08);
      box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--l-accent) 45%, var(--l-border));
      color: var(--l-text);

      > i { transform: translateX(-2px); }
    }

    &:focus-visible {
      outline: 2px solid var(--l-accent);
      outline-offset: 2px;
    }
  }

  &__head-sep {
    flex: none;
    width: 1px;
    height: 28px;
    background: var(--l-modal-border);
  }

  /* ---- Вкладки (как в профиле) ---- */

  &__tabs {
    position: relative;
    display: flex;
    gap: 8px;
    margin-top: 16px;
    overflow-x: auto;
    scrollbar-width: none;
  }

  &__tab {
    flex: none;
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
      transition: color var(--l-fast), transform var(--l-dur) var(--l-ease-spring);
    }

    &:hover {
      color: var(--l-text);

      i { transform: scale(1.12); }
    }

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

  /* Вкладка, которая открывает своё окно: значок «развернуть» справа. */
  &__tab-open {
    margin-left: -2px;
    font-size: 9px !important;
    opacity: 0.7;
  }

  &__tab.is-window:hover &__tab-open {
    color: var(--l-accent);
    opacity: 1;
    transform: none;
  }

  /* Перетекает между вкладками (.l-ink в launcher.css). */
  &__tab-ink {
    bottom: 0;
    height: 2px;
    border-radius: 2px 2px 0 0;
    background: var(--l-accent);
    box-shadow: 0 0 12px color-mix(in srgb, var(--l-accent) 60%, transparent);
  }

  /* ---- Содержимое ----
     Разделы — страницы в одной ячейке, каждая со своей прокруткой; при
     смене уезжают на одинаковую ширину (l-slide-*). */
  &__layout {
    position: relative;
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template: minmax(0, 1fr) / minmax(0, 1fr);
    overflow: hidden;
  }

  &__body {
    grid-area: 1 / 1;
    min-height: 0;
    box-sizing: border-box;
    padding: 12px 32px 24px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
  }

  /* Окно клавиатуры. Схема (GameKeyboard) занимает не больше ~28% высоты
     окна: в низком окне и при масштабе системы списку действий под ней
     остаётся место. */
  &__keyboard {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    padding: 12px 20px;
    overflow: hidden;
    --kb-fit: min(1120px, 126vh);
  }

  /* В окне клавиатуры шапка и подвал ниже: место уходит схеме и списку. */
  &__kb-window &__footer {
    padding: 10px 24px 12px;

    .settings-btn {
      min-height: 40px;
      padding: 7px 17px;
    }
  }

  &__body--personalization {
    padding: 16px 20px;
    overflow: hidden;
  }

  /* Страница «Персонализация»: строка со сборкой и папкой, под ней редактор
     во всю оставшуюся высоту (прокручивается сам список настроек). */
  &__body--configs {
    gap: 10px;
    padding-bottom: 20px;
    overflow: hidden;

    // Full HD: каждый пиксель высоты — строкам параметров.
    @media (max-height: 700px) { gap: 8px; padding-bottom: 12px; }
  }

  &__cfg-bar {
    display: flex;
    flex: none;
    align-items: stretch;
    gap: 8px;
    min-width: 0;
  }

  /* Путь к файлу: нормальная высота и поля; длинный путь переносится по
     символам (не больше двух строк), полный путь виден во всплывающей подсказке. */
  &__cfg-path {
    flex: 1;
    min-width: 0;
    box-sizing: border-box;
    min-height: 36px;
    padding: 8px 12px;
    overflow: hidden;
    border-radius: var(--l-radius-sm);
    background: var(--l-field-bg);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: var(--l-text-dim);
    font-family: Consolas, monospace;
    font-size: 12px;
    line-height: 20px;
    align-content: center;
    overflow-wrap: anywhere;
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
  }

  /* Правая группа подвала (заметки и кнопки); при нехватке места уходит на вторую строку. */
  &__footer-end {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: flex-end;
    gap: 8px 12px;
    min-width: 0;
  }

  &__cfg-alert {
    display: flex;
    flex: none;
    align-items: center;
    gap: 9px;
    margin: 0;
    padding: 9px 14px;
    border-radius: var(--l-radius-sm);
    background: rgba(255, 122, 28, 0.1);
    box-shadow: inset 0 0 0 1px rgba(255, 122, 28, 0.35);
    color: #ffb37a;
    font-size: 12.5px;
    line-height: 1.4;
  }

  &__cfg-editor {
    flex: 1;
    min-height: 0;
  }

  &__ghosts-head {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 0 16px;
  }

  &__empty {
    padding: 32px 16px;
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.02);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: var(--l-text-faint);
    font-size: 13px;
    text-align: center;
  }

  &__runs {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  /* Запись — карточкой, как забег в профиле: наведение подсвечивает
     рамку и фон, карточка не прыгает. */
  &__run {
    display: flex;
    align-items: center;
    gap: 12px;
    min-height: 56px;
    padding: 8px 8px 8px 12px;
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.03);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: var(--l-text);
    font-size: 13px;
    transition: background-color var(--l-fast), box-shadow var(--l-fast);

    &:hover {
      background: rgba(255, 255, 255, 0.05);
      box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--l-accent) 45%, var(--l-border));
    }

    i {
      color: var(--l-text-dim);
    }
  }

  &__run-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Значок файла — в плашке, как значки сводки профиля. */
  &__run-file {
    display: flex; align-items: center; gap: 12px; flex: 1; min-width: 0;
    padding: 0; border: 0; color: inherit; background: transparent; font: inherit; text-align: left;
    cursor: pointer;

    > i {
      flex: none;
      display: grid;
      place-items: center;
      width: 32px;
      height: 32px;
      border-radius: var(--l-radius-sm);
      background: var(--l-accent-soft);
      color: var(--l-accent);
      font-size: 13px;
      line-height: 1;
    }
  }
  &__run-actions { display: flex; align-items: center; gap: 8px; flex: none; }
  &__run-confirm { color: var(--l-text-dim); font-size: 12px; line-height: 20px; }
  &__run-delete {
    display: grid; place-items: center; width: 32px; height: 32px; flex: none; padding: 0;
    border: 0; border-radius: var(--l-radius-sm); background: transparent; color: var(--l-text-dim); cursor: pointer;
    transition: background var(--l-fast), color var(--l-fast);
    &:hover:not(:disabled) { background: rgba(255, 255, 255, 0.08); color: var(--l-text); }
    &:hover:not(:disabled) i { color: var(--l-text); }
    &.danger i { color: #ff8796; }
    &:disabled { opacity: 0.5; cursor: default; }
  }
  &__run-actions > &__run-delete:last-child:hover:not(:disabled) { background: rgba(255, 107, 107, 0.12); }
  &__run-actions > &__run-delete:last-child:hover:not(:disabled) i { color: #ff8796; }
  &__run-file:focus-visible, &__run-delete:focus-visible { outline: 2px solid var(--l-accent); outline-offset: 2px; }
  &__run-error { margin-top: 12px; color: #ff8796; font-size: 12px; }

  &__run-meta {
    color: var(--l-text-faint);
    font-size: 12px;
    flex: none;
    font-variant-numeric: tabular-nums;
  }

  /* Сведения о версии — сеткой карточек, как сводка профиля. */
  &__facts {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 8px;
    margin: 4px 0 16px;
  }

  &__fact {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    padding: 14px 16px;
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.03);
    box-shadow: inset 0 0 0 1px var(--l-border);
    animation: l-rise-in var(--l-dur) var(--l-ease) both;
    animation-delay: calc(var(--i, 0) * 30ms);
  }

  &__fact-label {
    order: 2;
    color: var(--l-text-dim);
    font-size: 12px;
    line-height: 18px;
  }

  &__fact-value {
    color: var(--l-text);
    font-size: 15px;
    font-weight: 700;
    line-height: 22px;
    font-variant-numeric: tabular-nums;
    word-break: break-word;
  }

  &__args {
    padding: 12px 16px;
    background: rgba(255, 255, 255, 0.03);
    border: 0;
    box-shadow: inset 0 0 0 1px var(--l-border);
    border-radius: var(--l-radius-sm);
    color: var(--l-text);
    font-family: inherit;
    font-size: 13px;
    line-height: 20px;
    transition: box-shadow var(--l-fast), background var(--l-fast);

    &:hover {
      background: rgba(255, 255, 255, 0.05);
    }

    &:focus {
      outline: none;
      box-shadow: inset 0 0 0 1px var(--accent), 0 0 0 3px var(--accent-soft);
    }

    &.invalid {
      box-shadow: inset 0 0 0 1px #ff4d5e;
    }
  }

  &__error {
    color: #ff6b78;
    font-size: 12px;
    line-height: 1.4;
  }
}

/* Выпадающие списки строк — как в настройках профиля. */
.settings-select {
  display: flex;
  align-items: center;
  gap: 12px;
  justify-content: space-between;
  min-width: 160px;
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
    transition: transform var(--l-med), color var(--l-fast);
  }

  &:hover,
  &.open {
    background: rgba(255, 255, 255, 0.08);
    border-color: var(--l-border-strong);
  }

  &.open i {
    transform: rotate(180deg);
    color: var(--l-accent);
  }
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
    background: rgba(255, 255, 255, 0.12);
    border-radius: 12px;
    display: flex;
    align-items: center;
    padding: 3px;
    transition: background var(--l-med), box-shadow var(--l-med);
  }

  /* Бегунок перепрыгивает с лёгким перелётом и вытягивается, пока
     переключатель нажат. */
  &__thumb {
    width: 18px;
    height: 18px;
    background: #fff;
    border-radius: 9px;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.35);
    transform: translateX(0);
    transition: transform var(--l-dur) var(--l-ease-spring), width var(--l-fast);
  }

  &:hover input:not(:checked) + .settings-switch__track {
    background: rgba(255, 255, 255, 0.18);
  }

  &:active .settings-switch__thumb {
    width: 22px;
  }

  input:checked + .settings-switch__track {
    background: var(--accent);
    box-shadow: 0 0 12px color-mix(in srgb, var(--l-accent) 40%, transparent);
  }

  input:checked + .settings-switch__track .settings-switch__thumb {
    background: var(--l-switch-thumb-on, #fff);
    transform: translateX(18px);
  }

  &:active input:checked + .settings-switch__track .settings-switch__thumb {
    transform: translateX(14px);
  }

  input:focus-visible + .settings-switch__track {
    outline: 2px solid var(--accent-focus);
    outline-offset: 2px;
  }

  &.disabled,
  &.disabled input {
    cursor: default;
  }

  input:disabled + .settings-switch__track {
    opacity: 0.45;
  }
}

.settings-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  min-height: 48px;
  padding: 11px 19px;
  /* Рамка — border, а не inset box-shadow: тень со скруглением на Windows
     (ANGLE) оставляла яркие одиночные пиксели на стыке дуги угла и края. */
  border: 1px solid var(--l-border-strong);
  border-radius: var(--l-radius-sm);
  background: rgba(255, 255, 255, 0.05);
  color: var(--l-text);
  font-family: inherit;
  font-size: 13.5px;
  font-weight: 600;
  line-height: 24px;
  cursor: pointer;
  transition: background var(--l-fast), border-color var(--l-fast), box-shadow var(--l-fast),
    color var(--l-fast), transform var(--l-fast);

  &:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.1);
  }

  &:active:not(:disabled) {
    transform: scale(0.97);
  }

  /* Кнопки импорта открывают системное окно выбора файла прямо из нажатия.
     Если в этот момент кнопка становилась disabled, Chromium на Windows
     не перерисовывал углы после сжатия, и по краям оставались точки:
     пока идёт выбор, кнопка не отключается, а повтор отсекает обработчик. */
  &[aria-busy="true"] {
    cursor: progress;
  }

  &:focus-visible {
    outline: 2px solid var(--l-accent);
    outline-offset: 2px;
  }

  &:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  &.ghost {
    background: transparent;
    color: var(--l-text-dim);

    &:hover:not(:disabled) {
      color: var(--l-text);
      background: rgba(255, 255, 255, 0.06);
    }
  }

  &.primary {
    background: var(--accent);
    border-color: transparent;
    color: var(--accent-contrast-text);

    &:hover:not(:disabled) {
      background: var(--accent-hover);
      box-shadow: 0 6px 20px color-mix(in srgb, var(--l-accent) 35%, transparent);
    }

    &:active:not(:disabled) {
      background: var(--accent-pressed);
    }
  }
}

@media (prefers-reduced-motion: reduce) {
  .game-settings__tab-ink,
  .game-settings__titles,
  .game-settings__fact,
  .settings-switch__thumb {
    transition: none;
    animation: none;
  }
}

.no-anim .game-settings__kb-window {
  transition: none;
}

@media (prefers-reduced-motion: reduce) {
  .game-settings__kb-window {
    transition: none;
  }
}
</style>
