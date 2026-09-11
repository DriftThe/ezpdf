<script setup lang="ts">
import { computed, type Component } from "vue";
import { useSettingsStore, type SettingsSection } from "../../stores/settings";
import LlmSection from "./sections/LlmSection.vue";
import OcrSection from "./sections/OcrSection.vue";
import ParseSection from "./sections/ParseSection.vue";
import CommonSection from "./sections/CommonSection.vue";

const settings = useSettingsStore();

/**
 * 设置子项注册表（新增子项三步，见 stores/settings.ts 的 SettingsSection 处注释）：
 * Record 键必须覆盖 SettingsSection union —— 扩了 union 忘了注册这里会直接编译报错。
 */
type SectionDef = { label: string; comp: Component };
const SECTIONS: Record<SettingsSection, SectionDef> = {
  llm: { label: "LLM 翻译", comp: LlmSection },
  ocr: { label: "OCR 服务", comp: OcrSection },
  parse: { label: "解析", comp: ParseSection },
  common: {label:"常规",comp:CommonSection}
};
/** Object.entries 的键是 string，收窄回 SettingsSection 供导航绑定 */
const navList = Object.entries(SECTIONS) as Array<[SettingsSection, SectionDef]>;

const activeComp = computed(() => SECTIONS[settings.section].comp);
</script>

<template>
  <!--
    设置整页：绝对定位覆盖 sidebar + reader 视窗（app-shell 相对定位），
    顶部工具栏 z-index 更高保持可见可点；组件常驻 v-show，原视窗不销毁。
    子项切换 = 只渲染当前 section 组件（表单状态在 store，切换不丢）。
  -->
  <section v-show="settings.pageOpen" class="settings-page">
    <!-- 左列：返回 + 子项导航（复用 sidebar 条目呈现） -->
    <aside class="sp-nav">
      <!-- 退出即保存：返回按钮直接触发保存（持久化阶段1接入） -->
      <button class="btn ghost back-btn" title="保存并返回" @click="settings.save">
        <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round">
          <path d="M10 3L5 8l5 5" />
        </svg>
        返回
      </button>

      <nav class="sp-items">
        <button
          v-for="[id, def] in navList"
          :key="id"
          class="sp-item"
          :class="{ active: settings.section === id }"
          @click="settings.section = id"
        >
          {{ def.label }}
        </button>
      </nav>
    </aside>

    <!-- 右侧内容区（避开顶部工具栏高度），动态渲染当前子项 -->
    <div class="sp-content">
      <div class="sp-scroll">
        <component :is="activeComp" />
      </div>
    </div>
  </section>
</template>

<style scoped>
.settings-page {
  position: absolute;
  inset: 0;
  z-index: 10; /* 顶部工具栏（z-index:20）保持在其上可见可点 */
  display: flex;
  background: var(--bg-app);
}

/* ---- 左列导航（对齐 sidebar：同宽同底色同条目规格） ---- */
.sp-nav {
  width: var(--sidebar-w);
  flex: none;
  height: 100%;
  display: flex;
  flex-direction: column;
  background: var(--bg-panel);
  border-right: 1px solid var(--border);
}
.back-btn {
  align-self: flex-start;
  margin: 8px 8px 0;
  color: var(--text-2);
  display: inline-flex;
  align-items: center;
  gap: 6px;
}
.back-btn:hover {
  color: var(--text-1);
}
.sp-items {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 10px 8px;
}
/* 条目规格与放大后的仓库树一致（34px 行高 / 17px 字号） */
.sp-item {
  border: none;
  background: transparent;
  height: 34px;
  padding: 0 10px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  color: var(--text-2);
  font-size: 17px;
  text-align: left;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.sp-item:hover {
  background: var(--bg-hover);
  color: var(--text-1);
}
.sp-item.active {
  background: var(--accent-weak);
  color: var(--accent);
  font-weight: 600;
}

/* ---- 右侧内容 ---- */
.sp-content {
  flex: 1;
  min-width: 0;
  height: 100%;
  display: flex;
  flex-direction: column;
  padding-top: var(--toolbar-h); /* 避开常驻顶部工具栏 */
}
.sp-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
}
</style>
