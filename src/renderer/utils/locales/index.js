const { ipcRenderer } = window.hlsrElectron;
import settingsStore from "@/utils/settingsStore";
import en from "./en.locale";
import ru from "./ru.locale";

const DEFAULT_LANG = "en";

const store = settingsStore;

const LOCALES = { en, ru };

/**
 * Язык при первом запуске.
 *
 * Спрашивается у main, потому что ответ зависит от вещей, которых
 * renderer не видит: метки установщика в реестре (русская и английская
 * страницы загрузки дают файлы с разными именами) и языка системы.
 *
 * Записывается сразу — дальше это обычная настройка, и переключатель в
 * верхней панели меняет её как любую другую.
 */
function resolveInitial() {
  let index = 0;

  try {
    const answer = ipcRenderer.sendSync("app-initial-language");
    if (Number.isInteger(answer) && answer >= 0 && answer < Object.keys(LOCALES).length)
      index = answer;
  } catch (ignored) {
  }

  const config = Object.assign({}, store.get("config"), { language: index });
  store.set("config", config);

  return index;
}

function currentIndex() {
  const config = store.get("config");
  const value = config ? config.language : undefined;

  return Number.isInteger(value) ? value : resolveInitial();
}

const LocalizationHelper = {
  lang: currentIndex(),
  locales: LOCALES,
  _parse(str, vars) {
    for (let i = 0; i < vars.length; i++) str = str.replace(`%${i}%`, vars[i]);
    return str;
  },
  update() {
    this.lang = currentIndex();
  },
  get(key, ...vars) {
    let lang = Object.keys(this.locales)[this.lang];

    if (this.locales[lang] && this.locales[lang][key])
      return this._parse(this.locales[lang][key], vars);
    if (this.locales[DEFAULT_LANG] && this.locales[DEFAULT_LANG][key])
      return this._parse(this.locales[DEFAULT_LANG][key], vars);

    return key;
  },
};

export default LocalizationHelper;
