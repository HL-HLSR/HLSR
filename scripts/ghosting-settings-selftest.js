"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const schema = require("../src/shared/ghosting-settings");
const locales = require("../src/shared/ghosting-settings-locales");
const personalization = require("../src/main/game-delivery/ghosting-personalization");
const playerConfig = require("../src/main/game-delivery/player-config");
// The same personalization path runs before the game starts on both platforms.
const LAUNCH_SERVICES = [
  ["Windows", require("../src/main/game-delivery/LaunchService")],
  ["Linux", require("../src/main/linux/LaunchService")],
];

function installation(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-ghosting-settings-"));
  const cfg = path.join(root, personalization.CFG_DIR);
  fs.mkdirSync(cfg, { recursive: true });
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  return { root, cfg, file: path.join(cfg, personalization.PERSONALIZATION_FILE) };
}

function launchFixture(t, LaunchService) {
  const fixture = installation(t);
  const { root, cfg, file } = fixture;
  fs.writeFileSync(path.join(root, "hl2.exe"), "test executable; never spawned");
  fs.writeFileSync(path.join(root, "ghostingmod", "gameinfo.txt"), '"GameInfo"\n{\nSteamAppId 243730\n}\n');
  fs.writeFileSync(path.join(root, "steam_appid.txt"), "243730\n");
  fs.writeFileSync(path.join(cfg, "config.cfg"), 'viewmodel_fov "84"\ngh_timer "1"\ngh_cross_size "30"\n');
  const oldFile = '// Original personalization\ngh_cross_size "8"\n';
  fs.writeFileSync(file, oldFile);
  const state = {
    settings: { personalization: { ...personalization.defaultPersonalization(), hudEnabled: true, crossSize: 8, extra: { gh_timer: false } } },
    failSave: false,
    events: [],
    warnings: [],
  };
  const clone = (value) => JSON.parse(JSON.stringify(value));
  const beforeSpawn = () => {
    state.events.push({ action: "spawn", cfg: fs.existsSync(file) ? fs.readFileSync(file, "utf8") : null });
    throw new Error("STOP_AFTER_PERSONALIZATION");
  };
  const service = new LaunchService({
    delivery: {
      processService: { isRunning: () => false, markStarting() {}, cancelStarting() {} },
      installedOf: () => ({ installPath: root }),
      isBusy: () => false,
    },
    launchRegistry: {
      "ghosting-renewed": {
        executable: "hl2.exe", gameDirectory: "ghostingmod", arguments: ["-game", "ghostingmod"],
        steamAppId: 243730, sourceSdkRequired: false,
      },
    },
    settingsStore: {
      get: () => clone(state.settings),
      save: (gameId, settings) => {
        state.events.push({ action: "save", cfg: fs.readFileSync(file, "utf8"), settings: clone(settings) });
        if (state.failSave) return { ok: false };
        state.settings = clone(settings);
        return { ok: true, settings: clone(settings) };
      },
    },
    // The Linux class uses protonRunner and the Windows class uses spawnFn.
    // Both stop immediately after the actual personalization write path.
    protonRunner: { launch: beforeSpawn },
    spawnFn: beforeSpawn,
    steamLocator: {
      locateSteam: () => ({ installPath: "/steam" }), assertSteamRunning: () => ({ pid: 1 }),
      describeSteamOnly: () => ({ ok: true, steam: { pid: 1 } }),
    },
    logger: { info() {}, error() {}, warn(message) { state.warnings.push(message); } },
  });
  return { ...fixture, oldFile, state, service };
}

test("all exposed fields have translations and stable default cfg roundtrips", () => {
  assert.equal(new Set(schema.FIELDS.map((field) => field.key)).size, schema.FIELDS.length);
  schema.SECTIONS.forEach((section) => {
    const keys = [section.title];
    section.fields.forEach((field) => {
      keys.push(field.label);
      if (field.hint) keys.push(field.hint);
      (field.options || []).forEach((option) => keys.push(option.label));
    });
    keys.forEach((key) => ["ru", "en"].forEach((lang) => assert.ok(locales[lang][key], `${lang}: ${key}`)));
  });
  assert.deepEqual(schema.importExtras(schema.serializeExtras(schema.defaults())), schema.defaults());
  const first = schema.defaults();
  first.gh_ghost_color.r = 0;
  assert.equal(schema.defaults().gh_ghost_color.r, 237, "preview colors must not mutate the schema");
});

