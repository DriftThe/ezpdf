<script setup lang="ts">
import { onMounted } from "vue";
import Sidebar from "./Sidebar.vue";
import Toasts from "./Toasts.vue";
import TitleBar from "./TitleBar.vue";
import WindowResizeEdges from "./WindowResizeEdges.vue";
import ConfirmDialog from "../common/ConfirmDialog.vue";
import ReaderToolbar from "../toolbar/ReaderToolbar.vue";
import ReaderArea from "../reader/ReaderArea.vue";
import PageStatusStrip from "../reader/PageStatusStrip.vue";
import SettingsPage from "../settings/SettingsPage.vue";
import { useSettingsStore } from "../../stores/settings";
import { useParseStore } from "../../stores/parse";
import { useLibraryStore } from "../../stores/library";

/**
 * App shell: self-drawn title bar (full width) | sidebar (repo tree) | main area
 * (toolbar + reader panes + page status strip) | global (settings page/toasts/confirm).
 * The settings page is absolutely positioned over app-body; the toolbar has a higher
 * z-index and stays visible. The title bar sits outside and is always visible
 * (window decorations are off, the bar draws its own drag region).
 */
const settings = useSettingsStore();
const parse = useParseStore();
const lib = useLibraryStore();

// Startup: load config.json (overrides auth.cfg defaults) → paused by default
// (toolbar shows "start translation"; auto-runs only when both auto-launch OCR and
// resume-on-start are on) → reopen last repo → auto-start OCR service per switches
onMounted(async () => {
  await settings.ensureLoaded();
  if (!(settings.general.autoLaunch && settings.general.resumeOnStart)) parse.paused = true;
  await lib.openLastRepo();
  void settings.checkUpdate(); // silent update check at launch (toasts only if a new version exists)
  void parse.autoStartIfEnabled();
});
</script>

<template>
  <div class="app-shell">
    <TitleBar />
    <div class="app-body">
      <Sidebar />
      <main class="app-main">
        <ReaderToolbar />
        <ReaderArea />
        <PageStatusStrip />
      </main>
      <SettingsPage />
    </div>
    <WindowResizeEdges />
    <Toasts />
    <ConfirmDialog />
  </div>
</template>

<style scoped>
.app-shell {
  height: 100%;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
/* Work area below the title bar: positioning base for the settings overlay */
.app-body {
  flex: 1;
  min-height: 0;
  display: flex;
  position: relative;
}
.app-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
}
</style>
