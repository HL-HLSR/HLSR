"use strict";

function bind(ipcMain, service, openExternal) {
  const handles = {
    "runner-state": () => service.snapshot(),
    "runner-sync": () => service.sync(),
    "runner-login": (consent) => service.beginLogin(consent === true),
    "runner-login-cancel": () => { service.cancelLogin(); service.changed(); return service.snapshot(); },
    "runner-logout": () => service.logout(),
    "runner-logout-all": () => service.logoutAll(),
    "runner-search": (query) => service.search(query),
    "runner-players": (query) => service.players(query),
    "runner-profile-view": (id) => service.view(id),
    "runner-rate": (id, value) => service.rate(id, value),
    "runner-customize": (data) => service.action("customize", data),
    "runner-speedrun-link": (key) => service.action("speedrun/link", { apiKey: key }),
    "runner-speedrun-unlink": () => service.action("speedrun/unlink", {}),
    "runner-speedrun-bests": () => service.action("speedrun/bests", {}),
    "runner-speedrun-pin": (id) => service.action("speedrun/pin", { runId: id }),
    "runner-speedrun-unpin": () => service.action("speedrun/unpin", {}),
    "runner-discord-code": () => service.action("discord/code", {}),
    "runner-discord-unlink": () => service.action("discord/unlink", {}),
    "runner-telegram-code": () => service.action("telegram/code", {}),
    "runner-telegram-unlink": () => service.action("telegram/unlink", {}),
    "runner-secret-unlock": (id) => service.unlockFromRenderer(id),
    "runner-delete-challenge": () => service.action("delete/challenge", {}),
    "runner-delete-request": (data) => service.action("delete/request", data),
    "runner-delete-cancel": () => service.action("delete/cancel", {}),
    "runner-open-link": async (value) => {
      if (typeof value !== "string" || value.length > 512) throw new Error("RUNNER_INVALID_URL");
      let url = new URL(value);
      const hosts = ["steamcommunity.com", "www.speedrun.com", "youtube.com", "www.youtube.com", "youtu.be", "twitch.tv", "www.twitch.tv"];
      if (url.protocol !== "https:" || hosts.indexOf(url.hostname) === -1 || url.username || url.password || url.port)
        throw new Error("RUNNER_INVALID_URL");
      // Старые закреплённые забеги ведут на /runs/<id> без игры — такой
      // страницы на speedrun.com нет, настоящую ссылку знает его API.
      const legacyRun = url.hostname === "www.speedrun.com" && /^\/runs\/([a-z0-9]{8})$/i.exec(url.pathname);
      if (legacyRun) url = new URL(await service.speedrunRunLink(legacyRun[1]));
      return openExternal(url.href);
    },
  };
  for (const channel of Object.keys(handles)) {
    ipcMain.handle(channel, async (event, ...args) => {
      try { return await handles[channel](...args); } catch (error) {
        return { status: "error", code: error.code || "RUNNER_NETWORK" };
      }
    });
  }
}

module.exports = { bind };
