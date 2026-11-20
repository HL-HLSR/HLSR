<template>
  <div class="friends" role="dialog" aria-modal="false" :aria-label="$t('#L_FRIENDS_TITLE')" ref="root">
    <header class="friends__head">
      <h2 class="friends__title">{{ $t("#L_FRIENDS_TITLE") }}</h2>
      <span class="friends__count">{{ counterText }}</span>

      <button
        type="button"
        class="friends__close"
        :title="$t('#L_CLOSE')"
        :aria-label="$t('#L_CLOSE')"
        @click="$emit('close')"
      >
        <i class="fas fa-times" aria-hidden="true"></i>
      </button>
    </header>

    <!-- Поиск. При двух с половиной сотнях друзей список без него
         пролистывается, но не читается. -->
    <div v-if="friends.length > 8" class="friends__search">
      <i class="fas fa-search" aria-hidden="true"></i>
      <input
        type="search"
        v-model="query"
        class="friends__input"
        :placeholder="$t('#L_FRIENDS_SEARCH')"
        :aria-label="$t('#L_FRIENDS_SEARCH')"
      />
    </div>

    <div class="friends__body">
      <p v-if="loading" class="friends__note"><i class="fas fa-spinner fa-spin" aria-hidden="true"></i></p>
      <p v-else-if="!signedIn" class="friends__note">{{ $t("#L_SB_STEAM_SIGNED_OUT") }}</p>

      <p v-else-if="!friends.length" class="friends__note">{{ $t("#L_FRIENDS_EMPTY") }}</p>

      <template v-else>
        <section v-for="group in groups" :key="group.id" class="friends__group">
          <h3 class="friends__group-title">{{ $t(group.title) }} <span>{{ group.friends.length }}</span></h3>
          <ul class="friends__list">
            <li v-for="friend in group.friends" :key="friend.steamId64" class="friend">
              <button type="button" class="friend__open" @click="$emit('open-profile', friend.steamId64)">
                <span class="friend__avatar"><img v-if="friend.avatarDataUri" :src="friend.avatarDataUri" alt="" /><span v-else class="friend__letter" aria-hidden="true">{{ letterOf(friend) }}</span></span>
                <span class="friend__text"><span class="friend__name">{{ friend.personaName }}</span></span>
                <i v-if="friend.inHlsr" class="fas fa-check-circle friend__hlsr" :title="$t('#L_RUNNER_FRIENDS_HLSR')" aria-hidden="true"></i>
              </button>
            </li>
          </ul>
        </section>

        <p v-if="!visible.length" class="friends__note">{{ $t("#L_FRIENDS_NOT_FOUND") }}</p>
      </template>
    </div>
  </div>
</template>

<script>
const { ipcRenderer } = window.hlsrElectron;

export default {
  emits: ["close", "open-profile"],
  name: "FriendsPanel",
  data() {
    return {
      signedIn: false,
      loading: true,
      loadError: false,
      friends: [],
      query: "",
      lastFocused: null,
      destroyed: false,
    };
  },
  computed: {
    visible() {
      const needle = this.query.trim().toLowerCase();
      if (!needle) return this.friends;

      return this.friends.filter(
        (friend) =>
          friend.personaName.toLowerCase().indexOf(needle) !== -1 ||
          friend.steamId64.indexOf(needle) !== -1
      );
    },
    groups() {
      return [{ id: "hlsr", title: "#L_RUNNER_FRIENDS_HLSR", friends: this.visible.filter((friend) => friend.inHlsr) },
        { id: "steam", title: "#L_RUNNER_FRIENDS_STEAM", friends: this.visible.filter((friend) => !friend.inHlsr) }]
        .filter((group) => group.friends.length);
    },
    counterText() {
      return this.friends.length ? String(this.friends.length) : "";
    },
  },
  methods: {
    letterOf(friend) {
      const name = (friend.personaName || "?").trim();
      return name ? name[0].toUpperCase() : "?";
    },
    // Сначала список из памяти лаунчера (он готов сразу), затем свежий:
    // он тихо заменяет старый, уже загруженные аватары сохраняются.
    async load() {
      let data = null;

      try {
        data = await ipcRenderer.invoke("friends-list");
      } catch (error) { this.loadError = true; }
      this.loading = false;
      if (!data) return;
      this.apply(data);

      if (!data.cached || data.signedIn === false) return;
      let fresh = null;
      try {
        fresh = await ipcRenderer.invoke("friends-list", { force: true });
      } catch (error) { return; }
      if (fresh && !this.destroyed) this.apply(fresh);
    },
    apply(data) {
      const avatars = new Map(this.friends.filter((friend) => friend.avatarDataUri).map((friend) => [friend.steamId64, friend.avatarDataUri]));
      this.signedIn = data.signedIn !== false;
      this.friends = (data.friends || []).map((friend) => Object.assign({}, friend, { avatarDataUri: friend.avatarDataUri || avatars.get(friend.steamId64) || null }));

      this.loadAvatars();
    },
    async loadAvatars() {
      const SIZE = 40;
      const ids = this.friends.filter((friend) => !friend.avatarDataUri).map((friend) => friend.steamId64);

      for (let at = 0; at < ids.length; at += SIZE) {
        if (this.destroyed) return;

        let batch = null;
        try {
          batch = await ipcRenderer.invoke("friends-avatars", ids.slice(at, at + SIZE));
        } catch (error) {
          return;
        }

        if (!batch) continue;

        for (const friend of this.friends) {
          const found = batch[friend.steamId64];
          if (found) friend.avatarDataUri = found;
        }
      }
    },
    onKeydown(e) {
      if (e.key !== "Escape") return;
      e.preventDefault();
      this.$emit("close");
    },
  },
  mounted() {
    this.lastFocused = document.activeElement;
    document.addEventListener("keydown", this.onKeydown);

    this.load();
  },
  beforeUnmount() {
    this.destroyed = true;
    document.removeEventListener("keydown", this.onKeydown);
    if (this.lastFocused && this.lastFocused.focus) this.lastFocused.focus();
  },
};
</script>

