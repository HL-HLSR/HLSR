import Store from "@/utils/Store";
import StoreDefaults from "@/utils/StoreDefaults";

// Один экземпляр на settings.json: Store держит копию файла в памяти, и
// второй экземпляр затёр бы при записи то, что сохранил первый (язык,
// аппаратное ускорение, масштаб интерфейса).
export default new Store({
  configName: "settings",
  defaults: StoreDefaults.settings,
});
