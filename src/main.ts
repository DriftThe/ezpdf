import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
import "./styles/main.css";
import "katex/dist/katex.min.css";

createApp(App).use(createPinia()).mount("#app");