<style lang="scss" scoped>
.friends__group-title { font-size: 11px; font-weight: 600; color: var(--l-text-dim); padding: 8px 12px; margin: 5px 0; }
.friends__group-title span { margin-left: 7px; color: var(--l-text-faint); font-variant-numeric: tabular-nums; }
.friend__open { display: flex; align-items: center; gap: 12px; width: 100%; min-width: 0; background: none; border: 0; padding: 0; color: inherit; text-align: left; font: inherit; cursor: pointer; }
.friend__open:focus-visible { outline: 2px solid var(--accent); outline-offset: 4px; }
.friend__hlsr { color: var(--accent); font-size: 12px; margin-left: auto; }

/* Панель опускается из правого верхнего угла, под меню аккаунта: оттуда
   её и открывают. Модальным затемнением не закрывается — это список, а не
   диалог, и лаунчер за ним остаётся рабочим. */
.friends {
  position: absolute;
  right: calc(var(--l-edge-pad) + var(--l-header-control) * 2 + 24px);
  top: calc((var(--l-topbar-h) + var(--l-header-control)) / 2 + 8px);
  z-index: 60;
  width: min(340px, calc(100vw - 32px));
  max-height: min(560px, calc(100vh - var(--l-topbar-h) - 40px));
  display: flex;
  flex-direction: column;
  background: var(--l-modal-bg);
  border: none;
  border-radius: var(--l-radius);
  box-shadow: inset 0 0 0 1px var(--l-modal-border), 0 20px 60px rgba(0, 0, 0, 0.55);
  overflow: hidden;

  &__head {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 16px;
    box-shadow: inset 0 -1px 0 var(--l-border);
  }

  &__title {
    margin: 0;
    color: var(--l-text);
    font-size: 12px;
    font-weight: 700;
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }

  &__count {
    flex: 1 1 auto;
    color: var(--l-text-faint);
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
  }

  &__close {
    flex: none;
    width: 40px;
    height: 40px;
    padding: 0;
    line-height: 24px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 0;
    border-radius: var(--l-radius-sm);
    background: none;
    color: var(--l-text-faint);
    cursor: pointer;

    &:hover {
      background: rgba(255, 255, 255, 0.06);
      color: var(--l-text);
    }

    &:focus-visible {
      outline: 2px solid var(--l-accent, #00abff);
      outline-offset: -2px;
    }
  }

  &__search {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 16px;
    border-bottom: 1px solid var(--l-border);
    color: var(--l-text-faint);

    i {
      font-size: 11px;
    }
  }

  &__input {
    flex: 1 1 auto;
    min-height: 40px;
    border: 0;
    background: none;
    color: var(--l-text);
    font: inherit;
    font-size: 13px;

    &::placeholder {
      color: var(--l-text-faint);
    }

    &:focus {
      outline: none;
    }

    &:focus-visible {
      outline: none;
    }
  }

  &__body {
    flex: 1 1 auto;
    overflow-y: auto;
    padding: 8px;
  }

  &__note {
    margin: 12px 8px;
    color: var(--l-text-faint);
    font-size: 12.5px;
    line-height: 1.5;
  }

  &__list {
    list-style: none;
    margin: 0;
    padding: 0;
  }
}

/* Строка друга. Именно строка, не кнопка: открывать нечего. */
.friend {
  width: 100%;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px;
  border: 1px solid transparent;
  border-radius: var(--l-radius-sm);
  background: none;
  color: inherit;
  font: inherit;
  text-align: left;

  &__avatar {
    flex: none;
    width: 32px;
    height: 32px;
    border-radius: 8px;
    overflow: hidden;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(255, 255, 255, 0.06);

    img {
      width: 100%;
      height: 100%;
      object-fit: cover;
      display: block;
    }
  }

  /* Буква вместо аватара. В кеше Steam лежат только недавно виденные, и
     пустой квадрат у большинства выглядел бы поломкой. */
  &__letter {
    color: var(--l-text-faint);
    font-size: 14px;
    font-weight: 700;
  }

  &__text {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  &__name {
    color: var(--l-text-dim);
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

}

/* Приглушения больше нет. Раньше строки не-в-HLSR шли с opacity 0.62,
   чтобы отличаться от играющих; теперь групп нет, и приглушённым
   оказался бы весь список целиком — то есть просто хуже читаемый. */
.friends__list li + li.friend {
  margin-top: 2px;
}
</style>
