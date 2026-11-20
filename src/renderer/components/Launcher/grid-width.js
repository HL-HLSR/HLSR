// Директива v-grid-width: доводит ширину блока до ближайшей сверху,
// кратной шагу сетки (4 px, см. «Сетка» в assets/css/launcher.css).
//
// Ширина вкладки, кнопки с ником или панели сборки зависит от текста и
// выходит дробной: 126.88 px. Тогда всё, что стоит за таким блоком в ряду,
// съезжает на долю пикселя, и рамки соседей размываются на два пикселя.
// Директива меряет естественную ширину и ставит min-width, округлённую
// вверх; текст внутри не трогается.
//
// Перемеряет, когда меняется содержимое компонента (updated), когда
// блок сам меняет размер (ResizeObserver: показали после display: none,
// догрузился шрифт) и когда загружены шрифты.

const STEP = 4;
const observers = new WeakMap();

// getBoundingClientRect под CSS zoom отдаёт экранные пиксели, а min-width
// задаётся в пикселях раскладки
function layoutWidth(el) {
  const root = el.closest(".launcher-root");
  const zoom = (root && parseFloat(getComputedStyle(root).zoom)) || 1;
  return el.getBoundingClientRect().width / zoom;
}

// Допуск в сотую шага: 52.0000001 после деления на zoom — это 52, а не 56
function gridStep(binding) {
  return binding && binding.arg === "8" ? 8 : STEP;
}

function onGrid(width, step) {
  return Math.abs(width / step - Math.round(width / step)) < 0.01;
}

function snap(el, step) {
  el.style.minWidth = "";
  const width = layoutWidth(el);
  if (width > 0) el.style.minWidth = Math.ceil(width / step - 0.01) * step + "px";
}

export default {
  mounted(el, binding) {
    // Центрируемая пара «значок + текст» кратна 8: её отступы кратны 4.
    const step = gridStep(binding);
    snap(el, step);

    // min-width меняется в следующем кадре: правка размера наблюдаемого
    // блока прямо в колбэке — это «ResizeObserver loop», браузер откладывает
    // уведомления и сообщает об ошибке.
    if (typeof ResizeObserver === "function") {
      let frame = 0;
      const observer = new ResizeObserver(() => {
        cancelAnimationFrame(frame);
        frame = requestAnimationFrame(() => {
          const width = el.isConnected ? layoutWidth(el) : 0;
          if (width > 0 && !onGrid(width, step)) snap(el, step);
        });
      });
      observer.observe(el);
      observers.set(el, { observer, cancel: () => cancelAnimationFrame(frame) });
    }

    if (document.fonts && document.fonts.ready)
      document.fonts.ready.then(() => {
        if (el.isConnected) snap(el, step);
      });
  },
  updated(el, binding) {
    snap(el, gridStep(binding));
  },
  unmounted(el) {
    const watch = observers.get(el);
    if (watch) {
      watch.observer.disconnect();
      watch.cancel();
    }
    observers.delete(el);
  },
};
