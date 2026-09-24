import { createApp } from "vue";
import App from "./App.vue";
import "./styles/tokens.css";

createApp(App).mount("#app");

// Dev builds only: Ctrl+Shift+O previews every orb state.
if (import.meta.env.DEV) {
  void import("./components/OrbLab.vue").then((lab) => createApp(lab.default).mount(document.body.appendChild(document.createElement("div"))));
}
