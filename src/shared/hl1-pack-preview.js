"use strict";

// Сцена предпросмотра WON: здоровье 100, броня 100, MP5 25/67 и две гранаты.
// Размеры берутся из настоящих hud.txt и спрайтов паков. Координаты HUD —
// по cl_dll/{health,battery,ammo,flashlight}.cpp (в WON броня ещё на width/5).
function hudCommands(rects, width = 1600, height = 900) {
  const commands = [];
  const digit = rects.number_0, dw = digit[2], dh = digit[3];
  const baseline = height - dh - Math.floor(dh / 2);
  const numberY = baseline + Math.floor(dh * 0.2);
  const sprite = (name, x, y) => commands.push({ name, box: rects[name], at: [x, y] });
  const number = (value, x, y) => {
    for (const char of String(value).padStart(3, " ")) {
      if (char !== " ") sprite("number_" + char, x, y);
      x += dw;
    }
    return x;
  };
  const divider = (x, y) => commands.push({ at: [x, y], fill: [Math.floor(dw / 10), dh] });

  sprite("cross", Math.floor(rects.cross[2] / 2), baseline);
  let x = number(100, rects.cross[2] + Math.floor(dw / 2), numberY);
  divider(x + Math.floor(dw / 2), numberY);
  const suitX = Math.floor(width / 5), suitY = baseline - Math.floor(rects.suit_empty[3] / 6);
  sprite("suit_empty", suitX, suitY);
  sprite("suit_full", suitX, suitY);
  number(100, suitX + rects.suit_empty[2], numberY);

  x = number(25, width - 8 * dw - rects.ammo[2], numberY);
  x += Math.floor(dw / 2);
  divider(x, numberY);
  x = number(67, x + Math.floor(dw / 10) + Math.floor(dw / 2), numberY);
  sprite("ammo", x, numberY - Math.floor(rects.ammo[3] / 8));
  const secondaryY = numberY - dh - Math.floor(dh / 4);
  x = number(2, width - 4 * dw - rects.ammo2[2], secondaryY);
  sprite("ammo2", x, secondaryY - Math.floor(rects.ammo2[3] / 8));

  const flashlightX = width - rects.flash_full[2] - Math.floor(rects.flash_full[2] / 2);
  const flashlightY = Math.floor(rects.flash_empty[3] / 2);
  sprite("flash_empty", flashlightX, flashlightY);
  sprite("flash_full", flashlightX, flashlightY);
  return commands;
}

function crosshairLayer(values, metadata) {
  if (values.packCrosshair === "execut4ble-crosshair")
    return { image: "cross-execut4ble", box: [0, 0, 128, 128] };
  if (values.packCrosshair !== "crosshair-changer") return null;
  const shape = Math.max(0, metadata.shapes.indexOf(values.changerShape));
  const color = Math.max(0, metadata.colors.indexOf(values.changerColor));
  return { image: "cross-changer", box: [color * metadata.changerCell + metadata.changerInset,
    shape * metadata.changerCell + metadata.changerInset, metadata.changerSize, metadata.changerSize] };
}

module.exports = { hudCommands, crosshairLayer };
