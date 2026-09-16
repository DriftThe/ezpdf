import { ref } from "vue";
import { t } from "../lib/i18n";

/**
 * 自绘确认框（用户 2026-09-14）：替代 plugin-dialog 的系统 ask——系统弹窗
 * 与整体设计不一致，且无法带灰色蒙版。用法与 toast 同构（模块级单例状态 +
 * 渲染器组件），调用方 `await confirmDialog({...})` 拿布尔结果。
 * 单实例：新请求会先取消旧请求（旧 Promise 以 false 结束）。
 */
interface ConfirmOptions {
  title: string;
  message: string;
  /** 确认按钮文案（默认「删除」，destructive 场景主导） */
  confirmText?: string;
  cancelText?: string;
  /** 危险操作：确认按钮红色实心 */
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

/** 当前待确认项（null = 无弹窗）；渲染器 ConfirmDialog.vue 读取并渲染 */
export const pendingConfirm = ref<PendingConfirm | null>(null);

export function confirmDialog(opts: ConfirmOptions): Promise<boolean> {
  settleConfirm(false); // 单实例：先取消未决请求
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

/** 结算当前请求（Esc/蒙版/取消 → false；确认 → true）；无待确认时无副作用 */
export function settleConfirm(ok: boolean): void {
  const c = pendingConfirm.value;
  if (!c) return;
  pendingConfirm.value = null;
  c.resolve(ok);
}
