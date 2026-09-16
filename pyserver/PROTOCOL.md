# 解析服务协议 / Parse service protocol

简体中文 | [English](#english)

本文档描述 ezpdf 的**解析服务**（"parsing server"）对外暴露的完整契约：端口与启动方式、
鉴权、请求/响应格式、坐标换算、错误与限制、连接如何建立与维护，以及用其他语言实现一个
兼容服务所需的最小集合。参考实现就是本目录（`pyserver/`，FastAPI + PaddleOCR-VL）。

> 术语：**客户端** = ezpdf 应用（`src-tauri/src/parse.rs`）；**服务** = 提供 OCR 的解析服务。
> 翻译**不在本协议内**：LLM 翻译由客户端自己完成（用户 2026-09-15 解耦），解析服务只回答
> "这一页图里有哪些文本块、分别是什么内容"。

## 1. 两种部署形态

| 形态 | 谁启动 | 地址 | 鉴权 | 用途 |
| --- | --- | --- | --- | --- |
| **本地托管** | 客户端 spawn `python -m app.main` | 由服务自选临时端口（见 §7.1） | 每次启动的随机会话 token（必带） | 默认形态；需先装环境与模型 |
| **在线服务** | 部署方自己（`app/server_docker.py`，见 §7.4；开发联调 `server_test.py`） | 固定端口，如 `http://127.0.0.1:9055` | 服务端部署形态一律校验（`token.txt`）；`server_test.py` 默认无 | 基础环境（没装 torch/模型）也能用；也可放内网/公网 |

客户端在 设置 → OCR 服务 → 服务来源 里二选一；在线服务填地址（+ 服务令牌），点「测试」探活、
点「启动服务」登记。两种形态用的是**同一套 HTTP 契约**，服务端不需要知道自己是哪一种。

## 2. 传输与鉴权

- HTTP/1.1，`Content-Type: application/json`，UTF-8；响应体必须是可解析的 JSON。
- 鉴权头：`x-ezpdf-token: <token>`。
  - 本地托管：客户端每次 spawn 生成随机 token，经环境变量 `EZPDF_TOKEN` 传给服务；服务
    **必须**校验（本实现用常量时间比较，不匹配回 `403`）。
  - 在线服务：部署方决定要不要校验。`server_test.py` 默认不校验；带 `--token X` 时校验；
    服务端部署形态（`python -m app.server_docker`，见 §7.4）**一律校验**，令牌由服务端
    自己生成并落在 `token.txt`。
  - 令牌范围为**所有路由**（含 `/health`）：服务端开了鉴权，客户端连健康探测都会带这个头。
  - 客户端在设置 → OCR 服务 → 服务令牌 里填同一串令牌（每次 OCR 前都会重新握手，所以
    填错的表现是每一批都失败、连败 3 次后暂停该书解析，日志里是 `HTTP 403`）。
- 请求体上限 **64 MB**（本实现按 `Content-Length` 在读体之前拦掉，超限 `413`）。一批 ≤4 页
  PNG（scale 2.0）远小于此值，单张图另有 12k px 单边 / 40 MP 总像素上限。
- **OCR 请求没有客户端超时**：引擎首次请求要懒加载模型（分钟级），服务端不要设过短的
  请求超时；`/health` 则被客户端按 5s 超时探活，必须立刻返回。
- **批大小由服务端公布**（2026-09-15）：`/health` 里带 `max_batch_pages`，客户端在**每次**
  `/ocr/pages` 之前都先握手读它，再按这个数决定这一批发几页。所以 `/health` 必须便宜
  （纯内存），也允许你按显存/负载把它调小。

## 3. 端点总览

| 方法 | 路径 | 用途 | 客户端使用 |
| --- | --- | --- | --- |
| GET | `/health` | 探活 | 启动确认 / 在线模式「测试」 |
| POST | `/ocr/page` | 单页 OCR | 仅调试（curl 冒烟） |
| POST | `/ocr/pages` | 批量 OCR（≤32 页） | **主路径**，客户端每批 ≤4 页 |

## 4. `GET /health`

无请求体。响应（`status` 必填；`pid` 供排查；`max_batch_pages` 见下）：

```json
{ "status": "ok", "pid": 12345, "max_batch_pages": 32 }
```

`max_batch_pages` = 本服务单次 `/ocr/pages` 能接受的**最大页数**。客户端语义：

- **每次** OCR 请求前先调 `/health` 读这个值，然后按它收集这一批的页（所以服务端可以随时
  调整它，客户端下一批就跟上）；
- 字段缺失/非法 → 客户端按 4 页发（保守值）；值 > 32 → 夹到 32（客户端本地 Rust 侧的批次
  上限是 32，超了整批会被拒，见 §8）；
- 本地托管形态下客户端用自己固定的 4 页/批，不读这个字段。

本实现的取值来自环境变量 `EZPDF_MAX_BATCH_PAGES`（默认 32，`server_test.py --max-batch N`
可直接改），同时它就是 `/ocr/pages` 请求里 `pages` 的 `max_length`——公布值和实际能收的
必须一致，否则客户端会发出服务端自己拒掉的批次。

## 5. `POST /ocr/page`

请求：

```json
{ "image_b64": "iVBORw0KGgo..." }
```

`image_b64` 是图片字节的 base64，**可以带 `data:image/png;base64,` 前缀**（服务端应兼容剥离）；
任何能解码的图片格式都行（参考实现用 PIL，转 RGB）。

响应：

```json
{
  "width": 1190,
  "height": 1684,
  "elapsed": 2.83,
  "blocks": [
    {
      "label": "text",
      "score": 0.9134,
      "bbox_px": [120.5, 340.0, 980.25, 402.75],
      "markdown": "6.7 风管设计"
    }
  ]
}
```

## 6. `POST /ocr/pages`（客户端主路径）

请求（`pages` 项数上限 = `/health` 的 `max_batch_pages`，服务端必须至少接受 1 项；客户端按
§4 的协商值收集，同书一批、页序任意）：

```json
{
  "pages": [
    { "image_b64": "iVBORw0KGgo..." },
    { "image_b64": "iVBORw0KGgo..." }
  ]
}
```

响应（`pages` 顺序与请求**一一对应**，长度必须相等，否则客户端判为协议错误并整批失败）：

```json
{
  "elapsed": 5.12,
  "pages": [
    {
      "width": 1190, "height": 1684, "elapsed": 2.4,
      "blocks": [{ "label": "text", "score": 0.91, "bbox_px": [120.5, 340.0, 980.25, 402.75], "markdown": "6.7 风管设计" }]
    },
    { "width": 1190, "height": 1684, "elapsed": 2.7, "blocks": [] }
  ]
}
```

### 字段契约

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `label` | ✔ | 块类型，客户端**原样**写进绑定 JSON 的 `type`。参考实现来自 PP-DocLayoutV3 的 21 类：`text`、`paragraph_title`、`doc_title`、`abstract`、`aside_text`、`footnote`、`footer`、`header`、`vision_footnote`、`figure_title`、`content`、`reference`、`reference_content`、`algorithm`、`number`、`formula`、`formula_number`、`table`、`chart`、`image`、`seal` |
| `bbox_px` | ✔ | `[x1, y1, x2, y2]`，**提交图片的像素坐标系**（左上原点，px），见下 |
| `markdown` | ✔ | 块文本。客户端原样存为绑定 JSON 的 `content`（公式带 LaTeX、表格带 Markdown 表格）；`label == "image"` 时客户端会丢弃内容（置空串） |
| `score` | ✖ | 置信度，客户端不读；保留给人工排查 |
| `width` / `height` | ✖ | 图片像素尺寸，客户端不读；便于调试 |
| `elapsed` | ✖ | 秒，客户端不读 |

### 坐标契约（最容易踩的地方）

`bbox_px` 必须是**你收到的那张图**的像素坐标——不是 PDF pt、不是缩放前的画布坐标。
客户端自己渲染页图（`scale = 2.0`，即 `px = pt × 2`），并**用同一个 scale 把 px 换回 PDF pt**
（`pt = px / scale`，保留 2 位小数）写进绑定 JSON。所以：服务端**不需要也不应该**做任何
pt 换算，更不要把 `bbox_px` 归一化到 0..1 或返回原始 PDF 尺寸的坐标。

客户端保证按 `bbox_px` 画出的框与实际页面对齐，唯一前提是"坐标属于我发的那张图"。

### 允许不是"块"的实现

协议不要求服务端用同一个模型：只要返回的 `blocks` 能覆盖页面文本（哪怕是把整页当成一个
`text` 块）就能跑通全流程；块越准，译文覆盖框越贴合原文。OCR 之外的版面分析（阅读顺序、
表结构）不参与协议。

## 7. 连接与生命周期

### 7.1 本地托管（客户端 spawn）

1. 客户端以 `EZPDF_TOKEN` / `EZPDF_MODELS_DIR` 环境变量启动 `python -m app.main`（cwd =
   `pyserver/`，stdin/stdout/stderr 全部接管）。
2. 服务绑定端口（参考实现是 `port = 0`，由内核分配临时端口），**必须**在开始服务前向
   stdout 打一行就绪信号，然后才能对外服务：

   ```
   EZPDF_READY {"port": 51234, "pid": 4567}
   ```

   客户端逐行读 stdout 直到这一行（60s 超时），从中取得端口；其余 stdout 行当日志。
3. 客户端立刻用该 token `GET /health` 复核，成功才置为"已连接"。
4. **保活靠 stdin**：客户端持有子进程 stdin 不关。客户端退出或点「停止服务」时关闭 stdin →
   服务读到 EOF 后应优雅退出（参考实现读裸 fd 0，退出前 5s 兜底硬退）。这条是防孤儿进程的
   关键；自己实现服务端时务必保留一个 stdin-EOF 看门狗。
5. 崩溃自动重启：非主动退出 → 指数退避（1s 起、上限 15s）重拉，连续 3 次失败进入终态
   `failed`（前端显示红色，需手动再启动）。

### 7.2 在线服务（客户端只做 HTTP）

1. 客户端对地址跑一次 `GET /health`（5s 超时，回环地址不走系统代理）；成功即把该地址登记为
   OCR 目标并把状态置为"已连接"（`ocr://status` 事件 → 前端「服务」灯），同时记下
   `max_batch_pages` 显示在设置页地址下方。
2. 连接是**无状态**的：没有心跳、没有长连接、没有会话。任意一次 `/ocr/pages` 失败都会让这一批
   计入连败；前端连败 3 次只暂停该类批次的调度，不会"断开连接"。
3. 客户端不主动重连也不做健康轮询：地址挂了要在设置页点「测试」/「启动服务」重新登记。

### 7.3 客户端认为"能用"的条件

`GET /health` 返回 2xx。仅此而已——`/health` 应答慢或返回非 JSON 都算失败。
在线模式下这个握手**每条 OCR 批次前都会重跑**，所以 `/health` 挂掉等价于"这一批解析失败"
（计入连败，见 §8）；翻译不受影响。

### 7.4 服务端部署（Docker / 裸机）：token 文件

`python -m app.server_docker` 是给"没有客户端 spawn"的部署形态准备的入口（容器、内网/公网
服务器），与 `app.main` 的差别只有三处：绑定 `EZPDF_HOST:EZPDF_PORT`（默认 `0.0.0.0:9055`）
而不是自选临时端口、没有 stdin-EOF 看门狗（容器里 stdin 是 `/dev/null`，看门狗会立刻自杀）、
不打 `EZPDF_READY` 行（没有父进程要读）。

令牌解析优先级（`app/config.py::resolve_token`）：

1. `EZPDF_TOKEN` 环境变量（显式指定，最高优先）；
2. `EZPDF_TOKEN_FILE` 指定的文件（默认 `<pyserver>/token.txt`）里已有的令牌；
3. 都没有 → `secrets.token_urlsafe(24)` 生成，写入该文件（POSIX 下 `0600`）。

**每次启动都把令牌打到终端**，部署方从日志里复制到客户端的「服务令牌」。文件存在即复用，
所以重启服务、重建容器都不会让客户端已保存的令牌失效——容器里请把 `token.txt` 所在目录
挂成卷（本仓库的 `docker-compose.yml` 用 `./data:/data` + `EZPDF_TOKEN_FILE=/data/token.txt`）。

裸机开发联调仍可用 `server_test.py`（默认不校验，`--token X` 开启），它不读令牌文件。

## 8. 错误与重试

| 状态码 | 何时 | 客户端行为 |
| --- | --- | --- |
| `400` | 图片解码失败/参数非法 | 整批失败，计入连败 |
| `403` | token 不匹配 | 同上（本地托管几乎只会因为装载了两份服务） |
| `413` | 请求体或图片过大 | 同上 |
| `500` | 推理异常 | 同上 |
| 2xx + `pages` 长度不符 | 协议错误 | 同上，日志写明"page count mismatch" |

错误响应体沿用 FastAPI 的 `{"detail": "..."}`；客户端把 `detail` 前缀进日志（截断 300 字符）。
前端同一本书解析失败 3 次就不再重试该书（OCR 支路关闭，翻译支路不受影响）。

## 9. 用别的语言/框架实现：最小清单

- [ ] `GET /health` → 2xx + `{"status":"ok","max_batch_pages":N}`（快，别做重活；N 就是你的
      批次上限，客户端每次请求前都来读一遍）。
- [ ] `POST /ocr/pages` → 严格按请求顺序返回同长度 `pages`，每页给 `blocks[]`，每块给
      `label` / `bbox_px`（**提交图片的 px 坐标**）/ `markdown`。
- [ ] 兼容 base64 的 `data:` 前缀；只接受 `{"pages":[{"image_b64": "..."}]}`（忽略未知字段，
      不要因为多了字段而报错——客户端将来会加字段）。
- [ ] 本地托管形态：支持 `EZPDF_READY` 就绪行、`x-ezpdf-token` 校验、stdin-EOF 自退。
- [ ] 无状态、可并发（客户端一次只发一批，但可能重连后立刻再发）。
- [ ] 服务端部署形态（可选）：固定地址启动、令牌可复现（见 §7.4）。

参考实现：本目录 `app/`（`routers/ocr.py` 是端点、`services/pipeline.py` 是版面+识别流程；
`app/server_docker.py` 是服务端/容器入口）。
开发联调：`python server_test.py`（默认 `127.0.0.1:9055`）。
容器部署：`pyserver/Dockerfile` + `docker compose --profile cpu|gpu up -d`（见 `docker-compose.yml`）。

## 10. 兼容性规则

- **没有版本号**：加字段是兼容的（客户端忽略未知字段），删字段/改语义不兼容。
- `markdown` 允许为空串（例如 `image` 块或识别失败），客户端照存。
- `blocks` 允许为空数组（整页无文本），客户端会把该页标为已 OCR 完成、无覆盖框。
- `/health` 里没有 `max_batch_pages` 不算错：客户端按 4 页发批（老服务/最小实现可直接省略）。
- 服务端不要做 PDF/pt 相关换算，也不要尝试翻译。

## English

The **parse service** answers one question: *which text blocks are on this page image, and what
does each one say*. Translation is **not** part of this protocol — the ezpdf client talks to the
LLM itself (`src-tauri/src/translate.rs`), independently of the parse service.

- Two deployment shapes, one HTTP contract: **managed locally** (the client spawns
  `python -m app.main`, gets an ephemeral port from an `EZPDF_READY {"port":..,"pid":..}` stdout
  line, talks with a per-spawn `x-ezpdf-token`, and stops the service by closing its stdin) and
  **online service** (any address the user types, e.g. `http://127.0.0.1:9055` started with
  `python -m app.server_docker` for a deployed/containerised server, or `python server_test.py`
  for local development).
- **Auth**: a single shared bearer secret in the `x-ezpdf-token` header, checked on *every* route
  including `/health` (403 otherwise). The managed shape has the client generate a random
  per-spawn token and pass it via `EZPDF_TOKEN`. A deployed server resolves its own token —
  `EZPDF_TOKEN` env > `EZPDF_TOKEN_FILE` (`token.txt`, default `<pyserver>/token.txt`) > generate
  and persist — and prints it on every start, so the operator can paste it into
  Settings → OCR service → Service token. Container images run `python -m app.server_docker`,
  bind `EZPDF_HOST:EZPDF_PORT` (`0.0.0.0:9055`), bake the models into the image and keep
  `token.txt` on a volume so recreating the container does not invalidate the client's token.
- Endpoints: `GET /health` (`{"status":"ok","pid":..,"max_batch_pages":N}`), `POST /ocr/page`
  (single image, debug), `POST /ocr/pages` (as many pages as `/health` advertises).
- **Batch size is the server's call**: the client calls `/health` *before every* `/ocr/pages`
  request and collects that many pages; `max_batch_pages` missing/invalid → 4, above 32 → clamped
  to 32. Keep `/health` cheap (pure memory) and consistent with your real `pages` limit. Locally
  managed services ignore the field and keep the client's fixed 4 pages per batch.
- Request: `{"pages":[{"image_b64":"<base64 PNG, data: prefix allowed>"}]}`.
- Response: `{"pages":[{"width","height","elapsed","blocks":[{"label","score","bbox_px","markdown"}]}]}`
  — same order and length as the request, otherwise the client fails the whole batch.
- `label` is stored verbatim as the block type (PP-DocLayoutV3's 21 classes, see the table above);
  `markdown` becomes the block `content`, `bbox_px` is `[x1,y1,x2,y2]` **in the pixel space of the
  submitted image** (the client renders at scale 2.0 and divides by its own scale — never send PDF
  points or normalized coordinates); `score`/`width`/`height`/`elapsed` are ignored by the client.
- Limits: 64 MB body (413 before reading), 12k px per side / 40 MP per image, no client timeout on
  `/ocr/pages` (the first request loads models and may take minutes), 5 s timeout on `/health`.
- Errors: FastAPI-style `{"detail": "..."}` with 400/403/413/500; 3 consecutive failures pause that
  book's OCR branch in the client (translation keeps working).
- Compatibility: no version field, unknown response fields are ignored, missing-but-required fields
  break the client; empty `blocks` means "page has no text".
