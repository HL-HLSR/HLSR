<template>
  <div id="app" :class="{ 'launcher-mode': isLauncher }">
    <div id="app__container">
      <router-view v-slot="{ Component, route }">
        <transition name="slide">
          <component :is="Component" id="router" :key="route.fullPath" />
        </transition>
      </router-view>
    </div>
  </div>
</template>

<script>
const { ipcRenderer } = window.hlsrElectron;

export default {
  name: "hlsr",
  computed: {
    isLauncher() {
      return this.$route.name === "launcher";
    },
  },
  mounted() {
    ipcRenderer.send("ready");
  },
};
</script>

<style>
* {
  margin: 0;
}

/* Окно прозрачное: фон отдаёт #app */
html,
body {
  background: transparent !important;
}

html {
  color-scheme: dark;
}

body {
  font-family: Rubik, "Noto Sans", "DejaVu Sans", "Segoe UI", sans-serif;
  user-select: none;
  display: flex;
}

#app {
  --window-radius: 16px;
  display: flex;
  flex-direction: column;
  width: 100vw;
  height: 100vh;
  overflow: hidden;
  position: relative;
  background: #0b0e13;
  box-sizing: border-box;
  border-radius: var(--window-radius);
  clip-path: inset(0 round var(--window-radius));
  isolation: isolate;
}

/* Переход в полный экран и обратно (WindowControls.vue): окно гаснет, меняет
   размер невидимым и проявляется уже в новом. Плавно тянуть сам размер
   прозрачного окна на Windows нельзя: каждый кадр — перерисовка всего UI. */
#app {
  transition: opacity 0.24s cubic-bezier(0.22, 1, 0.36, 1), transform 0.24s cubic-bezier(0.22, 1, 0.36, 1);
}

html.window-switching #app {
  opacity: 0;
  transform: scale(0.985);
  transition-duration: 0.13s;
  transition-timing-function: cubic-bezier(0.55, 0, 0.75, 0.2);
}

@media (prefers-reduced-motion: reduce) {
  #app {
    transition: none;
  }
}

/* Полный экран (кнопка в шапке, WindowControls.vue): углы и контур окна
   на краю экрана не нужны — в скруглённых углах просвечивал рабочий стол. */
html.window-fullscreen #app {
  --window-radius: 0px;
}

html.window-fullscreen #app::after {
  display: none;
}

/* Контур и обрезка принадлежат одному слою, вне масштабируемого UI. */
#app::after {
  content: "";
  position: absolute;
  inset: 0;
  border-radius: inherit;
  box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.12);
  pointer-events: none;
  z-index: 100000;
}

#app__container {
  display: flex;
  flex: 1;
  max-height: 100vh;
  position: relative;
  z-index: 1;
}

#app__container::-webkit-scrollbar {
  visibility: hidden;
  width: 0;
}

#router {
  position: absolute;
  inset: 0;
  display: flex;
}

/* Оба экрана занимают одну рамку и перекрываются во время перехода. */
.slide-leave-active,
.slide-enter-active {
  will-change: transform, opacity;
}

/* Экраны лежат вне .launcher-root, где объявлены токены движения, поэтому
   значения повторены: --l-dur-fast + --l-ease-in на уход, --l-dur-slow +
   --l-ease на появление (launcher.css). */
.slide-leave-active {
  transition: transform 0.16s cubic-bezier(0.55, 0, 0.75, 0.2), opacity 0.16s cubic-bezier(0.55, 0, 0.75, 0.2);
  pointer-events: none;
}

.slide-enter-active {
  transition: transform 0.42s cubic-bezier(0.22, 1, 0.36, 1), opacity 0.26s cubic-bezier(0.22, 1, 0.36, 1);
}

.slide-enter-from {
  transform: translate3d(0, 20px, 0);
  opacity: 0;
  pointer-events: none;
}

.slide-leave-to {
  transform: translate3d(0, -12px, 0);
  opacity: 0;
}

@media (prefers-reduced-motion: reduce) {
  .slide-enter-active, .slide-leave-active {
    transition: opacity 0.01ms;
  }
  .slide-enter-from, .slide-leave-to { transform: none; }
}

::-webkit-scrollbar {
  width: 4px;
  height: 4px;
}

::-webkit-scrollbar-track {
  background: rgba(255, 255, 255, 0.05);
  border-radius: 8px;
}

::-webkit-scrollbar-thumb {
  background: rgba(255, 255, 255, 0.1);
  transition: 100ms;
  border-radius: 8px;
}

::-webkit-scrollbar-thumb:hover {
  background: rgba(255, 255, 255, 0.2);
}

/* Угол между вертикальной и горизонтальной полосой (длинные строки в
   редакторе cfg): по умолчанию Chromium красит его белым. */
::-webkit-scrollbar-corner {
  background: transparent;
}

.hide {
  opacity: 0 !important;
}

*:focus {
  outline: none;
}
</style>
