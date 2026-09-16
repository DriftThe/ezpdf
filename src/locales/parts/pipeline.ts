import { defineMessages } from "../helpers";

export default defineMessages({
  "zh-CN": {

    "toast.parsePaused": "解析已暂停",
    "toast.parseServiceOk": "解析服务正常：{detail}",
    "toast.parseResumed": "解析已恢复",
    "toast.serviceInstallDone": "服务安装完成",
    "toast.serviceInstalled": "服务已安装",
    "toast.translateStrikes": "《{name}》翻译连续失败，本轮暂停其翻译（OCR 照跑）",
    "toast.ocrStrikes": "《{name}》解析连续失败，本轮跳过",
    "toast.llmMissing": "LLM 未配置，翻译已跳过（设置页填写或配置 auth.cfg）",


    "pi.thinkingHintIncompatible": "该模型使用 {api} 协议，当前版本只能识别、不能调用（仅 OpenAI 兼容协议可用）",
    "pi.thinkingHintNoReasoning": "预设模型无思考模式，无需关思考参数",
    "pi.thinkingKindUnknown": "未知（自定义端点靠验证探测）",
    "pi.thinkingHintFormatSuffix": "（pi-ai {fmt} 格式）",
    "pi.thinkingHintDetail": "关思考参数：{kind}{fmt}；验证策略：{strategy}",
  },
  "zh-TW": {

    "toast.parsePaused": "解析已暫停",
    "toast.parseServiceOk": "解析服務正常：{detail}",
    "toast.parseResumed": "解析已恢復",
    "toast.serviceInstallDone": "服務安裝完成",
    "toast.serviceInstalled": "服務已安裝",
    "toast.translateStrikes": "《{name}》翻譯連續失敗，本輪暫停其翻譯（OCR 照跑）",
    "toast.ocrStrikes": "《{name}》解析連續失敗，本輪跳過",
    "toast.llmMissing": "LLM 未設定，翻譯已跳過（設定頁填寫或設定 auth.cfg）",


    "pi.thinkingHintIncompatible": "該模型使用 {api} 協議，目前版本只能辨識、不能呼叫（僅 OpenAI 相容協議可用）",
    "pi.thinkingHintNoReasoning": "預設模型無思考模式，無需關思考參數",
    "pi.thinkingKindUnknown": "未知（自訂端點靠驗證探測）",
    "pi.thinkingHintFormatSuffix": "（pi-ai {fmt} 格式）",
    "pi.thinkingHintDetail": "關思考參數：{kind}{fmt}；驗證策略：{strategy}",
  },
  en: {

    "toast.parsePaused": "Parsing paused",
    "toast.parseServiceOk": "Parse service ok: {detail}",
    "toast.parseResumed": "Parsing resumed",
    "toast.serviceInstallDone": "Service install complete",
    "toast.serviceInstalled": "Service already installed",
    "toast.translateStrikes": "\"{name}\" translation keeps failing; its translation is paused this round (OCR continues)",
    "toast.ocrStrikes": "\"{name}\" parsing keeps failing; skipped this round",
    "toast.llmMissing": "LLM not configured, translation skipped (fill in Settings or configure auth.cfg)",


    "pi.thinkingHintIncompatible": "This model uses the {api} protocol; this version can only identify it, not call it (only OpenAI-compatible protocols work)",
    "pi.thinkingHintNoReasoning": "Preset model has no thinking mode; no thinking-off parameter needed",
    "pi.thinkingKindUnknown": "unknown (custom endpoints rely on verify probing)",
    "pi.thinkingHintFormatSuffix": " (pi-ai {fmt} format)",
    "pi.thinkingHintDetail": "thinking-off parameter: {kind}{fmt}; verify strategy: {strategy}",
  },
});
