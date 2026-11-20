import { createRouter, createWebHashHistory } from "vue-router";
import Loading from "@/views/Loading";
import Launcher from "@/views/Launcher";

// Окно грузит index.html с file://, поэтому история — через хеш.
export default createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/", name: "loading", component: Loading },
    { path: "/launcher", name: "launcher", component: Launcher },
  ],
});
