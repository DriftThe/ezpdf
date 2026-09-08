# ezpdf — 实时 PDF 翻译阅读器：架构与实施计划

> 本文件是已确认的实施计划（决策记录），后续开发以本文件为准。

## 概述

在现有 Tauri 2 + Vue 3 模板（实际接近纯净模板，无历史阅读器代码）上从零构建：仓库模式（Obsidian 式文件夹管理 + 持续后台解析）与离线模式（单文档解析/阅读/导出），双栏阅读器左原右译、跟随滚动。OCR 由仓库内托管的 Python FastAPI 服务承载（复用已有的 PP-DocLayoutV3 + PaddleOCR-VL-1.6 流水线），翻译由 OpenAI 兼容 LLM 完成，调度在 Rust 端，渲染与 UI 在前端。

## 关键决策（已确认）

| 决策点 | 选择 | 被否方案/原因 |
|---|---|---|
| OCR 形态 | 仓库内 `ocr-server/` FastAPI 服务；ezpdf 设置页可一键拉起（系统 Python/uv，启动命令可配置），也支持手动启动后仅填 URL | PaddleOCR-json（无公式/表格/版面）；打包内置 sidecar（链路复杂，后期再演进） |
| 在线 OCR | v1 只做本地推理；服务端保留 provider 抽象位，后续接远程同款服务/百度云 | — |
| 渲染栈 | 左右两栏统一用 `pdfjs-dist` 自绘页面条（canvas + 覆盖层），全虚拟化 | pdf-vue3 做左栏：两栏几何对齐/滚动同步别扭，等于维护两套逻辑 |
| 解析策略 | 导入即解析直到完成；全局暂停/恢复 + 并发设置；聚焦页随时插队 | 空闲才解析（体验差）、手动逐书（不符产品定位） |

## 总体架构

```
前端 Vue3 + Pinia + TS
  App Shell：左侧栏(仓库树/离线列表) │ 主区双栏阅读器 │ 工具栏/状态条/设置
  阅读器：pdfjs-dist 自绘 PageStrip（虚拟化），双栏同一套组件
  覆盖层：原文栏=虚线框+悬浮译文卡；译文栏=白底覆盖+渲染内容
  markdown-it + KaTeX 渲染公式/表格
──── Tauri IPC（invoke 命令 + events）────
Rust (src-tauri)
  ezpdf 格式 IO（serde）│ 仓库扫描/书目录 CRUD/导入导出
  调度器：页粒度优先级队列（聚焦优先/暂停/续跑）
  OCR 客户端（reqwest → ocr-server）│ LLM 客户端（reqwest，OpenAI 兼容）
  JSON 单写者原子写 │ 进度/请求事件推送 │ 设置与 key 存储（keyring）
──── HTTP ────
ocr-server/（本仓库内，FastAPI）
  GET /health → {status, mode, engine, device}
  POST /ocr/page {image_b64, scale} → {blocks:[{label, bbox_px, score, markdown}]}
  内部：现有流水线（PP-DocLayoutV3 → BoxFilter → RegionCropper → VLPredictor），
  推理用信号量串行化（可配置并发）；provider 抽象预留在线模式
```

**解析回路**（页级）：Rust 调度器取下一页 → emit `render-request{reqId,bookId,page,scale}` → 前端 pdfjs 渲染该页为 PNG（scale=2.0，144DPI）→ `invoke('submit_page_image',{reqId,pngB64})` → Rust POST 给 ocr-server → blocks 坐标 px→PDF点 写入 JSON → 该页文本类块批量发给 LLM → 译文写回 JSON → emit `page-status`。窗口未开时管线暂停（单窗口应用可接受，后续可加磁盘页图缓存）。

## ezpdf 格式规范

`<书名>/<书名>.pdf` + `<书名>.json`，四者名字一致（书文件夹名、pdf、json）；仓库模式中书文件夹可位于仓库的任意层级子目录。

```jsonc
{
  "version": 1,
  "meta": { "pageCount": 100, "createdAt": "...", "updatedAt": "...",
            "targetLang": "zh", "ocrEngine": "PP-DocLayoutV3+PaddleOCR-VL-1.6", "llmModel": "..." },
  "pages": [{
    "index": 0,
    "status": "pending | ocr_done | done | failed",   // 调度与断点续跑依据
    "widthPt": 612.0, "heightPt": 792.0,              // PDF 点，1pt=1/72in
    "blocks": [{
      "id": "p0-b3",
      "label": "text|title|list|figure|figure_caption|table|formula|header|footer|...",
      "bboxPt": [x, y, w, h],       // PDF 点，左上原点，scale=1 视口（与缩放无关）
      "score": 0.97,
      "source": "markdown 原文：正文纯文本；公式为 $$...$$；表格为 markdown 表格；figure 为 null",
      "translation": "markdown 译文（figure/formula 为 null，formula 原样渲染）"
    }]
  }]
}
```

要点：bbox 存点制坐标（像素会随缩放失效）；图片不入 JSON（右栏背景直接渲染原 PDF 页，图片天然保留且不覆盖 figure 块）；`source` 统一为 Markdown（与 VL 流水线输出对齐）；每页 `status` 驱动持续解析与重启续跑。

## 翻译协议（LLM）

- OpenAI 兼容端点（baseUrl + apiKey + model），目标语言默认简体中文，均可配置。
- 页级批量：一次请求带该页全部可译块 `[{id, label, markdown}]`，要求 JSON 输出 `[{id, translation}]`；表格翻单元格、公式不翻；serde 校验 id 齐全，缺/坏则重试（最多 2 次，仍败→页置 failed 可手动重试）；超长页自动分批。
- apiKey 存 OS 凭据库（Rust `keyring` crate），其余设置存 `settings.json`（appDataDir）。

