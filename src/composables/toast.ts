import { ref } from "vue";

interface ToastItem {
  id: number;
  text: string;
  kind: "info" | "warn" | "error";
}

const items = ref<ToastItem[]>([]);
let seq = 0;

/** 轻量 toast：错误/状态通知（自动 2.6s 消失；同屏多条纵向堆叠） */
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
