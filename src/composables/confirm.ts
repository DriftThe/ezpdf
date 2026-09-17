import { ref } from "vue";
import { t } from "../lib/i18n";

/** Self-drawn confirm dialog replacing plugin-dialog's system ask.
 *  Module-level singleton state + renderer component; `await confirmDialog({...})` returns a bool.
 *  Single instance: a new request cancels the old one (its promise resolves false). */
interface ConfirmOptions {
  title: string;
  message: string;
  /** Confirm button label (defaults to Delete, the dominant destructive case). */
  confirmText?: string;
  cancelText?: string;
  /** Destructive: solid red confirm button. */
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

/** Pending confirm (null = none); ConfirmDialog.vue renders it. */
export const pendingConfirm = ref<PendingConfirm | null>(null);

export function confirmDialog(opts: ConfirmOptions): Promise<boolean> {
  settleConfirm(false); // single instance: cancel any pending request
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

/** Settle the request (Esc/backdrop/cancel → false, confirm → true); no-op when none. */
export function settleConfirm(ok: boolean): void {
  const c = pendingConfirm.value;
  if (!c) return;
  pendingConfirm.value = null;
  c.resolve(ok);
}
