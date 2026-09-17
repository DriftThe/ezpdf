<script setup lang="ts">
import { getCurrentWindow } from "@tauri-apps/api/window";
import { isTauri } from "../../lib/env";

/**
 * Resize hit zones for the borderless window (Linux only):
 * with `decorations: false`, Windows still keeps its native resize border but
 * GTK/WebKitGTK has no resize entry point; these 8 zones call startResizeDragging
 * (permissions in capabilities/default.json). Other platforms use the native border.
 */
const isLinux = isTauri && /Linux/i.test(navigator.userAgent);
const win = isLinux ? getCurrentWindow() : null;

type Direction = "North" | "South" | "East" | "West" | "NorthEast" | "NorthWest" | "SouthEast" | "SouthWest";
const EDGES: Array<{ cls: string; dir: Direction }> = [
  { cls: "n", dir: "North" },
  { cls: "s", dir: "South" },
  { cls: "e", dir: "East" },
  { cls: "w", dir: "West" },
  { cls: "ne", dir: "NorthEast" },
  { cls: "nw", dir: "NorthWest" },
  { cls: "se", dir: "SouthEast" },
  { cls: "sw", dir: "SouthWest" },
];

function start(e: MouseEvent, dir: Direction): void {
  e.preventDefault();
  void win?.startResizeDragging(dir);
}
</script>

<template>
  <div v-if="isLinux" class="resize-edges">
    <div
      v-for="edge in EDGES"
      :key="edge.cls"
      class="edge"
      :class="edge.cls"
      @mousedown="start($event, edge.dir)"
    />
  </div>
</template>

<style scoped>
/* Pointer events only on the 4px edges (8px corners); the middle stays pass-through */
.resize-edges {
  position: absolute;
  inset: 0;
  z-index: 30;
  pointer-events: none;
}
.edge {
  position: absolute;
  pointer-events: auto;
}
.n,
.s {
  left: 8px;
  right: 8px;
  height: 4px;
  cursor: ns-resize;
}
.n {
  top: 0;
}
.s {
  bottom: 0;
}
.e,
.w {
  top: 8px;
  bottom: 8px;
  width: 4px;
  cursor: ew-resize;
}
.e {
  right: 0;
}
.w {
  left: 0;
}
.ne,
.nw,
.se,
.sw {
  width: 8px;
  height: 8px;
}
.ne,
.nw {
  top: 0;
}
.se,
.sw {
  bottom: 0;
}
.ne,
.se {
  right: 0;
  cursor: nesw-resize;
}
.nw,
.sw {
  left: 0;
  cursor: nwse-resize;
}
</style>
