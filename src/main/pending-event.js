"use strict";

// Событие для окна, которое могло ещё не подписаться (старт лаунчера): пока окно
// не сообщило «готово», последнее событие ждёт; после отправки не повторяется.
function createPendingEvent(send) {
  let pending = null;
  let ready = false;
  return {
    push(payload) {
      if (ready) send(payload);
      else if (pending === null) pending = payload; // сервер отдаёт новые первыми
    },
    // Окно подписалось. Вызывается и после перезагрузки окна.
    markReady() {
      ready = true;
      const payload = pending;
      pending = null;
      if (payload) send(payload);
    },
  };
}

module.exports = { createPendingEvent };