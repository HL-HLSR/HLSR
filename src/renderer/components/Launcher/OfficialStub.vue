<template>
  <div class="official-stub" :class="{ 'is-compact': compact }" role="note">
    <span class="official-stub__text">{{ $t(text) }}</span>
    <button type="button" class="official-stub__link" @click="openReleases">{{ $t("#L_OFFICIAL_RELEASES") }}</button>
  </div>
</template>

<script>
// Заглушка самосборки вместо «Главной» и входа через Steam: функция есть только в
// официальном лаунчере HLSR, ссылка ведёт на страницу выпусков (ключ github в src/shared/links.js).
export default {
  name: "OfficialStub",
  props: {
    text: { type: String, required: true },
    compact: { type: Boolean, default: false },
  },
  methods: {
    openReleases() {
      window.hlsrElectron.ipcRenderer.invoke("open-external-link", "github").catch(() => {});
    },
  },
};
</script>

<style scoped>
.official-stub {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  flex: 1;
  padding: 24px;
  text-align: center;
  color: rgba(255, 255, 255, 0.72);
  font-size: 14px;
}
.official-stub.is-compact {
  flex: 0 1 auto;
  flex-direction: row;
  gap: 6px;
  padding: 0 8px;
  font-size: 12px;
  white-space: nowrap;
}
.official-stub__link {
  border: 0;
  padding: 0;
  background: none;
  color: inherit;
  font: inherit;
  text-decoration: underline;
  cursor: pointer;
}
.official-stub__link:hover,
.official-stub__link:focus-visible {
  color: #fff;
}
</style>
