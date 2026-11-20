<template>
  <!-- Страница «Демо»: демки всех игр, плеер с метками и экспорт клипа.
       IPC — каналы demo-* главного процесса (src/main/demo-tool). -->
  <section
    class="launcher-main dmp"
    :class="{ 'is-player-expanded': playerExpanded }"
    @dragenter="onDragEnter"
    @dragover="onDragOver"
    @dragleave="onDragLeave"
    @drop="onDrop"
  >
    <header class="dmp__head">
      <h1 class="dmp__title">{{ $t("#L_DEMO_TITLE") }}</h1>
      <p class="dmp__lead">{{ $t("#L_DEMO_LEAD") }}</p>
      <span class="dmp__spacer"></span>
      <button type="button" class="dmp__icon-btn" :disabled="listBusy" :title="$t('#L_DEMO_REFRESH')" :aria-label="$t('#L_DEMO_REFRESH')" @click="refresh">
        <i class="fas fa-sync-alt" :class="{ 'fa-spin': listBusy }" aria-hidden="true"></i>
      </button>
      <button v-if="tab === 'demos'" type="button" class="dmp__btn" @click="pickFiles">
        <i class="fas fa-plus" aria-hidden="true"></i>{{ $t("#L_DEMO_ADD") }}
      </button>
    </header>

    <nav class="dmp__tabs" role="tablist" :aria-label="$t('#L_DEMO_TITLE')">
      <button v-for="id in ['demos', 'rendered']" :key="id" type="button" role="tab" class="dmp__tab" :class="{ active: tab === id }"
        :aria-selected="tab === id ? 'true' : 'false'" @click="setTab(id)">
        {{ $t(id === "demos" ? "#L_DEMO_TAB_DEMOS" : "#L_DEMO_TAB_RENDERED") }}
        <span v-if="id === 'rendered' && rendered.items.length" class="dmp__group-count">{{ rendered.items.length }}</span>
      </button>
    </nav>

    <div v-if="notice" class="dmp__notice" :class="{ 'is-error': notice.error }" role="status">
      <div class="dmp__notice-text">
        <span v-if="notice.text">{{ notice.text }}</span>
        <template v-if="notice.rejected && notice.rejected.length">
          <span>{{ $t("#L_DEMO_REJECTED") }}</span>
          <ul class="dmp__notice-list">
            <li v-for="(entry, index) in notice.rejected" :key="index"><b>{{ entry.name }}</b>: {{ entry.reason }}</li>
          </ul>
        </template>
      </div>
      <button type="button" class="dmp__notice-close" :aria-label="$t('#L_CLOSE')" @click="notice = null">
        <i class="fas fa-times" aria-hidden="true"></i>
      </button>
    </div>

    <div v-show="tab === 'demos'" class="dmp__body">
      <aside class="dmp__list" :aria-label="$t('#L_DEMO_TITLE')">
        <div v-if="loading && !items.length" class="dmp__empty">
          <i class="fas fa-spinner fa-spin" aria-hidden="true"></i>
          <span>{{ $t("#L_DEMO_LOADING") }}</span>
        </div>
        <div v-else-if="!items.length" class="dmp__empty">
          <i class="fas fa-film" aria-hidden="true"></i>
          <b>{{ $t("#L_DEMO_EMPTY_TITLE") }}</b>
          <span>{{ $t("#L_DEMO_EMPTY_TEXT") }}</span>
        </div>
        <section v-for="group in groups" v-else :key="group.id" class="dmp__group">
          <h2 class="dmp__group-title">
            <span v-if="groupLogo(group.id)" class="dmp__group-logo" :style="{ '--logo': 'url(' + groupLogo(group.id) + ')' }" aria-hidden="true"></span>
            <i v-else-if="group.id === 'manual'" class="fas fa-folder-plus dmp__group-icon" aria-hidden="true"></i>
            <span>{{ groupName(group) }}</span>
            <span class="dmp__group-count">{{ group.items.length }}</span>
          </h2>
          <button
            v-for="item in group.items"
            :key="item.id"
            type="button"
            class="dmp__item"
            :class="{ active: item.id === selectedId, 'is-unsupported': !item.supported }"
            :aria-current="item.id === selectedId ? 'true' : null"
            @click="select(item.id)"
          >
            <i class="dmp__item-icon" :class="kindIcon(item)" aria-hidden="true"></i>
            <span class="dmp__item-main">
              <span class="dmp__item-name">
                <i v-if="job && job.kind === 'export' && job.itemId === item.id" class="fas fa-spinner fa-spin dmp__item-busy"
                  :title="$t('#L_DEMO_EXPORT_RUNNING')" aria-hidden="true"></i>{{ itemName(item) }}
              </span>
              <span class="dmp__item-meta">{{ itemMeta(item) }}</span>
              <span class="dmp__item-meta is-faint is-wrap">{{ itemDate(item) }}</span>
              <span v-if="!item.supported" class="dmp__badge" :title="reasonText(item)">
                <i class="fas fa-ban" aria-hidden="true"></i>{{ $t(item.reason === "DEMO_NOT_FOUND" ? "#L_DEMO_UNAVAILABLE" : "#L_DEMO_UNSUPPORTED") }}
              </span>
            </span>
          </button>
        </section>
      </aside>

      <div class="dmp__main">
        <DemoPlayer
          ref="player"
          class="dmp__player"
          :src="player.src"
          :video="player.video"
          :markers="player.markers"
          :duration="detail ? detail.item.duration || 0 : 0"
          :range="range"
          :marker-text="markerText"
          @mark="onMark"
          @error="mediaError = true"
          @expanded="playerExpanded = $event"
        >
          <template v-if="screen" #overlay>
            <div class="dmp__screen">
              <template v-if="screen === 'empty'">
                <i class="fas fa-film dmp__screen-icon" aria-hidden="true"></i>
                <span class="dmp__screen-text">{{ $t("#L_DEMO_SCREEN_EMPTY") }}</span>
              </template>
              <template v-else-if="screen === 'unsupported'">
                <i class="fas fa-ban dmp__screen-icon" aria-hidden="true"></i>
                <span class="dmp__screen-text">{{ reasonText(detail.item) }}</span>
              </template>
              <template v-else-if="screen === 'not-ready'">
                <i class="dmp__screen-icon" :class="detail.render.code === 'RENDER_NOT_READY' ? 'fas fa-hourglass-half' : 'fas fa-exclamation-triangle'" aria-hidden="true"></i>
                <span class="dmp__screen-title">{{ errorText({ code: detail.render.code, gameName: itemGameName }) }}</span>
                <span class="dmp__screen-text">{{ $t("#L_DEMO_SCREEN_TIMELINE") }}</span>
              </template>
              <DemoFfmpegCard v-else-if="screen === 'bxtrs'" kind="bxtrs" :status="bxtrs" :error="bxtrsError" @install="installBxtrs" @cancel="cancelBxtrs" />
              <DemoFfmpegCard v-else-if="screen === 'ffmpeg'" :status="ffmpeg" :error="ffmpegError" @install="installFfmpeg" @cancel="cancelFfmpeg" />
              <template v-else-if="screen === 'preview-job'">
                <span class="dmp__screen-title">{{ $t("#L_DEMO_PREVIEW_RUNNING") }}</span>
                <span class="dmp__screen-text">{{ jobLine(job) }}</span>
                <div class="dmp__screen-bar"><div class="dmp__screen-fill" :style="{ width: (job.percent || 0) + '%' }"></div></div>
                <button type="button" class="dmp__btn" @click="cancelJob"><i class="fas fa-times" aria-hidden="true"></i>{{ $t("#L_DEMO_JOB_CANCEL") }}</button>
                <span class="dmp__screen-note"><i class="fab fa-steam" aria-hidden="true"></i>{{ $t("#L_DEMO_STEAM_NOTE") }}</span>
              </template>
              <template v-else-if="screen === 'preview-error'">
                <i class="fas fa-exclamation-triangle dmp__screen-icon is-error" aria-hidden="true"></i>
                <span class="dmp__screen-title">{{ errorText(previewError) }}</span>
                <button type="button" class="dmp__btn" @click="startPreview"><i class="fas fa-redo" aria-hidden="true"></i>{{ $t("#L_DEMO_PREVIEW") }}</button>
              </template>
              <template v-else-if="screen === 'media-error'">
                <i class="fas fa-exclamation-triangle dmp__screen-icon is-error" aria-hidden="true"></i>
                <span class="dmp__screen-title">{{ $t("#L_DEMO_PLAYER_ERROR") }}</span>
                <button type="button" class="dmp__btn" @click="closeMedia"><i class="fas fa-arrow-left" aria-hidden="true"></i>{{ $t("#L_DEMO_PLAYER_BACK") }}</button>
              </template>
              <template v-else-if="screen === 'ready'">
                <button type="button" class="dmp__play" :disabled="!canPreview" :title="$t('#L_DEMO_PREVIEW_HINT')" :aria-label="$t('#L_DEMO_PREVIEW')" @click="startPreview">
                  <i class="fas fa-play" aria-hidden="true"></i>
                </button>
                <span v-if="cancelled && cancelled.kind === 'preview' && cancelled.itemId === detail.item.id" class="dmp__screen-cancelled" role="status">
                  <i class="fas fa-ban" aria-hidden="true"></i>{{ $t("#L_DEMO_PREVIEW_CANCELLED") }}
                </span>
                <span class="dmp__screen-text">{{ $t("#L_DEMO_SCREEN_READY") }}</span>
                <!-- Вид в кадре предпросмотра (KeyHUD, HUD, конфиг и HUD игрока); выбор запоминает главный процесс. -->
                <DemoViewSettings v-model="previewView" :caps="detail.render.view || {}" :game="detail.item.game" popover />
                <span class="dmp__screen-note">{{ $t("#L_DEMO_PREVIEW_TIME") }}</span>
                <span class="dmp__screen-note"><i class="fab fa-steam" aria-hidden="true"></i>{{ $t("#L_DEMO_STEAM_NOTE") }}</span>
              </template>
            </div>
          </template>
        </DemoPlayer>

        <!-- Нижняя карточка сворачивается кнопкой посередине (просьба владельца 14.11):
             без неё плеер на всю высоту, без чёрных полос по бокам. -->
        <button type="button" class="dmp__fold" :class="{ 'is-folded': cardHidden }" :aria-expanded="cardHidden ? 'false' : 'true'"
          :aria-label="$t(cardHidden ? '#L_DEMO_CARD_SHOW' : '#L_DEMO_CARD_HIDE')" :title="$t(cardHidden ? '#L_DEMO_CARD_SHOW' : '#L_DEMO_CARD_HIDE')"
          @click="toggleCard">
          <i class="fas" :class="cardHidden ? 'fa-chevron-up' : 'fa-chevron-down'" aria-hidden="true"></i>
        </button>

        <div v-if="!detail" v-show="!cardHidden" class="dmp__card dmp__card--empty">
          <span>{{ detailLoading ? $t("#L_DEMO_LOADING") : $t(items.length ? "#L_DEMO_PICK_HINT" : "#L_DEMO_PICK_EMPTY") }}</span>
        </div>
        <div v-else v-show="!cardHidden" class="dmp__card">
          <div class="dmp__info">
            <div class="dmp__info-head">
              <h2 class="dmp__info-title" :title="itemName(detail.item)">{{ itemName(detail.item) }}</h2>
              <span v-if="detail.item.incomplete" class="dmp__tag" :title="$t('#L_DEMO_INCOMPLETE')">{{ $t("#L_DEMO_INCOMPLETE") }}</span>
              <!-- «Экспорт» в шапке карточки: внизу закреплённый, он закрывал «Дату» и «Размер» на низком окне.
                   Пока идёт экспорт, на нём ход задания: отдельная строка меняла бы высоту карточки. -->
              <button type="button" class="dmp__btn primary dmp__head-btn" :disabled="!detail.item.supported"
                :title="ownExport ? $t('#L_DEMO_EXPORT_RUNNING') + ' — ' + $t('#L_DEMO_EXPORT_OPEN') : null" @click="openExport">
                <i :class="ownExport ? 'fas fa-spinner fa-spin' : 'fas fa-file-video'" aria-hidden="true"></i>{{ $t("#L_DEMO_EXPORT") }}<template v-if="ownExport"> {{ Math.round(job.percent || 0) }}%</template>
              </button>
            </div>
            <dl class="dmp__facts">
              <div v-for="fact in facts" :key="fact.label" class="dmp__fact">
                <dt>{{ fact.label }}</dt>
                <dd :title="fact.value">{{ fact.value }}</dd>
              </div>
            </dl>
            <!-- Строка интервала занимает место всегда: иначе карточка растёт,
                 и шкала плеера уезжает из-под курсора между щелчками. -->
            <p class="dmp__range" :class="{ 'is-empty': !range && !rangeNote }" :aria-hidden="!range && !rangeNote ? 'true' : null">
              <template v-if="range">
                <i class="fas fa-cut" aria-hidden="true"></i>
                <span>{{ $t("#L_DEMO_RANGE", clock(range.from), clock(range.to)) }}</span>
                <button type="button" class="dmp__link" @click="resetRange">{{ $t("#L_DEMO_RANGE_RESET") }}</button>
              </template>
              <span v-if="rangeNote" class="dmp__range-note" role="status" :title="$t(rangeNote)">{{ $t(rangeNote) }}</span>
            </p>
            <p v-if="ownOutcome && ownOutcome.error" class="dmp__failed" role="alert">
              <i class="fas fa-exclamation-circle" aria-hidden="true"></i>
              <span>{{ $t("#L_DEMO_EX_FAILED") }}: {{ errorText(ownOutcome.error) }}</span>
              <button type="button" class="dmp__link" @click="exportOpen = true">{{ $t("#L_DEMO_EX_DETAILS") }}</button>
            </p>
            <div v-if="detail.item.source === 'manual'" class="dmp__actions">
              <button type="button" class="dmp__btn" :disabled="removing" :title="$t('#L_DEMO_REMOVE_HINT')" @click="removeItem">
                <i class="fas fa-times" aria-hidden="true"></i>{{ $t("#L_DEMO_REMOVE") }}
              </button>
            </div>
          </div>

          <div class="dmp__side">
            <!-- Набор: ручной — с порядком, серия из папки игры — по номерам, только файлы. -->
            <template v-if="detail.item.kind === 'segments'">
              <h3 class="dmp__side-title">{{ $t(detail.item.series ? "#L_DEMO_SERIES_FILES" : "#L_DEMO_ORDER_TITLE") }}</h3>
              <ol class="dmp__rows">
                <li v-for="(segment, index) in detail.segments" :key="segment.name + index" class="dmp__row">
                  <span class="dmp__row-num">{{ index + 1 }}</span>
                  <span class="dmp__row-name" :title="segment.name">{{ segment.name }}</span>
                  <span class="dmp__row-time">{{ segment.duration === null ? "" : clock(segment.duration, 1) }}</span>
                  <template v-if="detail.item.source === 'manual'">
                    <button type="button" class="dmp__row-btn" :disabled="index === 0 || reordering" :aria-label="$t('#L_DEMO_ORDER_UP') + ': ' + segment.name"
                      :title="$t('#L_DEMO_ORDER_UP')" @click="move(index, -1)"><i class="fas fa-arrow-up" aria-hidden="true"></i></button>
                    <button type="button" class="dmp__row-btn" :disabled="index === detail.segments.length - 1 || reordering"
                      :aria-label="$t('#L_DEMO_ORDER_DOWN') + ': ' + segment.name" :title="$t('#L_DEMO_ORDER_DOWN')" @click="move(index, 1)"><i class="fas fa-arrow-down" aria-hidden="true"></i></button>
                  </template>
                </li>
              </ol>
            </template>
            <template v-else>
              <h3 class="dmp__side-title">{{ $t("#L_DEMO_MARKS_TITLE") }} <span class="dmp__group-count">{{ sideMarkers.length }}</span></h3>
              <p v-if="!sideMarkers.length" class="dmp__side-empty">{{ $t("#L_DEMO_MARKS_EMPTY") }}</p>
              <ol v-else class="dmp__rows">
                <li v-for="(marker, index) in sideMarkers" :key="index">
                  <button type="button" class="dmp__row dmp__row--btn" @click="jumpTo(marker)">
                    <span class="dmp__row-dot" :class="'is-' + marker.type" aria-hidden="true"></span>
                    <span class="dmp__row-name">{{ markerText(marker) }}</span>
                    <span class="dmp__row-time">{{ clock(marker.demoTime) }}</span>
                  </button>
                </li>
              </ol>
            </template>
          </div>
        </div>
      </div>
    </div>

    <!-- «Отрендеренные»: видео из папки вывода с метками из их timeline.json. -->
    <div v-show="tab === 'rendered'" class="dmp__body">
      <aside class="dmp__list" :aria-label="$t('#L_DEMO_TAB_RENDERED')">
        <div v-if="rendered.loading && !rendered.items.length" class="dmp__empty">
          <i class="fas fa-spinner fa-spin" aria-hidden="true"></i>
        </div>
        <div v-else-if="!rendered.items.length" class="dmp__empty">
          <i class="fas fa-photo-video" aria-hidden="true"></i>
          <b>{{ $t("#L_DEMO_RENDERED_EMPTY_TITLE") }}</b>
          <span>{{ $t("#L_DEMO_RENDERED_EMPTY_TEXT") }}</span>
          <span class="dmp__empty-path" :title="rendered.outputDir">{{ rendered.outputDir }}</span>
        </div>
        <section v-else class="dmp__group">
          <button
            v-for="video in rendered.items"
            :key="video.name"
            type="button"
            class="dmp__item"
            :class="{ active: rendered.open && rendered.open.name === video.name }"
            :aria-current="rendered.open && rendered.open.name === video.name ? 'true' : null"
            @click="openRendered(video.name)"
          >
            <i class="dmp__item-icon fas fa-file-video" aria-hidden="true"></i>
            <span class="dmp__item-main">
              <span class="dmp__item-name" :title="video.name">{{ video.name }}</span>
              <span class="dmp__item-meta">{{ renderedSource(video) }}</span>
              <span class="dmp__item-meta is-faint">{{ renderedLine(video) }}</span>
            </span>
          </button>
        </section>
      </aside>

      <div class="dmp__main">
        <DemoPlayer
          ref="renderedPlayer"
          class="dmp__player"
          :src="rendered.open ? rendered.open.url : null"
          :video="rendered.open ? rendered.open.video : null"
          :markers="rendered.open ? rendered.open.markers : []"
          :marking="false"
          :marker-text="markerText"
          @error="rendered.mediaError = true"
          @expanded="playerExpanded = $event"
        >
          <template v-if="!rendered.open || rendered.mediaError" #overlay>
            <div class="dmp__screen">
              <template v-if="rendered.mediaError">
                <i class="fas fa-exclamation-triangle dmp__screen-icon is-error" aria-hidden="true"></i>
                <span class="dmp__screen-title">{{ $t("#L_DEMO_PLAYER_ERROR") }}</span>
              </template>
              <template v-else>
                <i class="fas fa-film dmp__screen-icon" aria-hidden="true"></i>
                <span class="dmp__screen-text">{{ $t("#L_DEMO_RENDERED_PICK") }}</span>
              </template>
            </div>
          </template>
        </DemoPlayer>

        <!-- Нижняя карточка сворачивается кнопкой посередине (просьба владельца 14.11):
             без неё плеер на всю высоту, без чёрных полос по бокам. -->
        <button type="button" class="dmp__fold" :class="{ 'is-folded': cardHidden }" :aria-expanded="cardHidden ? 'false' : 'true'"
          :aria-label="$t(cardHidden ? '#L_DEMO_CARD_SHOW' : '#L_DEMO_CARD_HIDE')" :title="$t(cardHidden ? '#L_DEMO_CARD_SHOW' : '#L_DEMO_CARD_HIDE')"
          @click="toggleCard">
          <i class="fas" :class="cardHidden ? 'fa-chevron-up' : 'fa-chevron-down'" aria-hidden="true"></i>
        </button>

        <div v-if="!rendered.open" v-show="!cardHidden" class="dmp__card dmp__card--empty">
          <span>{{ $t("#L_DEMO_RENDERED_PICK") }}</span>
        </div>
        <div v-else v-show="!cardHidden" class="dmp__card">
          <div class="dmp__info">
            <div class="dmp__info-head">
              <h2 class="dmp__info-title" :title="rendered.open.name">{{ rendered.open.name }}</h2>
              <button type="button" class="dmp__btn primary dmp__head-btn" :disabled="!rendered.open.sourceId || !!job"
                :title="$t(rendered.open.sourceId ? '#L_DEMO_RENDERED_EXPORT_HINT' : '#L_DEMO_RENDERED_NO_SOURCE')" @click="exportFromRendered">
                <i class="fas fa-file-video" aria-hidden="true"></i>{{ $t("#L_DEMO_EXPORT") }}
              </button>
              <!-- «Открыть папку» и «Удалить» — значками рядом с «Экспортом»: внизу на низком окне
                   их было видно только прокруткой карточки. -->
              <button type="button" class="dmp__icon-btn dmp__head-icon" :title="$t('#L_DEMO_RENDERED_REVEAL')" :aria-label="$t('#L_DEMO_RENDERED_REVEAL')" @click="revealRendered">
                <i class="fas fa-folder-open" aria-hidden="true"></i>
              </button>
              <button type="button" class="dmp__icon-btn dmp__head-icon" :disabled="rendered.deleting" :title="$t('#L_DEMO_RENDERED_DELETE_HINT')" :aria-label="$t('#L_DEMO_RENDERED_DELETE')" @click="rendered.confirm = rendered.open.name">
                <i class="fas fa-trash-alt" aria-hidden="true"></i>
              </button>
            </div>
            <dl class="dmp__facts">
              <div v-for="fact in renderedFacts" :key="fact.label" class="dmp__fact">
                <dt>{{ fact.label }}</dt>
                <dd :title="fact.value">{{ fact.value }}</dd>
              </div>
            </dl>
            <div v-if="!rendered.open.sourceId" class="dmp__actions">
              <span class="dmp__actions-note">{{ $t("#L_DEMO_RENDERED_NO_SOURCE") }}</span>
            </div>
          </div>
          <div class="dmp__side">
            <h3 class="dmp__side-title">{{ $t("#L_DEMO_MARKS_TITLE") }} <span class="dmp__group-count">{{ rendered.open.markers.length }}</span></h3>
            <p v-if="!rendered.open.markers.length" class="dmp__side-empty">{{ $t("#L_DEMO_MARKS_EMPTY") }}</p>
            <ol v-else class="dmp__rows">
              <li v-for="(marker, index) in rendered.open.markers" :key="index">
                <button type="button" class="dmp__row dmp__row--btn" @click="$refs.renderedPlayer.seek(marker.t)">
                  <span class="dmp__row-dot" :class="'is-' + marker.type" aria-hidden="true"></span>
                  <span class="dmp__row-name">{{ markerText(marker) }}</span>
                  <span class="dmp__row-time">{{ clock(marker.t) }}</span>
                </button>
              </li>
            </ol>
          </div>
        </div>
      </div>
    </div>

    <transition name="dmp-drop">
      <div v-if="dragging" class="dmp__drop" aria-hidden="true">
        <div class="dmp__drop-box">
          <i class="fas fa-file-import" aria-hidden="true"></i>
          <b>{{ $t("#L_DEMO_DROP_TITLE") }}</b>
          <span>{{ $t("#L_DEMO_DROP_TEXT") }}</span>
        </div>
      </div>
    </transition>

    <Teleport to=".launcher-root">
      <transition name="l-modal">
        <DemoExportModal
          v-if="exportOpen && detail"
          :item="detail.item"
          :duration="detail.item.duration || 0"
          :range="range"
          :ffmpeg="ffmpeg"
          :ffmpeg-error="ffmpegError"
          :bxtrs="bxtrs"
          :bxtrs-error="bxtrsError"
          :job="job && job.kind === 'export' && job.itemId === detail.item.id ? job : null"
          :result="ownOutcome && ownOutcome.result"
          :job-error="ownOutcome && ownOutcome.error"
          :busy="!!job"
          :cancelled="!!(cancelled && cancelled.kind === 'export' && cancelled.itemId === detail.item.id)"
          :long-preview="longPreview && longPreview.itemId === detail.item.id ? longPreview : null"
          :initial="exportInitial"
          @close="exportOpen = false"
          @preview-anyway="previewAnyway"
          @start="startExport"
          @cancel-job="cancelJob"
          @install-ffmpeg="installFfmpeg"
          @cancel-ffmpeg="cancelFfmpeg"
          @install-bxtrs="installBxtrs"
          @cancel-bxtrs="cancelBxtrs"
          @range="range = $event"
          @open-result="openResult"
          @again="exportOutcome = null"
        />
      </transition>
      <transition name="l-modal">
        <ConfirmModal
          v-if="rendered.confirm"
          :title="$t('#L_DEMO_RENDERED_DELETE_TITLE')"
          :text="$t('#L_DEMO_RENDERED_DELETE_TEXT')"
          :items="[rendered.confirm, rendered.confirm.replace(/\.mp4$/i, '.timeline.json')]"
          :confirm-label="$t('#L_DEMO_RENDERED_DELETE_OK')"
          danger
          @resolve="onDeleteResolved"
        />
      </transition>
    </Teleport>
  </section>
