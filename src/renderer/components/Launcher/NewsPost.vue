<template>
  <article class="news-post">
    <div class="news-post__meta">
      <button v-if="profile" type="button" class="news-post__author is-link" :title="$t('#L_HOME_AUTHOR_OPEN')" @click="$emit('author', profile.steamId64)">
        <AvatarFrame :frame="profile.avatarFrame" :width="2" hole="var(--l-surface-solid)"><span class="news-post__avatar" aria-hidden="true"><b>{{ letter }}</b><img v-if="avatar" :src="avatar" alt="" referrerpolicy="no-referrer" @error="avatarBroken = true" /></span></AvatarFrame>
        <NickName :nick="profile.nick" :subscriber="profile.subscriber === true">{{ authorName }}</NickName>
      </button>
      <span v-else class="news-post__author">{{ authorName }}</span>
      <span aria-hidden="true">·</span>
      <time :datetime="post.date">{{ date }}</time>
    </div>
    <h2 class="news-post__title">{{ post.title }}</h2>
    <div class="news-post__body" @click.prevent="onLink" v-html="body"></div>
    <div v-if="post.images.length" class="news-post__images" :class="{ 'is-single': post.images.length === 1 }">
      <button v-for="tile in tiles" :key="tile.image.url" type="button" :class="{ 'is-wide': tile.wide }" :style="{ aspectRatio: tile.aspect }" :aria-label="$t('#L_HOME_IMAGE_OPEN') + ' ' + (tile.index + 1)" @click="$emit('image', post, tile.index)">
        <img :src="tile.image.url" :width="tile.image.width" :height="tile.image.height" :style="tile.style" :alt="post.title + ' · ' + (tile.index + 1)" loading="lazy" decoding="async" referrerpolicy="no-referrer" @load="$emit('resize')" />
      </button>
    </div>
    <div class="news-post__reactions">
      <button v-for="(emoji, key) in reactions" :key="key" type="button" class="news-reaction" :class="{ 'is-selected': mine.includes(key) }" :aria-pressed="mine.includes(key) ? 'true' : 'false'" :aria-label="$t('#L_HOME_REACTION_' + key) + ' · ' + post.reactions[key]" :title="$t('#L_HOME_REACTION_' + key)" :disabled="pending" @click="$emit('react', post.id, key)">
        <span aria-hidden="true">{{ emoji }}</span><span>{{ post.reactions[key] }}</span>
      </button>
    </div>
    <time class="news-post__stamp" :datetime="post.date">{{ stamp }}</time>
    <p v-if="notice" class="news-post__notice" role="status">
      {{ $t(notice) }}
      <button v-if="notice === '#L_HOME_REACT_SIGN_IN'" type="button" @click="$emit('sign-in')">{{ $t('#L_HOME_SIGN_IN') }}</button>
    </p>
  </article>
</template>

<script>
const { render } = require('../../../shared/news-markdown');
const { REACTIONS } = require('../../../shared/news-contract');
const { tileAspect, tileImageStyle } = require('../../../shared/news-image-layout');
import NickName from './NickName';
import AvatarFrame from './AvatarFrame';
// Автор с профилем HLSR — кнопка с аватаром и оформленным ником, открывает
// профиль. Без профиля (убранный администратор, профиль удалён) — имя.
export default {
  name: 'NewsPost',
  components: { NickName, AvatarFrame },
  emits: ['image', 'react', 'sign-in', 'link', 'resize', 'author'],
  props: { post: { type: Object, required: true }, date: String, stamp: String, mine: { type: Array, default: () => [] }, pending: Boolean, notice: String },
  data: () => ({ reactions: REACTIONS, avatarBroken: false }),
  computed: {
    body() { return render(this.post.body); },
    profile() { return this.post.authorProfile || null; },
    authorName() { return (this.profile && this.profile.personaName) || this.post.author || this.$t('#L_HOME_AUTHOR_FALLBACK'); },
    avatar() { return !this.avatarBroken && this.profile && this.profile.avatarUrl ? this.profile.avatarUrl : null; },
    letter() { return Array.from(this.authorName.trim())[0] || '?'; },
    tiles() {
      const count = this.post.images.length;
      return this.post.images.map((image, index) => {
        const aspect = tileAspect(image, index, count);
        return { image, index, aspect: String(aspect), style: tileImageStyle(image, aspect), wide: count > 1 && count % 2 === 1 && index === 0 };
      });
    },
  },
  watch: { 'post.authorProfile'() { this.avatarBroken = false; } },
  methods: {
    onLink(event) {
      const link = event.target.closest('a[data-href]');
      if (link && event.currentTarget.contains(link)) this.$emit('link', link.dataset.href);
    },
  },
};
</script>

