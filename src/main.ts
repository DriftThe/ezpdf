import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
import "./styles/main.css";
import "katex/dist/katex.min.css";
import { initTheme } from "./composables/theme";

const app = createApp(App).use(createPinia());
initTheme(); // 主题在挂载前应用（首帧不闪变）
app.mount("#app");
