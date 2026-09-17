import { ref } from "vue";
import { t } from "../lib/i18n";

/** Singleton self-drawn confirm; a new request cancels the pending one (resolves false). */
interface ConfirmOptions {
  title: string;
  message: string;
  confirmText?: string;
  cancelText?: string;
  danger?: boolean;
}

interface PendingConfirm {
  title: string;
  message: string;
  confirmText: string;
  cancelText: string;
  danger: boolean;
  resolve: (ok: boolean) => void;
}

export const pendingConfirm = ref<PendingConfirm | null>(null);

export function confirmDialog(opts: ConfirmOptions): Promise<boolean> {
  settleConfirm(false);
  return new Promise<boolean>((resolve) => {
    pendingConfirm.value = {
      confirmText: t("common.delete"),
      cancelText: t("common.cancel"),
      danger: true,
      ...opts,
      resolve,
    };
  });
}

export function settleConfirm(ok: boolean): void {
  const c = pendingConfirm.value;
  if (!c) return;
  pendingConfirm.value = null;
  c.resolve(ok);
}