test("only explicitly managed extras are written; legacy personalization is preserved", () => {
  const old = { hudEnabled: true, crossEnabled: true, crossColor: { r: 14, g: 52, b: 91 }, crossSize: 8 };
  const migrated = personalization.sanitizePersonalization(old);
  assert.deepEqual(migrated.extra, {});
  assert.equal(migrated.crossSize, 8);
  assert.deepEqual(migrated.crossColor, old.crossColor);
  assert.ok(!personalization.toCfgLines(migrated).includes('gh_timer "'));
  assert.ok(!personalization.toCfgLines(migrated).includes('viewmodel_fov "'));
  assert.deepEqual(personalization.applyPreset({ ...old, extra: { gh_timer: false } }, "dot").extra, { gh_timer: false });
});

test("crosshair preset ids ignore inherited object properties", () => {
  ["constructor", "__proto__", "toString", "hasOwnProperty", "valueOf"].forEach((id) => {
    assert.equal(personalization.sanitizePersonalization({ crossPreset: id }).crossPreset, "standard", id);
    assert.deepEqual(personalization.applyPreset({ crossEnabled: true }, id), personalization.sanitizePersonalization({ crossEnabled: true }), id);
  });
  assert.equal(personalization.sanitizePersonalization({ crossPreset: "custom" }).crossPreset, "custom");
  assert.equal(personalization.applyPreset({}, "circle").crossCircleRadius, 12);
});

test("false, zero, decimals, colors and last cfg assignment survive roundtrip", () => {
  const input = {
    gh_timer: false,
    gh_timer_x: 0,
    viewmodel_offset_x: -2.4,
    gh_hud_fontscale: 1.35,
    gh_chat_emoji_scale: 1.15,
    gh_trail_color: { r: 11, g: 222, b: 33 },
    gh_health_align: "9",
    gh_hud_font: "Lucida Console",
  };
  assert.deepEqual(schema.importExtras(schema.serializeExtras(input)), input);
  const parsed = schema.importExtras('gh_timer 1\ngh_timer "0" // override\ngh_timer_x 0\n');
  assert.deepEqual(parsed, { gh_timer: false, gh_timer_x: 0 });
});

test("unknown commands and malicious text never reach a generated cfg", () => {
  const dangerous = ['Impact";quit', "Impact\nquit", "Impact\rquit", "Impact;quit", "Impact\u0000quit", "Impact\u2028quit"];
  dangerous.forEach((value) => assert.deepEqual(schema.sanitizeExtras({ gh_hud_font: value }), {}));
  assert.deepEqual(schema.sanitizeExtras({ exec: "evil.cfg", quit: true, gh_spray_set: "evil", gh_timer: "false", viewmodel_fov: Infinity }), {});
  assert.deepEqual(schema.sanitizeExtras(JSON.parse('{"__proto__":{"gh_timer":true},"constructor":"quit"}')), {});
  assert.deepEqual(schema.importExtras('gh_hud_font "Impact"; quit\nexec evil.cfg\ngh_timer "0;quit"\n'), {});
  assert.equal(schema.sanitizeExtras({ viewmodel_fov: 500 }).viewmodel_fov, 100);
  assert.equal(schema.sanitizeExtras({ gh_opacity: -30 }).gh_opacity, 0);
  assert.equal(schema.sanitizeExtras({ gh_spray_scale: 0 }).gh_spray_scale, 0.05);
  assert.equal(schema.sanitizeExtras({ gh_trail_length: 2.8 }).gh_trail_length, 3);
});

