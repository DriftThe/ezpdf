<script setup lang="ts">
import { ref } from "vue";
import { useParseStore } from "../../../stores/parse";

const parse = useParseStore();

/** 四项本地服务检查的占位状态（阶段2.5 由 Rust 探测结果驱动，届时迁入 parse store） */
type ReadyState = "notready" | "ready";
type EnvState = "notready" | "cpu" | "gpu";
type SvcState = "stopped" | "idle" | "busy";

const python = ref<ReadyState>("notready");
const cuda = ref<ReadyState>("notready");
const env = ref<EnvState>("notready");
const svc = ref<SvcState>("stopped");

/** 状态位 → 文案 / 状态灯修饰类（"" = 灰色未就绪态） */
const READY_LABEL: Record<ReadyState, string> = { notready: "未就绪", ready: "已就绪" };
const READY_CLASS: Record<ReadyState, string> = { notready: "", ready: "ok" };
const ENV_LABEL: Record<EnvState, string> = { notready: "未就绪", cpu: "CPU", gpu: "GPU" };
const ENV_CLASS: Record<EnvState, string> = { notready: "", cpu: "info", gpu: "ok" };
const SVC_LABEL: Record<SvcState, string> = { stopped: "未启动", idle: "空闲中", busy: "繁忙中" };
const SVC_CLASS: Record<SvcState, string> = { stopped: "", idle: "ok", busy: "warn" };
</script>

<template>
  <div class="set-pane">
    <h2 class="set-title">OCR服务</h2>
    <!-- 注意：含 button 的行不能用 label 包裹（label 会把整行点击转发给按钮） -->
    <div class="set-field">
      <span>本地服务</span>
      <button class="set-button" @click="parse.testOcrConnection">检查本地服务</button>
    </div>
    <div class="set-field">
      <span>服务检查</span>
      <div class="set-field-row">
        <span class="svc" :class="READY_CLASS[python]"><span class="svc-dot" />Python {{ READY_LABEL[python] }}</span>
        <span class="svc" :class="READY_CLASS[cuda]"><span class="svc-dot" />CUDA {{ READY_LABEL[cuda] }}</span>
        <span class="svc" :class="ENV_CLASS[env]"><span class="svc-dot" />环境 {{ ENV_LABEL[env] }}</span>
        <span class="svc" :class="SVC_CLASS[svc]"><span class="svc-dot" />服务 {{ SVC_LABEL[svc] }}</span>
      </div>
    </div>
  </div>
</template>
