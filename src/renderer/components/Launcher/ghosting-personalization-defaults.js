// Персонализация Ghosting по умолчанию: цвет HUD и прицел игры как есть.
// Общая для окна настроек игры (новый черновик, сброс) и списка разделов
// настроек мода (точка «изменено» у раздела «Персонализация»).
export function defaultPersonalization() {
  return {
    hudEnabled: false,
    hudColor: { r: 255, g: 220, b: 0 },
    hudAlpha: -1,
    hudAffectsCrosshair: false,
    hideQuickInfo: false,
    crossPreset: "standard",
    crossEnabled: false,
    crossColor: { r: 0, g: 255, b: 0 },
    crossAlpha: 255,
    crossThickness: 2,
    crossSize: 10,
    crossGap: 3,
    crossOutline: false,
    crossOutlineThickness: 1,
    crossCircleRadius: 0,
    crossDotColor: null,
    crossDotSize: 0,
    crossTopLine: true,
    crossBottomLine: true,
    crossLeftLine: true,
    crossRightLine: true,
    crossHideDefault: true,
    extra: {},
  };
}

// Сколько настроек персонализации отличается от игры по умолчанию.
// crossPreset не считается: это имя набора, а не отдельная настройка, а
// настройки мода (extra) считаются в своих разделах.
export function personalizationChanges(value) {
  const defaults = defaultPersonalization();
  if (!value) return 0;

  return Object.keys(defaults).filter(
    (key) =>
      key !== "extra" &&
      key !== "crossPreset" &&
      JSON.stringify(value[key]) !== JSON.stringify(defaults[key])
  ).length;
}

// Персонализация из сохранённых настроек, дополненная значениями по
// умолчанию: старые сохранения могли не знать о новых полях или цветах.
export function normalizePersonalization(value) {
  if (!value || typeof value !== "object") return defaultPersonalization();
  const result = Object.assign(defaultPersonalization(), value);
  result.hudColor = Object.assign({ r: 255, g: 220, b: 0 }, value.hudColor || {});
  result.crossColor = Object.assign({ r: 0, g: 255, b: 0 }, value.crossColor || {});
  return result;
}
