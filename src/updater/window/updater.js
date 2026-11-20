"use strict";

const ipcRenderer = window.hlsrUpdaterIpc;

const el = {
  title: document.getElementById("title"),
  subtitle: document.getElementById("subtitle"),
  stage: document.getElementById("stage"),
  track: document.getElementById("track"),
  fill: document.getElementById("fill"),
  bytes: document.getElementById("bytes"),
  speed: document.getElementById("speed"),
  note: document.getElementById("note"),
  code: document.getElementById("code"),
  action: document.getElementById("action"),
};

let strings = {};
let actionMode = "cancel";

function t(key, arg) {
  const value = strings[key] || "";
  return arg === undefined ? value : value.replace("%0%", arg);
}

function formatBytes(value) {
  const bytes = Number(value) || 0;
  if (bytes >= 1024 * 1024 * 1024) return (bytes / (1024 * 1024 * 1024)).toFixed(2) + " GB";
  if (bytes >= 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + " MB";
  if (bytes >= 1024) return (bytes / 1024).toFixed(0) + " KB";
  return bytes + " B";
}

function setIndeterminate(on) {
  el.fill.classList.toggle("indeterminate", !!on);
  if (on) {
    el.fill.style.width = "";
    el.track.removeAttribute("aria-valuenow");
  }
}

function setProgress(done, total) {
  setIndeterminate(false);
  const percent = total > 0 ? Math.min(100, (done / total) * 100) : 0;
  el.fill.style.width = percent.toFixed(1) + "%";
  el.track.setAttribute("aria-valuenow", String(Math.round(percent)));
}

function setAction(mode) {
  actionMode = mode;

  if (!mode) {
    el.action.hidden = true;
    return;
  }

  el.action.hidden = false;
  el.action.textContent = mode === "cancel" ? t("cancel") : t("close");
}

ipcRenderer.on("updater:init", (event, payload) => {
  strings = payload.strings || {};
  document.documentElement.lang = payload.language === "en" ? "en" : "ru";

  el.title.textContent = t("title");
  el.subtitle.textContent = t("subtitle", payload.targetVersion || "");
  el.stage.textContent = t("stageDownload");
  setAction("cancel");
  setIndeterminate(true);
});

ipcRenderer.on("updater:stage", (event, payload) => {
  const stage = payload.stage;

  el.code.textContent = "";
  el.note.textContent = "";

  if (stage === "download") {
    el.stage.textContent = t("stageDownload");
    setAction("cancel");
    setProgress(payload.done || 0, payload.total || 0);
    return;
  }

  setAction(null);
  el.bytes.textContent = "";
  el.speed.textContent = "";

  if (stage === "verify") {
    el.stage.textContent = t("stageVerify");
    setIndeterminate(true);
    return;
  }

  if (stage === "wait-exit") {
    el.stage.textContent = t("stageWaitExit");
    el.note.textContent = t("waitingHlsr");
    setIndeterminate(true);
    return;
  }

  if (stage === "install") {
    el.stage.textContent = t("stageInstall");
    setIndeterminate(true);
    return;
  }

  if (stage === "relaunch") {
    el.stage.textContent = t("stageRelaunch");
    setIndeterminate(true);
    return;
  }

  if (stage === "done") {
    setIndeterminate(false);
    el.fill.className = "upd__fill done";
    el.stage.textContent = t("done", payload.version || "");
    el.note.textContent = t("doneHint");
    return;
  }

  if (stage === "cancelled") {
    setIndeterminate(false);
    el.fill.className = "upd__fill";
    el.fill.style.width = "0";
    el.stage.textContent = t("cancelled");
    el.note.textContent = t("cancelledHint");
    setAction("close");
    return;
  }

  if (stage === "error") {
    setIndeterminate(false);
    el.fill.className = "upd__fill failed";
    el.stage.textContent = t("errorTitle");
    el.note.textContent =
      (payload.error && strings[payload.error.code]) || t("errorHint");
    el.code.textContent = payload.error ? payload.error.code : "";
    setAction("close");
  }
});

ipcRenderer.on("updater:progress", (event, payload) => {
  setProgress(payload.done || 0, payload.total || 0);

  el.bytes.textContent = `${formatBytes(payload.done)} ${t("of")} ${formatBytes(payload.total)}`;
  el.speed.textContent = payload.bytesPerSecond
    ? formatBytes(payload.bytesPerSecond) + t("perSecond")
    : "";
});

el.action.addEventListener("click", () => {
  if (actionMode === "cancel") {
    setAction(null);
    el.stage.textContent = t("cancelled");
    ipcRenderer.send("updater:cancel");
    return;
  }

  ipcRenderer.send("updater:close");
});

document.addEventListener("keydown", (event) => {
  if (event.key !== "Escape") return;
  if (actionMode) el.action.click();
});