</template>

<script>
import DemoPlayer from "./DemoPlayer";
import DemoExportModal from "./DemoExportModal";
import DemoFfmpegCard from "./DemoFfmpegCard";
import ConfirmModal from "./ConfirmModal";
import DemoViewSettings from "./DemoViewSettings";
import launcherData from "./launcher-data";
import { formatSize, formatDate } from "./file-format";
import { formatClock, gameName, groupName, groupItems, hasDemoExtension, errorMessage, playerInput, isRenderedFile, markRange, firstPick, chapterKey } from "./demo-tool";

const { ipcRenderer } = window.hlsrElectron;
const GROUP_LOGOS = {
  "hl1-won": launcherData.gameArt["70"].logo,
  "ghosting-renewed": launcherData.gameArt["218"].logo,
};
const EVENTS = ["demo-job-progress", "demo-job-done", "demo-job-error", "demo-ffmpeg-progress", "demo-bxtrs-progress"];
// Смена языка пересоздаёт рамку оболочки вместе с keep-alive: идущее задание,
// итог экспорта и готовое видео переходят в новую страницу через модуль.
const carried = { job: null, exportOutcome: null, media: null };

const CARD_HIDDEN_KEY = "hlsr.demo.cardHidden";

function readCardHidden() {
  try {
    return localStorage.getItem(CARD_HIDDEN_KEY) === "1";
  } catch (error) {
    return false;
  }
}