<style lang="scss" scoped>
.news-post {
  padding: 24px 28px;
  border-radius: var(--l-radius);
  background: var(--l-surface-solid);
  box-shadow: inset 0 0 0 1px var(--l-border);
  &__meta { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; color: var(--l-text-faint); font-size: 12px; line-height: 18px; }
  &__author { color: var(--l-text-dim); font-weight: 600; overflow-wrap: anywhere; }
  &__author.is-link { display: inline-flex; align-items: center; gap: 8px; min-width: 0; margin: -2px 0; padding: 2px 8px 2px 2px; border: 0; border-radius: 999px; background: none; font: inherit; font-weight: 600; text-align: left; cursor: pointer; transition: background var(--l-fast), color var(--l-fast); }
  &__author.is-link:hover { color: var(--l-text); background: rgba(255,255,255,.06); }
  &__author.is-link:focus-visible { outline: none; box-shadow: inset 0 0 0 1px var(--l-accent); }
  /* Буква лежит под картинкой: пока аватар грузится или если он битый,
     видна она, а не пустой кружок. */
  &__avatar { position: relative; flex: none; display: grid; place-items: center; width: 20px; height: 20px; border-radius: 50%; overflow: hidden; background: rgba(255,255,255,.08); box-shadow: inset 0 0 0 1px var(--l-border); font-size: 10px; line-height: 1; color: var(--l-text); }
  &__avatar img { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: cover; }
  &__title { margin: 8px 0 16px; color: var(--l-text); font-size: 22px; line-height: 30px; overflow-wrap: anywhere; }
  &__body { max-width: 90ch; color: var(--l-text-dim); font-size: 14px; line-height: 23px; overflow-wrap: anywhere;
    :deep(p) { margin: 12px 0; }
    :deep(h3), :deep(h4), :deep(h5), :deep(h6) { margin: 20px 0 8px; color: var(--l-text); line-height: 1.4; }
    :deep(h3) { font-size: 18px; }
    :deep(h4) { font-size: 16px; }
    :deep(strong) { color: var(--l-text); }
    :deep(a) { color: var(--l-accent); text-decoration: underline; text-underline-offset: 3px; }
    :deep(ul), :deep(ol) { padding-left: 24px; margin: 12px 0; }
    :deep(code) { padding: 2px 5px; border-radius: 4px; background: rgba(255,255,255,.07); color: var(--l-text); font-family: monospace; font-size: .93em; }
    :deep(pre) { padding: 12px 16px; overflow-x: auto; border-radius: 6px; background: rgba(0,0,0,.2); }
    :deep(pre code) { padding: 0; background: none; }
  }
  &__images { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 8px; max-width: 520px; margin-top: 16px;
    &.is-single { grid-template-columns: minmax(0, 1fr); max-width: 480px; }
    button { position: relative; padding: 0; overflow: hidden; border: 1px solid var(--l-border); border-radius: 8px; background: #101316; cursor: zoom-in;
      &.is-wide { grid-column: 1 / -1; }
    }
    img { position: absolute; display: block; max-width: none; transition: opacity var(--l-fast); }
    button:hover img { opacity: .85; }
  }
  &__reactions { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 20px; }
  &__stamp { display: block; margin-top: 12px; color: var(--l-text); opacity: 0.38; font-size: 11.5px; line-height: 16px; font-variant-numeric: tabular-nums; }
  &__notice { margin: 12px 0 0; color: var(--l-text-dim); font-size: 12px; line-height: 20px;
    button { margin-left: 8px; padding: 0; border: 0; background: none; color: var(--l-accent); font: inherit; text-decoration: underline; cursor: pointer; }
  }
  button:focus-visible, :deep(a:focus-visible) { outline: 2px solid var(--l-accent); outline-offset: 3px; }
}
.news-reaction { display: inline-flex; align-items: center; gap: 7px; min-width: 54px; min-height: 30px; padding: 3px 10px; border: 1px solid var(--l-border); border-radius: 16px; background: transparent; color: var(--l-text-dim); font: inherit; font-size: 12px; font-variant-numeric: tabular-nums; cursor: pointer; transition: background var(--l-fast), border-color var(--l-fast);
  &:hover { border-color: var(--l-border-strong); background: rgba(255,255,255,.04); }
  &.is-selected { border-color: var(--l-accent); background: color-mix(in srgb, var(--l-accent) 16%, transparent); color: var(--l-text); }
  &:disabled { cursor: wait; opacity: .65; }
}
@media(max-width: 1000px) { .news-post { padding: 20px; } }
</style>
