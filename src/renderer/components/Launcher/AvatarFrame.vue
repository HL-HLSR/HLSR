<template>
  <span class="avatar-frame" :class="{ 'is-framed': css, 'is-round': radius === '50%' }" :style="vars">
    <span v-if="css" class="avatar-frame__ring" :style="css" aria-hidden="true"></span>
    <slot></slot>
  </span>
</template>

<script>
// Рамка аватара, которую выдаёт админка (src/shared/avatar-frames.js).
// Слой рамки лежит позади аватара и выступает за него на width пикселей со
// всех сторон: всё, что админка нарисует в его середине, закрыто аватаром,
// а под полупрозрачной заглушкой без картинки — сплошной подложкой (hole).
// CSS рамки проверен сервером и main при разборе профиля, поэтому сюда
// доходят только безопасные объявления.
export default {
  name: "AvatarFrame",
  props: {
    frame: { type: Object, default: null },
    width: { type: Number, default: 3 },
    radius: { type: String, default: "50%" },
    hole: { type: String, default: "var(--l-modal-bg)" },
  },
  computed: {
    css() {
      return this.frame && this.frame.css ? this.frame.css : null;
    },
    vars() {
      return { "--frame-width": this.width + "px", "--frame-radius": this.radius, "--frame-hole": this.hole };
    },
  },
};
</script>

<style scoped>
.avatar-frame {
  position: relative;
  display: inline-flex;
  flex: none;
  isolation: isolate;
  border-radius: var(--frame-radius);
}

.avatar-frame__ring {
  position: absolute;
  inset: calc(-1 * var(--frame-width));
  z-index: -1;
  border-radius: calc(var(--frame-radius) + var(--frame-width));
  pointer-events: none;
}

/* Радиус в процентах (круг) с прибавкой пикселей перестаёт быть кругом. */
.avatar-frame.is-round .avatar-frame__ring,
.avatar-frame.is-round .avatar-frame__ring::after {
  border-radius: 50%;
}

.avatar-frame__ring::after {
  content: "";
  position: absolute;
  inset: var(--frame-width);
  border-radius: var(--frame-radius);
  background: var(--frame-hole);
}
</style>
