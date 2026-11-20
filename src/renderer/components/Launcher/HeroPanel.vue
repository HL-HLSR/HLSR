<template>

  <section class="hero">
    <!-- «HALF-LIFE» общий у всех игр и при смене игры стоит на месте; на
         месте же перетекают окончание названия, подзаголовок и значения
         статистики (переход l-value, старое и новое в одной ячейке сетки,
         подписи и значки статистики не меняются). Строка
         подзаголовка есть всегда, иначе у игр без него статистика
         подпрыгивала бы вверх. -->
    <div class="hero__content">
      <h1 class="hero__title" :aria-label="game.heroSubtitle ? game.heroTitle + ': ' + game.heroSubtitle : game.heroTitle">
        <span class="hero__title-line" aria-hidden="true">
          <span>{{ title.base }}</span>
          <span class="hero__swap">
            <transition name="l-value">
              <span :key="title.rest" class="hero__title-rest">{{ title.rest }}</span>
            </transition>
          </span>
        </span>
        <span class="hero__swap hero__subtitle-slot" aria-hidden="true">
          <transition name="l-value">
            <span :key="game.heroSubtitle || ''" class="hero__subtitle">{{ game.heroSubtitle || "" }}</span>
          </transition>
        </span>
      </h1>
      <GameStats
        class="hero__stats"
        :game="game"
        :stats="stats"
        :delivery="delivery"
      />
    </div>

  </section>
</template>

<script>
import GameStats from "./GameStats";

export default {
  name: "HeroPanel",
  components: { GameStats },
  computed: {
    // Общая часть серии отдельно от окончания: «Half-Life» + « 2».
    title() {
      const match = /^(Half-Life)(.*)$/i.exec(this.game.heroTitle || "");
      return match ? { base: match[1], rest: match[2] } : { base: this.game.heroTitle || "", rest: "" };
    },
  },
  props: {
    game: {
      type: Object,
      required: true,
    },
    stats: {
      type: Object,
      default: null,
    },
    delivery: {
      type: Object,
      default: null,
    },
  },
};
</script>

<style lang="scss" scoped>
.hero {
  position: relative;
  flex: 1;
  overflow: hidden;

  /* Собственной подложки читаемости у Hero НЕТ.
     Она начиналась от левой границы Hero, то есть ровно от края sidebar,
     и накладывалась на левый градиент общего слоя читаемости. На стыке
     возникала тёмная вертикальная полоса, которая читалась как вторая
     копия кадра слева. Затемнение делает один слой в LauncherBackground,
     отсчитываемый от края окна. */

  /* Верх и отступы — ступени сетки (launcher.css), а не 33 % высоты:
     процент давал дробную координату почти при любом размере окна */
  &__content {
    position: absolute;
    left: var(--l-hero-pad);
    top: var(--l-hero-top);
    bottom: 0;
    display: flex;
    flex-direction: column;
    gap: var(--l-hero-gap);
    pointer-events: none;
  }

  /* Размер задаётся ступенями, а не vw: при масштабировании напрямую от
     ширины окна заголовок «плыл» между разрешениями и DPI. Подзаголовок
     задан в em от заголовка, поэтому пара масштабируется согласованно;
     высота строк у обоих в px, кратных 4. */
  &__title {
    color: #fff;
    font-size: var(--l-hero-title);
    font-weight: 700;
    /* Разрядка положительная: отрицательный letter-spacing запрещён */
    letter-spacing: 0.14em;
    line-height: var(--l-hero-title-lh);
    text-transform: uppercase;
    /* Тень со смещением вниз, без ореола вокруг букв. Прежде тени не было
       вовсе: читаемость держал общий слой затемнения. С появлением
       переднего плана этого мало — вырезанные фигуры светлые и подходят
       к заголовку вплотную, а буквы на светлом плече теряются.
       Смещение вместо равномерного свечения выбрано намеренно: размытая
       копия по кругу мылит крупный шрифт, а падающая тень его не трогает. */
    text-shadow: 0 5px 14px rgba(0, 0, 0, 0.75);
    display: flex;
    flex-direction: column;
  }

  /* Подзаголовок не обрезается: перенос разрешён, ширина не ограничивается */
  &__subtitle {
    font-size: 0.4em;
    font-weight: 600;
    letter-spacing: 0.3em;
    color: rgba(255, 255, 255, 0.85);
    /* Подзаголовок тоньше заголовка и теряется первым — тень ему нужна
       та же, но мягче: крупной он бы забился. */
    text-shadow: 0 4px 10px rgba(0, 0, 0, 0.7);
    margin-top: 8px;
    line-height: var(--l-hero-subtitle-lh);
    white-space: nowrap;
  }

  &__stats {
    pointer-events: none;
  }

  &__title-line {
    display: flex;
    white-space: pre;
  }

  /* Ячейка, где старое и новое значение стоят друг на друге во время
     смены. Ширину и высоту задаёт то, что остаётся. */
  &__swap {
    display: grid;

    > * { grid-area: 1 / 1; }
  }

  &__title-rest {
    white-space: pre;
  }

  /* Строка подзаголовка всегда занимает место. */
  &__subtitle-slot {
    min-height: calc(var(--l-hero-subtitle-lh) + 8px);
  }

}

</style>
