"use strict";

// Полноэкранный плеер «Демо». Окно лаунчера создаётся без полноэкранного
// режима (CreateWindow: setFullScreenable(false), фиксированный размер),
// поэтому HTML Fullscreen API разворачивает плеер только на окно. На время
// показа плеера окну разрешается полный экран; при выходе (запрос интерфейса,
// Esc, сворачивание, скрытие, система сменила размер окна) возвращаются
// прежние размер, положение, resizable и запрет полного экрана, а интерфейс
// получает событие changed(false).
//
// Тот же механизм — у кнопки «Полный экран» в шапке лаунчера (AppManager.js):
// options.escape = false (Esc там закрывает окна интерфейса, а не полный экран),
// options.onEnter/onLeave меняют масштаб страницы, options.bounds(win) — куда
// развернуть окно вместо полного экрана (рабочая область: панель задач видна). Окно в полном экране держит
// один владелец (win.__hlsrFullscreen): второй получает true без входа — окно
// и так на весь экран, а его выход не трогает чужой полный экран.
//
// Окно лаунчера прозрачное и без рамки: на Windows Electron разворачивает его
// через SetBounds, isFullScreen() у такого окна всегда false, а
// leave-full-screen приходит только на наш же setFullScreen(false). Поэтому
// состояние — своё (state), а выход силами системы ловится по размеру: окно
// не свёрнуто, не в «настоящем» полном экране и уже не того размера, который
// получило при входе. Проверено в Electron 45 на окне с transparent, frame:false.
function createPlayerFullscreen(emit, options = {}) {
  const { escape = true, onEnter: enterHook, onLeave: leaveHook, bounds: targetBounds } = options;
  const self = {};
  let state = null;
  const same = (a, b) => !!a && !!b && a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height;

  const restore = (notify) => {
    if (!state) return;
    const { win, bounds, resizable, detach } = state;
    state = null;
    if (win.__hlsrFullscreen === self) win.__hlsrFullscreen = null;
    detach();
    const leave = () => {
      if (win.isDestroyed()) return;
      // Всегда: у прозрачного окна isFullScreen() не скажет, что оно развёрнуто.
      win.setFullScreen(false);
      win.setFullScreenable(false);
      win.setResizable(resizable);
      win.setBounds(bounds);
      if (leaveHook) leaveHook(win);
    };
    // Свёрнутое или спрятанное окно не трогаем: выход из полного экрана в таком
    // состоянии Windows не запоминает, и окно разворачивается во весь экран, а
    // setBounds после этого не действует (проверено в Electron 45). Выходим,
    // когда окно вернут; интерфейс узнаёт сразу.
    if (!win.isDestroyed() && (win.isMinimized() || !win.isVisible())) {
      const once = () => {
        win.removeListener("restore", once);
        win.removeListener("show", once);
        leave();
      };
      win.on("restore", once);
      win.on("show", once);
    } else leave();
    if (notify) emit(false);
  };

  function enter(win) {
    if (state) return;
    const saved = { win, bounds: win.getBounds(), resizable: win.isResizable(), fullBounds: null };
    const onEnter = () => {
      if (state === saved) saved.fullBounds = win.getBounds();
    };
    const onLeave = () => restore(true);
    // Свернули, спрятали в трей, система сменила размер (Win+↓, привязка к краю) —
    // полного экрана больше нет.
    const onGone = () => restore(true);
    const onBounds = () => {
      if (state !== saved || !saved.fullBounds || win.isMinimized() || win.isFullScreen()) return;
      if (!same(win.getBounds(), saved.fullBounds)) restore(true);
    };
    // Esc в плеере: у setFullScreen из main своего выхода по Esc нет. Само
    // нажатие страница тоже получает (закрыть плеер — её решение).
    const onKey = (event, input) => {
      if (input.type === "keyDown" && input.key === "Escape") restore(true);
    };
    const onClosed = () => {
      if (state && state.win === win) state = null;
    };
    win.on("enter-full-screen", onEnter);
    win.on("leave-full-screen", onLeave);
    win.on("minimize", onGone);
    win.on("hide", onGone);
    win.on("resize", onBounds);
    win.on("move", onBounds);
    win.once("closed", onClosed);
    if (escape) win.webContents.on("before-input-event", onKey);
    saved.detach = () => {
      win.removeListener("enter-full-screen", onEnter);
      win.removeListener("leave-full-screen", onLeave);
      win.removeListener("minimize", onGone);
      win.removeListener("hide", onGone);
      win.removeListener("resize", onBounds);
      win.removeListener("move", onBounds);
      win.removeListener("closed", onClosed);
      if (!win.webContents.isDestroyed()) win.webContents.removeListener("before-input-event", onKey);
    };
    state = saved;
    win.__hlsrFullscreen = self;
    if (enterHook) enterHook(win);
    // Окно лаунчера не resizable: без этого Windows не даёт развернуть его на весь экран.
    win.setResizable(true);
    if (targetBounds) {
      // Без полного экрана: окно ровно на рабочую область. Запомненный размер — фактический
      // (Windows округляет при масштабе экрана), по нему onBounds узнаёт, что окно сдвинули.
      win.setBounds(targetBounds(win));
      saved.fullBounds = win.getBounds();
      return;
    }
    win.setFullScreenable(true);
    win.setFullScreen(true);
    // Windows: enter-full-screen уже пришло синхронно; иначе размер запомнит onEnter.
    if (!saved.fullBounds && win.isFullScreen()) saved.fullBounds = win.getBounds();
  }

  return {
    // on: true — полный экран, иначе — обычное окно. Ответ — что применено
    // (своё состояние: isFullScreen() у прозрачного окна врёт).
    set(win, on) {
      if (!win || win.isDestroyed()) return false;
      // Окно уже на весь экран у другого владельца: применено, но не нами.
      if (win.__hlsrFullscreen && win.__hlsrFullscreen !== self) return on === true;
      if (on === true) enter(win);
      else restore(false);
      return !!state && state.win === win;
    },
    active: () => !!state,
    dispose: () => restore(false),
  };
}

module.exports = { createPlayerFullscreen };