test("spray and YouTube fields use real commands and validate user paths/URLs", () => {
  const clean = schema.sanitizeExtras({ gh_spray_file: "C:\\Pictures\\my spray.png", gh_chat_connect: "https://youtu.be/abcdefghijk?t=42" });
  assert.equal(clean.gh_spray_file, "C:/Pictures/my spray.png");
  assert.equal(clean.gh_chat_connect, "https://www.youtube.com/watch?v=abcdefghijk");
  const cfg = schema.serializeExtras(clean);
  assert.ok(cfg.includes('gh_spray_set "C:/Pictures/my spray.png"'));
  assert.ok(cfg.includes('gh_chat_connect "https://www.youtube.com/watch?v=abcdefghijk"'));
  assert.deepEqual(schema.importExtras(cfg), clean);
  const reset = schema.serializeExtras({ gh_spray_file: "", gh_chat_connect: "" });
  assert.ok(reset.includes("gh_spray_clear"));
  assert.ok(reset.includes("gh_chat_disconnect youtube"));
  assert.deepEqual(schema.importExtras(reset), { gh_chat_connect: "", gh_spray_file: "" });
  ["https://youtube.com.attacker.test/watch?v=abcdefghijk", "file:///etc/passwd", "https://youtube.com@attacker.test/watch?v=abcdefghijk", "https://youtube.com/watch?v=bad"].forEach((url) => {
    assert.deepEqual(schema.sanitizeExtras({ gh_chat_connect: url }), {});
  });
  assert.deepEqual(schema.sanitizeExtras({ gh_chat_channel: "Some_Runner" }), { gh_chat_channel: "some_runner" });
  assert.deepEqual(schema.sanitizeExtras({ gh_chat_channel: "runner;quit" }), {});
});

test("персонализация правит файл игрока на месте и повторно ничего не меняет", (t) => {
  const { root, cfg } = installation(t);
  const original = '// My settings\r\nbind F6 "save quick"\r\nexec speedrun.cfg\r\n';
  const autoexec = path.join(cfg, "autoexec.cfg");
  fs.writeFileSync(autoexec, original);
  const input = { personalization: { extra: { gh_timer: false, viewmodel_offset_z: -0.7 } } };
  const first = playerConfig.writePersonalization(root, "ghosting-renewed", "ghostingmod", {}, input);
  assert.deepEqual(first.changed.sort(), ["gh_timer", "viewmodel_offset_z"]);
  const written = fs.readFileSync(autoexec, "utf8");
  assert.ok(written.startsWith(original), "строки игрока на месте");
  assert.deepEqual(schema.importExtras(written), input.personalization.extra);
  assert.deepEqual(playerConfig.writePersonalization(root, "ghosting-renewed", "ghostingmod", input, input).changed, []);
  assert.equal(fs.readFileSync(autoexec, "utf8"), written);
  assert.ok(!written.includes("exec hlsr-personalization.cfg"), "exec снимка в файл игрока не добавляется");
  assert.equal(fs.existsSync(path.join(cfg, personalization.PERSONALIZATION_FILE)), false, "hlsr-personalization.cfg не создаётся");
});
test("chat connections respect disabled readers and retain their saved selection", () => {
  const input = { gh_chat: false, gh_chat_channel: "runner", gh_chat_connect: "https://www.youtube.com/watch?v=abcdefghijk" };
  const disabled = schema.serializeExtras(input);
  assert.ok(!disabled.split("\n").some((line) => line.startsWith("gh_chat_connect ")));
  assert.deepEqual(schema.importExtras(disabled), input);
  const youtubeDisabled = schema.serializeExtras({ gh_chat_youtube: false, gh_chat_connect: input.gh_chat_connect });
  assert.ok(!youtubeDisabled.split("\n").some((line) => line.startsWith("gh_chat_connect ")));
  const twitch = schema.serializeExtras({ gh_chat: true, gh_chat_channel: "runner" });
  assert.ok(twitch.indexOf('gh_chat "1"') < twitch.indexOf('gh_chat_connect "runner"'));
  assert.deepEqual(schema.importExtras(twitch), { gh_chat: true, gh_chat_channel: "runner" });
  assert.ok(schema.serializeExtras({ gh_chat_channel: "" }).includes("gh_chat_disconnect twitch"));
});

test("путь к спрею не ведёт на сетевую папку и наверх по каталогам", () => {
  for (const bad of ["//attacker.example/share/a.png", "\\\\attacker.example\\share\\a.png", "../../a.png", "custom/../../a.png", "C:/x/../a.png"])
    assert.equal(Object.keys(schema.sanitizeExtras({ gh_spray_file: bad })).includes("gh_spray_file"), false, bad);
  assert.equal(schema.sanitizeExtras({ gh_spray_file: "custom/picture.png" }).gh_spray_file, "custom/picture.png");
  assert.equal(schema.isManagedSpray(`materials/vgui/logos/hlsr/${"a".repeat(64)}.png`), true);
  assert.equal(schema.isManagedSpray("custom/picture.png"), false);
});

