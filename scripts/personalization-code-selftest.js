"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const crypto = require("node:crypto");
const zlib = require("node:zlib");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const codes = require("../src/main/game-delivery/personalization-code");
const personalization = require("../src/main/game-delivery/ghosting-personalization");
const ghosting = require("../src/shared/ghosting-settings");
const goldsrc = require("../src/shared/goldsrc-settings");
const GameSettingsStore = require("../src/main/game-delivery/GameSettingsStore");

function forged(gameId, payload, suffix) {
  const packed = Buffer.concat([zlib.deflateRawSync(Buffer.isBuffer(payload) ? payload : Buffer.from(JSON.stringify(payload))), suffix || Buffer.alloc(0)]);
  const sum = crypto.createHash("sha256").update(`HLSR-P1.${gameId}.`).update(packed).digest("hex").slice(0, 16);
  return `HLSR-P1.${gameId}.${packed.toString("base64url")}.${sum}`;
}

test("чужой код не указывает на файлы этого компьютера и сетевые папки", () => {
  const look = (spray) => ({ personalization: { extra: { gh_spray_file: spray } } });
  const local = codes.decode("ghosting-renewed", codes.encode("ghosting-renewed", look("C:/Users/x/secret.png")));
  assert.equal(local.personalization.extra.gh_spray_file, "");
  const managed = `materials/vgui/logos/hlsr/${"a".repeat(64)}.png`;
  assert.equal(codes.decode("ghosting-renewed", codes.encode("ghosting-renewed", look(managed))).personalization.extra.gh_spray_file, managed);
  const unc = forged("218", look("//attacker.example/share/a.png"));
  assert.throws(() => codes.decode("ghosting-renewed", unc), { code: "PERSONALIZATION_CODE_INVALID" });
});

test("Ghosting: код сохраняет цвета, расположение, вид оружия и пользовательские значения без настроек лаунчера", () => {
  const source = {
    launcherTheme: "xen", language: "ru", launchArgs: "-console", binds: { SPACE: "+jump" },
    personalization: {
      hudEnabled: true, hudColor: { r: 17, g: 140, b: 225 }, hudAlpha: 185,
      crossEnabled: true, crossPreset: "custom", crossSize: 14, crossDotColor: { r: 19, g: 255, b: 28 },
      extra: { gh_health_x: -123, gh_health_y: 57, gh_health_align: "6", gh_keyshud: true, gh_keyshud_x: 88,
        gh_ghost_color: { r: 123, g: 32, b: 198 }, viewmodel_fov: 83, viewmodel_offset_x: 1.5,
        gh_name: "Игрок λ", gh_hud_font: "Arial", gh_chat_channel: "my_channel" },
    },
  };
  const before = JSON.stringify(source);
  const code = codes.encode("218", source);
  const look = codes.decode("218", code);
  assert.equal(JSON.stringify(source), before, "экспорт не меняет черновик");
  assert.deepEqual(Object.keys(look), ["personalization"]);
  assert.deepEqual(look.personalization.hudColor, source.personalization.hudColor);
  assert.deepEqual(look.personalization.crossDotColor, source.personalization.crossDotColor);
  assert.equal(look.personalization.crossSize, 14);
  assert.deepEqual(look.personalization.extra, { ...ghosting.defaults(), ...source.personalization.extra });
  assert.equal(codes.encode("ghosting-renewed", source), code, "внешний и внутренний ID дают один код");
  assert.deepEqual(codes.decode("218", " \n" + code.replace(/.{60}/g, "$&\n") + "\t"), look);
});

test("Half-Life: код переносит прицел, паки, BXT и поле зрения", () => {
  const source = {
    personalization: { hudEnabled: true, hudColor: { r: 240, g: 150, b: 15 }, crossEnabled: true, crossGap: 9 },
    packs: { hud: "ag-hud", crosshair: "crosshair-changer", changerColor: "pink", changerShape: "3px" },
    bxt: { timer: true, jumpspeed: true, fov: 111, viewmodelFov: 77, fixWidescreenFov: true },
  };
  const look = codes.decode("hl1-won", codes.encode("70", source));
  assert.deepEqual(look.packs, source.packs);
  assert.deepEqual(look.bxt, goldsrc.fromFlat(goldsrc.toFlat(source.packs, source.bxt)).bxt);
  assert.equal(look.personalization.crossGap, 9);
  assert.deepEqual(look.personalization.extra, {});
});

