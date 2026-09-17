import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
import "./styles/main.css";
import "katex/dist/katex.min.css";
import { initTheme } from "./composables/theme";
import { i18n } from "./lib/i18n";

const app = createApp(App).use(createPinia()).use(i18n);
initTheme(); // apply theme before mount (no first-frame flash)
app.mount("#app");