test("managed spray paths resolve from the game's working directory and roundtrip", (t) => {
  const { root } = installation(t);
  const importedPath = `materials/vgui/logos/hlsr/${"a".repeat(64)}.png`;
  const physicalPath = path.join(root, "ghostingmod", importedPath);
  fs.mkdirSync(path.dirname(physicalPath), { recursive: true });
  fs.writeFileSync(physicalPath, "test image path");
  const cfg = schema.serializeExtras({ gh_spray_file: importedPath });
  const emittedPath = /^gh_spray_set "([^"]+)"$/.exec(cfg)[1];
  assert.equal(emittedPath, `ghostingmod/${importedPath}`);
  assert.equal(fs.realpathSync(path.resolve(root, emittedPath)), fs.realpathSync(physicalPath));
  assert.deepEqual(schema.importExtras(cfg), { gh_spray_file: importedPath });
  assert.equal(schema.serializeExtras({ gh_spray_file: "custom/picture.png" }), 'gh_spray_set "custom/picture.png"');
});

for (const [platform, LaunchService] of LAUNCH_SERVICES) {
  // Раньше запуск сам переносил extras из config.cfg в настройки и переписывал cfg
  // персонализации; теперь настройки на запуске не пишутся, а файл игрока готовит prepare.
  test(`${platform}: запуск не пишет настройки и не переписывает extras из config.cfg`, async (t) => {
    const { service, state, cfg } = launchFixture(t, LaunchService);
    const before = JSON.stringify(state.settings);
    await assert.rejects(service.launch("ghosting-renewed"), /STOP_AFTER_PERSONALIZATION/);
    assert.deepEqual(state.events.map((event) => event.action), ["spawn"]);
    assert.equal(JSON.stringify(state.settings), before);
    assert.ok(fs.existsSync(path.join(cfg, "autoexec.cfg")), "файл игрока создан или получен переходом");
    assert.equal(fs.readFileSync(path.join(cfg, "config.cfg"), "utf8"), 'viewmodel_fov "84"\ngh_timer "1"\ngh_cross_size "30"\n');
  });
}

test("extras, написанные игроком в своём файле, читаются поверх сохранённых", (t) => {
  const { root, cfg } = installation(t);
  fs.writeFileSync(path.join(cfg, "autoexec.cfg"), 'gh_keyshud "1"\r\ngh_hud_fontscale "1.4"\r\ngh_cross_size "30"\r\n');
  fs.writeFileSync(path.join(cfg, personalization.PERSONALIZATION_FILE), '// Old HLSR file\ngh_cross_size "8"\ngh_keyshud "0"\n');
  const stored = { personalization: personalization.sanitizePersonalization({ crossSize: 8, extra: { gh_keyshud: false } }) };
  const shown = playerConfig.readPersonalization(root, "ghosting-renewed", "ghostingmod", stored).personalization;
  assert.equal(shown.crossSize, 30);
  assert.equal(shown.extra.gh_keyshud, true);
  assert.equal(shown.extra.gh_hud_fontscale, 1.4);
});

