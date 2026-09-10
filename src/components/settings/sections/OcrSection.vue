<script setup lang="ts">
import { useParseStore } from "../../../stores/parse";

const parse = useParseStore();
</script>

<template>
  <div class="set-pane">
    <h2 class="set-title">OCR服务</h2>
    <label class="set-field">
      <span>本地服务</span>
      <button class="set-button" @click="parse.testOcrConnection">检查本地服务</button>
    </label>
  </div>
</template>
