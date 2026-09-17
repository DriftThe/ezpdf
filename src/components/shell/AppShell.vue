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

/** App shell order; the settings page overlays app-body (toolbar keeps a higher z-index) and window
 *  decorations are off, so the title bar is always visible and draws its own drag region. */
const settings = useSettingsStore();
const parse = useParseStore();
const lib = useLibraryStore();

// Load config → reopen last repo → auto-start OCR; paused unless both startup switches are on
onMounted(async () => {
  await settings.ensureLoaded();
  if (!(settings.general.autoLaunch && settings.general.resumeOnStart)) parse.paused = true;
  await lib.openLastRepo();
  void settings.checkUpdate(); // silent at launch; toasts only on error
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
/* Positioning base for the settings overlay */
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
