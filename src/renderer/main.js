import { createApp } from "vue";

import "@/assets/css/base.css";

import "@fortawesome/fontawesome-free/css/all.css";
import "@/assets/fonts/fonts.css";
import LocalizationHelper from "@/utils/locales";

import App from "./App";
import router from "./router";

const app = createApp(App);
app.config.globalProperties.$localization = LocalizationHelper;
app.config.globalProperties.$t = LocalizationHelper.get.bind(LocalizationHelper);
app.config.globalProperties.$isDebug = process.env.WEBPACK_DEV_SERVER === "true";

for (const type of ["dragover", "drop"])
  window.addEventListener(type, (event) => event.preventDefault(), false);

app.use(router).mount("#root");
