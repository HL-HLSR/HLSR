<template>
  <div class="runner-search" ref="root">
    <!-- Лупа своим SVG, а не глифом шрифта: линии одной толщины с
         рамкой поля и ровно по центру на любом масштабе. -->
    <svg class="runner-search__icon" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
      <circle cx="7" cy="7" r="4.75" />
      <path d="M10.5 10.5 14 14" />
    </svg>
    <input v-model="query" type="search" maxlength="64" role="combobox" :aria-expanded="open ? 'true' : 'false'" aria-controls="runner-search-results" :aria-label="$t('#L_RUNNER_SEARCH')" :placeholder="$t('#L_RUNNER_SEARCH')" :title="$t(signedIn ? '#L_RUNNER_SEARCH' : '#L_RUNNER_SEARCH_GUEST')" :disabled="!signedIn" @input="schedule" @focus="open = query.trim().length >= 2" @keydown.esc.stop="open = false" @keydown.down.prevent="move(1)" @keydown.up.prevent="move(-1)" @keydown.enter.prevent="choose(results[selected])" />
    <!-- Своя кнопка очистки вместо системной: стоит зеркально лупе, и
         отступы текста слева и справа одинаковые. -->
    <button v-if="query" type="button" class="runner-search__clear" :aria-label="$t('#L_RUNNER_SEARCH_CLEAR')" :title="$t('#L_RUNNER_SEARCH_CLEAR')" @click="clear"><i class="fas fa-times" aria-hidden="true"></i></button>
    <transition name="l-pop">
    <div v-if="open" class="runner-search__results" id="runner-search-results" role="listbox">
      <p v-if="loading"><i class="fas fa-spinner fa-spin" aria-hidden="true"></i></p>
      <p v-else-if="error" role="alert">{{ $t('#L_RUNNER_NETWORK') }}</p>
      <p v-else-if="!results.length">{{ $t('#L_RUNNER_SEARCH_EMPTY') }}</p>
      <button v-for="(profile, index) in results" :key="profile.steamId64" type="button" role="option" :aria-selected="selected === index ? 'true' : 'false'" :class="{ 'is-selected': index === selected }" @click="choose(profile)"><AvatarFrame :frame="profile.avatarFrame" :width="3" radius="var(--l-radius-sm)" hole="var(--l-modal-bg)"><span class="runner-search__avatar" aria-hidden="true"><img v-if="avatarOf(profile)" :src="avatarOf(profile)" alt="" @error="avatarFailed(profile)" /><b v-else>{{ letterOf(profile) }}</b></span></AvatarFrame><span class="runner-search__text"><span><NickName :nick="profile.nick" :subscriber="profile.subscriber === true">{{ profile.personaName || profile.steamId64 }}</NickName></span><small>{{ idOf(profile) }}</small></span></button>
    </div>
    </transition>
  </div>
</template>

<script>
import NickName from "./NickName";
import AvatarFrame from "./AvatarFrame";
const { ipcRenderer } = window.hlsrElectron;
export default {
  components: { NickName, AvatarFrame },
  emits: ["open-profile"],
  name: 'RunnerSearch',
  data() { return { query: '', results: [], signedIn: false, open: false, loading: false, error: false, timer: null, requestId: 0, selected: 0, avatars: {} }; },
  methods: {
    schedule() {
      if (this.timer) clearTimeout(this.timer);
      this.requestId++;
      if (this.query.trim().length < 2) { this.open = false; this.results = []; return; }
      this.open = true; this.loading = true; this.error = false;
      const requestId = this.requestId;
      this.timer = setTimeout(() => this.search(requestId), 350);
    },
    async search(requestId) {
      try {
        const result = await ipcRenderer.invoke('runner-search', this.query.trim());
        if (requestId !== this.requestId) return;
        this.error = !Array.isArray(result);
        this.results = Array.isArray(result) ? result : [];
        this.selected = 0;
        this.loadAvatars(requestId);
      } catch (error) { if (requestId === this.requestId) this.error = true; }
      finally { if (requestId === this.requestId) this.loading = false; }
    },
    // Аватар приходит с сервера (details.avatarUrl, сохраняется при входе
    // через Steam). У кого его нет или ссылка не открылась, берётся из
    // кеша Steam на этом компьютере, как в списке друзей; иначе — буква.
    avatarOf(profile) {
      const own = this.avatars[profile.steamId64];
      if (own !== undefined) return own;
      return (profile.details && profile.details.avatarUrl) || null;
    },
    avatarFailed(profile) {
      if (this.avatars[profile.steamId64] === undefined) this.avatars = { ...this.avatars, [profile.steamId64]: null };
      this.loadAvatars(this.requestId, [profile.steamId64]);
    },
    async loadAvatars(requestId, only) {
      const ids = only || this.results.filter((profile) => !this.avatarOf(profile)).map((profile) => profile.steamId64);
      if (!ids.length) return;
      let found = null;
      try { found = await ipcRenderer.invoke('friends-avatars', ids); } catch (error) { return; }
      if (requestId !== this.requestId || !found) return;
      const next = { ...this.avatars };
      for (const id of ids) if (found[id]) next[id] = found[id];
      this.avatars = next;
    },
    // Кто скрыл SteamID в настройках профиля, тому его не показывают и в поиске.
    idOf(profile) { return profile.details && profile.details.hideSteamId === true ? this.$t('#L_RUNNER_ID_HIDDEN') : profile.steamId64; },
    letterOf(profile) { return Array.from((profile.personaName || '?').trim() || '?')[0].toUpperCase(); },
    clear() { this.query = ''; this.schedule(); this.$el.querySelector('input').focus(); },
    move(step) { if (this.results.length) this.selected = (this.selected + step + this.results.length) % this.results.length; },
    choose(profile) { if (!profile) return; this.open = false; this.$emit('open-profile', profile.steamId64); },
    onProfile(event, data) { this.signedIn = !!(data && data.account && data.account.signedIn); if (!this.signedIn) { this.query = ''; this.open = false; this.results = []; this.requestId++; } },
    outside(event) { if (this.$refs.root && !this.$refs.root.contains(event.target)) this.open = false; },
  },
  mounted() { ipcRenderer.on('runner-profile-state', this.onProfile); ipcRenderer.invoke('runner-state').then((data) => this.onProfile(null, data)); document.addEventListener('mousedown', this.outside); },
  beforeUnmount() { if (this.timer) clearTimeout(this.timer); this.requestId++; ipcRenderer.removeListener('runner-profile-state', this.onProfile); document.removeEventListener('mousedown', this.outside); },
};
</script>

