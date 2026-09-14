<script setup lang="ts">
import { nextTick, ref, watch } from "vue";
import { pendingConfirm, settleConfirm } from "../../composables/confirm";

/**
 * 自绘确认框渲染器（全局单例，AppShell 挂载一次）：
 * 灰色蒙版 + 居中卡片，与设计体系一致（用户 2026-09-14 替换系统 ask）。
 * z-index 500：高于设置整页(10)/工具栏(20)/行菜单(30)/悬浮卡(100)，低于 toast(1000)。
 */
const confirmBtn = ref<HTMLButtonElement | null>(null);

// 打开即聚焦确认键：Enter/Esc 键盘操作可见
watch(pendingConfirm, (c) => {
  if (c) void nextTick(() => confirmBtn.value?.focus());
});

function onKeydown(e: KeyboardEvent): void {
  if (e.key === "Escape") {
    e.preventDefault();
    settleConfirm(false);
  } else if (e.key === "Enter") {
    e.preventDefault();
    settleConfirm(true);
  }
}
</script>

<template>
  <Teleport to="body">
    <div v-if="pendingConfirm" class="confirm-mask" @click.self="settleConfirm(false)" @keydown="onKeydown">
      <div class="confirm-card" role="alertdialog" aria-modal="true" :aria-label="pendingConfirm.title">
        <h3 class="confirm-title">{{ pendingConfirm.title }}</h3>
        <p class="confirm-message">{{ pendingConfirm.message }}</p>
        <div class="confirm-actions">
          <button class="btn ghost" @click="settleConfirm(false)">{{ pendingConfirm.cancelText }}</button>
          <button ref="confirmBtn" class="btn" :class="{ danger: pendingConfirm.danger }" @click="settleConfirm(true)">
            {{ pendingConfirm.confirmText }}
          </button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.confirm-mask {
  position: fixed;
  inset: 0;
  z-index: 500;
  background: rgba(0, 0, 0, 0.35); /* 灰色蒙版：突出弹窗、屏蔽底层交互 */
  display: flex;
  align-items: center;
  justify-content: center;
}
.confirm-card {
  width: min(420px, calc(100vw - 48px));
  padding: 16px 18px 14px;
  background: var(--bg-panel);
  color: var(--text-1);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  box-shadow: var(--shadow-2);
}
.confirm-title {
  margin: 0 0 8px;
  font-size: 15px;
  font-weight: 600;
}
.confirm-message {
  margin: 0 0 16px;
  font-size: 13px;
  line-height: 1.65;
  color: var(--text-2);
  overflow-wrap: anywhere;
}
.confirm-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}
</style>
