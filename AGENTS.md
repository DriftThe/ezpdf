# AGENTS.md

ezpdf — a Tauri 2 desktop **real-time PDF translation reader**: a PDF and its translation side by side, OCR by an
in-repo Python service, translation by an LLM of your choice. Windows-first (NSIS), Linux deb/rpm. Frontend Vue 3 +
TypeScript + Vite, backend Rust. Both panes are self-drawn with `pdfjs-dist` — do not reintroduce pdf-vue3.

Design notes live in `PLAN.md` / `PLAN-LLM.md` / `PLAN-OCR.md` / `PLAN-pyserver.md` (gitignored) and
`docs/protocols.md` (committed).

## Commands

pnpm only — no lint or test scripts.

- `pnpm dev` — Vite only (browser; the frontend is inert without Tauri). Runs `scripts/copy-pdfjs-assets.mjs` first.
- `pnpm build` — typecheck + build (`vue-tsc --noEmit` + asset copy + `vite build`). **This is the typecheck command.**
- `pnpm tauri dev` — desktop dev; spawns Vite via `beforeDevCommand`.
- `pnpm tauri build` — release bundle (Rust LTO/strip/panic=abort). `beforeBuildCommand` runs `scripts/pack-runtime.mjs`
  (bundled Python + pyserver + prompts → `src-tauri/resources/`, cached; `EZPDF_PYTHON_PKG` overrides the download).
- `cargo check` / `cargo build` in `src-tauri/`; `cargo test` also regenerates the ts-rs bindings.
- `node scripts/sync-pi-models.mjs` — refresh the vendored pi-ai catalog (`--latest`, `--check`). No network at build/run time.
- `pyserver/.venv/Scripts/python pyserver/tests/test_layout_dedup.py` — the OCR merge/line-scan regression tests
  (numpy/PIL/cv2 only, no torch, no models).

## Layout

- `src/` — Vue frontend. Shell `components/shell/AppShell.vue`, entry `main.ts` (installs Pinia). Pinia stores
  `library`/`reader`/`parse`/`settings`; domain types are ts-rs bindings re-exported by `types/domain.ts`.
- `src-tauri/src/` — `lib.rs` (commands/DTOs), `parse.rs` (repo, bound JSON, OCR batches, file locks), `translate.rs`
  (LLM client + agent loop), `table.rs` (table markup → grid), `pyenv.rs` (path resolution, env install), `pyserver.rs`
  (service lifecycle).
- `pyserver/` — the FastAPI OCR service (`app/`), wire protocol in `PROTOCOL.md`. Layout/merge tuning is
  `app/config.py`; `app/services/` is `pipeline.py` (models), `boxes.py` (geometry + merge), `textlines.py` (figure
  text lines), `engine.py` (lazy singleton).
- `.github/workflows/` — `release.yml` (app releases), `pyserver-images.yml` (manual docker snapshots).

## Data model

- A repo is a directory with `.ezrepo`: pretty JSON `{folders, pdfs[]}`, each pdf `{id, name, bind, belong}`. `id` is a
  content-hash key minted at import and used for every frontend lookup; `bind` is the repo-relative bound-JSON path;
  `belong` is a logical folder label.
- `import_pdf` is best-effort over a file list and returns `ImportOutcome {imported, failed}`. It copies PDFs in as
  `<name>-<id>.pdf` plus a skeleton `<name>-<id>.json`, and rejects duplicate content by hash.
- Folders are **logical only** (nothing on disk). `move_pdf(belong=null)` moves a book to the root; `delete_folder`
  **cascades** over the folder's PDFs and bound JSONs (the confirm dialog says so). Mutations return the new `RepoTree`
  and the frontend applies it in place — there is no refresh command.
- The chosen repo is persisted (`repo` in config.json) and restored at launch; a failed restore clears it. 导入 PDF and
  新增文件夹 are disabled without a repo.