export default {
  name: "DemoPanel",
  components: { DemoPlayer, DemoExportModal, DemoFfmpegCard, ConfirmModal, DemoViewSettings },
  emits: ["notify"],
  data() {
    return {
      // Нижняя карточка свёрнута (кнопка dmp__fold); запоминается в localStorage.
      cardHidden: readCardHidden(),
      items: [],
      loading: false,
      selectedId: null,
      detail: null,
      detailLoading: false,
      reordering: false,
      removing: false,
      notice: null,
      dragging: false,
      dragDepth: 0,
      // Интервал экспорта во времени демо выбранной записи; null — весь ран.
      range: null,
      // Подсказка у строки интервала (начало и конец поменялись местами).
      rangeNote: null,
      // Последняя отмена: { itemId, kind } — «Экспорт/Предпросмотр отменён».
      cancelled: null,
      ffmpeg: null,
      ffmpegError: null,
      // bxt-rs для рендера HL1 (demo-bxtrs-status): карточка вместо «не готово» при BXTRS_MISSING.
      bxtrs: null,
      bxtrsError: null,
      // Идущее задание: { jobId, kind, itemId, phase, percent, segment, message }.
      job: carried.job,
      // Видео в плеере: { itemId, result } (предпросмотр или готовый экспорт).
      media: carried.media,
      mediaError: false,
      previewError: null,
      // Итог последнего экспорта: { itemId, result } или { itemId, error }.
      exportOutcome: carried.exportOutcome,
      exportOpen: false,
      playerExpanded: false,
      active: false,
      // Окно экспорта открыто вместо долгого предпросмотра: { itemId, seconds, range, segments } (demo-estimate).
      longPreview: null,
      // Стартовые настройки окна экспорта поверх запомненных (экспорт из «Отрендеренных»): { speed }.
      exportInitial: null,
      // Вкладка под заголовком: «demos» — демки, «rendered» — сохранённые видео.
      tab: "demos",
      // Вид предпросмотра (demo-settings-get: preview): KeyHUD, HUD, конфиг и HUD игрока.
      previewView: { keyhud: true, hud: true, ownConfig: true, ownHud: true },
      // «Отрендеренные»: список папки вывода, открытое видео ({ name, url, video, source, markers }),
      // имя видео в окне подтверждения удаления.
      rendered: { items: [], outputDir: "", loading: false, open: null, mediaError: false, confirm: null, deleting: false },
    };
  },
  computed: {
    groups() {
      return groupItems(this.items);
    },
    listBusy() {
      return this.tab === "rendered" ? this.rendered.loading : this.loading;
    },
    renderedFacts() {
      const open = this.rendered.open;
      const item = this.rendered.items.find((video) => video.name === open.name) || {};
      const video = open.video;
      const facts = [{ label: this.$t("#L_DEMO_F_GAME"), value: gameName(open.source.game) || "—" }];
      facts.push({ label: this.$t("#L_DEMO_F_MAP"), value: open.source.map || "—" });
      facts.push({ label: this.$t("#L_DEMO_F_LENGTH"), value: video.duration ? this.clock(video.duration) : "—" });
      if (video.width && video.height) facts.push({ label: this.$t("#L_DEMO_F_VIDEO"), value: `${video.width}×${video.height}${video.fps ? `, ${video.fps} FPS` : ""}` });
      if (video.speed && video.speed !== 1) facts.push({ label: this.$t("#L_DEMO_F_SPEED"), value: `${String(video.speed).replace(".", this.$t("#L_DECIMAL"))}×` });
      if (item.mtimeMs) facts.push({ label: this.$t("#L_DEMO_F_DATE"), value: formatDate(item.mtimeMs) });
      if (item.size) facts.push({ label: this.$t("#L_DEMO_F_SIZE"), value: formatSize(item.size, this.$t) });
      return facts;
    },
    sourceMarkers() {
      return this.detail ? this.detail.markers : [];
    },
    // Плеер видит только видео выбранной записи (иначе шкалу её демки).
    player() {
      return playerInput(this.media, this.detail);
    },
    mediaForSelection() {
      return this.player.own;
    },
    // Идёт экспорт выбранной записи.
    ownExport() {
      return !!(this.job && this.job.kind === "export" && this.detail && this.job.itemId === this.detail.item.id);
    },
    // Итог экспорта выбранной записи.
    ownOutcome() {
      return this.exportOutcome && this.detail && this.exportOutcome.itemId === this.detail.item.id ? this.exportOutcome : null;
    },
    itemGameName() {
      const item = this.detail && this.detail.item;
      return item ? groupName(item.gameId) || gameName(item.game) || "" : "";
    },
    // Метки в карточке — всегда во времени демо.
    sideMarkers() {
      if (!this.detail) return [];
      if (this.mediaForSelection) return this.media.result.markers.map((marker) => Object.assign({}, marker, { demoTime: marker.sourceTime }));
      return this.detail.markers.map((marker) => Object.assign({}, marker, { demoTime: marker.t }));
    },
    ffmpegReady() {
      return !!(this.ffmpeg && this.ffmpeg.state === "ready");
    },
    canPreview() {
      return !!(this.detail && this.detail.item.supported && this.detail.render.ok && this.ffmpegReady && !this.job);
    },
    // Что показать поверх экрана плеера; null — само видео.
    screen() {
      if (!this.detail) return "empty";
      if (this.job && this.job.kind === "preview" && this.job.itemId === this.detail.item.id) return "preview-job";
      if (this.mediaForSelection) return this.mediaError ? "media-error" : null;
      if (!this.detail.item.supported) return "unsupported";
      if (!this.detail.render.ok) return this.detail.render.code === "BXTRS_MISSING" && this.bxtrs ? "bxtrs" : "not-ready";
      if (this.ffmpeg && !this.ffmpegReady) return "ffmpeg";
      if (this.previewError && this.previewError.itemId === this.detail.item.id) return "preview-error";
      return "ready";
    },
    facts() {
      const item = this.detail.item;
      const facts = [{ label: this.$t("#L_DEMO_F_GAME"), value: gameName(item.game) || "—" }];
      if (item.maps && item.maps.length > 1) facts.push({ label: this.$t("#L_DEMO_F_MAPS"), value: item.maps.join(", ") });
      else facts.push({ label: this.$t("#L_DEMO_F_MAP"), value: item.map || "—" });
      facts.push({ label: this.$t("#L_DEMO_F_LENGTH"), value: item.duration ? this.clock(item.duration) : "—" });
      if (item.runTime) facts.push({ label: this.$t("#L_DEMO_F_TIMER"), value: this.clock(item.runTime) });
      if (item.segmentCount > 1) facts.push({ label: this.$t("#L_DEMO_F_SEGMENTS"), value: String(item.segmentCount) });
      // Серия из папки игры: каких номеров нет (удалены или не записались).
      if (item.series && item.missing && item.missing.length) facts.push({ label: this.$t("#L_DEMO_F_MISSING"), value: item.missing.join(", ") });
      facts.push({ label: this.$t("#L_DEMO_F_DATE"), value: formatDate(item.mtimeMs) });
      facts.push({ label: this.$t("#L_DEMO_F_SIZE"), value: item.size ? formatSize(item.size, this.$t) : "—" });
      return facts;
    },
  },
  watch: {
    // Закрытое окно экспорта забывает «Экспорт отменён».
    exportOpen(open) {
      if (!open && this.cancelled && this.cancelled.kind === "export") this.cancelled = null;
      if (!open) {
        this.longPreview = null;
        this.exportInitial = null;
      }
    },
  },
  created() {
    for (const channel of EVENTS) ipcRenderer.on(channel, this.handlers()[channel]);
  },
  beforeUnmount() {
    for (const channel of EVENTS) ipcRenderer.removeListener(channel, this.handlers()[channel]);
    if (this.active) ipcRenderer.invoke("demo-preview-clear").catch(() => {});
    // Предпросмотр снят выше; экспорт и его итог живут дальше.
    carried.job = this.job;
    carried.exportOutcome = this.exportOutcome;
    carried.media = this.media && this.media.result.file ? this.media : null;
  },
  // Страница живёт в keep-alive: задание экспорта переживает уход на другую вкладку.
  activated() {
    this.active = true;
    this.loadList();
    this.loadFfmpeg();
    this.loadBxtrs();
    this.loadSettings();
    this.loadRendered();
  },
  // Предпросмотр хранится, только пока страница открыта.
  deactivated() {
    this.active = false;
    this.exportOpen = false;
    for (const player of [this.$refs.player, this.$refs.renderedPlayer])
      if (player) {
        player.pause();
        player.exitFull();
      }
    this.rendered.confirm = null;
    if (this.media && !this.media.result.file) this.media = null;
    // Предпросмотр гасится; задание уходит из интерфейса по его CANCELLED, а до
    // того «Просмотр» закрыт, и отменить его можно с экрана плеера.
    ipcRenderer.invoke("demo-preview-clear").catch(() => {});
  },
  methods: {
    toggleCard() {
      this.cardHidden = !this.cardHidden;
      try {
        localStorage.setItem(CARD_HIDDEN_KEY, this.cardHidden ? "1" : "0");
      } catch (error) {
        // Положение просто не запомнится.
      }
    },
    // Одни и те же функции для on и removeListener.
    handlers() {
      if (!this.boundHandlers)
        this.boundHandlers = {
          "demo-job-progress": (event, payload) => this.onJobProgress(payload),
          "demo-job-done": (event, payload) => this.onJobDone(payload),
          "demo-job-error": (event, payload) => this.onJobError(payload),
          "demo-ffmpeg-progress": (event, payload) => {
            if (this.ffmpeg && payload) this.ffmpeg = Object.assign({}, this.ffmpeg, { state: "installing", percent: payload.percent });
          },
          "demo-bxtrs-progress": (event, payload) => {
            if (this.bxtrs && payload) this.bxtrs = Object.assign({}, this.bxtrs, { state: "installing", percent: payload.percent });
          },
        };
      return this.boundHandlers;
    },
    clock(seconds, digits) {
      return formatClock(seconds, digits === undefined ? 1 : digits);
    },
    errorText(error) {
      const message = errorMessage(error);
      return this.$t(message.key, ...message.args);
    },
    reasonText(item) {
      const code = item.reason || "DEMO_UNSUPPORTED";
      // У SEGMENT_* номер уже в тексте.
      if (/^SEGMENT_/.test(code)) return this.errorText({ code, segment: item.failedSegment });
      const text = this.errorText({ code });
      // «Сегмент N:» — только у рана из нескольких файлов.
      return item.failedSegment && item.segmentCount > 1 ? this.$t("#L_DEMO_SEGMENT_PREFIX", String(item.failedSegment), text) : text;
    },
    markerText(marker) {
      if (marker.type === "segment") return this.$t("#L_DEMO_MARK_SEGMENT", marker.label);
      if (marker.type === "saveload") return this.$t("#L_DEMO_MARK_SAVELOAD", marker.label);
      if (marker.type === "timer") return this.$t("#L_DEMO_MARK_TIMER", marker.label);
      if (marker.type === "chapter") return this.$t("#L_DEMO_MARK_CHAPTER", chapterKey(marker.label) ? this.$t(chapterKey(marker.label)) : marker.label);
      if (marker.type === "finish") return this.$t("#L_DEMO_MARK_FINISH", marker.label);
      return marker.label;
    },
    groupName(group) {
      if (group.id === "manual") return this.$t("#L_DEMO_GROUP_MANUAL");
      return group.name || this.$t("#L_DEMO_GROUP_OTHER");
    },
    groupLogo(id) {
      return GROUP_LOGOS[id] || null;
    },
    kindIcon(item) {
      if (item.kind === "playlist") return "fas fa-list-ol";
      if (item.kind === "segments") return "fas fa-layer-group";
      return "fas fa-film";
    },
    // Две строки под именем: игра и карта; длина, сегменты и дата.
    itemMeta(item) {
      return [gameName(item.game), item.map].filter(Boolean).join(" · ");
    },
    // Серия HL1 из папки игры: «hlsrrun (60 демок)».
    itemName(item) {
      if (!item.series) return item.name;
      const lang = Object.keys(this.$localization.locales)[this.$localization.lang] === "ru" ? "ru-RU" : "en-GB";
      const form = new Intl.PluralRules(lang).select(item.segmentCount);
      const key = form === "one" ? "#L_DEMO_SERIES_ONE" : form === "few" ? "#L_DEMO_SERIES_FEW" : "#L_DEMO_SERIES_MANY";
      return this.$t(key, item.name, String(item.segmentCount));
    },
    itemDate(item) {
      const parts = [];
      if (item.duration) parts.push(this.clock(item.duration, 0));
      if (item.segmentCount > 1 && !item.series) parts.push(this.$t("#L_DEMO_SEGMENTS", String(item.segmentCount)));
      parts.push(formatDate(item.mtimeMs));
      // Строка переносится только после «·»: в узком списке (Full HD) дата целиком уходит на вторую строку.
      return parts.filter(Boolean).map((part) => part.replace(/ /g, "\u00a0")).join("\u00a0· ");
    },
    jobLine(job) {
      const phase = this.$t({ prepare: "#L_DEMO_PHASE_PREPARE", render: "#L_DEMO_PHASE_RENDER", encode: "#L_DEMO_PHASE_ENCODE",
        verify: "#L_DEMO_PHASE_VERIFY", done: "#L_DEMO_PHASE_DONE" }[job.phase] || "#L_DEMO_PHASE_PREPARE");
      const retry = /^retry-(\d+)$/.exec(job.message || "");
      const parts = [retry ? this.$t("#L_DEMO_JOB_RETRY", String(Number(retry[1]) + 1)) : phase];
      if (job.segment && job.segment.count > 1) parts.push(this.$t("#L_DEMO_JOB_SEGMENT", String(job.segment.index), String(job.segment.count)));
      parts.push(Math.round(job.percent || 0) + "%");
      return parts.join(" · ");
    },

    setTab(id) {
      if (id === this.tab) return;
      // Видео другой вкладки не играет за кадром.
      const leaving = this.tab === "demos" ? this.$refs.player : this.$refs.renderedPlayer;
      if (leaving) leaving.pause();
      this.tab = id;
      if (id === "rendered") this.loadRendered();
    },
    refresh() {
      if (this.tab === "rendered") this.loadRendered();
      else this.loadList();
    },
    async loadSettings() {
      const answer = await ipcRenderer.invoke("demo-settings-get").catch(() => null);
      if (answer && answer.status === "ok" && answer.preview) this.previewView = Object.assign({}, this.previewView, answer.preview);
    },

    // ---------- отрендеренные ----------
    renderedSource(video) {
      return [gameName(video.source.game), video.source.map].filter(Boolean).join(" · ");
    },
    renderedLine(video) {
      const parts = [];
      if (video.video.duration) parts.push(this.clock(video.video.duration, 0));
      if (video.size) parts.push(formatSize(video.size, this.$t));
      parts.push(formatDate(video.mtimeMs));
      return parts.filter(Boolean).join(" · ");
    },
    async loadRendered() {
      this.rendered.loading = true;
      try {
        const answer = await ipcRenderer.invoke("demo-rendered-list").catch(() => null);
        if (answer && answer.status === "ok") this.applyRendered(answer);
      } finally {
        this.rendered.loading = false;
      }
    },
    applyRendered(answer) {
      this.rendered.items = Array.isArray(answer.items) ? answer.items : [];
      this.rendered.outputDir = answer.outputDir || "";
      // Открытое видео пропало из папки — плеер пустеет.
      if (this.rendered.open && !this.rendered.items.some((video) => video.name === this.rendered.open.name)) this.rendered.open = null;
    },
    async openRendered(name) {
      if (this.rendered.open && this.rendered.open.name === name) return;
      if (this.$refs.renderedPlayer) this.$refs.renderedPlayer.pause();
      const answer = await ipcRenderer.invoke("demo-rendered-open", name).catch(() => null);
      if (answer && answer.status === "ok") {
        this.rendered.open = { name: answer.name, url: answer.url, video: answer.video, source: answer.source, markers: answer.markers || [],
          sourceId: answer.sourceId || null };
        this.rendered.mediaError = false;
        return;
      }
      this.notice = { text: this.errorText(answer || {}), error: true };
      this.loadRendered();
    },
    // Видео удалили: плеер «Демок» и итог экспорта о нём забывают — снова предпросмотр и экспорт демки.
    forgetRendered(name, file) {
      const which = { name, outputDir: this.rendered.outputDir, file };
      if (this.media && isRenderedFile(this.media.result, which)) this.closeMedia();
      if (this.exportOutcome && isRenderedFile(this.exportOutcome.result, which)) this.exportOutcome = null;
    },
    closeMedia() {
      if (this.$refs.player) this.$refs.player.pause();
      this.media = null;
      this.mediaError = false;
    },
    async revealRendered() {
      const answer = await ipcRenderer.invoke("demo-rendered-reveal", this.rendered.open.name).catch(() => null);
      if (!answer || answer.status !== "ok") {
        this.notice = { text: this.errorText(answer || {}), error: true };
        this.loadRendered();
      }
    },
    // «Экспорт» готового видео: окно экспорта его исходной демки с тем же отрезком и скоростью.
    async exportFromRendered() {
      const open = this.rendered.open;
      if (!open || !open.sourceId || this.job) return;
      this.setTab("demos");
      await this.select(open.sourceId);
      if (!this.detail || this.detail.item.id !== open.sourceId) return;
      const range = open.video && open.video.range;
      const duration = this.detail.item.duration || 0;
      this.range = range && !(range.from <= 0.001 && range.to >= duration - 0.001) ? { from: range.from, to: range.to } : null;
      this.rangeNote = null;
      this.exportInitial = { speed: open.video && open.video.speed ? open.video.speed : 1 };
      this.exportOpen = true;
    },
    async onDeleteResolved(accepted) {
      const name = this.rendered.confirm;
      this.rendered.confirm = null;
      if (!accepted || !name || this.rendered.deleting) return;
      // Плееры отпускают файл до Корзины: иначе Windows держит его открытым.
      if (this.rendered.open && this.rendered.open.name === name) this.rendered.open = null;
      this.forgetRendered(name, null);
      this.rendered.deleting = true;
      await this.$nextTick();
      try {
        const answer = await ipcRenderer.invoke("demo-rendered-delete", name).catch(() => null);
        if (answer && answer.status === "ok") {
          this.forgetRendered(name, answer.deleted);
          this.applyRendered(answer);
          this.$emit("notify", { text: this.$t("#L_DEMO_RENDERED_DELETED", name), icon: "fas fa-trash-alt" });
        } else {
          this.notice = { text: this.errorText(answer || {}), error: true };
          this.loadRendered();
        }
      } finally {
        this.rendered.deleting = false;
      }
    },

    // ---------- список ----------
    async loadList() {
      this.loading = true;
      try {
        const answer = await ipcRenderer.invoke("demo-list");
        if (answer && answer.status === "ok") this.applyItems(answer.items);
      } catch (error) {
        this.notice = { text: this.errorText({ code: "DEMO_FAILED" }), error: true };
      } finally {
        this.loading = false;
      }
    },
    applyItems(items) {
      this.items = Array.isArray(items) ? items : [];
      if (this.selectedId && !this.items.some((item) => item.id === this.selectedId)) {
        this.selectedId = null;
        this.detail = null;
      }
      // По умолчанию — первая поддерживаемая запись (битые файлы бывают новее).
      if (!this.selectedId && this.items.length) this.select(firstPick(this.groups));
      else if (this.selectedId) this.inspect(this.selectedId);
    },
    async select(id) {
      if (id === this.selectedId && this.detail) return;
      if (this.$refs.player) this.$refs.player.pause();
      this.selectedId = id;
      this.range = null;
      this.rangeNote = null;
      this.cancelled = null;
      this.mediaError = false;
      await this.inspect(id);
    },
    async inspect(id) {
      this.detailLoading = true;
      try {
        const answer = await ipcRenderer.invoke("demo-inspect", id);
        if (id !== this.selectedId) return;
        if (answer && answer.status === "ok") this.detail = answer;
        else {
          this.detail = null;
          this.notice = { text: this.errorText(answer || {}), error: true };
        }
      } catch (error) {
        if (id === this.selectedId) this.detail = null;
      } finally {
        this.detailLoading = false;
      }
    },
    async pickFiles() {
      try {
        const answer = await ipcRenderer.invoke("demo-pick-files");
        if (answer && answer.status !== "cancelled") this.applyAdded(answer, []);
      } catch (error) {
        this.notice = { text: this.errorText({ code: "DIALOG_UNAVAILABLE" }), error: true };
      }
    },
    async addPaths(paths, extraRejected) {
      try {
        const answer = await ipcRenderer.invoke("demo-add-paths", paths);
        this.applyAdded(answer, extraRejected);
      } catch (error) {
        this.notice = { text: this.errorText({ code: "DEMO_FAILED" }), error: true };
      }
    },
    // Итог добавления: сколько добавлено и что не подошло (с причиной).
    applyAdded(answer, extraRejected) {
      if (!answer || answer.status !== "ok") {
        // PATH_INVALID на весь вызов — больше 512 путей разом.
        this.notice = { text: answer && answer.code === "PATH_INVALID" ? this.$t("#L_DEMO_ERR_TOO_MANY") : this.errorText(answer || {}), error: true };
        return;
      }
      const rejected = extraRejected.concat((answer.rejected || []).map((entry) => ({ name: entry.name, reason: this.errorText({ code: entry.code }) })));
      const added = (answer.added || []).length;
      this.notice = {
        text: added ? this.$t("#L_DEMO_ADDED", String(added)) : rejected.length ? "" : this.$t("#L_DEMO_NOTHING_ADDED"),
        rejected,
        error: !added && rejected.length > 0,
      };
      this.items = answer.items || this.items;
      if (added) {
        this.tab = "demos";
        this.selectedId = null;
        this.select(answer.added[0]);
      }
    },
    async removeItem() {
      const id = this.detail && this.detail.item.id;
      if (!id || this.removing) return;
      this.removing = true;
      try {
        const answer = await ipcRenderer.invoke("demo-remove", id).catch(() => null);
        if (answer && answer.status === "ok") {
          if (this.media && this.media.itemId === id) this.media = null;
          this.selectedId = null;
          this.detail = null;
          this.applyItems(answer.items);
        } else this.notice = { text: this.errorText(answer || {}), error: true };
      } finally {
        this.removing = false;
      }
    },
    async move(index, delta) {
      const count = this.detail.segments.length;
      const order = [...Array(count).keys()];
      [order[index], order[index + delta]] = [order[index + delta], order[index]];
      this.reordering = true;
      try {
        const answer = await ipcRenderer.invoke("demo-reorder", this.detail.item.id, order).catch(() => null);
        if (answer && answer.status === "ok") {
          this.detail = answer;
          // Старый предпросмотр снят с другого порядка.
          if (this.mediaForSelection) this.media = null;
          this.range = null;
          this.items = this.items.map((item) => (item.id === answer.item.id ? answer.item : item));
        } else this.notice = { text: this.errorText(answer || {}), error: true };
      } finally {
        this.reordering = false;
      }
    },

    // ---------- перетаскивание ----------
    hasFiles(event) {
      return !!(event.dataTransfer && Array.from(event.dataTransfer.types || []).includes("Files"));
    },
    onDragEnter(event) {
      if (!this.hasFiles(event)) return;
      event.preventDefault();
      this.dragDepth++;
      this.dragging = true;
    },
    onDragOver(event) {
      if (!this.hasFiles(event)) return;
      event.preventDefault();
      event.dataTransfer.dropEffect = "copy";
    },
    onDragLeave(event) {
      if (!this.hasFiles(event)) return;
      this.dragDepth = Math.max(0, this.dragDepth - 1);
      if (!this.dragDepth) this.dragging = false;
    },
    onDrop(event) {
      if (!this.hasFiles(event)) return;
      event.preventDefault();
      this.dragDepth = 0;
      this.dragging = false;
      const files = Array.from(event.dataTransfer.files || []);
      const wrong = files.filter((file) => !hasDemoExtension(file.name));
      const paths = [];
      const lost = [];
      for (const file of files.filter((entry) => hasDemoExtension(entry.name))) {
        const path = window.hlsrElectron.files && window.hlsrElectron.files.pathForFile(file);
        if (path) paths.push(path);
        else lost.push(file.name);
      }
      const extra = wrong.length ? [{ name: wrong.map((file) => file.name).join(", "), reason: this.$t("#L_DEMO_ERR_FILE_TYPE") }] : [];
      if (lost.length) extra.push({ name: lost.join(", "), reason: this.$t("#L_DEMO_DROP_NO_PATH") });
      if (paths.length) return void this.addPaths(paths, extra);
      this.notice = {
        text: lost.length ? this.$t("#L_DEMO_DROP_NO_PATH") : this.$t("#L_DEMO_DROP_WRONG", wrong.map((file) => file.name).join(", ")),
        error: true,
      };
    },

    // ---------- плеер ----------
    onMark(which, demoTime) {
      if (!this.detail) return;
      const marked = markRange(this.range, which, demoTime, this.detail.item.duration);
      this.range = marked.range;
      this.rangeNote = marked.swapped ? "#L_DEMO_RANGE_SWAPPED" : null;
    },
    resetRange() {
      this.range = null;
      this.rangeNote = null;
    },
    jumpTo(marker) {
      if (this.$refs.player) this.$refs.player.seekDemo(marker.demoTime);
    },
    // force — «Всё равно предпросмотр» из окна про долгий предпросмотр.
    async startPreview(force) {
      if (!this.canPreview) return;
      const itemId = this.detail.item.id;
      // Долгий предпросмотр (оценка выше порога) — сначала окно экспорта с советом сразу отрендерить.
      if (force !== true) {
        const estimate = await ipcRenderer.invoke("demo-estimate", itemId, [{ preset: "preview", range: this.range }]).catch(() => null);
        const preview = estimate && estimate.status === "ok" ? estimate.items[0] : null;
        if (!this.detail || this.detail.item.id !== itemId || this.job) return;
        if (preview && preview.seconds > estimate.threshold) {
          this.longPreview = { itemId, seconds: preview.seconds, range: preview.range, segments: preview.segments };
          this.exportOpen = true;
          return;
        }
      }
      this.previewError = null;
      this.cancelled = null;
      this.mediaError = false;
      if (this.media && !this.media.result.file) this.media = null;
      this.job = { jobId: null, kind: "preview", itemId, phase: "prepare", percent: 0, segment: null, message: null };
      // Чего игра не умеет, главный процесс оставит как у игрока; выбор запоминается целиком.
      const answer = await ipcRenderer.invoke("demo-preview-start", itemId, Object.assign({ range: this.range }, this.previewView))
        .catch(() => ({ code: "DEMO_FAILED" }));
      if (answer && answer.status === "ok") {
        if (this.job && this.job.kind === "preview") this.job.jobId = this.job.jobId || answer.jobId;
      } else {
        this.job = null;
        if ((answer || {}).code === "CANCELLED") this.cancelled = { itemId, kind: "preview" };
        else this.previewError = Object.assign({ itemId }, answer || {});
      }
    },
    async startExport(settings) {
      if (this.job) return;
      const itemId = this.detail.item.id;
      this.exportOutcome = null;
      this.cancelled = null;
      this.job = { jobId: null, kind: "export", itemId, phase: "prepare", percent: 0, segment: null, message: null };
      const answer = await ipcRenderer.invoke("demo-export-start", itemId, settings).catch(() => ({ code: "DEMO_FAILED" }));
      if (answer && answer.status === "ok") {
        if (this.job && this.job.kind === "export") this.job.jobId = this.job.jobId || answer.jobId;
      } else {
        this.job = null;
        if ((answer || {}).code === "CANCELLED") this.cancelled = { itemId, kind: "export" };
        else this.exportOutcome = { itemId, error: answer || {} };
      }
    },
    cancelJob() {
      // Без jobId (ответ на старт ещё не пришёл) главный процесс гасит текущее задание.
      if (this.job) ipcRenderer.invoke("demo-cancel", ...(this.job.jobId ? [this.job.jobId] : [])).catch(() => {});
    },
    // Событие нашего задания; jobId может прийти раньше ответа на старт.
    ownJob(payload) {
      if (!this.job || !payload) return false;
      if (this.job.jobId) return this.job.jobId === payload.jobId;
      if (payload.kind !== this.job.kind) return false;
      this.job.jobId = payload.jobId;
      return true;
    },
    onJobProgress(payload) {
      if (!this.ownJob(payload)) return;
      Object.assign(this.job, { phase: payload.phase, percent: payload.percent, segment: payload.segment, message: payload.message });
    },
    onJobDone(payload) {
      if (!this.ownJob(payload)) return;
      const itemId = this.job.itemId;
      this.job = null;
      if (payload.kind === "preview") {
        if (!this.active) return;
        this.media = { itemId, result: payload.result };
        this.mediaError = false;
        this.$nextTick(() => this.$refs.player && this.$refs.player.$el.focus({ preventScroll: true }));
        return;
      }
      this.exportOutcome = { itemId, result: payload.result };
      // Новое видео в папке вывода — и во вкладке «Отрендеренные».
      this.loadRendered();
      if (!this.active || !this.exportOpen || !(this.detail && this.detail.item.id === itemId)) this.$emit("notify", { text: this.$t("#L_DEMO_EXPORT_DONE_TOAST", payload.result.fileName || ""), icon: "fas fa-film" });
    },
    onJobError(payload) {
      if (!this.ownJob(payload)) return;
      const job = this.job;
      this.job = null;
      const error = payload.error || {};
      // Отмена — не ошибка, но и не молчание: строка в окне или на экране плеера.
      if (error.code === "CANCELLED") {
        this.cancelled = { itemId: job.itemId, kind: payload.kind };
        if (payload.kind === "export" && (!this.active || !this.exportOpen))
          this.$emit("notify", { text: this.$t("#L_DEMO_EXPORT_CANCELLED"), icon: "fas fa-ban" });
        return;
      }
      const selected = this.detail && this.detail.item.id === job.itemId;
      if (payload.kind === "preview") {
        this.previewError = Object.assign({ itemId: job.itemId }, error);
        if (!this.active || !selected) this.$emit("notify", { text: this.$t("#L_DEMO_PREVIEW_FAILED_TOAST", this.errorText(error)), icon: "fas fa-exclamation-triangle" });
        return;
      }
      this.exportOutcome = { itemId: job.itemId, error };
      // Окно скрыто или страница не открыта: ошибку видно тостом, а в карточке
      // записи остаётся строка «Клип не сохранился».
      if (!this.active || !this.exportOpen || !selected)
        this.$emit("notify", { text: this.$t("#L_DEMO_EXPORT_FAILED_TOAST", this.errorText(error)), icon: "fas fa-exclamation-triangle" });
    },
    openExport() {
      this.exportOpen = true;
    },
    previewAnyway() {
      this.exportOpen = false;
      this.startPreview(true);
    },
    openResult(result) {
      this.exportOpen = false;
      const itemId = this.exportOutcome ? this.exportOutcome.itemId : this.selectedId;
      this.media = { itemId, result };
      this.mediaError = false;
      if (itemId !== this.selectedId) this.select(itemId);
    },

    // ---------- FFmpeg ----------
    async loadFfmpeg() {
      try {
        const answer = await ipcRenderer.invoke("demo-ffmpeg-status");
        if (answer && answer.status === "ok") this.ffmpeg = answer.ffmpeg;
      } catch (error) {
        // Статус узнается при экспорте.
      }
    },
    async installFfmpeg() {
      if (!this.ffmpeg || this.ffmpeg.state === "installing") return;
      this.ffmpegError = null;
      this.ffmpeg = Object.assign({}, this.ffmpeg, { state: "installing", percent: 0 });
      const answer = await ipcRenderer.invoke("demo-ffmpeg-install").catch(() => ({ code: "DEMO_FAILED" }));
      if (answer && answer.status === "ok") this.ffmpeg = answer.ffmpeg;
      else {
        if (answer && answer.status !== "cancelled") this.ffmpegError = answer.code || "DEMO_FAILED";
        await this.loadFfmpeg();
      }
    },
    cancelFfmpeg() {
      ipcRenderer.invoke("demo-ffmpeg-cancel").catch(() => {});
    },

    // ---------- bxt-rs (рендер HL1) ----------
    async loadBxtrs() {
      try {
        const answer = await ipcRenderer.invoke("demo-bxtrs-status");
        if (answer && answer.status === "ok") this.bxtrs = answer.bxtrs;
      } catch (error) {
        // Без статуса остаётся экран «не готово» с текстом кода.
      }
    },
    async installBxtrs() {
      if (!this.bxtrs || this.bxtrs.state === "installing") return;
      this.bxtrsError = null;
      this.bxtrs = Object.assign({}, this.bxtrs, { state: "installing", percent: 0 });
      const answer = await ipcRenderer.invoke("demo-bxtrs-install").catch(() => ({ code: "DEMO_FAILED" }));
      if (answer && answer.status === "ok") this.bxtrs = answer.bxtrs;
      else {
        if (answer && answer.status !== "cancelled") this.bxtrsError = answer.code || "DEMO_FAILED";
        await this.loadBxtrs();
      }
      // Готовность рендера HL1 зависит от bxt-rs: обновить выбранную запись.
      if (this.selectedId) this.inspect(this.selectedId);
    },
    cancelBxtrs() {
      ipcRenderer.invoke("demo-bxtrs-cancel").catch(() => {});
    },
  },
};
</script>

