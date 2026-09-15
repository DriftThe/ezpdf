<script setup lang="ts">
import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * 无边框窗口的缩放热区（用户 2026-09-15，Linux 适配）：
 * Windows 上 `decorations: false` 仍保留系统缩放边框，但 GTK/WebKitGTK 下窗口完全没有
 * 边框也就没有缩放入口——只剩拖动和最大化。这里在窗口四周补 8 条热区，
 * 按下即调用 startResizeDragging（权限见 capabilities/default.json）。
 * 只在 Linux 渲染：其他平台交给系统边框，避免与原生行为叠加。
 */
const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
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
/* 只吃边缘 4px（角落 8px）的指针事件，中间区域完全透传 */
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
