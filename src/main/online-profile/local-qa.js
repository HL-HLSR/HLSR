"use strict";

const path = require("path");
const os = require("os");
const fs = require("fs");

// Local tests may redirect all profile traffic only with an isolated, temporary
// user-data directory. Packaged launchers never accept this override.
function settings(app, env) {
  const source = env || process.env;
  if (app.isPackaged || !source.HLSR_PROFILE_QA_ROOT) return null;
  const root = path.resolve(source.HLSR_PROFILE_QA_ROOT);
  if (path.dirname(root) !== os.tmpdir() || !path.basename(root).startsWith("hlsr-profile-qa-"))
    throw new Error("Profile QA requires an isolated temporary directory");
  const url = new URL(source.HLSR_PROFILE_QA_API || "http://127.0.0.1:18895");
  if (url.protocol !== "http:" || url.hostname !== "127.0.0.1" || url.username || url.password || url.pathname !== "/")
    throw new Error("Profile QA requires a loopback API");
  return { root, apiBase: url.origin, accountFile: path.join(root, "fake-steam.json") };
}

function prepare(app) {
  const qa = settings(app);
  if (!qa) return null;
  fs.mkdirSync(qa.root, { recursive: true });
  app.setPath("userData", qa.root);
  return qa;
}

function account(qa) {
  try { return JSON.parse(fs.readFileSync(qa.accountFile, "utf8")); } catch (error) { return {}; }
}

function friends(qa) {
  try { return JSON.parse(fs.readFileSync(path.join(qa.root, "fake-friends.json"), "utf8")); } catch (error) { return []; }
}

module.exports = { settings, prepare, account, friends };
