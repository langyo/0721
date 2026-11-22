import { initTheme } from "@celestia-island/hikari";
import "@celestia-island/hikari/styles";
import { createPinia } from "pinia";
import { createApp } from "vue";

import App from "./App.vue";
import { createAppI18n, setGlobalI18n, setLocale } from "@/i18n";
import { router } from "@/router";
import { registerBrandPresets } from "@/theme";
import { useAuthStore, wireUnauthorizedLogout } from "@/stores/auth";

const app = createApp(App);

const i18n = createAppI18n();
setGlobalI18n(i18n);
app.use(i18n);
// The initial locale was detected during createI18n; re-apply through
// setLocale so the hikari mirror and the document lang/dir follow it.
setLocale(i18n.global.locale.value);

const pinia = createPinia();
app.use(pinia);
app.use(router);

// The brand presets must be installed BEFORE initTheme() so the very
// first applyTheme() can resolve a persisted brand selection.
registerBrandPresets();
initTheme();

const auth = useAuthStore(pinia);
wireUnauthorizedLogout(auth);
// Prove the persisted token (if any) before the first navigation so the
// router guard decides on verified identity, not a stored claim.
void auth.restore().finally(() => {
  app.mount("#app");
});
