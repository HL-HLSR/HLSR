"use strict";

const fs = require("fs");
const http = require("http");
const net = require("net");
const os = require("os");
const path = require("path");
const { spawn, spawnSync } = require("child_process");

const ROOT = path.resolve(__dirname, "..");
const EXE = path.join(ROOT, "build", "win-unpacked", "HLSR.exe");
const START_TIMEOUT_MS = 20000;
const POLL_MS = 250;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function reservePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.unref();
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close((error) => (error ? reject(error) : resolve(address.port)));
    });
  });
}

function readJson(url) {
  return new Promise((resolve, reject) => {
    const request = http.get(url, { timeout: 1000 }, (response) => {
      let body = "";
      response.setEncoding("utf8");
      response.on("data", (chunk) => {
        body += chunk;
      });
      response.on("end", () => {
        try {
          resolve(JSON.parse(body));
        } catch (error) {
          reject(error);
        }
      });
    });

    request.on("timeout", () => request.destroy(new Error("CDP request timed out")));
    request.on("error", reject);
  });
}

async function waitForTarget(port, child) {
  const deadline = Date.now() + START_TIMEOUT_MS;

  while (Date.now() < deadline) {
    if (child.exitCode !== null)
      throw new Error(`HLSR exited before the renderer was ready (code ${child.exitCode})`);

    try {
      const targets = await readJson(`http://127.0.0.1:${port}/json`);
      const page = targets.find((target) => target.type === "page" && target.webSocketDebuggerUrl);
      if (page) return page;
    } catch (error) {
      // Chromium has not opened the debugging endpoint yet.
    }

    await sleep(POLL_MS);
  }

  throw new Error("Timed out waiting for the packaged renderer");
}

function connectCdp(url, exceptions) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(url);
    const pending = new Map();
    let sequence = 0;

    socket.addEventListener("open", () => {
      resolve({
        async send(method, params) {
          const id = ++sequence;
          const result = new Promise((resolveCall, rejectCall) => {
            pending.set(id, { resolve: resolveCall, reject: rejectCall });
          });
          socket.send(JSON.stringify({ id, method, params: params || {} }));
          return result;
        },
        close() {
          socket.close();
        },
      });
    });

    socket.addEventListener("message", (event) => {
      let message;
      try {
        message = JSON.parse(String(event.data));
      } catch (error) {
        return;
      }

      if (message.id && pending.has(message.id)) {
        const call = pending.get(message.id);
        pending.delete(message.id);
        if (message.error) call.reject(new Error(message.error.message || "CDP command failed"));
        else call.resolve(message.result);
        return;
      }

      if (message.method === "Runtime.exceptionThrown") {
        const details = message.params && message.params.exceptionDetails;
        const exception = details && details.exception;
        exceptions.push(
          (exception && exception.description) || (details && details.text) || "Renderer exception"
        );
      }

      if (message.method === "Runtime.consoleAPICalled") {
        const params = message.params || {};
        if (params.type === "error" || params.type === "warning") {
          const args = (params.args || []).map((arg) => arg.value || arg.description || "");
          exceptions.push(`console.${params.type}: ${args.join(" ")}`);
        }
      }
    });

    socket.addEventListener("error", () => reject(new Error("Could not connect to renderer CDP")));
    socket.addEventListener("close", () => {
      for (const call of pending.values()) call.reject(new Error("Renderer CDP closed"));
      pending.clear();
    });
  });
}

async function waitForVue(cdp, child) {
  const deadline = Date.now() + START_TIMEOUT_MS;
  let lastState = null;

  while (Date.now() < deadline) {
    if (child.exitCode !== null)
      throw new Error(`HLSR exited while waiting for Vue (code ${child.exitCode})`);

    let evaluation;
    try {
      evaluation = await cdp.send("Runtime.evaluate", {
        expression: `({
          mounted: !!document.querySelector('#app__container') && !!document.querySelector('#router'),
          bridge: typeof window.hlsrElectron,
          body: document.body ? document.body.innerHTML.slice(0, 500) : '',
          route: location.hash,
          scripts: document.scripts.length,
          title: document.title,
          readyState: document.readyState
        })`,
        returnByValue: true,
      });
    } catch (error) {
      await sleep(POLL_MS);
      continue;
    }
    if (evaluation && evaluation.exceptionDetails) {
      throw new Error(evaluation.exceptionDetails.text || "DOM evaluation failed");
    }
    const state = evaluation && evaluation.result && evaluation.result.value;
    lastState = state || lastState;
    if (state && state.mounted) return state;

    await sleep(POLL_MS);
  }

  throw new Error(`Vue did not mount in the packaged renderer: ${JSON.stringify(lastState)}`);
}

function stopProcessTree(child) {
  if (!child || child.exitCode !== null) return;
  spawnSync("taskkill.exe", ["/PID", String(child.pid), "/T", "/F"], {
    stdio: "ignore",
    windowsHide: true,
  });
}

async function removeProfile(profile) {
  for (let attempt = 0; attempt < 10; attempt += 1) {
    try {
      fs.rmSync(profile, { recursive: true, force: true });
      return;
    } catch (error) {
      if (attempt === 9) {
        console.warn(`Could not remove smoke-test profile: ${error.message}`);
        return;
      }
      await sleep(250);
    }
  }
}

async function main() {
  if (process.platform !== "win32") throw new Error("Packaged smoke test requires Windows");
  if (!fs.existsSync(EXE)) throw new Error(`Packaged launcher not found: ${EXE}`);

  const profile = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-packaged-smoke-"));
  const port = await reservePort();
  const output = [];
  const exceptions = [];
  let child = null;
  let cdp = null;

  try {
    child = spawn(
      EXE,
      [
        `--user-data-dir=${profile}`,
        `--remote-debugging-port=${port}`,
        "--enable-logging=stderr",
        "--disable-gpu",
      ],
      {
        cwd: path.dirname(EXE),
        windowsHide: true,
        env: Object.assign({}, process.env, { ELECTRON_ENABLE_LOGGING: "1" }),
        stdio: ["ignore", "pipe", "pipe"],
      }
    );

    const capture = (chunk) => {
      output.push(String(chunk));
      if (output.join("").length > 1024 * 1024) output.shift();
    };
    child.stdout.on("data", capture);
    child.stderr.on("data", capture);

    const target = await waitForTarget(port, child);
    cdp = await connectCdp(target.webSocketDebuggerUrl, exceptions);
    await cdp.send("Runtime.enable");
    await cdp.send("Page.enable");
    await cdp.send("Page.reload", { ignoreCache: true });
    await sleep(500);
    const state = await waitForVue(cdp, child);
    await sleep(1000);

    const consoleFailures = output
      .join("")
      .split(/\r?\n/)
      .filter((line) => /CONSOLE/.test(line) && /Uncaught|ReferenceError|TypeError|SyntaxError/.test(line));

    if (exceptions.length || consoleFailures.length) {
      throw new Error([...exceptions, ...consoleFailures].join("\n"));
    }

    console.log(
      `Packaged UI smoke test: OK (${state.title}, ${state.readyState}, ${state.route || "loading"})`
    );
  } catch (error) {
    const captured = output.join("").trim();
    if (captured) console.error(captured.slice(-12000));
    throw error;
  } finally {
    if (cdp) cdp.close();
    stopProcessTree(child);
    await removeProfile(profile);
  }
}

main().catch((error) => {
  console.error(`Packaged UI smoke test: FAILED\n${error.stack || error.message || error}`);
  process.exitCode = 1;
});
