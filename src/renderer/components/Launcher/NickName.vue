<template>
  <span v-if="subscriber" class="nick-line"><img
      class="nick-badge"
      :src="badge"
      :alt="$t('#L_NICK_SUBSCRIBER')"
      :title="badgeTitle"
      draggable="false"
    /><span class="nick" :class="presetClass" :style="customCss"><slot></slot></span></span>
  <span v-else class="nick" :class="presetClass" :style="customCss"><slot></slot></span>
</template>

<script>
// Ник с оформлением, которое выдаёт админка (src/shared/nick-styles.js):
// готовый стиль — класс .nick--<id> из launcher.css, свой CSS — style.
// Свой CSS проверен дважды: сервером при выдаче и main при разборе
// профиля, поэтому сюда доходят только безопасные объявления.
//
// subscriber — подписка активна сейчас (поле subscriber карточки профиля):
// слева от ника значок подписчика (.nick-badge в launcher.css). Значок
// стоит вне .nick, иначе оформление ника (градиент по буквам, прозрачный
// цвет, filter, анимации) легло бы и на картинку. Классы и атрибуты
// родителя тогда достаются обёртке .nick-line; ник в ней строчный, поэтому
// многоточие длинного ника работает как раньше, а значок в начале строки
// не обрезается. Без подписки разметка та же, что была: один .nick.
//
// supporterSince (unix-секунды) известен только в профиле: там в подсказке
// «Поддерживает HLSR N мес. — с датой»; в списках остаётся «Подписчик HLSR».
const BADGE = require("@/assets/launcher/subscriber-badge.png");
const { supportMonths } = require("../../../shared/support-months");

export default {
  name: "NickName",
  props: {
    nick: { type: Object, default: null },
    subscriber: { type: Boolean, default: false },
    supporterSince: { type: Number, default: null },
  },
  computed: {
    presetClass() {
      return this.nick && this.nick.style ? "nick--" + this.nick.style : null;
    },
    customCss() {
      return this.nick && this.nick.css ? this.nick.css : null;
    },
    badge() {
      return BADGE;
    },
    badgeTitle() {
      if (!this.supporterSince) return this.$t("#L_NICK_SUBSCRIBER");
      // День игрока, как у даты подписки в настройках: ДД.ММ.ГГГГ.
      const date = new Date(this.supporterSince * 1000);
      const pad = (n) => String(n).padStart(2, "0");
      const since = `${pad(date.getDate())}.${pad(date.getMonth() + 1)}.${date.getFullYear()}`;
      const months = supportMonths(this.supporterSince, Date.now());
      return months > 0 ? this.$t("#L_NICK_SUPPORTER_MONTHS", months, since) : this.$t("#L_NICK_SUPPORTER_NEW", "", since);
    },
  },
};
</script>