<style lang="scss" scoped>
@import "./settings-fields";

.dmp {
  position: relative;
  min-height: 0;

  /* Плеер на всё окно (position: fixed) — выше шапки (20) и колонки (2). */
  &.is-player-expanded { z-index: 30; }
  padding: calc(var(--l-topbar-h) + 24px) var(--l-edge-pad) var(--l-edge-pad) var(--l-hero-pad);

  &__head {
    flex: none;
    display: flex;
    align-items: center;
    gap: 12px 20px;
    margin-bottom: 16px;
  }

  &__title {
    margin: 0;
    color: #fff;
    font-size: 32px;
    font-weight: 700;
    line-height: 40px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  &__lead {
    margin: 4px 0 0;
    color: var(--l-text-dim);
    font-size: 13.5px;
  }

  &__spacer { flex: 1; }

  /* Вкладки «Демки» и «Отрендеренные» под заголовком. */
  &__tabs {
    flex: none;
    display: flex;
    gap: 24px;
    margin: -4px 0 14px;
    border-bottom: 1px solid var(--l-border);
  }

  &__tab {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 8px;
    height: 36px;
    padding: 0 2px;
    border: 0;
    background: transparent;
    color: var(--l-text-dim);
    font: inherit;
    font-size: 13.5px;
    font-weight: 600;
    cursor: pointer;

    &:hover,
    &.active { color: var(--l-text); }
    &.active::after {
      content: "";
      position: absolute;
      right: 0;
      bottom: -1px;
      left: 0;
      height: 2px;
      border-radius: 2px 2px 0 0;
      background: var(--accent);
    }
    &:focus-visible { outline: 2px solid var(--l-accent); outline-offset: -2px; }
  }

  &__empty-path {
    max-width: 100%;
    overflow: hidden;
    color: var(--l-text-faint);
    font-size: 12px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  &__btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    min-height: 40px;
    padding: 0 16px;
    border: 1px solid var(--l-border-strong);
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.05);
    color: var(--l-text);
    font: inherit;
    font-size: 13px;
    font-weight: 600;
    white-space: nowrap;
    cursor: pointer;
    transition: background var(--l-fast), transform var(--l-fast), box-shadow var(--l-fast);

    i { font-size: 12px; }
    &:hover:not(:disabled) { background: rgba(255, 255, 255, 0.1); }
    &:active:not(:disabled) { transform: scale(0.97); }
    &:focus-visible { outline: 2px solid var(--l-accent); outline-offset: 2px; }
    &:disabled { opacity: 0.45; cursor: not-allowed; }

    &.primary {
      border-color: transparent;
      background: var(--accent);
      color: var(--accent-contrast-text);

      &:hover:not(:disabled) {
        background: var(--accent-hover);
        box-shadow: 0 6px 20px color-mix(in srgb, var(--l-accent) 35%, transparent);
      }
    }
  }

  &__icon-btn {
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    padding: 0;
    border: 1px solid var(--l-border);
    border-radius: var(--l-radius-sm);
    background: transparent;
    color: var(--l-text-dim);
    cursor: pointer;
    transition: background var(--l-fast), color var(--l-fast);

    &:hover:not(:disabled) { background: rgba(255, 255, 255, 0.06); color: var(--l-text); }
    &:focus-visible { outline: 2px solid var(--l-accent); outline-offset: 2px; }
    &:disabled { cursor: default; }
  }

  &__notice {
    flex: none;
    display: flex;
    align-items: flex-start;
    gap: 12px;
    margin: 0 0 12px;
    padding: 8px 8px 8px 12px;
    border-radius: var(--l-radius-sm);
    background: var(--l-note-bg);
    box-shadow: inset 0 0 0 1px var(--l-border);
    color: var(--l-text-dim);
    font-size: 13px;
    line-height: 1.45;

    &.is-error { color: var(--l-level-error); }
  }

  &__notice-text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding-top: 4px;
    overflow-wrap: anywhere;
  }

  &__notice-list {
    margin: 0;
    padding-left: 18px;
    color: var(--l-text-dim);

    b { color: var(--l-text); font-weight: 600; }
  }

  &__notice-close {
    flex: none;
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--l-text-dim);
    cursor: pointer;

    &:hover { background: rgba(255, 255, 255, 0.08); color: var(--l-text); }
  }

  &__body {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: clamp(260px, 24%, 340px) minmax(0, 1fr);
    gap: 16px;
  }

  /* Список: группы по играм и «Добавленные». */
  &__list {
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 10px 8px;
    overflow-y: auto;
    overscroll-behavior: contain;
    border: 1px solid var(--l-border);
    border-radius: var(--l-radius-sm);
    background: var(--l-modal-nav);
    @include gx-scrollbar;
  }

  &__empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    margin: auto 0;
    padding: 24px 16px;
    color: var(--l-text-dim);
    font-size: 13px;
    line-height: 1.45;
    text-align: center;

    i { color: var(--l-text-faint); font-size: 28px; margin-bottom: 4px; }
    b { color: var(--l-text); font-size: 14px; }
  }

  &__group {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  // Разрядка и зазоры уже: «Ghosting Mod: Renewed» в одну строку в списке шириной 260 px (Full HD).
  &__group-title {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0 0 4px;
    padding: 0 8px;
    color: var(--l-text-dim);
    font-size: 11.5px;
    font-weight: 600;
    letter-spacing: 0.05em;
    text-transform: uppercase;
  }

  &__group-logo {
    flex: none;
    width: 16px;
    height: 16px;
    background-color: currentColor;
    -webkit-mask: var(--logo) center / contain no-repeat;
    mask: var(--logo) center / contain no-repeat;
  }

  &__group-icon { font-size: 12px; }

  &__group-count {
    padding: 1px 7px;
    border-radius: 9px;
    background: rgba(255, 255, 255, 0.07);
    color: var(--l-text-faint);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0;
    font-variant-numeric: tabular-nums;
  }

  &__item {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    width: 100%;
    padding: 9px 10px;
    border: 0;
    border-radius: 8px;
    background: transparent;
    color: var(--l-text-dim);
    font: inherit;
    text-align: left;
    cursor: pointer;
    transition: background var(--l-fast), color var(--l-fast);

    &:hover { background: rgba(255, 255, 255, 0.06); color: var(--l-text); }
    &.active { background: var(--accent-soft); color: var(--l-text); box-shadow: inset 0 0 0 1px var(--accent-border); }
    &.active .dmp__item-icon { color: var(--accent); }
    &:focus-visible { outline: 2px solid var(--accent-focus, var(--l-accent)); outline-offset: -2px; }
  }

  &__item-icon {
    flex: none;
    width: 16px;
    margin-top: 2px;
    font-size: 13px;
    text-align: center;
  }

  &__item-main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  &__item-name {
    overflow: hidden;
    color: var(--l-text);
    font-size: 13px;
    font-weight: 600;
    line-height: 18px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  &__item-meta {
    overflow: hidden;
    font-size: 12px;
    line-height: 16px;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;

    &.is-faint { color: var(--l-text-faint); }
  }

  // Длина, сегменты и дата (itemDate) — переносом, без многоточия.
  &__item-meta.is-wrap { white-space: normal; }

  &__item.is-unsupported &__item-name { color: var(--l-text-dim); }

  &__badge {
    align-self: flex-start;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin-top: 3px;
    padding: 2px 8px;
    border-radius: 9px;
    background: color-mix(in srgb, var(--l-level-warning) 14%, transparent);
    color: var(--l-level-warning);
    font-size: 11px;
    font-weight: 600;
    line-height: 16px;

    i { font-size: 10px; }
  }

  /* Плеер и карточка выбранной демки. */
  &__main {
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    // Между плеером и карточкой — кнопка сворачивания 12 px: вместе с зазорами 16 px.
    gap: 2px;
  }

  // На низком окне (Full HD 125 % — 1152×648) плеер не отдаёт место карточке:
  // карточка сжимается и листается внутри, кнопки плеера остаются видны.
  &__player { flex: 1 1 auto; min-height: 60%; }

  // Полоса с кнопкой посередине: тонкая линия во всю ширину видна при наведении.
  &__fold {
    position: relative;
    flex: none;
    display: grid;
    place-items: center;
    height: 12px;
    padding: 0;
    border: none;
    background: transparent;
    color: var(--l-text-dim);
    cursor: pointer;

    &::before {
      content: "";
      position: absolute;
      left: 0;
      right: 0;
      top: calc(50% - 1px);
      height: 2px;
      border-radius: 1px;
      background: var(--accent);
      opacity: 0;
      transition: opacity var(--l-fast);
    }

    i {
      position: relative;
      display: grid;
      place-items: center;
      width: 44px;
      height: 14px;
      border-radius: 7px;
      background: var(--l-modal-bar);
      box-shadow: inset 0 0 0 1px var(--l-border-strong);
      font-size: 9px;
      transition: background var(--l-fast), color var(--l-fast), box-shadow var(--l-fast);
    }

    &:hover::before,
    &:focus-visible::before { opacity: 0.6; }

    &:hover i,
    &:focus-visible i {
      color: #fff;
      box-shadow: inset 0 0 0 1px var(--accent);
    }

    &:focus-visible { outline: none; }
  }

  &__screen {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    max-width: 520px;
    color: var(--l-text-dim);
    font-size: 13.5px;
    line-height: 1.5;
    text-align: center;
  }

  &__screen-icon {
    color: var(--l-text-faint);
    font-size: 30px;

    &.is-error { color: var(--l-level-error); }
  }

  &__screen-title {
    color: var(--l-text);
    font-size: 15px;
    font-weight: 600;
    line-height: 1.4;
  }

  &__screen-note {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--l-text-faint);
    font-size: 12px;
  }

  &__screen-bar {
    width: 280px;
    height: 6px;
    border-radius: 3px;
    background: rgba(255, 255, 255, 0.12);
    overflow: hidden;
  }

  &__screen-fill {
    height: 100%;
    background: var(--accent);
    transition: width var(--l-med);
  }

  &__play {
    display: grid;
    place-items: center;
    width: 64px;
    height: 64px;
    padding: 0 0 0 4px;
    border: 0;
    border-radius: 50%;
    background: var(--accent);
    color: var(--accent-contrast-text);
    font-size: 22px;
    cursor: pointer;
    box-shadow: 0 8px 26px color-mix(in srgb, var(--l-accent) 40%, transparent);
    transition: transform var(--l-fast), background var(--l-fast);

    &:hover:not(:disabled) { background: var(--accent-hover); transform: scale(1.05); }
    &:focus-visible { outline: 2px solid #fff; outline-offset: 3px; }
    &:disabled { opacity: 0.5; cursor: default; }
  }

  &__card {
    flex: 0 1 auto;
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(220px, 34%);
    grid-template-rows: minmax(0, 1fr);
    gap: 16px;
    min-height: 0;
    overflow: hidden;
    padding: 14px 16px;
    border: 1px solid var(--l-border);
    border-radius: var(--l-radius-sm);
    background: var(--l-modal-bar);

    &--empty {
      display: flex;
      align-items: center;
      justify-content: center;
      color: var(--l-text-faint);
      font-size: 13px;
    }
  }

  &__info {
    min-width: 0;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  &__info-head {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
  }

  &__info-title {
    margin: 0;
    overflow: hidden;
    color: var(--l-text);
    font-size: 16px;
    font-weight: 600;
    line-height: 22px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  // Рядом с «Экспортом» в узкой карточке сжимается метка, а не название.
  &__tag {
    flex: 0 1000 auto;
    min-width: 24px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    padding: 2px 8px;
    border-radius: 9px;
    background: rgba(255, 255, 255, 0.08);
    color: var(--l-text-dim);
    font-size: 11px;
    font-weight: 600;
  }

  &__facts {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 22px;
    margin: 0;
  }

  &__fact {
    min-width: 0;
    max-width: min(100%, 300px);
    display: flex;
    flex-direction: column;
    gap: 1px;

    dt {
      color: var(--l-text-faint);
      font-size: 11px;
      font-weight: 600;
      letter-spacing: 0.05em;
      text-transform: uppercase;
    }

    dd {
      margin: 0;
      overflow: hidden;
      color: var(--l-text);
      font-size: 13px;
      text-overflow: ellipsis;
      white-space: nowrap;
      font-variant-numeric: tabular-nums;
    }
  }

  &__range {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    color: var(--accent);
    font-size: 12.5px;
    font-variant-numeric: tabular-nums;

    span { color: var(--l-text); }
  }

  /* Строка интервала держит высоту и пустой (is-empty), в одну строку. */
  &__range {
    min-height: 20px;
    white-space: nowrap;

    &.is-empty { visibility: hidden; }
  }

  &__range &__range-note {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    color: var(--l-level-warning);
    text-overflow: ellipsis;
  }

  &__screen-cancelled {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 12px;
    border-radius: 12px;
    background: rgba(255, 255, 255, 0.07);
    color: var(--l-text);
    font-size: 12.5px;

    i { color: var(--l-text-dim); font-size: 11px; }
  }

  &__link {
    padding: 0;
    border: 0;
    background: none;
    color: var(--l-text-dim);
    font: inherit;
    text-decoration: underline;
    text-underline-offset: 2px;
    cursor: pointer;

    &:hover { color: var(--l-text); }
  }

  &__actions-note {
    color: var(--l-text-faint);
    font-size: 12px;
  }

  &__actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin-top: auto;
  }

  &__head-btn {
    flex: none;
    min-height: 32px;
    margin-left: auto;
    padding: 0 14px;
    font-variant-numeric: tabular-nums;
  }

  // Значки рядом с «Экспортом» — по его высоте.
  &__head-icon {
    flex: none;
    width: 32px;
    height: 32px;

    &:disabled { opacity: 0.45; }
  }

  /* Экспорт этой записи не удался (в том числе пока окно было закрыто). */
  &__failed {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    color: var(--l-level-error);
    font-size: 12.5px;
    line-height: 1.4;

    span { min-width: 0; overflow-wrap: anywhere; }
    .dmp__link { flex: none; }
  }

  &__item-busy {
    margin-right: 6px;
    color: var(--accent);
    font-size: 11px;
  }

  /* Высоту карточки задаёт левая часть; длинный список меток не
     раздувает её (contain: size), а прокручивается внутри. */
  &__side {
    contain: size;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding-left: 16px;
    border-left: 1px solid var(--l-border);
  }

  &__side-title {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    color: var(--l-text-dim);
    font-size: 11.5px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  &__side-empty {
    margin: 0;
    color: var(--l-text-faint);
    font-size: 12.5px;
  }

  &__rows {
    flex: 1;
    min-height: 0;
    margin: 0;
    padding: 0;
    list-style: none;
    overflow-y: auto;
    overscroll-behavior: contain;
    @include gx-scrollbar;
  }

  &__row {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    min-height: 28px;
    padding: 2px 4px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--l-text-dim);
    font: inherit;
    font-size: 12.5px;
    text-align: left;

    &--btn {
      cursor: pointer;
      &:hover { background: rgba(255, 255, 255, 0.06); color: var(--l-text); }
      &:focus-visible { outline: 2px solid var(--l-accent); outline-offset: -2px; }
    }
  }

  &__row-num {
    flex: none;
    min-width: 18px;
    color: var(--l-text-faint);
    font-variant-numeric: tabular-nums;
    text-align: right;
  }

  &__row-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    color: var(--l-text);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  &__row-time {
    flex: none;
    color: var(--l-text-faint);
    font-variant-numeric: tabular-nums;
  }

  &__row-dot {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 2px;
    background: var(--l-text-dim);

    &.is-map { background: var(--accent); }
    &.is-saveload { background: #ffb547; }
    &.is-timer { background: #5fd17f; }
    &.is-chapter { background: #b98cff; }
    &.is-finish { background: #ff6b6b; }
  }

  &__row-btn {
    flex: none;
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    padding: 0;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--l-text-dim);
    font-size: 11px;
    cursor: pointer;

    &:hover:not(:disabled) { background: rgba(255, 255, 255, 0.08); color: var(--l-text); }
    &:focus-visible { outline: 2px solid var(--l-accent); outline-offset: -2px; }
    &:disabled { opacity: 0.3; cursor: default; }
  }

  /* Зона перетаскивания поверх страницы. */
  &__drop {
    position: absolute;
    inset: calc(var(--l-topbar-h) + 8px) calc(var(--l-edge-pad) - 8px) calc(var(--l-edge-pad) - 8px) calc(var(--l-hero-pad) - 8px);
    z-index: 5;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 2px dashed var(--accent);
    border-radius: var(--l-radius);
    background: color-mix(in srgb, var(--l-bg) 92%, transparent);
    -webkit-backdrop-filter: blur(6px);
    backdrop-filter: blur(6px);
    pointer-events: none;
  }

  &__drop-box {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    color: var(--l-text-dim);
    font-size: 13.5px;

    i { color: var(--accent); font-size: 36px; margin-bottom: 6px; }
    b { color: var(--l-text); font-size: 18px; }
  }
}

.dmp-drop-enter-active,
.dmp-drop-leave-active { transition: opacity var(--l-fast); }
.dmp-drop-enter-from,
.dmp-drop-leave-to { opacity: 0; }

@media (max-height: 760px) {
  .dmp__card { min-height: 156px; padding-block: 8px; }
  .dmp__head { margin-bottom: 12px; }
  // Низкое окно (Full HD, 1280×720): шапка с «Экспортом» и три ряда фактов
  // (список карт — в подсказке) без прокрутки карточки.
  .dmp__head-btn { min-height: 30px; }
  .dmp__head-icon { width: 30px; height: 30px; }
  .dmp__info { gap: 6px; }
  .dmp__facts { gap: 4px 14px; }
  .dmp__fact { max-width: min(100%, 150px); }
  .dmp__fact dt { line-height: 13px; }
  .dmp__fact dd { line-height: 17px; }
}
</style>
