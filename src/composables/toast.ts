import { ref } from "vue";

interface ToastItem {
  id: number;
  text: string;
  kind: "info" | "warn" | "error";
}

const items = ref<ToastItem[]>([]);
let seq = 0;

/** Lightweight toast: auto-dismisses after 2.6 s, stacks vertically. */
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
