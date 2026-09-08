<script setup lang="ts">
import { useToasts } from "../../composables/toast";

const toasts = useToasts();
</script>

<template>
  <Teleport to="body">
    <TransitionGroup name="toast" tag="div" class="toast-stack">
      <div v-for="t in toasts" :key="t.id" class="toast" :class="t.kind">
        {{ t.text }}
      </div>
    </TransitionGroup>
  </Teleport>
</template>

<style scoped>
.toast-stack {
  position: fixed;
  bottom: 36px;
  left: 50%;
  transform: translateX(-50%);
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  z-index: 1000;
  pointer-events: none;
}
.toast {
  background: var(--text-1);
  color: var(--bg-panel);
  padding: 7px 14px;
  border-radius: 6px;
  font-size: 12px;
  box-shadow: var(--shadow-2);
  max-width: 420px;
}
.toast.warn {
  background: var(--warn);
  color: #fff;
}
.toast.error {
  background: var(--err);
  color: #fff;
}
.toast-enter-active,
.toast-leave-active {
  transition: opacity 0.18s, transform 0.18s;
}
.toast-enter-from,
.toast-leave-to {
  opacity: 0;
  transform: translateY(6px);
}
</style>
