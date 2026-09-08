<script setup lang="ts">
defineProps<{
  title: string;
  desc?: string;
  /** 图标尺寸 */
  size?: number;
}>();
</script>

<template>
  <div class="empty">
    <div class="empty-icon" :style="{ width: (size ?? 28) + 'px', height: (size ?? 28) + 'px' }" aria-hidden="true">
      <svg viewBox="0 0 24 24" width="100%" height="100%" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
        <path d="M7 3h7l4 4v13a1 1 0 0 1-1 1H7a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z" />
        <path d="M14 3v4h4" />
        <path d="M9.5 12.5h5M9.5 16h5" />
      </svg>
    </div>
    <div class="empty-title">{{ title }}</div>
    <div v-if="desc" class="empty-desc">{{ desc }}</div>
    <div class="empty-slot">
      <slot />
    </div>
  </div>
</template>

<style scoped>
.empty {
  height: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
  text-align: center;
  padding: 24px;
}
.empty-icon {
  color: var(--text-3);
  margin-bottom: 4px;
}
.empty-title {
  font-size: 14px;
  font-weight: 600;
  color: var(--text-1);
}
.empty-desc {
  font-size: 12px;
  color: var(--text-3);
  max-width: 260px;
}
.empty-slot {
  margin-top: 10px;
}
.empty-slot:empty {
  display: none;
}
</style>
