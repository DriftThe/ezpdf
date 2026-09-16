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


  },
});
