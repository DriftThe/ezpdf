<script setup lang="ts">
import { computed } from "vue";
import { useSettingsStore } from "../../stores/settings";
import { useParseStore } from "../../stores/parse";

const settings = useSettingsStore();
const parse = useParseStore();

const svcText = computed(
  () =>
    ({
      unknown: "未连接",
      starting: "启动中",
      connected: "已连接",
      disconnected: "已断开",
    })[parse.serviceStatus],
);
</script>

<template>
  <Teleport to="body">
    <div v-if="settings.modalOpen" class="mask" @click.self="settings.closeModal">
      <div class="modal">
        <header class="modal-head">
          <span class="modal-title">设置</span>
          <button class="icon-btn" title="关闭" @click="settings.closeModal">
            <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4">
              <path d="M4 4l8 8M12 4l-8 8" />
            </svg>
          </button>
        </header>

        <div class="modal-body">
          <section>
            <h3>LLM 翻译</h3>
            <label class="field">
              <span>API Base URL</span>
              <input v-model="settings.llm.baseUrl" placeholder="https://api.deepseek.com/v1" />
            </label>
            <label class="field">
              <span>API Key</span>
              <input v-model="settings.llm.apiKey" type="password" placeholder="阶段1起存储于系统凭据库" />
            </label>
            <label class="field">
              <span>模型</span>
              <input v-model="settings.llm.model" placeholder="deepseek-chat" />
            </label>
            <label class="field">
              <span>目标语言</span>
              <input v-model="settings.llm.targetLang" placeholder="zh（简体中文）" />
            </label>
          </section>

          <section>
            <h3>OCR 服务</h3>
            <div class="svc-line">
              <span class="svc" :class="parse.serviceStatus"><span class="svc-dot" />{{ svcText }}</span>
              <button class="btn ghost sm" @click="parse.testOcrConnection">测试连接</button>
            </div>
            <label class="field">
              <span>服务地址</span>
              <input v-model="settings.ocr.serverUrl" placeholder="http://127.0.0.1:8790" />
            </label>
            <label class="check">
              <input v-model="settings.ocr.autoLaunch" type="checkbox" />
              <span>ezpdf 自动拉起本地服务（否则需手动启动）</span>
            </label>
            <label class="field">
              <span>启动命令</span>
              <input v-model="settings.ocr.launchCommand" class="mono" />
            </label>
          </section>

          <section>
            <h3>解析</h3>
            <div class="field-row">
              <label class="field">
                <span>OCR 并发</span>
                <input v-model.number="settings.parse.ocrConcurrency" type="number" min="1" max="4" />
              </label>
              <label class="field">
                <span>翻译并发</span>
                <input v-model.number="settings.parse.llmConcurrency" type="number" min="1" max="8" />
              </label>
            </div>
            <label class="check">
              <input v-model="settings.parse.resumeOnStart" type="checkbox" />
              <span>启动时自动续跑未完成的解析</span>
            </label>
          </section>
        </div>

        <footer class="modal-foot">
          <button class="btn" @click="settings.closeModal">取消</button>
          <button class="btn primary" @click="settings.save">保存</button>
        </footer>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.mask {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.4);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 100;
}
.modal {
  width: 540px;
  max-height: 82vh;
  display: flex;
  flex-direction: column;
  background: var(--bg-panel);
  border-radius: 10px;
  box-shadow: var(--shadow-2);
  overflow: hidden;
}
.modal-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px 16px;
  border-bottom: 1px solid var(--border);
  flex: none;
}
.modal-title {
  font-size: 14px;
  font-weight: 600;
}
.modal-body {
  flex: 1;
  overflow-y: auto;
  padding: 6px 16px 14px;
}
.modal-foot {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  padding: 12px 16px;
  border-top: 1px solid var(--border);
  flex: none;
}

section + section {
  margin-top: 4px;
}
h3 {
  margin: 12px 0 6px;
  font-size: 12px;
  font-weight: 600;
  color: var(--text-3);
  text-transform: uppercase;
  letter-spacing: 0.4px;
}
.field {
  display: grid;
  grid-template-columns: 96px 1fr;
  align-items: center;
  gap: 10px;
  margin: 7px 0;
}
.field > span {
  font-size: 12px;
  color: var(--text-2);
}
.field input {
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  background: var(--bg-app);
  padding: 5px 9px;
  font-size: 12px;
  min-width: 0;
}
.field input:focus {
  outline: none;
  border-color: var(--accent);
}
.field input.mono {
  font-family: Consolas, monospace;
  font-size: 11px;
}
.field-row {
  display: flex;
  gap: 12px;
}
.field-row .field {
  flex: 1;
}
.check {
  display: flex;
  align-items: center;
  gap: 7px;
  font-size: 12px;
  color: var(--text-2);
  margin: 8px 0;
  cursor: pointer;
}
.check input {
  accent-color: var(--accent);
}
.svc-line {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin: 6px 0 10px;
}
</style>
