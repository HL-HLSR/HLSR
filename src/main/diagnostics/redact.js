"use strict";

const SECRET_ENV_RE = /(TOKEN|SECRET|PASSWORD|PASSWD|PWD|CREDENTIAL|APIKEY|API_KEY|_KEY|KEY_|AUTH|COOKIE|SESSION|SIGNATURE|PRIVATE)/i;

const ALLOWED_ENV = [
  "NUMBER_OF_PROCESSORS",
  "OS",
  "PROCESSOR_ARCHITECTURE",
  "PROCESSOR_IDENTIFIER",
  "SystemDrive",
  "COMPUTERNAME",
  // Linux: игры под Proton ведут себя по-разному в X11 и Wayland.
  // XDG_SESSION_TYPE отсекает SECRET_ENV_RE (SESSION), сессию видно по этим.
  "XDG_CURRENT_DESKTOP",
  "WAYLAND_DISPLAY",
  "DISPLAY",
];

const SIGNED_QUERY_RE = /(^|[?&])(sig|signature|token|access_token|id_token|refresh_token|key|apikey|api_key|password|auth|hash|expires|x-amz-[a-z-]+|se|sp|sv|sr|st)=/i;

const PLACEHOLDER = "<redacted>";

function redactUserPaths(text, options) {
  const opts = options || {};
  let out = String(text);

  if (opts.userName) {
    const escaped = opts.userName.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    out = out.replace(new RegExp(escaped, "gi"), "<user>");
  }

  out = out.replace(/([A-Za-z]:\\Users\\)[^\\/\r\n"']+/gi, "$1<user>");
  out = out.replace(/(\/Users\/)[^\\/\r\n"']+/gi, "$1<user>");

  return out;
}

function redactUrls(text) {
  return String(text).replace(/https?:\/\/[^\s"'<>)\]]+/gi, (url) => {
    const index = url.indexOf("?");
    if (index === -1) return url;

    const query = url.slice(index);
    if (!SIGNED_QUERY_RE.test(query)) return url;

    return url.slice(0, index) + "?" + PLACEHOLDER;
  });
}

function redactSecrets(text) {
  let out = String(text);

  out = out.replace(
    /-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----/g,
    `-----BEGIN PRIVATE KEY----- ${PLACEHOLDER} -----END PRIVATE KEY-----`
  );

  out = out.replace(/\b(Authorization|Cookie|Set-Cookie|Proxy-Authorization)\s*:\s*[^\r\n]+/gi,
    (m, header) => `${header}: ${PLACEHOLDER}`);

  out = out.replace(/\bBearer\s+[A-Za-z0-9._~+/-]+=*/g, `Bearer ${PLACEHOLDER}`);

  out = out.replace(
    /\b(token|secret|password|passwd|pwd|api[_-]?key|access[_-]?token|refresh[_-]?token|credential)\b(\s*[:=]\s*)("?)[^\s"'\r\n,;]+\3/gi,
    (m, key, sep) => `${key}${sep}${PLACEHOLDER}`
  );

  out = out.replace(/\b(gh[pousr]_[A-Za-z0-9]{16,}|github_pat_[A-Za-z0-9_]{20,})\b/g, PLACEHOLDER);

  out = out.replace(/\bssh-(rsa|ed25519)\s+[A-Za-z0-9+/=]{20,}/g, `ssh-key ${PLACEHOLDER}`);

  return out;
}

function redactIdentifiers(text) {
  let out = String(text);

  out = out.replace(/\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b/g, "<email>");
  out = out.replace(/\b7656119\d{10}\b/g, "<steamid>");

  return out;
}

function redactText(text, options) {
  if (text == null) return "";
  let out = String(text);

  out = redactSecrets(out);
  out = redactUrls(out);
  out = redactIdentifiers(out);
  out = redactUserPaths(out, options);

  return out;
}

function safeEnv(env, options) {
  const source = env || {};
  const allowed = (options && options.allow) || ALLOWED_ENV;
  const out = {};

  for (const name of allowed) {
    if (!Object.prototype.hasOwnProperty.call(source, name)) continue;
    if (SECRET_ENV_RE.test(name)) continue;
    out[name] = redactText(source[name], options);
  }

  return out;
}

function isSecretEnvName(name) {
  return SECRET_ENV_RE.test(String(name || ""));
}

module.exports = {
  PLACEHOLDER,
  ALLOWED_ENV,
  SECRET_ENV_RE,
  redactUserPaths,
  redactUrls,
  redactSecrets,
  redactIdentifiers,
  redactText,
  safeEnv,
  isSecretEnvName,
};