test("код другой игры, другой версии и повреждённая строка отклоняются", () => {
  const code = codes.encode("218", {});
  assert.throws(() => codes.decode("70", code), { code: "PERSONALIZATION_CODE_WRONG_GAME" });
  assert.throws(() => codes.decode("218", code.replace("HLSR-P1", "HLSR-P2")), { code: "PERSONALIZATION_CODE_VERSION_UNSUPPORTED" });
  assert.throws(() => codes.decode("220", code), { code: "PERSONALIZATION_CODE_GAME_UNSUPPORTED" });
  assert.throws(() => codes.decode("constructor", code), { code: "PERSONALIZATION_CODE_GAME_UNSUPPORTED" });
  for (const text of [null, "", "text", code.slice(0, -1), code + "0", code.replace(/\.[^.]+$/, ".0000000000000000")]) {
    assert.throws(() => codes.decode("218", text), { code: "PERSONALIZATION_CODE_INVALID" });
  }
});

test("даже код с верной суммой не разрешает чужие поля, команды и недопустимые значения", () => {
  const base = codes.decode("218", codes.encode("218", {}));
  const clone = () => JSON.parse(JSON.stringify(base));
  const theme = clone(); theme.launcherTheme = "combine";
  const args = clone(); args.launchArgs = "+quit";
  const command = clone(); command.personalization.extra.exec = "unknown.cfg";
  const separator = clone(); separator.personalization.extra.gh_name = 'runner";quit';
  const number = clone(); number.personalization.extra.viewmodel_fov = 10000;
  const type = clone(); type.personalization.hudEnabled = "yes";
  const color = clone(); color.personalization.hudColor.r = -100;
  const prototype = JSON.parse(JSON.stringify(base).replace('"personalization":{', '"personalization":{"__proto__":{"polluted":true},'));
  for (const raw of [theme, args, command, separator, number, type, color, prototype, [], { personalization: [] }]) {
    assert.throws(() => codes.decode("218", forged("218", raw)), { code: "PERSONALIZATION_CODE_INVALID" });
  }
  assert.equal({}.polluted, undefined);
});

test("ограничены длина, распаковка и лишние данные в сжатой части", () => {
  assert.throws(() => codes.decode("218", "x".repeat(codes.MAX_CODE_LENGTH + 1)), { code: "PERSONALIZATION_CODE_TOO_LARGE" });
  const bomb = forged("218", Buffer.alloc(codes.MAX_PAYLOAD_BYTES + 1, 0x61));
  assert.throws(() => codes.decode("218", bomb), { code: "PERSONALIZATION_CODE_TOO_LARGE" });
  const look = codes.decode("218", codes.encode("218", {}));
  assert.throws(() => codes.decode("218", forged("218", look, Buffer.from("extra"))), { code: "PERSONALIZATION_CODE_INVALID" });
});

test("сохранение импортированного вида сохраняет настройки запуска, бинды и другие игры", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-code-store-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const store = new GameSettingsStore({ file: path.join(root, "game-settings.json") });
  store.save("ghosting-renewed", { launchArgs: "-console", language: "en", rinput: false, startTimer: true, width: 1280, height: 720,
    binds: { space: { command: "+jump", base: "+duck" } } });
  store.save("hl1-won", { personalization: { crossGap: 7 }, bxt: { timer: true } });
  const before = store.get("ghosting-renewed"), other = store.get("hl1-won");
  assert.equal(Object.keys(before.binds).length, 1, "в исходных настройках есть свой бинд");
  const look = codes.decode("218", codes.encode("218", { personalization: { hudEnabled: true, extra: { gh_health_x: 93 } } }));
  const result = store.save("ghosting-renewed", { ...before, ...look });
  assert.equal(result.ok, true);
  assert.deepEqual({ ...result.settings, personalization: before.personalization }, before);
  assert.deepEqual(store.get("hl1-won"), other);
  assert.equal(result.settings.personalization.extra.gh_health_x, 93);
});

// Совместимость с лаунчером 0.4.8 (выпуск 31.10, коммит 26ba6fe): его разбор кода
// берётся из git как есть, со всеми модулями, которые он подключает.
const RELEASE_048 = "26ba6fe";
let release048Root = null;
test.after(() => { if (release048Root) fs.rmSync(release048Root, { recursive: true, force: true }); });
function release048(t) {
  if (!release048Root) {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-048-"));
    const copy = (rel) => {
      const file = path.join(root, rel);
      if (fs.existsSync(file)) return;
      const bytes = execFileSync("git", ["show", `${RELEASE_048}:${rel}`], { cwd: path.join(__dirname, ".."), stdio: ["ignore", "pipe", "ignore"] });
      fs.mkdirSync(path.dirname(file), { recursive: true });
      fs.writeFileSync(file, bytes);
      if (!rel.endsWith(".js")) return;
      for (const match of bytes.toString("utf8").matchAll(/require\("(\.{1,2}\/[^"]+)"\)/g)) {
        const next = path.posix.join(path.posix.dirname(rel), match[1]);
        copy(/\.(js|json)$/.test(next) ? next : next + ".js");
      }
    };
    try { copy("src/main/game-delivery/personalization-code.js"); } catch (error) {
      fs.rmSync(root, { recursive: true, force: true });
      t.skip(`нет коммита ${RELEASE_048} (лаунчер 0.4.8) в истории git`);
      return null;
    }
    release048Root = root;
  }
  return require(path.join(release048Root, "src/main/game-delivery/personalization-code.js"));
}