- **The bound JSON is the single source of truth for parse state**:

  ```
  { status: "Pending" | "Processing" | "Finished",
    pages: [{ index (1-based), finished, translated,
              blocks: [{ type, content, loc: [x1,y1,x2,y2] in pt, translation, grid? }] }] }
  ```

  - Import pre-fills `pages` 1..N (`finished`/`translated` false, no blocks). N comes from lopdf; a PDF it cannot read
    (e.g. encrypted) yields `pages: []` and the reason rides `ImportOutcome.warnings` into the import toast.
  - `translation: null` means "render `content` as-is" (the LLM returns null when it judges the text to already be in the
    target language), **not** hidden. `translated` marks the LLM pass done for that page; it serde-defaults to false so
    older JSONs backfill.
  - 清除解析状态 → `reset_pdf_state`: rebuilds the 1..=N skeleton, dropping all blocks and translations, returns
    `Pending`, and the frontend drops its cache, reloads the book and wakes the scheduler. `total` comes from the pdfjs
    `pageCount` for the open book and `0` otherwise, in which case the backend reuses the count already in the JSON.

## Frontend

- **i18n**: vue-i18n 11 (`legacy: false` + `globalInjection`), locales 简中/繁中/English in `locales/parts/`
  (`common`/`shell`/`reader`/`settings`/`pipeline`) assembled by `locales/index.ts`. Each part goes through
  `helpers.defineMessages()`, which makes **the zh-CN key set authoritative** — a key missing from another language is a
  compile error. Components use `useI18n()`; stores/composables import `t()` from `lib/i18n.ts`, which also owns
  `detectLocale()` (`navigator.language` → zh-CN / zh-TW for hant|tw|hk|mo / en), the `ezpdf.lang` localStorage mirror
  (read before mount so the first paint isn't in the wrong language), `setLocale()` and `defaultTargetLang()`.
  - 目标语言 is a preset dropdown (`lib/languages.ts`): value = the English name written into the prompt, label = native
    + English name (so it needs no catalog entry), plus a 自定义 entry that swaps in free text.
  - `ensureLoaded()` defaults `general.lang` and an empty `llm.targetLang` from the system language — keep those defaults
    outside the "config.json exists" branch or a first launch skips them.
  - Provider display labels are data in `piModels.ts` (Chinese for zh-CN/zh-TW, catalog English otherwise), not keys.
- **pdfjs**: import it only through `lib/pdfjs.ts` (worker setup + `openPdf()`, which injects the runtime resource paths —
  cMaps, standard fonts, wasm, ICC; without them CJK text and non-embedded standard fonts break). `usePdfDoc.ts` caches
  one `PDFDocumentProxy` per pdf id (LRU ≤2, the focus book pinned, never evicted); `ReaderArea.vue` owns the doc
  lifecycle and switch races. Page-list truth is pdfjs `numPages` (reader `pageCount` falls back to the bound-JSON
  length). PDF bytes never travel through invoke: the frontend fetches `convertFileSrc(pdfPath)` asset URLs.
- **Overlays** must divide `loc` by the *per-page* viewport (`reader.pageSizeFor(n)`, measured in background chunks after
  the doc is ready) — scan PDFs change size per page, and a page-1 assumption misaligns whole pages.
  `PdfPageCard`/`PdfPageCanvas` self-virtualize via IntersectionObserver.
- **Cover rule** (translation pane): the type set is `lib/blocks.ts` (`BLOCK_TYPE_OPTIONS`/`DEFAULT_TRANSLATED_TYPES`),
  persisted as `general.translateTypes` (设置→常规 checkboxes) and passed per invoke as `LlmConfig.translateTypes`.
  Checked types — plus `formula`, which is never a choice — get a white box showing `translation ?? content` (bypass
  stage: the original content). Everything else keeps the original pixels; its OCR text lives only in the JSON. An empty
  set means the built-in default on both sides, and a change only affects pages that are not translated yet (use
  清除解析状态 to re-run them).
  - Text covers auto-fit the box (binary search for the largest size that fills without overflowing: the `v-fit` directive
    + `composables/fitFont.ts`, `.cover-box` in `main.css`), and padding scales with box size. Newlines render as `<br>`
    (`renderRichText` → `textToHtml`, shared by covers and the hover card) — OCR `content` carries real `\n` that HTML
    would otherwise collapse.
  - **Tables** are parsed in Rust only: `table.rs` turns the markup stream (`<fcel>/<ecel>/<lcel>/<ucel>/<xcel>/<nl>`) into
    `block.grid` (cells with `colspan`, short tail rows right-padded, `<ucel>/<xcel>` blank; unparsable → no grid → no
    translate, no cover). The LLM gets `{"table": [[cell…], …]}` and must answer with a same-shaped 2-D array; a mismatch
    fails the page, and `A=null` ("already in the target language") stores the **source** matrix so the block is terminal.
    `reader/TableCover.vue` renders it (CSS grid with colspan, KaTeX per cell, the same `fitFont` search); if even the
    minimum font overflows, the cover goes transparent so the original pixels show.
  - Formula covers are vertically centered with KaTeX's display margins trimmed (`.cover-formula`), otherwise those 1em
    margins eat a small box and shrink the fit.
- **Hover preview**: block rects in the original pane are hover targets; a translated block pops a teleported rich-text
  card (`hover-preview`, follows the cursor, flips upward in the lower viewport half). Deliberately not a `title`
  attribute.
- **Zoom** is viewport-anchored: `ReaderPane.vue` records the midpoint page + in-page fraction before reflow and restores
  it after, with `overflow-anchor: none` so native anchoring doesn't fight it.
- **Window chrome** is self-drawn (`decorations: false`): a 34px `TitleBar.vue` with the `ezpdf` wordmark, the
  light/dark/system toggle and minimize/maximize/close (drag via `data-tauri-drag-region`, double-click maximizes).
  Theme is `general.theme`, applied by `composables/theme.ts` through `:root[data-theme]`; `index.html` pre-applies a
  localStorage mirror to avoid a first-paint flash.
- **Settings**: `SettingsPage.vue` is an absolute overlay in `AppShell` (z-10) over sidebar and reader with `v-show`, so
  nothing underneath unmounts; the toolbar (z-20) stays visible. Sections are registered in
  `SECTIONS: Record<SettingsSection, …>` — extending the union without registering is a build error. Section components
  in `settings/sections/` are pure markup (no scoped CSS, root element `set-pane`); state lives in the settings store.
  Every change is written **2 s after the last one** (debounced deep watcher, armed only after a successful load,
  cancelled by any explicit write; failures just `console.warn`) — no per-pane save buttons, no success toast.
  - Save = `save_settings`, which writes pretty JSON to `config.json` — repo root in dev, **beside the exe on Windows in
    production**, `~/.ezpdf/config.json` elsewhere — and closes. Writes are whole-file and gated by a `loaded` flag;
    writing before a successful load is refused (`settings.saveNotLoaded`) because it would wipe the user's config.
    Malformed sections are ignored field by field.
  - 常规 holds 启动时自动唤醒 OCR 服务 (`general.autoLaunch`) and 启动时自动续跑 (`general.resumeOnStart`), both default off:
    `AppShell` sets `parse.paused = true` unless both are on, then `autoStartIfEnabled()` probes the environment and
    starts the service only when Python/deps/models are all present (log line instead of a crash loop). 检查更新
    (`check_update` → GitHub releases of `DriftThe/ezpdf`) is silent at launch and toasts errors when run manually.
  - Shared form classes `.set-pane/.set-title/.set-field/.set-field-row/.set-check/.set-hint/.set-button` are global in
    `main.css` (`set-` prefix; don't combine `.set-button` with `.btn`).
  - **LLM pane**: a provider dropdown from `lib/piModels.ts` (usable presets first, then 自定义) fills the Base URL, locks
    协议 and preselects a model; the model field is a datalist fed by the vendored catalog. 协议 (Chat Completions /
    Messages / Responses) is read-only for presets and a real picker only for 自定义. `llm.provider` + `llm.preset` (a
    compat snapshot: `api`/`thinkingOffKind`/`thinkingOffValue`/`maxTokensField`/`reasoning`/`extraHeaders`) are persisted
    so a cold start needs no catalog load; the catalog chunk is lazy-imported. Providers whose protocol is unsupported
    are not listed (a saved one shows disabled) — see `docs/protocols.md`. 验证 = `verify_llm`, 获取在线列表 =
    `fetch_llm_models`.

## Backend (Rust)

- `pyenv.rs` owns all pyserver path resolution — the single dev/prod branch point (`cfg(dev)` plus
  `EZPDF_TOOLKIT_ROOT`/`EZPDF_PYTHON_EXE`/`EZPDF_MODELS_DIR` overrides), resolved once in the `setup` hook into global
  `PyPaths` state. On Linux the install directory is read-only, so the bundled interpreter (`base_python`, what
  `probe_blocking` reports before a venv exists) is split from the service interpreter (`~/.ezpdf/venv/bin/python3`,
  created on first 一键安装服务); Windows installs into the writable per-user directory, dev uses `pyserver/.venv`.
  Models live outside the install directory in production (`~/.ezpdf/models`) so upgrades don't re-download 1.9 GB.
  `probe_blocking()` runs `pyserver/bootstrap.py` (stdlib-only probe, one-line JSON contract on stdout).
- `install_env(mode, use_mirror)` = 一键安装服务. It no-ops when deps are complete and the torch build satisfies the pick
  (a CUDA env answers a CPU request, never the reverse), aborts GPU mode when `nvidia-smi` is missing, and streams pip to
  `ocr://log` plus staged progress (`ocr://install {phase, percent}`). The mirror switch swaps in TUNA pypi + SJTU
  pytorch-wheels (`pip --retries 6 --timeout 60`). Off Windows the CPU pick takes the `/cpu` wheel index because PyPI's
  Linux torch silently bundles CUDA (5.1 GB with nvidia-*/triton vs 1.4 GB) and its version carries no `+cu` tag — which
  is also why `bootstrap.py` falls back to an nvidia-*/triton presence check. `download_models(use_mirror)` installs
  `requirements-download.txt` (huggingface_hub, on demand) and runs `python -m app.fetch`.
- `pyserver.rs` is the service lifecycle: `supervise` spawns `python -m app.main`, waits for the `EZPDF_READY {port,pid}`
  line (60 s), then probes `/health` with the per-spawn session token; crashes restart with exponential backoff and 3
  consecutive failures are terminal (`failed`). Stopping drops the child's stdin, which the Python stdin-EOF watchdog
  turns into a clean exit. A second, child-less mode (`set_remote`/`disconnect`) points `ocr_target()` at a user-configured
  remote service (`ocr.mode`/`ocr.url`/`ocr.token` in 设置→OCR 服务; `ocr_start_remote` probes `/health` with the token then
  registers it, `ocr_health` is the read-only 测试 button). That token is the server-side shared secret (`EZPDF_TOKEN` env >
  `EZPDF_TOKEN_FILE` > generated and persisted; enforced on every route including `/health`). A compatible server must
  implement `pyserver/PROTOCOL.md`; `pyserver/server_test.py` fakes one locally on 9055 (`--max-batch`, `--token`).
- IPC: `invoke("cmd", {…})` — args are camelCase in JS and snake_case in Rust, and serde structs serialize **camelCase**.
  Events: `ocr://status` (a `ServiceStatus` string), `ocr://log`, `ocr://install` (`InstallProgress`), `llm://log`. The
  current command list lives in `generate_handler![]` (`lib.rs`).
- `src-tauri/bindings/` is ts-rs output (regenerated by `cargo test`) and `src-tauri/gen/` is generated: never hand-edit
  either, and delete stale binding files when a type disappears. `capabilities/default.json` is the permission allowlist —
  new plugin APIs don't work until their permissions are added there.
- **Scheduling loop** (frontend-driven, `src/stores/parse.ts`): one `tick()` = pick a book (focused first; per-book
  `load_pdf` reads the JSON flags) → ring-collect ≤batch unfinished pages (the focused book starts at `reader.currentPage`)
  → offscreen render (`lib/pageCapture.ts`, PNG b64, scale 2.0) → `parse_pdf` (OCR only) → fire-and-forget `translate_pdf`
  for those pages, so the next OCR batch starts immediately. Batch size is 4 locally, or whatever the remote advertises
  as `max_batch_pages` (re-read with a `/health` handshake before every online batch). Results patch
  `library.pdfs[id].pages` for the focused book; for background books Rust's atomic tmp+rename write is the truth and the
  next round re-reads it. The chain self-continues via `queueMicrotask`; with nothing to do it goes `standing` and waits
  for a wake (open book, import, repo load, service connect, unpause, prefill, drained chain). Per book, a translation
  retry outranks new OCR (a `finished && !translated` page with an idle chain is translated first), and table backfill
  rides the same chain (`load_pdf` persists a missing `block.grid`; `needsWork = needsTranslation || needsTableBackfill`;
  Rust then sends only that page's tables). OCR and translation keep separate 3-strike budgets per book, so one failing
  kind suspends only itself, and only external wakes reset them. `parse.paused` (toolbar 暂停翻译/启动翻译) gates
  everything: `isRunnable()` = `canOcr() || canTranslate()` (translation needs no OCR service) and `pickBook` only offers
  OCR targets while the service is connected. Books with empty pages are skipped until opened, where the geometry watch
  backfills via `prefill_pages`. In plain browser mode the whole loop is silently inert.
- `translate.rs` is the chat client for the three protocols plus the smart-context agent loop. Per page it packs the
  translated-type blocks (non-empty content, `translation == null`), allows ONE `need_context` round (context = up to 3
  boundary blocks of the neighbouring page), validates the answer 1:1 by index (`A=null` = same language) and binds a
  valid `external`/`external_index` back onto the neighbour page's block (which is then skipped there). Thinking is
  disabled in this precedence: an explicit `llm.thinkingOff` strategy → verified endpoint rules (zen:
  `reasoning.enabled=false` + `reasoning_effort=none`; SiliconFlow: `enable_thinking=false` + `thinking.type=disabled`)
  → the pi-ai preset shape (`thinkingOffKind`/`thinkingOffValue`); `modelReasoning: false` sends no thinking params at all.
  `Protocol::from_api` maps `api` to Chat/Messages/Responses (`""` = Chat, anything else is a hard error) and everything
  protocol-specific stays in three places: `chat_url`/`models_url`, `build_body` (top-level `system` for Messages vs
  `instructions` + typed `input[]` for Responses; `max_tokens` vs `max_output_tokens` vs the preset's field) and
  `completion_of` (choices/content, thinking blocks, output items, reasoning items). Auth follows the protocol
  (`x-api-key` + `anthropic-version` for Messages, bearer otherwise) and `extraHeaders` are applied on top.
  `translate_pdf` runs stride-2 phases (0,2,4… then 1,3,5…, so neighbouring pages never translate concurrently), does all
  network work outside the per-book file lock (`FILE_LOCKS` in `parse.rs`) and merges under it.
  `LlmConfig.translateEnabled` off selects `apply_bypass`: each block's `content` is written into `translation` and the
  page marked translated — a terminal, no-network path (the frontend sends a payload even with no keys configured, which
  is what lets a bare install finish OCR text).
  - System prompt = `<prompt dir>/intelli_context.md` for smart mode, `standard_translate.md` when `smartContext` is off
    (no context protocol at all). `{{target_language}}` is templated at load (appended if the placeholder is missing);
    `EZPDF_SYSTEM_PROMPT_DIR` overrides the directory; production is `resource_dir()/system_prompt`, dev the repo copy.
  - 验证 (`verify_llm`) probes with a short prompt. Candidates are per protocol: Chat keeps four concurrent shapes;
    Messages/Responses probe `auto`, then `none`/`reasoning`, sequentially, stopping at the first non-thinking reply. A
    non-reasoning preset skips the probe. The report's `thinkingOn` flag is what triggers the "cannot disable thinking"
    warning.
  - LLM config arrives per invoke from the settings store, dev-filled from the gitignored root `auth.cfg` (Vite raw
    import); `pack-runtime.mjs` also greps `dist/` for the key and aborts the build if it leaks.
  - reqwest needs `rustls-tls` + `system-proxy` (both off by default; the system proxy is how region-restricted endpoints
    are reached, and `ProxyOverride` keeps the local pyserver direct).
  - **Prompt-protocol changes must be mirrored in `translate.rs`'s parsing.** Rust and pyserver logs/errors are English,
    but the prompts sent to the model stay Chinese (`PROBE_PROMPT`, `CORRECTION`, `NO_MORE_CONTEXT`, `load_system_prompt`'s
    target-language fallback) — they are model input, not UI text, so don't translate them during i18n work.

## pyserver

- A ported Wise-Paddle OCR host: single-instance pipeline (`PP-DocLayoutV3` + `PaddleOCR-VL-1.6` in
  `app/services/pipeline.py`, lazy-loaded behind a `threading.Lock`). The upstream concurrency containers (Pool/
  Scheduler/vouchers) were intentionally removed — don't reintroduce them.
- `models/` (1.9 GB, gitignored) is not a hard prerequisite: `app/fetch.py` (`python -m app.fetch`, invoked by
  `ocr_download_models`) snapshot-downloads what is missing, resumable. Completeness = config.json +
  preprocessor_config.json + any weight file, defined once in `app/model_contract.py` and used by both `bootstrap.py` and
  `fetch.py`; both honour `EZPDF_MODELS_DIR`.
- Requests are bounded before they can hurt: 64 MB body (Content-Length check in `main.py` before reading), 12k px side /
  40 MP per image, decoding on the threadpool, `DecompressionBombError` caught.
- `requirements.txt` is deliberately minimal, but four pins are load-path requirements rather than preferences:
  `protobuf` (SentencePieceExtractor — without it the VL tokenizer is misread as a tiktoken file), `sentencepiece` +
  `tiktoken` (the tokenizer), `opencv-contrib-python` (PP-DocLayoutV3 post-processing). The VL tokenizer loads without
  `trust_remote_code` thanks to the rope patch in `pipeline.py` (module-level, must stay). `requirements-download.txt` is
  just huggingface_hub.
- Layout detection is multi-pass and add-only (`LayoutDetector.detect`): the 0.5 main pass, then a sharpened 0.45 pass
  merging text-family candidates, then sharpened top/bottom half-page tiles at 0.38 (headers/footers allowed) — that is
  what recovers small or widely spaced text that pdfjs rendering loses. New candidates must clear IoU ≤0.3 and
  containment ≤0.6 against kept boxes (`BoxFilter.min_score` is 0.35). Model forwards are chunked to ≤4 images
  (batch 8 hits a slow kernel). Boxes carry the pass they came from (`origin`).
- **Every pass reports in the caller's page pixels, and that is the whole contract**: the two full-image passes hand the
  detector the possibly-downscaled image but ask for `target_sizes` of the *page* back, and the tile pass crops the page
  itself before downscaling the crop, taking the crop rect, `target_sizes` and the box shift from one `boxes.split_tiles`
  rect. Mixing the two spaces is what put every tile box up and to the left of its text — an A4 page is 1684 px tall at
  the render scale 2.0, so it is shrunk to `LAYOUT_MAX_LONG_SIDE` (0.95×), and a pass that shifted *downscaled*
  coordinates into the page's cost up to 42 pt of drift, growing with the distance from the origin. Two other
  coordinate rules ride on the same idea: `_forward_one` clamps to the *target* size, not the downscaled input (or the
  last ~30 pt of every page is shaved off, losing a footer), and `_figure_regions` lifts a figure's inner boxes by the
  crop's own origin.
- `app/services/boxes.py` is the merge (no torch, so it is testable on its own): one priority-greedy pass, ordered
  structured (table/formula) → figure (image/chart) → text family, then by earlier pass, larger box, higher score. A
  candidate is dropped when it re-detects a kept box (IoU > 0.5, or covered by it — 0.35 of its area, 0.25 for text
  inside a table/formula), and it is **unioned into its survivor** so no pixel leaves the crop. Figures never absorb
  the text inside them. Boxes are first **pulled tight around their ink** (`BOX_TRIM_TO_INK`): the detector's boxes
  carry a margin (median 5-9 px at scale 2.0, up to 190 px on a spurious one) which is what makes two unrelated
  blocks overlap and their covers repaint each other. What is left around the ink is `BOX_TRIM_MARGIN_PX` — the
  detector's own 3 pt, not zero, or the covers come out cramped and the fit has to shrink the text to fit them.
  After OCR the same module drops boxes whose reading is
  punctuation, a stray glyph or the VL's `[Unlabeled]` placeholder, and merges near-duplicates — overlap *and*
  near-equal text (containment, substring, fuzzy words: OCR truncates word tails), most complete reading wins.
  Geometry alone cannot tell "the same line detected twice" from two boxes that merely touch; the text can.
- Text-dense figures are split into their inner text lines (`OCRPipeline._figure_regions`), so a flow chart's labels
  get translated instead of being one opaque `image` block. Both signals must agree: the figure's own VL OCR must read
  like a body of text (≥150 chars, ≥3 lines, ≥60 % alphanumeric) *and* `app/services/textlines.py` must find labelled
  text in the crop (a morphological line scan — PP-DocLayoutV3 answers "this is a figure" and finds one or two boxes in
  any crop presentation, so the layout model is not usable inside a figure). The `image` block stays, covers land on
  the lines, so a wrongly accepted figure costs covers and never content. Every decision logs one `[figure]` line.
- **Every number for the above lives in `app/config.py`** (`EZPDF_OCR_*` env overrides, defaults are the calibrated
  ones); `engine.py` passes `ocr_tuning()` into `OCRPipeline`, whose constructor has no defaults of its own. The
  measured baselines behind the thresholds (duplicate coverage, line ink vs frame ink, box margins) are in the config
  comments, and turning the merge up is a matter of lowering `DEDUP_CONTAINMENT` / `DEDUP_CONTAINMENT_STRUCTURED`.
  `pyserver/tests/test_layout_dedup.py` locks the behaviour with real duplicate pairs from a page that came out
  wrong — run `python pyserver/tests/test_layout_dedup.py` (42 cases, numpy/PIL/cv2 only, ~20 ms); it also pins the
  tile geometry above, the one bug a page-level smoke test showed as "the box is on the wrong lines".
- Two entries: `app/main.py` (managed: ephemeral port, `EZPDF_READY` line, stdin-EOF watchdog, token injected by Rust) and
  `app/server_docker.py` (deployed/container: binds `EZPDF_HOST:EZPDF_PORT`, default `0.0.0.0:9055`, **no** watchdog since
  a container's stdin is `/dev/null`, resolves its own token and prints it on every start).
- Docker: `pyserver/Dockerfile` (one file, `--build-arg VARIANT=cpu|gpu`) plus `docker-compose.yml` (`--profile cpu|gpu
  up -d --build`; models baked in via `COPY models`; `token.txt` on the `./data` volume; host bind 127.0.0.1:9055; CUDA
  deps come from pip's nvidia-* wheels, so no nvidia/cuda base image). Images are ~6 GB (CPU) and ~12.5 GB (GPU) reported
  and are meant to be faithful offline packages: **don't trim packages for size** (torch's CUDA build hard-links its
  nvidia-* deps even for single-GPU eager inference) and keep `build-essential` (the CUDA path JIT-compiles a triton
  kernel). `BASE_IMAGE`/`APT_MIRROR`/`PIP_INDEX`/`TORCH_INDEX` exist for the slow or blocked indexes in mainland China
  (known-good mirrors: `docker.m.daocloud.io`, `mirrors.tuna.tsinghua.edu.cn`, `mirror.sjtu.edu.cn/pytorch-wheels/{cpu,cu132}`).
  **Never run `docker builder prune` / `docker system prune` on the dev machine** — the BuildKit cache holds the
  torch/nvidia wheels and clearing it forces a multi-GB re-download, often on metered data.

## Gotchas

- **Port 1420 is strict** (`vite.config.ts`): Vite fails if it is taken and `devUrl` points at it. HMR moves to 1421 when
  `TAURI_DEV_HOST` is set.
- `src/lib/piModels.generated.ts` is generated — never hand-edit; refresh with `sync-pi-models.mjs`. It vendors pi-ai's
  data module plus a ported copy of its non-exported `detectCompat`, so re-diff the port whenever the pi-ai version moves.
- `public/pdfjs/` is generated by `copy-pdfjs-assets.mjs` at the start of `pnpm dev`/`pnpm build` (gitignored, never
  hand-edit). Running `vite` directly skips the copy and CJK text then renders misplaced or missing (404s, no crash).
- pdfjs v6 API: destroy through `doc.loadingTask.destroy()` (`PDFDocumentProxy.destroy()` is gone) and `page.render()`
  needs `canvas`, not just the context.
- Asset protocol: the static scope is empty on purpose; `gettree_from_config` grants the repo root at runtime
  (`allow_directory(root, true)`, least privilege). A failed render ("PDF 加载失败") usually means a missing scope grant.
  It needs the `protocol-asset` cargo feature on `tauri`.
- Vite doesn't watch `src-tauri/` — restart `pnpm tauri dev` after Rust changes.
- The Rust lib is `ezpdf_lib` (the `_lib` suffix avoids a Windows bin/lib name clash) — keep the suffix.
- TypeScript is strict with `noUnusedLocals`/`noUnusedParameters`; unused imports fail `pnpm build`.
- No CSP is set (`"csp": null`); if one is added, pdfjs needs `worker-src` for its module worker. The >500 kB chunk
  warning is expected (pdfjs-dist).
- `testfiles/` is gitignored and only for manual-test PDFs; the reader renders real repo data through IPC and there is no
  mock data anywhere.
- Repo entries not named `<name>-<id>.pdf` (hand-written leftovers) fail to load by design — re-import them.
- `parse.ts` store: `serviceStatus` (including `failed`) comes from `ocr://status`, `envReport`/`envLogs` from
  `ocr_env_report`/`ocr://log`, `installProgress` from `ocr://install`, `llmLogs` from `llm://log` (Rust `translate::log`
  plus frontend `[ui]` scheduler lines, shown in the LLM pane's `log-box`). The OCR pane's five lights derive from these —
  don't add separate probe commands. `installService(mode, useMirror)` chains `ocr_install_env` → `ocr_download_models`
  only when models are missing, guarded by `installing`.
- `listen()` subscriptions in `parse.ts` reject silently outside the Tauri runtime — keep the `.catch(() => undefined)`.

## Release

- Version source of truth is `"version"` in `src-tauri/tauri.conf.json` (installer name, `app.package_info()`,
  `getVersion()`, the update check). Mirroring `package.json` is optional; `Cargo.toml`'s version is not used.
- `pnpm tauri build` → typecheck → vite build → `pack-runtime.mjs` (cached; delete `src-tauri/resources/.runtime-stamp`
  or pass `--force` after changing the Python version) → Rust release → NSIS. Windows artifact:
  `src-tauri/target/release/bundle/nsis/ezpdf_<version>_x64-setup.exe`. On Linux the same command emits deb/rpm (`--bundles
  deb,rpm` overrides `bundle.targets`; needs libwebkit2gtk-4.1-dev, libgtk-3-dev, librsvg2-dev, patchelf,
  libayatana-appindicator3-dev).
- Pushing tag `v<version>` runs `release.yml`: a `check-version` gate, then windows (NSIS) and ubuntu (deb/rpm) via
  `tauri-action` into a **draft** Release, then a `notes` job that applies `docs/release-notes/v<version>.md` as the body —
  write release notes in that file; the workflow's `releaseBody` is only a fallback. Drafts and prereleases are invisible
  to 检查更新, so smoke test and then `gh release edit v<version> --draft=false`.
- Installers are unsigned (SmartScreen warns on first run; a code-signing certificate would remove that).
  `pack-runtime.mjs` fails the build if `dist/` contains the `auth.cfg` API key (CI has no `auth.cfg`).
- Docker snapshots are published separately: dispatch the **Pyserver images** workflow by hand (inputs: app tag,
  variants) to build the cpu/gpu images, smoke-test them in containers, split the tars (~2.1 GB / ~4.5 GB) into <2 GiB
  parts — the single-attachment limit — and upload them to a `pyserver-<app tag>` Release. It is deliberately not tied to
  the app release (20-30 min per variant, and it would bloat the app release): draft first, then
  `gh release edit pyserver-v<version> --draft=false`. The READMEs document merging the parts (`cat` / `copy /b`) and
  `docker load`. If the tag doesn't exist yet the workflow creates it at the app tag's commit.
- Linux is verified under WSL2 (Ubuntu 26.04 + WSLg): `cargo test`, `--bundles deb,rpm`, install and launch, production
  path resolution (resource root `/usr/lib/ezpdf`, read-only `base_python`, service venv `~/.ezpdf/venv/bin/python3`,
  settings in `~/.ezpdf/config.json`), venv creation from the bundled interpreter with torch CPU, and pyserver running
  from a read-only install directory. Unverified: GUI-level JS smoke tests (WebKitGTK has no CDP; that would need
  tauri-driver + webkit2gtk-driver), the 一键安装服务 button itself, the 1.9 GB model download and real OCR on Linux, and
  CI's ubuntu-22.04. AppImage is not shipped: its read-only random mount path invalidates the bundled-interpreter venv on
  every launch (`pyvenv.cfg`), which would need the interpreter copied to `~/.ezpdf/python` first. The bilingual READMEs
  (`README.md` zh + `README.en.md` en, cross-linked) carry a roadmap section for code signing (SignPath Foundation OSS)
  and AppImage.

## User Harness

- After smoke testing, kill whatever holds port 1420 so the user can start the app themselves.
