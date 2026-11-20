import { app, BrowserWindow, screen, shell } from "electron";
import path from "path";
import fs from "fs";
import log from "electron-log";

const isDevelopment = process.env.NODE_ENV === "development";

const DEV_URL = "http://localhost:9080";
const PROD_FILE = path.join(__dirname, "index.html");

const { computeWindowSize, scaleLayout, normalizeUiScale } = require("./window-size");

const ALLOWED_EXTERNAL_PROTOCOLS = ["https:", "http:"];

// Масштаб интерфейса лежит в settings.json рядом с аппаратным ускорением
// (renderer пишет его через Store, вкладка «Интерфейс»): окно создаётся
// раньше, чем renderer что-то спросит.
function readUiScale() {
  try {
    const raw = fs.readFileSync(path.join(app.getPath("userData"), "settings.json"), "utf8");
    const data = JSON.parse(raw.replace(/^\uFEFF/, ""));
    return normalizeUiScale(data && data.config ? data.config.uiScale : undefined);
  } catch (e) {
    return normalizeUiScale(undefined);
  }
}

function getTargetSize(uiScale = readUiScale()) {
  const display = screen.getPrimaryDisplay();
  return scaleLayout(computeWindowSize(display.workAreaSize), display.workAreaSize, uiScale);
}

// Новый масштаб без перезапуска: окно меняет размер вокруг своего центра,
// zoomFactor — вместе с ним. Возвращает масштаб, который поместился.
function applyUiScale(window, uiScale) {
  if (!window || window.isDestroyed()) return null;
  const target = getTargetSize(normalizeUiScale(uiScale));
  window.__hlsrLayout = target;
  const bounds = window.getBounds();
  const display = screen.getDisplayMatching(bounds).workArea;
  const centerX = bounds.x + bounds.width / 2;
  const centerY = bounds.y + bounds.height / 2;
  const x = Math.round(Math.min(Math.max(centerX - target.width / 2, display.x), display.x + display.width - target.width));
  const y = Math.round(Math.min(Math.max(centerY - target.height / 2, display.y), display.y + display.height - target.height));
  // Окно без изменения размера: на Linux setBounds такое окно не тянет.
  window.setResizable(true);
  window.setBounds({ x, y, width: target.width, height: target.height });
  window.setResizable(false);
  window.webContents.setZoomFactor(target.zoomFactor);
  log.info(`[ui-scale] масштаб ${Math.round(target.uiScale * 100)} %, окно ${target.width}x${target.height}`);
  return target.uiScale;
}

function isInternalURL(url) {
  if (isDevelopment) return url.startsWith(DEV_URL);

  let parsed;
  try {
    parsed = new URL(url);
  } catch (e) {
    return false;
  }

  if (parsed.protocol !== "file:") return false;

  try {
    const target = path.normalize(decodeURIComponent(parsed.pathname).replace(/^\//, ""));
    return target.toLowerCase() === path.normalize(PROD_FILE).toLowerCase();
  } catch (e) {
    return false;
  }
}

function openExternal(url) {
  try {
    let parsed = new URL(url);
    if (!ALLOWED_EXTERNAL_PROTOCOLS.includes(parsed.protocol)) {
      log.warn("Blocked external URL with disallowed protocol:", parsed.protocol);
      return;
    }

    shell.openExternal(url);
  } catch (e) {
    log.warn("Blocked malformed external URL");
  }
}

// Плеер YouTube во вкладке «Скрипты» HL1 без Referer показывает «ошибку
// настройки» (153): страница лаунчера открыта из file:// и его не передаёт.
// Подставляем адрес сайта только для запросов встроенного плеера.
const YOUTUBE_EMBED_URLS = ["https://www.youtube-nocookie.com/*"];
const EMBED_REFERER = "https://hlsr.pro/";

function applyEmbedReferer(contents) {
  contents.session.webRequest.onBeforeSendHeaders({ urls: YOUTUBE_EMBED_URLS }, (details, callback) => {
    callback({ requestHeaders: Object.assign({}, details.requestHeaders, { Referer: EMBED_REFERER }) });
  });
}

function applyNavigationPolicy(window) {
  const contents = window.webContents;

  contents.on("will-navigate", (event, url) => {
    if (isInternalURL(url)) return;

    event.preventDefault();
    log.info("Blocked in-window navigation to external URL");
    openExternal(url);
  });

  contents.setWindowOpenHandler(({ url }) => {
    openExternal(url);
    return { action: "deny" };
  });

  contents.on("did-fail-load", (e, code, description, validatedURL) => {
    log.error(`Renderer failed to load (${code}): ${description}`);
  });

  contents.on("render-process-gone", (e, details) => {
    const reason = details && details.reason;
    log.error("Renderer process gone:", reason);

    if (reason !== "clean-exit" && !contents.isDestroyed() && !contents.__hlsrReloaded) {
      contents.__hlsrReloaded = true;
      log.warn("Reloading renderer after crash");
      contents.reload();
    }
  });
}

function createWindow() {
  if (!app.requestSingleInstanceLock()) return app.quit();

  let targetSize = getTargetSize();
  const iconPath = path.join(process.resourcesPath, "icons", "hlsr.png");

  let window = new BrowserWindow({
    width: targetSize.width,
    height: targetSize.height,
    center: true,
    show: false,
    frame: false,
    resizable: false,
    // Один контур рисует renderer: системное скругление поверх CSS давало
    // разные радиусы и срезанные углы на Windows и Linux.
    transparent: true,
    roundedCorners: false,
    backgroundColor: "#00000000",
    icon: fs.existsSync(iconPath) ? iconPath : undefined,
    webPreferences: {
      preload: path.join(__dirname, "preload.js"),
      nodeIntegration: false,
      contextIsolation: true,
      sandbox: true,
      backgroundThrottling: false,
      webSecurity: !isDevelopment,
      devTools: isDevelopment,
      zoomFactor: targetSize.zoomFactor,
    },
  });

  window.__hlsrLayout = targetSize;

  // Chromium хранит масштаб по origin и может вернуть 1 после перезагрузки
  window.webContents.on("did-finish-load", () => {
    if (!window.isDestroyed()) window.webContents.setZoomFactor(window.__hlsrFullscreenZoom || window.__hlsrLayout.zoomFactor);
  });

  window.once("ready-to-show", () => window.show());

  const repaint = () => {
    if (window.isDestroyed() || window.isMinimized()) return;

    const contents = window.webContents;
    if (contents && !contents.isDestroyed()) contents.invalidate();
  };

  window.on("restore", repaint);
  window.on("show", repaint);
  window.on("focus", repaint);

  const sendVisibility = (visible) => {
    if (window.isDestroyed()) return;

    const contents = window.webContents;
    if (!contents || contents.isDestroyed()) return;

    contents.send("window-visibility", visible === true);
  };

  window.on("minimize", () => sendVisibility(false));
  window.on("hide", () => sendVisibility(false));
  window.on("restore", () => sendVisibility(true));
  window.on("show", () => sendVisibility(true));

  setTimeout(() => {
    if (!window.isDestroyed() && !window.isVisible()) window.show();
  }, 1500);

  window.setMenu(null);
  window.setFullScreenable(false);
  window.setMaximizable(false);
  applyNavigationPolicy(window);
  applyEmbedReferer(window.webContents);

  if (isDevelopment) {
    log.info(`Loading UI from ${DEV_URL}`);
    window.loadURL(DEV_URL);
  } else {
    log.info(`Loading UI from ${PROD_FILE}`);
    window.loadFile(PROD_FILE);
  }

  return window;
}

export { applyUiScale };
export default createWindow;
