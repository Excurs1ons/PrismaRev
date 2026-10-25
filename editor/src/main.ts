import { createApp } from "vue";
import App from "./App.vue";
import "./style.css";
import { applyTheme, loadThemeState } from "./theme";

// 先套用主题再挂载，避免启动时闪一下默认配色。
applyTheme(loadThemeState());
createApp(App).mount("#app");