<style scoped>
/* Свой слой: поле меняет ширину вместе с колонкой, и сглаженный угол
   рамки иначе оставлял на шапке не стёртый пиксель. */
.runner-search { position: relative; width: 100%; -webkit-app-region: no-drag; will-change: transform; }
.runner-search__icon { position: absolute; z-index: 1; left: 11px; top: 50%; width: 14px; height: 14px; transform: translateY(-50%); fill: none; stroke: var(--l-text-faint); stroke-width: 1.6; stroke-linecap: round; pointer-events: none; transition: stroke var(--l-fast); }
.runner-search:focus-within .runner-search__icon { stroke: var(--accent); }
.runner-search input { box-sizing: border-box; width: 100%; height: var(--l-header-control); padding: 0 32px; color: var(--l-text); background: rgba(255,255,255,.04); border: 0; box-shadow: inset 0 0 0 1px var(--l-border); border-radius: var(--l-radius-sm); font-family: inherit; font-size: 12.5px; line-height: 20px; transition: background var(--l-fast); }
.runner-search input::-webkit-search-cancel-button { -webkit-appearance: none; appearance: none; }
.runner-search__clear { position: absolute; right: 6px; top: 50%; display: grid; place-items: center; width: 24px; height: 24px; padding: 0; transform: translateY(-50%); color: var(--l-text-faint); background: none; border: 0; border-radius: var(--l-radius-sm); font-size: 12px; cursor: pointer; transition: color var(--l-fast), background var(--l-fast); }
.runner-search__clear:hover { color: var(--l-text); background: rgba(255,255,255,.06); }
.runner-search__clear:focus-visible { outline: 2px solid var(--accent); outline-offset: 0; }
.runner-search input::placeholder { color: var(--l-text-faint); }
/* Справа 32 px держатся под крестик очистки; пока поле пустое, крестика
   нет, и подсказка не обрезается в узком поле. */
.runner-search input:placeholder-shown { padding-right: 12px; }
.runner-search input:hover { background: rgba(255,255,255,.06); }
.runner-search input:focus { outline: 2px solid var(--accent); outline-offset: 2px; }
.runner-search input:disabled { opacity: .6; }
.runner-search__results { position: absolute; left: 0; right: 0; top: calc(var(--l-header-control) + 8px); max-height: 352px; overflow-y: auto; padding: 8px; border: 0; border-radius: var(--l-radius); background: var(--l-modal-bg); box-shadow: inset 0 0 0 1px var(--l-modal-border), 0 20px 60px rgba(0,0,0,.55); z-index: 80; }
.runner-search__results button { display: flex; align-items: center; gap: 12px; width: 100%; min-height: 48px; padding: 8px 12px 8px 8px; text-align: left; color: var(--l-text); background: none; border: 0; border-radius: var(--l-radius-sm); font-family: inherit; font-size: 13px; line-height: 20px; cursor: pointer; }
/* Аватар — как в списке друзей: квадрат 40 px со скруглением. */
.runner-search__avatar { flex: none; display: grid; place-items: center; width: 40px; height: 40px; border-radius: var(--l-radius-sm); overflow: hidden; background: rgba(255,255,255,.06); box-shadow: inset 0 0 0 1px var(--l-border); }
.runner-search__avatar img { display: block; width: 100%; height: 100%; object-fit: cover; }
.runner-search__avatar b { color: var(--l-text-faint); font-size: 15px; font-weight: 700; line-height: 1; }
.runner-search__text { display: flex; flex-direction: column; gap: 2px; flex: 1 1 auto; min-width: 0; }
/* Свечение значка подписчика и оформленного ника выходит за буквы, а
   overflow ради многоточия срезал бы его: поля под свечение съедаются
   отрицательным отступом, раскладка не меняется. */
.runner-search__text > span { margin: -4px -6px; padding: 4px 6px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: calc(100% + 12px); }
.runner-search__results button:hover, .runner-search__results button.is-selected { background: rgba(255,255,255,.06); }
.runner-search__results button:focus-visible { outline: 2px solid var(--accent); }
.runner-search__results small { color: var(--l-text-faint); font-size: 11.5px; line-height: 16px; }
.runner-search__results p { color: var(--l-text-faint); font-size: 12.5px; line-height: 20px; padding: 4px 8px; }

</style>