const SAMPLES = {
  "218": {
    personalization: { hudEnabled: true, hudColor: { r: 17, g: 140, b: 225 }, hudAlpha: 185, crossEnabled: true, crossSize: 14,
      extra: { gh_health_x: -123, gh_health_align: "6", gh_keyshud: true, viewmodel_fov: 83, gh_name: "Игрок λ", gh_hud_font: "Arial" } },
    mods: { "condensed-hud": true, "m9-bayonet": false },
  },
  "70": {
    personalization: { hudEnabled: true, hudColor: { r: 240, g: 150, b: 15 }, crossEnabled: true, crossGap: 9 },
    packs: { hud: "ag-hud", crosshair: "crosshair-changer", changerColor: "pink", changerShape: "3px" },
    bxt: { timer: true, jumpspeed: true, fov: 111 },
    mods: { "arianon-viewmodels": true, "item-sprites": false, "oxcjae-soundpack-lite": true },
  },
};
const withoutMods = ({ mods, ...look }) => look;

test("код 0.4.9 с модами читается в 0.4.8: всё, что 0.4.8 знает, без модов", (t) => {
  const old = release048(t);
  if (!old) return;
  for (const [id, source] of Object.entries(SAMPLES)) {
    const code = codes.encode(id, source);
    const look = codes.decode(id, code);
    assert.deepEqual(look.mods, source.mods, `моды в коде (${id})`);
    assert.deepEqual(old.decode(id, code), withoutMods(look), `0.4.8 читает вид (${id})`);
    // Без модов код тот же, байт в байт, что сделал бы 0.4.8.
    assert.equal(codes.encode(id, withoutMods(source)), old.encode(id, withoutMods(source)), `код без модов (${id})`);
  }
});

test("код 0.4.8 и ранней 0.4.9 импортируется как раньше, моды не трогаются", (t) => {
  const old = release048(t);
  if (!old) return;
  for (const [id, source] of Object.entries(SAMPLES)) {
    const code = old.encode(id, source);
    const look = codes.decode(id, code);
    assert.deepEqual(look, old.decode(id, code), id);
    assert.equal(look.mods, undefined, `в старом коде модов нет (${id})`);
  }
});

// Хвост после JSON так, как его пишет лаунчер: байт — четыре пробельных символа по 2 бита.
const spaces = (value) => [...Buffer.from(typeof value === "string" ? value : JSON.stringify(value))]
  .map((byte) => [6, 4, 2, 0].map((shift) => " \t\n\r"[(byte >> shift) & 3]).join("")).join("");

test("моды в коде: только моды этой игры и только да/нет, повреждённый хвост отклоняется", () => {
  const mixed = codes.encode("218", { mods: { "condensed-hud": true, "arianon-viewmodels": true, "future-pack": true, "m9-bayonet": "yes" } });
  assert.deepEqual(codes.decode("218", mixed).mods, { "condensed-hud": true });
  // Ни одного своего мода — хвоста нет, код как без модов.
  assert.equal(codes.encode("218", { mods: { "arianon-viewmodels": true } }), codes.encode("218", {}));
  const look = withoutMods(codes.decode("218", codes.encode("218", {})));
  const json = JSON.stringify(look);
  // Хвост из лаунчера новее: незнакомые разделы пропускаются, знакомые читаются.
  assert.deepEqual(codes.decode("218", forged("218", Buffer.from(json + spaces({ mods: { "m9-bayonet": true }, future: [1] })))).mods, { "m9-bayonet": true });
  assert.equal(codes.decode("218", forged("218", Buffer.from(json + spaces({ future: 1 })))).mods, undefined);
  for (const tail of ["   ", spaces("not json"), spaces([1]), spaces("null")])
    assert.throws(() => codes.decode("218", forged("218", Buffer.from(json + tail))), { code: "PERSONALIZATION_CODE_INVALID" });
});