## 调度器设计

- 任务粒度=页，状态机 `pending→ocr_queued→ocr_done→translating→done/failed`（与 JSON status 同步）。
- 优先级：聚焦页（当前书当前页 ±2 及其后 10 页）> 当前书其余页 > 其他书（按导入顺序轮转）；页粒度天然可抢占。
- 全局暂停/恢复；OCR 并发与 LLM 并发可配置；启动时扫描仓库，凡 status≠done/failed 的页自动续跑。
- JSON 写入：单写者 + 临时文件原子替换 + 防抖合并。

## UI 编排

- **App Shell**：左侧栏（顶部模式 Tab：仓库/离线；下方树/列表 + 导入按钮）│ 主区（顶部工具栏 + 阅读器）│ 全局：设置弹窗、暂停/恢复按钮、服务状态指示。
- **工具栏**：布局 4 态（原│译 / 译│原 / 仅原 / 仅译）、缩放（联动，两栏永远同倍率）、页码导航、悬浮预览开关、本页重新解析。
- **原文栏**：页面 canvas + 每个块虚线下划线/虚线框（公式表格块同样标注）；悬浮出译文卡（无译文时显示"翻译中/失败"态）。
- **译文栏**：页面 canvas 作背景 → done 页对可覆盖块（text/title/list/table/formula/caption/header/footer）画白底矩形 → 框内渲染 markdown 译文/公式（KaTeX）；figure 与未识别区域不覆盖。字号按 bbox 自适应缩小，达最小字号仍溢出→框内滚动（不破坏整页排版）。
- **状态条**：当前书页级状态点阵（pending/处理中/done/failed），点击跳页。
- **离线模式**：导入 pdf → 拷入 `appDataDir/offline/<书名>/` → 直接展开阅读器并进入解析；导入 ezpdf 文件夹同理跳过 OCR；导出=保存对话框拷出书文件夹。仓库模式导入 pdf 时拷贝到指定目录（根或所选子目录）自动建书文件夹并开解析。

## 开发流程 Todolist（阶段顺序即依赖顺序）

0. **骨架**：清理模板 greet；三区 Shell、Pinia、仓库/离线模式框架。验收：空壳可切换模式。
1. **格式层**：Rust serde 定义 JSON schema；书目录创建/导入(pdf/文件夹)/导出/校验/重名加后缀；仓库扫描；设置存储。验收：单元测试覆盖 schema 往返与扫描。
2. **阅读器核心**：pdfjs-dist 自绘 PageStrip（虚拟化、scale=1 点制坐标系）、双栏复用、布局 4 态、缩放联动、滚动同步。验收：`testfiles/test.pdf`（保留 AGENTS.md 约定）双栏滚动 1:1 跟随。
3. **OCR 服务化**：现有流水线包装进 `ocr-server/`（FastAPI + 统一契约 + /health + 信号量串行 + config）；ezpdf 内启动/连接/健康显示。验收：curl 单页图返回 blocks JSON。
4. **解析管线**：调度器（优先级/暂停/续跑/聚焦）、render-request 回路、px→点换算、JSON 原子写、进度事件。验收：导入一本书自动解析，重启后续跑。
5. **LLM 翻译**：客户端 + 页级批量协议 + 校验重试 + keyring + 设置 UI。验收：整页译文回写 JSON。
6. **译文渲染**：白底覆盖、markdown/KaTeX 渲染、自适应缩放与溢出策略、原文栏虚线+悬浮预览（含开关）。验收：含文字/表格/公式/图片的测试 PDF 四类块表现正确。
7. **仓库模式 UI**：目录树、导入至指定目录、书/页进度、状态条、聚焦优先联动。验收：切页后当前页优先出译文。
8. **离线模式**：导入→解析→阅读→导出完整流、离线列表。验收：全流程走通。
9. **打磨**：错误处理（服务掉线/LLM 失败/加密 PDF/坏 JSON）、空态、日志、大 PDF 性能。

## 边界与失败处理

- ocr-server 掉线：解析置暂停态 + 状态指示红点，自动拉起重试；LLM 失败：页 failed + 手动重试按钮 + 退避。
- 加密/损坏 PDF：导入时报错拒绝；书 JSON 损坏：校验失败提示重新导入。
- 重名书：自动加后缀 " (2)"；重命名书 = 同步改文件夹/两文件名。
- 已知限制（v1）：竖排文本不保证排版；极宽表格可能触发框内滚动；窗口关闭时解析暂停。

## 验证方式

- `pnpm build`（vue-tsc 严格类型检查 + 构建）、`cargo check` / `cargo test`（schema、调度优先级、坐标换算单元测试）。
- 手动场景：含文字+表格+公式+图片的多页 PDF → 仓库导入 → 聚焦第 3 页 → 验证插队、白底覆盖、公式渲染、悬浮预览、布局切换、滚动跟随、暂停/续跑、离线导出后再导入。

## 假设与默认值

- LLM 默认目标语言简体中文；OpenAI 兼容端点是唯一 LLM 接入方式（v1）。
- header/footer 块同样翻译渲染，不做特殊化。
- 在线/远程 OCR 仅留 provider 抽象，不实现。
- 仓库文件变更监听（notify）延后，v1 用启动扫描 + 手动刷新。
- AGENTS.md 中 `../testfiles/test.pdf` 的临时加载路径将被真实库流程取代，`testfiles/` 保留作测试素材目录。
