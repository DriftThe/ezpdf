import { ref } from "vue";

export interface ToastItem {
  id: number;
  text: string;
  kind: "info" | "warn" | "error";
}

const items = ref<ToastItem[]>([]);
let seq = 0;

/** 轻量 toast：骨架期占位提示，后续阶段复用为错误/状态通知 */
export function toast(text: string, kind: ToastItem["kind"] = "info"): void {
  const id = ++seq;
  items.value.push({ id, text, kind });
  window.setTimeout(() => {
    items.value = items.value.filter((t) => t.id !== id);
  }, 2600);
}

export function useToasts() {
  return items;
}