test("первое чтение принимает extras без цветов; сброс записывает значения по умолчанию в файл игрока", (t) => {
  const { root, cfg } = installation(t);
  fs.writeFileSync(path.join(cfg, "config.cfg"), 'gh_keyshud "1"\nviewmodel_fov "80"\n');
  assert.deepEqual(personalization.importFromConfigCfg(root).extra, { gh_keyshud: true, viewmodel_fov: 80 });
  const reset = schema.resetExtras({ gh_keyshud: true, viewmodel_fov: 80 });
  assert.deepEqual(reset, { gh_keyshud: false, viewmodel_fov: 62 });
  const was = { personalization: { extra: { gh_keyshud: true, viewmodel_fov: 80 } } };
  const result = playerConfig.writePersonalization(root, "ghosting-renewed", "ghostingmod", was, { personalization: { extra: reset } });
  assert.ok(result.changed.includes("viewmodel_fov") && result.changed.includes("gh_keyshud"));
  const text = fs.readFileSync(path.join(cfg, "autoexec.cfg"), "utf8");
  assert.deepEqual(schema.importExtras(text), reset);
  assert.equal(fs.readFileSync(path.join(cfg, "config.cfg"), "utf8"), 'gh_keyshud "1"\nviewmodel_fov "80"\n', "config.cfg не правится");
});
test("settings that only work with another one are off until it is set", () => {
  const field = (key) => schema.FIELDS.find((item) => item.key === key);
  const values = schema.defaults();
  schema.FIELDS.filter((item) => item.requires).forEach((item) => assert.ok(field(item.requires), item.key));
  assert.equal(schema.available(field("gh_hud_fontscale"), values), false);
  assert.equal(schema.available(field("gh_hud_fontscale"), Object.assign({}, values, { gh_hud_font: "Impact" })), true);
  assert.equal(schema.available(field("gh_showfps_font"), Object.assign({}, values, { cl_showfps: "2" })), true);
  // Requirements chain: the splits style needs the splits, which need the mirror.
  assert.equal(schema.available(field("gh_timer_mirror_splits_bold"), values), false);
  assert.equal(schema.available(field("gh_timer_mirror_splits_bold"), Object.assign({}, values, { gh_timer_mirror: true })), true);
  assert.equal(schema.available(field("gh_timer_mirror_splits_bold"), Object.assign({}, values, { gh_timer: false, gh_timer_mirror: true })), false);
  assert.equal(schema.available(field("gh_timer"), values), true);
});

test("FPS counter and position are on by default", () => {
  const values = schema.defaults();
  assert.equal(values.cl_showfps, "1");
  assert.equal(values.cl_showpos, true);
  assert.deepEqual(schema.resetExtras({ cl_showfps: "0", cl_showpos: false }), { cl_showfps: "1", cl_showpos: true });
});

test("field of view is written under all three Source names and reads back", () => {
  const cfg = schema.serializeExtras({ fov_desired: 120 });
  assert.match(cfg, /^default_fov "120"$/m);
  assert.match(cfg, /^fov_desired "120"$/m);
  assert.match(cfg, /^fov "120"$/m);
  assert.deepEqual(schema.importExtras(cfg), { fov_desired: 120 });
  assert.deepEqual(schema.sanitizeExtras({ fov_desired: 200 }), { fov_desired: 150 }, "clamped to the widest preview frame");
});

test("Windows launch restores steam_appid.txt that hl2.exe rewrote to HL2", async (t) => {
  const crypto = require("node:crypto");
  const manifests = require("../src/main/game-delivery/manifests");
  const LaunchService = LAUNCH_SERVICES[0][1];
  const fixture = launchFixture(t, LaunchService);
  // hl2.exe без -game пишет appid HL2 в том же формате, что и движок (с \0).
  const appIdFile = path.join(fixture.root, "steam_appid.txt");
  fs.writeFileSync(appIdFile, "220\n\0");
  fixture.service.registry["ghosting-renewed"] = manifests.launchConfigOf("ghosting-renewed");

  await assert.rejects(fixture.service.launch("ghosting-renewed"), /STOP_AFTER_PERSONALIZATION/);

  const files = JSON.parse(fs.readFileSync(path.join(__dirname, "../config/games/ghosting-renewed-1.0.7.files.json"), "utf8"));
  const list = files.files || files.entries || files;
  const entry = list.find((item) => item.path === "steam_appid.txt");
  const sha = crypto.createHash("sha256").update(fs.readFileSync(appIdFile)).digest("hex");
  assert.equal(sha, entry.sha256, "restored file must pass the integrity check");
});

test("Windows: без запущенного Steam Ghosting не запускается, а игра не трогается", async (t) => {
  const LaunchService = LAUNCH_SERVICES[0][1];
  const fixture = launchFixture(t, LaunchService);
  fixture.service.locator.describeSteamOnly = () => ({ ok: false, code: "STEAM_NOT_RUNNING", message: "Steam не запущен" });
  await assert.rejects(fixture.service.launch("ghosting-renewed"), (error) => error.code === "STEAM_NOT_RUNNING");
  assert.equal(fixture.state.events.length, 0, "ни cfg, ни запуска");
  fixture.service.locator.describeSteamOnly = () => ({ ok: false, code: "STEAM_NOT_INSTALLED", message: "Steam не найден" });
  await assert.rejects(fixture.service.launch("ghosting-renewed"), (error) => error.code === "STEAM_NOT_INSTALLED");
});
