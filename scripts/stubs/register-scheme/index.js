"use strict";

// discord-rpc берёт register-scheme, только если не загрузился electron.
// В лаунчере electron есть всегда, поэтому настоящий нативный пакет
// (git-зависимость с node-gyp) не нужен; не-функция включает запасной путь.
module.exports = null;
