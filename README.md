# ezpdf

[![Release](https://img.shields.io/github/v/release/DriftThe/ezpdf?color=4c8bf5)](https://github.com/DriftThe/ezpdf/releases)
[![License](https://img.shields.io/github/license/DriftThe/ezpdf)](LICENSE)

**实时 PDF 翻译阅读器**：左边原文、右边译文逐块覆盖，读到哪翻到哪。

用 OCR 把 PDF 每页切成带坐标的内容块，再用大模型逐块翻译，译文以白底覆盖框精确盖回原文位置。术语、公式、页脚都会被覆盖；表格、图片、图表保留原始像素，不做破坏性重绘。

![ezpdf 界面](docs/screenshot.png)

## 特性

- **双栏对照**：原文与译文左右并排、页码同步，悬停原文块可在译文侧预览对应译文。
- **增量实时**：启动后按页排队处理（4 页一批），先 OCR 再翻译，已完成的页即时可见，不用等整本书。
- **公式不糊**：`formula` 块用 KaTeX 渲染，覆盖框内自动适配字号。
- **智能上下文**：翻译时允许一次「回捞邻页边界块」的动作，保证跨页句子的术语与人称一致。
- **本地 OCR 服务**：内嵌 Python 服务（版面分析 + OCR），应用内「一键安装服务」，支持 CPU / GPU（CUDA）。
- **供应商预设**：内置 pi-ai 模型目录（31 家供应商 / 969 个模型），选供应商即自动填端点、协议与参数形态；支持任意 OpenAI 兼容端点。
- **书籍仓库**：一个普通文件夹就是仓库，放 PDF 与 `.ezrepo` 索引；支持文件夹归类、移动、删除、多选导入。
- **自绘界面**：无系统标题栏，浅色 / 深色 / 跟随系统主题。

## 工作原理

```
PDF ──pdfjs──▶ 页面渲染（自绘虚拟滚动）
  │
  └─▶ 取页图 ──▶ 本地 OCR 服务 ──▶ 内容块 + 坐标（bound JSON）
                                   │
                                   └──▶ LLM 翻译 ──▶ 译文覆盖框
```

- **前端**：Vue 3 + TypeScript + Vite，Pinia 管理状态，两栏都由 `pdfjs-dist` 自绘（未使用 pdf-vue3 之类的查看器）。
- **后端**：Rust（Tauri 2）。负责仓库索引、文件锁、翻译调度与 OpenAI 兼容客户端；PDF 二进制不走 IPC，前端用 asset 协议直接取。
- **OCR 服务**：仓内 `pyserver/`（Python + FastAPI），版面分析用 **PP-DocLayoutV3**、识别用 **PaddleOCR-VL-1.6**，单实例流水线，模型在首次使用时下载。
- **解析状态**：每本书对应一份 JSON（`{status, pages[{index, finished, translated, blocks[]}]}`），是用户可编辑的**唯一真相源**。

## 下载安装

到 [Releases](https://github.com/DriftThe/ezpdf/releases) 下载：

| 平台 | 产物 |
| --- | --- |
| Windows | `ezpdf_<version>_x64-setup.exe`（NSIS 安装包） |
| Debian / Ubuntu | `ezpdf_<version>_amd64.deb` |
| Fedora / RHEL | `ezpdf-<version>-1.x86_64.rpm` |

> 安装包尚未代码签名，Windows 首次运行会有 SmartScreen 提示；Linux 暂不提供 AppImage。

## 启动方法

### 直接用

安装后打开 `ezpdf`。首次启动的引导见下面的「初次使用」。

### 从源码运行

前置：Node 20+ 与 **pnpm**、Rust stable、以及对应平台的 Tauri 2 依赖（Windows 需 WebView2；Linux 需 `libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev patchelf libayatana-appindicator3-dev`）。

```bash
pnpm install
pnpm tauri dev      # 桌面应用（会自动拉起 Vite）
```

常用命令：

| 命令 | 作用 |
| --- | --- |
| `pnpm tauri dev` | 桌面应用开发模式（Rust 改动需重启） |
| `pnpm dev` | 只跑前端（浏览器里打开；无 Tauri 环境时 OCR/翻译静默不工作） |
| `pnpm build` | 类型检查（`vue-tsc --noEmit`）+ 前端构建，这也是本仓库的类型检查命令 |
| `pnpm tauri build` | 出安装包（Windows NSIS；Linux 加 `--bundles deb,rpm`） |
| `cd src-tauri && cargo check` / `cargo test` | Rust 检查与测试（`cargo test` 会顺带重生成 ts-rs bindings） |
| `node scripts/sync-pi-models.mjs` | 刷新内置的 pi-ai 模型目录（`--latest` 跟最新版） |

## 初次使用

1. **选仓库**：启动后点左侧「选择」挑一个文件夹当仓库（任意空文件夹即可）。仓库路径会被记住，下次自动打开。
2. **导入 PDF**：「导入 PDF」选择文件（可多选）。PDF 会被复制进仓库，并生成同名的骨架 JSON。
3. **配置翻译模型**：设置 → **LLM**。选「供应商」→ 选「预设模型」（或手填模型名）→ 填 **API Key** → 点「验证」确认连通。默认走 OpenAI 兼容协议。
4. **安装 OCR 服务**：设置 → **OCR** → 「一键安装服务」。会依次安装 Python 依赖（含 torch）并下载模型（约 1.9GB，可切国内镜像）。五个状态灯全绿即就绪；有 NVIDIA 显卡可选 CUDA。
5. **开始翻译**：工具栏默认是「启动翻译」（出于省电考虑，启动即暂停）。点一下开始；若希望开机自动干活，到 设置 → 常规 打开「启动时自动唤醒 OCR 服务」与「启动时自动续跑」。
6. **阅读**：左侧点选一本书进入双栏阅读。顶部可切「原译 / 译原 / 原文 / 译文」、缩放与页码；底部是整本书的处理进度。

### 数据放在哪里

| 内容 | 位置 |
| --- | --- |
| 应用设置（含 API Key） | Windows：安装目录（exe 旁）`config.json`；Linux / macOS：`~/.ezpdf/config.json` |
| PDF 与解析 JSON | 你选的仓库目录内 |
| 随包 Python 环境 / 模型 | Linux：`~/.ezpdf/venv`、`~/.ezpdf/models`；Windows：安装目录下的 `python/`、`~/.ezpdf/models` |

> API Key 以明文保存在 `config.json` 中，请自行注意文件权限。

## 贡献方法

欢迎 Issue 与 Pull Request。

1. Fork 本仓库，从 `main` 建特性分支。
2. 提交前请确保通过：
   ```bash
   pnpm build                      # 类型检查 + 前端构建
   cd src-tauri && cargo test      # Rust 测试 + 重生成 bindings
   ```
   本仓库暂无 lint / 单元测试脚本，上述两条既是门禁也是类型检查命令。
3. Commit message 用 [Conventional Commits](https://www.conventionalcommits.org/)（`feat:` / `fix:` / `docs:` …），正文说明**为什么**改。
4. 提交 PR 时描述复现步骤与验证方式（截图更好）。

几点约定：

- **不要手改生成物**：`src/lib/piModels.generated.ts`、`src-tauri/bindings/`、`src-tauri/gen/`。
- **不要提交密钥**：`auth.cfg`、`config.json` 已在 `.gitignore` 中，构建脚本也会检查产物是否泄漏 Key。
- 新增 Tauri 插件能力时，记得在 `src-tauri/capabilities/default.json` 中加权限。
- 改动提示词协议时，需同步 `src-tauri/src/translate.rs` 的解析逻辑。
- 仓库架构与各模块设计见 `AGENTS.md`。

## 鸣谢

- **百度 PaddlePaddle 团队**：本项目的 OCR 全部构建在其开源成果之上 —— 版面分析用 **PP-DocLayoutV3**，文本识别用 **PaddleOCR-VL-1.6**。没有这两个模型，就没有 ezpdf 的内容块与坐标，也就没有译文覆盖。同时感谢 **PaddleOCR / PaddleX** 项目及其社区。
- **Mozilla pdf.js**（`pdfjs-dist`）：两侧页面的渲染基础。
- **Tauri**、**Vue 3**、**Vite**、**Pinia**：应用外壳与界面。
- **KaTeX**：公式渲染。
- **pi-ai**（Mario Zechner）：内置的模型/供应商目录数据。
- **python-build-standalone**（Gregory Szorc / Astral）：随包分发的可重定位 Python 运行时。
- 以及所有上游依赖的维护者。

> 代码以 Apache-2.0 许可发布（见 [LICENSE](LICENSE)）；随包下载的模型版权归各自作者所有，使用请遵守其许可协议。
